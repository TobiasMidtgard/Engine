//! Level of detail: far physics simulates coarsely, splits off and merges back
//! (`engine_physics::lod`).

use engine_core::math::{DVec3, Quat, Vec3};
use engine_core::prelude::*;
use engine_core::time::TimeSource;
use engine_physics::{PhysicsPlugin, PhysicsStats};
use engine_world::physics::{
    CharacterController, CharacterMove, Collider, ColliderShape, RigidBody, Velocity,
};
use engine_world::{Gravity, Scalability, SimulationFocus, WorldPlugin};

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
    // Simulate far out, so the coarse and half rate tiers are seen instead of resting.
    app.world.resource_mut::<Scalability>().simulation_m = 10_000.0;
    app
}

fn stats(app: &App) -> PhysicsStats {
    *app.world.resource::<PhysicsStats>()
}

/// A flat floor 2.4 km across, its top at y = 0.
fn floor(app: &mut App) {
    app.world.spawn((
        Transform::from_xyz(0.0, -0.5, 0.0),
        RigidBody::fixed(),
        Collider::cuboid(Vec3::new(1200.0, 0.5, 1200.0)),
    ));
}

fn seeded(mut seed: u64) -> impl FnMut() -> f32 {
    move || {
        seed = seed
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        ((seed >> 33) as f64 / (1u64 << 31) as f64) as f32
    }
}

fn rock(app: &mut App, at: DVec3, rnd: &mut impl FnMut() -> f32) -> Entity {
    let size = Vec3::new(0.18 + 0.12 * rnd(), 0.12 + 0.1 * rnd(), 0.15 + 0.12 * rnd());
    let points: Vec<Vec3> = (0..12)
        .map(|_| {
            Vec3::new(rnd() * 2.0 - 1.0, rnd() * 2.0 - 1.0, rnd() * 2.0 - 1.0).normalize() * size
        })
        .collect();
    app.world
        .spawn((
            Transform::from_translation(at).with_rotation(Quat::from_rotation_y(rnd() * 6.0)),
            RigidBody::dynamic(),
            Collider {
                shape: ColliderShape::ConvexHull { points },
                ..Collider::cuboid(Vec3::ONE)
            }
            .with_density(2700.0),
            Velocity::default(),
        ))
        .id()
}

/// A kinematic drum (radius 3 m, 4 m long) turning about +Z, with `rocks` rocks inside.
fn drum(app: &mut App, center: DVec3, rocks: usize, rnd: &mut impl FnMut() -> f32) -> Vec<Entity> {
    let body = app
        .world
        .spawn((
            Transform::from_translation(center),
            RigidBody::kinematic(),
            Velocity {
                linear: Vec3::ZERO,
                angular: Vec3::new(0.0, 0.0, 0.6),
            },
        ))
        .id();
    let staves = 10;
    let half_chord = 3.0 * (std::f32::consts::PI / staves as f32).tan() + 0.1;
    for k in 0..staves {
        let a = k as f32 / staves as f32 * std::f32::consts::TAU;
        app.world.spawn((
            Transform::from_xyz(a.cos() as f64 * 3.2, a.sin() as f64 * 3.2, 0.0)
                .with_rotation(Quat::from_rotation_z(a)),
            Collider::cuboid(Vec3::new(0.2, half_chord, 2.3)),
            ChildOf(body),
        ));
    }
    for z in [-2.15, 2.15] {
        app.world.spawn((
            Transform::from_xyz(0.0, 0.0, z),
            Collider::cuboid(Vec3::new(3.4, 3.4, 0.15)),
            ChildOf(body),
        ));
    }
    (0..rocks)
        .map(|i| {
            let cell = |k: usize| (k as f64 - 2.5) * 0.6;
            let at = DVec3::new(cell(i % 6), cell((i / 6) % 6), cell(i / 36) * 0.8);
            rock(app, center + at, rnd)
        })
        .collect()
}

fn position(app: &App, e: Entity) -> DVec3 {
    app.world.get::<Transform>(e).unwrap().translation
}

