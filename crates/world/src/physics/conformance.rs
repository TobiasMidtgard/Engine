//! Conformance suite every physics backend must pass, so backends are interchangeable.
//!
//! In the backend crate:
//!
//! ```ignore
//! #[test]
//! fn conforms_to_the_physics_contract() {
//!     engine_world::physics::conformance::assert_all(|app| {
//!         app.add_plugins(MyPhysicsPlugin);
//!     });
//! }
//! ```
//!
//! Each check builds a fresh app (core + `WorldPlugin` + your plugin), steps it at a fixed
//! 60 Hz with deterministic time, and compares against physics with generous tolerances. The
//! checks cover what other modules rely on: gravity, resting contact, impulses, contact
//! events (one `Started` per touch carrying the impact impulse and a normal from `a` toward
//! `b`, one `Ended` when it stops), raycasts, time dilation, precision far from the origin,
//! following a [`WorldShift`](crate::WorldShift) (bodies keep their place relative to each
//! other when the world is re-anchored and turned) and the character controller (including its time scaling: a character moves in its own seconds,
//! so one that ignores world slow motion keeps full speed).

use engine_core::math::{DVec3, Vec3};
use engine_core::time::{LocalTimeScale, TimeSource};

use super::*;
use crate::{WorldPlugin, WorldShift, WorldShiftSet};

/// Result of one check.
#[derive(Debug, Clone)]
pub struct Check {
    pub name: &'static str,
    pub result: Result<(), String>,
}

type CheckFn = fn(&dyn Fn(&mut App)) -> Result<(), String>;

const CHECKS: &[(&str, CheckFn)] = &[
    (
        "installs_queries_and_claims_backend",
        installs_queries_and_claims_backend,
    ),
    ("falls_under_gravity", falls_under_gravity),
    ("rests_on_static_box", rests_on_static_box),
    ("raycast_hits_static_box", raycast_hits_static_box),
    ("impulse_changes_velocity", impulse_changes_velocity),
    ("uses_mass_overrides", uses_mass_overrides),
    ("reports_contacts", reports_contacts),
    ("reports_contact_end", reports_contact_end),
    ("time_dilation_slows_bodies", time_dilation_slows_bodies),
    (
        "bodies_honor_local_time_scale",
        bodies_honor_local_time_scale,
    ),
    ("precise_far_from_origin", precise_far_from_origin),
    ("follows_world_shifts", follows_world_shifts),
    ("character_walks_and_grounds", character_walks_and_grounds),
    (
        "character_honors_local_time_scale",
        character_honors_local_time_scale,
    ),
];

/// Run every check with `install` adding the backend to a fresh app.
pub fn run_all(install: impl Fn(&mut App)) -> Vec<Check> {
    CHECKS
        .iter()
        .map(|(name, check)| {
            let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| check(&install)))
                .unwrap_or_else(|p| {
                    let msg = p
                        .downcast_ref::<String>()
                        .cloned()
                        .or_else(|| p.downcast_ref::<&str>().map(|s| s.to_string()))
                        .unwrap_or_else(|| "panicked".into());
                    Err(format!("panicked: {msg}"))
                });
            Check { name, result }
        })
        .collect()
}

/// Run every check and panic with a list of failures.
pub fn assert_all(install: impl Fn(&mut App)) {
    let failures: Vec<String> = run_all(install)
        .into_iter()
        .filter_map(|c| c.result.err().map(|e| format!("{}: {e}", c.name)))
        .collect();
    assert!(
        failures.is_empty(),
        "physics backend fails the contract:\n  {}",
        failures.join("\n  ")
    );
}

// ---------------------------------------------------------------- helpers

const DT: f64 = 1.0 / 60.0;

fn app_with(install: &dyn Fn(&mut App)) -> App {
    let mut app = App::new();
    app.add_plugins(WorldPlugin);
    install(&mut app);
    app.world
        .resource_mut::<Time>()
        .set_source(TimeSource::Manual { step_secs: DT });
    app.world.resource_mut::<FixedTime>().set_hz(60.0);
    app.init_resource::<SeenContacts>();
    app.add_systems(Last, collect_contacts);
    app
}

#[derive(Resource, Default)]
struct SeenContacts(Vec<ContactEvent>);

fn collect_contacts(mut seen: ResMut<SeenContacts>, mut events: MessageReader<ContactEvent>) {
    seen.0.extend(events.read().copied());
}

