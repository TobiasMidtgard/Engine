//! The README's "Done when" scenarios, run headless at a fixed 60 Hz.

use engine_core::math::{DVec3, Quat, Vec3};
use engine_core::prelude::*;
use engine_core::time::TimeSource;
use engine_physics::{PhysicsPlugin, PhysicsStats};
use engine_world::WorldPlugin;
use engine_world::physics::{Collider, ColliderShape, Joint, JointKind, RigidBody, Velocity};

const DT: f64 = 1.0 / 60.0;

fn app() -> App {
    let mut app = App::new();
    app.add_plugins(WorldPlugin);
    app.add_plugins(PhysicsPlugin);
    app.world
        .resource_mut::<Time>()
        .set_source(TimeSource::Manual { step_secs: DT });
    app.world.resource_mut::<FixedTime>().set_hz(60.0);
    app
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

fn pos(app: &App, e: Entity) -> DVec3 {
    app.world.get::<Transform>(e).unwrap().translation
}

fn rot(app: &App, e: Entity) -> Quat {
    app.world.get::<Transform>(e).unwrap().rotation
}

fn speed(app: &App, e: Entity) -> f32 {
    let v = app.world.get::<Velocity>(e).unwrap();
    v.linear.length().max(v.angular.length())
}

fn stats(app: &App) -> PhysicsStats {
    *app.world.resource::<PhysicsStats>()
}

/// Deterministic pseudo-random numbers in 0..1.
struct Rng(u32);

impl Rng {
    fn next(&mut self) -> f32 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 17;
        self.0 ^= self.0 << 5;
        (self.0 >> 8) as f32 / (1u32 << 24) as f32
    }
}

/// An irregular rock: a convex hull of jittered box corners and face centers.
fn rock(rng: &mut Rng, size: f32) -> Collider {
    let mut points = Vec::new();
    for x in [-1.0f32, 0.0, 1.0] {
        for y in [-1.0f32, 0.0, 1.0] {
            for z in [-1.0f32, 0.0, 1.0] {
                if x == 0.0 && y == 0.0 && z == 0.0 {
                    continue;
                }
                let j = 0.7 + 0.3 * rng.next();
                points.push(Vec3::new(x, y, z).normalize() * size * j);
            }
        }
    }
    Collider {
        shape: ColliderShape::ConvexHull { points },
        density: 2600.0,
        ..Collider::default()
    }
}

// ---------------------------------------------------------------- pile and tower

#[test]
fn pile_of_1000_bodies_settles_without_jitter() {
    let mut app = app();
    ground(&mut app, DVec3::ZERO, 60.0);
    let mut rng = Rng(0x9e37_79b9);
    let mut bodies = Vec::new();
    for i in 0..1000 {
        let (x, y, z) = ((i % 10) as f64, (i / 100) as f64, ((i / 10) % 10) as f64);
        let at = DVec3::new(
            (x - 4.5) * 1.1 + rng.next() as f64 * 0.1,
            0.8 + y * 1.1,
            (z - 4.5) * 1.1 + rng.next() as f64 * 0.1,
        );
        let rotation = Quat::from_euler(
            EulerRot::XYZ,
            rng.next() * TAU,
            rng.next() * TAU,
            rng.next() * TAU,
        );
        let collider = if i % 2 == 0 {
            Collider::cuboid(Vec3::new(0.45, 0.3, 0.35)).with_density(600.0)
        } else {
            rock(&mut rng, 0.45)
        };
        bodies.push(body(&mut app, at, rotation, collider));
    }
    // Fall and settle: 15 s.
    let mut step_ms = Vec::new();
    for _ in 0..900 {
        app.update();
        step_ms.push(stats(&app).step_ms);
    }
    let before: Vec<DVec3> = bodies.iter().map(|e| pos(&app, *e)).collect();
    app.update_n(120);
    let jitter = bodies
        .iter()
        .zip(&before)
        .map(|(e, p)| pos(&app, *e).distance(*p))
        .fold(0.0, f64::max);
    let s = stats(&app);
    let busiest = step_ms.iter().copied().fold(0.0, f64::max);
    let mean = step_ms.iter().sum::<f64>() / step_ms.len() as f64;
    println!(
        "1000-body pile: mean step {mean:.2} ms, worst {busiest:.2} ms, {} awake after 17 s, \
         jitter {jitter:.5} m, {} regions",
        s.awake, s.regions
    );
    assert!(
        bodies.iter().all(|e| pos(&app, *e).y > -0.1),
        "a body fell through the ground"
    );
    assert!(
        jitter < 0.005,
        "bodies still moved {jitter} m over 2 s after settling"
    );
    assert!(s.awake <= 10, "{} bodies still awake after 17 s", s.awake);
}

