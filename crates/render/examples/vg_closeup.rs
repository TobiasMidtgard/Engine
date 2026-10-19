//! Virtual geometry close-ups: a dry-stone wall of procedural split-face, rough-hewn and slab
//! stones ([`WallStone`]) whose chisel and pick marks, fractures and laminations are real
//! displaced geometry ([`Displacement`], tessellated to 3 mm edges when the clusters are built,
//! cached on disk), at 30 cm, 2 m and 30 m, beside the same wall built from chamfered boxes the
//! way the showcase's walls are today ("before"), the cluster debug views, and a field of 64
//! walls to show what distant walls cost.
//!
//! ```text
//! cargo run -p engine_render --example vg_closeup -- [output dir] [--frames N] [--only NAME]
//! ```
//!
//! Needs a GPU or lavapipe. Prints triangles, clusters and frame times for every shot, and the
//! build (or cache load) times. Run it twice to see the disk cache at work; set
//! `ENGINE_VG_CACHE` to put the cache somewhere else.

use std::path::PathBuf;
use std::time::{Duration, Instant};

use engine_assets::{Assets, Handle};
use engine_core::prelude::*;
use engine_render::prelude::*;
use engine_render::{DebugView, RenderDisabled, RenderStats, Renderer, capture_now};

/// Small deterministic random numbers for the wall layout.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> f32 {
        self.0 = self
            .0
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        ((self.0 >> 40) as f32) / (1u64 << 24) as f32
    }

    fn range(&mut self, lo: f32, hi: f32) -> f32 {
        lo + (hi - lo) * self.next()
    }
}

/// Length of the wall, m.
const LENGTH: f32 = 4.0;
/// Width at the foot and under the coping, height of the body, m.
const FOOT: f32 = 0.7;
const TOP: f32 = 0.45;
const BODY: f32 = 0.95;

fn half_width(y: f32) -> f32 {
    let t = (y / BODY).clamp(0.0, 1.0);
    (FOOT + (TOP - FOOT) * t) * 0.5
}

/// One stone of the wall: where, how big, what kind.
struct Slot {
    center: Vec3,
    rotation: Quat,
    size: Vec3,
    kind: StoneKind,
}

