//! Renders the engine exhausts (`exhaust.*`) side on, firing along +X over flat ground, by day,
//! at dusk and at night into `screenshots/vfx/engines/`.
//!
//! ```sh
//! cargo run -p engine_vfx --release --example engines                        # all of them
//! cargo run -p engine_vfx --release --example engines -- night exhaust.rocket
//! cargo run -p engine_vfx --release --example engines -- vacuum exhaust.thruster
//! cargo run -p engine_vfx --release --example engines -- day launch          # a lift-off
//! ```
//!
//! `vacuum` takes the air away, as in space. Needs a GPU or lavapipe.

use engine_core::math::{DVec3, Quat, Vec3};
use engine_core::prelude::*;
use engine_core::time::TimeSource;
use engine_render::prelude::*;
use engine_render::{RenderDisabled, capture_now};
use engine_vfx::prelude::*;

const W: u32 = 960;
const H: u32 = 540;
const STEP: f64 = 1.0 / 30.0;

/// (effect, scale, nozzle height, camera distance, seconds to run before the shot)
const SHOTS: &[(&str, f32, f64, f64, f32)] = &[
    ("exhaust.rocket", 0.6, 1.6, 8.0, 3.0),
    ("exhaust.afterburner", 0.8, 1.6, 9.0, 3.0),
    ("exhaust.jet", 0.8, 1.6, 6.0, 2.0),
    ("exhaust.thruster", 0.5, 1.6, 6.0, 3.0),
    ("exhaust.tailpipe", 1.0, 0.35, 2.2, 3.0),
    ("exhaust.backfire", 1.0, 0.35, 1.6, 0.05),
    ("exhaust.jumpjet", 1.0, 1.0, 2.2, 0.12),
];

fn main() {
    let lighting = ["day", "dusk", "night"];
    let args: Vec<String> = std::env::args().skip(1).collect();
    let vacuum = args.iter().any(|a| a == "vacuum");
    let when: Vec<&String> = args
        .iter()
        .filter(|a| lighting.contains(&a.as_str()))
        .collect();
    let only: Vec<&String> = args
        .iter()
        .filter(|a| !lighting.contains(&a.as_str()) && *a != "vacuum")
        .collect();
    let out = std::path::Path::new("screenshots/vfx/engines");
    std::fs::create_dir_all(out).expect("create screenshots/vfx/engines");

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
    if vacuum {
        app.world.resource_mut::<engine_world::Atmospheres>().top_m = 0.0;
    }
    let sun = app
        .world
        .spawn((DirectionalLight::default(), Transform::default()))
        .id();
    app.world.spawn((
        Transform::from_xyz(0.0, -0.01, 0.0),
        MeshRenderer::new(
            Shape::Plane {
                size: [400.0, 400.0],
                subdivisions: 1,
            },
            StandardMaterial::hex(0x5d5a52).with_roughness(0.95),
        ),
    ));
    // A wall behind, for the heat haze to bend.
    for i in 0..128 {
        app.world.spawn((
            Transform::from_xyz(-160.0 + i as f64 * 2.5, 2.0, -6.0),
            MeshRenderer::new(
                Shape::Cuboid {
                    size: [1.1, 4.0, 0.4],
                },
                StandardMaterial::hex(if i % 2 == 0 { 0x8a8580 } else { 0x4d4a45 })
                    .with_roughness(0.85),
            ),
        ));
    }
    let camera = app
        .world
        .spawn((Camera::perspective(50.0), Transform::default()))
        .id();
    app.update();
    if let Some(d) = app.world.get_resource::<RenderDisabled>() {
        eprintln!("no GPU: {}", d.0);
        return;
    }

    // (label, sun lux, sun elevation in degrees, sun color)
    let lights = [
        ("day", 60_000.0, 40.0f32, [1.0, 0.96, 0.9]),
        ("dusk", 250.0, 2.0, [1.0, 0.7, 0.45]),
        ("night", 0.2, 35.0, [0.6, 0.7, 1.0]),
    ];
    let tag = if vacuum { "_vacuum" } else { "" };
    for (label, lux, elevation, color) in lights {
        if !when.is_empty() && !when.iter().any(|w| *w == label) {
            continue;
        }
        {
            let mut light = app.world.get_mut::<DirectionalLight>(sun).unwrap();
            light.illuminance = lux;
            light.color = color;
            let e = elevation.to_radians();
            let dir = DVec3::new(
                -0.6 * e.cos() as f64,
                -(e.sin() as f64),
                -0.8 * e.cos() as f64,
            );
            *app.world.get_mut::<Transform>(sun).unwrap() =
                Transform::default().looking_at(dir, Vec3::Y);
        }
        for (i, &(name, scale, height, distance, run)) in SHOTS.iter().enumerate() {
            if !only.is_empty() && !only.iter().any(|o| name.starts_with(o.as_str())) {
                continue;
            }
            // Each well away from the others, so no smoke lingers from the last one.
            let x0 = (i as f64 - 3.0) * 50.0;
            // Side on, a little above the nozzle, framing the flame's first few meters.
            let look = DVec3::new(x0 + distance * 0.3, height, 0.0);
            *app.world.get_mut::<Transform>(camera).unwrap() =
                Transform::from_translation(look + DVec3::new(-distance * 0.1, 0.3, distance))
                    .looking_at(look, Vec3::Y);
            // Let the sky and exposure settle on the empty scene first.
            app.update_n(40);
            let engine = app
                .world
                .spawn((
                    Transform::from_xyz(x0, height, 0.0)
                        .with_rotation(Quat::from_rotation_arc(Vec3::Y, Vec3::X)),
                    ParticleEffect::new(name).with_scale(scale),
                ))
                .id();
            app.update_n((run as f64 / STEP).round().max(1.0) as usize);
            let path = out.join(format!("{label}{tag}_{name}.png"));
            match capture_now(&mut app.world, &path) {
                Ok(_) => println!("{}", path.display()),
                Err(e) => eprintln!("{label}_{name}: {e}"),
            }
            app.world.despawn(engine);
        }
        if only.is_empty() || only.iter().any(|o| o.as_str() == "launch") {
            launch(&mut app, camera, out, &format!("{label}{tag}"));
        }
    }
}

/// A rocket lifting off and climbing faster and faster, seen from 150 m away, to show the
/// trail of exhaust it leaves behind.
fn launch(app: &mut App, camera: Entity, out: &std::path::Path, label: &str) {
    let base = DVec3::new(400.0, 4.0, 0.0);
    *app.world.get_mut::<Transform>(camera).unwrap() =
        Transform::from_translation(base + DVec3::new(-40.0, 30.0, 170.0))
            .looking_at(base + DVec3::new(0.0, 50.0, 0.0), Vec3::Y);
    app.update_n(40);
    let rocket = app
        .world
        .spawn((
            Transform::from_translation(base)
                .with_rotation(Quat::from_rotation_arc(Vec3::Y, -Vec3::Y)),
            ParticleEffect::new("exhaust.rocket").with_scale(2.0),
        ))
        .id();
    // Held down for a second, then climbing at 12 m/s^2.
    let frames = (5.0 / STEP).round() as usize;
    for f in 0..frames {
        let t = (f as f64 * STEP - 1.0).max(0.0);
        app.world.get_mut::<Transform>(rocket).unwrap().translation =
            base + DVec3::Y * (6.0 * t * t);
        app.update();
    }
    let path = out.join(format!("{label}_launch.png"));
    match capture_now(&mut app.world, &path) {
        Ok(_) => println!("{}", path.display()),
        Err(e) => eprintln!("{label}_launch: {e}"),
    }
    app.world.despawn(rocket);
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