fn ground(app: &mut App, base: DVec3) -> Entity {
    app.world
        .spawn((
            Transform::from_translation(base),
            RigidBody::fixed(),
            Collider::cuboid(Vec3::new(10.0, 0.5, 10.0)),
        ))
        .id()
}

fn position(app: &App, e: Entity) -> DVec3 {
    app.world
        .get::<Transform>(e)
        .map(|t| t.translation)
        .unwrap_or(DVec3::NAN)
}

fn velocity(app: &App, e: Entity) -> Vec3 {
    app.world
        .get::<Velocity>(e)
        .map(|v| v.linear)
        .unwrap_or(Vec3::NAN)
}

fn near(what: &str, got: f64, want: f64, tol: f64) -> Result<(), String> {
    if (got - want).abs() <= tol {
        Ok(())
    } else {
        Err(format!("{what}: got {got:.4}, expected {want:.4} ± {tol}"))
    }
}

// ---------------------------------------------------------------- checks

fn installs_queries_and_claims_backend(install: &dyn Fn(&mut App)) -> Result<(), String> {
    let mut app = app_with(install);
    if app.world.get_resource::<PhysicsBackend>().is_none() {
        return Err("the plugin must call engine_world::physics::claim_backend".into());
    }
    let g = ground(&mut app, DVec3::ZERO);
    app.update_n(2);
    let hit = app.world.resource::<PhysicsQueries>().raycast(
        DVec3::new(0.0, 5.0, 0.0),
        Vec3::NEG_Y,
        10.0,
        &QueryFilter::default(),
    );
    match hit {
        Some(h) if h.entity == g => Ok(()),
        Some(h) => Err(format!(
            "raycast hit {:?}, expected the ground {g:?}",
            h.entity
        )),
        None => Err(
            "PhysicsQueries still answers nothing: replace it with the backend's queries".into(),
        ),
    }
}

fn falls_under_gravity(install: &dyn Fn(&mut App)) -> Result<(), String> {
    let mut app = app_with(install);
    let ball = app
        .world
        .spawn((
            Transform::from_xyz(0.0, 100.0, 0.0),
            RigidBody::dynamic(),
            Collider::sphere(0.5),
            Velocity::default(),
        ))
        .id();
    app.update_n(60);
    near(
        "height after 1 s of free fall",
        position(&app, ball).y,
        100.0 - 4.905,
        0.25,
    )?;
    near(
        "speed after 1 s of free fall",
        velocity(&app, ball).y as f64,
        -9.81,
        0.4,
    )
}

fn rests_on_static_box(install: &dyn Fn(&mut App)) -> Result<(), String> {
    resting_at(install, DVec3::ZERO)
}

fn precise_far_from_origin(install: &dyn Fn(&mut App)) -> Result<(), String> {
    resting_at(install, DVec3::new(1.0e8, 2.0e7, -1.0e8))
        .map_err(|e| format!("at 100 000 km from the origin: {e}"))
}

fn resting_at(install: &dyn Fn(&mut App), base: DVec3) -> Result<(), String> {
    let mut app = app_with(install);
    ground(&mut app, base);
    let crate_e = app
        .world
        .spawn((
            Transform::from_translation(base + DVec3::new(0.0, 3.0, 0.0)),
            RigidBody::dynamic(),
            Collider::cuboid(Vec3::splat(0.5)),
            Velocity::default(),
        ))
        .id();
    app.update_n(180);
    let p = position(&app, crate_e) - base;
    near(
        "crate resting height above the ground center",
        p.y,
        1.0,
        0.05,
    )?;
    near("crate drift sideways", p.x.hypot(p.z), 0.0, 0.05)?;
    near(
        "crate speed at rest",
        velocity(&app, crate_e).length() as f64,
        0.0,
        0.05,
    )
}

