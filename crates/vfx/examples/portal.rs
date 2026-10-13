//! Renders `spell.portal_gate` standing between two pillars, by day and at dusk, into
//! `screenshots/vfx/portal/`, with colored posts behind it so the swirl through it shows.
//!
//! ```sh
//! cargo run -p engine_vfx --release --example portal
//! ```
//!
//! Needs a GPU or lavapipe.

use engine_core::math::{DVec3, Quat, Vec3};
use engine_core::prelude::*;
use engine_core::time::TimeSource;
use engine_render::prelude::*;
use engine_render::{RenderDisabled, capture_now};
use engine_vfx::prelude::*;

const W: u32 = 960;
const H: u32 = 540;
const STEP: f64 = 1.0 / 20.0;
/// Height of the portal's center.
const CENTER: f64 = 2.7;

#[derive(Component)]
struct Sun;

fn main() {
    let out = std::path::Path::new("screenshots/vfx/portal");
    std::fs::create_dir_all(out).expect("create screenshots/vfx/portal");

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
    let w = &mut app.world;
    w.spawn((Sun, DirectionalLight::default(), Transform::default()));
    w.spawn((
        Transform::from_xyz(0.0, -0.01, 0.0),
        MeshRenderer::new(
            Shape::Plane {
                size: [120.0, 120.0],
                subdivisions: 1,
            },
            StandardMaterial::hex(0x5d6b4a).with_roughness(0.9),
        ),
    ));
    // Pillars either side of the portal.
    for x in [-2.85, 2.85] {
        w.spawn((
            Transform::from_xyz(x, 2.6, 0.0),
            MeshRenderer::new(
                Shape::Cuboid {
                    size: [0.7, 5.2, 0.9],
                },
                StandardMaterial::hex(0x9a9286).with_roughness(0.85),
            ),
        ));
    }
    // Colored posts behind, near and far, to see the world bend through it.
    let colors = [0xc0392b, 0xe67e22, 0xf1c40f, 0x27ae60, 0x2980b9, 0x8e44ad];
    for (i, color) in colors.into_iter().enumerate() {
        for (row, z) in [(0, -4.0), (1, -9.0)] {
            let x = (i as f64 - 2.5) * 1.6 + row as f64 * 0.8;
            w.spawn((
                Transform::from_xyz(x, 1.8, z),
                MeshRenderer::new(
                    Shape::Cuboid {
                        size: [0.5, 3.6, 0.5],
                    },
                    StandardMaterial::hex(color).with_roughness(0.6),
                ),
            ));
        }
    }
    // Facing the camera (+Z).
    w.spawn((
        Transform::from_xyz(0.0, CENTER, 0.0)
            .with_rotation(Quat::from_rotation_arc(Vec3::Y, Vec3::Z)),
        ParticleEffect::new("spell.portal_gate"),
    ));
    let camera = w
        .spawn((
            Camera::perspective(55.0),
            Transform::from_xyz(1.6, 2.2, 8.0).looking_at(DVec3::new(0.0, CENTER, 0.0), Vec3::Y),
        ))
        .id();
    app.update();
    if let Some(d) = app.world.get_resource::<RenderDisabled>() {
        eprintln!("no GPU: {}", d.0);
        return;
    }
    for _ in 0..40 {
        app.update();
    }

    // (label, sun lux, sun elevation in degrees, EV100)
    let lights = [("day", 90_000.0, 35.0f32, 14.5), ("dusk", 400.0, 4.0, 6.0)];
    let views = [
        ("front", DVec3::new(1.6, 2.2, 8.0)),
        ("side", DVec3::new(6.5, 1.8, 5.5)),
    ];
    for (label, lux, elevation, ev) in lights {
        {
            let mut q = app
                .world
                .query_filtered::<(&mut DirectionalLight, &mut Transform), With<Sun>>();
            for (mut light, mut t) in q.iter_mut(&mut app.world) {
                light.illuminance = lux;
                let e = elevation.to_radians();
                let dir = DVec3::new(
                    -0.7 * e.cos() as f64,
                    -(e.sin() as f64),
                    -0.7 * e.cos() as f64,
                );
                *t = Transform::default().looking_at(dir, Vec3::Y);
            }
            let mut cam = app.world.get_mut::<Camera>(camera).unwrap();
            cam.exposure = Exposure::Manual { ev100: ev };
        }
        for (name, eye) in views {
            *app.world.get_mut::<Transform>(camera).unwrap() =
                Transform::from_translation(eye).looking_at(DVec3::new(0.0, CENTER, 0.0), Vec3::Y);
            for _ in 0..3 {
                app.update();
            }
            let path = out.join(format!("{label}_{name}.png"));
            match capture_now(&mut app.world, &path) {
                Ok(_) => println!("{}", path.display()),
                Err(e) => eprintln!("{name}: {e}"),
            }
        }
    }
}