#[test]
fn far_drums_tumble_coarsely_and_keep_their_rocks() {
    let mut app = app();
    let mut rnd = seeded(3);
    let drums: Vec<(DVec3, Vec<Entity>)> = [20.0, 300.0, 900.0]
        .into_iter()
        .map(|x| {
            let c = DVec3::new(x, 3.5, 0.0);
            (c, drum(&mut app, c, 60, &mut rnd))
        })
        .collect();
    app.update_n(240);
    let s = stats(&app);
    let full = s.awake - s.coarse_awake - s.half_rate_awake;
    // Each tier tumbles its own drum (the drums are kinematic movers too).
    assert!((55..=61).contains(&full), "{s:?}");
    assert!((55..=61).contains(&s.coarse_awake), "{s:?}");
    assert!((55..=61).contains(&s.half_rate_awake), "{s:?}");
    assert!(s.regions >= 3, "{s:?}");
    for (c, rocks) in &drums {
        for r in rocks {
            let p = position(&app, *r) - *c;
            assert!(
                DVec3::new(p.x, p.y, 0.0).length() < 3.3 && p.z.abs() < 2.3,
                "a rock left the drum at {c}: {p}"
            );
        }
    }
}

#[test]
fn far_piles_settle_and_sleep() {
    let mut app = app();
    floor(&mut app);
    let mut rnd = seeded(11);
    let mut rocks = Vec::new();
    for x in [400.0, 800.0] {
        for i in 0..40 {
            let at = DVec3::new(
                x + (i % 5) as f64 * 0.6,
                0.5 + (i / 25) as f64 * 0.6,
                ((i / 5) % 5) as f64 * 0.6,
            );
            rocks.push((x, rock(&mut app, at, &mut rnd)));
        }
    }
    let (mut coarse, mut half) = (0, 0);
    for _ in 0..600 {
        app.update();
        let s = stats(&app);
        coarse = coarse.max(s.coarse_awake);
        half = half.max(s.half_rate_awake);
    }
    assert!(
        coarse >= 35 && half >= 35,
        "coarse {coarse}, half rate {half}"
    );
    assert_eq!(stats(&app).awake, 0, "{:?}", stats(&app));
    for (x, r) in rocks {
        let p = position(&app, r);
        // (A rock's origin can sit just outside its hull of 12 random points.)
        assert!(p.y > -0.1 && p.y < 2.0, "rock at {p}");
        assert!(
            (p.x - x).abs() < 6.0 && p.z.abs() < 8.0,
            "rock rolled far: {p}"
        );
    }
}

#[test]
fn a_ball_flying_in_from_afar_hits_the_near_crate_at_full_detail() {
    let mut app = app();
    app.world.resource_mut::<Gravity>().uniform = Vec3::ZERO;
    // Something near the focus keeps the near region at full detail.
    let crate_ = app
        .world
        .spawn((
            Transform::from_xyz(5.0, 0.0, 0.0),
            RigidBody::dynamic(),
            Collider::cuboid(Vec3::splat(0.5)).with_density(200.0),
            Velocity::default(),
        ))
        .id();
    let ball = app
        .world
        .spawn((
            Transform::from_xyz(260.0, 0.0, 0.0),
            RigidBody::dynamic(),
            Collider::sphere(0.4).with_density(2000.0),
            Velocity {
                linear: Vec3::new(-40.0, 0.0, 0.0),
                angular: Vec3::ZERO,
            },
        ))
        .id();
    let mut was_coarse = false;
    let mut hit = None;
    for frame in 0..600 {
        app.update();
        let s = stats(&app);
        was_coarse |= s.coarse_awake == 1;
        let v = app.world.get::<Velocity>(crate_).unwrap().linear;
        if hit.is_none() && v.length() > 0.5 {
            hit = Some((frame, s));
        }
    }
    assert!(was_coarse, "the far ball never simulated coarsely");
    let (frame, s) = hit.expect("the ball never hit the crate");
    assert_eq!(
        (s.coarse_awake, s.half_rate_awake),
        (0, 0),
        "the hit (frame {frame}) was not at full detail: {s:?}"
    );
    assert!(
        position(&app, crate_).x < 0.0,
        "the crate wasn't knocked away"
    );
    let ball_v = app.world.get::<Velocity>(ball).unwrap().linear;
    assert!(ball_v.x > -40.0 * 0.8, "the ball didn't slow: {ball_v}");
}