#[test]
fn tower_of_30_boxes_stands_still() {
    let mut app = app();
    ground(&mut app, DVec3::ZERO, 10.0);
    let boxes: Vec<Entity> = (0..30)
        .map(|i| {
            body(
                &mut app,
                DVec3::new(0.0, 0.5 + i as f64, 0.0),
                Quat::IDENTITY,
                Collider::cuboid(Vec3::splat(0.5)).with_density(500.0),
            )
        })
        .collect();
    app.update_n(300);
    let early = pos(&app, boxes[29]);
    app.update_n(300);
    let top = pos(&app, boxes[29]);
    println!("30-box tower: top at {top:?}, {} awake", stats(&app).awake);
    assert!(
        top.x.abs() < 0.01 && top.z.abs() < 0.01,
        "tower leaned: top at {top:?}"
    );
    // Contacts give a little under load (about 2 mm each at the bottom).
    assert!((top.y - 29.5).abs() < 0.1, "tower sagged: top at {top:?}");
    assert!(
        top.distance(early) < 0.001,
        "tower still creeping: {early:?} → {top:?}"
    );
    for b in &boxes {
        assert!(speed(&app, *b) < 0.01, "box still moving");
        assert!(
            rot(&app, *b).angle_between(Quat::IDENTITY) < 0.01,
            "box rotated"
        );
    }
}

// ---------------------------------------------------------------- ragdoll and chain

struct Rig {
    parts: Vec<Entity>,
    joints: Vec<Entity>,
}

fn joint_at(app: &mut App, a: Entity, b: Option<Entity>, at: DVec3, kind: JointKind) -> Entity {
    let pa = pos(app, a);
    let anchor_b = match b {
        Some(b) => at - pos(app, b),
        None => at,
    };
    app.world
        .spawn(Joint {
            kind,
            body_a: a,
            body_b: b,
            anchor_a: (at - pa).as_vec3(),
            anchor_b,
            break_force: None,
            collide_connected: false,
        })
        .id()
}

/// A 10-part humanoid ragdoll standing with its feet at `origin`.
fn ragdoll(app: &mut App, origin: DVec3) -> Rig {
    let o = origin;
    let part = |app: &mut App, at: DVec3, c: Collider| body(app, o + at, Quat::IDENTITY, c);
    let pelvis = part(
        app,
        DVec3::new(0.0, 1.0, 0.0),
        Collider::cuboid(Vec3::new(0.16, 0.1, 0.1)),
    );
    let chest = part(
        app,
        DVec3::new(0.0, 1.38, 0.0),
        Collider::capsule(0.15, 0.12),
    );
    let head = part(app, DVec3::new(0.0, 1.8, 0.0), Collider::sphere(0.12));
    let mut parts = vec![pelvis, chest, head];
    let mut joints = vec![
        joint_at(
            app,
            pelvis,
            Some(chest),
            o + DVec3::new(0.0, 1.1, 0.0),
            JointKind::Spherical {
                cone_limit: Some(0.5),
            },
        ),
        joint_at(
            app,
            chest,
            Some(head),
            o + DVec3::new(0.0, 1.66, 0.0),
            JointKind::Spherical {
                cone_limit: Some(0.6),
            },
        ),
    ];
    let knee = |axis| JointKind::Revolute {
        axis,
        limits: Some([-2.2, 0.0]),
        motor: None,
    };
    for side in [-1.0, 1.0] {
        let upper_arm = part(
            app,
            DVec3::new(side * 0.3, 1.38, 0.0),
            Collider::capsule(0.05, 0.1),
        );
        let forearm = part(
            app,
            DVec3::new(side * 0.3, 1.08, 0.0),
            Collider::capsule(0.045, 0.1),
        );
        let thigh = part(
            app,
            DVec3::new(side * 0.1, 0.7, 0.0),
            Collider::capsule(0.07, 0.14),
        );
        let shin = part(
            app,
            DVec3::new(side * 0.1, 0.28, 0.0),
            Collider::capsule(0.06, 0.15),
        );
        joints.push(joint_at(
            app,
            chest,
            Some(upper_arm),
            o + DVec3::new(side * 0.3, 1.55, 0.0),
            JointKind::Spherical {
                cone_limit: Some(1.4),
            },
        ));
        joints.push(joint_at(
            app,
            upper_arm,
            Some(forearm),
            o + DVec3::new(side * 0.3, 1.23, 0.0),
            knee(Vec3::X),
        ));
        joints.push(joint_at(
            app,
            pelvis,
            Some(thigh),
            o + DVec3::new(side * 0.1, 0.92, 0.0),
            JointKind::Spherical {
                cone_limit: Some(1.2),
            },
        ));
        joints.push(joint_at(
            app,
            thigh,
            Some(shin),
            o + DVec3::new(side * 0.1, 0.5, 0.0),
            knee(Vec3::X),
        ));
        parts.extend([upper_arm, forearm, thigh, shin]);
    }
    Rig { parts, joints }
}

