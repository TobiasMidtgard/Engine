//! Behaviors other modules rely on beyond the conformance suite: removal, joints breaking,
//! kinematic platforms, characters on steps and slopes, sensors and layers, local time,
//! forces, water, blasts, and bodies travelling between regions.

use std::sync::Arc;

use engine_core::math::{DVec3, Quat, Vec3};
use engine_core::prelude::*;
use engine_core::time::TimeSource;
use engine_physics::{PhysicsPlugin, PhysicsStats};
use engine_world::physics::{
    CharacterController, CharacterMove, CharacterState, Collider, CollisionLayers, ContactEvent,
    ContactPhase, ExternalForce, Joint, JointKind, PhysicsQueries, QueryFilter, RigidBody,
    Velocity,
};
use engine_world::{Gravity, Water, WaterQuery, WaterSample, WorldImpulse, WorldPlugin};

const DT: f64 = 1.0 / 60.0;

fn app() -> App {
    let mut app = App::new();
    app.add_plugins(WorldPlugin);
    app.add_plugins(PhysicsPlugin);
    app.world
        .resource_mut::<Time>()
        .set_source(TimeSource::Manual { step_secs: DT });
    app.world.resource_mut::<FixedTime>().set_hz(60.0);
    app.init_resource::<Seen>();
    app.add_systems(Last, collect);
    app
}

#[derive(Resource, Default)]
struct Seen(Vec<ContactEvent>);

fn collect(mut seen: ResMut<Seen>, mut events: MessageReader<ContactEvent>) {
    seen.0.extend(events.read().copied());
}

fn ground(app: &mut App, top_center: DVec3, half: f32) -> Entity {
    app.world
        .spawn((
            Transform::from_translation(top_center - DVec3::Y * 0.5),
            RigidBody::fixed(),
            Collider::cuboid(Vec3::new(half, 0.5, half)),
        ))
        .id()
}

fn cube(app: &mut App, at: DVec3, half: f32) -> Entity {
    app.world
        .spawn((
            Transform::from_translation(at),
            RigidBody::dynamic(),
            Collider::cuboid(Vec3::splat(half)),
            Velocity::default(),
        ))
        .id()
}

fn pos(app: &App, e: Entity) -> DVec3 {
    app.world.get::<Transform>(e).unwrap().translation
}

fn vel(app: &App, e: Entity) -> Vec3 {
    app.world.get::<Velocity>(e).unwrap().linear
}

fn stats(app: &App) -> PhysicsStats {
    *app.world.resource::<PhysicsStats>()
}

fn zero_g(app: &mut App) {
    app.world.resource_mut::<Gravity>().uniform = Vec3::ZERO;
}

// ---------------------------------------------------------------- lifecycle

#[test]
fn despawned_bodies_leave_the_simulation() {
    let mut app = app();
    ground(&mut app, DVec3::ZERO, 10.0);
    let a = cube(&mut app, DVec3::new(0.0, 0.5, 0.0), 0.5);
    let b = cube(&mut app, DVec3::new(3.0, 0.5, 0.0), 0.5);
    app.update_n(5);
    assert_eq!(stats(&app).bodies, 2);
    app.world.despawn(a);
    // Removing only the RigidBody leaves a static collider behind.
    app.world.entity_mut(b).remove::<RigidBody>();
    app.update_n(2);
    assert_eq!(stats(&app).bodies, 0);
    let q = app.world.resource::<PhysicsQueries>().clone();
    let down = |x: f64| {
        q.raycast(
            DVec3::new(x, 5.0, 0.0),
            -Vec3::Y,
            10.0,
            &QueryFilter::default(),
        )
    };
    let hit_a = down(0.0).unwrap();
    assert!(
        (hit_a.point.y - 0.0).abs() < 1e-3,
        "despawned cube still hit: {hit_a:?}"
    );
    let hit_b = down(3.0).unwrap();
    assert_eq!(
        hit_b.entity, b,
        "the cube without RigidBody should stay as a static collider"
    );
    assert!((hit_b.point.y - 1.0).abs() < 0.02);
}

