//! Blended surfaces: a clear, smooth pane of water over dark ground still mirrors the sky at a
//! low angle, as a puddle does, while a faint rough smoke card over the same ground stays as
//! faint as its alpha. Needs a GPU or lavapipe; passes with a note when neither is available.

use engine_core::prelude::*;
use engine_render::prelude::*;
use engine_render::{RenderDisabled, capture_now};

const W: u32 = 160;
const H: u32 = 120;

/// Mean linear radiance (sRGB decoded, mean of the channels) of a band just below the middle
/// of the view: dark ground seen from 1.7 m at 10-15° below level by day, with `cover`
/// (a blended material) lying 1 cm above it, if any.
fn ground(name: &str, cover: Option<StandardMaterial>) -> Option<f32> {
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
        s.anti_aliasing = AntiAliasing::None;
    }
    app.world.resource_mut::<FogSettings>().density = 0.0;
    let w = &mut app.world;
    w.spawn((
        Camera {
            exposure: Exposure::Manual { ev100: 14.0 },
            ..Camera::perspective(40.0)
        },
        Transform::from_xyz(0.0, 1.7, 0.0).looking_at(DVec3::new(0.0, 0.0, -6.0), Vec3::Y),
    ));
    // The sun high behind the camera, so no highlight lands in view: what the cover shows is
    // the sky it mirrors.
    w.spawn((
        DirectionalLight::default(),
        Transform::default().looking_at(DVec3::new(0.0, -0.8, -0.6), Vec3::Y),
    ));
    w.spawn((
        Transform::default(),
        MeshRenderer::new(
            Shape::Plane {
                size: [200.0, 200.0],
                subdivisions: 1,
            },
            StandardMaterial::color(0.02, 0.02, 0.02),
        ),
    ));
    if let Some(material) = cover {
        w.spawn((
            Transform::from_xyz(0.0, 0.01, 0.0),
            MeshRenderer::new(
                Shape::Plane {
                    size: [200.0, 200.0],
                    subdivisions: 1,
                },
                material,
            ),
        ));
    }
    app.update_n(4);
    if let Some(d) = app.world.get_resource::<RenderDisabled>() {
        eprintln!("skipping: no GPU ({})", d.0);
        return None;
    }
    let path = std::path::Path::new(env!("CARGO_TARGET_TMPDIR")).join(format!("blend_{name}.png"));
    capture_now(&mut app.world, &path).expect("screenshot");
    let img = image::open(&path).expect("png").to_rgba8();
    let decode = |v: u8| {
        let v = v as f32 / 255.0;
        if v <= 0.04045 {
            v / 12.92
        } else {
            ((v + 0.055) / 1.055).powf(2.4)
        }
    };
    let mut sum = 0.0;
    let mut n = 0.0;
    for y in (H / 2 + 2)..(H / 2 + 22) {
        for x in (W / 2 - 30)..(W / 2 + 30) {
            let p = img.get_pixel(x, y);
            sum += (decode(p[0]) + decode(p[1]) + decode(p[2])) / 3.0;
            n += 1.0;
        }
    }
    Some(sum / n)
}

fn cover(alpha: f32, roughness: f32) -> StandardMaterial {
    StandardMaterial {
        base_color: [0.02, 0.03, 0.04, alpha],
        perceptual_roughness: roughness,
        alpha_mode: AlphaMode::Blend,
        ..Default::default()
    }
}

#[test]
fn clear_water_mirrors_the_sky_and_faint_smoke_stays_faint() {
    let Some(bare) = ground("bare", None) else {
        return;
    };
    let water = ground("water", Some(cover(0.1, 0.03))).unwrap();
    let smoke = ground("smoke", Some(cover(0.1, 1.0))).unwrap();
    eprintln!("bare {bare:.4}, water {water:.4}, smoke {smoke:.4}");
    // At 10-15° the water reflects some 10% of the sky, about 0.016 here. With that reflection
    // faded by its alpha (straight alpha blending) it added only 0.0015.
    assert!(
        water - bare > 0.008,
        "clear water doesn't mirror the sky: {water} vs {bare}"
    );
    // The smoke card's own light fades with its alpha, its reflections included.
    assert!(
        smoke - bare < 0.004,
        "a faint smoke card shows its reflections: {smoke} vs {bare}"
    );
}