fn follows_world_shifts(install: &dyn Fn(&mut App)) -> Result<(), String> {
    let mut app = app_with(install);
    app.init_resource::<PendingShift>()
        .add_systems(PostUpdate, write_pending_shift.in_set(WorldShiftSet::Write));
    let g = ground(&mut app, DVec3::ZERO);
    // Off the turn's axis, so a backend that moves bodies but doesn't turn them shows drift.
    let start = DVec3::new(6.0, 3.0, -4.0);
    let crate_e = app
        .world
        .spawn((
            Transform::from_translation(start),
            RigidBody::dynamic(),
            Collider::cuboid(Vec3::splat(0.5)),
            Velocity::default(),
        ))
        .id();
    // Re-anchor and turn the world while the crate is falling, the way a game does it (in
    // `WorldShiftSet::Write`). A turn about the vertical, so the check doesn't also demand
    // tilted colliders; gravity turning with the world is `engine_world`'s job, tested there.
    app.update_n(20);
    let shift = WorldShift {
        rotation: engine_core::math::DQuat::from_rotation_y(0.7),
        translation: DVec3::new(5.0e4, -300.0, 1.0e4),
        ..WorldShift::IDENTITY
    };
    app.world.resource_mut::<PendingShift>().0 = Some(shift);
    app.update();
    // Queries answer in the new frame right away, before the next step.
    let q = app.world.resource::<PhysicsQueries>().clone();
    let hit = q
        .raycast(
            shift.point(DVec3::new(0.0, 5.0, 0.0)),
            -shift.direction(Vec3::Y),
            20.0,
            &QueryFilter::default(),
        )
        .ok_or("right after the shift, a raycast down onto the shifted ground missed")?;
    if hit.entity != g {
        return Err(format!(
            "right after the shift, the raycast hit {:?}, expected the ground {g:?}",
            hit.entity
        ));
    }
    near(
        "ground top hit right after the shift",
        hit.point.y,
        shift.point(DVec3::new(0.0, 0.5, 0.0)).y,
        0.05,
    )?;
    app.update_n(160);
    let ground_t = *app.world.get::<Transform>(g).ok_or("the ground is gone")?;
    let local =
        ground_t.rotation.as_dquat().inverse() * (position(&app, crate_e) - ground_t.translation);
    near(
        "crate resting height above the shifted ground, in the ground's frame",
        local.y,
        1.0,
        0.05,
    )?;
    near(
        "crate drift from where it fell, in the ground's frame",
        (local.x - start.x).hypot(local.z - start.z),
        0.0,
        0.05,
    )?;
    near(
        "crate speed at rest after the shift",
        velocity(&app, crate_e).length() as f64,
        0.0,
        0.05,
    )
}

/// A shift for `follows_world_shifts` to write in `WorldShiftSet::Write`.
#[derive(Resource, Default)]
struct PendingShift(Option<WorldShift>);

fn write_pending_shift(mut pending: ResMut<PendingShift>, mut shifts: MessageWriter<WorldShift>) {
    if let Some(s) = pending.0.take() {
        shifts.write(s);
    }
}

fn uses_mass_overrides(install: &dyn Fn(&mut App)) -> Result<(), String> {
    let mut app = app_with(install);
    // Off the box's middle, like a plane whose balance point isn't its volume center.
    let com = Vec3::new(0.4, 0.0, -0.2);
    let inertia = Vec3::new(2.0, 3.0, 4.0);
    let start = DVec3::new(0.0, 10.0, 0.0);
    let body = app
        .world
        .spawn((
            Transform::from_translation(start),
            RigidBody {
                gravity_scale: 0.0,
                ..RigidBody::dynamic().with_mass_properties(12.0, com, inertia)
            },
            Collider::cuboid(Vec3::splat(0.5)),
            Velocity::default(),
        ))
        .id();
    app.update_n(3);
    let m = *app
        .world
        .get::<MassProperties>(body)
        .ok_or("no MassProperties on a dynamic body: write them back")?;
    near("reported mass", m.mass_kg as f64, 12.0, 1e-3)?;
    near(
        "reported center of mass offset from the override",
        (m.center_of_mass - com).length() as f64,
        0.0,
        1e-3,
    )?;
    near(
        "reported inertia offset from the override",
        (m.inertia - inertia).length() as f64,
        0.0,
        1e-3,
    )?;
    // A push through the overridden center of mass moves the body without turning it.
    let at = position(&app, body) + com.as_dvec3();
    app.world.write_message(ApplyImpulse {
        entity: body,
        impulse: Vec3::new(0.0, 0.0, 12.0),
        at: Some(at),
        angular: Vec3::ZERO,
    });
    app.update_n(10);
    let v = *app
        .world
        .get::<Velocity>(body)
        .ok_or("the body lost its Velocity")?;
    near(
        "speed after 12 N·s on 12 kg",
        v.linear.length() as f64,
        1.0,
        0.02,
    )?;
    near(
        "spin after a push through the center of mass",
        v.angular.length() as f64,
        0.0,
        0.01,
    )
}

