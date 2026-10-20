//! Temporal anti-aliasing keeps thin things the transparent phase draws: falling snowflakes a
//! pixel or two wide, which write no depth or motion, keep most of their strength with TAA (the
//! reactive mask, `RenderSettings::taa_reactive`) instead of fading to a fraction, while a wide
//! glass pane over a checkerboard is anti-aliased exactly as before. Needs a GPU or lavapipe;
//! passes with a note when neither is available.
//!
//! The camera stands at the origin looking down -Z; the scenes are unlit, so every pixel is
//! exactly the color it was given.

use engine_core::prelude::*;
use engine_core::time::TimeSource;
use engine_render::prelude::*;
use engine_render::{AntiAliasing, RenderDisabled, capture_now};
use image::RgbaImage;

const W: u32 = 480;
const H: u32 = 270;
const FOV_Y_DEG: f32 = 60.0;
/// The flakes and the pane, and the wall behind them (m in front of the camera).
const FLAKES_AT: f64 = 10.0;
const WALL_AT: f64 = 20.0;

fn px_per_m(distance: f64) -> f64 {
    0.5 * H as f64 / ((0.5 * FOV_Y_DEG as f64).to_radians().tan() * distance)
}

fn app(anti_aliasing: AntiAliasing, reactive: f32) -> App {
    engine_core::log::init("error");
    let mut app = App::new();
    app.world.resource_mut::<EngineConfig>().headless = true;
    {
        let mut w = app.world.resource_mut::<Window>();
        w.width = W;
        w.height = H;
    }
    app.add_plugins(RenderPlugin);
    app.world
        .resource_mut::<Time>()
        .set_source(TimeSource::Manual {
            step_secs: 1.0 / 60.0,
        });
    {
        let mut s = app.world.resource_mut::<RenderSettings>();
        s.anti_aliasing = anti_aliasing;
        s.taa_reactive = reactive;
        s.sharpness = 0.0;
        s.film_grain = 0.0;
        s.vignette = 0.0;
        s.bloom_intensity = 0.0;
        s.volumetric_fog = false;
    }
    app.world.resource_mut::<FogSettings>().density = 0.0;
    app.world.spawn((
        Camera {
            exposure: Exposure::Manual { ev100: 0.0 },
            ..Camera::perspective(FOV_Y_DEG)
        },
        Transform::default(),
    ));
    app
}

fn unlit(rgb: [f32; 3]) -> StandardMaterial {
    StandardMaterial {
        unlit: true,
        ..StandardMaterial::color(rgb[0], rgb[1], rgb[2])
    }
}

fn quad(app: &mut App, at: DVec3, size: [f32; 2], material: StandardMaterial) -> Entity {
    app.world
        .spawn((
            Transform::from_translation(at),
            MeshRenderer::new(Shape::Quad { size }, material).without_shadows(),
        ))
        .id()
}

fn gpu_missing(app: &App) -> bool {
    if let Some(d) = app.world.get_resource::<RenderDisabled>() {
        eprintln!("skipping: no GPU ({})", d.0);
        return true;
    }
    false
}

fn shoot(app: &mut App, name: &str) -> RgbaImage {
    let path =
        std::path::Path::new(env!("CARGO_TARGET_TMPDIR")).join(format!("taa_reactive_{name}.png"));
    capture_now(&mut app.world, &path).expect("screenshot");
    image::open(&path).expect("png").to_rgba8()
}

fn luma(img: &RgbaImage, x: u32, y: u32) -> f64 {
    let p = img.get_pixel(x, y);
    0.2126 * p[0] as f64 + 0.7152 * p[1] as f64 + 0.0722 * p[2] as f64
}

