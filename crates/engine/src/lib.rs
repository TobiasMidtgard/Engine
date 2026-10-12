//! # engine
//!
//! Games depend on this crate only:
//!
//! ```ignore
//! use engine::prelude::*;
//!
//! fn main() {
//!     App::new()
//!         .add_plugins(DefaultPlugins)
//!         .add_systems(Startup, setup)
//!         .run();
//! }
//! ```
//!
//! Each subsystem lives in its own crate (`engine_render`, `engine_input`, …) and is
//! re-exported here under a short name (`engine::render`, `engine::input`, …).

pub use bevy_ecs as ecs;
pub use engine_ai_bridge as ai_bridge;
pub use engine_assets as assets;
pub use engine_core as core;
pub use engine_debug as debug;
pub use engine_input as input;
pub use engine_platform as platform;
pub use engine_render as render;
pub use engine_world as world;

use engine_core::{App, PluginGroup, PluginGroupBuilder};

/// Everything a typical game needs: world simulation contracts, input, rendering, the window,
/// debug drawing and the AI bridge (off unless `ENGINE_BRIDGE` is set, see `engine_ai_bridge`).
/// Disable or replace parts with `.set(...)` / `.disable::<T>()`.
pub struct DefaultPlugins;

impl PluginGroup for DefaultPlugins {
    fn build(self) -> PluginGroupBuilder {
        PluginGroupBuilder::start()
            .add(engine_world::WorldPlugin)
            .add(engine_input::InputPlugin)
            .add(engine_render::RenderPlugin)
            .add(engine_platform::PlatformPlugin)
            .add(engine_debug::DebugPlugin)
            .add(engine_ai_bridge::AiBridgePlugin::default())
    }
}

/// Configure the app from command-line flags shared by every game and tool:
///
/// - `--headless`: no window; render offscreen.
/// - `--frames N`: stop after N frames (headless).
/// - `--size WxH`: window / render size.
/// - `--seed N`: world seed.
///
/// Call before adding plugins.
pub fn apply_cli_args(app: &mut App) {
    let args: Vec<String> = std::env::args().collect();
    let mut config = app
        .world
        .resource::<engine_core::config::EngineConfig>()
        .clone();
    let mut frames = None;
    let mut i = 1;
    while i < args.len() {
        match args[i].as_str() {
            "--headless" => config.headless = true,
            "--frames" => {
                frames = args.get(i + 1).and_then(|v| v.parse().ok());
                i += 1;
            }
            "--size" => {
                if let Some((w, h)) = args.get(i + 1).and_then(|v| v.split_once('x')) {
                    config.width = w.parse().unwrap_or(config.width);
                    config.height = h.parse().unwrap_or(config.height);
                }
                i += 1;
            }
            "--seed" => {
                config.seed = args
                    .get(i + 1)
                    .and_then(|v| v.parse().ok())
                    .unwrap_or(config.seed);
                i += 1;
            }
            _ => {}
        }
        i += 1;
    }
    {
        let mut w = app.world.resource_mut::<engine_core::window::Window>();
        w.width = config.width;
        w.height = config.height;
    }
    app.insert_resource(config);
    if frames.is_some() {
        app.insert_resource(engine_platform::HeadlessRun { max_frames: frames });
    }
}

pub mod prelude {
    pub use crate::DefaultPlugins;
    pub use engine_assets::{Asset, AssetAppExt, Assets, Handle};
    pub use engine_core::prelude::*;
    pub use engine_debug::DebugDraw;
    pub use engine_input::{ActionState, ButtonInput, InputMap, KeyCode, Mouse, MouseButton};
    pub use engine_render::prelude::*;
    pub use engine_world::{Env, Gravity, SurfaceMaterials, WeatherState, WindField};
}
