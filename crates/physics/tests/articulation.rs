//! Articulated joints (`PhysicsSettings::articulated_joints`): joints between dynamic bodies
//! that cannot stretch, and still answer everything a constraint joint does: impulses, written
//! velocities and poses, limits, motors, world shifts, region changes and removal.

use engine_core::math::{DQuat, DVec3, Quat, Vec3};
use engine_core::prelude::*;
use engine_core::time::TimeSource;
use engine_physics::{PhysicsPlugin, PhysicsSettings, PhysicsStats};
use engine_world::physics::{
    ApplyImpulse, Collider, Joint, JointKind, JointMotor, MassProperties, RigidBody, Velocity,
};
use engine_world::{Gravity, SimulationFocus, WorldPlugin, WorldShift};

fn app(articulated: bool) -> App {
    let mut app = App::new();
    app.add_plugins(WorldPlugin);
    app.add_plugins(PhysicsPlugin);
    app.world
        .resource_mut::<Time>()
        .set_source(TimeSource::Manual {
            step_secs: 1.0 / 60.0,
        });
    app.world.resource_mut::<FixedTime>().set_hz(60.0);
    app.world
        .resource_mut::<PhysicsSettings>()
        .articulated_joints = articulated;
    app
}

fn zero_g(app: &mut App) {
    app.world.resource_mut::<Gravity>().uniform = Vec3::ZERO;
}

fn ground(app: &mut App) {
    app.world.spawn((
        Transform::from_xyz(0.0, -0.5, 0.0),
        RigidBody::fixed(),
        Collider::cuboid(Vec3::new(20.0, 0.5, 20.0)),
    ));
}

fn body(app: &mut App, at: DVec3, rotation: Quat, collider: Collider) -> Entity {
    app.world
        .spawn((
            Transform::from_translation(at).with_rotation(rotation),
            RigidBody::dynamic(),
            collider,
            Velocity::default(),
        ))
        .id()
}

fn joint(app: &mut App, kind: JointKind, a: Entity, b: Entity, at: DVec3) -> Entity {
    let (ta, tb) = (transform(app, a), transform(app, b));
    app.world
        .spawn(Joint {
            kind,
            body_a: a,
            body_b: Some(b),
            anchor_a: ta.rotation.inverse() * (at - ta.translation).as_vec3(),
            anchor_b: (tb.rotation.inverse() * (at - tb.translation).as_vec3()).as_dvec3(),
            break_force: None,
            collide_connected: false,
        })
        .id()
}

fn transform(app: &App, e: Entity) -> Transform {
    *app.world.get::<Transform>(e).unwrap()
}

fn pos(app: &App, e: Entity) -> DVec3 {
    transform(app, e).translation
}

fn vel(app: &App, e: Entity) -> Velocity {
    *app.world.get::<Velocity>(e).unwrap()
}

fn mass(app: &App, e: Entity) -> f32 {
    app.world.get::<MassProperties>(e).unwrap().mass_kg
}

/// Distance between a joint's two anchors, meters.
fn gap(app: &App, j: Entity) -> f64 {
    let joint = app.world.get::<Joint>(j).unwrap();
    let ta = transform(app, joint.body_a);
    let tb = transform(app, joint.body_b.unwrap());
    let a = ta.translation + (ta.rotation * joint.anchor_a).as_dvec3();
    let b = tb.translation + (tb.rotation * joint.anchor_b.as_vec3()).as_dvec3();
    a.distance(b)
}

fn worst_gap(app: &App, joints: &[Entity]) -> f64 {
    joints.iter().map(|j| gap(app, *j)).fold(0.0, f64::max)
}

fn momentum(app: &App, parts: &[Entity]) -> Vec3 {
    parts
        .iter()
        .map(|p| vel(app, *p).linear * mass(app, *p))
        .sum()
}

