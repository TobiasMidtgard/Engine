//! Behaviors other modules rely on beyond the conformance suite: removal, joints breaking,
//! kinematic platforms, characters on steps and slopes, sensors and layers, local time,
//! forces, water, blasts, and bodies travelling between regions.

use std::sync::Arc;

use engine_core::math::{DVec3, Quat, Vec3};
use engine_core::prelude::*;
use engine_core::time::TimeSource;
use engine_physics::{PhysicsPlugin, PhysicsSettings, PhysicsStats};
use engine_world::physics::{
    BodyKind, CharacterController, CharacterMove, CharacterState, Collider, ColliderShape,
    CollisionLayers, ContactEvent, ContactPhase, ExternalForce, Joint, JointKind, PhysicsQueries,
    QueryFilter, RigidBody, Velocity,
};
use engine_world::{
    Gravity, GroundContact, NoiseEvent, Water, WaterQuery, WaterSample, WorldImpulse, WorldPlugin,
};

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

/// A region following a fast body across a field of posts keeps copies of the posts near it
/// only: it drops the ones it leaves behind instead of collecting every static it passes.
#[test]
fn a_travelling_region_drops_the_statics_it_leaves_behind() {
    let mut app = app();
    // A post every 250 m along 30 km, 20 m to the side of the shot's path.
    let posts = 121;
    for i in 0..posts {
        app.world.spawn((
            Transform::from_xyz(i as f64 * 250.0, 0.0, 20.0),
            Collider::cuboid(Vec3::new(0.2, 2.0, 0.2)),
        ));
    }
    app.world.spawn((
        Transform::from_xyz(0.0, 1.0, 0.0),
        RigidBody {
            gravity_scale: 0.0,
            ..RigidBody::dynamic()
        },
        Collider::sphere(0.5),
        Velocity {
            linear: Vec3::new(3000.0, 0.0, 0.0),
            angular: Vec3::ZERO,
        },
    ));
    app.update_n(1);
    let start = stats(&app).static_copies;
    let mut most = start;
    for _ in 0..600 {
        app.update();
        most = most.max(stats(&app).static_copies);
    }
    println!("static copies: {start} at the start, at most {most} on the way");
    // The posts' own regions hold their copies from the start; the shot's region adds the
    // ones within its reach (about 1.3 km each way: a dozen posts), not all of them.
    assert!(
        most < start + 30,
        "{most} static copies, {start} at the start: the travelling region kept the posts it passed"
    );
    let diagnostics = app
        .world
        .resource::<engine_core::diagnostics::Diagnostics>();
    assert_eq!(
        diagnostics.get("physics.static_copies"),
        Some(stats(&app).static_copies as f64)
    );
    assert_eq!(diagnostics.get("physics.substeps"), Some(2.0));
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

/// A region emptied of every moving body, then filled again, keeps simulating: the step that
/// only applies removals must keep Rapier's islands in step with its contact graph (it used to
/// leave them a link behind, and a later contact panicked in debug builds and was mislinked in
/// release ones; a ragdoll going limp a second time hit it).
#[test]
fn a_region_emptied_and_filled_again_keeps_simulating() {
    let mut app = app();
    ground(&mut app, DVec3::ZERO, 20.0);
    let first: Vec<Entity> = (0..3)
        .map(|i| cube(&mut app, DVec3::new(2.0 * i as f64, 0.25, 0.0), 0.25))
        .collect();
    app.update_n(30);
    for e in first {
        app.world.despawn(e);
    }
    app.update_n(5);
    let a = cube(&mut app, DVec3::new(0.0, 0.25, 0.0), 0.25);
    let b = cube(&mut app, DVec3::new(2.0, 0.25, 0.0), 0.25);
    app.update_n(30);
    app.world.get_mut::<Transform>(a).unwrap().translation.y = 6.0;
    app.update_n(120);
    for e in [a, b] {
        assert!(
            (pos(&app, e).y - 0.25).abs() < 0.01,
            "{e:?} rests at {:?}",
            pos(&app, e)
        );
    }
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
fn characters_keep_sliding_along_moving_walls_they_press_into() {
    // Movement's case: running along a wall while pressing into it a little. Against a
    // kinematic wall the controller drops the into-the-wall part of the move, and the slide
    // left over used to count as a hit at time 0 every other step (boxes) or nearly every step
    // (hulls), so the character stopped dead along the wall.
    let half = Vec3::new(0.5, 12.0, 6.0);
    let corners: Vec<Vec3> = (0..8)
        .map(|i| {
            Vec3::new(
                if i & 1 == 0 { -half.x } else { half.x },
                if i & 2 == 0 { -half.y } else { half.y },
                if i & 4 == 0 { -half.z } else { half.z },
            )
        })
        .collect();
    for (name, shape) in [
        ("box", ColliderShape::Cuboid { half_extents: half }),
        ("hull", ColliderShape::ConvexHull { points: corners }),
    ] {
        for wall_speed in [0.0, 3.0] {
            for climb in [0.0, 6.5] {
                let mut app = app();
                // The wall's face is at x = 0, facing -x; it slides along z.
                app.world.spawn((
                    Transform::from_xyz(0.5, 0.0, 0.0),
                    RigidBody::kinematic(),
                    Collider {
                        shape: shape.clone(),
                        ..Default::default()
                    },
                    Velocity {
                        linear: Vec3::new(0.0, 0.0, wall_speed),
                        angular: Vec3::ZERO,
                    },
                ));
                let desc = CharacterController::default();
                let gap = 0.02;
                let c = app
                    .world
                    .spawn((
                        Transform::from_xyz(-(desc.radius as f64) - gap, 0.0, -4.0),
                        desc,
                        CharacterMove {
                            velocity: Vec3::new(0.5, climb, 3.0),
                        },
                    ))
                    .id();
                // Parry's hull casts are a little noisy in the gap: a step can lose a few
                // percent of its slide, never all of it.
                let slack = if name == "box" { 0.01 } else { 0.15 };
                let mut slid = 0.0;
                for step in 0..90 {
                    app.update();
                    let v = app.world.get::<CharacterState>(c).unwrap().velocity;
                    let what =
                        format!("{name}, wall at {wall_speed} m/s, climb {climb}, step {step}");
                    assert!((v.z - 3.0).abs() < slack, "{what}: velocity {v}");
                    slid += v.z / 90.0;
                    let x = pos(&app, c).x + desc.radius as f64;
                    assert!((x + gap).abs() < 0.003, "{what}: gap {:.4}", -x);
                }
                assert!(
                    slid > 2.99,
                    "{name}, wall at {wall_speed} m/s: slid at {slid:.3} m/s"
                );
            }
        }
    }
}

#[test]
fn running_on_flat_ground_finds_no_walls() {
    // Movement's setup: capsules spawned on a floor, running and falling. Wall runs, wall
    // jumps and mantles start from `wall_normal`, so a phantom wall in the middle of a field
    // throws the player sideways. The sweeps' own normals pointed sideways about one step in
    // a hundred, on small floors and huge ones, in real time and in slow motion.
    for (half, dilation) in [(50.0, None), (500.0, None), (500.0, Some(0.25))] {
        let mut app = app();
        ground(&mut app, DVec3::ZERO, half);
        if let Some(scale) = dilation {
            app.world
                .resource_mut::<TimeDilation>()
                .hold("focus", scale, 0.0);
        }
        let runners: Vec<Entity> = (0..3)
            .map(|i| {
                let mut e = app.world.spawn((
                    Transform::from_xyz(-20.0, 0.9, i as f64 * 4.0 - 4.0),
                    CharacterController::default(),
                    CharacterMove {
                        velocity: Vec3::new(4.5 + i as f32 * 2.0, -0.5, 0.3 * i as f32),
                    },
                    CharacterState::default(),
                ));
                // The focus player runs at full speed through the slowed world.
                if i == 2 {
                    e.insert(LocalTimeScale {
                        factor: 1.0,
                        ignore_world_dilation: true,
                    });
                }
                e.id()
            })
            .collect();
        for step in 0..240 {
            app.update();
            for &r in &runners {
                let state = *app.world.get::<CharacterState>(r).unwrap();
                assert!(
                    state.wall_normal.is_none(),
                    "floor {half}, dilation {dilation:?}, step {step}: phantom wall {:?} at {:?}",
                    state.wall_normal,
                    pos(&app, r)
                );
                assert!(
                    step < 2 || (state.grounded && state.ground_normal.y > 0.999),
                    "floor {half}, dilation {dilation:?}, step {step}: ground {:?}",
                    state.ground_normal
                );
            }
        }
    }
}

#[test]
fn characters_run_over_rolling_terrain() {
    // Gentle hills on a heightfield, like a planet's or the showcase's ground: runners keep
    // their footing, follow the surface, and never find a wall.
    let mut app = app();
    let height = |x: f32, z: f32| 1.5 * (x * 0.15).sin() * (z * 0.11).cos();
    let (rows, cols, size) = (81u32, 81u32, 80.0f32);
    let mut heights = Vec::new();
    for row in 0..rows {
        for col in 0..cols {
            let x = -size / 2.0 + size * col as f32 / (cols - 1) as f32;
            let z = -size / 2.0 + size * row as f32 / (rows - 1) as f32;
            heights.push(height(x, z));
        }
    }
    app.world.spawn((
        Transform::default(),
        RigidBody::fixed(),
        Collider {
            shape: ColliderShape::Heightfield {
                heights,
                rows,
                cols,
                size: [size, size],
            },
            ..Default::default()
        },
    ));
    let runners: Vec<Entity> = [-10.0f32, 0.0, 10.0]
        .into_iter()
        .map(|z| {
            app.world
                .spawn((
                    Transform::from_xyz(-30.0, (height(-30.0, z) + 0.95) as f64, z as f64),
                    CharacterController::default(),
                    CharacterMove {
                        velocity: Vec3::new(5.0, -3.0, 0.0),
                    },
                    CharacterState::default(),
                ))
                .id()
        })
        .collect();
    for step in 0..240 {
        app.update();
        if step < 5 {
            continue;
        }
        for &r in &runners {
            let p = pos(&app, r).as_vec3();
            let state = *app.world.get::<CharacterState>(r).unwrap();
            assert!(
                state.wall_normal.is_none(),
                "step {step}: phantom wall {:?} at {p:?}",
                state.wall_normal
            );
            assert!(state.grounded, "step {step}: lost the ground at {p:?}");
            // The surface's own normal, from the slope of the height function.
            let e = 0.01;
            let dx = (height(p.x + e, p.z) - height(p.x - e, p.z)) / (2.0 * e);
            let dz = (height(p.x, p.z + e) - height(p.x, p.z - e)) / (2.0 * e);
            let n = Vec3::new(-dx, 1.0, -dz).normalize();
            assert!(
                state.ground_normal.dot(n) > 0.98,
                "step {step}: ground normal {:?}, surface {n:?} at {p:?}",
                state.ground_normal
            );
            let gap = p.y - 0.9 - height(p.x, p.z);
            assert!(
                (-0.02..0.08).contains(&gap),
                "step {step}: feet {gap} m off the ground at {p:?}"
            );
        }
    }
    assert!(pos(&app, runners[0]).x > -15.0, "the runners got stuck");
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

/// A body on a slow clock lives in its own seconds for every push, not just gravity: a force
/// that holds it up in normal time holds it up at half speed too, its damping slows it per its
/// own second, and a lift driven by `Velocity` rises at half speed.
#[test]
fn forces_damping_and_kinematic_motion_follow_local_time() {
    let slow = LocalTimeScale {
        factor: 0.5,
        ignore_world_dilation: false,
    };
    // Hover: weight held up by an ExternalForce.
    let mut app = app();
    let c = cube(&mut app, DVec3::new(0.0, 10.0, 0.0), 0.5);
    app.world.entity_mut(c).insert(slow);
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
    let drift = pos(&app, c).y - y0;
    assert!(drift.abs() < 0.01, "slowed hovering body drifted {drift} m");

    // Damping: after 2 world seconds at half speed the slowed body has lived 1 s, so it has
    // the speed the normal one has after 1 s.
    let mut app = crate::app();
    let drag = RigidBody {
        linear_damping: 0.5,
        angular_damping: 0.5,
        gravity_scale: 0.0,
        ..RigidBody::dynamic()
    };
    let normal = cube(&mut app, DVec3::new(0.0, 10.0, 0.0), 0.5);
    let slowed = cube(&mut app, DVec3::new(0.0, 10.0, 10.0), 0.5);
    for e in [normal, slowed] {
        app.world.entity_mut(e).insert((
            drag.clone(),
            Velocity {
                linear: Vec3::new(10.0, 0.0, 0.0),
                angular: Vec3::new(0.0, 6.0, 0.0),
            },
        ));
    }
    app.world.entity_mut(slowed).insert(slow);
    app.update_n(60);
    let after_1s = *app.world.get::<Velocity>(normal).unwrap();
    app.update_n(60);
    let v = *app.world.get::<Velocity>(slowed).unwrap();
    assert!(
        (v.linear.x / after_1s.linear.x - 1.0).abs() < 0.02,
        "slowed body's speed {} after 2 world s, normal body's {} after 1 s",
        v.linear.x,
        after_1s.linear.x
    );
    assert!(
        (v.angular.y / after_1s.angular.y - 1.0).abs() < 0.02,
        "slowed body's spin {} after 2 world s, normal body's {} after 1 s",
        v.angular.y,
        after_1s.angular.y
    );

    // A lift moved by its Velocity: 1 m/s of its own time is 0.5 m per world second.
    let mut app = crate::app();
    let lift = app
        .world
        .spawn((
            Transform::from_xyz(0.0, 0.0, 0.0),
            RigidBody {
                kind: BodyKind::Kinematic,
                ..Default::default()
            },
            Collider::cuboid(Vec3::new(1.0, 0.1, 1.0)),
            Velocity {
                linear: Vec3::Y,
                angular: Vec3::ZERO,
            },
            slow,
        ))
        .id();
    app.update_n(60);
    let rise = pos(&app, lift).y;
    assert!(
        (rise - 0.5).abs() < 0.02,
        "slowed lift rose {rise} m in 1 world s, wanted 0.5"
    );
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

/// Water with a swell `amp` meters high running along +x, `len` meters from crest to crest;
/// `clock` is the time in seconds.
struct Swell {
    amp: f64,
    len: f64,
    clock: Arc<std::sync::Mutex<f64>>,
}

impl WaterQuery for Swell {
    fn sample(&self, p: DVec3) -> Option<WaterSample> {
        let k = std::f64::consts::TAU / self.len;
        let t = *self.clock.lock().unwrap();
        Some(WaterSample {
            surface_height: self.amp * (k * p.x - (9.81 * k).sqrt() * t).sin(),
            depth: 20.0,
            velocity: Vec3::ZERO,
            normal: Vec3::Y,
        })
    }
}

fn floater(app: &mut App, at: DVec3, rotation: Quat, collider: Collider) -> Entity {
    app.world
        .spawn((
            Transform::from_translation(at).with_rotation(rotation),
            RigidBody::dynamic(),
            collider,
            Velocity::default(),
        ))
        .id()
}

#[test]
fn floating_bodies_settle_the_way_real_ones_do() {
    let mut app = app();
    app.world.insert_resource(Water(Arc::new(Lake)));
    // A 2.4 m log dropped in standing up lies down.
    let log = floater(
        &mut app,
        DVec3::new(0.0, 0.5, 0.0),
        Quat::from_rotation_z(0.05),
        Collider::capsule(0.2, 1.0).with_density(600.0),
    );
    // A plank thrown in on its edge lands flat.
    let plank = floater(
        &mut app,
        DVec3::new(5.0, 0.5, 0.0),
        Quat::from_rotation_x(1.0),
        Collider::cuboid(Vec3::new(1.0, 0.05, 0.3)).with_density(500.0),
    );
    // A cube of half-density wood rolls onto an edge, 45° over (floating flat is unstable
    // for densities between 0.21 and 0.79 of the water's); at 0.9 it floats flat.
    let light_cube = floater(
        &mut app,
        DVec3::new(10.0, 0.3, 0.0),
        Quat::from_rotation_z(0.05),
        Collider::cuboid(Vec3::splat(0.5)).with_density(500.0),
    );
    let heavy_cube = floater(
        &mut app,
        DVec3::new(15.0, 0.3, 0.0),
        Quat::from_rotation_z(0.05),
        Collider::cuboid(Vec3::splat(0.5)).with_density(900.0),
    );
    // A quarter-density ball floats with its center 0.347 radii above the water.
    let ball = floater(
        &mut app,
        DVec3::new(20.0, 1.0, 0.0),
        Quat::IDENTITY,
        Collider::sphere(0.5).with_density(250.0),
    );
    app.update_n(600);
    let up = |app: &App, e: Entity| app.world.get::<Transform>(e).unwrap().rotation * Vec3::Y;
    let tilt = |app: &App, e: Entity| up(app, e).y.clamp(-1.0, 1.0).acos().to_degrees();
    assert!(
        up(&app, log).y.abs() < 0.1,
        "log not lying down: {}",
        up(&app, log)
    );
    assert!(
        tilt(&app, plank) < 5.0,
        "plank tilted {}°",
        tilt(&app, plank)
    );
    assert!(
        (tilt(&app, light_cube) - 45.0).abs() < 3.0,
        "half-density cube at {}°",
        tilt(&app, light_cube)
    );
    assert!(
        tilt(&app, heavy_cube) < 3.0,
        "0.9 cube at {}°",
        tilt(&app, heavy_cube)
    );
    assert!(
        (pos(&app, heavy_cube).y - (0.5 - 0.9)).abs() < 0.01,
        "0.9 cube's center at {}",
        pos(&app, heavy_cube).y
    );
    assert!(
        (pos(&app, ball).y - 0.347 * 0.5).abs() < 0.01,
        "ball's center at {}",
        pos(&app, ball).y
    );
    for e in [log, plank, light_cube, heavy_cube, ball] {
        let v = *app.world.get::<Velocity>(e).unwrap();
        assert!(
            v.linear.length() < 0.02 && v.angular.length() < 0.05,
            "still moving: {v:?}"
        );
    }
}

#[test]
fn barges_pitch_with_a_passing_swell() {
    // A 4 m barge on a 30 cm swell 8 m long (slopes up to 13°) rides it bow up, bow down.
    let pitch_range = |amp: f64| {
        let mut app = app();
        let clock = Arc::new(std::sync::Mutex::new(0.0));
        app.world.insert_resource(Water(Arc::new(Swell {
            amp,
            len: 8.0,
            clock: clock.clone(),
        })));
        let barge = floater(
            &mut app,
            DVec3::ZERO,
            Quat::IDENTITY,
            Collider::cuboid(Vec3::new(2.0, 0.4, 0.8)).with_density(400.0),
        );
        let (mut low, mut high) = (0.0f32, 0.0f32);
        for step in 0..600 {
            *clock.lock().unwrap() = step as f64 * DT;
            app.update();
            let bow = app.world.get::<Transform>(barge).unwrap().rotation * Vec3::X;
            if step > 240 {
                low = low.min(bow.y.asin().to_degrees());
                high = high.max(bow.y.asin().to_degrees());
            }
        }
        (low, high)
    };
    let (low, high) = pitch_range(0.3);
    assert!(
        low < -5.0 && high > 5.0,
        "pitched only {low:.1}°..{high:.1}°"
    );
    let (low, high) = pitch_range(0.0);
    assert!(
        low > -0.5 && high < 0.5,
        "pitched {low:.1}°..{high:.1}° in still water"
    );
}

/// Water whose level (m) and current (m/s) can change: a tide, a sluice, a flood.
struct Tide(Arc<std::sync::Mutex<(f64, Vec3)>>);

impl WaterQuery for Tide {
    fn sample(&self, _p: DVec3) -> Option<WaterSample> {
        let (level, current) = *self.0.lock().unwrap();
        Some(WaterSample {
            surface_height: level,
            depth: 20.0,
            velocity: current,
            normal: Vec3::Y,
        })
    }
}

fn asleep(app: &App, e: Entity) -> bool {
    *app.world.get::<Velocity>(e).unwrap() == Velocity::default()
}

#[test]
fn sleeping_bodies_wake_when_the_water_rises_falls_or_flows() {
    let mut app = app();
    let tide = Arc::new(std::sync::Mutex::new((0.0, Vec3::ZERO)));
    app.world
        .insert_resource(Water(Arc::new(Tide(tide.clone()))));
    // A wooden plank floating, and a light crate on a quay 1 m above the water.
    let plank = floater(
        &mut app,
        DVec3::new(0.0, 0.1, 0.0),
        Quat::IDENTITY,
        Collider::cuboid(Vec3::new(1.0, 0.1, 0.5)).with_density(500.0),
    );
    ground(&mut app, DVec3::new(10.0, 1.0, 0.0), 3.0);
    let crate_ = floater(
        &mut app,
        DVec3::new(10.0, 1.4, 0.0),
        Quat::IDENTITY,
        Collider::cuboid(Vec3::splat(0.4)).with_density(200.0),
    );
    // Left alone by the water: a stone on the bed 3 m down, and a crate on a high quay.
    ground(&mut app, DVec3::new(-10.0, -3.0, 0.0), 3.0);
    let stone = floater(
        &mut app,
        DVec3::new(-10.0, -2.7, 0.0),
        Quat::IDENTITY,
        Collider::cuboid(Vec3::splat(0.3)).with_density(2600.0),
    );
    ground(&mut app, DVec3::new(0.0, 3.0, 10.0), 3.0);
    let dry = floater(
        &mut app,
        DVec3::new(0.0, 3.4, 10.0),
        Quat::IDENTITY,
        Collider::cuboid(Vec3::splat(0.4)).with_density(200.0),
    );
    app.update_n(20 * 60);
    let all = [plank, crate_, stone, dry];
    assert!(all.iter().all(|&e| asleep(&app, e)), "nothing fell asleep");
    let rest = pos(&app, plank).y;
    // Steps the app, noting whether the stone or the dry crate ever woke.
    let stirred = std::cell::RefCell::new(Vec::new());
    let run = |app: &mut App, frames: usize| {
        for _ in 0..frames {
            app.update();
            for (name, e) in [("stone", stone), ("dry crate", dry)] {
                let mut s = stirred.borrow_mut();
                if !asleep(app, e) && !s.contains(&name) {
                    s.push(name);
                }
            }
        }
    };

    // The tide comes in: 1.5 m over 6 s. The plank rides up, the crate floats off the quay.
    for i in 1..=360 {
        tide.lock().unwrap().0 = 1.5 * i as f64 / 360.0;
        run(&mut app, 1);
    }
    run(&mut app, 180);
    let rose = pos(&app, plank).y - rest;
    assert!(
        (rose - 1.5).abs() < 0.05,
        "the plank rose {rose} m with a 1.5 m tide"
    );
    // 200 kg/m³ draws 0.16 m: the crate's center 0.24 m above the water.
    let c = pos(&app, crate_).y;
    assert!(
        (c - 1.74).abs() < 0.05,
        "the crate's center at {c} m on 1.5 m of water"
    );

    // Calm again: both fall asleep and stay asleep.
    run(&mut app, 20 * 60);
    assert!(
        asleep(&app, plank) && asleep(&app, crate_),
        "still awake in calm water"
    );

    // A sluice opens: the water drops 1 m at once and the plank drops with it.
    tide.lock().unwrap().0 = 0.5;
    run(&mut app, 5 * 60);
    let fell = rest + 1.5 - pos(&app, plank).y;
    assert!(
        (fell - 1.0).abs() < 0.05,
        "the plank fell {fell} m with the water"
    );

    // A current starts: 0.5 m/s along x carries the sleeping plank off, slowly at first (below
    // the speed at which bodies fall asleep), but the stone on the bed stays where it is.
    run(&mut app, 20 * 60);
    assert!(asleep(&app, plank), "plank still awake before the current");
    assert!(stirred.borrow().is_empty(), "the tide woke the {stirred:?}");
    let x = pos(&app, plank).x;
    let bed = pos(&app, stone);
    tide.lock().unwrap().1 = Vec3::new(0.5, 0.0, 0.0);
    run(&mut app, 5 * 60);
    let drift = pos(&app, plank).x - x;
    assert!(
        drift > 1.0,
        "the current carried the plank only {drift} m in 5 s"
    );
    // The stone may wake to feel the new current, but sleeps again where it lay.
    assert!(
        asleep(&app, stone),
        "the stone is still awake in the current"
    );
    let moved = pos(&app, stone).distance(bed);
    assert!(moved < 0.01, "the current moved the stone {moved} m");
    assert!(
        !stirred.borrow().contains(&"dry crate"),
        "the water woke the dry crate"
    );
}

#[derive(Resource, Default)]
struct Landings(Vec<GroundContact>, Vec<NoiseEvent>);

fn landings(
    mut seen: ResMut<Landings>,
    mut ground: MessageReader<GroundContact>,
    mut noise: MessageReader<NoiseEvent>,
) {
    seen.0.extend(ground.read().copied());
    seen.1.extend(noise.read().copied());
}

#[test]
fn tumbling_bodies_report_every_hard_landing() {
    let mut app = app();
    app.init_resource::<Landings>();
    app.add_systems(Last, landings);
    ground(&mut app, DVec3::ZERO, 40.0);
    // A crate thrown along +x, spinning forward: it lands on face after face.
    let tumbler = cube(&mut app, DVec3::new(0.0, 2.0, 0.0), 0.4);
    // A post standing on one edge, leaning 20° toward +x: it slaps down flat without ever
    // leaving the ground (its foot stays in contact, so only its impulse tells the landing).
    let lean = 20f32.to_radians();
    let post = app
        .world
        .spawn((
            Transform::from_xyz(0.0, (0.6 * lean.cos() + 0.15 * lean.sin()) as f64, 20.0)
                .with_rotation(Quat::from_rotation_z(-lean)),
            RigidBody::dynamic(),
            Collider::cuboid(Vec3::new(0.15, 0.6, 0.15)),
            Velocity::default(),
        ))
        .id();
    // One resting where it was put, one sliding flat: neither ever lands hard.
    let resting = cube(&mut app, DVec3::new(0.0, 0.4, 10.0), 0.4);
    let sliding = cube(&mut app, DVec3::new(0.0, 0.4, -10.0), 0.4);
    app.world.entity_mut(tumbler).insert(Velocity {
        linear: Vec3::new(6.0, 4.0, 0.0),
        angular: Vec3::new(0.0, 0.0, -20.0),
    });
    app.world.entity_mut(sliding).insert(Velocity {
        linear: Vec3::new(5.0, 0.0, 0.0),
        angular: Vec3::ZERO,
    });
    app.update_n(1);
    let mass = app
        .world
        .get::<engine_world::physics::MassProperties>(tumbler)
        .unwrap()
        .mass_kg;
    app.update_n(4 * 60);
    let seen = app.world.resource::<Landings>();
    let of = |e: Entity| -> Vec<GroundContact> {
        seen.0
            .iter()
            .filter(|c| c.entity == Some(e))
            .copied()
            .collect()
    };
    let hits = of(tumbler);
    assert!(
        hits.len() >= 3,
        "the tumbling crate landed {} times",
        hits.len()
    );
    for h in &hits {
        assert!(h.load_kg > mass, "a landing carried {} kg", h.load_kg);
        if h.speed > 0.5 {
            // Thrown along +x (its corners kick it a little sideways).
            let f = h.forward();
            assert!(
                f.x > 0.6,
                "a landing travelling along +x faces {f:?} (yaw {})",
                h.yaw
            );
        }
    }
    let slap = of(post);
    assert_eq!(
        slap.len(),
        1,
        "the toppled post landed {} times",
        slap.len()
    );
    let f = slap[0].forward();
    assert!(
        f.x.abs() > 0.9,
        "the post lies along x, its landing along {f:?}"
    );
    assert!(
        (slap[0].size[1] - 1.2).abs() < 0.1,
        "the post's landing is {:?} m",
        slap[0].size
    );
    let heard = seen.1.iter().filter(|n| n.source == Some(tumbler)).count();
    assert_eq!(heard, hits.len(), "every landing is heard");
    for (name, e) in [("resting", resting), ("sliding", sliding)] {
        let n = of(e).len();
        assert!(n <= 1, "the {name} crate reported {n} landings");
    }
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
fn joints_pinned_after_a_region_recenters_hang_in_place() {
    // A region re-centers on its moving bodies as they drift (cars driving off, crates thrown
    // far). Joints pinned to the world afterwards (a rebuilt rope bridge, a new pendulum) must
    // hang where they were placed; they used to be off by however far the region moved.
    let mut app = app();
    app.world.resource_mut::<PhysicsSettings>().region_radius_m = 40.0;
    ground(&mut app, DVec3::ZERO, 100.0);
    let old = cube(&mut app, DVec3::new(-3.0, 6.0, 3.0), 0.25);
    let pin = |app: &mut App, body: Entity, at: DVec3| {
        app.world.spawn(Joint {
            kind: JointKind::Spherical { cone_limit: None },
            body_a: body,
            body_b: None,
            anchor_a: Vec3::new(0.0, 2.0, 0.0),
            anchor_b: at,
            break_force: None,
            collide_connected: false,
        });
    };
    pin(&mut app, old, DVec3::new(-3.0, 8.0, 3.0));
    // A body gliding away drags the region's center 12 m along x.
    app.world.spawn((
        Transform::from_xyz(0.0, 3.0, 6.0),
        RigidBody {
            gravity_scale: 0.0,
            ..RigidBody::dynamic()
        },
        Collider::sphere(0.3),
        Velocity {
            linear: Vec3::new(12.0, 0.0, 0.0),
            angular: Vec3::ZERO,
        },
    ));
    app.update_n(150);
    let new = cube(&mut app, DVec3::new(0.0, 6.0, 3.0), 0.25);
    pin(&mut app, new, DVec3::new(0.0, 8.0, 3.0));
    app.update_n(120);
    for (e, under) in [
        (old, DVec3::new(-3.0, 6.0, 3.0)),
        (new, DVec3::new(0.0, 6.0, 3.0)),
    ] {
        let p = pos(&app, e);
        assert!(
            p.distance(under) < 0.05,
            "a pendulum hangs at {p:?}, not under its pin above {under:?}"
        );
    }
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

#[test]
fn falling_characters_are_grounded_only_once_they_land() {
    let mut app = app();
    ground(&mut app, DVec3::ZERO, 50.0);
    let ch = app
        .world
        .spawn((
            Transform::from_xyz(0.0, 3.0, 0.0),
            CharacterController::default(),
            CharacterMove {
                velocity: Vec3::new(0.0, -6.0, 0.0),
            },
        ))
        .id();
    let mut seen = Vec::new();
    for _ in 0..60 {
        app.update();
        let state = *app.world.get::<CharacterState>(ch).unwrap();
        seen.push((pos(&app, ch).y, state.grounded));
    }
    let (standing, landed) = *seen.last().unwrap();
    assert!(landed, "never landed");
    for (y, grounded) in seen {
        assert!(
            !grounded || y - standing < 0.03,
            "grounded {:.3} m above the floor, still falling",
            y - standing
        );
    }
}

#[test]
fn characters_stand_exactly_on_boxes_of_any_size() {
    // Parry's GJK sweeps let a character sink 10-20 cm into a 5-25 m box when it stood within
    // a few millimeters of the box's center axis, and meters into a 500 m box.
    for half in [5.0f32, 25.0, 500.0, 5000.0] {
        for off in [0.0, 0.002, 0.37] {
            let mut app = app();
            ground(&mut app, DVec3::ZERO, half);
            let ch = app
                .world
                .spawn((
                    Transform::from_xyz(off, 0.95, off * 0.7),
                    CharacterController::default(),
                    // Gravity as the movement module applies it, after a fall.
                    CharacterMove {
                        velocity: Vec3::new(0.0, -9.0, 0.0),
                    },
                ))
                .id();
            for step in 0..60 {
                app.update();
                // The capsule's feet sit 0.9 m below its center; the controller keeps 2 cm.
                let gap = pos(&app, ch).y - 0.9;
                assert!(
                    (gap - 0.02).abs() < 0.003,
                    "{half} m box, {off} m off center, step {step}: feet {gap:.3} m above it"
                );
                assert!(app.world.get::<CharacterState>(ch).unwrap().grounded);
            }
        }
    }
}
