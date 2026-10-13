//! Renders effects built on sub-emitters into `screenshots/vfx/sub_emitters/`: the
//! `spell.starburst` firework at dusk as its shell bursts, droops and crackles, and
//! `impact.metal` sparks splitting as they skid across a steel plate.
//!
//! ```sh
//! cargo run -p engine_vfx --release --example sub_emitters
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
const STEP: f64 = 1.0 / 30.0;

fn main() {
    let out = std::path::Path::new("screenshots/vfx/sub_emitters");
    std::fs::create_dir_all(out).expect("create screenshots/vfx/sub_emitters");

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
    app.insert_resource(engine_world::Ground(std::sync::Arc::new(Flat)));
    let w = &mut app.world;
    // Dusk: the sun just under the horizon.
    w.spawn((
        DirectionalLight {
            illuminance: 120.0,
            ..Default::default()
        },
        Transform::default().looking_at(DVec3::new(-0.6, -0.05, -0.8), Vec3::Y),
    ));
    w.spawn((
        Transform::from_xyz(0.0, 0.0, 0.0),
        MeshRenderer::new(
            Shape::Plane {
                size: [400.0, 400.0],
                subdivisions: 1,
            },
            StandardMaterial::hex(0x3d4636).with_roughness(0.95),
        ),
    ));
    // Towers for scale under the burst.
    for (x, z, h) in [(-14.0, -6.0, 12.0), (-9.0, -2.0, 7.0), (11.0, -8.0, 15.0)] {
        w.spawn((
            Transform::from_xyz(x, h / 2.0, z),
            MeshRenderer::new(
                Shape::Cuboid {
                    size: [3.0, h as f32, 3.0],
                },
                StandardMaterial::hex(0x6b655c).with_roughness(0.9),
            ),
        ));
    }
    // A steel plate for the sparks.
    let plate = DVec3::new(30.0, 0.0, 30.0);
    w.spawn((
        Transform::from_translation(plate + DVec3::new(0.0, 0.05, 0.0)),
        MeshRenderer::new(
            Shape::Cuboid {
                size: [3.0, 0.1, 2.0],
            },
            StandardMaterial::hex(0x55585c)
                .with_roughness(0.4)
                .with_metallic(1.0),
        ),
    ));
    let camera = w
        .spawn((
            Camera::perspective(60.0).with_exposure(Exposure::Manual { ev100: 4.5 }),
            Transform::from_xyz(0.0, 2.0, 42.0).looking_at(DVec3::new(0.0, 17.0, 0.0), Vec3::Y),
        ))
        .id();
    app.update();
    if let Some(d) = app.world.get_resource::<RenderDisabled>() {
        eprintln!("no GPU: {}", d.0);
        return;
    }
    app.update_n(10);

    let capture = |app: &mut App, name: &str| {
        let path = out.join(name);
        match capture_now(&mut app.world, &path) {
            Ok(_) => println!("{}", path.display()),
            Err(e) => eprintln!("{name}: {e}"),
        }
    };

    // The firework: launch, then shots as the shell bursts and the stars droop and crackle.
    app.world.spawn((
        Transform::default(),
        ParticleEffect::once("spell.starburst"),
    ));
    let mut t = 0.0;
    for (at, name) in [
        (0.9, "starburst_1_climb.png"),
        (1.95, "starburst_2_burst.png"),
        (2.6, "starburst_3_open.png"),
        (3.5, "starburst_4_droop.png"),
        (4.3, "starburst_5_crackle.png"),
    ] {
        while t < at {
            app.update();
            t += STEP;
        }
        capture(&mut app, name);
    }

    // Sparks: a strike on the plate seen from close by, in daylight.
    {
        let mut q = app.world.query::<(&mut DirectionalLight, &mut Transform)>();
        for (mut light, mut t) in q.iter_mut(&mut app.world) {
            light.illuminance = 30_000.0;
            *t = Transform::default().looking_at(DVec3::new(-0.5, -0.7, -0.4), Vec3::Y);
        }
        let mut cam = app.world.get_mut::<Camera>(camera).unwrap();
        cam.exposure = Exposure::Manual { ev100: 12.5 };
    }
    *app.world.get_mut::<Transform>(camera).unwrap() =
        Transform::from_translation(plate + DVec3::new(1.6, 1.0, 2.4))
            .looking_at(plate + DVec3::new(0.0, 0.2, 0.0), Vec3::Y);
    app.update_n(4);
    for (i, after) in [0.12, 0.3].into_iter().enumerate() {
        app.world.spawn((
            Transform::from_translation(plate + DVec3::new(0.0, 0.12, 0.0)),
            ParticleEffect::once("impact.metal"),
        ));
        let mut t = 0.0;
        while t < after {
            app.update();
            t += STEP;
        }
        capture(&mut app, &format!("sparks_{i}.png"));
        app.update_n(45);
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
