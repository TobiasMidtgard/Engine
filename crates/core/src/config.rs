//! Engine-wide configuration chosen at startup.

use bevy_ecs::prelude::*;
use serde::{Deserialize, Serialize};

/// Startup configuration. Insert your own before adding `DefaultPlugins` to override it.
#[derive(Resource, Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct EngineConfig {
    /// Shown in the window title and logs.
    pub app_name: String,
    /// Run without a window (servers, tests, AI capture). Rendering still works offscreen.
    pub headless: bool,
    /// Window or offscreen size in physical pixels.
    pub width: u32,
    pub height: u32,
    pub vsync: bool,
    /// Folder that relative asset paths resolve against.
    pub asset_root: String,
    /// Seed for procedural generation, so worlds are reproducible.
    pub seed: u64,
}

impl Default for EngineConfig {
    fn default() -> Self {
        Self {
            app_name: "Engine".into(),
            headless: false,
            width: 1600,
            height: 900,
            vsync: true,
            asset_root: "assets".into(),
            seed: 0x5EED,
        }
    }
}
