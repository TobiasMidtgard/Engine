//! Swaying meshes on the cluster path (virtual geometry): a dense mesh with a `wind_sway`
//! material and a bone plant (`PlantWindMeshes`) draw as clusters, sway exactly as instances
//! do, and are not culled where the wind carries them into view while they stand just outside
//! it at rest. Needs a GPU or lavapipe; passes with a note when neither is available.

use std::sync::Arc;
use std::time::{Duration, Instant};

use engine_core::prelude::*;
use engine_render::prelude::*;
use engine_render::{
    NoMeshLod, PlantBone, PlantSkeleton, PlantSpeciesWind, PlantWindMesh, PlantWindMeshes,
    PlantWindSpecies, RenderDisabled, RenderStats, Renderer, Vertex, capture_now,
    pack_plant_vertex,
};
use image::RgbaImage;

const W: u32 = 200;
const H: u32 = 150;
/// Post height (m) and where the bone post's limb turns.
const HEIGHT: f32 = 8.0;
const LIMB: f32 = 4.0;

/// An open tube `HEIGHT` tall of `rings` x `segments` quads (2 triangles each), its base at
/// y = 0; `alpha(y)` is each vertex's color alpha (wind weight, or packed bone).
fn post(rings: u32, segments: u32, alpha: impl Fn(f32) -> f32) -> Mesh {
    let radius = 0.3;
    let mut vertices = Vec::new();
    for r in 0..=rings {
        let y = HEIGHT * r as f32 / rings as f32;
        for s in 0..=segments {
            let a = std::f32::consts::TAU * s as f32 / segments as f32;
            let n = Vec3::new(a.cos(), 0.0, a.sin());
            let mut v = Vertex::new(
                n * radius + Vec3::Y * y,
                n,
                Vec2::new(s as f32 / segments as f32, y / HEIGHT),
            );
            v.color = [1.0, 1.0, 1.0, alpha(y)];
            vertices.push(v);
        }
    }
    let mut indices = Vec::new();
    let row = segments + 1;
    for r in 0..rings {
        for s in 0..segments {
            let a = r * row + s;
            let b = a + row;
            // Counter-clockwise seen from outside.
            indices.extend([a, b, a + 1, a + 1, b, b + 1]);
        }
    }
    Mesh::new(vertices, indices)
}

/// The trunk (to `LIMB`) and a limb above it, pointing up.
fn skeleton() -> PlantSkeleton {
    PlantSkeleton {
        bones: vec![
            PlantBone {
                pivot: Vec3::ZERO,
                length: HEIGHT,
                axis: Vec3::Y,
                face: Vec3::X,
                parent: None,
                level: 0,
                phase: 0.0,
                stiffness: 1.0,
            },
            PlantBone {
                pivot: Vec3::Y * LIMB,
                length: HEIGHT - LIMB,
                axis: Vec3::Y,
                face: Vec3::Z,
                parent: Some(0),
                level: 1,
                phase: 0.0,
                stiffness: 0.5,
            },
        ],
    }
}

/// The view, sun and ground every scene shares. `clusters`: dense meshes are drawn as
/// clusters (else every mesh is instanced).
fn base_app(clusters: bool) -> App {
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
        s.anti_aliasing = AntiAliasing::None;
        s.film_grain = 0.0;
        s.bloom_intensity = 0.0;
        s.vignette = 0.0;
        s.tonemapper = Tonemapper::None;
        s.volumetric_fog = false;
        s.probe_gi = false;
        s.virtual_geometry = clusters;
        s.virtual_geometry_min_triangles = 1000;
        // Nearly full detail, so clusters and instances draw the same triangles.
        s.cluster_error_px = 0.05;
        s.cluster_shadow_error_texels = 0.05;
    }
    app.world.resource_mut::<FogSettings>().density = 0.0;
    app.world
        .resource_mut::<Time>()
        .set_source(engine_core::time::TimeSource::Manual { step_secs: 0.05 });
    let w = &mut app.world;
    w.spawn((
        Camera {
            exposure: Exposure::Manual { ev100: 13.0 },
            ..Camera::perspective(50.0)
        },
        Transform::from_xyz(0.0, 4.0, 14.0).looking_at(DVec3::new(0.0, 4.0, 0.0), Vec3::Y),
    ));
    w.spawn((
        DirectionalLight::default(),
        Transform::default().looking_at(DVec3::new(-0.4, -0.8, 0.45), Vec3::Y),
    ));
    w.spawn((
        Transform::default(),
        MeshRenderer::new(
            Shape::Plane {
                size: [80.0, 80.0],
                subdivisions: 1,
            },
            StandardMaterial::color(0.5, 0.5, 0.5).with_roughness(1.0),
        ),
    ));
    app
}