#[test]
fn collider_changes_rebuild_the_shape() {
    let mut app = app();
    ground(&mut app, DVec3::ZERO, 10.0);
    let c = cube(&mut app, DVec3::new(0.0, 0.5, 0.0), 0.5);
    app.update_n(30);
    app.world.get_mut::<Collider>(c).unwrap().shape =
        engine_world::physics::ColliderShape::Cuboid {
            half_extents: Vec3::splat(1.0),
        };
    app.update_n(60);
    let y = pos(&app, c).y;
    assert!(
        (y - 1.0).abs() < 0.03,
        "bigger cube should rest at 1.0, got {y}"
    );
}

#[test]
fn child_colliders_make_one_compound_body() {
    let mut app = app();
    ground(&mut app, DVec3::ZERO, 10.0);
    // A dumbbell: body with two child spheres 1 m to each side.
    let body = app
        .world
        .spawn((
            Transform::from_xyz(0.0, 3.0, 0.0),
            RigidBody::dynamic(),
            Velocity::default(),
        ))
        .id();
    for x in [-1.0, 1.0] {
        app.world.spawn((
            Transform::from_xyz(x, 0.0, 0.0),
            Collider::sphere(0.4),
            ChildOf(body),
        ));
    }
    app.update_n(180);
    let p = pos(&app, body);
    assert!(
        (p.y - 0.4).abs() < 0.03,
        "dumbbell should rest on its spheres: {p:?}"
    );
    let mass = app
        .world
        .get::<engine_world::physics::MassProperties>(body)
        .unwrap()
        .mass_kg;
    let expected = 2.0 * 1000.0 * 4.0 / 3.0 * std::f32::consts::PI * 0.4f32.powi(3);
    assert!(
        (mass - expected).abs() / expected < 0.01,
        "mass {mass} vs {expected}"
    );
}

// ---------------------------------------------------------------- joints

#[test]
fn joints_break_above_their_break_force() {
    let mut app = app();
    zero_g(&mut app);
    let a = cube(&mut app, DVec3::new(0.0, 5.0, 0.0), 0.5);
    let joint = app
        .world
        .spawn(Joint {
            kind: JointKind::Fixed,
            body_a: a,
            body_b: None,
            anchor_a: Vec3::ZERO,
            anchor_b: DVec3::new(0.0, 5.0, 0.0),
            break_force: Some(5000.0),
            collide_connected: false,
        })
        .id();
    // 1000 kg cube pulled with 2 kN: holds.
    app.world.entity_mut(a).insert(ExternalForce {
        force: Vec3::new(2000.0, 0.0, 0.0),
        ..Default::default()
    });
    app.update_n(30);
    assert!(
        app.world.get_entity(joint).is_ok(),
        "joint broke under 2 kN"
    );
    assert!(pos(&app, a).distance(DVec3::new(0.0, 5.0, 0.0)) < 0.01);
    // 20 kN: breaks, the joint entity is despawned and the cube flies off.
    app.world.get_mut::<ExternalForce>(a).unwrap().force = Vec3::new(20_000.0, 0.0, 0.0);
    app.update_n(30);
    assert!(app.world.get_entity(joint).is_err(), "joint held 20 kN");
    assert!(pos(&app, a).x > 1.0);
}

#[test]
fn revolute_motor_spins_a_wheel() {
    let mut app = app();
    zero_g(&mut app);
    let wheel = app
        .world
        .spawn((
            Transform::from_xyz(0.0, 2.0, 0.0),
            RigidBody::dynamic(),
            Collider::sphere(0.5),
            Velocity::default(),
        ))
        .id();
    app.world.spawn(Joint {
        kind: JointKind::Revolute {
            axis: Vec3::Z,
            limits: None,
            motor: Some(engine_world::physics::JointMotor {
                target_velocity: 5.0,
                target_position: None,
                stiffness: 0.0,
                damping: 1000.0,
                max_force: 10_000.0,
            }),
        },
        body_a: wheel,
        body_b: None,
        anchor_a: Vec3::ZERO,
        anchor_b: DVec3::new(0.0, 2.0, 0.0),
        break_force: None,
        collide_connected: false,
    });
    app.update_n(120);
    let w = app.world.get::<Velocity>(wheel).unwrap().angular;
    assert!(
        (w.z.abs() - 5.0).abs() < 0.3,
        "wheel spins at {w:?}, wanted 5 rad/s about z"
    );
    assert!(pos(&app, wheel).distance(DVec3::new(0.0, 2.0, 0.0)) < 0.01);
}

