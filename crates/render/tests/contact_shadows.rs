//! Contact shadows under thin sheets: a leaf held 15 cm over the ground, lit from behind and
//! above, shades the ground where its shadow falls, and not the ground under it that the
//! camera sees below it on screen. That ground's rays toward the sun pass behind the leaf (on
//! screen they cross the leaf's pixels a few centimeters farther away than the leaf); taken as a
//! solid 25 cm thick, the leaf shaded them, a dark streak hanging under every leaf of Foliage's
//! garden beds. Needs a GPU or lavapipe; passes with a note when neither is available.

use engine_core::prelude::*;
use engine_render::prelude::*;
use engine_render::{RenderDisabled, Renderer, capture_now};
use image::RgbaImage;

const W: u32 = 320;
const H: u32 = 200;
const LEAF_Y: f64 = 0.15;

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

/// Mean linear luma of the 3x3 pixels around where `p` lands.
fn luma_at(app: &App, img: &RgbaImage, p: DVec3) -> f32 {
    let (x, y) = screen(app, p);
    let mut sum = 0.0;
    for dy in -1..=1 {
        for dx in -1..=1 {
            let px = img.get_pixel(
                (x + dx).clamp(0, W as i32 - 1) as u32,
                (y + dy).clamp(0, H as i32 - 1) as u32,
            );
            sum += (0.2126 * linear(px[0]) + 0.7152 * linear(px[1]) + 0.0722 * linear(px[2])) / 9.0;
        }
    }
    sum
}

/// The ground lit by the sun alone (no sky, no ambient), a leaf 40 cm wide and 12 cm deep over
/// it (a double-sided sheet), the sun from behind it 45 degrees up. Rendered with contact
/// shadows or without.
fn leaf_scene(contact: bool) -> Option<(App, RgbaImage)> {
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
        if !contact {
            s.contact_shadow_length = 0.0;
        }
    }
    app.world.resource_mut::<FogSettings>().density = 0.0;
    app.world.resource_mut::<Atmosphere>().enabled = false;
    app.world.resource_mut::<AmbientLight>().brightness = 0.0;
    let w = &mut app.world;
    w.spawn((
        Camera {
            exposure: Exposure::Manual { ev100: 15.0 },
            ..Camera::perspective(40.0)
        },
        Transform::from_xyz(0.0, 0.6, 1.2).looking_at(DVec3::new(0.0, 0.05, 0.0), Vec3::Y),
    ));
    // From -z (behind the leaf), 45 degrees up: the leaf's shadow falls 15 cm toward the camera.
    w.spawn((
        DirectionalLight::default(),
        Transform::default().looking_at(DVec3::new(0.0, -1.0, 1.0), Vec3::Y),
    ));
    w.spawn((
        Transform::default(),
        MeshRenderer::new(
            Shape::Plane {
                size: [20.0, 20.0],
                subdivisions: 1,
            },
            StandardMaterial {
                perceptual_roughness: 1.0,
                ..StandardMaterial::color(0.5, 0.5, 0.5)
            },
        ),
    ));
    w.spawn((
        Transform::from_xyz(0.0, LEAF_Y, 0.0),
        MeshRenderer::new(
            Shape::Plane {
                size: [0.4, 0.12],
                subdivisions: 1,
            },
            StandardMaterial {
                perceptual_roughness: 1.0,
                double_sided: true,
                ..StandardMaterial::color(0.2, 0.5, 0.15)
            },
        ),
    ));
    app.update_n(8);
    if let Some(d) = app.world.get_resource::<RenderDisabled>() {
        eprintln!("skipping: no GPU ({})", d.0);
        return None;
    }
    let name = format!("contact_leaf_{}.png", if contact { "on" } else { "off" });
    let path = std::path::Path::new(env!("CARGO_TARGET_TMPDIR")).join(name);
    capture_now(&mut app.world, &path).expect("screenshot");
    let img = image::open(&path).expect("png").to_rgba8();
    Some((app, img))
}

#[test]
fn a_leaf_shades_where_its_shadow_falls_not_the_ground_under_it() {
    let Some((app, img)) = leaf_scene(true) else {
        return;
    };
    let (_, plain) = leaf_scene(false).expect("rendered once");
    // Under the leaf, seen below it on screen: its rays toward the sun leave the leaf's plane
    // 15 cm behind it, lit. The leaf's shadow: 15 cm toward the camera, under its middle.
    let under = DVec3::new(0.0, 0.0, -0.02);
    let shadow = DVec3::new(0.0, 0.0, LEAF_Y);
    let open = DVec3::new(0.5, 0.0, -0.02);
    let (u, s, o) = (
        luma_at(&app, &img, under),
        luma_at(&app, &img, shadow),
        luma_at(&app, &img, open),
    );
    let (u0, s0, o0) = (
        luma_at(&app, &plain, under),
        luma_at(&app, &plain, shadow),
        luma_at(&app, &plain, open),
    );
    eprintln!("contact shadows: under {u:.4}, shadow {s:.4}, open {o:.4}");
    eprintln!("without: under {u0:.4}, shadow {s0:.4}, open {o0:.4}");
    assert!(o > 0.05, "the open ground is dark: {o}");
    assert!(
        u > o * 0.85,
        "the ground under the leaf is shaded by contact shadows: {u:.4} vs {o:.4} in the open \
         ({u0:.4} without them)"
    );
    assert!(
        s < o * 0.3,
        "no shadow where the leaf's falls: {s:.4} vs {o:.4}"
    );
    // The leaf's own lit top doesn't shade itself, and the ground just past its shadow (whose
    // rays cross the leaf's plane in front of its edge) stays lit.
    let (top, past) = (DVec3::new(0.0, LEAF_Y, 0.0), DVec3::new(0.0, 0.0, 0.3));
    let (t, p) = (luma_at(&app, &img, top), luma_at(&app, &img, past));
    let (t0, p0) = (luma_at(&app, &plain, top), luma_at(&app, &plain, past));
    eprintln!("leaf top {t:.4} ({t0:.4} without), past its shadow {p:.4} ({p0:.4} without)");
    assert!(
        t > t0 * 0.95,
        "the leaf shades its own top: {t:.4} vs {t0:.4}"
    );
    assert!(
        p > p0 * 0.95,
        "the ground past the shadow is shaded: {p:.4} vs {p0:.4}"
    );
}
