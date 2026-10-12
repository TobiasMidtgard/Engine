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
//! events, raycasts, time dilation, precision far from the origin and the character
//! controller.

use engine_core::math::{DVec3, Vec3};
use engine_core::prelude::*;
use engine_core::time::TimeSource;

use super::*;
use crate::WorldPlugin;

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
    ("reports_contacts", reports_contacts),
    ("time_dilation_slows_bodies", time_dilation_slows_bodies),
    ("precise_far_from_origin", precise_far_from_origin),
    ("character_walks_and_grounds", character_walks_and_grounds),
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

fn reports_contacts(install: &dyn Fn(&mut App)) -> Result<(), String> {
    let mut app = app_with(install);
    let g = ground(&mut app, DVec3::ZERO);
    let ball = app
        .world
        .spawn((
            Transform::from_xyz(0.0, 2.0, 0.0),
            RigidBody::dynamic(),
            Collider::sphere(0.5),
            Velocity::default(),
        ))
        .id();
    app.update_n(90);
    let seen = &app.world.resource::<SeenContacts>().0;
    let started = seen.iter().any(|c| {
        c.phase == ContactPhase::Started && ((c.a == g && c.b == ball) || (c.a == ball && c.b == g))
    });
    if started {
        Ok(())
    } else {
        Err(format!(
            "no ContactEvent::Started between the falling ball and the ground ({} events seen)",
            seen.len()
        ))
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
