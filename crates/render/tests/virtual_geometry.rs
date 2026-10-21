//! Virtual geometry: a dense procedural rock (1.3 million triangles) drawn as clusters looks
//! like the regular instanced path up close, draws far fewer triangles from afar, never lets
//! the background through while the camera flies through level-of-detail changes, and culls
//! clusters outside the view. Needs a GPU or lavapipe; passes with a note when neither is
//! available.

use std::sync::OnceLock;
use std::time::{Duration, Instant};

use engine_assets::Assets;
use engine_core::prelude::*;
use engine_render::prelude::*;
use engine_render::{RenderDisabled, RenderStats, Renderer, capture_now};
use image::RgbaImage;

const W: u32 = 320;
const H: u32 = 180;

/// Hash of a lattice point to [0, 1].
fn hash(x: i32, y: i32, z: i32) -> f32 {
    let mut h = (x as u32).wrapping_mul(0x8da6_b343)
        ^ (y as u32).wrapping_mul(0xd816_3841)
        ^ (z as u32).wrapping_mul(0xcb1a_b31f);
    h ^= h >> 13;
    h = h.wrapping_mul(0x5bd1_e995);
    h ^= h >> 15;
    (h & 0xff_ffff) as f32 / 16_777_215.0
}

/// Smooth value noise in [0, 1].
fn noise(p: Vec3) -> f32 {
    let i = p.floor();
    let f = p - i;
    let s = f * f * (Vec3::splat(3.0) - 2.0 * f);
    let (x, y, z) = (i.x as i32, i.y as i32, i.z as i32);
    let lerp = |a: f32, b: f32, t: f32| a + (b - a) * t;
    let c = |dx, dy, dz| hash(x + dx, y + dy, z + dz);
    lerp(
        lerp(
            lerp(c(0, 0, 0), c(1, 0, 0), s.x),
            lerp(c(0, 1, 0), c(1, 1, 0), s.x),
            s.y,
        ),
        lerp(
            lerp(c(0, 0, 1), c(1, 0, 1), s.x),
            lerp(c(0, 1, 1), c(1, 1, 1), s.x),
            s.y,
        ),
        s.z,
    )
}

fn fbm(mut p: Vec3, octaves: u32) -> f32 {
    let (mut sum, mut amp, mut total) = (0.0, 0.5, 0.0);
    for _ in 0..octaves {
        sum += amp * noise(p);
        total += amp;
        p = Vec3::new(
            p.y * 1.6 + p.z * 1.2,
            p.z * 1.6 - p.x * 1.2,
            p.x * 1.6 + p.y * 1.2,
        ) + Vec3::splat(5.3);
        amp *= 0.5;
    }
    sum / total
}

/// A boulder about 2.8 × 1.8 × 2.2 m: an icosphere displaced by layered noise with sharp
/// ridges, `subdivisions = 8` gives 1,310,720 triangles.
fn rock(subdivisions: u32) -> Mesh {
    let mut m = engine_render::mesh::icosphere(1.0, subdivisions);
    for v in &mut m.vertices {
        let p = Vec3::from(v.position);
        let ridge = 1.0 - (2.0 * noise(p * 5.0 + Vec3::splat(11.0)) - 1.0).abs();
        let r = 1.0
            + 0.4 * (fbm(p * 1.4, 4) - 0.5)
            + 0.08 * ridge * ridge
            + 0.03 * (fbm(p * 18.0, 4) - 0.5);
        v.position = (p * r * Vec3::new(1.4, 0.9, 1.1)).to_array();
    }
    m.compute_normals();
    m
}

fn dense_rock() -> &'static Mesh {
    static ROCK: OnceLock<Mesh> = OnceLock::new();
    ROCK.get_or_init(|| rock(8))
}

