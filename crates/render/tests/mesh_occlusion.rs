//! GPU occlusion culling of instanced meshes (`RenderSettings::mesh_occlusion_culling`): a
//! crowd of crates hidden behind a wall isn't drawn, the ones beside it are, and the picture
//! is the same as without culling, still and while the camera walks around the wall's end
//! (the late phase catches what last frame's depth hid). Needs a GPU or lavapipe; passes with
//! a note when neither is available.

use engine_core::prelude::*;
use engine_render::prelude::*;
use engine_render::{RenderDisabled, capture_now};

const W: u32 = 160;
const H: u32 = 90;

fn app(culling: bool) -> App {
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
        s.mesh_occlusion_culling = culling;
        s.anti_aliasing = AntiAliasing::None;
        s.film_grain = 0.0;
        s.probe_gi = false;
    }
    let w = &mut app.world;
    w.spawn((
        Camera {
            exposure: Exposure::Manual { ev100: 12.0 },
            ..Camera::perspective(60.0)
        },
        Transform::from_xyz(0.0, 2.0, 0.0).looking_at(DVec3::new(0.0, 2.0, -50.0), Vec3::Y),
    ));
    w.spawn((
        DirectionalLight::default(),
        Transform::default().looking_at(-DVec3::new(0.3, 0.8, 0.4).normalize(), Vec3::Y),
    ));
    w.spawn((
        Transform::default(),
        MeshRenderer::new(
            Shape::Plane {
                size: [400.0, 400.0],
                subdivisions: 1,
            },
            StandardMaterial::color(0.3, 0.35, 0.25),
        ),
    ));
    // The wall: 30 m wide, 12 m high, 15 m ahead, hiding most of the view straight on.
    w.spawn((
        Transform::from_xyz(-3.0, 6.0, -15.0),
        MeshRenderer::new(
            Shape::Cuboid {
                size: [30.0, 12.0, 1.0],
            },
            StandardMaterial::color(0.6, 0.55, 0.5),
        ),
    ));
    // 200 crates behind it, and a row of 10 beyond its right end (seen).
    for i in 0..200 {
        let (x, z) = ((i % 20) as f64 * 1.2 - 14.0, -20.0 - (i / 20) as f64 * 1.5);
        w.spawn((
            Transform::from_xyz(x, 0.5, z),
            MeshRenderer::new(
                Shape::Cuboid {
                    size: [0.8, 1.0, 0.8],
                },
                StandardMaterial::color(0.7, 0.3, 0.2),
            ),
        ));
    }
    for i in 0..10 {
        w.spawn((
            Transform::from_xyz(14.0 + i as f64 * 1.5, 0.5, -25.0),
            MeshRenderer::new(
                Shape::Cuboid {
                    size: [0.8, 1.0, 0.8],
                },
                StandardMaterial::color(0.2, 0.4, 0.7),
            ),
        ));
    }
    app
}

fn shot(app: &mut App, name: &str) -> image::RgbaImage {
    let path = std::path::Path::new(env!("CARGO_TARGET_TMPDIR")).join(format!("{name}.png"));
    capture_now(&mut app.world, &path).expect("screenshot");
    image::open(&path).expect("png").to_rgba8()
}

fn differing(a: &image::RgbaImage, b: &image::RgbaImage) -> usize {
    a.pixels()
        .zip(b.pixels())
        .filter(|(p, q)| (0..3).any(|c| (p[c] as i32 - q[c] as i32).abs() > 2))
        .count()
}

/// Walk the camera to the right, past the wall's end, one step a frame.
fn walk(app: &mut App, step: usize) {
    let mut q = app.world.query::<(&Camera, &mut Transform)>();
    for (_, mut t) in q.iter_mut(&mut app.world) {
        let x = step as f64 * 1.5;
        *t = Transform::from_xyz(x, 2.0, 0.0).looking_at(DVec3::new(x, 2.0, -50.0), Vec3::Y);
    }
}

#[test]
fn hidden_crates_are_skipped_and_the_picture_is_the_same() {
    let mut on = app(true);
    on.update_n(8);
    if let Some(d) = on.world.get_resource::<RenderDisabled>() {
        eprintln!("skipping: no GPU ({})", d.0);
        return;
    }
    let mut off = app(false);
    off.update_n(8);
    let (a, b) = (
        shot(&mut on, "occlusion_on"),
        shot(&mut off, "occlusion_off"),
    );
    let still = differing(&a, &b);
    let d = on.world.resource::<Diagnostics>();
    let tested = d.get("render.meshes.occlusion_tested").unwrap_or(0.0);
    let drawn = d.get("render.meshes.occlusion_drawn").unwrap_or(0.0);
    eprintln!("still: {still} pixels differ; tested {tested}, drawn {drawn}");
    assert_eq!(still, 0, "culling changed {still} pixels");
    // The 200 behind the wall are tested and skipped; the ground, the wall and the 10 beside
    // it are drawn.
    assert!(tested >= 210.0, "tested {tested}");
    assert!(drawn <= 40.0, "drew {drawn} of {tested}");

    // Walking past the wall's end, every frame matches (what comes into view is drawn the
    // frame it does).
    for step in 1..=12 {
        walk(&mut on, step);
        walk(&mut off, step);
        on.update();
        off.update();
        let (a, b) = (
            shot(&mut on, &format!("occlusion_on_{step}")),
            shot(&mut off, &format!("occlusion_off_{step}")),
        );
        let n = differing(&a, &b);
        assert_eq!(n, 0, "step {step}: culling changed {n} pixels");
    }
}
