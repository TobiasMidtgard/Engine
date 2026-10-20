//! Pendulums: a Newton's cradle passes its blow down the row (`engine_physics::FineSteps`), and
//! a pendulum wave comes back into step.

use engine_core::math::{DVec3, Quat, Vec3};
use engine_core::prelude::*;
use engine_core::time::TimeSource;
use engine_physics::{FineSteps, PhysicsPlugin, PhysicsStats, demo};
use engine_world::physics::{Collider, Joint, JointKind, RigidBody, Velocity};
use engine_world::{SimulationFocus, SurfaceId, SurfaceMaterial, SurfaceMaterials, WorldPlugin};

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
    app.world
        .spawn((Transform::default(), SimulationFocus::default()));
    app
}

fn stats(app: &App) -> PhysicsStats {
    *app.world.resource::<PhysicsStats>()
}

fn position(app: &App, e: Entity) -> DVec3 {
    app.world.get::<Transform>(e).unwrap().translation
}

/// A steel bob of `radius` hung from `pivot` at `length` (pivot to center) on a hinge about
/// `axis`, pulled `angle` radians out of the vertical.
fn bob(
    app: &mut App,
    pivot: DVec3,
    length: f64,
    radius: f32,
    axis: Vec3,
    angle: f32,
    surface: SurfaceId,
) -> Entity {
    let turn = Quat::from_axis_angle(axis, angle);
    let center = pivot + (turn * Vec3::new(0.0, -length as f32, 0.0)).as_dvec3();
    let e = app
        .world
        .spawn((
            Transform::from_translation(center).with_rotation(turn),
            RigidBody::dynamic(),
            Collider::sphere(radius)
                .with_surface(surface)
                .with_density(7800.0),
            Velocity::default(),
        ))
        .id();
    app.world.spawn(Joint {
        kind: JointKind::Revolute {
            axis: turn.inverse() * axis,
            limits: None,
            motor: None,
        },
        body_a: e,
        body_b: None,
        anchor_a: turn.inverse() * (pivot - center).as_vec3(),
        anchor_b: pivot,
        break_force: None,
        collide_connected: false,
    });
    e
}

fn steel(app: &mut App) -> SurfaceId {
    app.world
        .resource_mut::<SurfaceMaterials>()
        .add(SurfaceMaterial {
            name: "hardened_steel".into(),
            friction: 0.05,
            restitution: 0.97,
            ..Default::default()
        })
}

const R: f32 = 0.1;
/// Between two balls at rest: more than the solver's contact prediction distance (2 mm), so
/// a ball at rest is not touching its neighbor before the blow arrives.
const GAP: f64 = 0.003;
const PITCH: f64 = 2.0 * R as f64 + GAP;

/// A cradle of five balls on 0.8 m hinges about Z, along +X from `at`, with the first `lifted`
/// pulled out 0.6 rad. Fine steps unless `coarse`.
fn cradle(app: &mut App, at: DVec3, lifted: usize, coarse: bool) -> Vec<Entity> {
    let surface = steel(app);
    (0..5)
        .map(|i| {
            let pivot = at + DVec3::new(i as f64 * PITCH, 0.8, 0.0);
            let angle = if i < lifted { -0.6 } else { 0.0 };
            let e = bob(app, pivot, 0.8, R, Vec3::Z, angle, surface);
            if !coarse {
                app.world.entity_mut(e).insert(FineSteps::default());
            }
            e
        })
        .collect()
}

/// Each ball's farthest swing from rest over `frames` frames, meters along X.
fn swings(app: &mut App, at: DVec3, balls: &[Entity], frames: usize) -> Vec<f64> {
    let mut peak = vec![0.0f64; balls.len()];
    for _ in 0..frames {
        app.update();
        for (i, b) in balls.iter().enumerate() {
            let rest = at.x + i as f64 * PITCH;
            peak[i] = peak[i].max((position(app, *b).x - rest).abs());
        }
    }
    peak
}

#[test]
fn newtons_cradle_passes_the_blow_down_the_row() {
    // Lifted out 0.6 rad, a ball sits 0.45 m from rest.
    let out = 0.8 * 0.6f64.sin();
    for lifted in [1, 2] {
        let mut app = app();
        let at = DVec3::new(0.0, 0.8, 0.0);
        let balls = cradle(&mut app, at, lifted, false);
        // Up to the moment the far balls come back down.
        let peak = swings(&mut app, at, &balls, 70);
        for (i, p) in peak.iter().enumerate() {
            if i < 5 - lifted && i >= lifted {
                assert!(*p < 0.02, "{lifted} lifted: ball {i} swung {p} m: {peak:?}");
            }
            if i >= 5 - lifted {
                assert!(
                    *p > out * 0.85,
                    "{lifted} lifted: ball {i} swung {p} m: {peak:?}"
                );
            }
        }
        let s = stats(&app);
        assert!(s.fine_awake >= lifted && s.fine_awake == s.awake, "{s:?}");
    }
}