/// The rock at the origin, a camera at `eye` looking at `target`, sun from the upper left.
fn rock_app(
    eye: DVec3,
    target: DVec3,
    material: StandardMaterial,
    configure: impl FnOnce(&mut RenderSettings),
) -> App {
    engine_core::log::init("error");
    let mut app = App::new();
    app.world.resource_mut::<EngineConfig>().headless = true;
    {
        let mut w = app.world.resource_mut::<Window>();
        w.width = W;
        w.height = H;
    }
    app.add_plugins(RenderPlugin);
    {
        let mut s = app.world.resource_mut::<RenderSettings>();
        s.tonemapper = Tonemapper::None;
        s.anti_aliasing = AntiAliasing::None;
        s.bloom_intensity = 0.0;
        s.vignette = 0.0;
        s.film_grain = 0.0;
        s.saturation = 1.0;
        s.sharpness = 0.0;
        configure(&mut s);
    }
    let mesh = app
        .world
        .resource_mut::<Assets<Mesh>>()
        .add(dense_rock().clone());
    let w = &mut app.world;
    w.spawn((
        Camera {
            exposure: Exposure::Manual { ev100: 14.0 },
            ..Camera::perspective(50.0)
        },
        Transform::from_translation(eye).looking_at(target, Vec3::Y),
    ));
    w.spawn((
        DirectionalLight::default(),
        Transform::default().looking_at(DVec3::new(0.5, -0.7, -0.5), Vec3::Y),
    ));
    w.spawn((Transform::default(), MeshRenderer::new(mesh, material)));
    app
}

fn rock_material() -> StandardMaterial {
    StandardMaterial::color(0.42, 0.38, 0.33).with_roughness(0.85)
}

fn has_gpu(app: &App) -> bool {
    if let Some(d) = app.world.get_resource::<RenderDisabled>() {
        eprintln!("skipping: no GPU ({})", d.0);
        return false;
    }
    true
}

/// The mesh feature's virtual geometry state from `render.stats`.
fn cluster_state(app: &App) -> serde_json::Value {
    let describe = app.world.resource::<Renderer>().describe();
    describe["features"]
        .as_array()
        .and_then(|f| f.iter().find(|f| f["name"] == "meshes"))
        .map(|f| f["state"]["virtual_geometry"].clone())
        .unwrap_or_default()
}

/// Run frames until `ready` meshes are drawn as clusters (their builds run in the
/// background), then a few more so the GPU counters are read back. False without a GPU.
fn wait_for_clusters(app: &mut App, ready: u64) -> bool {
    app.update();
    if !has_gpu(app) {
        return false;
    }
    let started = Instant::now();
    while cluster_state(app)["meshes_ready"].as_u64().unwrap_or(0) < ready {
        assert!(
            started.elapsed() < Duration::from_secs(300),
            "cluster builds didn't finish: {}",
            cluster_state(app)
        );
        std::thread::sleep(Duration::from_millis(20));
        app.update();
    }
    app.update_n(6);
    true
}

fn capture(app: &mut App, name: &str) -> RgbaImage {
    let path = std::path::Path::new(env!("CARGO_TARGET_TMPDIR")).join(format!("{name}.png"));
    capture_now(&mut app.world, &path).expect("screenshot");
    image::open(&path).expect("png").to_rgba8()
}

fn move_camera(app: &mut App, eye: DVec3, target: DVec3) {
    let mut q = app.world.query_filtered::<&mut Transform, With<Camera>>();
    for mut t in q.iter_mut(&mut app.world) {
        *t = Transform::from_translation(eye).looking_at(target, Vec3::Y);
    }
}