/// A dry-stone wall along x, both faces: courses of stones getting smaller toward the top, the
/// faces battered in, through-stones, and a coping of slabs set on edge.
fn wall_layout(seed: u64) -> Vec<Slot> {
    let mut rng = Rng(seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1);
    let mut slots = Vec::new();
    let batter = (FOOT - TOP) * 0.5 / BODY;
    for side in [-1.0f32, 1.0] {
        let n = Vec3::new(0.0, batter, side).normalize();
        let up = Vec3::new(0.0, 1.0, -side * batter).normalize();
        let mut y = 0.0;
        while y < BODY - 0.05 {
            let t = y / BODY;
            let mut h = rng.range(0.17, 0.27) * (1.0 - 0.4 * t);
            if BODY - (y + h) < 0.08 {
                h = BODY - y;
            }
            let mut x = -LENGTH * 0.5 - rng.range(0.0, 0.3);
            while x < LENGTH * 0.5 {
                let len = if rng.next() < 0.25 {
                    h * rng.range(1.0, 1.5)
                } else {
                    rng.range(0.3, 0.62) * (1.0 - 0.35 * t)
                };
                let (a, b) = (x.max(-LENGTH * 0.5), (x + len).min(LENGTH * 0.5));
                x += len;
                if b - a < 0.08 {
                    continue;
                }
                let gap = rng.range(0.004, 0.014);
                let sh = h * rng.range(0.93, 0.99);
                let yc = y + h * 0.5 + (h - sh) * rng.range(-0.5, 0.5);
                let depth = rng.range(0.22, 0.3);
                let out = rng.range(-0.02, 0.025);
                let surface = Vec3::new((a + b) * 0.5, yc, side * half_width(yc)) + n * out;
                let center = surface - n * depth * 0.5;
                let tilt = Quat::from_axis_angle(n, rng.range(-0.1, 0.1))
                    * Quat::from_axis_angle(up, rng.range(-0.06, 0.06));
                // The stone's z axis points out of the face.
                let frame = Quat::from_mat3(&Mat3::from_cols(up.cross(n).normalize(), up, n));
                let kind = if rng.next() < 0.55 {
                    StoneKind::SplitFace
                } else {
                    StoneKind::RoughHewn
                };
                slots.push(Slot {
                    center,
                    rotation: tilt * frame,
                    size: Vec3::new(b - a - gap, sh - gap * 0.6, depth),
                    kind,
                });
            }
            y += h;
        }
    }
    // Through-stones: slabs across the whole wall, ends showing on both faces.
    let mut x = -LENGTH * 0.5 + rng.range(0.4, 0.8);
    while x < LENGTH * 0.5 - 0.3 {
        let y = rng.range(0.42, 0.52);
        slots.push(Slot {
            center: Vec3::new(x, y, 0.0),
            rotation: Quat::from_rotation_y(std::f32::consts::FRAC_PI_2 + rng.range(-0.1, 0.1)),
            size: Vec3::new(half_width(y) * 2.0 + 0.08, rng.range(0.07, 0.1), 0.3),
            kind: StoneKind::Slab,
        });
        x += rng.range(1.1, 1.6);
    }
    // Coping: slabs on edge across the top, tall and short in turn.
    let mut x = -LENGTH * 0.5 + 0.03;
    let mut tall = true;
    while x < LENGTH * 0.5 - 0.05 {
        let thick = rng.range(0.07, 0.13);
        let h = if tall {
            rng.range(0.22, 0.3)
        } else {
            rng.range(0.15, 0.21)
        };
        tall = !tall;
        slots.push(Slot {
            center: Vec3::new(x + thick * 0.5, BODY + h * 0.5 - 0.02, 0.0),
            // Its length stands up, its thickness runs along the wall, leaning a little.
            rotation: Quat::from_rotation_z(std::f32::consts::FRAC_PI_2 + rng.range(-0.08, 0.08)),
            size: Vec3::new(h, thick, TOP + rng.range(0.0, 0.08)),
            kind: StoneKind::Slab,
        });
        x += thick + rng.range(0.008, 0.02);
    }
    slots
}

/// Stones made once and reused across the wall (scaled to fit each slot), as virtual geometry
/// would have it: a few dozen distinct stones, thousands of instances.
struct Palette {
    stones: Vec<(WallStone, Handle<Mesh>)>,
}

impl Palette {
    fn new(meshes: &mut Assets<Mesh>) -> Self {
        let mut stones = Vec::new();
        let mut seed = 100;
        for kind in [StoneKind::SplitFace, StoneKind::RoughHewn] {
            for &(len, h) in &[
                (0.32, 0.2),
                (0.45, 0.22),
                (0.55, 0.18),
                (0.4, 0.26),
                (0.25, 0.2),
            ] {
                for lichen in [0.15, 0.45] {
                    seed += 1;
                    let s = WallStone::new(kind, Vec3::new(len, h, 0.26), seed).with_lichen(lichen);
                    stones.push((s, meshes.add(s.base_mesh())));
                }
            }
        }
        for &(len, t, d) in &[(0.25, 0.1, 0.5), (0.3, 0.12, 0.55), (0.7, 0.09, 0.32)] {
            for lichen in [0.3, 0.6] {
                seed += 1;
                let s =
                    WallStone::new(StoneKind::Slab, Vec3::new(len, t, d), seed).with_lichen(lichen);
                stones.push((s, meshes.add(s.base_mesh())));
            }
        }
        Self { stones }
    }