fn raycast_hits_static_box(install: &dyn Fn(&mut App)) -> Result<(), String> {
    let mut app = app_with(install);
    let g = ground(&mut app, DVec3::ZERO);
    app.update_n(2);
    let q = app.world.resource::<PhysicsQueries>().clone();
    let hit = q
        .raycast(
            DVec3::new(1.0, 10.0, -2.0),
            Vec3::NEG_Y,
            20.0,
            &QueryFilter::default(),
        )
        .ok_or("raycast down onto the ground missed")?;
    if hit.entity != g {
        return Err(format!("hit {:?}, expected the ground {g:?}", hit.entity));
    }
    near("hit point y", hit.point.y, 0.5, 0.01)?;
    near("hit distance", hit.distance as f64, 9.5, 0.01)?;
    near("hit normal y", hit.normal.y as f64, 1.0, 0.01)?;
    if q.raycast(
        DVec3::new(1.0, 10.0, -2.0),
        Vec3::Y,
        20.0,
        &QueryFilter::default(),
    )
    .is_some()
    {
        return Err("raycast pointing away from everything hit something".into());
    }
    if q.raycast(
        DVec3::new(1.0, 10.0, -2.0),
        Vec3::NEG_Y,
        20.0,
        &QueryFilter::excluding(g),
    )
    .is_some()
    {
        return Err("QueryFilter::excluding was ignored".into());
    }
    Ok(())
}

fn impulse_changes_velocity(install: &dyn Fn(&mut App)) -> Result<(), String> {
    let mut app = app_with(install);
    let ball = app
        .world
        .spawn((
            Transform::from_xyz(0.0, 50.0, 0.0),
            RigidBody {
                gravity_scale: 0.0,
                ..RigidBody::dynamic().with_mass(2.0)
            },
            Collider::sphere(0.5),
            Velocity::default(),
        ))
        .id();
    app.update();
    app.world.write_message(ApplyImpulse {
        entity: ball,
        impulse: Vec3::new(4.0, 0.0, 0.0),
        at: None,
        angular: Vec3::ZERO,
    });
    app.update_n(3);
    near(
        "velocity after a 4 N·s impulse on 2 kg",
        velocity(&app, ball).x as f64,
        2.0,
        0.05,
    )
}

/// Mass of the ball the contact checks drop, kg.
const BALL_KG: f32 = 2.0;
/// How far the ball falls before it touches the ground, m.
const BALL_DROP_M: f64 = 1.5;
/// Steps the dropped ball gets to land, bounce and come to rest (3 s).
const SETTLE_STEPS: usize = 180;

/// Drop a [`BALL_KG`] ball (radius 0.5 m) from [`BALL_DROP_M`] above the ground and step until
/// it should rest there. Returns the app, the ground and the ball.
fn drop_ball(install: &dyn Fn(&mut App)) -> (App, Entity, Entity) {
    let mut app = app_with(install);
    let g = ground(&mut app, DVec3::ZERO);
    let ball = app
        .world
        .spawn((
            Transform::from_xyz(0.0, 0.5 + 0.5 + BALL_DROP_M, 0.0),
            RigidBody::dynamic().with_mass(BALL_KG),
            Collider::sphere(0.5),
            Velocity::default(),
        ))
        .id();
    app.update_n(SETTLE_STEPS);
    (app, g, ball)
}

/// Contact events seen so far.
fn seen_contacts(app: &App) -> &[ContactEvent] {
    &app.world.resource::<SeenContacts>().0
}

/// Events of `phase` between `x` and `y` (in either order) in `events`.
fn pair_events(
    events: &[ContactEvent],
    x: Entity,
    y: Entity,
    phase: ContactPhase,
) -> impl Iterator<Item = &ContactEvent> {
    events
        .iter()
        .filter(move |c| c.phase == phase && ((c.a == x && c.b == y) || (c.a == y && c.b == x)))
}

