//! # engine_platform
//!
//! Opens the OS window (winit), turns OS events into [`InputEvent`]s and window state, and
//! drives the frame loop. With `EngineConfig::headless` it runs without a window instead
//! (servers, tests, AI tools); the renderer then draws offscreen.
//!
//! The window is created when the OS says the app may draw (`resumed`), which on mobile can
//! happen more than once; the renderer starts on the first frame after it exists.

use std::sync::Arc;

use bevy_ecs::prelude::*;
use engine_core::prelude::*;
use engine_core::window::{RawWindowHandle, WindowCloseRequested, WindowResized};
use engine_input::{FileDrop, InputEvent, KeyCode, MouseButton, TextInput};
use winit::application::ApplicationHandler;
use winit::dpi::PhysicalSize;
use winit::event::{DeviceEvent, DeviceId, ElementState, MouseScrollDelta, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop};
use winit::keyboard::PhysicalKey;
use winit::window::{CursorGrabMode, Fullscreen, WindowId};

/// How the headless runner paces frames.
#[derive(Resource, Debug, Clone, Copy, Default)]
pub struct HeadlessRun {
    /// Stop after this many frames (`None` = until `AppExit`).
    pub max_frames: Option<u64>,
}

/// Window + event loop (or the headless loop).
pub struct PlatformPlugin;

impl Plugin for PlatformPlugin {
    fn build(&self, app: &mut App) {
        let (title, width, height, headless) = {
            let c = app.world.resource::<EngineConfig>();
            (c.app_name.clone(), c.width, c.height, c.headless)
        };
        {
            let mut w = app.world.resource_mut::<Window>();
            w.title = title;
            w.width = width;
            w.height = height;
        }
        app.init_resource::<HeadlessRun>();
        if headless {
            app.set_runner(headless_runner);
        } else {
            app.set_runner(winit_runner);
        }
    }
}

/// Runs frames until `AppExit` (or `HeadlessRun::max_frames`).
pub fn headless_runner(mut app: App) -> AppExit {
    let max = app.world.resource::<HeadlessRun>().max_frames;
    let mut frames = 0u64;
    loop {
        app.update();
        frames += 1;
        if let Some(exit) = app.should_exit() {
            return exit;
        }
        if max.is_some_and(|m| frames >= m) {
            return AppExit::Success;
        }
    }
}

/// Keep Windows from swapping the window for a frozen "Not responding" ghost when a frame (a
/// loading frame building a zone) runs longer than 5 s without pumping messages. Unreal keeps
/// its loading screen alive on a thread of its own for the same reason; until every module's
/// first-use build runs off the frame thread, this keeps the real window on screen.
#[cfg(windows)]
fn disable_ghosting() {
    #[link(name = "user32")]
    unsafe extern "system" {
        fn DisableProcessWindowsGhosting();
    }
    // SAFETY: takes no arguments and only sets a flag for this process.
    unsafe { DisableProcessWindowsGhosting() }
}

#[cfg(not(windows))]
fn disable_ghosting() {}

/// Runs the app inside the winit event loop.
pub fn winit_runner(app: App) -> AppExit {
    let event_loop = match EventLoop::new() {
        Ok(l) => l,
        Err(e) => {
            tracing::error!("could not start the window system ({e}); running headless");
            let mut app = app;
            app.world.resource_mut::<EngineConfig>().headless = true;
            return headless_runner(app);
        }
    };
    event_loop.set_control_flow(ControlFlow::Poll);
    disable_ghosting();
    let mut handler = Handler {
        app,
        window: None,
        exit: None,
        applied: AppliedWindow::default(),
        shown: false,
    };
    if let Err(e) = event_loop.run_app(&mut handler) {
        tracing::error!("event loop error: {e}");
        return AppExit::Error(1);
    }
    handler.exit.unwrap_or(AppExit::Success)
}

#[derive(Default, Clone, PartialEq)]
struct AppliedWindow {
    title: String,
    cursor_mode: Option<CursorMode>,
    fullscreen: bool,
}

