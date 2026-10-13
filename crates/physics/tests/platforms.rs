//! Riding moving bodies: crates and characters on lifts, turntables and ships, up to orbital
//! speed, must behave like they do on solid ground.

use engine_core::math::{DVec3, Vec3};
use engine_core::prelude::*;
use engine_core::time::TimeSource;
use engine_physics::PhysicsPlugin;
use engine_world::physics::{
    CharacterController, CharacterMove, CharacterState, Collider, ColliderShape, ExternalForce,
    Joint, JointKind, PhysicsQueries, QueryFilter, RigidBody, Velocity,
};
use engine_world::{Gravity, WorldPlugin};

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
    app
}

fn pos(app: &App, e: Entity) -> DVec3 {
    app.world.get::<Transform>(e).unwrap().translation
}

fn state(app: &App, e: Entity) -> CharacterState {
    *app.world.get::<CharacterState>(e).unwrap()
}

fn character(app: &mut App, at: DVec3, velocity: Vec3) -> Entity {
    app.world
        .spawn((
            Transform::from_translation(at),
            CharacterController::default(),
            CharacterMove { velocity },
        ))
        .id()
}

fn velocity(linear: Vec3, angular: Vec3) -> Velocity {
    Velocity { linear, angular }
}

/// A 20 × 20 m deck (top at y = 0) moving at `speed` along +x, with a 5-crate stack and a
/// character standing on it. Returns how far the stack and the character drifted on the deck.
fn ride_deck(speed: f32, secs: f64) -> (f64, f64) {
    let mut app = app();
    let v = Vec3::new(speed, 0.0, 0.0);
    let deck = app
        .world
        .spawn((
            Transform::from_xyz(0.0, -0.5, 0.0),
            RigidBody::kinematic(),
            Collider::cuboid(Vec3::new(10.0, 0.5, 10.0)),
            velocity(v, Vec3::ZERO),
        ))
        .id();
    let crates: Vec<Entity> = (0..5)
        .map(|i| {
            app.world
                .spawn((
                    Transform::from_xyz(2.0, 0.5 + i as f64, 0.0),
                    RigidBody::dynamic(),
                    Collider::cuboid(Vec3::splat(0.5)),
                    velocity(v, Vec3::ZERO),
                ))
                .id()
        })
        .collect();
    let ch = character(&mut app, DVec3::new(-3.0, 0.92, 0.0), -Vec3::Y);
    let start: Vec<DVec3> = crates.iter().map(|c| pos(&app, *c)).collect();
    let (deck0, ch0) = (pos(&app, deck), pos(&app, ch));
    let mut stack_drift = 0.0f64;
    for _ in 0..(secs * 60.0) as usize {
        app.update();
        let moved = pos(&app, deck) - deck0;
        for (c, s) in crates.iter().zip(&start) {
            stack_drift = stack_drift.max((pos(&app, *c) - moved - *s).length());
        }
    }
    let moved = pos(&app, deck) - deck0;
    assert!(
        (moved.x - speed as f64 * secs).abs() < 1.0,
        "deck travelled {moved:?}"
    );
    let st = state(&app, ch);
    assert!(st.grounded, "{speed} m/s: character fell off: {st:?}");
    assert_eq!(st.ground_entity, Some(deck));
    assert!(
        st.velocity.length() < 1.0e-3,
        "{speed} m/s: velocity is the character's own: {st:?}"
    );
    (stack_drift, (pos(&app, ch) - moved - ch0).length())
}

#[test]
fn crates_and_characters_ride_a_deck_at_orbital_speed() {
    for speed in [0.0, 300.0, 7_800.0, 30_000.0] {
        let (stack, ch) = ride_deck(speed, 5.0);
        assert!(stack < 0.02, "{speed} m/s: the stack drifted {stack} m");
        assert!(ch < 0.02, "{speed} m/s: the character drifted {ch} m");
    }
}