#[test]
fn clusters_look_like_the_full_mesh_up_close() {
    let eye = DVec3::new(0.6, 0.9, 3.4);
    let target = DVec3::new(0.0, 0.1, 0.0);
    let mut instanced = rock_app(eye, target, rock_material(), |s| s.virtual_geometry = false);
    let mut clustered = rock_app(eye, target, rock_material(), |_| {});
    // Nearly full detail: shows that clusters shade exactly like the instanced mesh.
    let mut fine = rock_app(eye, target, rock_material(), |s| s.cluster_error_px = 0.05);
    if !wait_for_clusters(&mut clustered, 1) || !wait_for_clusters(&mut fine, 1) {
        return;
    }
    instanced.update_n(8);
    let reference = capture(&mut instanced, "vg_close_instanced");
    let source = dense_rock().triangle_count() as u64;
    for (app, name, max_mean, max_large) in [
        (&mut clustered, "vg_close_clusters", 2.5, 0.015),
        (&mut fine, "vg_close_fine_clusters", 1.0, 0.0005),
    ] {
        let img = capture(app, name);
        let stats = app.world.resource::<RenderStats>().clone();
        let (mean, large) = difference(&img, &reference);
        eprintln!(
            "{name}: {} clusters, {} of {source} triangles; mean difference {mean:.3}/255, \
             {:.3}% of pixels differ by more than 24",
            stats.clusters,
            stats.triangles,
            large * 100.0
        );
        assert!(stats.clusters > 0, "{name}: nothing was drawn as clusters");
        assert!(mean < max_mean, "{name}: mean difference {mean}");
        assert!(
            large < max_large,
            "{name}: {:.3}% of pixels differ visibly",
            large * 100.0
        );
    }
}

/// Mean largest channel difference (0..255) and the fraction of pixels differing by more
/// than 24.
fn difference(a: &RgbaImage, b: &RgbaImage) -> (f64, f64) {
    let (mut sum, mut large) = (0.0f64, 0u32);
    for (pa, pb) in a.pixels().zip(b.pixels()) {
        let d = (0..3)
            .map(|c| (pa[c] as i32 - pb[c] as i32).abs())
            .max()
            .unwrap();
        sum += d as f64;
        if d > 24 {
            large += 1;
        }
    }
    let n = (a.width() * a.height()) as f64;
    (sum / n, large as f64 / n)
}

#[test]
fn distant_rocks_draw_far_fewer_triangles() {
    let target = DVec3::new(0.0, 0.0, 0.0);
    let mut app = rock_app(DVec3::new(0.0, 0.5, 3.5), target, rock_material(), |_| {});
    if !wait_for_clusters(&mut app, 1) {
        return;
    }
    let near = app.world.resource::<RenderStats>().clone();
    move_camera(&mut app, DVec3::new(0.0, 5.0, 80.0), target);
    app.update_n(6);
    let far = app.world.resource::<RenderStats>().clone();
    let source = dense_rock().triangle_count() as u64;
    eprintln!(
        "source {source} triangles; near: {} clusters, {} triangles; far: {} clusters, {} triangles",
        near.clusters, near.triangles, far.clusters, far.triangles
    );
    assert!(near.triangles > 0 && far.triangles > 0);
    // Up close part of the rock faces away (culled by the normal cones) and the rest is
    // still far more detailed than 1 px; far away only a few hundred triangles are needed.
    assert!(near.triangles < source, "near: {}", near.triangles);
    assert!(
        far.triangles * 1000 < source,
        "far: {} of {source} triangles",
        far.triangles
    );
    assert!(
        far.triangles * 50 < near.triangles,
        "far {} vs near {}",
        far.triangles,
        near.triangles
    );
}

/// Pixels inside a disk around the rock's center, which a closed rock always covers.
fn background_pixels_inside(img: &RgbaImage, radius_px: f32) -> u32 {
    let (cx, cy) = (W as f32 * 0.5, H as f32 * 0.5);
    let mut leaks = 0;
    for (x, y, p) in img.enumerate_pixels() {
        let (dx, dy) = (x as f32 + 0.5 - cx, y as f32 + 0.5 - cy);
        if dx * dx + dy * dy > radius_px * radius_px {
            continue;
        }
        // The rock is unlit red; anything else is sky showing through.
        if !(p[0] as i32 > p[1] as i32 + 60 && p[0] as i32 > p[2] as i32 + 60) {
            leaks += 1;
        }
    }
    leaks
}

