//! Renders spell projectiles (`spell.frost_bolt`, `spell.fireball`) flying past close up,
//! side on, by day, at dusk and at night, into `screenshots/vfx/bolts/`.
//!
//! ```sh
//! cargo run -p engine_vfx --release --example bolts
//! cargo run -p engine_vfx --release --example bolts -- night frost
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

/// (label, effect)
const BOLTS: &[(&str, &str)] = &[("frost", "spell.frost_bolt"), ("fire", "spell.fireball")];
/// How fast the spells fly, m/s (the Spell court's casters throw them at 15).
const SPEED: f64 = 15.0;

fn main() {
    let lighting = ["day", "dusk", "night"];
    let args: Vec<String> = std::env::args().skip(1).collect();
    let when: Vec<&String> = args
        .iter()
        .filter(|a| lighting.contains(&a.as_str()))
        .collect();
    let out = std::path::Path::new("screenshots/vfx/bolts");
    std::fs::create_dir_all(out).expect("create screenshots/vfx/bolts");

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
    app.world.spawn((
        Transform::from_xyz(0.0, -0.002, 0.0),
        MeshRenderer::new(
            Shape::Plane {
                size: [200.0, 200.0],
                subdivisions: 1,
            },
            StandardMaterial::hex(0x6b675e).with_roughness(0.9),
        ),
    ));
    let camera = app
        .world
        .spawn((Camera::perspective(45.0), Transform::default()))
        .id();
    app.update();
    if let Some(d) = app.world.get_resource::<RenderDisabled>() {
        eprintln!("no GPU: {}", d.0);
        return;
    }
    // Side on to the flight path, the bolt just past the middle of the frame.
    *app.world.get_mut::<Transform>(camera).unwrap() =
        Transform::from_xyz(-1.0, 1.6, 4.5).looking_at(DVec3::new(-1.0, 1.5, 0.0), Vec3::Y);

    // (label, sun lux, sun elevation in degrees, sun color)
    let lights = [
        ("day", 60_000.0, 40.0f32, [1.0, 0.96, 0.9]),
        ("dusk", 600.0, 4.0, [1.0, 0.7, 0.45]),
        ("night", 0.2, 35.0, [0.6, 0.7, 1.0]),
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
            let dir = DVec3::new(
                0.6 * e.cos() as f64,
                -(e.sin() as f64),
                0.8 * e.cos() as f64,
            );
            *app.world.get_mut::<Transform>(sun).unwrap() =
                Transform::default().looking_at(dir, Vec3::Y);
        }
        // Let the exposure settle to the new light.
        app.update_n(90);
        for &(name, effect) in BOLTS {
            if !args.iter().any(|a| a == name)
                && args.iter().any(|a| BOLTS.iter().any(|b| b.0 == a))
            {
                continue;
            }
            // Fly in from well off screen so the trail is fully formed, and past the middle.
            let mut at = DVec3::new(-16.0, 1.5, 0.0);
            let bolt = app
                .world
                .spawn((Transform::from_translation(at), ParticleEffect::new(effect)))
                .id();
            for _ in 0..64 {
                at.x += SPEED * STEP;
                *app.world.get_mut::<Transform>(bolt).unwrap() = Transform::from_translation(at);
                app.update();
            }
            let path = out.join(format!("{label}_{name}.png"));
            match capture_now(&mut app.world, &path) {
                Ok(_) => println!("{}", path.display()),
                Err(e) => eprintln!("{label}_{name}: {e}"),
            }
            app.world.despawn(bolt);
            // Let what it left behind clear before the next one.
            app.update_n(150);
        }
    }
}

/// Flat ground at y = 0.
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