struct Handler {
    app: App,
    window: Option<Arc<winit::window::Window>>,
    exit: Option<AppExit>,
    applied: AppliedWindow,
    /// The window opens hidden and shows once the first frame has run (and presented), so
    /// the player never sees the OS's blank white client area while the GPU starts up.
    shown: bool,
}

impl Handler {
    fn send(&mut self, e: InputEvent) {
        self.app.world.write_message(e);
    }

    /// Push game-side window changes (title, cursor, fullscreen) to the OS window.
    fn apply_window_changes(&mut self) {
        let Some(window) = &self.window else { return };
        let w = self.app.world.resource::<Window>().clone();
        if w.title != self.applied.title {
            window.set_title(&w.title);
            self.applied.title = w.title.clone();
        }
        if self.applied.cursor_mode != Some(w.cursor_mode) {
            match w.cursor_mode {
                CursorMode::Normal => {
                    let _ = window.set_cursor_grab(CursorGrabMode::None);
                    window.set_cursor_visible(true);
                }
                CursorMode::Hidden => {
                    let _ = window.set_cursor_grab(CursorGrabMode::None);
                    window.set_cursor_visible(false);
                }
                CursorMode::Locked => {
                    if window.set_cursor_grab(CursorGrabMode::Locked).is_err() {
                        let _ = window.set_cursor_grab(CursorGrabMode::Confined);
                    }
                    window.set_cursor_visible(false);
                }
            }
            self.applied.cursor_mode = Some(w.cursor_mode);
        }
        if w.fullscreen != self.applied.fullscreen {
            window.set_fullscreen(w.fullscreen.then_some(Fullscreen::Borderless(None)));
            self.applied.fullscreen = w.fullscreen;
        }
    }
}