/// Hinge angle of `b` relative to `a` about the world `axis` (both start square).
fn angle(app: &App, a: Entity, b: Entity, axis: Vec3) -> f32 {
    let rel = transform(app, a).rotation.inverse() * transform(app, b).rotation;
    let (x, y, z, w) = (rel.x, rel.y, rel.z, rel.w);
    2.0 * Vec3::new(x, y, z).dot(axis).atan2(w)
}

/// `n` boxes in a row along +x from `start`, each ball-jointed to the next.
fn chain(app: &mut App, start: DVec3, n: usize) -> (Vec<Entity>, Vec<Entity>) {
    let link = 0.6;
    let parts: Vec<Entity> = (0..n)
        .map(|i| {
            body(
                app,
                start + DVec3::X * (link * i as f64),
                Quat::IDENTITY,
                Collider::cuboid(Vec3::new(0.25, 0.1, 0.1)),
            )
        })
        .collect();
    let joints = parts
        .windows(2)
        .map(|w| {
            let at = (pos(app, w[0]) + pos(app, w[1])) * 0.5;
            joint(
                app,
                JointKind::Spherical { cone_limit: None },
                w[0],
                w[1],
                at,
            )
        })
        .collect();
    (parts, joints)
}

#[test]
fn an_impulse_on_one_link_moves_the_whole_articulation_by_its_momentum() {
    for articulated in [true, false] {
        let mut app = app(articulated);
        zero_g(&mut app);
        let (parts, joints) = chain(&mut app, DVec3::ZERO, 5);
        app.update();
        app.world.write_message(ApplyImpulse {
            entity: parts[4],
            impulse: Vec3::new(0.0, 6.0, 0.0),
            at: None,
            angular: Vec3::ZERO,
        });
        app.update_n(10);
        let p = momentum(&app, &parts);
        assert!(
            (p - Vec3::new(0.0, 6.0, 0.0)).length() < 0.06,
            "articulated {articulated}: momentum {p:?} after a 6 N·s push"
        );
        // The pushed end swings up and drags its neighbour along.
        assert!(vel(&app, parts[3]).linear.y > 0.01);
        if articulated {
            assert!(worst_gap(&app, &joints) < 1e-4);
        }
    }
}

#[test]
fn velocities_written_to_links_come_through() {
    let mut app = app(true);
    zero_g(&mut app);
    let (parts, joints) = chain(&mut app, DVec3::ZERO, 4);
    app.update();
    // Every link at once: a motion the joints allow comes through exactly.
    for p in &parts {
        app.world.get_mut::<Velocity>(*p).unwrap().linear = Vec3::new(3.0, 0.0, -1.0);
    }
    app.update();
    for p in &parts {
        let v = vel(&app, *p).linear;
        assert!(
            (v - Vec3::new(3.0, 0.0, -1.0)).length() < 1e-3,
            "a link moves at {v:?}"
        );
    }
    // One link: shared with the rest like a push.
    let before = momentum(&app, &parts);
    app.world.get_mut::<Velocity>(parts[0]).unwrap().linear += Vec3::new(0.0, 4.0, 0.0);
    app.update();
    let gained = momentum(&app, &parts) - before;
    let expected = Vec3::new(0.0, 4.0 * mass(&app, parts[0]), 0.0);
    assert!(
        (gained - expected).length() < 0.02 * expected.length(),
        "momentum gained {gained:?}, pushed {expected:?}"
    );
    assert!(worst_gap(&app, &joints) < 1e-4);
}