/// Two dense posts just left of the view in a 15 m/s wind along +x, which carries their
/// tops into it: a red one on the legacy sway (3 m at its top) and a blue bone plant.
fn posts_app(clusters: bool) -> App {
    let mut app = base_app(clusters);
    app.world.insert_resource(engine_world::WindField {
        prevailing: Vec3::new(15.0, 0.0, 0.0),
        gustiness: 0.0,
        ..Default::default()
    });
    let (legacy, bones) = {
        let mut meshes = app.world.resource_mut::<engine_assets::Assets<Mesh>>();
        (
            meshes.add(post(60, 24, |_| 1.0)),
            meshes.add(post(60, 24, |y| {
                let (bone, tip) = if y < LIMB {
                    (0, y / HEIGHT)
                } else {
                    (1, (y - LIMB) / (HEIGHT - LIMB))
                };
                pack_plant_vertex(bone, tip, 0.0, false)
            })),
        )
    };
    let mut plants = PlantWindMeshes::default();
    plants.insert(
        bones,
        PlantWindMesh {
            skeleton: Arc::new(skeleton()),
            species: 0,
            detail: 3,
        },
    );
    app.world.insert_resource(plants);
    app.world.insert_resource(PlantWindSpecies {
        slots: vec![PlantSpeciesWind {
            bend_deg: [40.0, 40.0, 40.0],
            gale_lean: 0.08,
            ..Default::default()
        }],
    });
    let w = &mut app.world;
    // The view's left edge is about 8.7 m left of its axis 14 m away and 10.6 m 17 m away:
    // each post stands about 0.5 m outside it.
    let mut red = StandardMaterial::color(0.9, 0.12, 0.08);
    red.wind_sway = 3.0;
    w.spawn((
        Transform::from_xyz(-9.5, 0.0, 0.0),
        MeshRenderer::new(legacy, red),
        NoMeshLod,
    ));
    let mut blue = StandardMaterial::color(0.08, 0.2, 0.9);
    // Bone plants keep a sway (the passes treat them as moving); its value is unused.
    blue.wind_sway = 1.0;
    w.spawn((
        Transform::from_xyz(-11.4, 0.0, -3.0),
        MeshRenderer::new(bones, blue),
        NoMeshLod,
    ));
    app
}

