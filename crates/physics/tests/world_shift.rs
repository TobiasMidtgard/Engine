//! Following world shifts (a floating-origin move, or a turn into another planet's frame): the
//! simulation moves with the world in the same frame, without waking, jolting or forgetting
//! anything.

use engine_core::math::{DQuat, DVec3, EulerRot, Vec3};
use engine_core::prelude::*;
use engine_core::time::TimeSource;
use engine_physics::{PhysicsPlugin, PhysicsStats};
use engine_world::physics::{
    CharacterController, CharacterMove, CharacterState, Collider, ContactEvent, ContactPhase,
    Joint, JointKind, PhysicsQueries, QueryFilter, RigidBody, Velocity,
};
use engine_world::{Gravity, WorldPlugin, WorldShift};

fn app() -> App {
    let mut app = App::new();
    app.add_plugins(WorldPlugin);
    app.add_plugins(PhysicsPlugin);
    app.world
        .resource_mut::<Time>()
        .set_source(TimeSource::Manual {
            step_secs: 1.0 / 60.0,
        });
    app.world.resource_mut::<FixedTime>().set_hz(60.0);
    app.init_resource::<Started>();
    app.add_systems(Last, count_started);
    app
}

#[derive(Resource, Default)]
struct Started(usize);

fn count_started(mut n: ResMut<Started>, mut events: MessageReader<ContactEvent>) {
    n.0 += events
        .read()
        .filter(|e| e.phase == ContactPhase::Started)
        .count();
}

fn ground(app: &mut App) -> Entity {
    app.world
        .spawn((
            Transform::from_xyz(0.0, -0.5, 0.0),
            RigidBody::fixed(),
            Collider::cuboid(Vec3::new(8.0, 0.5, 8.0)),
        ))
        .id()
}

fn cube(app: &mut App, at: DVec3) -> Entity {
    app.world
        .spawn((
            Transform::from_translation(at),
            RigidBody::dynamic(),
            Collider::cuboid(Vec3::splat(0.25)),
            Velocity::default(),
        ))
        .id()
}

fn pos(app: &App, e: Entity) -> DVec3 {
    app.world.get::<Transform>(e).unwrap().translation
}

/// `e`'s position in `frame`'s own coordinates.
fn local_to(app: &App, frame: Entity, e: Entity) -> DVec3 {
    let f = app.world.get::<Transform>(frame).unwrap();
    f.rotation.as_dquat().inverse() * (pos(app, e) - f.translation)
}

fn shift(app: &mut App, s: WorldShift) {
    app.world.write_message(s);
    app.update();
}

fn awake(app: &App) -> usize {
    app.world.resource::<PhysicsStats>().awake
}

#[test]
fn floating_origin_moves_keep_sleeping_piles_asleep() {
    let mut app = app();
    let g = ground(&mut app);
    let mut crates = Vec::new();
    for i in 0..27 {
        let (x, y, z) = ((i % 3) as f64, (i / 9) as f64, ((i / 3) % 3) as f64);
        crates.push(cube(
            &mut app,
            DVec3::new(x * 0.6 - 0.6, 0.25 + y * 0.5, z * 0.6 - 0.6),
        ));
    }
    app.update_n(600);
    assert_eq!(awake(&app), 0, "the pile should be asleep before the shift");
    let before: Vec<DVec3> = crates.iter().map(|c| local_to(&app, g, *c)).collect();
    let started = app.world.resource::<Started>().0;
    for t in [
        DVec3::new(1.0e5, -2.0e3, 3.0e4),
        DVec3::new(-7.0e6, 0.0, 1.0e3),
    ] {
        shift(&mut app, WorldShift::translate(t));
        app.update_n(30);
        assert_eq!(awake(&app), 0, "a floating-origin move woke the pile");
    }
    for (c, b) in crates.iter().zip(&before) {
        let d = local_to(&app, g, *c).distance(*b);
        assert!(d < 1e-6, "a crate moved {d} m relative to the ground");
    }
    assert_eq!(
        app.world.resource::<Started>().0,
        started,
        "contacts restarted after the shift"
    );
}

#[test]
fn a_tilted_turn_keeps_a_stack_standing() {
    let mut app = app();
    let g = ground(&mut app);
    let stack: Vec<Entity> = (0..10)
        .map(|i| cube(&mut app, DVec3::new(0.0, 0.25 + 0.5 * i as f64, 0.0)))
        .collect();
    app.update_n(120);
    let before: Vec<DVec3> = stack.iter().map(|c| local_to(&app, g, *c)).collect();
    // A turn that tilts the ground: gravity turns with the world (engine_world's job).
    shift(
        &mut app,
        WorldShift {
            rotation: DQuat::from_euler(EulerRot::XYZ, 0.5, 0.3, -0.4),
            translation: DVec3::new(1.0e4, 50.0, -3.0e3),
            ..WorldShift::IDENTITY
        },
    );
    app.update_n(240);
    for (i, (c, b)) in stack.iter().zip(&before).enumerate() {
        let d = local_to(&app, g, *c).distance(*b);
        assert!(d < 0.02, "box {i} moved {d} m on the turned ground");
    }
}