    /// The stone that fits `slot` with the least stretching, and its scale.
    fn pick(&self, slot: &Slot, salt: usize) -> (usize, Vec3) {
        let mut best = (0, f32::MAX);
        for (i, (s, _)) in self.stones.iter().enumerate() {
            if s.kind != slot.kind {
                continue;
            }
            let r = slot.size / s.size;
            let stretch = r.max_element() / r.min_element() + (i + salt) as f32 % 7.0 * 0.004;
            if stretch < best.1 {
                best = (i, stretch);
            }
        }
        (best.0, slot.size / self.stones[best.0].0.size)
    }
}

/// The wall's stones as entities, and the dark rubble core showing in the chinks. `new` uses
/// the palette's displaced stones; otherwise chamfered, lumpy boxes like today's showcase walls.
fn spawn_wall(
    world: &mut World,
    at: Vec3,
    slots: &[Slot],
    palette: &Palette,
    new: bool,
    boxes: &mut Vec<Handle<Mesh>>,
) -> Vec<Entity> {
    let stone = if new {
        StandardMaterial::color(1.0, 1.0, 1.0)
            .with_roughness(0.88)
            .with_detail(0.35, 0.004)
    } else {
        StandardMaterial::color(1.0, 1.0, 1.0)
            .with_roughness(0.85)
            .with_detail(0.6, 0.02)
    };
    let core_material = StandardMaterial::color(0.11, 0.105, 0.095).with_roughness(0.95);
    let mut entities = Vec::new();
    for (i, slot) in slots.iter().enumerate() {
        let (index, scale) = palette.pick(slot, i);
        let (stone_def, base) = &palette.stones[index];
        let transform = Transform::from_translation((at + slot.center).as_dvec3())
            .with_rotation(slot.rotation)
            .with_scale(if new { scale } else { Vec3::ONE });
        let e = if new {
            world
                .spawn((
                    transform,
                    MeshRenderer::new(*base, stone),
                    stone_def.displacement(),
                ))
                .id()
        } else {
            // Today's stones: a chamfered box with its corners pushed in and out a little and
            // a flat color per stone.
            let mut rng = Rng(i as u64 * 7919 + 13);
            let chamfer = rng.range(0.02, 0.05).min(slot.size.min_element() * 0.3);
            let mut m = engine_render::mesh::rounded_cuboid(slot.size, chamfer, 1);
            for v in &mut m.vertices {
                let p = Vec3::from(v.position);
                let k = (p.x * 13.1 + p.y * 7.3 + p.z * 5.7 + i as f32).sin();
                v.position = (p * (1.0 + 0.04 * k)).to_array();
            }
            m.compute_normals();
            let tint = stone_def.base_mesh().vertices[0].color;
            let m = m.with_vertex_color(Vec4::from(tint) * rng.range(0.9, 1.15));
            let handle = world.resource_mut::<Assets<Mesh>>().add(m);
            boxes.push(handle);
            world
                .spawn((
                    transform,
                    MeshRenderer::new(handle, stone),
                    NoVirtualGeometry,
                ))
                .id()
        };
        entities.push(e);
    }
    // The hearting: a dark block well inside the faces (narrower than the wall's top).
    let core = engine_render::mesh::cuboid(Vec3::new(LENGTH, BODY, TOP - 0.12));
    let core = world.resource_mut::<Assets<Mesh>>().add(core);
    entities.push(
        world
            .spawn((
                Transform::from_translation((at + Vec3::new(0.0, BODY * 0.5, 0.0)).as_dvec3()),
                MeshRenderer::new(core, core_material),
            ))
            .id(),
    );
    entities
}

fn mesh_state(app: &App) -> serde_json::Value {
    let describe = app.world.resource::<Renderer>().describe();
    describe["features"]
        .as_array()
        .and_then(|f| f.iter().find(|f| f["name"] == "meshes"))
        .map(|f| f["state"]["virtual_geometry"].clone())
        .unwrap_or_default()
}

