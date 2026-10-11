//! # engine_core
//!
//! The foundation every other engine crate builds on. It owns the contracts that the
//! rest of the engine (and every game) agrees on:
//!
//! * [`App`] and [`Plugin`]: how features are packaged and added.
//! * [`schedule`]: the fixed order a frame runs in (`First → PreUpdate → FixedUpdate×N →
//!   Update → PostUpdate → Extract → Render → Last`).
//! * [`time`]: real time, world time, fixed steps, and **time dilation** (global slow motion
//!   plus per-entity time scales, so a player can move at full speed through a slowed world).
//! * [`transform`]: large-world transforms with `f64` positions (planet and solar-system scale)
//!   and hierarchy propagation.
//! * [`registry`]: reflection by name over serde/JSON, used by scenes, the AI bridge, scripting
//!   and debug tools. Also the [`registry::ApiRegistry`] of named engine functions.
//! * [`scene`]: JSON scene/prefab documents that spawn entities through the registry.
//! * [`window`]: platform-neutral window state shared by the platform layer and the renderer.
//!
//! Games normally import everything through `engine::prelude::*` rather than this crate.

pub mod app;
pub mod config;
pub mod diagnostics;
pub mod log;
pub mod math;
pub mod registry;
pub mod scene;
pub mod schedule;
pub mod tasks;
pub mod time;
pub mod transform;
pub mod window;

pub use app::{App, AppExit, Plugin, PluginGroup, PluginGroupBuilder, Plugins};
pub use bevy_ecs as ecs;
pub use glam;

/// Everything a typical game or engine module needs, in one import.
pub mod prelude {
    pub use crate::app::{App, AppExit, Plugin, PluginGroup, PluginGroupBuilder};
    pub use crate::config::EngineConfig;
    pub use crate::diagnostics::Diagnostics;
    pub use crate::math::*;
    pub use crate::registry::{ApiRegistry, TypeRegistry};
    pub use crate::scene::{EntityDoc, SceneDoc};
    pub use crate::schedule::{
        Extract, First, FixedPostUpdate, FixedPreUpdate, FixedUpdate, Last, PostUpdate,
        PreUpdate, Render, SimSet, Startup, Update, UpdateSet,
    };
    pub use crate::time::{FixedTime, LocalTimeScale, Time, TimeDilation};
    pub use crate::transform::{GlobalTransform, Transform, TransformInterpolation};
    pub use crate::window::{CursorMode, Window};
    pub use bevy_ecs::prelude::*;
    pub use bevy_ecs::system::SystemParam;
    pub use serde::{Deserialize, Serialize};
}