#[test]
fn characters_ride_lifts_up_and_down() {
    let mut app = app();
    let lift = app
        .world
        .spawn((
            Transform::from_xyz(0.0, -0.25, 0.0),
            RigidBody::kinematic(),
            Collider::cuboid(Vec3::new(2.0, 0.25, 2.0)),
            velocity(Vec3::new(0.0, 3.0, 0.0), Vec3::ZERO),
        ))
        .id();
    let ch = character(
        &mut app,
        DVec3::new(0.0, 0.92, 0.0),
        Vec3::new(0.0, -1.0, 0.0),
    );
    app.update_n(120);
    let (l, c) = (pos(&app, lift).y, pos(&app, ch).y);
    assert!((l - 5.75).abs() < 0.05, "lift at {l}");
    assert!(
        (c - l - 0.25 - 0.92).abs() < 0.05,
        "rising: character at {c}, lift at {l}"
    );
    assert!(state(&app, ch).grounded);
    // Going down at 3 m/s is faster than the character falls by itself in 2 s (1 m/s here):
    // it must stay on the floor.
    app.world.get_mut::<Velocity>(lift).unwrap().linear = Vec3::new(0.0, -3.0, 0.0);
    app.update_n(60);
    let (l, c) = (pos(&app, lift).y, pos(&app, ch).y);
    assert!(
        (c - l - 0.25 - 0.92).abs() < 0.05,
        "sinking: character at {c}, lift at {l}"
    );
    assert!(state(&app, ch).grounded);
}

#[test]
fn characters_turn_with_a_turntable() {
    let mut app = app();
    app.world.spawn((
        Transform::from_xyz(0.0, -0.15, 0.0),
        RigidBody::kinematic(),
        Collider {
            shape: ColliderShape::Cylinder {
                radius: 6.0,
                half_height: 0.15,
            },
            ..Default::default()
        },
        velocity(Vec3::ZERO, Vec3::new(0.0, 1.0, 0.0)),
    ));
    let ch = character(&mut app, DVec3::new(4.0, 0.92, 0.0), -Vec3::Y);
    // Half a turn.
    app.update_n((std::f64::consts::PI * 60.0).round() as usize);
    let p = pos(&app, ch);
    assert!(
        (p.x + 4.0).abs() < 0.1 && p.z.abs() < 0.1,
        "character should be across the turntable: {p:?}"
    );
    assert!(state(&app, ch).grounded);
}

#[test]
fn crates_ride_a_turntable_without_creeping_off() {
    // Rapier reuses contacts while two bodies barely move relative to each other, with its
    // lever arms frozen until a body has turned about 11°: crates riding a turning deck crept
    // outward at a tenth of its speed and slid off the edge.
    let mut app = app();
    app.world.spawn((
        Transform::from_xyz(0.0, -0.15, 0.0),
        RigidBody::kinematic(),
        Collider {
            shape: ColliderShape::Cylinder {
                radius: 4.0,
                half_height: 0.15,
            },
            ..Default::default()
        },
        velocity(Vec3::ZERO, Vec3::new(0.0, 0.4, 0.0)),
    ));
    let crates: Vec<(Entity, f64)> = [1.5, 3.0]
        .into_iter()
        .map(|r| {
            let e = app
                .world
                .spawn((
                    Transform::from_xyz(r, 0.35, 0.0),
                    RigidBody::dynamic(),
                    Collider::cuboid(Vec3::splat(0.35)),
                    Velocity::default(),
                ))
                .id();
            (e, r)
        })
        .collect();
    // Over a turn and a quarter.
    app.update_n(20 * 60);
    for (e, r) in crates {
        let p = pos(&app, e);
        let now = p.with_y(0.0).length();
        assert!(
            (now - r).abs() < 0.05 && p.y > 0.3,
            "a crate placed {r} m from the axis is {now} m out: {p:?}"
        );
    }
}

#[test]
fn characters_walk_inside_a_thrusting_ship() {
    let mut app = app();
    app.world.resource_mut::<Gravity>().uniform = Vec3::ZERO;
    // A 100 t ship (a hollow box would be nicer; a floor and a back wall do) at 7.8 km/s,
    // accelerating at 10 m/s² and turning slowly.
    let ship = app
        .world
        .spawn((
            Transform::from_xyz(0.0, -0.5, 0.0),
            RigidBody {
                mass_kg: Some(100_000.0),
                linear_damping: 0.0,
                angular_damping: 0.0,
                ..RigidBody::dynamic()
            },
            Collider::cuboid(Vec3::new(15.0, 0.5, 5.0)),
            velocity(Vec3::new(7_800.0, 0.0, 0.0), Vec3::new(0.0, 0.05, 0.0)),
            ExternalForce {
                force: Vec3::new(1.0e6, 0.0, 0.0),
                torque: Vec3::ZERO,
                at: None,
            },
        ))
        .id();
    // The game's artificial gravity pulls the character onto the floor; it walks 1 m/s aft,
    // steering by the ship's heading.
    let walk = Vec3::new(-1.0, -2.0, 0.0);
    let ch = character(&mut app, DVec3::new(5.0, 0.92, 0.0), walk);
    let start_local = pos(&app, ch) - pos(&app, ship);
    for _ in 0..240 {
        let heading = app.world.get::<Transform>(ship).unwrap().rotation;
        app.world.get_mut::<CharacterMove>(ch).unwrap().velocity = heading * walk;
        app.update();
    }
    let ship_tf = *app.world.get::<Transform>(ship).unwrap();
    let local = ship_tf.rotation.inverse().as_dquat() * (pos(&app, ch) - ship_tf.translation);
    let st = state(&app, ch);
    assert!(st.grounded, "fell off the ship: {st:?}");
    // Standing and walking must not shove the ship (its 7.8 km/s is not the character's).
    let dip = ship_tf.translation.y + 0.5;
    assert!(
        dip.abs() < 0.01,
        "the character pushed the ship {dip} m off course"
    );
    assert_eq!(st.ground_entity, Some(ship));
    // 4 s at 1 m/s aft, on the ship's own floor.
    let walked = start_local.x - local.x;
    assert!(
        (walked - 4.0).abs() < 0.1 && local.z.abs() < 0.1,
        "walked {walked} m aft, at {local:?} on the ship"
    );
}