#[test]
fn moving_any_one_link_moves_the_whole_articulation() {
    let mut app = app(true);
    zero_g(&mut app);
    let (parts, joints) = chain(&mut app, DVec3::ZERO, 4);
    app.update_n(2);
    for (moved, by) in [
        (0, DVec3::new(5.0, 0.0, 0.0)),
        (2, DVec3::new(0.0, 3.0, -2.0)),
    ] {
        let before: Vec<DVec3> = parts.iter().map(|p| pos(&app, *p)).collect();
        app.world
            .get_mut::<Transform>(parts[moved])
            .unwrap()
            .translation += by;
        app.update();
        for (p, b) in parts.iter().zip(&before) {
            let d = pos(&app, *p) - *b - by;
            assert!(
                d.length() < 1e-3,
                "moving link {moved} by {by:?} left another {d:?} behind"
            );
        }
        assert!(worst_gap(&app, &joints) < 1e-4);
    }
    // Links posed about their joint (an animation pose) bend it; the rest stays put.
    let bent = Quat::from_rotation_y(0.6);
    let pivot = pos(&app, parts[2]) - DVec3::X * 0.3;
    let still = [pos(&app, parts[0]), pos(&app, parts[1])];
    for p in &parts[2..] {
        let mut t = app.world.get_mut::<Transform>(*p).unwrap();
        t.translation = pivot + (bent * (t.translation - pivot).as_vec3()).as_dvec3();
        t.rotation = bent * t.rotation;
    }
    let wanted = pos(&app, parts[3]);
    app.update();
    assert!(pos(&app, parts[3]).distance(wanted) < 1e-3);
    assert!(pos(&app, parts[0]).distance(still[0]) < 1e-3);
    assert!(pos(&app, parts[1]).distance(still[1]) < 1e-3);
    assert!(worst_gap(&app, &joints) < 1e-4);
}

#[test]
fn cone_limits_hold_on_articulated_ball_joints() {
    for articulated in [true, false] {
        let mut app = app(articulated);
        ground(&mut app);
        // A heavy block with an arm sticking out sideways: gravity swings the arm down until
        // the 0.4 rad cone stops it.
        let block = body(
            &mut app,
            DVec3::new(0.0, 0.5, 0.0),
            Quat::IDENTITY,
            Collider::cuboid(Vec3::splat(0.5)),
        );
        let arm = body(
            &mut app,
            DVec3::new(1.1, 0.8, 0.0),
            Quat::IDENTITY,
            Collider::cuboid(Vec3::new(0.5, 0.08, 0.08)),
        );
        joint(
            &mut app,
            JointKind::Spherical {
                cone_limit: Some(0.4),
            },
            block,
            arm,
            DVec3::new(0.6, 0.8, 0.0),
        );
        let mut widest = 0.0f32;
        for _ in 0..120 {
            app.update();
            widest = widest.max(angle(&app, block, arm, Vec3::Z).abs());
        }
        let end = angle(&app, block, arm, Vec3::Z).abs();
        assert!(
            widest < 0.44 && end > 0.36,
            "articulated {articulated}: swung to {widest}, rests at {end} (cone 0.4)"
        );
    }
}

/// Angle between `a`'s and `b`'s `local` axis, radians.
fn axes_apart(app: &App, a: Entity, b: Entity, local: Vec3) -> f32 {
    let (ra, rb) = (transform(app, a).rotation, transform(app, b).rotation);
    (ra * local).angle_between(rb * local)
}

/// A heavy block with an arm hanging from its underside on a ball joint with a 0.4 rad cone.
fn hanging_arm(app: &mut App) -> (Entity, Entity) {
    let block = body(
        app,
        DVec3::new(0.0, 3.0, 0.0),
        Quat::IDENTITY,
        Collider::cuboid(Vec3::splat(0.5)),
    );
    let arm = body(
        app,
        DVec3::new(0.0, 1.9, 0.0),
        Quat::IDENTITY,
        Collider::cuboid(Vec3::new(0.08, 0.5, 0.08)),
    );
    joint(
        app,
        JointKind::Spherical {
            cone_limit: Some(0.4),
        },
        block,
        arm,
        DVec3::new(0.0, 2.4, 0.0),
    );
    (block, arm)
}

