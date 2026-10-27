//! Cloth that doesn't ride a dynamic body steps in `SimSet::DuringPhysics`, beside the
//! physics step, from frames snapshotted before it; cloth on a dynamic body (a sail on its
//! boat) still steps after physics and follows this step's pose.
//!
//! A system in `SimSet::Physics` stands in for the physics backend and slides each `Mover`
//! along +X every step. A body stepped after physics keeps its pins on the moved frame; one
//! stepped beside physics sees the move a step later.

use engine_core::prelude::*;
use engine_core::time::{FixedTime, TimeSource};
use engine_softbody::{Cloth, ClothPins, SoftBodyPlugin, SoftBodySim, SoftMaterial};
use engine_world::WeatherState;
use engine_world::physics::RigidBody;

/// How far each `Mover` slides per step, m.
const STEP_X: f32 = 0.02;

#[derive(Component)]
struct Mover;

#[derive(Resource, Default)]
struct Moving(bool);

fn slide(moving: Res<Moving>, mut movers: Query<&mut Transform, With<Mover>>) {
    if moving.0 {
        for mut t in &mut movers {
            t.translation.x += STEP_X as f64;
        }
    }
}

fn app() -> App {
    let mut app = App::new();
    app.add_plugins((engine_world::WorldPlugin, SoftBodyPlugin));
    app.world
        .resource_mut::<Time>()
        .set_source(TimeSource::Manual {
            step_secs: 1.0 / 60.0,
        });
    app.world.resource_mut::<FixedTime>().set_hz(60.0);
    app.init_resource::<Moving>()
        .add_systems(FixedUpdate, slide.in_set(SimSet::Physics));
    let mut weather = app.world.resource_mut::<WeatherState>();
    weather.wind = Vec3::ZERO;
    weather.gustiness = 0.0;
    app
}

/// Mean X of the body's pinned particles (hard pins, or the soft ties a sail laced to a
/// dynamic body gets) less the mover's X, m.
fn pin_offset(app: &App, body: Entity, mover: Entity) -> f64 {
    let b = app.world.get::<SoftBodySim>(body).unwrap();
    let tied: Vec<u32> = (b.sim.pins.iter().map(|p| p.p))
        .chain(b.sim.goals.iter().map(|g| g.p))
        .collect();
    assert!(!tied.is_empty(), "nothing tied");
    let x: f64 = tied
        .iter()
        .map(|&p| b.world_position(p as usize).x)
        .sum::<f64>()
        / tied.len() as f64;
    x - app.world.get::<Transform>(mover).unwrap().translation.x
}

/// Pins' offset from the mover at rest, then after sliding it for a while.
fn offsets(app: &mut App, body: Entity, mover: Entity) -> (f64, f64) {
    app.update_n(60);
    assert!(app.world.get::<SoftBodySim>(body).is_some(), "body built");
    let still = pin_offset(app, body, mover);
    app.world.resource_mut::<Moving>().0 = true;
    app.update_n(30);
    (still, pin_offset(app, body, mover))
}

#[test]
fn a_sail_on_its_boat_tracks_the_mast_in_the_same_step() {
    let mut app = app();
    let boat = app
        .world
        .spawn((
            Transform::from_xyz(0.0, 0.0, 0.0),
            RigidBody::dynamic().with_mass(200.0),
            Mover,
        ))
        .id();
    let rig = app
        .world
        .spawn((Transform::from_xyz(0.0, 0.0, -1.2), ChildOf(boat)))
        .id();
    let corners = [[0.1, 3.4], [1.9, 4.7], [2.85, 1.13], [0.1, 1.12]];
    let sail = app
        .world
        .spawn((
            Transform::from_rotation(Quat::from_rotation_y(-std::f32::consts::FRAC_PI_2)),
            Cloth::sail(corners, SoftMaterial::hex(0xf2ead8)),
            ChildOf(rig),
        ))
        .id();
    let (still, moving) = offsets(&mut app, sail, boat);
    let b = app.world.get::<SoftBodySim>(sail).unwrap();
    assert!(
        !b.steps_beside_physics(),
        "a sail on a boat steps after physics"
    );
    assert!(
        (moving - still).abs() < 0.2 * STEP_X as f64,
        "the sail's pins lag the mast: {still} at rest, {moving} sailing"
    );
}

#[test]
fn cloth_beside_physics_reads_only_the_snapshot() {
    let mut app = app();
    // A curtain rod moved by "physics" but not a rigid body itself.
    let rod = app
        .world
        .spawn((Transform::from_xyz(0.0, 3.0, 0.0), Mover))
        .id();
    let curtain = app
        .world
        .spawn((
            Transform::IDENTITY,
            Cloth {
                size: [1.0, 1.5],
                resolution: [12, 18],
                pins: ClothPins::TopEdge { every: 1 },
                ..Cloth::default()
            },
            ChildOf(rod),
        ))
        .id();
    let (still, moving) = offsets(&mut app, curtain, rod);
    let b = app.world.get::<SoftBodySim>(curtain).unwrap();
    assert!(
        b.steps_beside_physics(),
        "a curtain on a rod steps beside physics"
    );
    // Its frame was snapshotted before this step's move, so its pins trail by one step.
    let lag = still - moving;
    assert!(
        (lag - STEP_X as f64).abs() < 0.2 * STEP_X as f64,
        "pins should trail the rod by one step ({STEP_X} m), trail {lag} m"
    );
}
