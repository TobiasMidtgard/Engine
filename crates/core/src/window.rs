//! Platform-neutral window state.
//!
//! The platform layer (`engine_platform`) owns the real OS window. It mirrors size and focus
//! into [`Window`] each frame and applies changes games make to it (title, cursor mode).
//! The renderer reads [`RawWindowHandle`] to create its surface. Neither side depends on the
//! other, so headless runs simply have no `RawWindowHandle`.

use std::sync::Arc;

use bevy_ecs::prelude::*;
use raw_window_handle::{HasDisplayHandle, HasWindowHandle};
use serde::{Deserialize, Serialize};

/// How the mouse cursor behaves.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum CursorMode {
    /// Visible and free.
    #[default]
    Normal,
    /// Hidden and locked to the window (first-person / third-person camera control).
    Locked,
    /// Hidden but free.
    Hidden,
}

/// The primary window as games see it. Edit `title` or `cursor_mode` to change the window.
#[derive(Resource, Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Window {
    pub title: String,
    /// Physical pixels.
    pub width: u32,
    pub height: u32,
    pub scale_factor: f64,
    pub focused: bool,
    pub cursor_mode: CursorMode,
    pub fullscreen: bool,
}

impl Default for Window {
    fn default() -> Self {
        Self {
            title: "Engine".into(),
            width: 1600,
            height: 900,
            scale_factor: 1.0,
            focused: true,
            cursor_mode: CursorMode::Normal,
            fullscreen: false,
        }
    }
}

impl Window {
    pub fn aspect(&self) -> f32 {
        self.width.max(1) as f32 / self.height.max(1) as f32
    }
}

/// Anything that can give the renderer a native window handle.
pub trait WindowHandleSource: HasWindowHandle + HasDisplayHandle + Send + Sync {}
impl<T: HasWindowHandle + HasDisplayHandle + Send + Sync> WindowHandleSource for T {}

/// Native handle of the primary window, inserted by the platform layer.
#[derive(Resource, Clone)]
pub struct RawWindowHandle(pub Arc<dyn WindowHandleSource>);

/// Sent when the window's physical size changes.
#[derive(Message, Debug, Clone, Copy)]
pub struct WindowResized {
    pub width: u32,
    pub height: u32,
}

/// Sent when the user asks to close the window.
#[derive(Message, Debug, Clone, Copy)]
pub struct WindowCloseRequested;