fn set_visible(app: &mut App, entities: &[Entity], visible: bool) {
    for &e in entities {
        if let Some(mut r) = app.world.get_mut::<MeshRenderer>(e) {
            r.visible = visible;
        }
    }
}

/// `--only stones`: one stone of each kind on the ground in a raking light, 40 cm away (quick
/// to build: three stones).
fn stones_closeup(mut app: App, out: &std::path::Path, frames: usize) {
    let stones = [
        WallStone::new(StoneKind::SplitFace, Vec3::new(0.42, 0.22, 0.26), 3).with_lichen(0.4),
        WallStone::new(StoneKind::RoughHewn, Vec3::new(0.38, 0.2, 0.26), 4).with_lichen(0.2),
        WallStone::new(StoneKind::Slab, Vec3::new(0.4, 0.08, 0.3), 5).with_lichen(0.5),
    ];
    let material = StandardMaterial::color(1.0, 1.0, 1.0)
        .with_roughness(0.88)
        .with_detail(0.35, 0.004);
    for (i, s) in stones.iter().enumerate() {
        let mesh = app.world.resource_mut::<Assets<Mesh>>().add(s.base_mesh());
        app.world.spawn((
            Transform::from_xyz(i as f64 * 0.47 - 0.47, s.size.y as f64 * 0.5, 0.0)
                .with_rotation(Quat::from_rotation_y(0.25)),
            MeshRenderer::new(mesh, material),
            s.displacement(),
        ));
    }
    app.world.spawn((
        Transform::default(),
        MeshRenderer::new(
            Shape::Plane {
                size: [20.0, 20.0],
                subdivisions: 1,
            },
            StandardMaterial::preset(MaterialPreset::ForestSoil).with_triplanar_world(),
        ),
    ));
    app.world.spawn((
        DirectionalLight::default(),
        Transform::default().looking_at(DVec3::new(0.7, -0.45, -0.55), Vec3::Y),
    ));
    app.world.spawn((
        Camera::perspective(50.0),
        Transform::from_xyz(0.05, 0.5, 0.75).looking_at(DVec3::new(0.0, 0.08, 0.0), Vec3::Y),
    ));
    app.update();
    if let Some(d) = app.world.get_resource::<RenderDisabled>() {
        eprintln!("no GPU: {}", d.0);
        return;
    }
    let started = Instant::now();
    while mesh_state(&app)["meshes_ready"].as_u64().unwrap_or(0) < 3 {
        assert!(
            started.elapsed() < Duration::from_secs(900),
            "builds didn't finish"
        );
        std::thread::sleep(Duration::from_millis(50));
        app.update();
    }
    eprintln!("builds: {}", mesh_state(&app)["builds"]);
    app.update_n(frames);
    capture_now(&mut app.world, out.join("stones.png")).expect("screenshot");
    let stats = app.world.resource::<RenderStats>().clone();
    eprintln!("stones: {} triangles on screen", stats.triangles);
    app.world.resource_mut::<RenderSettings>().debug_view = DebugView::Clusters;
    app.update_n(2);
    capture_now(&mut app.world, out.join("stones_clusters.png")).expect("screenshot");
    eprintln!("{}", mesh_state(&app));
    {
        let mut q = app.world.query_filtered::<&mut Transform, With<Camera>>();
        for mut t in q.iter_mut(&mut app.world) {
            *t = Transform::from_xyz(0.3, 1.2, 2.2).looking_at(DVec3::new(0.0, 0.1, 0.0), Vec3::Y);
        }
    }
    app.update_n(2);
    capture_now(&mut app.world, out.join("stones_clusters_2m.png")).expect("screenshot");
}