impl ApplicationHandler for Handler {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.window.is_some() {
            return;
        }
        let w = self.app.world.resource::<Window>().clone();
        let attributes = winit::window::Window::default_attributes()
            .with_title(w.title.clone())
            .with_inner_size(PhysicalSize::new(w.width.max(64), w.height.max(64)))
            .with_visible(false);
        match event_loop.create_window(attributes) {
            Ok(window) => {
                let window = Arc::new(window);
                let size = window.inner_size();
                {
                    let mut res = self.app.world.resource_mut::<Window>();
                    res.width = size.width.max(1);
                    res.height = size.height.max(1);
                    res.scale_factor = window.scale_factor();
                }
                self.app
                    .world
                    .insert_resource(RawWindowHandle(window.clone()));
                self.applied.title = w.title;
                self.window = Some(window);
            }
            Err(e) => {
                tracing::error!("could not create the window: {e}");
                self.exit = Some(AppExit::Error(1));
                event_loop.exit();
            }
        }
    }

    fn window_event(&mut self, event_loop: &ActiveEventLoop, _id: WindowId, event: WindowEvent) {
        match event {
            WindowEvent::CloseRequested => {
                self.app.world.write_message(WindowCloseRequested);
                self.exit = Some(AppExit::Success);
                event_loop.exit();
            }
            WindowEvent::Resized(size) => {
                let (width, height) = (size.width.max(1), size.height.max(1));
                {
                    let mut w = self.app.world.resource_mut::<Window>();
                    w.width = width;
                    w.height = height;
                }
                self.app
                    .world
                    .write_message(WindowResized { width, height });
            }
            WindowEvent::ScaleFactorChanged { scale_factor, .. } => {
                self.app.world.resource_mut::<Window>().scale_factor = scale_factor;
            }
            WindowEvent::Focused(focused) => {
                self.app.world.resource_mut::<Window>().focused = focused;
                if !focused {
                    self.send(InputEvent::FocusLost);
                }
            }
            WindowEvent::KeyboardInput { event, .. } => {
                if let PhysicalKey::Code(code) = event.physical_key
                    && !event.repeat
                {
                    let key = map_key(code);
                    self.send(InputEvent::Key {
                        key,
                        pressed: event.state == ElementState::Pressed,
                    });
                }
                if event.state == ElementState::Pressed
                    && let Some(text) = &event.text
                {
                    self.app.world.write_message(TextInput(text.to_string()));
                }
            }
            WindowEvent::MouseInput { state, button, .. } => {
                let button = match button {
                    winit::event::MouseButton::Left => MouseButton::Left,
                    winit::event::MouseButton::Right => MouseButton::Right,
                    winit::event::MouseButton::Middle => MouseButton::Middle,
                    winit::event::MouseButton::Back => MouseButton::Back,
                    winit::event::MouseButton::Forward => MouseButton::Forward,
                    winit::event::MouseButton::Other(n) => MouseButton::Other(n),
                };
                self.send(InputEvent::MouseButton {
                    button,
                    pressed: state == ElementState::Pressed,
                });
            }
            WindowEvent::CursorMoved { position, .. } => {
                self.send(InputEvent::CursorMoved {
                    position: Vec2::new(position.x as f32, position.y as f32),
                });
            }
            WindowEvent::CursorLeft { .. } => self.send(InputEvent::CursorLeft),
            WindowEvent::MouseWheel { delta, .. } => {
                let delta = match delta {
                    MouseScrollDelta::LineDelta(x, y) => Vec2::new(x, y),
                    MouseScrollDelta::PixelDelta(p) => Vec2::new(p.x as f32, p.y as f32) / 40.0,
                };
                self.send(InputEvent::MouseWheel { delta });
            }
            WindowEvent::HoveredFile(path) => {
                self.app.world.write_message(FileDrop::Hovered(path));
            }
            WindowEvent::DroppedFile(path) => {
                self.app.world.write_message(FileDrop::Dropped(path));
            }
            WindowEvent::HoveredFileCancelled => {
                self.app.world.write_message(FileDrop::Cancelled);
            }
            WindowEvent::RedrawRequested => {}
            _ => {}
        }
    }

    fn device_event(&mut self, _event_loop: &ActiveEventLoop, _id: DeviceId, event: DeviceEvent) {
        if let DeviceEvent::MouseMotion { delta } = event {
            self.send(InputEvent::MouseMotion {
                delta: Vec2::new(delta.0 as f32, delta.1 as f32),
            });
        }
    }

    fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
        if self.window.is_none() {
            return;
        }
        self.app.update();
        if !self.shown
            && let Some(window) = &self.window
        {
            window.set_visible(true);
            self.shown = true;
        }
        self.apply_window_changes();
        if let Some(exit) = self.app.should_exit() {
            self.exit = Some(exit);
            event_loop.exit();
        }
    }
}