/// A cone limit is round, around the limb's rest direction, and leaves the limb free to twist
/// about itself: an arm kicked out diagonally stops at the same angle as one kicked straight
/// out (a square cone lets the diagonal go 40% farther), and an arm turned half a turn about
/// its own length stays turned, hanging straight.
#[test]
fn cone_limits_are_round_and_let_the_limb_twist() {
    for articulated in [true, false] {
        for kick in [Vec3::X, Vec3::new(1.0, 0.0, 1.0).normalize()] {
            let mut app = app(articulated);
            zero_g(&mut app);
            let (block, arm) = hanging_arm(&mut app);
            app.update();
            // Swing the arm about the joint, its tip out along `kick`.
            let turn = Vec3::NEG_Y.cross(kick) * 6.0;
            *app.world.get_mut::<Velocity>(arm).unwrap() = Velocity {
                linear: turn.cross(Vec3::NEG_Y * 0.5),
                angular: turn,
            };
            let mut widest = 0.0f32;
            for _ in 0..60 {
                app.update();
                widest = widest.max(axes_apart(&app, block, arm, Vec3::Y));
            }
            assert!(
                (0.36..0.44).contains(&widest),
                "articulated {articulated}, kicked along {kick}: swung {widest} rad (cone 0.4)"
            );
        }
        let mut app = app(articulated);
        zero_g(&mut app);
        let (block, arm) = hanging_arm(&mut app);
        app.update();
        app.world.get_mut::<Transform>(arm).unwrap().rotation = Quat::from_rotation_y(2.0);
        let mut widest = 0.0f32;
        for _ in 0..60 {
            app.update();
            widest = widest.max(axes_apart(&app, block, arm, Vec3::Y));
        }
        let twist = angle(&app, block, arm, Vec3::Y);
        assert!(
            (twist - 2.0).abs() < 0.05 && widest < 0.05,
            "articulated {articulated}: twisted back to {twist} rad (put at 2), swung {widest} rad"
        );
    }
}

/// A limb pressed against its cone's rim rubs there: an arm sticking out sideways, held at the
/// bottom of its cone by gravity and swung along the rim, stops within a few swings instead of
/// rocking along the rim for half a minute.
#[test]
fn a_limb_swinging_along_its_cone_rim_stops() {
    for articulated in [true, false] {
        let mut app = app(articulated);
        ground(&mut app);
        let block = body(
            &mut app,
            DVec3::new(0.0, 0.5, 0.0),
            Quat::IDENTITY,
            Collider::cuboid(Vec3::splat(0.5)),
        );
        let arm = body(
            &mut app,
            DVec3::new(1.1, 0.8, 0.0),
            Quat::IDENTITY,
            Collider::cuboid(Vec3::new(0.5, 0.08, 0.08)),
        );
        joint(
            &mut app,
            JointKind::Spherical {
                cone_limit: Some(0.4),
            },
            block,
            arm,
            DVec3::new(0.6, 0.8, 0.0),
        );
        app.update_n(60);
        // Send the tip along the rim, toward -Z.
        app.world.get_mut::<Velocity>(arm).unwrap().angular = Vec3::Y * 3.0;
        let mut moving = 0;
        for f in 1..=360 {
            app.update();
            if vel(&app, arm).angular.length() > 0.05 {
                moving = f;
            }
        }
        let secs = moving as f32 / 60.0;
        assert!(
            secs < 3.0,
            "articulated {articulated}: the arm swung along the rim for {secs} s"
        );
    }
}

