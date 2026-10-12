//! Renders the proving ground's structure meshes headless, for checking them by eye.
//!
//! ```sh
//! cargo run -p engine_physics --example art_structures -- [out_dir] [only] [width]x[height]
//! ```
//!
//! `only` renders just the shots whose name contains it (`NONE` renders none, for the
//! triangle counts alone; `ART_COUNTS=1` also prints each mesh's). Materials are plain
//! colors (the textures come later). Needs a GPU or a software Vulkan driver (lavapipe).

// The kit and the structures are shared with the crate, which uses more of them.
#![allow(dead_code)]

#[path = "../src/demo/art/kit.rs"]
mod kit;
#[path = "../src/demo/art/structures.rs"]
mod structures;

use bevy_ecs::component::{Component, Mutable};
use engine_core::math::{DVec3, Quat, Vec3};
use engine_core::prelude::*;
use engine_core::time::TimeSource;
use engine_render::prelude::*;
use engine_render::{RenderDisabled, capture_now};

/// `Assets<Mesh>` without naming `engine_assets` (not a dependency of this crate): the
/// type of `resolve_mesh_renderers`' third parameter.
trait ResInner {
    type Inner: Resource + Component<Mutability = Mutable>;
}
impl<T: Resource + Component<Mutability = Mutable>> ResInner for ResMut<'_, T> {
    type Inner = T;
}
fn mesh_assets<A, B, C: ResInner, D, E>(
    _: fn(A, B, C, D, E),
    world: &mut World,
) -> Mut<'_, C::Inner> {
    world.resource_mut::<C::Inner>()
}

#[derive(Clone, Copy, Debug)]
enum M {
    Concrete,
    Slab,
    Painted,
    PaintedDark,
    Timber,
    Steel,
    Rubber,
    Hazard,
    Diamond,
}

fn material(m: M) -> StandardMaterial {
    match m {
        M::Concrete => StandardMaterial::hex(0x9a958c).with_roughness(0.9),
        M::Slab => StandardMaterial::hex(0x8c8b86).with_roughness(0.92),
        M::Painted => StandardMaterial::hex(0xd6a21e).with_roughness(0.5),
        M::PaintedDark => StandardMaterial::hex(0x3d4247).with_roughness(0.55),
        M::Timber => StandardMaterial::hex(0x8a6a48).with_roughness(0.85),
        M::Steel => StandardMaterial::hex(0x8a8d92)
            .with_metallic(1.0)
            .with_roughness(0.45),
        M::Rubber => StandardMaterial::hex(0x1e1e20).with_roughness(0.9),
        M::Hazard => StandardMaterial::hex(0xe0b020).with_roughness(0.6),
        M::Diamond => StandardMaterial::hex(0x9a9ea4)
            .with_metallic(1.0)
            .with_roughness(0.4),
    }
}

/// Yard-local positions, as in `demo::build`.
const STAIRS: [f64; 3] = [24.0, 0.0, -10.0];
const PILE: [f64; 3] = [-14.0, 0.0, -14.0];
const GANTRY: [f64; 3] = [-26.0, 0.0, 10.0];
const SEESAW: [f64; 3] = [-10.0, 0.0, 10.0];
const WHEEL: [f64; 3] = [4.0, 0.0, 10.0];
const BRIDGE: [f64; 3] = [18.0, 0.0, 10.0];
const TURNTABLE: [f64; 3] = [30.0, 0.0, 10.0];
const ORIGIN: [f64; 3] = [0.0, 0.0, 0.0];

