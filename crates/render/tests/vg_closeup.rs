//! Virtual geometry close-ups: displaced geometry ([`Displacement`]) matches its analytic height
//! up close and simplifies far away, adjacent displaced meshes and mixed levels of detail
//! never let the sky through, builds round-trip through the disk cache, wall stones are the
//! same for a seed, and many distant walls cost few triangles. Needs a GPU or lavapipe; passes
//! with a note when neither is available.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use engine_assets::{Assets, Handle};
use engine_core::prelude::*;
use engine_render::prelude::*;
use engine_render::{RenderDisabled, RenderStats, Renderer, capture_now};
use image::RgbaImage;

const W: u32 = 320;
const H: u32 = 180;

/// A scratch folder of this test binary (cleared first).
fn scratch(name: &str) -> PathBuf {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join(name);
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// An app with a camera at `eye` looking at `target` and the build cache in `cache` (or off).
/// Add [`sun`] for a sky.
fn test_app(camera: Camera, eye: DVec3, target: DVec3, cache: Option<&Path>) -> App {
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
        s.sky_occlusion = false;
        s.virtual_geometry_cache = cache.is_some();
        s.virtual_geometry_cache_dir = cache.map(|c| c.display().to_string()).unwrap_or_default();
    }
    app.world.spawn((
        Camera {
            exposure: Exposure::Manual { ev100: 13.0 },
            ..camera
        },
        Transform::from_translation(eye).looking_at(target, Vec3::Y),
    ));
    app
}

fn sun(app: &mut App) {
    app.world.spawn((
        DirectionalLight::default(),
        Transform::default().looking_at(DVec3::new(0.5, -0.6, -0.6), Vec3::Y),
    ));
}

fn has_gpu(app: &App) -> bool {
    if let Some(d) = app.world.get_resource::<RenderDisabled>() {
        eprintln!("skipping: no GPU ({})", d.0);
        return false;
    }
    true
}

fn cluster_state(app: &App) -> serde_json::Value {
    let describe = app.world.resource::<Renderer>().describe();
    describe["features"]
        .as_array()
        .and_then(|f| f.iter().find(|f| f["name"] == "meshes"))
        .map(|f| f["state"]["virtual_geometry"].clone())
        .unwrap_or_default()
}

/// Run frames until `ready` meshes are drawn as clusters, then a few more so the GPU counters
/// are read back. False without a GPU.
fn wait_for_clusters(app: &mut App, ready: u64) -> bool {
    app.update();
    if !has_gpu(app) {
        return false;
    }
    let started = Instant::now();
    while cluster_state(app)["meshes_ready"].as_u64().unwrap_or(0) < ready {
        assert!(
            started.elapsed() < Duration::from_secs(600),
            "cluster builds didn't finish: {}",
            cluster_state(app)
        );
        std::thread::sleep(Duration::from_millis(20));
        app.update();
    }
    app.update_n(6);
    true
}

/// Pixels that differ by more than the output's dithering (a level or two per channel).
fn differing_pixels(a: &RgbaImage, b: &RgbaImage) -> usize {
    a.pixels()
        .zip(b.pixels())
        .filter(|(p, q)| (0..3).any(|c| p[c].abs_diff(q[c]) > 3))
        .count()
}

fn capture(app: &mut App, name: &str) -> RgbaImage {
    let path = Path::new(env!("CARGO_TARGET_TMPDIR")).join(format!("{name}.png"));
    capture_now(&mut app.world, &path).expect("screenshot");
    image::open(&path).expect("png").to_rgba8()
}

fn move_camera(app: &mut App, eye: DVec3, target: DVec3) {
    let mut q = app.world.query_filtered::<&mut Transform, With<Camera>>();
    for mut t in q.iter_mut(&mut app.world) {
        *t = Transform::from_translation(eye).looking_at(target, Vec3::Y);
    }
}

/// Sky pixels are blue and bright (with a [`sun`]); the unlit black surfaces are dark.
fn is_sky(p: &image::Rgba<u8>) -> bool {
    p[2] as u32 > 60 && p[2] > p[0]
}

fn black() -> StandardMaterial {
    let mut m = StandardMaterial::color(0.0, 0.0, 0.0);
    m.unlit = true;
    // Seen from either side (the profile strip is seen from below).
    m.double_sided = true;
    m
}

/// Waves along x, the same everywhere along z: 0..1.
fn waves(x: f32) -> f32 {
    0.5 + 0.5 * (x * std::f32::consts::TAU / 0.4).sin()
}

const AMPLITUDE: f32 = 0.04;

fn wave_displacement() -> Displacement {
    Displacement::function(AMPLITUDE, 42, |p, _, _| waves(p.x)).with_edges(0.004, 0.016)
}