/// Two arms hinged to a hub, each holding a motor's target angle. One of the two joints can't
/// make the hub its child (it already has a parent), so it is made from the hub's side: the
/// motor must still hold the same angle.
#[test]
fn hinge_motors_and_limits_match_constraint_joints() {
    let run = |articulated: bool| {
        let mut app = app(articulated);
        ground(&mut app);
        let hub = body(
            &mut app,
            DVec3::new(0.0, 0.5, 0.0),
            Quat::IDENTITY,
            Collider::cuboid(Vec3::splat(0.5)),
        );
        let arm = |app: &mut App, side: f64| {
            body(
                app,
                DVec3::new(side * 1.1, 0.8, 0.0),
                Quat::IDENTITY,
                Collider::cuboid(Vec3::new(0.5, 0.08, 0.08)),
            )
        };
        let (left, right) = (arm(&mut app, -1.0), arm(&mut app, 1.0));
        let motor = JointKind::Revolute {
            axis: Vec3::Z,
            limits: None,
            motor: Some(JointMotor {
                target_velocity: 0.0,
                target_position: Some(0.5),
                stiffness: 2.0e4,
                damping: 800.0,
                max_force: 1.0e5,
            }),
        };
        joint(
            &mut app,
            motor.clone(),
            left,
            hub,
            DVec3::new(-0.6, 0.8, 0.0),
        );
        joint(&mut app, motor, right, hub, DVec3::new(0.6, 0.8, 0.0));
        // A limited hinge on a third arm, out the front: gravity holds it on its limit.
        let front = body(
            &mut app,
            DVec3::new(0.0, 0.8, 1.1),
            Quat::IDENTITY,
            Collider::cuboid(Vec3::new(0.08, 0.08, 0.5)),
        );
        joint(
            &mut app,
            JointKind::Revolute {
                axis: Vec3::X,
                limits: Some([-0.3, 0.3]),
                motor: None,
            },
            hub,
            front,
            DVec3::new(0.0, 0.8, 0.6),
        );
        app.update_n(180);
        [
            angle(&app, left, hub, Vec3::Z),
            angle(&app, right, hub, Vec3::Z),
            angle(&app, hub, front, Vec3::X),
        ]
    };
    let (articulated, constraint) = (run(true), run(false));
    for (a, c) in articulated.iter().zip(&constraint) {
        assert!(
            (a - c).abs() < 0.03,
            "articulated {articulated:?} vs constraint {constraint:?}"
        );
    }
    assert!((articulated[0] - 0.5).abs() < 0.03 && (articulated[1] - 0.5).abs() < 0.03);
    assert!((articulated[2].abs() - 0.3).abs() < 0.03);
}

#[test]
fn fixed_joints_hold_bodies_as_they_were() {
    for articulated in [true, false] {
        let mut app = app(articulated);
        ground(&mut app);
        let turned = Quat::from_rotation_y(0.7) * Quat::from_rotation_x(0.2);
        let a = body(
            &mut app,
            DVec3::new(0.0, 2.0, 0.0),
            Quat::IDENTITY,
            Collider::cuboid(Vec3::splat(0.3)),
        );
        let b = body(
            &mut app,
            DVec3::new(0.7, 2.0, 0.0),
            turned,
            Collider::cuboid(Vec3::splat(0.3)),
        );
        joint(&mut app, JointKind::Fixed, a, b, DVec3::new(0.35, 2.0, 0.0));
        app.update_n(90);
        let rel = transform(&app, a).rotation.inverse() * transform(&app, b).rotation;
        assert!(
            rel.angle_between(turned) < 0.01,
            "articulated {articulated}: the weld turned the bodies to {rel:?}"
        );
    }
}

#[test]
fn articulations_follow_world_shifts() {
    let mut app = app(true);
    zero_g(&mut app);
    let (parts, joints) = chain(&mut app, DVec3::ZERO, 4);
    for (i, p) in parts.iter().enumerate() {
        let mut v = app.world.get_mut::<Velocity>(*p).unwrap();
        v.linear = Vec3::new(0.5, 0.0, 0.2 * i as f32);
    }
    app.update_n(20);
    let p0 = momentum(&app, &parts);
    let s = WorldShift {
        rotation: DQuat::from_rotation_y(0.8) * DQuat::from_rotation_x(-0.4),
        translation: DVec3::new(2.0e4, -300.0, 5.0e3),
        ..WorldShift::IDENTITY
    };
    app.world.write_message(s);
    app.update();
    let p1 = momentum(&app, &parts);
    assert!(
        (p1 - s.direction(p0)).length() < 1e-3 * p0.length().max(1.0),
        "momentum {p1:?}, expected {:?}",
        s.direction(p0)
    );
    for _ in 0..60 {
        app.update();
        assert!(worst_gap(&app, &joints) < 1e-4);
    }
}