/// A grid of snowflakes 1.5 px wide falling 2.5 px a frame over a dark wall, filling the left
/// half of the screen. Returns how much they brighten the wall where they are (the mean over
/// the flakes of their brightest pixel's luma above the wall's) when they have fallen for half a
/// second.
fn snowfall(anti_aliasing: AntiAliasing, reactive: f32, name: &str) -> Option<f64> {
    let mut app = app(anti_aliasing, reactive);
    quad(
        &mut app,
        DVec3::new(0.0, 0.0, -WALL_AT),
        [40.0, 40.0],
        unlit([0.04, 0.04, 0.05]),
    );
    let s = px_per_m(FLAKES_AT);
    let size = (1.5 / s) as f32;
    let mut flakes = Vec::new();
    for row in 0..8 {
        for col in 0..10 {
            // Columns over the left half, rows staggered so the flakes don't line up.
            let x = (-0.5 * W as f64 + 12.0 + 21.0 * col as f64) / s;
            let y = (0.5 * H as f64 - 20.0 - 26.0 * row as f64 - 7.0 * (col % 3) as f64) / s;
            let material = unlit([1.0, 1.0, 1.0]).with_alpha(0.9);
            flakes.push(quad(
                &mut app,
                DVec3::new(x, y + 75.0 / s, -FLAKES_AT),
                [size, size],
                material,
            ));
        }
    }
    let fall = 2.5 / s;
    for _ in 0..30 {
        for &f in &flakes {
            app.world.get_mut::<Transform>(f).unwrap().translation.y -= fall;
        }
        app.update();
    }
    if gpu_missing(&app) {
        return None;
    }
    let img = shoot(&mut app, name);
    // The wall's own luma, from the right half (no flakes there).
    let wall = luma(&img, 3 * W / 4, H / 2);
    // Each flake's brightest pixel: the brightest in the 5x5 square around it.
    let mut peaks = Vec::new();
    for y in 2..H - 2 {
        for x in 2..W / 2 - 10 {
            let v = luma(&img, x, y) - wall;
            let brightest = (y - 2..=y + 2)
                .flat_map(|y| (x - 2..=x + 2).map(move |x| (x, y)))
                .all(|(qx, qy)| luma(&img, qx, qy) - wall <= v);
            if v > 6.0 && brightest {
                peaks.push(v);
            }
        }
    }
    assert!(!peaks.is_empty(), "no flakes in {name}");
    Some(peaks.iter().sum::<f64>() / peaks.len() as f64)
}

#[test]
fn falling_snowflakes_keep_their_strength_through_temporal_anti_aliasing() {
    let Some(plain) = snowfall(AntiAliasing::None, 1.0, "flakes_no_aa") else {
        return;
    };
    let erased = snowfall(AntiAliasing::Taa, 0.0, "flakes_taa_without_mask").unwrap();
    let kept = snowfall(AntiAliasing::Taa, 1.0, "flakes_taa").unwrap();
    eprintln!(
        "flakes stand {plain:.0} above the wall without anti-aliasing, {erased:.0} with TAA and \
         no reactive mask, {kept:.0} with the mask"
    );
    assert!(
        erased < 0.5 * plain,
        "without the mask TAA should erase most of the flakes (the bug this guards): \
         {erased:.0} of {plain:.0}"
    );
    assert!(
        kept > 0.75 * plain,
        "with the mask the flakes should keep most of their strength: {kept:.0} of {plain:.0}"
    );
}

/// A glass pane (alpha 0.5) over a checkerboard, both slanted so their edges alias, after the
/// history has settled.
fn glass_pane(reactive: f32, name: &str) -> Option<RgbaImage> {
    let mut app = app(AntiAliasing::Taa, reactive);
    let wall = app
        .world
        .resource_mut::<engine_assets::Assets<Image>>()
        .add(Image::checker(
            256,
            16,
            [255, 255, 255, 255],
            [24, 24, 24, 255],
        ));
    let board = quad(
        &mut app,
        DVec3::new(0.0, 0.0, -WALL_AT),
        [24.0, 24.0],
        unlit([1.0, 1.0, 1.0]).with_base_color_texture(wall),
    );
    app.world.get_mut::<Transform>(board).unwrap().rotation = Quat::from_rotation_z(0.2);
    let pane = quad(
        &mut app,
        DVec3::new(1.0, 0.0, -FLAKES_AT),
        [4.0, 3.0],
        unlit([0.3, 0.6, 1.0]).with_alpha(0.5),
    );
    app.world.get_mut::<Transform>(pane).unwrap().rotation = Quat::from_rotation_z(-0.3);
    app.update_n(24);
    if gpu_missing(&app) {
        return None;
    }
    Some(shoot(&mut app, name))
}

#[test]
fn a_wide_glass_pane_is_anti_aliased_as_before() {
    let Some(without) = glass_pane(0.0, "pane_without_mask") else {
        return;
    };
    let with = glass_pane(1.0, "pane").unwrap();
    let (mut sum, mut changed) = (0.0f64, 0);
    for y in 0..H {
        for x in 0..W {
            let d = (luma(&with, x, y) - luma(&without, x, y)).abs();
            sum += d;
            changed += usize::from(d > 12.0);
        }
    }
    let mean = sum / (W * H) as f64;
    eprintln!(
        "glass pane with the mask vs without: mean change {mean:.3}, {changed} pixels changed \
         visibly"
    );
    assert!(
        mean < 0.05,
        "the mask should leave a wide pane alone: mean change {mean:.3}"
    );
    // A few pixels on the pane's edge where it crosses the checkerboard's corners may look thin
    // (about 30 of the 130 000).
    assert!(
        changed < 60,
        "the mask should leave the pane's edges and the pattern behind it anti-aliased: \
         {changed} pixels changed visibly"
    );
}
