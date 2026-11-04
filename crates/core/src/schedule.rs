//! Schedule labels and system sets: the fixed order every frame runs in.
//!
//! ```text
//! Startup (once, before the first frame)
//! ┌ frame ───────────────────────────────────────────────────────────────┐
//! │ First           time, messages, off-frame applies (engine only)      │
//! │ PreUpdate       platform events → input state, AI bridge requests    │
//! │ ┌ fixed loop: 0..N times per frame, at a fixed REAL-time rate ─────┐ │
//! │ │ FixedPreUpdate   snapshot transforms for interpolation           │ │
//! │ │ FixedUpdate      simulation: gameplay, physics, world sims       │ │
//! │ │ FixedPostUpdate  post-physics reactions                          │ │
//! │ └───────────────────────────────────────────────────────────────────┘ │
//! │ Update          per-frame gameplay, cameras, UI logic                 │
//! │ PostUpdate      transform propagation (with interpolation)           │
//! │ Extract         copy what the renderer needs out of the world        │
//! │ Render          GPU work (renderer-owned exclusive systems)           │
//! │ Last            end-of-frame cleanup                                  │
//! └──────────────────────────────────────────────────────────────────────┘
//! ```
//!
//! Inside `FixedUpdate`, use [`SimSet`] to order work relative to physics.

use bevy_ecs::schedule::{ScheduleLabel, SystemSet};

/// Runs once before the first frame.
#[derive(ScheduleLabel, Clone, Debug, PartialEq, Eq, Hash)]
pub struct Startup;
/// Engine bookkeeping at the very start of a frame (time, messages).
#[derive(ScheduleLabel, Clone, Debug, PartialEq, Eq, Hash)]
pub struct First;
/// Input and external requests are processed here.
#[derive(ScheduleLabel, Clone, Debug, PartialEq, Eq, Hash)]
pub struct PreUpdate;
/// Start of each fixed step.
#[derive(ScheduleLabel, Clone, Debug, PartialEq, Eq, Hash)]
pub struct FixedPreUpdate;
/// The simulation step. Runs at [`crate::time::FixedTime::hz`] in real time.
#[derive(ScheduleLabel, Clone, Debug, PartialEq, Eq, Hash)]
pub struct FixedUpdate;
/// End of each fixed step.
#[derive(ScheduleLabel, Clone, Debug, PartialEq, Eq, Hash)]
pub struct FixedPostUpdate;
/// Per-frame gameplay.
#[derive(ScheduleLabel, Clone, Debug, PartialEq, Eq, Hash)]
pub struct Update;
/// Transform propagation and other late, per-frame work.
#[derive(ScheduleLabel, Clone, Debug, PartialEq, Eq, Hash)]
pub struct PostUpdate;
/// Gather render data from the world.
#[derive(ScheduleLabel, Clone, Debug, PartialEq, Eq, Hash)]
pub struct Extract;
/// GPU work.
#[derive(ScheduleLabel, Clone, Debug, PartialEq, Eq, Hash)]
pub struct Render;
/// End of the frame.
#[derive(ScheduleLabel, Clone, Debug, PartialEq, Eq, Hash)]
pub struct Last;

/// Ordering inside [`FixedUpdate`]. Sets run in declaration order.
///
/// * `Input`: turn input/AI intents into forces and desired velocities.
/// * `Gameplay`: game rules, abilities, AI decisions.
/// * `Physics`: rigid bodies, character controllers, vehicles (owned by `engine_physics`).
/// * `PostPhysics`: reactions to contacts, damage, triggers.
/// * `Environment`: world simulations (weather, fire, smoke, water, ecology) that read the
///   settled state of the step.
/// * `DuringPhysics`: work that may run at the same time as `Physics`, as Unreal's
///   `TG_DuringPhysics` tick group does. It runs after `Gameplay` and finishes before
///   `PostPhysics`, but is not ordered against `Physics`. Put a system here only if it doesn't
///   need this step's bodies: it must not query physics (raycasts, overlaps, body poses) or
///   write anything physics reads. A field step that uses last step's state (wind, clouds,
///   smoke, water flow) fits; reactions to contacts belong in `PostPhysics`.
#[derive(SystemSet, Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum SimSet {
    Input,
    Gameplay,
    Physics,
    PostPhysics,
    Environment,
    DuringPhysics,
}

/// Ordering inside [`Update`] and [`PostUpdate`].
///
/// * `Logic` (Update): gameplay that runs per frame.
/// * `Camera` (Update): cameras follow their targets after logic has moved them.
/// * `Propagate` (PostUpdate): `Transform` → `GlobalTransform`.
/// * `AfterPropagate` (PostUpdate): anything that needs final world transforms.
#[derive(SystemSet, Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum UpdateSet {
    Logic,
    Camera,
    Propagate,
    AfterPropagate,
}

/// The `Last` system that moves [`Metrics`](crate::diagnostics::Metrics) into
/// [`Diagnostics`](crate::diagnostics::Diagnostics). Anything in `Last` that reads the
/// diagnostics for this frame (profile reports, the AI bridge) runs after it.
#[derive(SystemSet, Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct MetricsSet;

/// The second half of `due.pipe(queue_when(work))`: a plain system that checks for work (`due`,
/// any read-only system returning `bool`) and, on the steps that have some, queues the
/// exclusive `work` as a command, so it runs at the next sync point that is already there.
///
/// Use it in place of `work.run_if(due)` for an exclusive system: Bevy's executor only starts
/// an exclusive system once nothing else runs, and it checks that *before* the run condition,
/// so `run_if` still waits for an idle pool every step, holding up everything ordered after it
/// (bevy_ecs 0.20 multi_threaded.rs `can_run`; engine-perf/fixed-step-chain.md). Order other
/// systems after the piped system through a [`SystemSet`] it is in, as the command's changes
/// land only at the sync point Bevy puts after it.
pub fn queue_when(
    work: fn(&mut bevy_ecs::world::World),
) -> impl FnMut(bevy_ecs::system::In<bool>, bevy_ecs::system::Commands) {
    move |bevy_ecs::system::In(due), mut commands| {
        if due {
            commands.queue(work);
        }
    }
}

#[cfg(test)]
mod queue_when_tests {
    use super::*;
    use bevy_ecs::prelude::*;

    #[derive(Resource, Default)]
    struct Work {
        due: bool,
        ran: u32,
        seen_after: u32,
    }

    #[derive(SystemSet, Clone, Debug, PartialEq, Eq, Hash)]
    struct Queued;

    fn has_work(w: Res<Work>) -> bool {
        w.due
    }

    fn work(world: &mut World) {
        let mut w = world.resource_mut::<Work>();
        w.ran += 1;
        w.due = false;
    }

    fn after(mut w: ResMut<Work>) {
        w.seen_after = w.ran;
    }

    #[test]
    fn queued_work_runs_on_due_steps_and_lands_before_what_is_ordered_after() {
        let mut world = World::new();
        world.init_resource::<Work>();
        let mut schedule = bevy_ecs::schedule::Schedule::default();
        schedule.add_systems((
            has_work.pipe(queue_when(work)).in_set(Queued),
            after.after(Queued),
        ));
        schedule.run(&mut world);
        assert_eq!(world.resource::<Work>().ran, 0);
        world.resource_mut::<Work>().due = true;
        schedule.run(&mut world);
        let w = world.resource::<Work>();
        assert_eq!((w.ran, w.seen_after), (1, 1));
        schedule.run(&mut world);
        assert_eq!(world.resource::<Work>().ran, 1);
    }
}