#[test]
fn a_chain_flying_into_another_region_stays_whole() {
    let mut app = app(true);
    zero_g(&mut app);
    app.world.resource_mut::<PhysicsSettings>().region_radius_m = 30.0;
    // A box left behind keeps the first region where it is.
    body(
        &mut app,
        DVec3::new(0.0, 0.0, -3.0),
        Quat::IDENTITY,
        Collider::cuboid(Vec3::splat(0.3)),
    );
    let (parts, joints) = chain(&mut app, DVec3::ZERO, 4);
    for p in &parts {
        app.world.get_mut::<Velocity>(*p).unwrap().linear = Vec3::new(40.0, 0.0, 0.0);
    }
    let mut regions = 0;
    for _ in 0..240 {
        app.update();
        regions = regions.max(app.world.resource::<PhysicsStats>().regions);
        assert!(worst_gap(&app, &joints) < 1e-3);
    }
    assert!(regions >= 2, "the chain never left its region");
    for p in &parts {
        let v = vel(&app, *p).linear;
        assert!((v.x - 40.0).abs() < 0.05, "a link slowed to {v:?}");
    }
}

#[test]
fn removing_a_joint_splits_the_articulation() {
    let mut app = app(true);
    zero_g(&mut app);
    let (parts, joints) = chain(&mut app, DVec3::ZERO, 5);
    for p in &parts {
        app.world.get_mut::<Velocity>(*p).unwrap().linear = Vec3::new(0.0, 1.0, 0.0);
    }
    app.update_n(5);
    app.world.despawn(joints[1]);
    app.update_n(30);
    for p in &parts {
        let v = vel(&app, *p).linear;
        assert!(
            (v - Vec3::Y).length() < 1e-3,
            "a link changed speed when the chain split: {v:?}"
        );
    }
    // Pull the two halves apart: they come apart at the removed joint only.
    app.world.get_mut::<Velocity>(parts[0]).unwrap().linear = Vec3::new(-2.0, 1.0, 0.0);
    app.update_n(30);
    assert!(gap(&app, joints[0]) < 1e-4 && gap(&app, joints[2]) < 1e-4);
    assert!(pos(&app, parts[1]).distance(pos(&app, parts[2])) > 1.0);
}

#[test]
fn switching_articulated_joints_off_and_on_rebuilds_them() {
    let mut app = app(true);
    ground(&mut app);
    let (parts, joints) = chain(&mut app, DVec3::new(-1.0, 3.0, 0.0), 6);
    app.update_n(30);
    app.world
        .resource_mut::<PhysicsSettings>()
        .articulated_joints = false;
    app.update_n(30);
    assert!(worst_gap(&app, &joints) < 0.03);
    app.world
        .resource_mut::<PhysicsSettings>()
        .articulated_joints = true;
    app.update_n(30);
    assert!(worst_gap(&app, &joints) < 1e-4);
    assert!(parts.iter().all(|p| pos(&app, *p).y > 0.0));
}

fn articulated(app: &App) -> usize {
    app.world.resource::<PhysicsStats>().articulated
}