#[test]
fn characters_weigh_on_what_they_stand_on() {
    let mut app = app();
    // A 4 m, 800 kg seesaw plank on a hinge at its middle; a character steps on one end.
    let plank = app
        .world
        .spawn((
            Transform::from_xyz(0.0, 1.0, 0.0),
            RigidBody::dynamic(),
            Collider::cuboid(Vec3::new(2.0, 0.1, 0.5)),
            Velocity::default(),
        ))
        .id();
    app.world.spawn(Joint {
        kind: JointKind::Revolute {
            axis: Vec3::Z,
            limits: None,
            motor: None,
        },
        body_a: plank,
        body_b: None,
        anchor_a: Vec3::ZERO,
        anchor_b: DVec3::new(0.0, 1.0, 0.0),
        break_force: None,
        collide_connected: false,
    });
    let ch = character(&mut app, DVec3::new(1.5, 1.1 + 0.92, 0.0), -Vec3::Y);
    app.update_n(40);
    let (axis, angle) = app
        .world
        .get::<Transform>(plank)
        .unwrap()
        .rotation
        .to_axis_angle();
    let tilt = axis.z * angle;
    assert!(
        tilt < -0.1,
        "the character's end should go down: tilt {tilt}"
    );
    assert!(
        state(&app, ch).grounded,
        "the character rides the plank down"
    );
}

#[test]
fn falling_crates_land_on_characters() {
    let mut app = app();
    app.world.spawn((
        Transform::from_xyz(0.0, -0.5, 0.0),
        RigidBody::fixed(),
        Collider::cuboid(Vec3::new(10.0, 0.5, 10.0)),
    ));
    let ch = character(&mut app, DVec3::new(0.0, 0.92, 0.0), -Vec3::Y);
    let crate_ = app
        .world
        .spawn((
            Transform::from_xyz(0.0, 4.0, 0.0),
            RigidBody::dynamic(),
            Collider::cuboid(Vec3::splat(0.3)),
            Velocity::default(),
        ))
        .id();
    app.update_n(60);
    let (c, k) = (pos(&app, crate_), pos(&app, ch));
    assert!(
        c.y > k.y + 0.8,
        "the crate should be on the character's head, not inside it: crate {c:?}, character {k:?}"
    );
    assert!(
        (k.y - 0.92).abs() < 0.03,
        "the crate pushed the character: {k:?}"
    );
}

#[test]
fn rays_find_bodies_moving_at_orbital_speed() {
    for speed in [300.0f32, 7_800.0, 30_000.0] {
        let mut app = app();
        app.world.resource_mut::<Gravity>().uniform = Vec3::ZERO;
        let ship = app
            .world
            .spawn((
                Transform::default(),
                RigidBody::dynamic(),
                Collider::cuboid(Vec3::new(10.0, 0.5, 10.0)),
                velocity(Vec3::new(speed, 0.0, 0.0), Vec3::ZERO),
            ))
            .id();
        let q = app.world.resource::<PhysicsQueries>().clone();
        for step in 0..120 {
            app.update();
            let p = pos(&app, ship);
            let hit = q.raycast(
                p + DVec3::new(0.0, 20.0, 0.0),
                -Vec3::Y,
                40.0,
                &QueryFilter::default(),
            );
            assert_eq!(
                hit.map(|h| h.entity),
                Some(ship),
                "{speed} m/s, step {step}: the ray missed"
            );
        }
    }
}

