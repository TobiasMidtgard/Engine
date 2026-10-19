//! Renders water splashes (`water.splash`, `water.splash_small`, `water.column`) close up on a
//! calm pool, a few moments into each, by day or at dusk into `screenshots/vfx/splashes/`.
//!
//! ```sh
//! cargo run -p engine_vfx --release --example splashes                      # all of them
//! cargo run -p engine_vfx --release --example splashes -- dusk water.splash
//! ```
//!
//! Needs a GPU or lavapipe.

use engine_core::math::{DVec3, Vec3};
use engine_core::prelude::*;
use engine_core::time::TimeSource;
use engine_render::prelude::*;
use engine_render::{RenderDisabled, capture_now};
use engine_vfx::prelude::*;

const W: u32 = 960;
const H: u32 = 540;
const STEP: f64 = 1.0 / 60.0;

/// (effect, scale, camera distance, camera height, moments to capture in seconds)
const SHOTS: &[(&str, f32, f64, f64, &[f32])] = &[
    ("water.splash", 1.0, 3.2, 0.9, &[0.12, 0.3, 0.5, 0.8, 1.6]),
    ("water.splash_small", 1.0, 1.0, 0.3, &[0.08, 0.2, 0.4]),
    ("water.column", 1.0, 26.0, 5.0, &[0.4, 1.2, 3.0]),
];

fn main() {
    let lighting = ["day", "dusk"];
    let args: Vec<String> = std::env::args().skip(1).collect();
    let when: Vec<&String> = args
        .iter()
        .filter(|a| lighting.contains(&a.as_str()))
        .collect();
    let only: Vec<&String> = args
        .iter()
        .filter(|a| !lighting.contains(&a.as_str()))
        .collect();
    let out = std::path::Path::new("screenshots/vfx/splashes");
    std::fs::create_dir_all(out).expect("create screenshots/vfx/splashes");

    let mut app = App::new();
    app.world.resource_mut::<EngineConfig>().headless = true;
    {
        let mut w = app.world.resource_mut::<Window>();
        w.width = W;
        w.height = H;
    }
    app.world
        .resource_mut::<Time>()
        .set_source(TimeSource::Manual { step_secs: STEP });
    app.add_plugins((engine_world::WorldPlugin, RenderPlugin, VfxPlugin));
    app.insert_resource(engine_world::Ground(std::sync::Arc::new(Flat)));
    let sun = app
        .world
        .spawn((DirectionalLight::default(), Transform::default()))
        .id();
    // A still pool: dark, glossy water, with a bank of stones behind it.
    app.world.spawn((
        Transform::from_xyz(0.0, 0.0, 0.0),
        MeshRenderer::new(
            Shape::Plane {
                size: [400.0, 400.0],
                subdivisions: 1,
            },
            StandardMaterial::hex(0x0e2a33).with_roughness(0.04),
        ),
    ));
    for i in 0..60 {
        let x = -75.0 + i as f64 * 2.5;
        app.world.spawn((
            Transform::from_xyz(x, 0.4, -9.0 - (i % 3) as f64),
            MeshRenderer::new(
                Shape::Cuboid {
                    size: [2.0, 1.4 + (i % 4) as f32 * 0.5, 1.2],
                },
                StandardMaterial::hex(if i % 2 == 0 { 0x6f6a60 } else { 0x4a4a42 })
                    .with_roughness(0.9),
            ),
        ));
    }
    let camera = app
        .world
        .spawn((Camera::perspective(45.0), Transform::default()))
        .id();
    app.update();
    if let Some(d) = app.world.get_resource::<RenderDisabled>() {
        eprintln!("no GPU: {}", d.0);
        return;
    }

    // (label, sun lux, sun elevation in degrees, sun color)
    let lights = [
        ("day", 60_000.0, 35.0f32, [1.0, 0.96, 0.9]),
        ("dusk", 600.0, 4.0, [1.0, 0.7, 0.45]),
    ];
    for (label, lux, elevation, color) in lights {
        if !when.is_empty() && !when.iter().any(|w| *w == label) {
            continue;
        }
        {
            let mut light = app.world.get_mut::<DirectionalLight>(sun).unwrap();
            light.illuminance = lux;
            light.color = color;
            let e = elevation.to_radians();
            // Sun ahead and to the left, so crowns are back lit and glint.
            let dir = DVec3::new(
                0.5 * e.cos() as f64,
                -(e.sin() as f64),
                0.85 * e.cos() as f64,
            );
            *app.world.get_mut::<Transform>(sun).unwrap() =
                Transform::default().looking_at(dir, Vec3::Y);
        }
        for (i, &(name, scale, distance, height, moments)) in SHOTS.iter().enumerate() {
            if !only.is_empty() && !only.iter().any(|o| name.starts_with(o.as_str())) {
                continue;
            }
            let x0 = (i as f64 - 1.0) * 60.0;
            let look = DVec3::new(x0, height * 0.45, 0.0);
            *app.world.get_mut::<Transform>(camera).unwrap() =
                Transform::from_translation(DVec3::new(x0, height, distance))
                    .looking_at(look, Vec3::Y);
            app.update_n(40);
            let splash = app
                .world
                .spawn((
                    Transform::from_xyz(x0, 0.0, 0.0),
                    ParticleEffect::new(name).with_scale(scale),
                ))
                .id();
            let mut t = 0.0f32;
            for &m in moments {
                let steps = ((m - t) as f64 / STEP).round().max(1.0) as usize;
                app.update_n(steps);
                t = m;
                let path = out.join(format!("{label}_{name}_{:03}.png", (m * 100.0) as u32));
                match capture_now(&mut app.world, &path) {
                    Ok(_) => println!("{}", path.display()),
                    Err(e) => eprintln!("{label}_{name}: {e}"),
                }
            }
            app.world.despawn(splash);
            app.update_n(1);
        }
    }
}

/// Flat ground at y = 0 for the particles to land on.
struct Flat;

impl engine_world::GroundQuery for Flat {
    fn sample(&self, _p: DVec3) -> Option<engine_world::GroundSample> {
        Some(engine_world::GroundSample {
            height: 0.0,
            normal: Vec3::Y,
            surface: engine_world::SurfaceId(0),
        })
    }
}