fn reports_contacts(install: &dyn Fn(&mut App)) -> Result<(), String> {
    let (mut app, g, ball) = drop_ball(install);
    let landed = seen_contacts(&app).len();
    app.update_n(60);
    let seen = seen_contacts(&app);
    let Some(first) = pair_events(seen, g, ball, ContactPhase::Started).next() else {
        return Err(format!(
            "no ContactEvent::Started between the falling ball and the ground ({} events seen)",
            seen.len()
        ));
    };

    let again = pair_events(&seen[landed..], g, ball, ContactPhase::Started).count();
    if again > 0 {
        return Err(format!(
            "the dropped ball should rest on the ground after 3 s, but {again} more \
             ContactEvent::Started for it arrived in the second after that; send Started once \
             when two colliders start touching, not every step they touch"
        ));
    }
    let started = pair_events(seen, g, ball, ContactPhase::Started).count();
    let ended = pair_events(seen, g, ball, ContactPhase::Ended).count();
    if started != ended + 1 {
        return Err(format!(
            "the ball now rests on the ground, so every Started but the last needs an Ended \
             (one per bounce), but there were {started} Started and {ended} Ended events"
        ));
    }

    let gravity = app
        .world
        .resource::<crate::Gravity>()
        .at(DVec3::ZERO)
        .length() as f64;
    let speed = (2.0 * gravity * BALL_DROP_M).sqrt();
    let momentum = BALL_KG as f64 * speed;
    let (low, high) = (0.5 * momentum, 2.5 * momentum);
    let impulse = first.impulse as f64;
    if !(low..=high).contains(&impulse) {
        return Err(format!(
            "the first Started event of a {BALL_KG} kg ball hitting the ground at {speed:.1} m/s \
             reports an impulse of {impulse:.2} N·s; expected about m·v = {momentum:.1} N·s \
             ({low:.1} to {high:.1} with restitution). `impulse` is the total normal impulse of \
             the contact in that step, in N·s"
        ));
    }

    // The ground is below the ball, so "from a toward b" is up when a is the ground.
    let (a, b, up) = if first.a == g {
        ("the ground", "the ball", 1.0)
    } else {
        ("the ball", "the ground", -1.0)
    };
    if first.normal.y * up < 0.5 {
        return Err(format!(
            "the first Started event has a = {a}, b = {b} and normal {:?}; the normal must point \
             from a toward b, which is {} here",
            first.normal,
            if up > 0.0 { "up (+Y)" } else { "down (-Y)" }
        ));
    }
    Ok(())
}

fn reports_contact_end(install: &dyn Fn(&mut App)) -> Result<(), String> {
    let (mut app, g, ball) = drop_ball(install);
    let seen = seen_contacts(&app);
    let started = pair_events(seen, g, ball, ContactPhase::Started).count();
    let ended = pair_events(seen, g, ball, ContactPhase::Ended).count();
    if started <= ended {
        return Err(format!(
            "the ball dropped onto the ground should rest on it after 3 s, but its contact \
             events say it is not touching ({started} Started, {ended} Ended), so the end of the \
             contact can't be checked"
        ));
    }

    let before = seen.len();
    if let Some(mut t) = app.world.get_mut::<Transform>(ball) {
        t.translation.y += 5.0;
    }
    if let Some(mut v) = app.world.get_mut::<Velocity>(ball) {
        *v = Velocity::default();
    }
    app.update_n(10);
    let after = &seen_contacts(&app)[before..];
    let ended = pair_events(after, g, ball, ContactPhase::Ended).count();
    let started = pair_events(after, g, ball, ContactPhase::Started).count();
    match ended {
        0 => Err(
            "the ball was lifted 5 m off the ground but no ContactEvent::Ended followed; send \
             Ended once when two colliders stop touching"
                .into(),
        ),
        1 if started > 0 => Err(format!(
            "the ball was lifted 5 m off the ground, touching nothing, but {started} \
             ContactEvent::Started for it and the ground followed"
        )),
        1 => Ok(()),
        n => Err(format!(
            "the ball was lifted 5 m off the ground and {n} ContactEvent::Ended followed; send \
             exactly one when two colliders stop touching"
        )),
    }
}

fn time_dilation_slows_bodies(install: &dyn Fn(&mut App)) -> Result<(), String> {
    let mut app = app_with(install);
    app.world
        .resource_mut::<TimeDilation>()
        .hold("conformance", 0.5, 0.0);
    let ball = app
        .world
        .spawn((
            Transform::from_xyz(0.0, 100.0, 0.0),
            RigidBody::dynamic(),
            Collider::sphere(0.5),
            Velocity::default(),
        ))
        .id();
    app.update_n(60);
    near(
        "fall speed after 1 real second at half time scale",
        velocity(&app, ball).y as f64,
        -4.905,
        0.4,
    )
}