#[test]
fn displaced_geometry_matches_its_height_up_close_and_simplifies_far_away() {
    // A flat, narrow strip (8 triangles) 2 m up, displaced into waves, seen almost from the
    // side, from below, through a 1 degree telephoto lens 11 m away (as good as orthographic,
    // with the sky behind): its top edge against the sky is the height profile.
    let height_m = 0.2f32;
    let distance = 0.5 * height_m as f64 / 0.5f64.to_radians().tan();
    let center = DVec3::new(0.0, 2.0, 0.0);
    let elevation = 10f64.to_radians();
    let eye = center + DVec3::new(0.0, -elevation.sin(), elevation.cos()) * distance;
    let mut app = test_app(Camera::perspective(1.0), eye, center, None);
    sun(&mut app);
    let strip_width = 0.06;
    let strip = app
        .world
        .resource_mut::<Assets<Mesh>>()
        .add(engine_render::mesh::plane(Vec2::new(2.4, strip_width), 1));
    app.world.spawn((
        Transform::from_translation(center),
        MeshRenderer::new(strip, black()),
        wave_displacement(),
    ));
    // Up in the image, in the world.
    let forward = (center - eye).normalize().as_vec3();
    let up = Vec3::X.cross(forward).normalize();
    if !wait_for_clusters(&mut app, 1) {
        return;
    }
    let img = capture(&mut app, "vgc_profile");
    let near = app.world.resource::<RenderStats>().clone();
    assert!(near.clusters > 0, "nothing was drawn as clusters");
    // Pixel rows to meters: the view spans `height_m` vertically, centered on the strip.
    let m_per_px = height_m / H as f32;
    let width_m = m_per_px * W as f32;
    let (mut worst, mut columns) = (0.0f32, 0);
    for x in 2..W - 2 {
        let Some(top) = (0..H).find(|&y| !is_sky(img.get_pixel(x, y))) else {
            continue;
        };
        columns += 1;
        let surface_y = (H as f32 * 0.5 - top as f32 - 0.5) * m_per_px;
        let world_x = (x as f32 + 0.5) * m_per_px - width_m * 0.5;
        // The highest point across the strip's width: its near or far edge.
        let d = AMPLITUDE * (waves(world_x) - 0.5);
        let expected = [-0.5, 0.5]
            .map(|e| Vec3::new(world_x, d, e * strip_width).dot(up))
            .into_iter()
            .fold(f32::MIN, f32::max);
        worst = worst.max((surface_y - expected).abs());
    }
    assert!(columns as u32 > W - 8, "the strip doesn't span the view");
    eprintln!(
        "profile: worst {:.2} mm ({:.2} px) off the analytic height; {} triangles",
        worst * 1000.0,
        worst / m_per_px,
        near.triangles
    );
    assert!(worst < 2.5 * m_per_px, "the profile is {worst} m off");

    // Far away (perspective, 80 m), the same strip draws a small fraction of its triangles.
    let mut far = test_app(
        Camera::perspective(40.0),
        DVec3::new(0.0, 20.0, 80.0),
        DVec3::ZERO,
        None,
    );
    let strip = far
        .world
        .resource_mut::<Assets<Mesh>>()
        .add(engine_render::mesh::plane(Vec2::new(2.4, 0.06), 1));
    far.world.spawn((
        Transform::default(),
        MeshRenderer::new(strip, black()),
        wave_displacement(),
    ));
    if !wait_for_clusters(&mut far, 1) {
        return;
    }
    let source = cluster_state(&far)["builds"]["source_triangles"]
        .as_u64()
        .unwrap();
    let far_tris = far.world.resource::<RenderStats>().triangles;
    eprintln!("far: {far_tris} of {source} triangles");
    assert!(
        source > 10_000,
        "only {source} triangles after displacement"
    );
    assert!(
        far_tris * 20 < source,
        "far away it still draws {far_tris} of {source} triangles"
    );
}

