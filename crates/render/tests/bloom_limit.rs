//! `RenderSettings::bloom_limit`: a light far brighter than the exposure (a neon ring at night)
//! glows without swamping the frame, while scenes within the limit bloom exactly as before.
//! Needs a GPU or lavapipe; passes with a note when neither is available.

use engine_core::prelude::*;
use engine_render::prelude::*;
use engine_render::{RenderDisabled, capture_now};
use image::RgbaImage;

const W: u32 = 192;
const H: u32 = 128;

/// A small blue neon ring (600 cd/m², as the showcase's plaza) in the dark, seen at `ev100`.
fn render(name: &str, ev100: f32, bloom: f32, limit: f32) -> Option<RgbaImage> {
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
        s.vignette = 0.0;
        s.volumetric_fog = false;
        s.bloom_intensity = bloom;
        s.bloom_limit = limit;
    }
    app.world.resource_mut::<FogSettings>().density = 0.0;
    let w = &mut app.world;
    w.spawn((
        Camera {
            exposure: Exposure::Manual { ev100 },
            ..Camera::perspective(40.0)
        },
        Transform::from_xyz(0.0, 1.5, 10.0).looking_at(DVec3::new(0.0, 1.5, 0.0), Vec3::Y),
    ));
    // Upright, facing the camera.
    w.spawn((
        Transform::from_xyz(0.0, 1.5, 0.0)
            .with_rotation(Quat::from_rotation_x(std::f32::consts::FRAC_PI_2)),
        MeshRenderer::new(
            Shape::Torus {
                radius: 0.8,
                tube: 0.08,
                segments: 48,
            },
            StandardMaterial::hex(0x202830).with_emissive([0.3, 0.7, 1.0], 600.0),
        )
        .without_shadows(),
    ));
    app.update_n(3);
    if let Some(d) = app.world.get_resource::<RenderDisabled>() {
        eprintln!("skipping: no GPU ({})", d.0);
        return None;
    }
    let path = std::path::Path::new(env!("CARGO_TARGET_TMPDIR")).join(format!("bloom_{name}.png"));
    capture_now(&mut app.world, &path).expect("screenshot");
    Some(image::open(&path).expect("png").to_rgba8())
}

/// Pixels clearly lit (by the ring or its glow).
fn lit_pixels(img: &RgbaImage) -> usize {
    img.pixels()
        .filter(|p| p[0] as u32 + p[1] as u32 + p[2] as u32 > 3 * 48)
        .count()
}

#[test]
fn bloom_limit_keeps_a_night_neon_from_swamping_the_frame() {
    // At night (EV -2) the ring is about 2000 times brighter than white.
    let Some(ring) = render("ring_only", -2.0, 0.0, 0.0) else {
        return;
    };
    let unlimited = render("unlimited", -2.0, 0.04, 0.0).unwrap();
    let limited = render("limited", -2.0, 0.04, RenderSettings::default().bloom_limit).unwrap();
    let (r, u, l) = (
        lit_pixels(&ring),
        lit_pixels(&unlimited),
        lit_pixels(&limited),
    );
    eprintln!(
        "lit pixels: ring {r}, unlimited bloom {u}, limited bloom {l} of {}",
        W * H
    );
    assert!(r > 100, "the ring isn't in view");
    assert!(
        u > r + 2000,
        "an unlimited neon at night should glow far around it"
    );
    assert!(
        (l - r) * 4 < u - r,
        "the limited glow should cover far less of the frame: {} vs {} px",
        l - r,
        u - r
    );
    assert!(l > r, "the limited neon should still glow");
}

#[test]
fn bloom_within_the_limit_is_unchanged() {
    // In daylight exposure (EV 10) the ring is about half of white: far below the limit.
    let Some(unlimited) = render("day_unlimited", 10.0, 0.04, 0.0) else {
        return;
    };
    let limited = render(
        "day_limited",
        10.0,
        0.04,
        RenderSettings::default().bloom_limit,
    )
    .unwrap();
    let diff = unlimited
        .pixels()
        .zip(limited.pixels())
        .map(|(a, b)| {
            (0..3)
                .map(|c| (a[c] as i32 - b[c] as i32).abs())
                .max()
                .unwrap()
        })
        .max()
        .unwrap();
    assert!(diff <= 1, "bloom within the limit changed by {diff}/255");
}