#[test]
fn no_background_shows_through_while_levels_change() {
    let target = DVec3::ZERO;
    let red = StandardMaterial {
        unlit: true,
        ..StandardMaterial::color(0.8, 0.02, 0.02)
    };
    let mut app = rock_app(DVec3::new(0.0, 0.0, 2.6), target, red, |_| {});
    if !wait_for_clusters(&mut app, 1) {
        return;
    }
    // Fly away from 2.6 m to 120 m while orbiting, so every level of detail is crossed and
    // clusters at different levels meet along the way.
    let focal = H as f32 * 0.5 / (25.0f32).to_radians().tan();
    let mut checked = 0;
    let mut levels_seen = 0.0;
    for step in 0..24 {
        let t = step as f64 / 23.0;
        let distance = 2.6 * (120.0f64 / 2.6).powf(t);
        let angle = t * 2.5;
        let eye = DVec3::new(angle.sin(), 0.25 * angle.cos(), angle.cos()).normalize() * distance;
        move_camera(&mut app, eye, target);
        app.update();
        let img = capture(&mut app, &format!("vg_fly_{step}"));
        // The rock's smallest half-extent is 0.9 * (1 - 0.2) = 0.72 m; check well inside it.
        let radius = 0.55 * 0.72 * focal / distance as f32;
        if radius < 1.5 {
            continue;
        }
        let leaks = background_pixels_inside(&img, radius);
        let stats = app.world.resource::<RenderStats>().clone();
        levels_seen += 1.0;
        eprintln!(
            "{distance:6.1} m: {} clusters, {} triangles, {leaks} background pixels in a {radius:.0} px disk",
            stats.clusters, stats.triangles
        );
        assert_eq!(
            leaks, 0,
            "background shows through the rock at {distance:.1} m (see vg_fly_{step}.png)"
        );
        checked += 1;
    }
    assert!(
        checked >= 12,
        "only {checked} frames checked ({levels_seen})"
    );
}

#[test]
fn clusters_outside_the_view_are_culled() {
    let target = DVec3::ZERO;
    let mut app = rock_app(DVec3::new(0.0, 0.5, 6.0), target, rock_material(), |_| {});
    // A second dense mesh that opts out: it must never be built or drawn as clusters.
    let opted_out = app
        .world
        .resource_mut::<Assets<Mesh>>()
        .add(engine_render::mesh::icosphere(1.0, 7));
    app.world.spawn((
        Transform::from_xyz(30.0, 0.0, 0.0),
        MeshRenderer::new(opted_out, rock_material()),
        NoVirtualGeometry,
    ));
    if !wait_for_clusters(&mut app, 1) {
        return;
    }
    let state = cluster_state(&app);
    assert_eq!(state["meshes_ready"], 1, "{state}");
    assert_eq!(state["meshes_building"], 0, "{state}");

    let centered = app.world.resource::<RenderStats>().clusters;
    // Turned so the rock's center is on the left edge of the view (horizontal half field of
    // view: atan(tan(25°) * 16 / 9) = 39.7°): about half of it is outside.
    move_camera(
        &mut app,
        DVec3::new(0.0, 0.5, 6.0),
        DVec3::new(4.98, 0.5, 0.0),
    );
    app.update_n(6);
    let edge = app.world.resource::<RenderStats>().clusters;
    // Looking away: nothing.
    move_camera(
        &mut app,
        DVec3::new(0.0, 0.5, 6.0),
        DVec3::new(0.0, 0.5, 12.0),
    );
    app.update_n(6);
    let away = app.world.resource::<RenderStats>().clusters;
    eprintln!("clusters drawn: centered {centered}, at the edge {edge}, looking away {away}");
    assert!(centered > 0);
    assert!(
        edge > 0 && edge * 10 < centered * 8,
        "edge {edge} vs centered {centered}"
    );
    assert_eq!(away, 0);
}