/// A chain of `n` capsule links hanging sideways from a world anchor at `top`.
fn chain(app: &mut App, top: DVec3, n: usize) -> Rig {
    let link = 0.4;
    let mut parts = Vec::new();
    let mut joints = Vec::new();
    for i in 0..n {
        let at = top + DVec3::X * (link * (i as f64 + 0.5));
        let e = body(
            app,
            at,
            Quat::from_rotation_z(std::f32::consts::FRAC_PI_2),
            Collider::capsule(0.06, 0.14).with_density(2000.0),
        );
        let hinge = top + DVec3::X * (link * i as f64);
        let j = match parts.last() {
            None => joint_at(
                app,
                e,
                None,
                hinge,
                JointKind::Spherical { cone_limit: None },
            ),
            Some(prev) => joint_at(
                app,
                *prev,
                Some(e),
                hinge,
                JointKind::Spherical { cone_limit: None },
            ),
        };
        // Joint anchors were computed with identity rotation; re-express them in the rotated
        // link frames.
        let r = rot(app, e).inverse();
        let mut joint = app.world.get_mut::<Joint>(j).unwrap();
        if joint.body_b.is_none() {
            joint.anchor_a = r * joint.anchor_a;
        } else {
            joint.anchor_a = r * joint.anchor_a;
            joint.anchor_b = (r * joint.anchor_b.as_vec3()).as_dvec3();
        }
        parts.push(e);
        joints.push(j);
    }
    Rig { parts, joints }
}

/// Largest distance between the two ends of any joint, meters.
fn worst_gap(app: &App, rig: &Rig) -> f64 {
    rig.joints
        .iter()
        .map(|j| {
            let joint = app.world.get::<Joint>(*j).unwrap();
            let ta = app.world.get::<Transform>(joint.body_a).unwrap();
            let a = ta.translation + (ta.rotation * joint.anchor_a).as_dvec3();
            let b = match joint.body_b {
                Some(b) => {
                    let tb = app.world.get::<Transform>(b).unwrap();
                    tb.translation + (tb.rotation * joint.anchor_b.as_vec3()).as_dvec3()
                }
                None => joint.anchor_b,
            };
            a.distance(b)
        })
        .fold(0.0, f64::max)
}

/// Linear kinetic plus potential energy of the rig, joules. Rotational energy is left out,
/// so with everything starting at rest this can only go up if the solver adds energy.
fn energy(app: &App, rig: &Rig) -> f64 {
    rig.parts
        .iter()
        .map(|p| {
            let m = app
                .world
                .get::<engine_world::physics::MassProperties>(*p)
                .map_or(0.0, |m| m.mass_kg as f64);
            let v = app.world.get::<Velocity>(*p).unwrap().linear.length() as f64;
            0.5 * m * v * v + m * 9.81 * pos(app, *p).y
        })
        .sum()
}

/// Step `frames` frames; returns the worst joint gap (m) and the largest energy gain over the
/// starting energy (J, as a fraction of the starting energy).
fn measure_rig(app: &mut App, rig: &Rig, frames: usize) -> (f64, f64) {
    app.update();
    let start = energy(app, rig);
    let mut worst = 0.0f64;
    let mut gain = 0.0f64;
    for _ in 0..frames {
        app.update();
        worst = worst.max(worst_gap(app, rig));
        for p in &rig.parts {
            let t = app.world.get::<Transform>(*p).unwrap();
            if !t.translation.is_finite() {
                return (f64::INFINITY, f64::INFINITY);
            }
        }
        gain = gain.max((energy(app, rig) - start) / start.abs().max(1.0));
    }
    (worst, gain)
}

