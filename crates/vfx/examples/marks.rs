//! Renders every kind of mark, by day and at dusk, into `screenshots/vfx/marks/`: a scorch
//! (with glowing embers), frost, splashed stone and chipped stone on a paved slab, and a
//! scorch beside frost on a wall, as spells leave them.
//!
//! ```sh
//! cargo run -p engine_vfx --release --example marks
//! ```
//!
//! Needs a GPU or lavapipe.

use engine_core::math::{DVec3, Quat, Vec3};
use engine_core::prelude::*;
use engine_core::time::TimeSource;
use engine_render::prelude::*;
use engine_render::{RenderDisabled, capture_now};
use engine_vfx::marks::{Mark, MarkDesc, MarkKind, Marks};
use engine_vfx::prelude::*;

const W: u32 = 960;
const H: u32 = 540;
const STEP: f64 = 1.0 / 20.0;

#[derive(Component)]
struct Sun;

fn main() {
    let out = std::path::Path::new("screenshots/vfx/marks");
    std::fs::create_dir_all(out).expect("create screenshots/vfx/marks");

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
                size: [80.0, 80.0],
                subdivisions: 1,
            },
            StandardMaterial::hex(0x5d6b4a).with_roughness(0.9),
        ),
    ));
    // A wall behind, facing the camera, for marks left by spells that hit it.
    w.spawn((
        Transform::from_xyz(0.0, 1.5, 0.0),
        MeshRenderer::new(
            Shape::Cuboid {
                size: [6.0, 3.0, 0.6],
            },
            StandardMaterial::hex(0x9a9286).with_roughness(0.85),
        ),
    ));
    // A paved slab in front for scorch, frost, wet and chips.
    w.spawn((
        Transform::from_xyz(0.0, 0.0, 4.2),
        MeshRenderer::new(
            Shape::Cuboid {
                size: [9.4, 0.04, 2.4],
            },
            StandardMaterial::hex(0x8d877e).with_roughness(0.8),
        ),
    ));
    let camera = w
        .spawn((
            Camera::perspective(55.0),
            Transform::from_xyz(0.0, 3.4, 9.5).looking_at(DVec3::new(0.0, 0.0, 0.6), Vec3::Y),
        ))
        .id();
    app.update();
    if let Some(d) = app.world.get_resource::<RenderDisabled>() {
        eprintln!("no GPU: {}", d.0);
        return;
    }

    leave_marks(&mut app.world.resource_mut::<Marks>());
    // Let the scorch's embers cool a little and the frost settle.
    for _ in 0..20 {
        app.update();
    }

    // (label, sun lux, sun elevation in degrees, EV100)
    let lights = [("day", 90_000.0, 35.0f32, 14.5), ("dusk", 400.0, 4.0, 6.0)];
    let views = [
        ("wide", DVec3::new(0.0, 3.4, 9.5), DVec3::new(0.0, 0.0, 0.6)),
        ("wall", DVec3::new(0.6, 1.7, 4.6), DVec3::new(0.0, 1.5, 0.3)),
        ("slab", DVec3::new(0.0, 1.8, 7.4), DVec3::new(0.0, 0.0, 4.2)),
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
        for (name, eye, target) in views {
            *app.world.get_mut::<Transform>(camera).unwrap() =
                Transform::from_translation(eye).looking_at(target, Vec3::Y);
            app.update();
            let path = out.join(format!("{label}_{name}.png"));
            match capture_now(&mut app.world, &path) {
                Ok(_) => println!("{}", path.display()),
                Err(e) => eprintln!("{name}: {e}"),
            }
        }
    }
}

fn leave_marks(marks: &mut Marks) {
    let desc = |kind, radius, glow| MarkDesc {
        kind,
        radius,
        life_secs: 600.0,
        strength: 1.0,
        glow_secs: glow,
    };
    // On the wall: a fireball's scorch beside a frost bolt's frost, facing the camera.
    let facing = Quat::from_rotation_arc(Vec3::Y, Vec3::Z);
    let on_wall = |x: f64| DVec3::new(x, 1.6, 0.3);
    marks.add(Mark::from_effect(
        &desc(MarkKind::Scorch, 0.9, 30.0),
        on_wall(-1.1),
        facing,
        1.0,
    ));
    marks.add(Mark::from_effect(
        &desc(MarkKind::Frost, 1.0, 0.0),
        on_wall(1.1),
        facing,
        1.0,
    ));
    // On the slab: what spells and shots leave on the ground.
    let up = Quat::IDENTITY;
    let on_slab = |x: f64| DVec3::new(x, 0.02, 4.2);
    marks.add(Mark::from_effect(
        &desc(MarkKind::Scorch, 1.0, 30.0),
        on_slab(-3.3),
        up,
        1.0,
    ));
    marks.add(Mark::from_effect(
        &desc(MarkKind::Frost, 1.0, 0.0),
        on_slab(-0.9),
        up,
        1.0,
    ));
    marks.add(Mark::from_effect(
        &desc(MarkKind::Wet, 0.9, 0.0),
        on_slab(1.4),
        up,
        1.0,
    ));
    for (dx, dz) in [(0.0, 0.0), (0.35, 0.25), (-0.2, 0.4), (0.25, -0.3)] {
        marks.add(Mark::from_effect(
            &desc(MarkKind::Chip, 0.08, 0.0),
            on_slab(3.4 + dx) + DVec3::new(0.0, 0.0, dz),
            up,
            1.0,
        ));
    }
}