#[test]
fn a_changed_mesh_is_rebuilt_alone() {
    let target = DVec3::ZERO;
    let mut app = rock_app(DVec3::new(0.0, 1.0, 9.0), target, rock_material(), |s| {
        s.virtual_geometry_min_triangles = 10_000;
    });
    // A small wall-sized mesh (82k triangles) next to the big rock.
    let wall = app.world.resource_mut::<Assets<Mesh>>().add(rock(6));
    app.world.spawn((
        Transform::from_xyz(3.0, 0.0, 0.0),
        MeshRenderer::new(wall, rock_material()),
    ));
    if !wait_for_clusters(&mut app, 2) {
        return;
    }
    let before = cluster_state(&app);
    // Damage it: dent part of the surface (the asset's version changes).
    if let Some(m) = app.world.resource_mut::<Assets<Mesh>>().get_mut(wall) {
        for v in &mut m.vertices {
            if v.position[0] > 0.6 {
                v.position[0] = 0.6 + (v.position[0] - 0.6) * 0.3;
            }
        }
        m.compute_normals();
    }
    app.update();
    let during = cluster_state(&app);
    // The big rock is still drawn as clusters while the wall is rebuilt.
    assert_eq!(during["meshes_ready"], 1, "{during}");
    let started = Instant::now();
    if !wait_for_clusters(&mut app, 2) {
        return;
    }
    let after = cluster_state(&app);
    eprintln!(
        "rebuilt in {:.2} s; before {before}, after {after}",
        started.elapsed().as_secs_f32()
    );
    assert!(after["stats"]["clusters"].as_u64().unwrap() > 0);
    // The new clusters mostly reuse the space the old ones freed; the rock's 50 MB were not
    // uploaded again.
    let (before, after) = (
        before["pool_bytes"].as_u64().unwrap(),
        after["pool_bytes"].as_u64().unwrap(),
    );
    assert!(
        after * 10 < before * 11,
        "GPU pools grew from {before} to {after} bytes"
    );
}

#[test]
fn a_mesh_that_keeps_changing_is_not_reported_as_building() {
    let target = DVec3::ZERO;
    let mut app = rock_app(DVec3::new(0.0, 1.0, 9.0), target, rock_material(), |s| {
        s.virtual_geometry_min_triangles = 10_000;
    });
    // A mesh edited every frame (a deforming sheet): it stays on the instanced path, and
    // whoever waits for builds to finish (the showcase tour) must not wait for it.
    let wobbly = app.world.resource_mut::<Assets<Mesh>>().add(rock(6));
    app.world.spawn((
        Transform::from_xyz(3.0, 0.0, 0.0),
        MeshRenderer::new(wobbly, rock_material()),
    ));
    if !wait_for_clusters(&mut app, 1) {
        return;
    }
    for frame in 0..40 {
        if let Some(m) = app.world.resource_mut::<Assets<Mesh>>().get_mut(wobbly) {
            m.vertices[0].position[1] += 0.001 * frame as f32;
        }
        app.update();
    }
    let state = cluster_state(&app);
    assert_eq!(state["meshes_ready"], 1, "{state}");
    assert_eq!(state["meshes_building"], 0, "{state}");
    assert_eq!(state["meshes_changing"], 1, "{state}");
}

#[test]
fn mirrored_rocks_keep_their_outside() {
    let eye = DVec3::new(-0.6, 0.9, 3.4);
    let target = DVec3::new(0.0, 0.1, 0.0);
    let mirror = |app: &mut App| {
        let mut q = app.world.query::<(&mut Transform, &MeshRenderer)>();
        for (mut t, _) in q.iter_mut(&mut app.world) {
            t.scale = Vec3::new(-1.0, 1.0, 1.0);
        }
    };
    let mut instanced = rock_app(eye, target, rock_material(), |s| s.virtual_geometry = false);
    let mut clustered = rock_app(eye, target, rock_material(), |_| {});
    mirror(&mut instanced);
    mirror(&mut clustered);
    if !wait_for_clusters(&mut clustered, 1) {
        return;
    }
    instanced.update_n(8);
    let reference = capture(&mut instanced, "vg_mirrored_instanced");
    let img = capture(&mut clustered, "vg_mirrored_clusters");
    let (mean, large) = difference(&img, &reference);
    let clusters = clustered.world.resource::<RenderStats>().clusters;
    eprintln!(
        "mirrored: {clusters} clusters; mean difference {mean:.3}/255, {:.3}% of pixels differ",
        large * 100.0
    );
    assert!(clusters > 0);
    assert!(mean < 2.5 && large < 0.015, "mean {mean}, large {large}");
}
