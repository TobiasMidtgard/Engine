//! The showcase content runs on its own: the arena fight keeps going (cover, flanking,
//! explosives, duel, revives) and the wildlife meadow lives (grazing, wandering, hunting)
//! without anything leaving its area.

use std::collections::HashSet;

use engine_core::prelude::*;
use engine_core::time::TimeSource;
use engine_npc::*;
use engine_world::WorldPlugin;

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

#[test]
fn the_arena_fight_keeps_going() {
    let mut app = app();
    let origin = DVec3::new(-70.0, 0.0, 170.0);
    {
        let mut commands = app.world.commands();
        demo::spawn_demo(&mut commands, origin);
    }
    app.world.flush();
    let mut npcs = app.world.query_filtered::<Entity, With<Npc>>();
    let fighters: Vec<Entity> = npcs.iter(&app.world).collect();
    assert_eq!(fighters.len(), 12);
    let mut actions = HashSet::new();
    let mut reasons = HashSet::new();
    let mut downed_seen = 0;
    for frame in 0..60 * 60 {
        app.update();
        if frame % 10 != 0 {
            continue;
        }
        for &e in &fighters {
            let b = app.world.get::<NpcBrain>(e).unwrap();
            actions.insert(b.action);
            if b.destination.is_some() {
                reasons.insert(b.destination_reason);
            }
            if app.world.get::<NpcVitals>(e).unwrap().downed {
                downed_seen += 1;
            }
            let p = app.world.get::<Transform>(e).unwrap().translation;
            assert!(p.is_finite(), "{e:?} at {p}");
            assert!(p.distance(origin) < 40.0, "{e:?} left the arena: {p}");
        }
    }
    let stats: Vec<_> = fighters
        .iter()
        .map(|&e| app.world.get::<NpcCombat>(e).unwrap().stats)
        .collect();
    let shots: u32 = stats.iter().map(|s| s.shots).sum();
    let attacks: u32 = stats.iter().map(|s| s.attacks).sum();
    let defended: u32 = stats.iter().map(|s| s.blocks + s.parries + s.dodges).sum();
    assert!(shots > 30, "shots {shots}");
    assert!(attacks > 40, "attacks {attacks}");
    assert!(defended > 5, "defended {defended}");
    assert!(downed_seen > 0, "nobody went down");
    assert!(actions.contains(&NpcAction::Engage), "{actions:?}");
    assert!(actions.contains(&NpcAction::TakeCover), "{actions:?}");
    assert!(reasons.len() >= 3, "{reasons:?}");
    // The fight never ends: downed fighters get back up.
    let alive = fighters
        .iter()
        .filter(|&&e| !app.world.get::<NpcVitals>(e).unwrap().downed)
        .count();
    assert!(alive >= 6, "only {alive} standing after a minute");
}

#[test]
fn the_wildlife_meadow_lives() {
    let mut app = app();
    let center = DVec3::new(-190.0, 0.0, -120.0);
    {
        let mut commands = app.world.commands();
        demo::spawn_wildlife(&mut commands, center, 60.0);
    }
    app.world.flush();
    let mut q = app.world.query_filtered::<Entity, With<Animal>>();
    let animals: Vec<Entity> = q.iter(&app.world).collect();
    assert!(animals.len() >= 30);
    let mut states = HashSet::new();
    // Hungry packs hunt sooner.
    app.world.resource_mut::<WildlifeSettings>().hunger_secs = 40.0;
    for frame in 0..60 * 120 {
        app.update();
        if frame % 30 != 0 {
            continue;
        }
        for &e in &animals {
            states.insert(app.world.get::<AnimalMind>(e).unwrap().state);
            let p = app.world.get::<Transform>(e).unwrap().translation;
            assert!(p.is_finite());
            assert!(p.distance(center) < 160.0, "{e:?} wandered off to {p}");
        }
    }
    for s in [
        AnimalState::Graze,
        AnimalState::Wander,
        AnimalState::Perched,
    ] {
        assert!(states.contains(&s), "{s:?} never seen: {states:?}");
    }
    // Hungry wolves go hunting within two minutes.
    assert!(
        states.contains(&AnimalState::Stalk) || states.contains(&AnimalState::Chase),
        "{states:?}"
    );
    assert!(states.contains(&AnimalState::Flee), "{states:?}");
}