#[test]
fn without_fine_steps_the_cradle_swings_as_one_lump() {
    // What `FineSteps` is for: in 8 ms substeps the middle balls swing off too.
    let mut app = app();
    let at = DVec3::new(0.0, 0.8, 0.0);
    let balls = cradle(&mut app, at, 1, true);
    let peak = swings(&mut app, at, &balls, 70);
    assert!(peak[2] > 0.03, "{peak:?}");
    assert_eq!(stats(&app).fine_awake, 0, "{:?}", stats(&app));
}

#[test]
fn fine_steps_keep_to_a_region_of_their_own_and_still_get_hit() {
    let mut app = app();
    app.world.spawn((
        Transform::from_xyz(0.0, -0.5, 0.0),
        RigidBody::fixed(),
        Collider::cuboid(Vec3::new(50.0, 0.5, 50.0)),
    ));
    // Crates tumbling down a stack 4 m from the cradle.
    let crates: Vec<Entity> = (0..12)
        .map(|i| {
            app.world
                .spawn((
                    Transform::from_xyz(-4.0 + (i % 3) as f64 * 0.05, 0.3 + i as f64 * 0.62, 0.0)
                        .with_rotation(Quat::from_rotation_y(i as f32 * 0.4)),
                    RigidBody::dynamic(),
                    Collider::cuboid(Vec3::splat(0.3)).with_density(300.0),
                    Velocity::default(),
                ))
                .id()
        })
        .collect();
    let at = DVec3::new(0.0, 0.8, 0.0);
    let balls = cradle(&mut app, at, 0, false);
    app.update_n(30);
    let s = stats(&app);
    assert!(s.regions >= 2, "{s:?}");
    assert!(s.fine_awake <= 5, "the crates step finely too: {s:?}");
    assert!(s.awake > s.fine_awake, "{s:?}");
    // A ball rolled along the row from 1.2 m (no gravity, so it flies straight) joins the
    // cradle's region before it hits, and knocks the far ball out.
    let end = at + DVec3::new(4.0 * PITCH, 0.0, 0.0);
    let surface = steel(&mut app);
    let thrown = app
        .world
        .spawn((
            Transform::from_translation(end + DVec3::new(1.2, 0.0, 0.0)),
            RigidBody {
                gravity_scale: 0.0,
                ..RigidBody::dynamic()
            },
            Collider::sphere(R)
                .with_surface(surface)
                .with_density(7800.0),
            Velocity {
                linear: Vec3::new(-2.5, 0.0, 0.0),
                angular: Vec3::ZERO,
            },
        ))
        .id();
    let peak = swings(&mut app, at, &balls, 60);
    assert!(
        peak[0] > 0.2,
        "the blow never reached the first ball: {peak:?}"
    );
    assert!(peak[2] < 0.03, "{peak:?}");
    let v = app.world.get::<Velocity>(thrown).unwrap().linear;
    assert!(v.x > -0.5, "the thrown ball went on through: {v}");
    // Once the thrown ball is gone again, the crates step coarsely.
    app.world.entity_mut(thrown).despawn();
    app.update_n(40);
    let s = stats(&app);
    assert!(s.regions >= 2, "{s:?}");
    assert!(s.fine_awake <= 5, "{s:?}");
    for c in crates {
        assert!(position(&app, c).y > 0.0);
    }
}

/// A pendulum wave: 15 bobs whose lengths make them swing 20, 21 … 34 times a minute. They
/// start in step, fall into waves and snakes, swing in two alternate rows at half a minute and
/// come back into step at a minute.
#[test]
fn a_pendulum_wave_comes_back_into_step_after_a_minute() {
    let mut app = app();
    let surface = steel(&mut app);
    // As the proving ground's (`demo::pendulum_wave`).
    let (cycle, r, amp) = (demo::WAVE_CYCLE_SECS, 0.1f32, 0.2f32);
    let pivots: Vec<DVec3> = (0..15)
        .map(|k| DVec3::new(-1.89 + 0.27 * k as f64, 3.2, 0.0))
        .collect();
    let bobs: Vec<Entity> = (0..15)
        .map(|k| {
            let length = demo::wave_length(20 + k, cycle, r, amp);
            bob(&mut app, pivots[k], length as f64, r, Vec3::X, amp, surface)
        })
        .collect();
    let angles = |app: &App| -> Vec<f32> {
        bobs.iter()
            .zip(&pivots)
            .map(|(b, p)| {
                let d = position(app, *b) - *p;
                (d.z as f32).atan2(-d.y as f32)
            })
            .collect()
    };
    app.update_n(30 * 60);
    let half = angles(&app);
    for (k, a) in half.iter().enumerate() {
        let want = if k % 2 == 0 { -amp } else { amp };
        assert!(
            (a - want).abs() < amp * 0.4,
            "at half a minute bob {k} is at {a}: {half:?}"
        );
    }
    app.update_n(30 * 60);
    let full = angles(&app);
    for (k, a) in full.iter().enumerate() {
        assert!(
            (a + amp).abs() < amp * 0.4,
            "at a minute bob {k} is at {a}: {full:?}"
        );
    }
}
