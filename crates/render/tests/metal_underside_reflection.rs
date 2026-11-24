//! A reflection heading down sees the ground, which no cover from above hides: the underside of
//! a bare metal ball, seen from below, mirrors the lit ground under it. The sky occlusion map
//! sees only cover from above, and the specular sky march took the ball's own top, over the
//! underside, for a wall in the reflection's way: black patches on the underside of a metal ball
//! (seen in a lake's drawn mirror, which shows that underside), none with sky occlusion off.
//! Needs a GPU or lavapipe; passes with a note when neither is available.

use engine_core::prelude::*;
use engine_render::prelude::*;
use engine_render::{RenderDisabled, Renderer, capture_now};
use image::RgbaImage;

const W: u32 = 320;
const H: u32 = 200;
const BALL: DVec3 = DVec3::new(0.0, 2.2, -5.0);
const RADIUS: f32 = 1.0;

fn linear(v: u8) -> f32 {
    let v = v as f32 / 255.0;
    if v <= 0.04045 {
        v / 12.92
    } else {
        ((v + 0.055) / 1.055).powf(2.4)
    }
}

/// Where world point `p` lands on screen, in pixels.
fn screen(app: &App, p: DVec3) -> (i32, i32) {
    let view = app
        .world
        .resource::<Renderer>()
        .last_view()
        .unwrap()
        .clone();
    let clip = view.unjittered_view_proj * (p - view.origin).as_vec3().extend(1.0);
    let ndc = clip.truncate() / clip.w;
    (
        ((ndc.x * 0.5 + 0.5) * W as f32) as i32,
        ((0.5 - ndc.y * 0.5) * H as f32) as i32,
    )
}

/// The ball over lit ground, the camera below its middle looking up at its underside. Rendered
/// with sky occlusion or without.
fn render(sky_occlusion: bool) -> Option<(App, RgbaImage)> {
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
        s.sharpness = 0.0;
        s.anti_aliasing = AntiAliasing::None;
        s.night_desaturation = 0.0;
        s.screen_space_gi = false;
        s.probe_gi = false;
        s.sky_occlusion = sky_occlusion;
    }
    app.world.resource_mut::<FogSettings>().density = 0.0;
    let w = &mut app.world;
    w.spawn((
        Camera {
            exposure: Exposure::Manual { ev100: 14.0 },
            ..Camera::perspective(50.0)
        },
        Transform::from_xyz(0.0, 0.4, 0.0).looking_at(BALL, Vec3::Y),
    ));
    w.spawn((
        DirectionalLight::default(),
        Transform::default().looking_at(DVec3::new(0.4, -0.8, -0.45), Vec3::Y),
    ));
    w.spawn((
        Transform::default(),
        MeshRenderer::new(
            Shape::Plane {
                size: [200.0, 200.0],
                subdivisions: 4,
            },
            StandardMaterial::color(0.5, 0.5, 0.5).with_roughness(1.0),
        ),
    ));
    w.spawn((
        Transform::from_translation(BALL),
        MeshRenderer::new(
            Shape::Sphere {
                radius: RADIUS,
                segments: 48,
            },
            StandardMaterial {
                metallic: 1.0,
                perceptual_roughness: 0.2,
                ..StandardMaterial::color(0.9, 0.9, 0.9)
            },
        ),
    ));
    app.update_n(6);
    if let Some(d) = app.world.get_resource::<RenderDisabled>() {
        eprintln!("skipping: no GPU ({})", d.0);
        return None;
    }
    let name = format!(
        "metal_underside_{}.png",
        if sky_occlusion { "on" } else { "off" }
    );
    let path = std::path::Path::new(env!("CARGO_TARGET_TMPDIR")).join(name);
    capture_now(&mut app.world, &path).expect("screenshot");
    let img = image::open(&path).expect("png").to_rgba8();
    Some((app, img))
}

/// The ball's pixels on its lower half (below its centre on screen, inside 85% of its radius
/// there), as linear luma.
fn underside(app: &App, img: &RgbaImage) -> Vec<f32> {
    let c = screen(app, BALL);
    let edge = screen(app, BALL - DVec3::new(0.0, RADIUS as f64, 0.0));
    let r = ((edge.1 - c.1).abs() as f32 * 0.85) as i32;
    let mut out = Vec::new();
    for y in c.1..=c.1 + r {
        for x in c.0 - r..=c.0 + r {
            let (dx, dy) = (x - c.0, y - c.1);
            if dx * dx + dy * dy > r * r || x < 0 || y < 0 || x >= W as i32 || y >= H as i32 {
                continue;
            }
            let p = img.get_pixel(x as u32, y as u32);
            out.push(0.2126 * linear(p[0]) + 0.7152 * linear(p[1]) + 0.0722 * linear(p[2]));
        }
    }
    out
}

#[test]
fn a_metal_balls_underside_mirrors_the_ground_under_it() {
    let Some((app, img)) = render(true) else {
        return;
    };
    let (plain_app, plain) = render(false).expect("rendered once");
    let (on, off) = (underside(&app, &img), underside(&plain_app, &plain));
    assert!(
        on.len() > 400,
        "the ball's underside is too small: {}",
        on.len()
    );
    let mean = |v: &[f32]| v.iter().sum::<f32>() / v.len() as f32;
    let (m_on, m_off) = (mean(&on), mean(&off));
    // Black: under a tenth of what the underside shows without sky occlusion.
    let black = on.iter().filter(|&&l| l < m_off * 0.1).count();
    eprintln!(
        "underside: mean {m_on:.4} with sky occlusion, {m_off:.4} without; {black} of {} pixels black",
        on.len()
    );
    assert!(
        m_off > 0.02,
        "the underside is dark without sky occlusion: {m_off}"
    );
    assert_eq!(black, 0, "black patches on the underside");
    assert!(
        m_on > m_off * 0.9,
        "sky occlusion darkens the underside: {m_on:.4} vs {m_off:.4}"
    );
}
