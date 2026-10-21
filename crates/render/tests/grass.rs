//! Grass on screen: a meadow covers the ground with green blades that thin out into the
//! distance and move in the wind. Needs a GPU or lavapipe; passes with a note when neither is
//! available.

use engine_core::prelude::*;
use engine_render::prelude::*;
use engine_render::{DebugView, GrassField, RenderDisabled, capture_now};
use engine_world::WeatherState;
use image::RgbaImage;

const W: u32 = 320;
const H: u32 = 180;

/// A flat meadow on brown soil seen from eye height, sun low ahead and to the left.
fn meadow_app(wind: Vec3) -> App {
    meadow_app_with(wind, |_| {})
}

fn meadow_app_with(wind: Vec3, configure: impl FnOnce(&mut RenderSettings)) -> App {
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
        configure(&mut s);
    }
    app.world.insert_resource(WeatherState {
        wind,
        ..Default::default()
    });
    let w = &mut app.world;
    w.spawn((
        Camera {
            // Fixed, so frames differ only by what moves.
            exposure: Exposure::Manual { ev100: 14.0 },
            ..Camera::perspective(60.0)
        },
        Transform::from_xyz(0.0, 1.6, 8.0).looking_at(DVec3::new(0.0, 0.4, -4.0), Vec3::Y),
    ));
    w.spawn((
        DirectionalLight::default(),
        Transform::default().looking_at(DVec3::new(0.5, -0.45, 0.6), Vec3::Y),
    ));
    w.spawn((
        Transform::default(),
        MeshRenderer::new(
            Shape::Plane {
                size: [200.0, 200.0],
                subdivisions: 1,
            },
            StandardMaterial::color(0.22, 0.14, 0.08).with_roughness(0.95),
        ),
    ));
    // Small enough to keep a software renderer quick, big enough to fill the view.
    let mut field = GrassField::flat(60.0, 60.0);
    field.style.draw_distance = 40.0;
    w.spawn((Transform::from_xyz(0.0, 0.0, -18.0), field));
    app
}

fn render(app: &mut App, name: &str, frames: usize) -> Option<RgbaImage> {
    app.update_n(frames);
    if let Some(d) = app.world.get_resource::<RenderDisabled>() {
        eprintln!("skipping: no GPU ({})", d.0);
        return None;
    }
    let path = std::path::Path::new(env!("CARGO_TARGET_TMPDIR")).join(format!("{name}.png"));
    capture_now(&mut app.world, &path).expect("screenshot");
    Some(image::open(&path).expect("png").to_rgba8())
}

/// Mean color and mean absolute difference between horizontal neighbors (texture) over the
/// rows `y0..y1`.
fn band(img: &RgbaImage, y0: u32, y1: u32) -> ([f32; 3], f32) {
    let mut mean = [0.0; 3];
    let mut detail = 0.0;
    let mut n = 0.0;
    for y in y0..y1 {
        for x in 1..W {
            let p = img.get_pixel(x, y);
            let q = img.get_pixel(x - 1, y);
            for c in 0..3 {
                mean[c] += p[c] as f32;
                detail += (p[c] as f32 - q[c] as f32).abs() / 3.0;
            }
            n += 1.0;
        }
    }
    (mean.map(|m| m / n), detail / n)
}

#[test]
fn a_meadow_covers_the_ground_with_blades() {
    let mut app = meadow_app(Vec3::new(3.0, 0.0, -1.0));
    let Some(img) = render(&mut app, "grass_meadow", 6) else {
        return;
    };
    // Near the camera (bottom of the frame): green, and full of blade-sized detail.
    let (near, near_detail) = band(&img, H - 40, H);
    eprintln!("near grass: color {near:?}, detail {near_detail:.1}");
    assert!(
        near[1] > near[0] && near[1] > near[2],
        "the near ground isn't green: {near:?}"
    );
    assert!(
        near_detail > 6.0,
        "the near grass has no blade detail: {near_detail:.1}"
    );
}

#[test]
fn wind_moves_the_blades() {
    // Normals only: noisy lighting effects change every frame without TAA, the blades' shapes
    // change only when they move. Compared in 16-pixel blocks: a calm meadow's idle quiver
    // shifts single blades by a pixel, gusts sweeping through change how whole patches lean.
    const BLOCK: u32 = 16;
    let block_mean = |img: &RgbaImage, bx: u32, by: u32| {
        let mut sum = [0.0f32; 3];
        for y in by * BLOCK..(by + 1) * BLOCK {
            for x in bx * BLOCK..(bx + 1) * BLOCK {
                let p = img.get_pixel(x, y);
                for c in 0..3 {
                    sum[c] += p[c] as f32;
                }
            }
        }
        sum.map(|s| s / (BLOCK * BLOCK) as f32)
    };
    let diff = |wind: Vec3, name: &str| {
        let mut app = meadow_app_with(wind, |s| s.debug_view = DebugView::Normals);
        let a = render(&mut app, &format!("{name}_a"), 6)?;
        let b = render(&mut app, &format!("{name}_b"), 10)?;
        let (mut sum, mut n) = (0.0, 0.0);
        for by in H / 2 / BLOCK..H / BLOCK {
            for bx in 0..W / BLOCK {
                let (p, q) = (block_mean(&a, bx, by), block_mean(&b, bx, by));
                for c in 0..3 {
                    sum += (p[c] - q[c]).abs();
                }
                n += 3.0;
            }
        }
        Some(sum / n)
    };
    let (Some(windy), Some(calm)) = (
        diff(Vec3::new(8.0, 0.0, 0.0), "grass_windy"),
        diff(Vec3::ZERO, "grass_calm"),
    ) else {
        return;
    };
    eprintln!("change between frames: {windy:.2} in wind, {calm:.2} calm");
    assert!(
        windy > calm * 2.0 + 0.5,
        "the wind doesn't move the grass: {windy:.2} vs {calm:.2} calm"
    );
}

#[test]
fn deep_snow_buries_the_meadow() {
    let mut app = meadow_app(Vec3::ZERO);
    let mut layers = engine_world::SurfaceLayers::default();
    layers.grid.fill(engine_world::SurfaceCell {
        snow_m: 0.6,
        ..Default::default()
    });
    app.world.insert_resource(layers);
    let Some(img) = render(&mut app, "grass_snowed", 6) else {
        return;
    };
    // Near the camera: snow, no green blades standing out of it.
    let (near, _) = band(&img, H - 40, H);
    eprintln!("near ground under snow: color {near:?}");
    let green = near[1] - 0.5 * (near[0] + near[2]);
    assert!(green < 4.0, "green blades stand out of deep snow: {near:?}");
}
