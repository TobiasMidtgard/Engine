//! The aerial perspective table (`RenderSettings::aerial_table`) looks like marching every
//! pixel: a hazy valley with ridges from 600 m to 40 km away, the sun low behind them (their
//! shadows in the haze) and then behind the camera, with volumetric fog on, renders the same
//! with the table as without it. Needs a GPU or lavapipe; passes with a note when neither is
//! available.

use engine_core::prelude::*;
use engine_render::prelude::*;
use engine_render::{RenderDisabled, capture_now};

const W: u32 = 192;
const H: u32 = 108;
const EV: f32 = 14.0;

/// Linear radiance (cd/m², mean of the channels) of each pixel.
fn render(name: &str, table: bool, backlit: bool) -> Option<Vec<f32>> {
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
        s.aerial_table = table;
        s.far_shadow_distance = 8000.0;
    }
    {
        let mut fog = app.world.resource_mut::<FogSettings>();
        fog.density = 0.00012;
        fog.height_falloff = 0.02;
        fog.base_height = 0.0;
    }
    let w = &mut app.world;
    w.spawn((
        Camera {
            exposure: Exposure::Manual { ev100: EV },
            ..Camera::perspective(40.0)
        },
        Transform::from_xyz(0.0, 12.0, 0.0).looking_at(DVec3::new(300.0, 250.0, -3000.0), Vec3::Y),
    ));
    let elevation = 15f64.to_radians();
    let z = if backlit { -1.0 } else { 1.0 };
    let to_sun = DVec3::new(0.25, elevation.sin(), z * elevation.cos()).normalize();
    w.spawn((
        DirectionalLight::default(),
        Transform::default().looking_at(-to_sun, Vec3::Y),
    ));
    w.spawn((
        Transform::default(),
        MeshRenderer::new(
            Shape::Plane {
                size: [100_000.0, 100_000.0],
                subdivisions: 1,
            },
            StandardMaterial::color(0.25, 0.3, 0.15),
        ),
    ));
    // Ridges 600 m, 3 km, 9 km and 40 km away, each larger than the last.
    for (x, distance, width, height) in [
        (-300.0, 600.0, 300.0, 60.0),
        (500.0, 3000.0, 2000.0, 500.0),
        (-2500.0, 9000.0, 6000.0, 1200.0),
        (4000.0, 40_000.0, 30_000.0, 4000.0),
    ] {
        w.spawn((
            Transform::from_xyz(x, height * 0.5, -distance),
            MeshRenderer::new(
                Shape::Cuboid {
                    size: [width as f32, height as f32, (width * 0.2) as f32],
                },
                StandardMaterial::color(0.3, 0.3, 0.3),
            ),
        ));
    }
    app.update_n(6);
    if let Some(d) = app.world.get_resource::<RenderDisabled>() {
        eprintln!("skipping: no GPU ({})", d.0);
        return None;
    }
    let path = std::path::Path::new(env!("CARGO_TARGET_TMPDIR")).join(format!("aerial_{name}.png"));
    capture_now(&mut app.world, &path).expect("screenshot");
    if let Ok(out) = std::env::var("ENGINE_SHOTS") {
        let _ = std::fs::copy(
            &path,
            std::path::Path::new(&out).join(format!("aerial_{name}.png")),
        );
    }
    let img = image::open(&path).expect("png").to_rgba8();
    let exposure = 1.0 / (1.2 * 2f32.powf(EV));
    let lin = |v: u8| {
        let c = v as f32 / 255.0;
        if c <= 0.04045 {
            c / 12.92
        } else {
            ((c + 0.055) / 1.055).powf(2.4)
        }
    };
    Some(
        img.pixels()
            .map(|p| (lin(p[0]) + lin(p[1]) + lin(p[2])) / 3.0 / exposure)
            .collect(),
    )
}

/// Mean relative difference over the image, and the largest over 8 × 8 blocks.
fn compare(a: &[f32], b: &[f32]) -> (f32, f32) {
    let rel = |x: f32, y: f32| (x - y).abs() / (0.5 * (x + y)).max(1.0);
    let mean = a.iter().zip(b).map(|(x, y)| rel(*x, *y)).sum::<f32>() / a.len() as f32;
    let mut worst: f32 = 0.0;
    for by in 0..H / 8 {
        for bx in 0..W / 8 {
            let (mut sa, mut sb) = (0.0, 0.0);
            for y in by * 8..by * 8 + 8 {
                for x in bx * 8..bx * 8 + 8 {
                    let i = (y * W + x) as usize;
                    sa += a[i];
                    sb += b[i];
                }
            }
            worst = worst.max(rel(sa / 64.0, sb / 64.0));
        }
    }
    (mean, worst)
}

#[test]
fn distant_ridges_look_the_same_with_the_table() {
    for backlit in [true, false] {
        let lit = if backlit { "backlit" } else { "frontlit" };
        let Some(marched) = render(&format!("{lit}_marched"), false, backlit) else {
            return;
        };
        let table = render(&format!("{lit}_table"), true, backlit).expect("GPU");
        let (mean, worst) = compare(&marched, &table);
        eprintln!(
            "{lit}: mean difference {:.2}%, worst block {:.2}%",
            mean * 100.0,
            worst * 100.0
        );
        assert!(
            mean < 0.02,
            "{lit}: the table differs by {:.2}% on average",
            mean * 100.0
        );
        assert!(
            worst < 0.08,
            "{lit}: a block differs by {:.2}%",
            worst * 100.0
        );
    }
}