fn run_rig(app: &mut App, rig: &Rig, frames: usize, what: &str) -> f64 {
    let (gap, gain) = measure_rig(app, rig, frames);
    assert!(gap.is_finite(), "{what}: a part's position went NaN");
    assert!(
        gain < 0.02,
        "{what}: the solver added {:.1}% energy",
        gain * 100.0
    );
    gap
}

#[test]
fn ragdoll_is_stable_in_slow_motion_and_fast_forward() {
    for (scale, frames, label) in [(0.1f32, 1200, "0.1x"), (1.0, 600, "1x"), (10.0, 300, "10x")] {
        let mut app = app();
        app.world.resource_mut::<Time>().set_base_scale(scale);
        ground(&mut app, DVec3::ZERO, 20.0);
        let rig = ragdoll(&mut app, DVec3::new(0.0, 0.5, 0.0));
        // Tip it over.
        let chest = rig.parts[1];
        app.world.get_mut::<Velocity>(chest).unwrap().linear = Vec3::new(2.0, 0.0, 1.0);
        let gap = run_rig(&mut app, &rig, frames, label);
        let lowest = rig
            .parts
            .iter()
            .map(|p| pos(&app, *p).y)
            .fold(f64::MAX, f64::min);
        let fastest = rig
            .parts
            .iter()
            .map(|p| speed(&app, *p))
            .fold(0.0, f32::max);
        println!("ragdoll {label}: worst joint gap {gap:.4} m, fastest part {fastest:.3}");
        assert!(gap < 0.03, "{label}: joints pulled apart by {gap} m");
        assert!(lowest > -0.05, "{label}: ragdoll fell through the ground");
        if scale >= 1.0 {
            assert!(
                fastest < 0.05,
                "{label}: ragdoll still moving ({fastest}) after {} world s",
                frames as f32 / 60.0 * scale
            );
        }
    }
}

#[test]
fn jointed_chain_is_stable_in_slow_motion_and_fast_forward() {
    for (scale, frames, label) in [(0.1f32, 1200, "0.1x"), (1.0, 600, "1x"), (10.0, 600, "10x")] {
        let mut app = app();
        app.world.resource_mut::<Time>().set_base_scale(scale);
        let rig = chain(&mut app, DVec3::new(0.0, 12.0, 0.0), 20);
        let gap = run_rig(&mut app, &rig, frames, label);
        let tip = pos(&app, *rig.parts.last().unwrap());
        println!("chain {label}: worst joint gap {gap:.4} m, tip at {tip:?}");
        assert!(gap < 0.03, "{label}: links pulled apart by {gap} m");
        // It swung down: the tip is below the anchor.
        assert!(
            tip.y < 10.0,
            "{label}: chain didn't swing down (tip {tip:?})"
        );
    }
}

// ---------------------------------------------------------------- far from the origin

/// A sliding box, a 3-box stack, a pendulum and a bouncing ball around `base`.
fn far_scene(app: &mut App, base: DVec3) -> Vec<Entity> {
    ground(app, base, 30.0);
    let slider = body(
        app,
        base + DVec3::new(-10.0, 0.5, 0.0),
        Quat::IDENTITY,
        Collider::cuboid(Vec3::splat(0.5)),
    );
    app.world.get_mut::<Velocity>(slider).unwrap().linear = Vec3::new(4.0, 0.0, 0.0);
    let mut out = vec![slider];
    for i in 0..3 {
        out.push(body(
            app,
            base + DVec3::new(5.0, 0.5 + i as f64, 5.0),
            Quat::IDENTITY,
            Collider::cuboid(Vec3::splat(0.5)),
        ));
    }
    let bob = body(
        app,
        base + DVec3::new(3.0, 6.0, -5.0),
        Quat::IDENTITY,
        Collider::sphere(0.3),
    );
    joint_at(
        app,
        bob,
        None,
        base + DVec3::new(0.0, 6.0, -5.0),
        JointKind::Spherical { cone_limit: None },
    );
    out.push(bob);
    let ball = body(
        app,
        base + DVec3::new(-5.0, 4.0, 8.0),
        Quat::IDENTITY,
        Collider::sphere(0.4),
    );
    app.world.get_mut::<Velocity>(ball).unwrap().linear = Vec3::new(1.0, 0.0, -0.5);
    out.push(ball);
    out
}

