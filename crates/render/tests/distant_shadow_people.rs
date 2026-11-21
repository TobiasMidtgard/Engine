//! Distant shadows (the screen-space march past the shadow cascades) and thin casters: a person
//! standing past the cascades' reach shades the ground where their shadow falls, and not the
//! ground behind them on screen. The march took every caster to be at least 3 m deep behind its
//! front face, so the ground behind a person, whose rays toward the sun pass a metre or two
//! behind their back, went dark: a long rectangular slab on the ground behind everyone (seen in
//! the character creator's daylight scene, its cascades ending at 6 m). Needs a GPU or lavapipe;
//! passes with a note when neither is available.

use engine_core::prelude::*;
use engine_render::prelude::*;
use engine_render::{RenderDisabled, Renderer, capture_now};
use image::RgbaImage;

const W: u32 = 480;
const H: u32 = 270;
/// The person: a 0.5 x 1.8 x 0.3 m box 12 m in front of the camera.
const PERSON: DVec3 = DVec3::new(0.0, 0.9, -12.0);

fn app(distant: f32) -> App {
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
        s.bloom_intensity = 0.0;
        s.vignette = 0.0;
        s.film_grain = 0.0;
        s.saturation = 1.0;
        s.anti_aliasing = AntiAliasing::None;
        s.volumetric_fog = false;
        s.probe_gi = false;
        s.ambient_occlusion = false;
        // The cascades end at 6 m, as in the character creator: the person is past them.
        s.shadow_distance = 6.0;
        s.far_shadow_distance = 0.0;
        s.distant_shadows = distant;
    }
    app.world.resource_mut::<FogSettings>().density = 0.0;
    let w = &mut app.world;
    w.spawn((
        Camera {
            exposure: Exposure::Manual { ev100: 14.5 },
            ..Camera::perspective(40.0)
        },
        Transform::from_xyz(0.0, 1.6, 0.0).looking_at(DVec3::new(0.0, 0.6, -12.0), Vec3::Y),
    ));
    // From the right (+x), 45 degrees up: the person's shadow falls 1.8 m to their left.
    w.spawn((
        DirectionalLight::default(),
        Transform::default().looking_at(DVec3::new(-1.0, -1.0, 0.0), Vec3::Y),
    ));
    w.spawn((
        Transform::default(),
        MeshRenderer::new(
            Shape::Plane {
                size: [400.0, 400.0],
                subdivisions: 4,
            },
            StandardMaterial::color(0.5, 0.5, 0.5).with_roughness(1.0),
        ),
    ));
    w.spawn((
        Transform::from_translation(PERSON),
        MeshRenderer::new(
            Shape::Cuboid {
                size: [0.5, 1.8, 0.3],
            },
            StandardMaterial::color(0.4, 0.3, 0.25),
        ),
    ));
    app
}

fn linear(v: u8) -> f32 {
    let v = v as f32 / 255.0;
    if v <= 0.04045 {
        v / 12.92
    } else {
        ((v + 0.055) / 1.055).powf(2.4)
    }
}

/// Mean linear luma of the 5x5 pixels around where ground point `p` lands.
fn luma_at(app: &App, img: &RgbaImage, p: DVec3) -> f32 {
    let view = app
        .world
        .resource::<Renderer>()
        .last_view()
        .unwrap()
        .clone();
    let clip = view.unjittered_view_proj * (p - view.origin).as_vec3().extend(1.0);
    let ndc = clip.truncate() / clip.w;
    let (x, y) = (
        ((ndc.x * 0.5 + 0.5) * W as f32) as i32,
        ((0.5 - ndc.y * 0.5) * H as f32) as i32,
    );
    let mut sum = 0.0;
    for dy in -2..=2 {
        for dx in -2..=2 {
            let px = img.get_pixel(
                (x + dx).clamp(0, W as i32 - 1) as u32,
                (y + dy).clamp(0, H as i32 - 1) as u32,
            );
            sum +=
                (0.2126 * linear(px[0]) + 0.7152 * linear(px[1]) + 0.0722 * linear(px[2])) / 25.0;
        }
    }
    sum
}

/// The ground behind the person (2 m farther, 1 m to their left, out of their shadow), in
/// their shadow (1 m to their left, beside them) and in the open.
fn measure(distant: f32) -> Option<(f32, f32, f32)> {
    let mut app = app(distant);
    app.world
        .resource_mut::<Time>()
        .set_source(engine_core::time::TimeSource::Manual { step_secs: 1.0 });
    app.world.resource_mut::<Time>().set_max_delta(1.0);
    app.update();
    if let Some(d) = app.world.get_resource::<RenderDisabled>() {
        eprintln!("skipping: no GPU ({})", d.0);
        return None;
    }
    app.update_n(6);
    let path = std::path::Path::new(env!("CARGO_TARGET_TMPDIR"))
        .join(format!("distant_shadow_person_{distant}.png"));
    capture_now(&mut app.world, &path).expect("screenshot");
    let img = image::open(&path).expect("png").to_rgba8();
    let ground = |dx: f64, dz: f64| DVec3::new(PERSON.x + dx, 0.0, PERSON.z + dz);
    Some((
        luma_at(&app, &img, ground(-1.0, -2.0)),
        luma_at(&app, &img, ground(-1.0, 0.0)),
        luma_at(&app, &img, ground(1.5, -2.0)),
    ))
}

#[test]
fn a_person_past_the_cascades_casts_no_slab_behind_them() {
    let Some((behind, shadow, open)) = measure(1.0) else {
        return;
    };
    let (behind0, shadow0, open0) = measure(0.0).expect("rendered once");
    eprintln!("distant shadows: behind {behind:.4}, shadow {shadow:.4}, open {open:.4}");
    eprintln!("without: behind {behind0:.4}, shadow {shadow0:.4}, open {open0:.4}");
    assert!(open > 0.05, "the open ground is dark: {open}");
    assert!(
        behind > behind0 * 0.9,
        "a slab behind the person: {behind:.4} vs {behind0:.4} without distant shadows"
    );
    assert!(
        shadow < shadow0 * 0.85,
        "no shadow beside the person: {shadow:.4} vs {shadow0:.4} without distant shadows"
    );
}