#[test]
fn displaced_neighbors_and_mixed_levels_never_crack() {
    // A ceiling of two displaced panels, each its own mesh, meeting at x = 0 (shared border with
    // the same positions, normals and UVs), with nothing but sky above. The camera looks up at
    // it from below at a slant, so the ceiling fills the view and every frame mixes fine
    // clusters overhead with coarse ones in the distance: any sky pixel is a crack.
    let ceiling = 3.0;
    let mut app = test_app(
        Camera::perspective(50.0),
        DVec3::new(-2.0, 1.5, 1.5),
        DVec3::new(-1.0, ceiling as f64, -0.5),
        None,
    );
    sun(&mut app);
    let panel = |x0: f32| {
        let mut m = engine_render::mesh::plane(Vec2::new(5.0, 10.0), 8);
        for v in &mut m.vertices {
            // From x0 to x0 + 5, facing down.
            let p = Vec3::from(v.position);
            v.position = [p.x + x0 + 2.5, ceiling, p.z];
            v.normal = [0.0, -1.0, 0.0];
            v.uv = [v.position[0], v.position[2]];
        }
        for t in m.indices.chunks_mut(3) {
            t.swap(1, 2);
        }
        m
    };
    let rough = Displacement::function(0.08, 43, |p, _, _| {
        0.5 + 0.25 * (p.x * 7.0).sin() * (p.z * 5.0).cos() + 0.25 * (p.x * 13.0 + p.z * 11.0).sin()
    })
    .with_edges(0.03, 0.12);
    for x0 in [-5.0, 0.0] {
        let mesh = app.world.resource_mut::<Assets<Mesh>>().add(panel(x0));
        app.world.spawn((
            Transform::default(),
            MeshRenderer::new(mesh, black()),
            rough.clone(),
        ));
    }
    if !wait_for_clusters(&mut app, 2) {
        return;
    }
    let source = cluster_state(&app)["builds"]["source_triangles"]
        .as_u64()
        .unwrap();
    let mut fewest = u64::MAX;
    for step in 0..6 {
        let s = step as f64;
        // Across the seam, from 0.8 m to 2.3 m under the ceiling.
        let eye = DVec3::new(-1.0 + 0.4 * s, ceiling as f64 - 0.8 - 0.3 * s, 1.5);
        // Looking up 60 degrees: the lowest rays still meet the ceiling a few meters away.
        let rise = ceiling as f64 - eye.y;
        move_camera(
            &mut app,
            eye,
            DVec3::new(eye.x + 0.2 * rise, ceiling as f64, eye.z - 0.55 * rise),
        );
        app.update_n(4);
        let img = capture(&mut app, &format!("vgc_cracks_{step}"));
        let holes = img.pixels().filter(|p| is_sky(p)).count();
        let stats = cluster_state(&app)["stats"].clone();
        eprintln!("step {step}: {holes} sky pixels through the ceiling, {stats}");
        assert_eq!(holes, 0, "step {step}: the sky shows through the ceiling");
        fewest = fewest.min(stats["triangles"].as_u64().unwrap_or(u64::MAX));
    }
    // Some clusters were coarse: mixed levels were on screen.
    assert!(fewest < source / 2, "drew {fewest} of {source} triangles");

    // Without the ceiling the same view is sky (so the check above can see a crack).
    let ceilings: Vec<Entity> = app
        .world
        .query_filtered::<Entity, With<Displacement>>()
        .iter(&app.world)
        .collect();
    for e in ceilings {
        app.world.despawn(e);
    }
    app.update_n(3);
    let img = capture(&mut app, "vgc_cracks_open");
    let sky = img.pixels().filter(|p| is_sky(p)).count();
    assert!(
        sky as u32 > W * H * 9 / 10,
        "only {sky} sky pixels without the ceiling"
    );
}