/// Shots relative to a structure's position: (name, origin, eye, target, sun direction).
type ShotDef = (&'static str, [f64; 3], [f64; 3], [f64; 3], [f64; 3]);
const SHOTS: &[ShotDef] = &[
    (
        "yard_overview",
        ORIGIN,
        [0.0, 24.0, 44.0],
        [0.0, 2.0, -3.0],
        [-0.45, -0.75, -0.5],
    ),
    (
        "yard_curb_close",
        ORIGIN,
        [-3.0, 1.3, 21.5],
        [0.0, 0.45, 24.6],
        [-0.7, -0.45, 0.4],
    ),
    (
        "yard_corner",
        ORIGIN,
        [30.5, 2.2, 20.5],
        [34.6, 0.3, 24.6],
        [-0.4, -0.5, 0.75],
    ),
    (
        "stairs_wide",
        STAIRS,
        [9.0, 4.5, 8.0],
        [0.0, 1.4, -2.0],
        [-0.6, -0.55, -0.35],
    ),
    (
        "stairs_nosing_close",
        STAIRS,
        [1.6, 1.25, 0.9],
        [0.4, 0.5, -0.8],
        [-0.8, -0.45, -0.2],
    ),
    (
        "stairs_side",
        STAIRS,
        [-7.0, 2.2, 1.5],
        [-2.0, 1.6, -3.5],
        [0.7, -0.5, -0.4],
    ),
    (
        "stairs_coping",
        STAIRS,
        [-1.6, 4.0, -4.6],
        [-3.0, 3.2, -6.0],
        [-0.5, -0.6, 0.45],
    ),
    (
        "stairs_landing",
        STAIRS,
        [5.0, 6.0, -5.0],
        [0.0, 3.3, -6.5],
        [-0.5, -0.6, 0.45],
    ),
    (
        "gantry_wide",
        GANTRY,
        [9.0, 4.5, 11.0],
        [0.0, 4.0, 0.0],
        [-0.6, -0.5, -0.6],
    ),
    (
        "gantry_base",
        GANTRY,
        [2.0, 1.1, 4.4],
        [0.0, 0.35, 2.4],
        [-0.7, -0.5, -0.5],
    ),
    (
        "gantry_top",
        GANTRY,
        [2.6, 6.6, 2.4],
        [0.0, 7.6, 1.0],
        [-0.6, -0.4, -0.7],
    ),
    (
        "seesaw_wide",
        SEESAW,
        [4.5, 2.4, 6.5],
        [0.0, 0.6, 0.0],
        [-0.6, -0.55, -0.55],
    ),
    (
        "seesaw_pivot",
        SEESAW,
        [1.1, 1.05, 1.7],
        [0.0, 0.72, 0.4],
        [-0.7, -0.45, -0.55],
    ),
    (
        "seesaw_top",
        SEESAW,
        [2.6, 1.9, 1.6],
        [1.6, 1.0, 0.0],
        [-0.6, -0.55, -0.55],
    ),
    (
        "seesaw_end",
        SEESAW,
        [5.2, 1.1, 1.6],
        [3.8, 0.8, 0.2],
        [-0.5, -0.6, -0.6],
    ),
    (
        "wheel_wide",
        WHEEL,
        [6.5, 4.5, 9.0],
        [0.0, 1.2, 0.0],
        [-0.55, -0.6, -0.55],
    ),
    (
        "wheel_hub",
        WHEEL,
        [3.4, 2.8, 2.6],
        [2.3, 2.0, 0.4],
        [-0.6, -0.5, -0.6],
    ),
    (
        "wheel_wall",
        WHEEL,
        [4.8, 1.2, 3.8],
        [3.0, 0.8, 1.6],
        [-0.75, -0.35, -0.55],
    ),
    (
        "pile_wide",
        PILE,
        [8.0, 5.5, 10.0],
        [0.0, 0.8, 0.0],
        [-0.5, -0.6, -0.6],
    ),
    (
        "pile_corner",
        PILE,
        [5.6, 2.4, 6.2],
        [4.0, 1.3, 4.0],
        [-0.65, -0.5, -0.55],
    ),
    (
        "turntable_wide",
        TURNTABLE,
        [7.0, 3.5, 8.0],
        [0.0, 0.0, 0.0],
        [-0.55, -0.55, -0.6],
    ),
    (
        "turntable_rim",
        TURNTABLE,
        [4.4, 0.75, 1.6],
        [3.4, 0.1, 0.3],
        [-0.6, -0.5, -0.6],
    ),
    (
        "bridge_side",
        BRIDGE,
        [9.5, 4.0, 0.0],
        [0.0, 2.6, 0.0],
        [-0.5, -0.55, -0.65],
    ),
    (
        "bridge_tower",
        BRIDGE,
        [3.2, 5.6, -9.5],
        [0.0, 4.0, -6.3],
        [-0.6, -0.5, 0.6],
    ),
    (
        "bridge_deck",
        BRIDGE,
        [2.0, 4.6, -2.5],
        [0.8, 3.5, -0.3],
        [-0.6, -0.55, -0.55],
    ),
];

fn spawn(world: &mut World, mesh: engine_render::Mesh, m: M, at: DVec3, rot: Quat) -> usize {
    let tris = mesh.triangle_count();
    if tris == 0 {
        return 0;
    }
    if std::env::var_os("ART_COUNTS").is_some() {
        println!("    {m:?}: {tris}");
    }
    let handle = mesh_assets(engine_render::resolve_mesh_renderers, world).add(mesh);
    world.spawn((
        Transform::from_translation(at).with_rotation(rot),
        MeshRenderer::new(handle, material(m)),
    ));
    tris
}

/// The bridge deck's plank ends, as `demo::bridge` computes them (yard-local, from `center`).
fn bridge_ends(center: DVec3) -> Vec<DVec3> {
    let (span, planks, plank, height) = (12.0f64, 15usize, 0.82f64, 4.5f64);
    let arc = |sag: f64| {
        let n = 2000;
        let pts: Vec<DVec3> = (0..=n)
            .map(|k| {
                let z = -span * 0.5 + span * k as f64 / n as f64;
                DVec3::new(0.0, -sag * (1.0 - (2.0 * z / span).powi(2)), z)
            })
            .collect();
        let mut lengths = vec![0.0];
        for w in pts.windows(2) {
            lengths.push(lengths.last().unwrap() + w[0].distance(w[1]));
        }
        (pts, lengths)
    };
    let deck = planks as f64 * plank;
    let (mut lo, mut hi) = (0.0, span);
    for _ in 0..60 {
        let mid = 0.5 * (lo + hi);
        if *arc(mid).1.last().unwrap() < deck {
            lo = mid;
        } else {
            hi = mid;
        }
    }
    let (pts, lengths) = arc(0.5 * (lo + hi));
    let point_at = |s: f64| {
        let i = lengths.partition_point(|l| *l < s).clamp(1, pts.len() - 1);
        let t = (s - lengths[i - 1]) / (lengths[i] - lengths[i - 1]).max(1e-12);
        pts[i - 1].lerp(pts[i], t)
    };
    (0..=planks)
        .map(|k| center + DVec3::Y * height + point_at(k as f64 * plank))
        .collect()
}

fn build_scene(world: &mut World) {
    let mut total = 0;
    let mut report = |name: &str, tris: usize| {
        println!("{name:<28} {tris:>7} triangles");
        total += tris;
    };
    let v = |a: [f64; 3]| DVec3::from_array(a);
    let id = Quat::IDENTITY;

    let y = structures::yard([70.0, 50.0], 0.5, 0.8, 0.4);
    let t = spawn(world, y.slab, M::Slab, DVec3::ZERO, id)
        + spawn(world, y.curb, M::Concrete, DVec3::ZERO, id)
        + spawn(world, y.hazard, M::Hazard, DVec3::ZERO, id);
    report("yard (slab, curb)", t);

    let s = structures::stairs(0.22, 0.32, 14, 6.0, 3.5);
    let t = spawn(world, s.concrete, M::Concrete, v(STAIRS), id)
        + spawn(world, s.steel, M::Painted, v(STAIRS), id);
    report("stairs", t);

    let g = structures::gantry(7.6, 4.8);
    let o = v(GANTRY);
    let mut t = spawn(world, g.painted, M::Painted, o, id)
        + spawn(world, g.steel, M::Steel, o, id)
        + spawn(world, g.concrete, M::Concrete, o, id);
    report("gantry", t);
    // Stand-in chain and ball, hanging at rest.
    for i in 0..6 {
        t += spawn(
            world,
            Shape::Capsule {
                radius: 0.07,
                height: 0.8,
                segments: 12,
            }
            .mesh(),
            M::Steel,
            o + DVec3::new(0.0, 7.6 - 0.8 * (i as f64 + 0.5), 0.0),
            id,
        );
    }
    spawn(
        world,
        Shape::Sphere {
            radius: 0.8,
            segments: 32,
        }
        .mesh(),
        M::Steel,
        o + DVec3::new(0.0, 7.6 - 4.8 - 0.8, 0.0),
        id,
    );
    let _ = t;

    let o = v(SEESAW);
    let st = structures::seesaw_stand(Vec3::new(0.3, 0.4, 0.6));
    let pl = structures::seesaw_plank(Vec3::new(4.0, 0.1, 0.6));
    let so = o + DVec3::new(0.0, 0.4, 0.0);
    let po = o + DVec3::new(0.0, 0.9, 0.0);
    let t = spawn(world, st.painted, M::PaintedDark, so, id)
        + spawn(world, st.steel, M::Steel, so, id)
        + spawn(world, pl.timber, M::Timber, po, id)
        + spawn(world, pl.painted, M::PaintedDark, po, id)
        + spawn(world, pl.steel, M::Steel, po, id)
        + spawn(world, pl.rubber, M::Rubber, po, id);
    report("seesaw (stand + plank)", t);

    let o = v(WHEEL);
    let bin = structures::paddle_bin(engine_core::math::Vec2::new(3.0, 2.5), 0.3, 1.4, 2.0);
    let mut t = spawn(world, bin.concrete, M::Concrete, o, id)
        + spawn(world, bin.painted, M::PaintedDark, o, id)
        + spawn(world, bin.steel, M::Steel, o, id);
    let hub_at = o + DVec3::new(0.0, 2.0, 0.0);
    let spin = Quat::from_rotation_x(0.35);
    let hub = structures::paddle_hub(0.3, 2.7);
    t += spawn(world, hub.painted, M::Painted, hub_at, spin)
        + spawn(world, hub.steel, M::Steel, hub_at, spin);
    for k in 0..4 {
        let r = spin * Quat::from_rotation_x(k as f32 * std::f32::consts::FRAC_PI_2);
        let at = hub_at + (r * Vec3::new(0.0, 1.05, 0.0)).as_dvec3();
        let b = structures::paddle_blade(Vec3::new(2.6, 0.75, 0.05), 1.05);
        t += spawn(world, b.timber, M::Timber, at, r)
            + spawn(world, b.painted, M::Painted, at, r)
            + spawn(world, b.steel, M::Steel, at, r);
    }
    report("paddle wheel (bin, hub, 4)", t);

    let o = v(PILE);
    let bin = structures::pile_bin(engine_core::math::Vec2::new(4.0, 4.0), 0.3, 1.6);
    let t = spawn(world, bin.concrete, M::Concrete, o, id)
        + spawn(world, bin.painted, M::Painted, o, id)
        + spawn(world, bin.steel, M::Steel, o, id);
    report("pile bin", t);

    let o = v(TURNTABLE) + DVec3::new(0.0, 0.15, 0.0);
    let tt = structures::turntable(4.0, 0.3);
    let t = spawn(world, tt.deck, M::Diamond, o, id)
        + spawn(world, tt.painted, M::Painted, o, id)
        + spawn(world, tt.steel, M::Steel, o, id);
    report("turntable", t);

    let o = v(BRIDGE);
    let mut t = 0;
    for (dz, side) in [(-1.0f64, 1.0f32), (1.0, -1.0)] {
        let tw = structures::bridge_tower(Vec3::new(1.3, 2.25, 0.3), side);
        let at = o + DVec3::new(0.0, 2.25, dz * 6.3);
        t += spawn(world, tw.timber, M::Timber, at, id)
            + spawn(world, tw.steel, M::Steel, at, id)
            + spawn(world, tw.concrete, M::Concrete, at, id);
    }
    let ends = bridge_ends(o);
    for k in 0..ends.len() - 1 {
        let (a, b) = (ends[k], ends[k + 1]);
        let dir = (b - a).normalize().as_vec3();
        let half = Vec3::new(1.0, 0.06, (a.distance(b) * 0.5) as f32);
        let pl = structures::bridge_plank(half, k);
        let rot = Quat::from_rotation_arc(Vec3::Z, dir);
        t += spawn(world, pl.timber, M::Timber, (a + b) * 0.5, rot)
            + spawn(world, pl.steel, M::Steel, (a + b) * 0.5, rot);
    }
    report("bridge (towers, 15 planks)", t);

    println!("{:<28} {total:>7} triangles", "total");
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let out = std::path::PathBuf::from(args.get(1).cloned().unwrap_or("structure_shots".into()));
    let only = args.get(2).cloned().unwrap_or_default();
    let (w, h) = args
        .get(3)
        .and_then(|s| s.split_once('x'))
        .and_then(|(a, b)| Some((a.parse().ok()?, b.parse().ok()?)))
        .unwrap_or((1280u32, 720u32));
    std::fs::create_dir_all(&out).expect("output directory");

    let mut app = App::new();
    app.world.resource_mut::<EngineConfig>().headless = true;
    {
        let mut win = app.world.resource_mut::<Window>();
        win.width = w;
        win.height = h;
    }
    app.add_plugins(RenderPlugin);
    app.world
        .resource_mut::<Time>()
        .set_source(TimeSource::Manual {
            step_secs: 1.0 / 60.0,
        });
    app.world.resource_mut::<AmbientLight>().brightness = 400.0;
    let camera = app
        .world
        .spawn((
            Camera::perspective(50.0),
            Transform::from_xyz(0.0, 24.0, 44.0),
        ))
        .id();
    let sun = app
        .world
        .spawn((
            DirectionalLight::default(),
            Transform::default().looking_at(DVec3::new(-0.45, -0.75, -0.5), Vec3::Y),
        ))
        .id();
    app.world.spawn((
        Transform::from_xyz(0.0, -0.02, 0.0),
        MeshRenderer::new(
            Shape::Plane {
                size: [600.0, 600.0],
                subdivisions: 1,
            },
            StandardMaterial::hex(0x4f7a3a).with_roughness(0.95),
        ),
    ));
    build_scene(&mut app.world);
    // A 1.8 m stand-in person next to each structure, for scale.
    for p in [
        [27.6, 0.9, -8.0],
        [-23.6, 0.9, 12.5],
        [-7.4, 0.9, 11.4],
        [7.9, 0.9, 13.6],
        [-9.5, 0.9, -9.2],
        [33.0, 1.2, 13.0],
        [20.8, 0.9, 1.2],
    ] {
        app.world.spawn((
            Transform::from_translation(DVec3::from_array(p)),
            MeshRenderer::new(
                Shape::Capsule {
                    radius: 0.25,
                    height: 1.8,
                    segments: 16,
                },
                StandardMaterial::hex(0x2e86e8).with_roughness(0.6),
            ),
        ));
    }

    for &(name, origin, eye, target, sun_dir) in
        SHOTS.iter().filter(|s| s.0.contains(only.as_str()))
    {
        let o = DVec3::from_array(origin);
        *app.world.get_mut::<Transform>(camera).unwrap() =
            Transform::from_translation(o + DVec3::from_array(eye))
                .looking_at(o + DVec3::from_array(target), Vec3::Y);
        *app.world.get_mut::<Transform>(sun).unwrap() =
            Transform::default().looking_at(DVec3::from_array(sun_dir), Vec3::Y);
        for _ in 0..4 {
            app.update();
        }
        if let Some(d) = app.world.get_resource::<RenderDisabled>() {
            eprintln!("no GPU: {}", d.0);
            return;
        }
        let path = out.join(format!("{name}.png"));
        match capture_now(&mut app.world, &path) {
            Ok(_) => println!("{}", path.display()),
            Err(e) => eprintln!("{name}: {e}"),
        }
    }
}
