//! `MaxDrawDistance`: an entity farther from the camera than its distance is not drawn (nor its
//! shadow), on the instanced path and the virtual geometry cluster path alike. Needs a GPU or
//! lavapipe; passes with a note when neither is available.

use std::time::{Duration, Instant};

use engine_core::prelude::*;
use engine_render::prelude::*;
use engine_render::{RenderDisabled, RenderStats, Renderer, capture_now};
use image::RgbaImage;

const W: u32 = 160;
const H: u32 = 90;

/// A camera 40 m up and `distance` meters back from a red ball (1 m radius) above a gray
/// floor, the sun nearly overhead.
fn app_with_ball(distance: f64, mesh: impl FnOnce(&mut World) -> MeshRef, max: Option<f32>) -> App {
    engine_core::log::init("error");
    let mut app = App::new();
    app.world.resource_mut::<EngineConfig>().headless = true;
    {
        let mut w = app.world.resource_mut::<Window>();
        w.width = W;
        w.height = H;
    }
    app.add_plugins(RenderPlugin);
    app.world.resource_mut::<RenderSettings>().anti_aliasing = AntiAliasing::None;
    let w = &mut app.world;
    let mesh = mesh(w);
    w.spawn((
        Camera {
            exposure: Exposure::Manual { ev100: 14.5 },
            ..Camera::perspective(4.0)
        },
        Transform::from_xyz(0.0, 40.0, distance).looking_at(DVec3::new(0.0, 1.0, 0.0), Vec3::Y),
    ));
    w.spawn((
        DirectionalLight::default(),
        Transform::default().looking_at(DVec3::new(0.1, -1.0, -0.1), Vec3::Y),
    ));
    w.spawn((
        Transform::default(),
        MeshRenderer::new(
            Shape::Plane {
                size: [40.0, 40.0],
                subdivisions: 1,
            },
            StandardMaterial::color(0.5, 0.5, 0.5),
        ),
    ));
    let mut ball = w.spawn((
        Transform::from_xyz(0.0, 1.2, 0.0),
        MeshRenderer::new(mesh, StandardMaterial::color(0.9, 0.05, 0.02)),
    ));
    if let Some(m) = max {
        ball.insert(MaxDrawDistance(m));
    }
    app
}

fn capture(app: &mut App, name: &str) -> RgbaImage {
    let path = std::path::Path::new(env!("CARGO_TARGET_TMPDIR")).join(format!("{name}.png"));
    capture_now(&mut app.world, &path).expect("screenshot");
    image::open(&path).expect("png").to_rgba8()
}

/// Red pixels (the ball) and dark pixels (its shadow on the floor).
fn ball_and_shadow(img: &RgbaImage) -> (usize, usize) {
    let red = |p: &image::Rgba<u8>| p[0] as i32 > p[1] as i32 + 60;
    let luma = |p: &image::Rgba<u8>| p[0] as u32 + p[1] as u32 + p[2] as u32;
    let floor = luma(img.get_pixel(5, H - 5));
    let dark = img
        .pixels()
        .filter(|p| !red(p) && luma(p) * 2 < floor)
        .count();
    (img.pixels().filter(|p| red(p)).count(), dark)
}

fn sphere(_: &mut World) -> MeshRef {
    Shape::Sphere {
        radius: 1.0,
        segments: 32,
    }
    .into()
}

#[test]
fn max_draw_distance_hides_the_mesh_and_its_shadow() {
    let mut cases = Vec::new();
    for (name, max) in [
        ("none", None),
        ("near", Some(60.0)),
        ("beyond", Some(200.0)),
    ] {
        let mut app = app_with_ball(100.0, sphere, max);
        app.update();
        if let Some(d) = app.world.get_resource::<RenderDisabled>() {
            eprintln!("skipping: no GPU ({})", d.0);
            return;
        }
        app.update_n(2);
        let img = capture(&mut app, &format!("draw_distance_{name}"));
        let stats = app.world.resource::<RenderStats>().clone();
        cases.push((name, ball_and_shadow(&img), stats.culled));
    }
    eprintln!("(red px, shadow px, culled): {cases:?}");
    let [(_, none, _), (_, near, near_culled), (_, beyond, _)] = cases[..] else {
        unreachable!()
    };
    assert!(none.0 > 50, "the ball isn't in view: {none:?}");
    assert_eq!(near.0, 0, "a ball beyond its MaxDrawDistance is drawn");
    assert!(near_culled >= 1, "the ball wasn't counted as culled");
    assert!(
        near.1 + 20 < none.1,
        "the hidden ball still casts its shadow: {near:?} vs {none:?}"
    );
    assert_eq!(
        beyond.0, none.0,
        "a ball within its MaxDrawDistance is hidden"
    );
}

/// The camera's virtual geometry clusters this frame (read back from the GPU).
fn clusters_drawn(app: &App) -> u64 {
    app.world.resource::<RenderStats>().clusters as u64
}

fn clusters_ready(app: &App) -> u64 {
    let describe = app.world.resource::<Renderer>().describe();
    describe["features"]
        .as_array()
        .and_then(|f| f.iter().find(|f| f["name"] == "meshes"))
        .and_then(|f| f["state"]["virtual_geometry"]["meshes_ready"].as_u64())
        .unwrap_or(0)
}

#[test]
fn max_draw_distance_hides_clustered_meshes() {
    // A dense ball (81 920 triangles) takes the cluster path.
    let dense = |w: &mut World| -> MeshRef {
        let mesh = engine_render::mesh::icosphere(1.0, 6);
        w.resource_mut::<engine_assets::Assets<Mesh>>()
            .add(mesh)
            .into()
    };
    let mut app = app_with_ball(50.0, dense, Some(80.0));
    app.update();
    if let Some(d) = app.world.get_resource::<RenderDisabled>() {
        eprintln!("skipping: no GPU ({})", d.0);
        return;
    }
    let started = Instant::now();
    while clusters_ready(&app) < 1 {
        assert!(
            started.elapsed() < Duration::from_secs(300),
            "the cluster build didn't finish"
        );
        std::thread::sleep(Duration::from_millis(20));
        app.update();
    }
    app.update_n(6);
    let near = capture(&mut app, "draw_distance_clusters_near");
    let near_clusters = clusters_drawn(&app);
    // Step back past the ball's draw distance.
    let mut q = app.world.query_filtered::<&mut Transform, With<Camera>>();
    for mut t in q.iter_mut(&mut app.world) {
        *t = Transform::from_xyz(0.0, 40.0, 100.0).looking_at(DVec3::new(0.0, 1.0, 0.0), Vec3::Y);
    }
    app.update_n(6);
    let far = capture(&mut app, "draw_distance_clusters_far");
    let far_clusters = clusters_drawn(&app);
    let (near_red, far_red) = (ball_and_shadow(&near).0, ball_and_shadow(&far).0);
    eprintln!(
        "clusters near {near_clusters} ({near_red} red px), far {far_clusters} ({far_red} red px)"
    );
    assert!(
        near_clusters > 0 && near_red > 50,
        "the clustered ball isn't drawn up close"
    );
    assert_eq!(far_clusters, 0, "clusters drawn beyond the draw distance");
    assert_eq!(
        far_red, 0,
        "the clustered ball is drawn beyond its draw distance"
    );
}
