//! Schedule labels and system sets: the fixed order every frame runs in.
//!
//! ```text
//! Startup (once, before the first frame)
//! ┌ frame ───────────────────────────────────────────────────────────────┐
//! │ First           time + message bookkeeping (engine only)             │
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
#[derive(SystemSet, Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum SimSet {
    Input,
    Gameplay,
    Physics,
    PostPhysics,
    Environment,
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
