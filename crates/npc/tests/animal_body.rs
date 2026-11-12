//! Animals ask the animation module for their bodies (`animal_body`): deer, wolves, boars and
//! rabbits a `BodyRequest` of their kind with their seed, starlings keep the stand-in. Once a
//! body is answered the stand-in and its coat are hidden, and they come back when it goes. The
//! body is told what the animal does: `graze`, `lie`, and the `flinch` and `rear` triggers.

use engine_assets::Assets;
use engine_core::ecs::message::{MessageCursor, Messages};
use engine_core::prelude::*;
use engine_core::time::TimeSource;
use engine_npc::visuals::StandInPart;
use engine_npc::*;
use engine_render::{Image, Mesh, MeshRenderer};
use engine_world::animation::{AnimationParams, AnimationTrigger, BodyRequest, SkeletonPose};
use engine_world::locomotion::{LocomotionMode, LocomotionState};
use engine_world::{DamageEvent, DamageKind, WorldPlugin};

const STEP: f64 = 1.0 / 60.0;

fn app() -> App {
    let mut app = App::new();
    app.world.resource_mut::<EngineConfig>().headless = true;
    app.world
        .resource_mut::<Time>()
        .set_source(TimeSource::Manual { step_secs: STEP });
    app.add_plugins((WorldPlugin, NpcPlugin));
    for (_, schedule) in app.world.resource_mut::<Schedules>().iter_mut() {
        schedule.set_executor(engine_core::ecs::schedule::SingleThreadedExecutor::new());
    }
    app
}

fn animal(app: &mut App, species: Species, x: f64) -> Entity {
    app.world
        .spawn((
            Transform::from_translation(DVec3::new(x, 0.0, 0.0)),
            Animal::new(species),
            Herd(x as u32 + 100),
        ))
        .id()
}

fn run(app: &mut App, secs: f64) {
    app.update_n((secs / STEP).ceil() as usize);
}

fn cues(app: &App, cursor: &mut MessageCursor<AnimationTrigger>) -> Vec<(Entity, String)> {
    let msgs = app.world.resource::<Messages<AnimationTrigger>>();
    cursor
        .read(msgs)
        .map(|t| (t.entity, t.name.clone()))
        .collect()
}

fn parts_shown(app: &mut App, e: Entity) -> (usize, usize) {
    let children: Vec<Entity> = app
        .world
        .get::<Children>(e)
        .map(|c| c.iter().collect())
        .unwrap_or_default();
    let mut shown = 0;
    let mut parts = 0;
    for c in children {
        if app.world.get::<StandInPart>(c).is_some() {
            parts += 1;
            shown += usize::from(app.world.get::<MeshRenderer>(c).unwrap().visible);
        }
    }
    (shown, parts)
}

#[test]
fn deer_wolves_boars_and_rabbits_ask_for_bodies_and_birds_keep_the_stand_in() {
    let mut app = app();
    let all: Vec<(Species, Entity)> = Species::ALL
        .iter()
        .enumerate()
        .map(|(i, &s)| (s, animal(&mut app, s, i as f64 * 20.0)))
        .collect();
    run(&mut app, 0.2);
    for &(s, e) in &all {
        let asked = app.world.get::<BodyRequest>(e);
        match animal_body::kind_of(s) {
            Some(kind) => {
                let r = asked.unwrap_or_else(|| panic!("{s:?} asks"));
                assert_eq!(r.kind, kind);
                assert_eq!(r.seed, app.world.get::<Seed>(e).unwrap().0, "its own seed");
            }
            None => assert!(asked.is_none(), "{s:?} keeps the stand-in"),
        }
    }
    // Off: the bodies go back.
    app.world.resource_mut::<NpcSettings>().anim_bodies = false;
    run(&mut app, 0.1);
    for &(s, e) in &all {
        assert!(app.world.get::<BodyRequest>(e).is_none(), "{s:?}");
    }
}