/// A kinematic deck with no `Velocity`, moved only by writing its `Transform` (a train car, a
/// lift driven by gameplay): it moves in the physics and carries what stands on it.
#[test]
fn a_deck_moved_by_its_transform_carries_crates_and_characters() {
    let mut app = app();
    let deck = app
        .world
        .spawn((
            Transform::from_xyz(0.0, -0.5, 0.0),
            RigidBody::kinematic(),
            Collider::cuboid(Vec3::new(3.0, 0.5, 3.0)),
        ))
        .id();
    let crate_box = app
        .world
        .spawn((
            Transform::from_xyz(1.5, 0.5, 0.0),
            RigidBody::dynamic(),
            Collider::cuboid(Vec3::splat(0.5)),
            Velocity::default(),
        ))
        .id();
    let ch = character(&mut app, DVec3::new(-1.5, 0.92, 0.0), -Vec3::Y);
    app.update_n(30);
    let (crate0, ch0) = (pos(&app, crate_box), pos(&app, ch));
    // Up to 3.8 m/s in a second (gently enough for the crate not to slip), then a second at
    // that speed.
    for frame in 1..=120 {
        let speed = 3.8 * (frame as f64 / 60.0).min(1.0);
        app.world.get_mut::<Transform>(deck).unwrap().translation.x += speed / 60.0;
        app.update();
    }
    let moved = pos(&app, deck).x;
    assert!(moved > 5.7, "the deck's Transform is at {moved}");
    let carried = pos(&app, crate_box).x - crate0.x;
    assert!(
        (carried - moved).abs() < 0.1,
        "the crate rode {carried:.3} m of the deck's {moved} m"
    );
    let walked = pos(&app, ch).x - ch0.x;
    assert!(
        (walked - moved).abs() < 0.1,
        "the character rode {walked:.3} m of the deck's {moved} m"
    );
    assert!(state(&app, ch).grounded, "the character fell off");
    let q = app.world.resource::<PhysicsQueries>().clone();
    let down = |x: f64| {
        q.raycast(
            DVec3::new(x, 5.0, 2.5),
            -Vec3::Y,
            10.0,
            &QueryFilter::default(),
        )
    };
    assert_eq!(
        down(moved).map(|h| h.entity),
        Some(deck),
        "rays miss the deck where it is"
    );
    assert_eq!(
        down(0.0).map(|h| h.entity),
        None,
        "rays hit the deck where it was"
    );
}

/// A kinematic door hung under a parent entity, swung by writing its `Transform`.
#[test]
fn a_door_swung_by_its_transform_turns_in_the_physics() {
    let mut app = app();
    let frame = app.world.spawn(Transform::from_xyz(4.0, 0.0, 0.0)).id();
    let door = app
        .world
        .spawn((
            Transform::from_xyz(0.0, 1.0, 0.0),
            RigidBody::kinematic(),
            Collider::cuboid(Vec3::new(1.0, 1.0, 0.05)),
            ChildOf(frame),
        ))
        .id();
    app.update_n(10);
    let q = app.world.resource::<PhysicsQueries>().clone();
    let ray = || {
        q.raycast(
            DVec3::new(4.0, 1.0, 5.0),
            -Vec3::Z,
            10.0,
            &QueryFilter::default(),
        )
    };
    let shut = ray().expect("the shut door stops the ray");
    assert!((shut.distance - 4.95).abs() < 0.01, "{shut:?}");
    app.world.get_mut::<Transform>(door).unwrap().rotation =
        engine_core::math::Quat::from_rotation_y(std::f32::consts::FRAC_PI_2);
    app.update();
    let open = ray().expect("the open door, edge on, still stops the ray");
    assert!((open.distance - 4.0).abs() < 0.01, "{open:?}");
}

/// Writing the `Transform` of a kinematic body that moves by `Velocity` puts it there, and it
/// goes on moving from there.
#[test]
fn a_kinematic_body_moving_by_velocity_jumps_where_its_transform_is_put() {
    let mut app = app();
    let cart = app
        .world
        .spawn((
            Transform::default(),
            RigidBody::kinematic(),
            Collider::cuboid(Vec3::splat(0.5)),
            velocity(Vec3::new(2.0, 0.0, 0.0), Vec3::ZERO),
        ))
        .id();
    app.update_n(30);
    assert!((pos(&app, cart).x - 1.0).abs() < 1e-3);
    app.world.get_mut::<Transform>(cart).unwrap().translation.x = 10.0;
    app.update_n(30);
    let x = pos(&app, cart).x;
    assert!((x - 11.0).abs() < 1e-3, "the cart is at {x}");
}
