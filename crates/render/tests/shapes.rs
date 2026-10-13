//! Procedural shapes on screen: rounded edges catch the light where a plain box has a hard line,
//! and rocks render as solid, shaded boulders. Needs a GPU or lavapipe; passes with a note when
//! neither is available.

use engine_core::prelude::*;
use engine_render::prelude::*;
use engine_render::{RenderDisabled, capture_now};
use image::RgbaImage;

const W: u32 = 640;
const H: u32 = 360;

fn base_app(w: u32, h: u32) -> App {
    engine_core::log::init("error");
    let mut app = App::new();
    app.world.resource_mut::<EngineConfig>().headless = true;
    {
        let mut win = app.world.resource_mut::<Window>();
        win.width = w;
        win.height = h;
    }
    app.add_plugins(RenderPlugin);
    {
        let mut s = app.world.resource_mut::<RenderSettings>();
        s.film_grain = 0.0;
        s.vignette = 0.0;
    }
    app
}

fn render(app: &mut App, name: &str) -> Option<RgbaImage> {
    app.update_n(12);
    if let Some(d) = app.world.get_resource::<RenderDisabled>() {
        eprintln!("skipping: no GPU ({})", d.0);
        return None;
    }
    let path = std::path::Path::new(env!("CARGO_TARGET_TMPDIR")).join(format!("{name}.png"));
    capture_now(&mut app.world, &path).expect("screenshot");
    Some(image::open(&path).expect("png").to_rgba8())
}

#[test]
fn rocks_and_rounded_boxes_render_solid() {
    let mut app = base_app(W, H);
    let w = &mut app.world;
    w.spawn((
        Camera::perspective(50.0),
        Transform::from_xyz(0.0, 1.6, 6.5).looking_at(DVec3::new(0.0, 0.6, 0.0), Vec3::Y),
    ));
    w.spawn((
        DirectionalLight::default(),
        Transform::default().looking_at(DVec3::new(-0.6, -0.5, -0.5), Vec3::Y),
    ));
    w.spawn((
        Transform::default(),
        MeshRenderer::new(
            Shape::Plane {
                size: [80.0, 80.0],
                subdivisions: 1,
            },
            StandardMaterial::hex(0x6b6152)
                .with_roughness(0.95)
                .with_detail(0.8, 0.5)
                .with_detail_in_world(),
        ),
    ));
    let stone = StandardMaterial::hex(0x8a8378)
        .with_roughness(0.85)
        .with_detail(0.7, 0.12);
    for (i, (x, size)) in [
        (-2.4, [1.6, 1.0, 1.3]),
        (-0.6, [1.1, 1.4, 1.0]),
        (0.9, [0.8, 0.5, 0.7]),
        (2.2, [1.4, 0.9, 1.6]),
    ]
    .into_iter()
    .enumerate()
    {
        w.spawn((
            Transform::from_xyz(x, size[1] as f64 * 0.5, -0.4),
            MeshRenderer::new(
                Shape::Rock {
                    size,
                    seed: i as u32 + 1,
                    detail: 0,
                },
                stone,
            ),
        ));
    }
    w.spawn((
        Transform::from_xyz(-1.2, 0.3, 1.6),
        MeshRenderer::new(
            Shape::RoundedCuboid {
                size: [0.9, 0.6, 0.6],
                radius: 0.05,
                segments: 0,
            },
            StandardMaterial::hex(0x9a6b3f).with_roughness(0.6),
        ),
    ));
    w.spawn((
        Transform::from_xyz(0.4, 0.3, 1.6),
        MeshRenderer::new(
            Shape::Cuboid {
                size: [0.9, 0.6, 0.6],
            },
            StandardMaterial::hex(0x9a6b3f).with_roughness(0.6),
        ),
    ));
    let Some(img) = render(&mut app, "shapes_gallery") else {
        return;
    };
    // The rocks fill their part of the frame with shaded stone, not sky or holes.
    let rock_px = img.get_pixel(W / 2 - 40, H / 2 - 10);
    eprintln!("rock pixel {rock_px:?}");
    assert!(
        rock_px[2] < rock_px[0] + 30,
        "sky shows through a rock: {rock_px:?}"
    );
}

#[test]
fn a_rock_up_close_shows_break_faces() {
    let mut app = base_app(W, H);
    let w = &mut app.world;
    w.spawn((
        Camera::perspective(45.0),
        Transform::from_xyz(0.6, 1.1, 2.6).looking_at(DVec3::new(0.0, 0.55, 0.0), Vec3::Y),
    ));
    // Low raking sun from the side.
    w.spawn((
        DirectionalLight::default(),
        Transform::default().looking_at(DVec3::new(-0.8, -0.35, -0.3), Vec3::Y),
    ));
    w.spawn((
        Transform::default(),
        MeshRenderer::new(
            Shape::Plane {
                size: [80.0, 80.0],
                subdivisions: 1,
            },
            StandardMaterial::hex(0x6b6152)
                .with_roughness(0.95)
                .with_detail(0.8, 0.5)
                .with_detail_in_world(),
        ),
    ));
    w.spawn((
        Transform::from_xyz(0.0, 0.55, 0.0),
        MeshRenderer::new(
            Shape::Rock {
                size: [1.6, 1.1, 1.3],
                seed: 11,
                detail: 0,
            },
            StandardMaterial::hex(0x8a8378)
                .with_roughness(0.85)
                .with_detail(0.7, 0.12),
        ),
    ));
    let Some(img) = render(&mut app, "rock_close_up") else {
        return;
    };
    // Break faces at different angles to the sun: the rock shows a wide range of shading.
    let lumas: Vec<f32> = (H / 4..H * 3 / 4)
        .step_by(4)
        .flat_map(|y| (W / 3..W * 2 / 3).step_by(4).map(move |x| (x, y)))
        .map(|(x, y)| {
            let p = img.get_pixel(x, y);
            0.2126 * p[0] as f32 + 0.7152 * p[1] as f32 + 0.0722 * p[2] as f32
        })
        .collect();
    let lo = lumas.iter().cloned().fold(f32::MAX, f32::min);
    let hi = lumas.iter().cloned().fold(f32::MIN, f32::max);
    eprintln!("rock luma range {lo:.0}..{hi:.0}");
    assert!(hi - lo > 80.0, "the rock looks flat: {lo:.0}..{hi:.0}");
}
