//! Renders effects close up, by day and at dusk, into `screenshots/vfx/` (one PNG each).
//!
//! ```sh
//! cargo run -p engine_vfx --release --example gallery                   # every effect
//! cargo run -p engine_vfx --release --example gallery -- fire.torch spell.fireball
//! ```
//!
//! Effects with a trail fly in a circle so the trail shows; one-shot effects are captured
//! shortly after they start, loops once they have settled. Needs a GPU or lavapipe.

use engine_core::math::{DVec3, Vec3};
use engine_core::prelude::*;
use engine_core::time::TimeSource;
use engine_render::prelude::*;
use engine_render::{RenderDisabled, capture_now};
use engine_vfx::prelude::*;

const W: u32 = 640;
const H: u32 = 360;
const STEP: f64 = 1.0 / 20.0;

#[derive(Component)]
struct Sun;

#[derive(Component)]
struct Circling;

fn circle(time: Res<Time>, mut q: Query<&mut Transform, With<Circling>>) {
    let t = time.elapsed_secs_f64() * 2.5;
    for mut tr in &mut q {
        tr.translation = DVec3::new(t.cos() * 2.2, 1.2, t.sin() * 1.2 - 0.5);
    }
}

fn main() {
    let only: Vec<String> = std::env::args().skip(1).collect();
    let out = std::path::Path::new("screenshots/vfx");
    std::fs::create_dir_all(out).expect("create screenshots/vfx");

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
    app.add_systems(Update, circle);
    let w = &mut app.world;
    w.spawn((Sun, DirectionalLight::default(), Transform::default()));
    w.spawn((
        Transform::default(),
        MeshRenderer::new(
            Shape::Plane {
                size: [80.0, 80.0],
                subdivisions: 1,
            },
            StandardMaterial::hex(0x6b655d).with_roughness(0.9),
        ),
    ));
    w.spawn((
        Transform::from_xyz(0.0, 2.5, -4.0),
        MeshRenderer::new(
            Shape::Cuboid {
                size: [12.0, 5.0, 0.4],
            },
            StandardMaterial::hex(0x8a8580).with_roughness(0.85),
        ),
    ));
    let camera = w
        .spawn((
            Camera::perspective(50.0),
            Transform::from_xyz(0.0, 1.7, 5.5).looking_at(DVec3::new(0.0, 0.9, 0.0), Vec3::Y),
        ))
        .id();
    app.update();
    if let Some(d) = app.world.get_resource::<RenderDisabled>() {
        eprintln!("no GPU: {}", d.0);
        return;
    }

    let names: Vec<String> = {
        let lib = app.world.resource::<EffectLibrary>();
        lib.names()
            .into_iter()
            .filter(|n| only.is_empty() || only.iter().any(|o| n.starts_with(o.as_str())))
            .map(str::to_string)
            .collect()
    };
    // Ribbons without particles: a lightning beam and a bare trail.
    let ribbons: Vec<&str> = ["ribbon.beam", "ribbon.trail"]
        .into_iter()
        .filter(|n| only.is_empty() || only.iter().any(|o| n.starts_with(o.as_str())))
        .collect();
    // (label, sun lux, sun elevation in degrees, EV100)
    let lights = [("day", 90_000.0, 40.0f32, 14.5), ("dusk", 300.0, 2.0, 5.5)];
    for (label, lux, elevation, ev) in lights {
        {
            let mut q = app
                .world
                .query_filtered::<(&mut DirectionalLight, &mut Transform), With<Sun>>();
            for (mut light, mut t) in q.iter_mut(&mut app.world) {
                light.illuminance = lux;
                let e = elevation.to_radians();
                let dir = DVec3::new(
                    -0.6 * e.cos() as f64,
                    -(e.sin() as f64),
                    -0.8 * e.cos() as f64,
                );
                *t = Transform::default().looking_at(dir, Vec3::Y);
            }
            let mut cam = app.world.get_mut::<Camera>(camera).unwrap();
            cam.exposure = Exposure::Manual { ev100: ev };
        }
        for name in &ribbons {
            let entity = if *name == "ribbon.beam" {
                app.world
                    .spawn((
                        Transform::from_xyz(-2.5, 2.2, 0.0),
                        Beam {
                            target: DVec3::new(2.5, 0.6, 0.0),
                            ..Default::default()
                        },
                    ))
                    .id()
            } else {
                app.world
                    .spawn((Transform::default(), Trail::default(), Circling))
                    .id()
            };
            for _ in 0..20 {
                app.update();
            }
            let path = out.join(format!("{label}_{name}.png"));
            match capture_now(&mut app.world, &path) {
                Ok(_) => println!("{}", path.display()),
                Err(e) => eprintln!("{name}: {e}"),
            }
            app.world.despawn(entity);
            for _ in 0..20 {
                app.update();
            }
        }
        for name in &names {
            let desc = app
                .world
                .resource::<EffectLibrary>()
                .get(name)
                .cloned()
                .unwrap();
            let grounded = ["impact.", "step.", "impulse.", "water.splash", "explosion."]
                .iter()
                .any(|p| name.starts_with(p));
            let pos = if grounded {
                DVec3::ZERO
            } else {
                DVec3::new(0.0, 1.0, 0.0)
            };
            let mut e = app.world.spawn((
                Transform::from_translation(pos),
                ParticleEffect::new(name.clone()),
            ));
            if desc.trail.is_some() {
                e.insert(Circling);
            }
            let entity = e.id();
            let secs = if desc.is_one_shot() {
                (desc.total_secs().unwrap_or(1.0) * 0.2).clamp(0.15, 0.6)
            } else {
                1.5
            };
            for _ in 0..(secs / STEP as f32).ceil() as u32 {
                app.update();
            }
            let path = out.join(format!("{label}_{name}.png"));
            match capture_now(&mut app.world, &path) {
                Ok(_) => println!("{}", path.display()),
                Err(e) => eprintln!("{name}: {e}"),
            }
            // Clear the stage: remove the effect and let every particle die.
            app.world.despawn(entity);
            for _ in 0..(desc.total_secs().unwrap_or(4.0).min(4.0) / STEP as f32).ceil() as u32 + 2
            {
                app.update();
            }
        }
    }
}