#[test]
fn builds_load_from_the_disk_cache_on_the_next_start() {
    let dir = scratch("vgc_cache");
    let stone =
        WallStone::new(StoneKind::RoughHewn, Vec3::new(0.35, 0.2, 0.24), 5).with_edge(0.005);
    let run = |name: &str| -> Option<(RgbaImage, serde_json::Value)> {
        let mut app = test_app(
            Camera::perspective(40.0),
            DVec3::new(0.25, 0.25, 0.6),
            DVec3::ZERO,
            Some(&dir),
        );
        sun(&mut app);
        let base = app
            .world
            .resource_mut::<Assets<Mesh>>()
            .add(stone.base_mesh());
        app.world.spawn((
            Transform::default(),
            MeshRenderer::new(base, StandardMaterial::color(1.0, 1.0, 1.0)),
            stone.displacement(),
        ));
        if !wait_for_clusters(&mut app, 1) {
            return None;
        }
        Some((
            capture(&mut app, name),
            cluster_state(&app)["builds"].clone(),
        ))
    };
    let Some((first, built)) = run("vgc_cache_first") else {
        return;
    };
    let (second, loaded) = run("vgc_cache_second").unwrap();
    let files: Vec<_> = std::fs::read_dir(&dir).unwrap().flatten().collect();
    let bytes: u64 = files.iter().map(|f| f.metadata().unwrap().len()).sum();
    eprintln!(
        "first start: {built}\nsecond start: {loaded}\ncache: {} files, {bytes} bytes",
        files.len()
    );
    assert_eq!(built["built"], 1);
    assert_eq!(built["loaded"], 0);
    assert_eq!(loaded["built"], 0, "the second start built again");
    assert_eq!(loaded["loaded"], 1);
    assert_eq!(files.len(), 1);
    assert!(
        loaded["load_seconds"].as_f64().unwrap() < built["build_seconds"].as_f64().unwrap(),
        "loading took longer than building"
    );
    // The very same triangles.
    let differ = differing_pixels(&first, &second);
    assert_eq!(differ, 0, "{differ} pixels differ between the builds");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn wall_stones_are_the_same_for_a_seed() {
    let shot = |seed: u32, name: &str| -> Option<(RgbaImage, RenderStats)> {
        let mut app = test_app(
            Camera::perspective(40.0),
            DVec3::new(0.3, 0.2, 0.7),
            DVec3::ZERO,
            None,
        );
        sun(&mut app);
        let mesh = app
            .world
            .resource_mut::<Assets<Mesh>>()
            .add(engine_render::wall_stone(
                StoneKind::SplitFace,
                Vec3::new(0.4, 0.22, 0.26),
                seed,
            ));
        app.world.spawn((
            Transform::default(),
            MeshRenderer::new(mesh, StandardMaterial::color(1.0, 1.0, 1.0)),
        ));
        if !wait_for_clusters(&mut app, 1) {
            return None;
        }
        Some((
            capture(&mut app, name),
            app.world.resource::<RenderStats>().clone(),
        ))
    };
    let Some((a, stats)) = shot(9, "vgc_stone_9a") else {
        return;
    };
    let (b, _) = shot(9, "vgc_stone_9b").unwrap();
    let (c, _) = shot(10, "vgc_stone_10").unwrap();
    // A finished stone is dense enough to be drawn as clusters without any setup.
    assert!(stats.clusters > 0, "the stone wasn't drawn as clusters");
    assert_eq!(
        differing_pixels(&a, &b),
        0,
        "the same seed made a different stone"
    );
    assert!(
        differing_pixels(&a, &c) > (W * H / 20) as usize,
        "seeds 9 and 10 look alike"
    );
}

/// A small wall: `n` stones in two courses, each a [`WallStone`] base mesh with its
/// displacement, from a palette shared by every wall.
fn spawn_wall(app: &mut App, at: Vec3, palette: &[(WallStone, Handle<Mesh>)]) {
    let material = StandardMaterial::color(1.0, 1.0, 1.0).with_roughness(0.9);
    for i in 0..12 {
        let (stone, mesh) = &palette[i % palette.len()];
        let x = (i % 6) as f32 * 0.42 - 1.05;
        let y = (i / 6) as f32 * 0.24 + 0.11;
        app.world.spawn((
            Transform::from_translation((at + Vec3::new(x, y, 0.0)).as_dvec3()),
            MeshRenderer::new(*mesh, material),
            stone.displacement(),
        ));
    }
}

#[test]
fn distant_walls_cost_few_triangles() {
    let mut app = test_app(
        Camera::perspective(50.0),
        DVec3::new(0.0, 6.0, 40.0),
        DVec3::new(0.0, 0.0, -20.0),
        None,
    );
    sun(&mut app);
    let palette: Vec<(WallStone, Handle<Mesh>)> = (0..3)
        .map(|k| {
            let s = WallStone::new(
                [StoneKind::SplitFace, StoneKind::RoughHewn, StoneKind::Slab][k],
                Vec3::new(0.4, 0.22, 0.26),
                k as u32 + 20,
            )
            .with_edge(0.005);
            let mesh = app.world.resource_mut::<Assets<Mesh>>().add(s.base_mesh());
            (s, mesh)
        })
        .collect();
    spawn_wall(&mut app, Vec3::ZERO, &palette);
    if !wait_for_clusters(&mut app, 3) {
        return;
    }
    let source_per_stone = cluster_state(&app)["builds"]["source_triangles"]
        .as_u64()
        .unwrap()
        / 3;
    let one = app.world.resource::<RenderStats>().triangles;
    // 31 more walls, 30 to 130 m away.
    for k in 1..32 {
        let at = Vec3::new(
            (k % 8) as f32 * 4.0 - 14.0,
            0.0,
            -(k / 8) as f32 * 30.0 - 10.0,
        );
        spawn_wall(&mut app, at, &palette);
    }
    app.update_n(8);
    let many = app.world.resource::<RenderStats>().triangles;
    let instances = cluster_state(&app)["stats"]["instances"].as_u64().unwrap();
    let full = instances * source_per_stone;
    // Each distant stone costs about its coarsest level: a few hundred triangles of its tens of
    // thousands.
    let per_far_stone = (many - one) / (instances - 12);
    eprintln!(
        "1 wall: {one} triangles; 32 walls ({instances} stones, {full} triangles at full \
         detail): {many} triangles, {per_far_stone} per distant stone of {source_per_stone}"
    );
    assert_eq!(instances, 32 * 12);
    assert!(
        many * 100 < full,
        "32 walls draw {many} of {full} triangles"
    );
    assert!(
        per_far_stone * 100 < source_per_stone,
        "a distant stone draws {per_far_stone} of its {source_per_stone} triangles"
    );
}
