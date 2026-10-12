//! Smoke and fog (`engine_world::Obscurance`): NPCs can't see through thick smoke, see poorly
//! through thin smoke, miss more when they shoot into it, and hide in it when there is no
//! solid cover.

use std::sync::Arc;

use engine_core::prelude::*;
use engine_core::time::TimeSource;
use engine_npc::*;
use engine_world::locomotion::LocomotionState;
use engine_world::{
    DamageEvent, Obscurance, ObscuranceQuery, ProjectileTrace, SimulationFocus, WorldPlugin,
};

/// A uniform ball of smoke.
struct Ball {
    center: DVec3,
    radius: f64,
    extinction: f32,
}

impl ObscuranceQuery for Ball {
    fn transmittance(&self, from: DVec3, to: DVec3) -> f32 {
        let d = to - from;
        let len = d.length();
        if len == 0.0 {
            return 1.0;
        }
        let dir = d / len;
        let oc = from - self.center;
        let b = oc.dot(dir);
        let disc = b * b - (oc.length_squared() - self.radius * self.radius);
        if disc <= 0.0 {
            return 1.0;
        }
        let (t0, t1) = (-b - disc.sqrt(), -b + disc.sqrt());
        let inside = (t1.min(len) - t0.max(0.0)).max(0.0);
        (-(self.extinction as f64) * inside).exp() as f32
    }

    fn extinction(&self, p: DVec3) -> f32 {
        if p.distance(self.center) < self.radius {
            self.extinction
        } else {
            0.0
        }
    }
}

fn app() -> App {
    let mut app = App::new();
    app.world
        .resource_mut::<Time>()
        .set_source(TimeSource::Manual {
            step_secs: 1.0 / 60.0,
        });
    app.add_plugins((WorldPlugin, NpcPlugin));
    app
}

fn smoke(app: &mut App, center: DVec3, radius: f64, extinction: f32) {
    app.world.resource_mut::<Obscurance>().set(
        "test smoke",
        Arc::new(Ball {
            center,
            radius,
            extinction,
        }),
    );
}

fn run(app: &mut App, secs: f32) {
    app.update_n((secs * 60.0) as usize);
}

fn shooter(app: &mut App, at: DVec3, skills: NpcSkills) -> Entity {
    app.world
        .spawn((
            Transform::from_translation(at),
            Npc {
                home: Some(at),
                ..Default::default()
            },
            Faction::ENEMY,
            skills,
            NpcWeapon::rifle(),
        ))
        .id()
}

fn player(app: &mut App, at: DVec3) -> Entity {
    app.world
        .spawn((
            Transform::from_translation(at),
            Faction::PLAYER,
            LocomotionState::default(),
            // NPCs near the focus look often (far ones only now and then).
            SimulationFocus::default(),
        ))
        .id()
}

fn awareness(app: &App, npc: Entity, of: Entity) -> f32 {
    app.world
        .get::<NpcMemory>(npc)
        .unwrap()
        .get(of)
        .map(|k| k.awareness)
        .unwrap_or(0.0)
}

#[test]
fn thick_smoke_hides_and_thin_smoke_slows_spotting() {
    // Thick smoke between them: never spotted.
    let mut world = app();
    let npc = shooter(&mut world, DVec3::ZERO, NpcSkills::veteran());
    world.world.get_mut::<Npc>(npc).unwrap().active = false;
    let p = player(&mut world, DVec3::new(0.0, 0.0, -16.0));
    smoke(&mut world, DVec3::new(0.0, 1.2, -8.0), 2.5, 2.0);
    run(&mut world, 3.0);
    assert_eq!(awareness(&world, npc, p), 0.0, "seen through thick smoke");
    // The smoke clears: spotted.
    world
        .world
        .resource_mut::<Obscurance>()
        .remove("test smoke");
    run(&mut world, 2.0);
    assert!(awareness(&world, npc, p) > 0.5);

    // Thin smoke: spotted, but more slowly than in clear air.
    let spot_time = |extinction: Option<f32>| {
        let mut app = app();
        let npc = shooter(&mut app, DVec3::ZERO, NpcSkills::soldier());
        app.world.get_mut::<Npc>(npc).unwrap().active = false;
        let p = player(&mut app, DVec3::new(0.0, 0.0, -22.0));
        if let Some(e) = extinction {
            smoke(&mut app, DVec3::new(0.0, 1.2, -11.0), 2.5, e);
        }
        for frame in 0..60 * 10 {
            app.update();
            if awareness(&app, npc, p) >= 0.99 {
                return frame;
            }
        }
        60 * 10
    };
    let clear = spot_time(None);
    let thin = spot_time(Some(0.12));
    assert!(clear < 60 * 4, "{clear}");
    assert!(thin > clear + 20, "thin smoke {thin} vs clear {clear}");
}

