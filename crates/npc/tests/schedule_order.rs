//! The NPC simulation's outcome doesn't depend on the order the scheduler happens to pick:
//! no two systems in the fixed step or in `PostUpdate` touch the same data without an order
//! between them, and the arena fight and the meadow run with unrelated systems added end bit
//! for bit where the plain run does.
//!
//! Headless (no GPU).

use bevy_ecs::schedule::{LogLevel, ScheduleBuildSettings, ScheduleLabel};
use engine_core::prelude::*;
use engine_core::time::TimeSource;
use engine_npc::*;
use engine_world::{FollowsWorldShift, WorldPlugin, WorldShift, WorldShiftAppExt, WorldShiftSet};

/// `schedule` of an app with the world and the NPCs builds with no ambiguous pair of systems
/// (two systems with conflicting data access and no order between them).
fn assert_unambiguous(schedule: impl ScheduleLabel + Clone) {
    let mut app = App::new();
    app.add_plugins((WorldPlugin, NpcPlugin));
    app.finish();
    let mut s = app
        .world
        .resource_mut::<Schedules>()
        .remove(schedule.clone())
        .expect("schedule");
    s.set_build_settings(ScheduleBuildSettings {
        ambiguity_detection: LogLevel::Error,
        ..Default::default()
    });
    if let Err(e) = s.initialize(&mut app.world) {
        panic!(
            "{schedule:?}: the run order of these systems is up to the scheduler:\n{}",
            e.to_string(s.graph(), &app.world)
        );
    }
}

#[test]
fn every_npc_system_has_a_fixed_order() {
    assert_unambiguous(FixedUpdate);
    assert_unambiguous(PostUpdate);
}

/// A message type that follows world shifts, like the one #287 added to the world crate.
#[derive(Message, Clone, Copy)]
struct Ripple {
    at: DVec3,
}

impl FollowsWorldShift for Ripple {
    fn follow(&mut self, s: &WorldShift) {
        self.at = s.point(self.at);
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum Extra {
    /// Nothing: the run repeats.
    Nothing,
    /// An unrelated system in `WorldShiftSet::Apply`, where a shifted message type's system
    /// goes.
    ShiftSystem,
    /// An unrelated system among the gameplay systems of the fixed step.
    GameplaySystem,
    /// An unrelated system first in the fixed step's input.
    InputSystem,
    /// A new shifted message type, as #287 added: its system and its `Messages` resource
    /// (a resource is an entity, so every entity spawned later gets another id).
    ShiftedMessage,
    /// An unrelated entity spawned before the NPCs.
    Entity,
}

fn nothing() {}

/// Every fighter's and animal's position, facing, health and action after `secs` of the
/// arena fight and the meadow, in spawn order.
fn run(extra: Extra, secs: f64) -> Vec<String> {
    let mut app = App::new();
    app.world
        .resource_mut::<Time>()
        .set_source(TimeSource::Manual {
            step_secs: 1.0 / 60.0,
        });
    app.add_plugins((WorldPlugin, NpcPlugin));
    match extra {
        Extra::ShiftSystem => {
            app.add_systems(PostUpdate, nothing.in_set(WorldShiftSet::Apply));
        }
        Extra::GameplaySystem => {
            app.add_systems(FixedUpdate, nothing.in_set(SimSet::Gameplay));
        }
        Extra::InputSystem => {
            app.add_systems(FixedUpdate, nothing.in_set(SimSet::Input));
        }
        Extra::ShiftedMessage => {
            app.add_message::<Ripple>().shift_messages::<Ripple>();
        }
        Extra::Entity => {
            app.world.spawn(Transform::default());
        }
        Extra::Nothing => {}
    }
    {
        let mut commands = app.world.commands();
        demo::spawn_demo(&mut commands, DVec3::ZERO);
        demo::spawn_wildlife(&mut commands, DVec3::new(400.0, 0.0, 0.0), 45.0);
    }
    app.world.flush();
    let mut q = app
        .world
        .query_filtered::<Entity, Or<(With<Npc>, With<Animal>)>>();
    let bodies: Vec<Entity> = q.iter(&app.world).collect();
    app.update_n((secs * 60.0).round() as usize);
    bodies
        .iter()
        .enumerate()
        .map(|(i, &e)| {
            let t = app.world.get::<Transform>(e).unwrap();
            let v = app.world.get::<NpcVitals>(e).unwrap();
            let p = t.translation;
            let r = t.rotation;
            let doing = match (app.world.get::<NpcBrain>(e), app.world.get::<AnimalMind>(e)) {
                (Some(b), _) => format!("{:?}", b.action),
                (_, Some(m)) => format!("{:?}", m.state),
                _ => String::new(),
            };
            format!(
                "#{i} at {:x} {:x} {:x} turned {:x} {:x} {:x} {:x} health {:x} downed {} {doing}",
                p.x.to_bits(),
                p.y.to_bits(),
                p.z.to_bits(),
                r.x.to_bits(),
                r.y.to_bits(),
                r.z.to_bits(),
                r.w.to_bits(),
                v.health.to_bits(),
                v.downed,
            )
        })
        .collect()
}

/// Which of `extras` change the outcome, with the first difference of each.
fn changed_by(extras: &[Extra]) -> Vec<String> {
    let secs = 8.0;
    let plain = run(Extra::Nothing, secs);
    assert!(plain.len() > 20, "{} fighters and animals", plain.len());
    let mut changed = Vec::new();
    for &extra in extras {
        let other = run(extra, secs);
        let differ: Vec<_> = plain.iter().zip(&other).filter(|(a, b)| a != b).collect();
        if let Some((a, b)) = differ.first() {
            changed.push(format!(
                "{extra:?}: {} of {} differ, first:\n  {a}\n  {b}",
                differ.len(),
                plain.len()
            ));
        } else {
            eprintln!("{extra:?}: identical");
        }
    }
    changed
}

#[test]
fn unrelated_systems_change_nothing() {
    let changed = changed_by(&[
        Extra::Nothing,
        Extra::ShiftSystem,
        Extra::GameplaySystem,
        Extra::InputSystem,
    ]);
    assert!(changed.is_empty(), "{}", changed.join("\n"));
}

/// One more entity spawned before the bodies (any new resource is one, such as #287's
/// `Messages<WaterDisturbance>`) moves every later `Entity` id, so randomness drawn from ids
/// would change; the bodies' `Seed`s don't come from ids.
#[test]
fn unrelated_entities_change_nothing() {
    let changed = changed_by(&[Extra::ShiftedMessage, Extra::Entity]);
    assert!(changed.is_empty(), "{}", changed.join("\n"));
}