/// A dense upright screen (6 x 4 m, 3200 triangles) of an alpha-masked checker, holes and
/// all, its shadow falling on the ground toward the view.
fn screen_app(clusters: bool) -> App {
    let mut app = base_app(clusters);
    // 8 x 8 texels a meter: every other 2 x 2 block a hole.
    let mut data = Vec::new();
    for y in 0..8u32 {
        for x in 0..8u32 {
            let hole = (x / 2 + y / 2) % 2 == 0;
            data.extend_from_slice(&[230, 200, 60, if hole { 0 } else { 255 }]);
        }
    }
    let image = app
        .world
        .resource_mut::<engine_assets::Assets<Image>>()
        .add(Image::new(8, 8, data, true));
    let mut material = StandardMaterial::color(1.0, 1.0, 1.0).with_base_color_texture(image);
    material.alpha_mode = AlphaMode::Mask(0.5);
    material.double_sided = true;
    let mesh = app.world.resource_mut::<engine_assets::Assets<Mesh>>().add(
        Shape::Plane {
            size: [6.0, 4.0],
            subdivisions: 39,
        }
        .mesh(),
    );
    app.world.spawn((
        Transform::from_xyz(0.0, 2.5, -2.0)
            .with_rotation(Quat::from_rotation_x(std::f32::consts::FRAC_PI_2)),
        MeshRenderer::new(mesh, material),
    ));
    app
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

fn capture(app: &mut App, name: &str) -> RgbaImage {
    let path = std::path::Path::new(env!("CARGO_TARGET_TMPDIR")).join(format!("{name}.png"));
    capture_now(&mut app.world, &path).expect("screenshot");
    image::open(&path).expect("png").to_rgba8()
}

/// Mean largest channel difference (0..255) and the share of pixels differing by more than 24.
fn difference(a: &RgbaImage, b: &RgbaImage) -> (f32, f32) {
    let (mut sum, mut far) = (0u64, 0u32);
    for (p, q) in a.pixels().zip(b.pixels()) {
        let d = (0..3).map(|c| p[c].abs_diff(q[c]) as u64).max().unwrap();
        sum += d;
        far += (d > 24) as u32;
    }
    let n = (W * H) as f32;
    (sum as f32 / n, far as f32 / n)
}

/// Pixels where `pick` holds (red: the legacy post; blue: the bone post).
fn count(img: &RgbaImage, pick: impl Fn(i32, i32, i32) -> bool) -> u32 {
    img.pixels()
        .filter(|p| pick(p[0] as i32, p[1] as i32, p[2] as i32))
        .count() as u32
}

/// The scene drawn as clusters, once `ready` meshes are built, and the same frames (the same
/// moment of the sway) with every mesh instanced; and the cluster state. None without a GPU.
fn both(
    make: fn(bool) -> App,
    ready: u64,
    name: &str,
) -> Option<(RgbaImage, RgbaImage, serde_json::Value, RenderStats)> {
    let mut clustered = make(true);
    clustered.update();
    if let Some(d) = clustered.world.get_resource::<RenderDisabled>() {
        eprintln!("skipping: no GPU ({})", d.0);
        return None;
    }
    // Built in the background, then a few frames so the counters are read back.
    let started = Instant::now();
    let mut frames = 1;
    while cluster_state(&clustered)["meshes_ready"]
        .as_u64()
        .unwrap_or(0)
        < ready
    {
        assert!(
            started.elapsed() < Duration::from_secs(300),
            "cluster builds didn't finish: {}",
            cluster_state(&clustered)
        );
        std::thread::sleep(Duration::from_millis(20));
        clustered.update();
        frames += 1;
    }
    clustered.update_n(6);
    frames += 6;
    let state = cluster_state(&clustered);
    let stats = clustered.world.resource::<RenderStats>().clone();
    let with_clusters = capture(&mut clustered, &format!("{name}_clusters"));
    let mut instanced = make(false);
    instanced.update_n(frames);
    let reference = capture(&mut instanced, &format!("{name}_instanced"));
    Some((with_clusters, reference, state, stats))
}

#[test]
fn swaying_posts_draw_as_clusters_where_the_wind_carries_them() {
    let Some((with_clusters, reference, state, stats)) = both(posts_app, 2, "swaying") else {
        return;
    };
    let red = |r: i32, g: i32, b: i32| r > g + 40 && r > b + 40;
    let blue = |r: i32, g: i32, b: i32| b > r + 40 && b > g + 20;
    let (reds, blues) = (count(&reference, red), count(&reference, blue));
    let (mean, share) = difference(&with_clusters, &reference);
    eprintln!(
        "{} clusters drawn, swaying instances {}; post pixels instanced: red {reds}, blue \
         {blues}; clustered: red {}, blue {}; mean difference {mean:.3}, {:.3}% far",
        stats.clusters,
        state["stats"]["swaying"],
        count(&with_clusters, red),
        count(&with_clusters, blue),
        share * 100.0
    );
    assert_eq!(
        state["stats"]["swaying"].as_u64(),
        Some(2),
        "both posts should sway as clusters: {state}"
    );
    assert!(stats.clusters > 0, "nothing was drawn as clusters");
    // The wind carries both tops into view.
    assert!(
        reds > 40 && blues > 40,
        "the posts' tops should show: red {reds}, blue {blues}"
    );
    assert!(
        mean < 1.0 && share < 0.005,
        "clusters drew another picture than instances (mean {mean}, {share} far)"
    );
}

#[test]
fn masked_clusters_cut_out_their_holes_and_their_shadows_holes() {
    let Some((with_clusters, reference, state, stats)) = both(screen_app, 1, "masked") else {
        return;
    };
    let (mean, share) = difference(&with_clusters, &reference);
    // Sunlit ground seen through the holes and through the shadow's holes: the checker shows
    // as many bright and dark ground pixels (the screen's yellow aside).
    let yellow = |r: i32, g: i32, b: i32| r > b + 60 && g > b + 40;
    eprintln!(
        "{} clusters drawn, shadow buckets {}; yellow pixels instanced {}, clustered {}; mean \
         difference {mean:.3}, {:.3}% far",
        stats.clusters,
        state["shadow_buckets"],
        count(&reference, yellow),
        count(&with_clusters, yellow),
        share * 100.0
    );
    assert!(stats.clusters > 0, "nothing was drawn as clusters");
    assert_eq!(
        state["shadow_buckets"].as_u64(),
        Some(1),
        "the screen should cast alpha-tested: {state}"
    );
    assert!(
        mean < 1.0 && share < 0.005,
        "clusters drew another picture than instances (mean {mean}, {share} far)"
    );
}