#[test]
fn shots_into_smoke_miss_more() {
    let hits = |extinction: Option<f32>| {
        let mut app = app();
        let npc = shooter(&mut app, DVec3::ZERO, NpcSkills::elite());
        app.world.get_mut::<NpcSkills>(npc).unwrap().courage = 1.0;
        let target = app
            .world
            .spawn((
                Transform::from_translation(DVec3::new(0.0, 0.0, -24.0)),
                Npc {
                    active: false,
                    ..Default::default()
                },
                Faction::PLAYER,
                NpcSkills::rookie(),
                NpcWeapon::rifle(),
            ))
            .id();
        app.world.get_mut::<NpcVitals>(target).unwrap().max_health = 1.0e6;
        app.world.get_mut::<NpcVitals>(target).unwrap().health = 1.0e6;
        if let Some(e) = extinction {
            smoke(&mut app, DVec3::new(0.0, 1.3, -12.0), 3.0, e);
        }
        let mut hits = 0;
        let mut cursor = app
            .world
            .resource::<engine_core::ecs::message::Messages<DamageEvent>>()
            .get_cursor();
        for _ in 0..60 * 20 {
            app.update();
            let msgs = app
                .world
                .resource::<engine_core::ecs::message::Messages<DamageEvent>>();
            hits += cursor.read(msgs).filter(|d| d.target == target).count();
        }
        let shots = app.world.get::<NpcCombat>(npc).unwrap().stats.shots;
        (hits as f32 / shots.max(1) as f32, shots)
    };
    let (clear, clear_shots) = hits(None);
    let (smoky, smoky_shots) = hits(Some(0.1));
    assert!(
        clear_shots > 10 && smoky_shots > 5,
        "{clear_shots} {smoky_shots}"
    );
    assert!(
        smoky < clear * 0.8,
        "hit rate in smoke {smoky:.2} vs clear {clear:.2}"
    );
}

#[test]
fn shooters_hide_in_smoke_when_there_is_no_cover() {
    let mut app = app();
    let npc = shooter(&mut app, DVec3::ZERO, NpcSkills::veteran());
    let enemy = player(&mut app, DVec3::new(0.0, 0.0, -20.0));
    // A smoke bank 5 m to the side, nothing solid anywhere.
    let cloud = DVec3::new(6.0, 1.0, -3.0);
    smoke(&mut app, cloud, 3.0, 1.5);
    run(&mut app, 1.0);
    for _ in 0..40 {
        app.world.write_message(ProjectileTrace {
            from: DVec3::new(0.0, 1.5, -20.0),
            to: app.world.get::<Transform>(npc).unwrap().translation + DVec3::new(0.4, 1.2, 0.0),
            radius: 0.05,
            speed: 600.0,
            shooter: Some(enemy),
        });
        run(&mut app, 0.2);
    }
    let at = app.world.get::<Transform>(npc).unwrap().translation;
    let seen = app
        .world
        .resource::<Obscurance>()
        .transmittance(DVec3::new(0.0, 1.6, -20.0), at + DVec3::Y * 0.95);
    let brain = app.world.get::<NpcBrain>(npc).unwrap();
    assert!(
        seen < 0.25,
        "still in plain view at {at} (transmittance {seen}) doing {:?} toward {:?}",
        brain.action,
        brain.destination
    );
}