fn bodies_honor_local_time_scale(install: &dyn Fn(&mut App)) -> Result<(), String> {
    let mut app = app_with(install);
    app.world
        .resource_mut::<TimeDilation>()
        .hold("conformance", 0.5, 0.0);
    let start = DVec3::new(0.0, 100.0, 0.0);
    let ball = |app: &mut App, x: f64, local: LocalTimeScale| {
        app.world
            .spawn((
                Transform::from_translation(start + DVec3::new(x, 0.0, 0.0)),
                RigidBody::dynamic(),
                Collider::sphere(0.5),
                Velocity::default(),
                local,
            ))
            .id()
    };
    // A hero who ignores the world's slow motion, and a box in a bubble at half of it.
    let hero = ball(&mut app, -5.0, LocalTimeScale::unaffected_by_dilation(1.0));
    let bubble = ball(
        &mut app,
        5.0,
        LocalTimeScale {
            factor: 0.5,
            ignore_world_dilation: false,
        },
    );
    app.update_n(60);
    let fell = |app: &App, e: Entity| start.y - position(app, e).y;
    near(
        "fall speed of a body with LocalTimeScale::unaffected_by_dilation(1.0) after 1 real \
         second while the world ran at 0.5 (advance each body by time.delta_for(its \
         LocalTimeScale); Velocity is in its own seconds)",
        velocity(&app, hero).y as f64,
        -9.81,
        0.4,
    )?;
    near("fall of that body", fell(&app, hero), 4.9, 0.4)?;
    near(
        "fall speed of a body with LocalTimeScale factor 0.5 after 1 real second while the \
         world ran at 0.5 (a quarter second of its own time)",
        velocity(&app, bubble).y as f64,
        -2.45,
        0.15,
    )?;
    near("fall of that body", fell(&app, bubble), 0.31, 0.06)
}

fn character_honors_local_time_scale(install: &dyn Fn(&mut App)) -> Result<(), String> {
    let mut app = app_with(install);
    ground(&mut app, DVec3::ZERO);
    app.world
        .resource_mut::<TimeDilation>()
        .hold("conformance", 0.25, 0.0);
    let walker = |app: &mut App, z: f64| {
        app.world
            .spawn((
                Transform::from_xyz(0.0, 0.5 + 0.9 + 0.2, z),
                CharacterController::default(),
                CharacterMove {
                    velocity: Vec3::new(2.0, -2.0, 0.0),
                },
                CharacterState::default(),
            ))
            .id()
    };
    let fast = walker(&mut app, -3.0);
    app.world
        .entity_mut(fast)
        .insert(LocalTimeScale::unaffected_by_dilation(1.0));
    let slowed = walker(&mut app, 3.0);
    app.update_n(60);
    near(
        "character with LocalTimeScale::unaffected_by_dilation(1.0) walked 1 real second at \
         2 m/s while the world ran at 0.25 (move it by velocity × time.delta_for(local scale))",
        position(&app, fast).x,
        2.0,
        0.25,
    )?;
    near(
        "character without a LocalTimeScale walked 1 real second at 2 m/s while the world ran \
         at 0.25",
        position(&app, slowed).x,
        0.5,
        0.1,
    )
}

fn character_walks_and_grounds(install: &dyn Fn(&mut App)) -> Result<(), String> {
    let mut app = app_with(install);
    ground(&mut app, DVec3::ZERO);
    let ch = app
        .world
        .spawn((
            Transform::from_xyz(0.0, 0.5 + 0.9 + 0.2, 0.0),
            CharacterController::default(),
            CharacterMove {
                velocity: Vec3::new(2.0, -2.0, 0.0),
            },
            CharacterState::default(),
        ))
        .id();
    app.update_n(60);
    let state = *app
        .world
        .get::<CharacterState>(ch)
        .ok_or("CharacterState missing")?;
    if !state.grounded {
        return Err("character pushed down onto the ground isn't grounded".into());
    }
    let p = position(&app, ch);
    near("character walked 1 s at 2 m/s", p.x, 2.0, 0.25)?;
    near(
        "character stands on the ground (center height)",
        p.y,
        0.5 + 0.9,
        0.1,
    )
}
