//! Renders the Spell court's braziers burning (`fire.torch` on the brazier prop) into
//! `screenshots/vfx/braziers/`: close up, from a few steps away and from across the court,
//! by day, at dusk and at night.
//!
//! ```sh
//! cargo run -p engine_vfx --release --example braziers
//! ```
//!
//! Needs a GPU or lavapipe.

use engine_core::math::{DVec3, Quat, Vec3};
use engine_core::prelude::*;
use engine_core::time::TimeSource;
use engine_render::prelude::*;
use engine_render::{MaterialRef, MeshRef, RenderDisabled, capture_now};
use engine_vfx::demo::art::{self, props};
use engine_vfx::prelude::*;

const W: u32 = 960;
const H: u32 = 540;
const STEP: f64 = 1.0 / 30.0;

fn main() {
    let out = std::path::Path::new("screenshots/vfx/braziers");
    std::fs::create_dir_all(out).expect("create screenshots/vfx/braziers");

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
    app.add_plugins((RenderPlugin, VfxPlugin));
    let sun = app
        .world
        .spawn((DirectionalLight::default(), Transform::default()))
        .id();
    let camera = app
        .world
        .spawn((Camera::perspective(50.0), Transform::default()))
        .id();
    app.update();
    if let Some(d) = app.world.get_resource::<RenderDisabled>() {
        eprintln!("no GPU: {}", d.0);
        return;
    }

    // The court's flagstones, and three braziers along their edge.
    let spawn = |world: &mut World, key: &str, at: Transform, build: fn() -> art::Prop| {
        for (mesh, material) in art::register(world, key, build) {
            world.spawn((
                at,
                MeshRenderer::new(MeshRef::Handle(mesh), MaterialRef::Handle(material)),
            ));
        }
    };
    spawn(&mut app.world, "floor", Transform::default(), || {
        props::court::floor(21.0, &engine_vfx::demo::clearings())
    });
    let braziers = [
        DVec3::new(0.0, 0.0, 0.0),
        DVec3::new(-3.2, 0.0, -1.2),
        DVec3::new(3.4, 0.0, -1.6),
    ];
    for (i, p) in braziers.iter().enumerate() {
        spawn(
            &mut app.world,
            "brazier",
            Transform::from_translation(*p).with_rotation(Quat::from_rotation_y(i as f32 * 0.7)),
            props::rim::brazier,
        );
        app.world.spawn((
            Transform::from_translation(*p + DVec3::Y * props::rim::COALS as f64),
            ParticleEffect::new("fire.torch"),
        ));
    }
    app.update_n(60);

    let flame = DVec3::Y * (props::rim::COALS as f64 + 0.25);
    // (label, eye, target)
    let views = [
        ("close", DVec3::new(0.9, 2.2, 1.5), flame),
        (
            "steps",
            DVec3::new(1.5, 1.8, 6.0),
            DVec3::new(0.0, 1.5, -0.8),
        ),
        (
            "court",
            DVec3::new(4.0, 1.7, 18.0),
            DVec3::new(0.0, 1.6, -1.0),
        ),
    ];
    // (label, sun lux, sun elevation in degrees, sun color); the camera exposes on its own,
    // as the showcase's does.
    let lights = [
        ("day", 60_000.0, 40.0f32, [1.0, 0.96, 0.9]),
        ("dusk", 250.0, 2.0, [1.0, 0.7, 0.45]),
        ("night", 0.2, 35.0, [0.6, 0.7, 1.0]),
    ];
    for (label, lux, elevation, color) in lights {
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
        // The sky and its lighting catch up with a sudden change over a few seconds.
        app.update_n(100);
        for (view, eye, target) in views {
            *app.world.get_mut::<Transform>(camera).unwrap() =
                Transform::from_translation(eye).looking_at(target, Vec3::Y);
            app.update_n(8);
            let path = out.join(format!("{label}_{view}.png"));
            match capture_now(&mut app.world, &path) {
                Ok(_) => println!("{}", path.display()),
                Err(e) => eprintln!("{label}_{view}: {e}"),
            }
        }
    }
}