fn main() {
    let mut args = std::env::args().skip(1);
    let mut out =
        PathBuf::from("/mnt/project-files/engine-screenshots/rendering/round8/vg_closeup");
    let mut frames = 24usize;
    let mut only: Option<String> = None;
    while let Some(a) = args.next() {
        match a.as_str() {
            "--frames" => frames = args.next().and_then(|v| v.parse().ok()).unwrap_or(frames),
            "--only" => only = args.next(),
            _ => out = PathBuf::from(a),
        }
    }
    std::fs::create_dir_all(&out).expect("output directory");
    engine_core::log::init("warn");
    let mut app = App::new();
    app.world.resource_mut::<EngineConfig>().headless = true;
    {
        let mut w = app.world.resource_mut::<Window>();
        w.width = 1280;
        w.height = 720;
    }
    app.add_plugins(RenderPlugin);
    app.world.resource_mut::<RenderSettings>().sky_occlusion = false;
    if only.as_deref() == Some("stones") {
        stones_closeup(app, &out, frames);
        return;
    }

    let palette = {
        let mut meshes = app.world.resource_mut::<Assets<Mesh>>();
        Palette::new(&mut meshes)
    };
    let slots = wall_layout(7);
    let mut boxes = Vec::new();
    let new_wall = spawn_wall(
        &mut app.world,
        Vec3::ZERO,
        &slots,
        &palette,
        true,
        &mut boxes,
    );
    let old_wall = spawn_wall(
        &mut app.world,
        Vec3::ZERO,
        &slots,
        &palette,
        false,
        &mut boxes,
    );
    eprintln!(
        "wall: {} stones from a palette of {}",
        slots.len(),
        palette.stones.len()
    );
    let ground = StandardMaterial::preset(MaterialPreset::ForestSoil).with_triplanar_world();
    app.world.spawn((
        Transform::default(),
        MeshRenderer::new(
            Shape::Plane {
                size: [600.0, 600.0],
                subdivisions: 1,
            },
            ground,
        ),
    ));
    app.world.spawn((
        Camera::perspective(50.0),
        Transform::from_xyz(0.0, 2.0, 8.0).looking_at(DVec3::new(0.0, 50.0, 0.0), Vec3::Z),
    ));

    app.update();
    if let Some(d) = app.world.get_resource::<RenderDisabled>() {
        eprintln!("no GPU: {}", d.0);
        return;
    }
    let started = Instant::now();
    // Palette stones no slot picked are never drawn, so never built.
    let wanted = (slots.iter().enumerate())
        .map(|(i, s)| palette.pick(s, i).0)
        .collect::<std::collections::HashSet<_>>()
        .len() as u64;
    while mesh_state(&app)["meshes_ready"].as_u64().unwrap_or(0) < wanted {
        assert!(
            started.elapsed() < Duration::from_secs(1800),
            "builds didn't finish: {}",
            mesh_state(&app)
        );
        std::thread::sleep(Duration::from_millis(50));
        app.update();
    }
    let builds = mesh_state(&app)["builds"].clone();
    eprintln!(
        "{wanted} stones clustered in {:.1} s (wall clock): {builds}",
        started.elapsed().as_secs_f32()
    );
    app.world.resource_mut::<RenderSettings>().sky_occlusion = true;
    // Low afternoon sun from the left: raking light shows the relief.
    app.world.spawn((
        DirectionalLight::default(),
        Transform::default().looking_at(DVec3::new(0.62, -0.42, -0.66), Vec3::Y),
    ));

    // Views of the +z face: 30 cm from a stone, 2 m, 30 m.
    let face = half_width(0.45) as f64;
    let close = (
        DVec3::new(0.18, 0.62, face + 0.3),
        DVec3::new(0.05, 0.48, face - 0.05),
    );
    let mid = (
        DVec3::new(0.6, 1.3, face + 2.0),
        DVec3::new(-0.2, 0.55, 0.0),
    );
    let far = (DVec3::new(9.0, 6.0, 28.5), DVec3::new(0.0, 0.5, 0.0));
    let mut shots: Vec<(&str, (DVec3, DVec3), DebugView, bool)> = vec![
        ("after_030cm", close, DebugView::None, true),
        ("after_2m", mid, DebugView::None, true),
        ("after_30m", far, DebugView::None, true),
        ("before_030cm", close, DebugView::None, false),
        ("before_2m", mid, DebugView::None, false),
        ("before_30m", far, DebugView::None, false),
        ("after_030cm_clusters", close, DebugView::Clusters, true),
        ("after_2m_clusters", mid, DebugView::Clusters, true),
        (
            "after_2m_cluster_levels",
            mid,
            DebugView::ClusterLevels,
            true,
        ),
        (
            "after_30m_cluster_levels",
            far,
            DebugView::ClusterLevels,
            true,
        ),
    ];
    if let Some(o) = &only {
        shots.retain(|s| s.0.contains(o.as_str()));
    }
    for (name, (eye, target), debug, new) in shots {
        set_visible(&mut app, &new_wall, new);
        set_visible(&mut app, &old_wall, !new);
        {
            let mut q = app.world.query_filtered::<&mut Transform, With<Camera>>();
            for mut t in q.iter_mut(&mut app.world) {
                *t = Transform::from_translation(eye).looking_at(target, Vec3::Y);
            }
        }
        app.world.resource_mut::<RenderSettings>().debug_view = debug;
        // Let exposure and temporal anti-aliasing settle, then time a few frames.
        app.update_n(frames);
        let timed = Instant::now();
        app.update_n(4);
        let frame_ms = timed.elapsed().as_secs_f32() * 1000.0 / 4.0;
        let path = out.join(format!("{name}.png"));
        capture_now(&mut app.world, &path).expect("screenshot");
        let stats = app.world.resource::<RenderStats>().clone();
        let vg = mesh_state(&app)["stats"].clone();
        eprintln!(
            "{name}: {} triangles on screen ({} clusters, {} in clusters), shadows {} \
             triangles, {frame_ms:.0} ms per frame",
            stats.triangles, stats.clusters, vg["triangles"], vg["shadow_triangles"]
        );
    }
    set_visible(&mut app, &old_wall, false);
    set_visible(&mut app, &new_wall, true);
    app.world.resource_mut::<RenderSettings>().debug_view = DebugView::None;

    // Many walls far away: the triangles on screen barely grow.
    if only.as_deref().is_none_or(|o| "far_walls".contains(o)) {
        let mut placed = 1;
        for count in [8, 64] {
            while placed < count {
                let k = placed as f32;
                let at = Vec3::new(
                    ((k * 37.0) % 9.0 - 4.0) * 9.0,
                    0.0,
                    -12.0 - (k / 9.0).floor() * 14.0,
                );
                spawn_wall(&mut app.world, at, &slots, &palette, true, &mut boxes);
                placed += 1;
            }
            {
                let mut q = app.world.query_filtered::<&mut Transform, With<Camera>>();
                for mut t in q.iter_mut(&mut app.world) {
                    *t = Transform::from_translation(DVec3::new(6.0, 7.0, 34.0))
                        .looking_at(DVec3::new(0.0, 0.0, -30.0), Vec3::Y);
                }
            }
            app.update_n(frames);
            let timed = Instant::now();
            app.update_n(4);
            let frame_ms = timed.elapsed().as_secs_f32() * 1000.0 / 4.0;
            let name = format!("far_walls_{count}");
            capture_now(&mut app.world, out.join(format!("{name}.png"))).expect("screenshot");
            let stats = app.world.resource::<RenderStats>().clone();
            let vg = mesh_state(&app)["stats"].clone();
            eprintln!(
                "{name}: {} stones, {} triangles on screen ({} clusters), shadows {} triangles, \
                 {frame_ms:.0} ms per frame",
                vg["instances"], stats.triangles, stats.clusters, vg["shadow_triangles"]
            );
        }
    }
}