fn map_key(code: winit::keyboard::KeyCode) -> KeyCode {
    use winit::keyboard::KeyCode as W;
    match code {
        W::KeyA => KeyCode::A,
        W::KeyB => KeyCode::B,
        W::KeyC => KeyCode::C,
        W::KeyD => KeyCode::D,
        W::KeyE => KeyCode::E,
        W::KeyF => KeyCode::F,
        W::KeyG => KeyCode::G,
        W::KeyH => KeyCode::H,
        W::KeyI => KeyCode::I,
        W::KeyJ => KeyCode::J,
        W::KeyK => KeyCode::K,
        W::KeyL => KeyCode::L,
        W::KeyM => KeyCode::M,
        W::KeyN => KeyCode::N,
        W::KeyO => KeyCode::O,
        W::KeyP => KeyCode::P,
        W::KeyQ => KeyCode::Q,
        W::KeyR => KeyCode::R,
        W::KeyS => KeyCode::S,
        W::KeyT => KeyCode::T,
        W::KeyU => KeyCode::U,
        W::KeyV => KeyCode::V,
        W::KeyW => KeyCode::W,
        W::KeyX => KeyCode::X,
        W::KeyY => KeyCode::Y,
        W::KeyZ => KeyCode::Z,
        W::Digit0 => KeyCode::Digit0,
        W::Digit1 => KeyCode::Digit1,
        W::Digit2 => KeyCode::Digit2,
        W::Digit3 => KeyCode::Digit3,
        W::Digit4 => KeyCode::Digit4,
        W::Digit5 => KeyCode::Digit5,
        W::Digit6 => KeyCode::Digit6,
        W::Digit7 => KeyCode::Digit7,
        W::Digit8 => KeyCode::Digit8,
        W::Digit9 => KeyCode::Digit9,
        W::F1 => KeyCode::F1,
        W::F2 => KeyCode::F2,
        W::F3 => KeyCode::F3,
        W::F4 => KeyCode::F4,
        W::F5 => KeyCode::F5,
        W::F6 => KeyCode::F6,
        W::F7 => KeyCode::F7,
        W::F8 => KeyCode::F8,
        W::F9 => KeyCode::F9,
        W::F10 => KeyCode::F10,
        W::F11 => KeyCode::F11,
        W::F12 => KeyCode::F12,
        W::Escape => KeyCode::Escape,
        W::Tab => KeyCode::Tab,
        W::CapsLock => KeyCode::CapsLock,
        W::ShiftLeft => KeyCode::ShiftLeft,
        W::ShiftRight => KeyCode::ShiftRight,
        W::ControlLeft => KeyCode::ControlLeft,
        W::ControlRight => KeyCode::ControlRight,
        W::AltLeft => KeyCode::AltLeft,
        W::AltRight => KeyCode::AltRight,
        W::SuperLeft => KeyCode::SuperLeft,
        W::SuperRight => KeyCode::SuperRight,
        W::Space => KeyCode::Space,
        W::Enter => KeyCode::Enter,
        W::Backspace => KeyCode::Backspace,
        W::Delete => KeyCode::Delete,
        W::Insert => KeyCode::Insert,
        W::Home => KeyCode::Home,
        W::End => KeyCode::End,
        W::PageUp => KeyCode::PageUp,
        W::PageDown => KeyCode::PageDown,
        W::ArrowUp => KeyCode::ArrowUp,
        W::ArrowDown => KeyCode::ArrowDown,
        W::ArrowLeft => KeyCode::ArrowLeft,
        W::ArrowRight => KeyCode::ArrowRight,
        W::Minus => KeyCode::Minus,
        W::Equal => KeyCode::Equal,
        W::BracketLeft => KeyCode::BracketLeft,
        W::BracketRight => KeyCode::BracketRight,
        W::Backslash => KeyCode::Backslash,
        W::Semicolon => KeyCode::Semicolon,
        W::Quote => KeyCode::Quote,
        W::Backquote => KeyCode::Backquote,
        W::Comma => KeyCode::Comma,
        W::Period => KeyCode::Period,
        W::Slash => KeyCode::Slash,
        W::Numpad0 => KeyCode::Numpad0,
        W::Numpad1 => KeyCode::Numpad1,
        W::Numpad2 => KeyCode::Numpad2,
        W::Numpad3 => KeyCode::Numpad3,
        W::Numpad4 => KeyCode::Numpad4,
        W::Numpad5 => KeyCode::Numpad5,
        W::Numpad6 => KeyCode::Numpad6,
        W::Numpad7 => KeyCode::Numpad7,
        W::Numpad8 => KeyCode::Numpad8,
        W::Numpad9 => KeyCode::Numpad9,
        W::NumpadAdd => KeyCode::NumpadAdd,
        W::NumpadSubtract => KeyCode::NumpadSubtract,
        W::NumpadMultiply => KeyCode::NumpadMultiply,
        W::NumpadDivide => KeyCode::NumpadDivide,
        W::NumpadEnter => KeyCode::NumpadEnter,
        W::NumpadDecimal => KeyCode::NumpadDecimal,
        W::PrintScreen => KeyCode::PrintScreen,
        W::Pause => KeyCode::Pause,
        other => KeyCode::Other(other as u32),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn headless_runner_stops_after_max_frames() {
        let mut app = App::new();
        app.world.resource_mut::<EngineConfig>().headless = true;
        app.add_plugins((engine_input::InputPlugin, PlatformPlugin));
        app.insert_resource(HeadlessRun {
            max_frames: Some(5),
        });
        assert_eq!(app.run(), AppExit::Success);
    }
}