/// A leg hinged to a fixed post it overlaps (`collide_connected: false`) swings through the
/// post like a lid through its box rim, while other bodies still land on the post.
#[test]
fn bodies_hinged_to_fixed_bodies_pass_through_them() {
    let mut app = app();
    let post = app
        .world
        .spawn((
            Transform::from_xyz(0.0, 1.0, 0.0),
            RigidBody::fixed(),
            Collider::cuboid(Vec3::new(0.1, 0.25, 0.1)),
        ))
        .id();
    // Spans y 0.39..0.81: 6 cm inside the post's bottom (0.75).
    let leg = app
        .world
        .spawn((
            Transform::from_xyz(0.0, 0.6, 0.0),
            RigidBody::dynamic(),
            Collider::capsule(0.06, 0.15),
            Velocity {
                linear: Vec3::ZERO,
                angular: Vec3::new(4.0, 0.0, 0.0),
            },
        ))
        .id();
    app.world.spawn(Joint {
        kind: JointKind::Revolute {
            axis: Vec3::X,
            limits: None,
            motor: None,
        },
        body_a: post,
        body_b: Some(leg),
        anchor_a: Vec3::new(0.0, -0.2, 0.0),
        anchor_b: DVec3::new(0.0, 0.2, 0.0),
        break_force: None,
        collide_connected: false,
    });
    let ball = app
        .world
        .spawn((
            Transform::from_xyz(0.0, 1.6, 0.0),
            RigidBody::dynamic(),
            Collider::sphere(0.08),
            Velocity::default(),
        ))
        .id();
    app.update_n(20);
    let swing = app.world.get::<Transform>(leg).unwrap().rotation * Vec3::NEG_Y;
    assert!(
        swing.z < -0.05,
        "the leg should swing back through the post, its foot points {swing:?}"
    );
    app.update_n(60);
    let rest = pos(&app, ball).y;
    assert!(
        (rest - 1.33).abs() < 0.02,
        "the ball should rest on the post's top (1.33), it is at {rest}"
    );
}

// ---------------------------------------------------------------- kinematic & characters

#[test]
fn kinematic_platform_carries_boxes_and_characters() {
    let mut app = app();
    ground(&mut app, DVec3::ZERO, 50.0);
    let platform = app
        .world
        .spawn((
            Transform::from_xyz(0.0, 1.0, 0.0),
            RigidBody::kinematic(),
            Collider::cuboid(Vec3::new(3.0, 0.25, 3.0)),
            Velocity {
                linear: Vec3::new(2.0, 0.0, 0.0),
                angular: Vec3::ZERO,
            },
        ))
        .id();
    let crate_ = cube(&mut app, DVec3::new(-1.0, 1.75, 0.0), 0.5);
    let ch = app
        .world
        .spawn((
            Transform::from_xyz(1.0, 1.25 + 0.9 + 0.05, 0.0),
            CharacterController::default(),
            CharacterMove {
                velocity: Vec3::new(0.0, -1.0, 0.0),
            },
        ))
        .id();
    app.update_n(120);
    let px = pos(&app, platform).x;
    assert!((px - 4.0).abs() < 0.05, "platform moved to {px}, wanted 4");
    // It slips a little while friction brings it up to speed (about v²/2μg ≈ 0.3 m).
    let cx = pos(&app, crate_).x;
    assert!(cx > 2.5 && cx < 3.05, "crate rode to {cx}, wanted ≈ 2.7");
    let state = *app.world.get::<CharacterState>(ch).unwrap();
    assert!(state.grounded);
    assert_eq!(state.ground_entity, Some(platform));
    let chx = pos(&app, ch).x;
    assert!(
        (chx - 5.0).abs() < 0.25,
        "character rode to {chx}, wanted ≈ 5"
    );
}