#[test]
fn far_characters_keep_walking_every_frame() {
    let mut app = app();
    floor(&mut app);
    let mut rnd = seeded(5);
    // Rocks tumbling beside the walker make its region worth simulating at half rate.
    for i in 0..20 {
        rock(
            &mut app,
            DVec3::new(
                805.0 + (i % 5) as f64 * 0.6,
                1.0 + (i / 5) as f64 * 0.6,
                0.0,
            ),
            &mut rnd,
        );
    }
    let walker = app
        .world
        .spawn((
            Transform::from_xyz(800.0, 1.0, 0.0),
            CharacterController::default(),
            CharacterMove {
                velocity: Vec3::new(0.0, 0.0, 1.5),
            },
        ))
        .id();
    app.update_n(60);
    let mut last = position(&app, walker);
    for frame in 0..120 {
        app.update();
        let p = position(&app, walker);
        assert!(
            p.z - last.z > 0.01,
            "the walker stood still on frame {frame}"
        );
        last = p;
    }
    assert_eq!(stats(&app).half_rate_awake, 0, "{:?}", stats(&app));
}

#[test]
fn piles_past_the_simulation_range_rest_until_someone_comes_near() {
    let mut app = app();
    *app.world.resource_mut::<Scalability>() = Scalability::default();
    floor(&mut app);
    let mut rnd = seeded(5);
    // Rocks in the air 300 m out: past the range they stay there, not stepped.
    let rocks: Vec<Entity> = (0..20)
        .map(|i| {
            let at = DVec3::new(300.0 + (i % 5) as f64 * 0.6, 3.0, (i / 5) as f64 * 0.6);
            rock(&mut app, at, &mut rnd)
        })
        .collect();
    app.update_n(120);
    let s = stats(&app);
    assert_eq!(s.resting_awake, 20, "{s:?}");
    for r in &rocks {
        assert!(position(&app, *r).y > 2.9, "a resting rock fell");
    }
    // A shot flying in from as far keeps flying: travelling things don't rest.
    let ball = app
        .world
        .spawn((
            Transform::from_xyz(-300.0, 1.0, 0.0),
            RigidBody::dynamic(),
            Collider::sphere(0.3).with_density(2000.0),
            Velocity {
                linear: Vec3::new(30.0, 0.0, 0.0),
                angular: Vec3::ZERO,
            },
        ))
        .id();
    app.update_n(60);
    assert!(
        position(&app, ball).x > -290.0,
        "the far shot hung in the air"
    );
    // Walk the focus over: the rocks wake and fall.
    let focus = app
        .world
        .query_filtered::<Entity, With<SimulationFocus>>()
        .single(&app.world)
        .unwrap();
    app.world.get_mut::<Transform>(focus).unwrap().translation = DVec3::new(290.0, 2.0, 0.0);
    app.update_n(240);
    for r in &rocks {
        assert!(position(&app, *r).y < 1.0, "a rock never fell");
    }
}

/// A rock fallen through the ground far away (out there the terrain's colliders may not be
/// loaded) rests like the rest past the range, though it falls fast: it isn't travelling
/// anywhere, and kept its region stepping for good, falling ever deeper.
#[test]
fn a_rock_fallen_through_the_ground_far_away_rests() {
    let mut app = app();
    *app.world.resource_mut::<Scalability>() = Scalability::default();
    let rock = app
        .world
        .spawn((
            Transform::from_xyz(400.0, -120.0, 0.0),
            RigidBody::dynamic(),
            Collider::sphere(0.3).with_density(2000.0),
            Velocity {
                linear: Vec3::new(0.0, -40.0, 0.0),
                angular: Vec3::ZERO,
            },
        ))
        .id();
    app.update_n(30);
    let y = position(&app, rock).y;
    app.update_n(60);
    let s = stats(&app);
    assert_eq!(s.resting_awake, 1, "{s:?}");
    assert!(
        (position(&app, rock).y - y).abs() < 0.01,
        "the lost rock kept falling: {y} -> {}",
        position(&app, rock).y
    );
}
