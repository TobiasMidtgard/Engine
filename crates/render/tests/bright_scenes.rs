//! Auto exposure in bright scenes: a sunlit snowfield is exposed white, not metered down to mid
//! grey like a field of grey stones under the same sun, while an ordinary view of sky and grass
//! keeps its exposure. Needs a GPU or lavapipe; passes with a note when neither is available.

use engine_core::prelude::*;
use engine_render::prelude::*;
use engine_render::{RenderDisabled, RenderStats, capture_now};

const W: u32 = 160;
const H: u32 = 90;

/// The exposure auto exposure settles on (EV100) and the mean linear value (sRGB decoded, mean
/// of the channels) of the middle of the view: a field of `albedo` under the sun 40° up, seen
/// from 1.7 m looking `pitch` degrees down.
fn settle(name: &str, albedo: f32, pitch: f64) -> Option<(f32, f32)> {
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
    let p = pitch.to_radians();
    w.spawn((
        Camera {
            exposure: Exposure::Auto {
                min_ev100: -3.0,
                max_ev100: 20.0,
                speed: 50.0,
                compensation: 0.0,
            },
            ..Camera::perspective(60.0)
        },
        Transform::from_xyz(0.0, 1.7, 0.0)
            .looking_at(DVec3::new(0.0, 1.7 - p.tan(), -1.0), Vec3::Y),
    ));
    // The sun 40° up, off to the side.
    let e = 40f64.to_radians();
    w.spawn((
        DirectionalLight::default(),
        Transform::default()
            .looking_at(DVec3::new(0.6 * e.cos(), -e.sin(), -0.8 * e.cos()), Vec3::Y),
    ));
    w.spawn((
        Transform::default(),
        MeshRenderer::new(
            Shape::Plane {
                size: [4000.0, 4000.0],
                subdivisions: 1,
            },
            StandardMaterial::color(albedo, albedo, albedo),
        ),
    ));
    app.update_n(20);
    if let Some(d) = app.world.get_resource::<RenderDisabled>() {
        eprintln!("skipping: no GPU ({})", d.0);
        return None;
    }
    let ev = app.world.resource::<RenderStats>().ev100;
    let path = std::path::Path::new(env!("CARGO_TARGET_TMPDIR")).join(format!("bright_{name}.png"));
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
    for y in (H / 2 + 5)..(H / 2 + 25) {
        for x in (W / 2 - 30)..(W / 2 + 30) {
            let p = img.get_pixel(x, y);
            sum += (decode(p[0]) + decode(p[1]) + decode(p[2])) / 3.0;
            n += 1.0;
        }
    }
    Some((ev, sum / n))
}

#[test]
fn snow_is_exposed_white_and_a_grassy_view_is_unchanged() {
    let Some((snow_ev, snow)) = settle("snow", 0.9, 40.0) else {
        return;
    };
    let (stone_ev, stone) = settle("stone", 0.18, 40.0).unwrap();
    let (grass_ev, grass) = settle("grass", 0.12, 5.0).unwrap();
    eprintln!(
        "snow: ev {snow_ev:.2} value {snow:.3}; stone: ev {stone_ev:.2} value {stone:.3}; \
         grass and sky: ev {grass_ev:.2} value {grass:.3}"
    );
    // The meter alone showed both fields the same gray (0.28); snow reflects five times what
    // gray stone does.
    assert!(
        snow > stone * 2.5,
        "snow is metered down to gray: {snow} vs stone {stone}"
    );
    // Gray stone and an ordinary view stay where the meter puts them.
    assert!(
        (0.2..0.4).contains(&stone),
        "gray stone isn't mid gray: {stone}"
    );
    assert!(
        (0.15..0.3).contains(&grass),
        "the grassy view changed: {grass}"
    );
}