#[test]
fn far_from_origin_behaves_like_the_origin() {
    for base in [
        DVec3::new(1.0e8, 0.0, -1.0e8),
        DVec3::new(3.84e8, 1.0e6, 0.0),
    ] {
        let mut far = app();
        let far_bodies = far_scene(&mut far, base);
        let mut near = app();
        let near_bodies = far_scene(&mut near, DVec3::ZERO);
        let mut worst = 0.0f64;
        for _ in 0..300 {
            near.update();
            far.update();
            for (a, b) in near_bodies.iter().zip(&far_bodies) {
                let d = pos(&near, *a).distance(pos(&far, *b) - base);
                worst = worst.max(d);
            }
        }
        println!("far from origin ({base:?}): worst difference {worst:.6} m");
        assert!(
            worst < 1.0e-3,
            "at {base:?} bodies drifted {worst} m from the same scene at the origin"
        );
        // And the scene did move.
        let slid = pos(&far, far_bodies[0]) - base;
        assert!(slid.x > -9.0, "the slider didn't slide: {slid:?}");
    }
}

// ---------------------------------------------------------------- API

#[test]
fn raycast_is_an_api_function() {
    let mut app = app();
    let g = ground(&mut app, DVec3::new(0.0, 2.0, 0.0), 5.0);
    app.world.entity_mut(g).insert(Name::new("Floor"));
    app.update_n(2);
    let hit = engine_core::registry::call_api(
        &mut app.world,
        "physics.raycast",
        serde_json::json!({"origin": [1, 10, 1], "dir": [0, -1, 0], "max_distance": 50}),
    )
    .unwrap();
    assert_eq!(hit["hit"], true, "{hit}");
    assert_eq!(hit["name"], "Floor", "{hit}");
    assert!(
        (hit["distance"].as_f64().unwrap() - 8.0).abs() < 1e-3,
        "{hit}"
    );
    let miss = engine_core::registry::call_api(
        &mut app.world,
        "physics.raycast",
        serde_json::json!({"origin": [1, 10, 1], "dir": [0, 1, 0]}),
    )
    .unwrap();
    assert_eq!(miss["hit"], false);
}

#[test]
fn every_api_example_runs() {
    let mut app = app();
    let g = ground(&mut app, DVec3::ZERO, 5.0);
    app.world.entity_mut(g).insert(Name::new("Player"));
    let c = body(
        &mut app,
        DVec3::new(0.0, 1.0, 0.0),
        Quat::IDENTITY,
        Collider::sphere(0.5),
    );
    app.world.entity_mut(c).insert(Name::new("Crate"));
    app.update_n(2);
    let examples: Vec<(String, String)> = app
        .world
        .resource::<ApiRegistry>()
        .list()
        .filter(|e| e.name.starts_with("physics."))
        .map(|e| (e.name.clone(), e.args_example.clone()))
        .collect();
    assert_eq!(examples.len(), 9);
    for (name, args) in examples {
        let args = serde_json::from_str(&args).unwrap_or_else(|e| panic!("{name}: {e}"));
        engine_core::registry::call_api(&mut app.world, &name, args)
            .unwrap_or_else(|e| panic!("{name}: {e}"));
    }
}

#[test]
fn same_scene_same_result() {
    let run = || {
        let mut app = app();
        let rig = chain(&mut app, DVec3::new(0.0, 12.0, 0.0), 20);
        ground(&mut app, DVec3::ZERO, 10.0);
        let mut rng = Rng(7);
        let boxes: Vec<Entity> = (0..60)
            .map(|i| {
                let at = DVec3::new(
                    rng.next() as f64 * 4.0,
                    1.0 + i as f64 * 0.6,
                    rng.next() as f64 * 4.0,
                );
                body(
                    &mut app,
                    at,
                    Quat::from_rotation_y(rng.next() * TAU),
                    Collider::cuboid(Vec3::splat(0.25)),
                )
            })
            .collect();
        app.update_n(240);
        rig.parts
            .iter()
            .chain(&boxes)
            .map(|e| pos(&app, *e))
            .collect::<Vec<_>>()
    };
    let a = run();
    for _ in 0..3 {
        assert_eq!(a, run(), "two runs of the same scene differ");
    }
}