#[test]
fn an_answered_animal_hides_its_stand_in_until_the_body_goes() {
    let mut app = app();
    app.world.insert_resource(Assets::<Mesh>::default());
    app.world.insert_resource(Assets::<Image>::default());
    let deer = animal(&mut app, Species::Deer, 0.0);
    app.world.entity_mut(deer).insert(StandIn::default());
    // Its sculpted art is made on a worker: wait for it.
    for _ in 0..2_000 {
        app.update();
        if parts_shown(&mut app, deer).1 > 0 {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    run(&mut app, 0.2);
    let (shown, parts) = parts_shown(&mut app, deer);
    assert!(
        parts > 5 && shown > 5,
        "the stand-in is drawn ({shown} of {parts})"
    );
    app.world.entity_mut(deer).insert(SkeletonPose::default());
    run(&mut app, 0.2);
    assert_eq!(
        parts_shown(&mut app, deer).0,
        0,
        "the module's body is drawn instead"
    );
    app.world.entity_mut(deer).remove::<SkeletonPose>();
    run(&mut app, 0.2);
    assert!(parts_shown(&mut app, deer).0 > 5, "the stand-in is back");
}

#[test]
fn the_body_is_told_when_the_animal_grazes_and_lies_down() {
    let mut app = app();
    let deer: Vec<Entity> = (0..4)
        .map(|i| animal(&mut app, Species::Deer, i as f64 * 3.0))
        .collect();
    let mut grazed = false;
    for _ in 0..(30.0 / STEP) as usize {
        app.update();
        for &e in &deer {
            let p = app.world.get::<AnimationParams>(e).unwrap();
            let resting = app.world.get::<AnimalMind>(e).unwrap().state == AnimalState::Rest;
            assert_eq!(p.get("lie"), if resting { 1.0 } else { 0.0 });
            grazed |= p.get("graze") > 0.5;
        }
    }
    assert!(grazed, "a deer put its head down to graze");
}

#[test]
fn a_hurt_animal_flinches_and_a_killed_one_is_downed() {
    let mut app = app();
    let deer = animal(&mut app, Species::Deer, 0.0);
    run(&mut app, 0.5);
    let mut cursor = app
        .world
        .resource::<Messages<AnimationTrigger>>()
        .get_cursor();
    let _ = cues(&app, &mut cursor);
    let hurt = |app: &mut App, amount: f32, kind: DamageKind| {
        app.world.write_message(DamageEvent {
            target: deer,
            amount,
            kind,
            point: DVec3::Y,
            impulse: Vec3::X,
            source: None,
        });
        app.update();
    };
    let flinched = |app: &App, cursor: &mut MessageCursor<AnimationTrigger>| {
        cues(app, cursor)
            .iter()
            .filter(|(e, n)| *e == deer && n == "flinch")
            .count()
    };
    hurt(&mut app, 5.0, DamageKind::Slash);
    assert_eq!(flinched(&app, &mut cursor), 1);
    hurt(&mut app, 1.0, DamageKind::Bleed);
    assert_eq!(flinched(&app, &mut cursor), 0, "a bleed is no blow");
    hurt(&mut app, 10_000.0, DamageKind::Slash);
    assert_eq!(flinched(&app, &mut cursor), 0, "killed, it falls instead");
    assert_eq!(
        app.world.get::<AnimalMind>(deer).unwrap().state,
        AnimalState::Dead
    );
    app.update();
    let down = |app: &App| {
        app.world
            .get::<AnimationParams>(deer)
            .unwrap()
            .get("downed")
    };
    assert_eq!(down(&app), 1.0, "its body is told it is down");
    // Back from the dead (respawned): up again.
    app.world.resource_mut::<WildlifeSettings>().respawn_secs = 0.5;
    run(&mut app, 1.0);
    assert_ne!(
        app.world.get::<AnimalMind>(deer).unwrap().state,
        AnimalState::Dead
    );
    assert_eq!(down(&app), 0.0);
}

#[test]
fn a_deer_startled_close_rears_as_it_bolts() {
    let mut app = app();
    let deer: Vec<Entity> = (0..3)
        .map(|i| animal(&mut app, Species::Deer, i as f64 * 2.0))
        .collect();
    run(&mut app, 1.0);
    let mut cursor = app
        .world
        .resource::<Messages<AnimationTrigger>>()
        .get_cursor();
    let _ = cues(&app, &mut cursor);
    // Someone bursts out of cover a few meters off, sprinting at them.
    let from = DVec3::new(2.0, 0.0, -5.0);
    let player = app
        .world
        .spawn((
            Transform::from_translation(from),
            Faction::PLAYER,
            LocomotionState {
                mode: LocomotionMode::Sprint,
                velocity: Vec3::Z * 7.0,
                ..Default::default()
            },
        ))
        .id();
    let mut reared = 0;
    for _ in 0..(3.0 / STEP) as usize {
        app.world
            .get_mut::<Transform>(player)
            .unwrap()
            .translation
            .z += 7.0 * STEP;
        app.update();
        reared += cues(&app, &mut cursor)
            .iter()
            .filter(|(e, n)| deer.contains(e) && n == "rear")
            .count();
    }
    assert!(
        deer.iter()
            .all(|&e| app.world.get::<AnimalMind>(e).unwrap().state == AnimalState::Flee),
        "they bolt"
    );
    assert!(reared >= 1, "a deer rears");
}
