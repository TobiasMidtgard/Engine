//! Renders a skinned character offscreen and checks the picture: the GPU skinning pass draws
//! the mesh with its paint, and the drawn pose follows the animation (a flex raises the
//! arms). Needs a GPU or a software Vulkan driver (lavapipe); passes with a note when neither
//! is available (CI sets ENGINE_REQUIRE_GPU=1, which turns that into a failure).

use engine_anim::AnimationPlugin;
use engine_anim::build::humanoid::Outfit;
use engine_anim::demo::spawn_humanoid;
use engine_core::prelude::*;
use engine_core::time::TimeSource;
use engine_render::prelude::*;
use engine_render::{RenderDisabled, capture_now};
use engine_world::WorldPlugin;
use engine_world::animation::AnimationTrigger;

const W: u32 = 320;
const H: u32 = 240;

fn capture(app: &mut App, name: &str) -> image::RgbaImage {
    let path = std::path::Path::new(env!("CARGO_TARGET_TMPDIR")).join(name);
    capture_now(&mut app.world, &path).expect("screenshot");
    image::open(&path).expect("png").to_rgba8()
}

/// Pixels of the athlete's red tank top inside a box (fractions of the image).
fn red_pixels(img: &image::RgbaImage, x0: f32, x1: f32, y0: f32, y1: f32) -> usize {
    let (w, h) = (img.width() as f32, img.height() as f32);
    let mut n = 0;
    for y in (y0 * h) as u32..(y1 * h) as u32 {
        for x in (x0 * w) as u32..(x1 * w) as u32 {
            let p = img.get_pixel(x, y);
            let (r, g, b) = (p[0] as f32, p[1] as f32, p[2] as f32);
            if r > 40.0 && r > g * 1.6 && r > b * 1.6 {
                n += 1;
            }
        }
    }
    n
}

/// Skin-colored pixels (the bare arms) in a box.
fn skin_pixels(img: &image::RgbaImage, x0: f32, x1: f32, y0: f32, y1: f32) -> usize {
    let (w, h) = (img.width() as f32, img.height() as f32);
    let mut n = 0;
    for y in (y0 * h) as u32..(y1 * h) as u32 {
        for x in (x0 * w) as u32..(x1 * w) as u32 {
            let p = img.get_pixel(x, y);
            let (r, g, b) = (p[0] as f32, p[1] as f32, p[2] as f32);
            if r > 60.0 && r > g * 1.1 && g > b * 1.05 && r < g * 1.8 {
                n += 1;
            }
        }
    }
    n
}

#[test]
fn skinned_character_renders_and_follows_the_pose() {
    let mut app = App::new();
    app.world.resource_mut::<EngineConfig>().headless = true;
    {
        let mut w = app.world.resource_mut::<Window>();
        w.width = W;
        w.height = H;
    }
    app.world
        .resource_mut::<Time>()
        .set_source(TimeSource::Manual {
            step_secs: 1.0 / 60.0,
        });
    app.add_plugins((WorldPlugin, RenderPlugin, AnimationPlugin));
    let w = &mut app.world;
    w.spawn((
        DirectionalLight {
            illuminance: 80_000.0,
            ..Default::default()
        },
        Transform::default().looking_at(DVec3::new(-0.3, -0.7, -0.6), Vec3::Y),
    ));
    w.spawn((
        Transform::default(),
        MeshRenderer::new(
            Shape::Plane {
                size: [40.0, 40.0],
                subdivisions: 1,
            },
            StandardMaterial::hex(0x6b7b5a).with_roughness(0.95),
        ),
    ));
    w.spawn((
        Camera::perspective(40.0),
        Transform::from_xyz(0.0, 1.15, 3.6).looking_at(DVec3::new(0.0, 1.05, 0.0), Vec3::Y),
    ));
    // Facing the camera (+Z).
    let athlete = spawn_humanoid(
        w,
        Outfit::Athlete,
        Transform::from_rotation(Quat::from_rotation_y(std::f32::consts::PI)),
    );
    app.update_n(10);
    if let Some(d) = app.world.get_resource::<RenderDisabled>() {
        eprintln!("skipping: no GPU ({})", d.0);
        return;
    }
    let idle = capture(&mut app, "anim_idle.png");
    let stats = app.world.resource::<engine_render::RenderStats>().clone();
    assert!(
        stats.features.iter().any(|f| f.contains("skin")),
        "skinning feature missing: {:?}",
        stats.features
    );
    // The tank top fills the middle of the picture.
    let top = red_pixels(&idle, 0.4, 0.6, 0.25, 0.55);
    assert!(top > 400, "tank top not drawn ({top} red pixels)");

    // Double-biceps: the fists come up beside the head.
    app.world.write_message(AnimationTrigger {
        entity: athlete,
        name: "flex".into(),
    });
    app.update_n(90);
    let flex = capture(&mut app, "anim_flex.png");
    let above_shoulders = |img: &image::RgbaImage| {
        skin_pixels(img, 0.2, 0.42, 0.05, 0.3) + skin_pixels(img, 0.58, 0.8, 0.05, 0.3)
    };
    let (before, after) = (above_shoulders(&idle), above_shoulders(&flex));
    assert!(
        after > before + 150,
        "arms did not come up: {before} skin pixels beside the head at idle, {after} flexing"
    );
}