#[test]
fn characters_climb_steps_and_stop_at_steep_slopes() {
    let mut app = app();
    ground(&mut app, DVec3::ZERO, 50.0);
    // A 25 cm step at x = 2.
    app.world.spawn((
        Transform::from_xyz(4.0, 0.125, 0.0),
        RigidBody::fixed(),
        Collider::cuboid(Vec3::new(2.0, 0.125, 2.0)),
    ));
    // A 70° wall-ish ramp at z = -5 (too steep), facing +z.
    app.world.spawn((
        Transform::from_xyz(0.0, 0.0, -6.0)
            .with_rotation(Quat::from_rotation_x(70f32.to_radians())),
        RigidBody::fixed(),
        Collider::cuboid(Vec3::new(3.0, 0.1, 3.0)),
    ));
    let walker = app
        .world
        .spawn((
            Transform::from_xyz(0.0, 0.95, 0.0),
            CharacterController::default(),
            CharacterMove {
                velocity: Vec3::new(2.0, -2.0, 0.0),
            },
        ))
        .id();
    let climber = app
        .world
        .spawn((
            Transform::from_xyz(-10.0, 0.95, 0.0),
            CharacterController::default(),
            CharacterMove {
                velocity: Vec3::new(0.0, -2.0, -2.0),
            },
        ))
        .id();
    // Move the climber next to the ramp.
    app.world.get_mut::<Transform>(climber).unwrap().translation = DVec3::new(0.0, 0.95, -3.0);
    app.update_n(120);
    let p = pos(&app, walker);
    assert!(p.x > 3.0, "walker stuck at the step: {p:?}");
    assert!(
        (p.y - (0.25 + 0.9)).abs() < 0.08,
        "walker isn't standing on the step: {p:?}"
    );
    let c = pos(&app, climber);
    assert!(c.y < 1.6, "character walked up a 70° slope: {c:?}");
    let state = *app.world.get::<CharacterState>(climber).unwrap();
    assert!(
        state.wall_normal.is_some(),
        "steep slope should count as a wall: {state:?}"
    );
}

#[test]
fn dashing_characters_stop_at_thin_walls() {
    let mut app = app();
    ground(&mut app, DVec3::ZERO, 50.0);
    // A 0.5 m wall at x = 6.
    app.world.spawn((
        Transform::from_xyz(6.0, 1.5, 0.0),
        RigidBody::fixed(),
        Collider::cuboid(Vec3::new(0.25, 1.5, 10.0)),
    ));
    // 80 m/s moves 1.3 m per step, 800 m/s 13 m.
    let dashers: Vec<Entity> = [80.0, 800.0]
        .into_iter()
        .enumerate()
        .map(|(i, speed)| {
            app.world
                .spawn((
                    Transform::from_xyz(0.0, 0.95, i as f64 * 3.0 - 1.5),
                    CharacterController::default(),
                    CharacterMove {
                        velocity: Vec3::new(speed, -1.0, 0.0),
                    },
                ))
                .id()
        })
        .collect();
    app.update_n(30);
    for e in dashers {
        let p = pos(&app, e);
        assert!(p.x < 6.0 - 0.25 - 0.3, "dashed through the wall: {p:?}");
        let state = *app.world.get::<CharacterState>(e).unwrap();
        let n = state.wall_normal.expect("pressed against the wall");
        assert!(n.x < -0.95, "wall normal {n:?}");
    }
}

#[test]
fn running_along_a_wall_reports_its_normal() {
    let mut app = app();
    ground(&mut app, DVec3::ZERO, 50.0);
    // Wall face at z = -1, facing +z.
    app.world.spawn((
        Transform::from_xyz(0.0, 1.5, -1.25),
        RigidBody::fixed(),
        Collider::cuboid(Vec3::new(20.0, 1.5, 0.25)),
    ));
    // Running parallel to the wall, 3 cm from it: the sweep never hits it.
    let runner = app
        .world
        .spawn((
            Transform::from_xyz(-5.0, 0.95, -1.0 + 0.35 + 0.03),
            CharacterController::default(),
            CharacterMove {
                velocity: Vec3::new(8.0, -1.0, 0.0),
            },
        ))
        .id();
    app.update_n(30);
    let p = pos(&app, runner);
    assert!(p.x > -1.5, "runner was slowed down by the wall: {p:?}");
    let state = *app.world.get::<CharacterState>(runner).unwrap();
    let n = state.wall_normal.expect("touching the wall");
    assert!(n.z > 0.95, "wall normal {n:?}");
    assert!(state.grounded);
}

