//! Renders the Spell court's fountain (`water.fountain` on its basin) close up by day and at
//! dusk into `screenshots/vfx/fountain/`: the whole fountain, the jet and where it lands.
//!
//! ```sh
//! cargo run -p engine_vfx --release --example fountain
//! cargo run -p engine_vfx --release --example fountain -- dusk
//! ```
//!
//! Needs a GPU or lavapipe.

use engine_core::math::{DVec3, Vec3};
use engine_core::prelude::*;
use engine_core::time::TimeSource;
use engine_render::prelude::*;
use engine_render::{MaterialRef, MeshRef, RenderDisabled, capture_now};
use engine_vfx::demo::art::{self, props};
use engine_vfx::prelude::*;

const W: u32 = 960;
const H: u32 = 540;
const STEP: f64 = 1.0 / 60.0;

/// (label, eye, target)
const VIEWS: &[(&str, [f64; 3], [f64; 3])] = &[
    ("whole", [3.2, 1.9, 4.6], [0.0, 1.2, 0.0]),
    ("jet", [1.2, 1.9, 2.2], [0.0, 2.2, 0.0]),
    ("landing", [1.3, 1.2, 1.9], [0.0, 0.55, 0.0]),
];

fn main() {
    let lighting = ["day", "dusk"];
    let args: Vec<String> = std::env::args().skip(1).collect();
    let when: Vec<&String> = args
        .iter()
        .filter(|a| lighting.contains(&a.as_str()))
        .collect();
    let out = std::path::Path::new("screenshots/vfx/fountain");
    std::fs::create_dir_all(out).expect("create screenshots/vfx/fountain");

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
    for (mesh, material) in art::register(&mut app.world, "court.fountain", props::water::fountain)
    {
        app.world.spawn((
            Transform::default(),
            MeshRenderer::new(MeshRef::Handle(mesh), MaterialRef::Handle(material)),
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
    app.world.spawn((
        Transform::from_xyz(0.0, props::water::NOZZLE as f64, 0.0),
        ParticleEffect::new("water.fountain"),
    ));

    // (label, sun lux, sun elevation in degrees, sun color)
    let lights = [
        ("day", 60_000.0, 40.0f32, [1.0, 0.96, 0.9]),
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
            // Sun from behind and to the left, so the water is back lit and glints.
            let dir = DVec3::new(
                0.6 * e.cos() as f64,
                -(e.sin() as f64),
                0.8 * e.cos() as f64,
            );
            *app.world.get_mut::<Transform>(sun).unwrap() =
                Transform::default().looking_at(dir, Vec3::Y);
        }
        for &(view, eye, target) in VIEWS {
            *app.world.get_mut::<Transform>(camera).unwrap() =
                Transform::from_translation(DVec3::from(eye))
                    .looking_at(DVec3::from(target), Vec3::Y);
            // Let the jet run and the exposure settle.
            app.update_n(90);
            let path = out.join(format!("{label}_{view}.png"));
            match capture_now(&mut app.world, &path) {
                Ok(_) => println!("{}", path.display()),
                Err(e) => eprintln!("{label}_{view}: {e}"),
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