#[test]
fn world_pinned_joints_turn_with_the_world() {
    let mut app = app();
    let pivot = DVec3::new(0.0, 5.0, 0.0);
    let bob = cube(&mut app, pivot + DVec3::new(2.0, 0.0, 0.0));
    app.world.spawn(Joint {
        kind: JointKind::Revolute {
            axis: Vec3::Z,
            limits: None,
            motor: None,
        },
        body_a: bob,
        body_b: None,
        anchor_a: Vec3::new(-2.0, 0.0, 0.0),
        anchor_b: pivot,
        break_force: None,
        collide_connected: false,
    });
    app.update_n(30);
    let s = WorldShift {
        rotation: DQuat::from_rotation_y(1.0) * DQuat::from_rotation_x(0.3),
        translation: DVec3::new(-500.0, 20.0, 800.0),
        ..WorldShift::IDENTITY
    };
    shift(&mut app, s);
    let (pivot, axis) = (s.point(pivot), s.direction(Vec3::Z).as_dvec3());
    for _ in 0..120 {
        app.update();
        let off = pos(&app, bob) - pivot;
        assert!(
            (off.length() - 2.0).abs() < 0.01,
            "the bob left its pivot: {off:?}"
        );
        assert!(
            off.dot(axis).abs() < 0.01,
            "the hinge axis didn't turn: {off:?}"
        );
    }
}

#[test]
fn a_moving_frame_adds_its_velocity() {
    let mut app = app();
    app.world.resource_mut::<Gravity>().uniform = Vec3::ZERO;
    let body = cube(&mut app, DVec3::ZERO);
    app.world.get_mut::<Velocity>(body).unwrap().linear = Vec3::new(1.0, 0.0, 0.0);
    app.update_n(10);
    shift(
        &mut app,
        WorldShift {
            velocity: DVec3::new(100.0, 0.0, -50.0),
            ..WorldShift::IDENTITY
        },
    );
    let start = pos(&app, body);
    app.update_n(60);
    let v = app.world.get::<Velocity>(body).unwrap().linear;
    assert!(
        (v - Vec3::new(101.0, 0.0, -50.0)).length() < 1e-3,
        "velocity in the new frame: {v:?}"
    );
    let moved = pos(&app, body) - start;
    assert!(
        (moved - DVec3::new(101.0, 0.0, -50.0)).length() < 0.1,
        "moved {moved:?} in 1 s"
    );
}

#[test]
fn characters_stand_through_shifts() {
    let mut app = app();
    let g = ground(&mut app);
    let ch = app
        .world
        .spawn((
            Transform::from_xyz(1.0, 0.92, 2.0),
            CharacterController::default(),
            CharacterMove { velocity: -Vec3::Y },
        ))
        .id();
    app.update_n(60);
    let before = local_to(&app, g, ch);
    for s in [
        WorldShift::translate(DVec3::new(3.0e5, 0.0, -1.0e5)),
        WorldShift {
            rotation: DQuat::from_rotation_y(2.0),
            translation: DVec3::new(10.0, 0.0, 10.0),
            ..WorldShift::IDENTITY
        },
    ] {
        shift(&mut app, s);
        // The game steers in the new frame; gravity's direction is unchanged by a turn about Y.
        app.update_n(60);
        let st = *app.world.get::<CharacterState>(ch).unwrap();
        assert!(st.grounded, "fell after the shift: {st:?}");
        let d = local_to(&app, g, ch).distance(before);
        assert!(d < 0.01, "the character slid {d} m");
    }
}

#[test]
fn queries_see_turned_bodies_at_once() {
    let mut app = app();
    ground(&mut app);
    let c = cube(&mut app, DVec3::new(3.0, 0.25, 0.0));
    app.update_n(120);
    let s = WorldShift {
        rotation: DQuat::from_rotation_y(std::f64::consts::FRAC_PI_2),
        translation: DVec3::new(40.0, 0.0, 0.0),
        ..WorldShift::IDENTITY
    };
    shift(&mut app, s);
    // No physics step has run since the shift: queries must already see the new frame.
    let at = pos(&app, c);
    assert!(at.distance(s.point(DVec3::new(3.0, 0.25, 0.0))) < 0.01);
    let q = app.world.resource::<PhysicsQueries>().clone();
    let hit = q.raycast(at + DVec3::Y * 3.0, -Vec3::Y, 10.0, &QueryFilter::default());
    assert_eq!(
        hit.map(|h| h.entity),
        Some(c),
        "the ray missed the turned crate"
    );
    let old = q.raycast(
        DVec3::new(3.0, 3.0, 0.0),
        -Vec3::Y,
        2.9,
        &QueryFilter::default(),
    );
    assert!(old.is_none(), "the crate is still where it was: {old:?}");
}