#[test]
fn characters_can_crouch_under_low_ceilings() {
    let mut app = app();
    ground(&mut app, DVec3::ZERO, 50.0);
    // A beam with its underside 1.3 m up, from x = 3 to x = 5.
    app.world.spawn((
        Transform::from_xyz(4.0, 1.3 + 0.25, 0.0),
        RigidBody::fixed(),
        Collider::cuboid(Vec3::new(1.0, 0.25, 3.0)),
    ));
    let ch = app
        .world
        .spawn((
            Transform::from_xyz(0.0, 0.95, 0.0),
            CharacterController::default(),
            CharacterMove {
                velocity: Vec3::new(2.0, -1.0, 0.0),
            },
        ))
        .id();
    app.update_n(90);
    let p = pos(&app, ch);
    assert!(p.x < 3.0 - 0.3, "walked through the beam standing: {p:?}");
    // Crouch the way the movement module does: shorter capsule, feet stay on the ground.
    app.world.get_mut::<CharacterController>(ch).unwrap().height = 1.0;
    app.world.get_mut::<Transform>(ch).unwrap().translation.y -= 0.4;
    app.update_n(120);
    let p = pos(&app, ch);
    assert!(p.x > 6.0, "couldn't crouch under the beam: {p:?}");
    assert!(app.world.get::<CharacterState>(ch).unwrap().grounded);
}

#[test]
fn fast_characters_ride_platforms_at_the_platform_speed() {
    let mut app = app();
    ground(&mut app, DVec3::ZERO, 50.0);
    let platform = app
        .world
        .spawn((
            Transform::from_xyz(0.0, 1.0, 0.0),
            RigidBody::kinematic(),
            Collider::cuboid(Vec3::new(3.0, 0.25, 3.0)),
            Velocity {
                linear: Vec3::new(2.0, 0.0, 0.0),
                angular: Vec3::ZERO,
            },
        ))
        .id();
    // A character running at triple speed (its own clock) standing still on the platform.
    let ch = app
        .world
        .spawn((
            Transform::from_xyz(1.0, 1.25 + 0.9 + 0.05, 0.0),
            CharacterController::default(),
            CharacterMove {
                velocity: Vec3::new(0.0, -1.0, 0.0),
            },
            LocalTimeScale {
                factor: 3.0,
                ignore_world_dilation: false,
            },
        ))
        .id();
    app.update_n(120);
    let state = *app.world.get::<CharacterState>(ch).unwrap();
    assert_eq!(state.ground_entity, Some(platform));
    let chx = pos(&app, ch).x;
    assert!(
        (chx - 5.0).abs() < 0.25,
        "character rode to {chx}, wanted ≈ 5 like the platform"
    );
}

// ---------------------------------------------------------------- sensors and layers

#[test]
fn sensors_report_overlaps_without_pushing() {
    let mut app = app();
    ground(&mut app, DVec3::ZERO, 10.0);
    let zone = app
        .world
        .spawn((
            Transform::from_xyz(0.0, 2.0, 0.0),
            Collider {
                sensor: true,
                ..Collider::cuboid(Vec3::splat(1.0))
            },
        ))
        .id();
    let ball = cube(&mut app, DVec3::new(0.0, 6.0, 0.0), 0.25);
    app.update_n(120);
    // Fell through the trigger to the floor.
    assert!((pos(&app, ball).y - 0.25).abs() < 0.03);
    let seen = &app.world.resource::<Seen>().0;
    let with_zone: Vec<_> = seen
        .iter()
        .filter(|c| (c.a == zone || c.b == zone) && (c.a == ball || c.b == ball))
        .collect();
    assert_eq!(with_zone.len(), 2, "{with_zone:?}");
    assert!(with_zone.iter().all(|c| c.sensor));
    assert_eq!(with_zone[0].phase, ContactPhase::Started);
    assert_eq!(with_zone[1].phase, ContactPhase::Ended);
}