#[test]
fn chains_pinned_to_the_world_keep_constraint_joints_until_let_go() {
    let mut app = app(true);
    ground(&mut app);
    let (parts, joints) = chain(&mut app, DVec3::new(-1.0, 3.0, 0.0), 5);
    app.update();
    assert_eq!(articulated(&app), 4);
    // Hang it from a world point: pinned, reduced coordinates can't give way to a contact they
    // can't satisfy, so the whole chain goes back to constraint joints.
    let first = transform(&app, parts[0]);
    let pin = app
        .world
        .spawn(Joint {
            kind: JointKind::Spherical { cone_limit: None },
            body_a: parts[0],
            body_b: None,
            anchor_a: Vec3::new(-0.3, 0.0, 0.0),
            anchor_b: first.translation + (first.rotation * Vec3::new(-0.3, 0.0, 0.0)).as_dvec3(),
            break_force: None,
            collide_connected: false,
        })
        .id();
    app.update_n(60);
    assert_eq!(articulated(&app), 0);
    assert!(
        worst_gap(&app, &joints) < 0.03,
        "the chain holds while pinned"
    );
    // Let go: articulated again, and it falls in one piece.
    app.world.despawn(pin);
    app.update_n(60);
    assert_eq!(articulated(&app), 4);
    assert!(worst_gap(&app, &joints) < 1e-4);
}

#[test]
fn a_far_heavier_body_keeps_its_group_on_constraint_joints() {
    let mut app = app(true);
    ground(&mut app);
    let (parts, joints) = chain(&mut app, DVec3::new(-1.0, 3.0, 0.0), 4);
    app.update();
    assert_eq!(articulated(&app), 3);
    // A 17 t steel ball on 4 kg links: Rapier's articulations blow up when it hits something,
    // so the chain keeps constraint joints while the ball hangs on it.
    let end = pos(&app, parts[3]) + DVec3::X * 1.1;
    let ball = body(
        &mut app,
        end,
        Quat::IDENTITY,
        Collider::sphere(0.8).with_density(7800.0),
    );
    let hook = joint(
        &mut app,
        JointKind::Spherical { cone_limit: None },
        parts[3],
        ball,
        end - DVec3::X * 0.8,
    );
    app.update();
    assert_eq!(articulated(&app), 0);
    app.update_n(120);
    assert!(
        pos(&app, ball).is_finite() && pos(&app, ball).y < 1.0,
        "the ball lands"
    );
    app.world.despawn(hook);
    app.update();
    assert_eq!(articulated(&app), 3);
    app.update_n(60);
    assert!(worst_gap(&app, &joints) < 1e-4);
}

/// Only the `max_articulated` moving groups nearest the focus are articulations; the rest use
/// constraint joints. The budget follows the focus, and a group that falls asleep keeps its
/// joints but gives its place to the next nearest.
#[test]
fn the_budget_articulates_the_groups_nearest_the_focus() {
    let mut app = app(true);
    zero_g(&mut app);
    app.world.resource_mut::<PhysicsSettings>().max_articulated = 2;
    let focus = app
        .world
        .spawn((
            Transform::from_xyz(0.0, 5.0, -2.0),
            SimulationFocus::default(),
        ))
        .id();
    // Chain `i` has `i + 1` joints, so the count of articulated joints tells which are.
    let chains: Vec<Vec<Entity>> = (0..6)
        .map(|i| {
            let (parts, _) = chain(&mut app, DVec3::new(0.0, 5.0, 4.0 * i as f64), i + 2);
            for p in &parts {
                app.world.get_mut::<Velocity>(*p).unwrap().linear = Vec3::Y;
            }
            parts
        })
        .collect();
    app.update_n(2);
    assert_eq!(articulated(&app), 1 + 2, "the two chains nearest the focus");
    app.world.get_mut::<Transform>(focus).unwrap().translation = DVec3::new(0.0, 5.0, 22.0);
    app.update_n(10);
    assert_eq!(articulated(&app), 5 + 6, "the two nearest the moved focus");
    // Those two stop and fall asleep: the next two nearest take their places.
    for p in chains[4].iter().chain(&chains[5]) {
        *app.world.get_mut::<Velocity>(*p).unwrap() = Velocity::default();
    }
    app.update_n(240);
    assert_eq!(articulated(&app), 5 + 6 + 3 + 4);
}
