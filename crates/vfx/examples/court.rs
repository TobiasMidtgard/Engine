//! Renders the whole Spell court, without the showcase around it, from some of its tour
//! viewpoints by day, at dusk and at night into `screenshots/vfx/court/`. The camera exposes on
//! its own, as the showcase's does, so this shows how the court's glows (runes, crystals,
//! coals, spells) hold up from noon to night.
//!
//! ```sh
//! cargo run -p engine_vfx --release --example court
//! cargo run -p engine_vfx --release --example court -- magic_starburst   # some views only
//! cargo run -p engine_vfx --release --example court -- night magic_portal # and one light
//! ```
//!
//! Needs a GPU or lavapipe.

use engine_core::math::{DVec3, Vec3};
use engine_core::prelude::*;
use engine_core::time::TimeSource;
use engine_render::prelude::*;
use engine_render::{RenderDisabled, capture_now};
use engine_vfx::demo::{self, VIEWS};
use engine_vfx::prelude::*;

const W: u32 = 1280;
const H: u32 = 720;
const STEP: f64 = 1.0 / 30.0;

fn main() {
    let lighting = ["day", "dusk", "night"];
    let (when, only): (Vec<String>, Vec<String>) = std::env::args()
        .skip(1)
        .partition(|a| lighting.contains(&a.as_str()));
    let out = std::path::Path::new("screenshots/vfx/court");
    std::fs::create_dir_all(out).expect("create screenshots/vfx/court");

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
        Transform::from_xyz(0.0, -0.01, 0.0),
        MeshRenderer::new(
            Shape::Plane {
                size: [400.0, 400.0],
                subdivisions: 1,
            },
            StandardMaterial::hex(0x4f5a3c).with_roughness(0.95),
        ),
    ));
    let camera = app
        .world
        .spawn((Camera::perspective(55.0), Transform::default()))
        .id();
    app.add_systems(Startup, |mut commands: Commands| {
        demo::spawn_demo(&mut commands, DVec3::ZERO)
    });
    app.update();
    if let Some(d) = app.world.get_resource::<RenderDisabled>() {
        eprintln!("no GPU: {}", d.0);
        return;
    }
    app.update_n(30);

    let views: Vec<_> = VIEWS
        .iter()
        .filter(|v| {
            if only.is_empty() {
                ["magic_spells", "magic_starburst", "magic_portal"].contains(&v.key)
            } else {
                only.iter().any(|o| v.key.starts_with(o.as_str()))
            }
        })
        .collect();
    // (label, sun lux, sun elevation in degrees, sun color)
    let lights = [
        ("day", 60_000.0, 40.0f32, [1.0, 0.96, 0.9]),
        ("dusk", 250.0, 2.0, [1.0, 0.7, 0.45]),
        ("night", 0.2, 35.0, [0.6, 0.7, 1.0]),
    ];
    for (label, lux, elevation, color) in lights {
        if !when.is_empty() && !when.iter().any(|w| w == label) {
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
        for view in &views {
            *app.world.get_mut::<Transform>(camera).unwrap() =
                Transform::from_translation(DVec3::from_array(view.eye))
                    .looking_at(DVec3::from_array(view.target), Vec3::Y);
            // The sky, its lighting and the exposure catch up with a sudden change over a few
            // seconds.
            app.update_n(100);
            let path = out.join(format!("{label}_{}.png", view.key));
            match capture_now(&mut app.world, &path) {
                Ok(_) => println!("{}", path.display()),
                Err(e) => eprintln!("{label}_{}: {e}", view.key),
            }
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