#[test]
fn layers_decide_what_collides_and_what_queries_see() {
    let mut app = app();
    ground(&mut app, DVec3::ZERO, 10.0);
    let ghost = app
        .world
        .spawn((
            Transform::from_xyz(0.0, 3.0, 0.0),
            RigidBody::dynamic(),
            Collider {
                layers: CollisionLayers {
                    member: 0b10,
                    collides_with: 0b10,
                },
                ..Collider::cuboid(Vec3::splat(0.5))
            },
            Velocity::default(),
        ))
        .id();
    app.update_n(60);
    assert!(
        pos(&app, ghost).y < -1.0,
        "layer-2 box should fall through the layer-1 floor"
    );
    let q = app.world.resource::<PhysicsQueries>().clone();
    let filter = QueryFilter {
        layers: 0b10,
        ..Default::default()
    };
    assert!(
        q.raycast(DVec3::new(0.0, 10.0, 0.0), -Vec3::Y, 9.0, &filter)
            .is_none(),
        "layer-2 ray should ignore the floor"
    );
}

// ---------------------------------------------------------------- time, forces, water, blasts

#[test]
fn local_time_scale_speeds_up_one_body() {
    let mut app = app();
    let normal = cube(&mut app, DVec3::new(0.0, 100.0, 0.0), 0.5);
    let fast = cube(&mut app, DVec3::new(5.0, 100.0, 0.0), 0.5);
    app.world.entity_mut(fast).insert(LocalTimeScale {
        factor: 2.0,
        ignore_world_dilation: false,
    });
    app.update_n(60);
    let (vn, vf) = (vel(&app, normal).y, vel(&app, fast).y);
    let (dn, df) = (100.0 - pos(&app, normal).y, 100.0 - pos(&app, fast).y);
    assert!((vn + 9.81).abs() < 0.3, "normal body: {vn}");
    // In its own time it fell for 2 s: twice the speed (Velocity is in the body's own time)
    // and four times the distance.
    assert!(
        (vf / vn - 2.0).abs() < 0.15,
        "fast body velocity ratio {}",
        vf / vn
    );
    assert!(
        (df / dn - 4.0).abs() < 0.3,
        "fast body distance ratio {}",
        df / dn
    );
}

#[test]
fn external_force_can_hover_a_body() {
    let mut app = app();
    let c = cube(&mut app, DVec3::new(0.0, 10.0, 0.0), 0.5);
    app.update_n(1);
    let m = app
        .world
        .get::<engine_world::physics::MassProperties>(c)
        .unwrap()
        .mass_kg;
    app.world.entity_mut(c).insert(ExternalForce {
        force: Vec3::new(0.0, m * 9.81, 0.0),
        ..Default::default()
    });
    *app.world.get_mut::<Velocity>(c).unwrap() = Velocity::default();
    let y0 = pos(&app, c).y;
    app.update_n(120);
    assert!((pos(&app, c).y - y0).abs() < 0.01, "hovering body drifted");
}

struct Lake;

impl WaterQuery for Lake {
    fn sample(&self, _p: DVec3) -> Option<WaterSample> {
        Some(WaterSample {
            surface_height: 0.0,
            depth: 20.0,
            velocity: Vec3::ZERO,
            normal: Vec3::Y,
        })
    }
}

#[test]
fn light_bodies_float_heavy_ones_sink() {
    let mut app = app();
    app.world.insert_resource(Water(Arc::new(Lake)));
    let wood = app
        .world
        .spawn((
            Transform::from_xyz(0.0, 2.0, 0.0),
            RigidBody::dynamic(),
            Collider::cuboid(Vec3::splat(0.5)).with_density(500.0),
            Velocity::default(),
        ))
        .id();
    let steel = app
        .world
        .spawn((
            Transform::from_xyz(3.0, 2.0, 0.0),
            RigidBody::dynamic(),
            Collider::cuboid(Vec3::splat(0.5)).with_density(7800.0),
            Velocity::default(),
        ))
        .id();
    app.update_n(600);
    let w = pos(&app, wood).y;
    assert!(
        (w - 0.0).abs() < 0.1,
        "half-density wood should float half-submerged: {w}"
    );
    assert!(pos(&app, steel).y < -10.0, "steel should sink");
}

#[test]
fn blasts_throw_nearby_bodies_away() {
    let mut app = app();
    ground(&mut app, DVec3::ZERO, 30.0);
    // Empty wooden crates, 100 kg each.
    let near = cube(&mut app, DVec3::new(2.0, 0.5, 0.0), 0.5);
    let far = cube(&mut app, DVec3::new(20.0, 0.5, 0.0), 0.5);
    for c in [near, far] {
        app.world.get_mut::<Collider>(c).unwrap().density = 100.0;
    }
    app.update_n(10);
    app.world.write_message(WorldImpulse {
        center: DVec3::new(0.0, 0.2, 0.0),
        radius: 8.0,
        strength: 150.0,
        energy_j: 0.0,
        heat_c: 0.0,
        source: None,
    });
    app.update_n(3);
    let v = vel(&app, near);
    assert!(v.x > 1.0 && v.y > 0.0, "near box wasn't thrown: {v:?}");
    assert!(vel(&app, far).length() < 0.01, "far box moved");
}

// ---------------------------------------------------------------- regions

#[test]
fn fast_bodies_travel_between_regions() {
    let mut app = app();
    ground(&mut app, DVec3::ZERO, 10.0);
    let resting = cube(&mut app, DVec3::new(0.0, 0.5, 0.0), 0.5);
    let shot = app
        .world
        .spawn((
            Transform::from_xyz(0.0, 5.0, 0.0),
            RigidBody {
                gravity_scale: 0.0,
                angular_damping: 0.0,
                ..RigidBody::dynamic()
            },
            Collider::sphere(0.2),
            Velocity {
                linear: Vec3::new(300.0, 0.0, 0.0),
                angular: Vec3::ZERO,
            },
        ))
        .id();
    app.update_n(1800);
    let x = pos(&app, shot).x;
    assert!(
        (x - 9000.0).abs() < 10.0,
        "shot travelled to {x}, wanted 9000"
    );
    assert_eq!(
        stats(&app).regions,
        2,
        "the shot should be in its own region"
    );
    assert!((pos(&app, resting).y - 0.5).abs() < 0.01);
    // A ray far away still finds it.
    let q = app.world.resource::<PhysicsQueries>().clone();
    let hit = q
        .raycast(
            DVec3::new(x, 50.0, 0.0),
            -Vec3::Y,
            100.0,
            &QueryFilter::default(),
        )
        .expect("ray should find the shot 9 km away");
    assert_eq!(hit.entity, shot);
}

#[test]
fn bodies_from_different_regions_collide() {
    let mut app = app();
    zero_g(&mut app);
    let ship = |app: &mut App, x: f64, v: f32| {
        app.world
            .spawn((
                Transform::from_xyz(x, 0.0, 0.0),
                RigidBody::dynamic(),
                Collider::cuboid(Vec3::splat(2.0)),
                Velocity {
                    linear: Vec3::new(v, 0.0, 0.0),
                    angular: Vec3::ZERO,
                },
            ))
            .id()
    };
    let a = ship(&mut app, -2500.0, 100.0);
    let b = ship(&mut app, 2500.0, -100.0);
    app.update_n(2);
    assert_eq!(stats(&app).regions, 2);
    app.update_n(60 * 30);
    // They met around 25 s and bounced or stopped; they never pass through each other.
    assert!(
        pos(&app, a).x < pos(&app, b).x,
        "ships passed through each other"
    );
    assert!(
        vel(&app, a).x < 50.0 && vel(&app, b).x > -50.0,
        "no collision happened"
    );
}
