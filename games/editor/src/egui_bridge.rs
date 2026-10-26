//! egui inside the engine: input comes from `engine_input`'s messages, and the tessellated
//! output is drawn as a render overlay onto the tonemapped image, so the scene renders under
//! the panels at full quality.
//!
//! Frame flow: [`gather_input`] (Update, before the editor UI) turns this frame's input
//! messages into an [`egui::RawInput`]; the editor's exclusive UI system runs egui and stores
//! the result with [`EguiFrame::submit`]; [`EguiFeature`] uploads and draws it.

use engine::input::{InputEvent, KeyCode, MouseButton, TextInput};
use engine::prelude::*;
use engine::render::wgpu;
use engine::render::{FrameContext, GpuShared, RenderFeature, RenderSettings, ViewData};

/// The editor's egui context (cheap to clone; shared with the overlay).
#[derive(Resource, Clone, Default)]
pub struct Egui {
    pub ctx: egui::Context,
}

/// Input gathered for the next egui pass.
#[derive(Resource, Default)]
pub struct EguiInput {
    pub raw: egui::RawInput,
    modifiers: egui::Modifiers,
    pointer: Option<egui::Pos2>,
    /// Physical pixels per egui point.
    pub pixels_per_point: f32,
    /// Synthetic events queued by scripts (headless screenshot runs) for the next pass.
    pub injected: Vec<egui::Event>,
}

/// What the last egui pass drew, waiting for the overlay.
#[derive(Resource, Default)]
pub struct EguiFrame {
    primitives: Vec<egui::ClippedPrimitive>,
    textures: TextureChanges,
    pixels_per_point: f32,
}

/// Texture uploads and frees not yet applied (egui's own `TexturesDelta` asserts if it is
/// dropped unapplied, which a skipped or final frame would do).
#[derive(Default)]
struct TextureChanges {
    set: Vec<(egui::TextureId, egui::epaint::ImageDelta)>,
    free: Vec<egui::TextureId>,
}

impl TextureChanges {
    fn take_from(&mut self, mut delta: egui::TexturesDelta) {
        for (id, deltas) in delta.set.drain() {
            self.set.extend(deltas.into_iter().map(|d| (id, d)));
        }
        self.free.extend(delta.free.drain());
    }

    fn append(&mut self, mut other: TextureChanges) {
        self.set.append(&mut other.set);
        self.free.append(&mut other.free);
    }
}

impl EguiFrame {
    /// Tessellate a finished pass and keep it for drawing. Texture changes accumulate until the
    /// overlay uploads them, so a frame the renderer skips loses no glyphs.
    pub fn submit(&mut self, ctx: &egui::Context, output: egui::FullOutput) {
        self.pixels_per_point = output.pixels_per_point;
        self.primitives = ctx.tessellate(output.shapes, output.pixels_per_point);
        self.textures.take_from(output.textures_delta);
    }
}

fn map_key(key: KeyCode) -> Option<egui::Key> {
    use egui::Key as E;
    Some(match key {
        KeyCode::A => E::A,
        KeyCode::B => E::B,
        KeyCode::C => E::C,
        KeyCode::D => E::D,
        KeyCode::E => E::E,
        KeyCode::F => E::F,
        KeyCode::G => E::G,
        KeyCode::H => E::H,
        KeyCode::I => E::I,
        KeyCode::J => E::J,
        KeyCode::K => E::K,
        KeyCode::L => E::L,
        KeyCode::M => E::M,
        KeyCode::N => E::N,
        KeyCode::O => E::O,
        KeyCode::P => E::P,
        KeyCode::Q => E::Q,
        KeyCode::R => E::R,
        KeyCode::S => E::S,
        KeyCode::T => E::T,
        KeyCode::U => E::U,
        KeyCode::V => E::V,
        KeyCode::W => E::W,
        KeyCode::X => E::X,
        KeyCode::Y => E::Y,
        KeyCode::Z => E::Z,
        KeyCode::Digit0 => E::Num0,
        KeyCode::Digit1 => E::Num1,
        KeyCode::Digit2 => E::Num2,
        KeyCode::Digit3 => E::Num3,
        KeyCode::Digit4 => E::Num4,
        KeyCode::Digit5 => E::Num5,
        KeyCode::Digit6 => E::Num6,
        KeyCode::Digit7 => E::Num7,
        KeyCode::Digit8 => E::Num8,
        KeyCode::Digit9 => E::Num9,
        KeyCode::Escape => E::Escape,
        KeyCode::Tab => E::Tab,
        KeyCode::Space => E::Space,
        KeyCode::Enter | KeyCode::NumpadEnter => E::Enter,
        KeyCode::Backspace => E::Backspace,
        KeyCode::Delete => E::Delete,
        KeyCode::Insert => E::Insert,
        KeyCode::Home => E::Home,
        KeyCode::End => E::End,
        KeyCode::PageUp => E::PageUp,
        KeyCode::PageDown => E::PageDown,
        KeyCode::ArrowUp => E::ArrowUp,
        KeyCode::ArrowDown => E::ArrowDown,
        KeyCode::ArrowLeft => E::ArrowLeft,
        KeyCode::ArrowRight => E::ArrowRight,
        KeyCode::Minus | KeyCode::NumpadSubtract => E::Minus,
        KeyCode::Equal | KeyCode::NumpadAdd => E::Plus,
        KeyCode::F1 => E::F1,
        KeyCode::F2 => E::F2,
        KeyCode::F3 => E::F3,
        KeyCode::F4 => E::F4,
        KeyCode::F5 => E::F5,
        KeyCode::F6 => E::F6,
        KeyCode::F7 => E::F7,
        KeyCode::F8 => E::F8,
        KeyCode::F9 => E::F9,
        KeyCode::F10 => E::F10,
        KeyCode::F11 => E::F11,
        KeyCode::F12 => E::F12,
        _ => return None,
    })
}

/// Turn this frame's input messages into egui events (Update, before the UI pass).
pub fn gather_input(
    window: Res<engine::core::window::Window>,
    time: Res<Time>,
    mut input: ResMut<EguiInput>,
    mut events: MessageReader<InputEvent>,
    mut text: MessageReader<TextInput>,
) {
    let ppp = (window.scale_factor as f32).max(0.5);
    input.pixels_per_point = ppp;
    let size = egui::vec2(window.width as f32, window.height as f32) / ppp;
    let mut raw = egui::RawInput {
        screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, size)),
        time: Some(time.real_elapsed_secs_f64()),
        focused: window.focused,
        ..Default::default()
    };
    raw.viewports
        .entry(egui::ViewportId::ROOT)
        .or_default()
        .native_pixels_per_point = Some(ppp);
    for event in events.read() {
        match *event {
            InputEvent::Key { key, pressed } => {
                let m = &mut input.modifiers;
                match key {
                    KeyCode::ShiftLeft | KeyCode::ShiftRight => m.shift = pressed,
                    KeyCode::ControlLeft | KeyCode::ControlRight => {
                        m.ctrl = pressed;
                        m.command = pressed;
                    }
                    KeyCode::AltLeft | KeyCode::AltRight => m.alt = pressed,
                    _ => {}
                }
                if let Some(key) = map_key(key) {
                    let modifiers = input.modifiers;
                    raw.events.push(egui::Event::Key {
                        key,
                        physical_key: Some(key),
                        pressed,
                        repeat: false,
                        modifiers,
                    });
                    // Text editing shortcuts egui expects as their own events.
                    if pressed && modifiers.command {
                        match key {
                            egui::Key::C => raw.events.push(egui::Event::Copy),
                            egui::Key::X => raw.events.push(egui::Event::Cut),
                            _ => {}
                        }
                    }
                }
            }
            InputEvent::MouseButton { button, pressed } => {
                let button = match button {
                    MouseButton::Left => egui::PointerButton::Primary,
                    MouseButton::Right => egui::PointerButton::Secondary,
                    MouseButton::Middle => egui::PointerButton::Middle,
                    MouseButton::Back => egui::PointerButton::Extra1,
                    MouseButton::Forward => egui::PointerButton::Extra2,
                    MouseButton::Other(_) => continue,
                };
                if let Some(pos) = input.pointer {
                    raw.events.push(egui::Event::PointerButton {
                        pos,
                        button,
                        pressed,
                        modifiers: input.modifiers,
                    });
                }
            }
            InputEvent::CursorMoved { position } => {
                let pos = egui::pos2(position.x, position.y) / ppp;
                input.pointer = Some(pos);
                raw.events.push(egui::Event::PointerMoved(pos));
            }
            InputEvent::CursorLeft => {
                input.pointer = None;
                raw.events.push(egui::Event::PointerGone);
            }
            InputEvent::MouseWheel { delta } => raw.events.push(egui::Event::MouseWheel {
                unit: egui::MouseWheelUnit::Line,
                delta: egui::vec2(delta.x, delta.y),
                phase: egui::TouchPhase::Move,
                modifiers: input.modifiers,
            }),
            InputEvent::FocusLost => {
                input.modifiers = egui::Modifiers::NONE;
                raw.events.push(egui::Event::WindowFocused(false));
            }
            _ => {}
        }
    }
    for t in text.read() {
        let printable: String = t.0.chars().filter(|c| !c.is_control()).collect();
        if !printable.is_empty() && !input.modifiers.command {
            raw.events.push(egui::Event::Text(printable));
        }
    }
    let injected = std::mem::take(&mut input.injected);
    for event in &injected {
        if let egui::Event::PointerMoved(pos) = event {
            input.pointer = Some(*pos);
        }
    }
    raw.events.extend(injected);
    input.raw = raw;
}

/// Draws the egui output on top of the tonemapped image.
#[derive(Default)]
pub struct EguiFeature {
    renderer: Option<egui_wgpu::Renderer>,
    primitives: Vec<egui::ClippedPrimitive>,
    textures: TextureChanges,
    pixels_per_point: f32,
}

impl RenderFeature for EguiFeature {
    fn name(&self) -> &str {
        "editor ui"
    }

    fn order(&self) -> i32 {
        // After every other overlay (the engine's own UI, debug text).
        1_000
    }

    fn init(&mut self, gpu: &GpuShared) {
        self.renderer = Some(egui_wgpu::Renderer::new(
            &gpu.device,
            gpu.formats.ldr,
            egui_wgpu::RendererOptions::default(),
        ));
    }

    fn prepare(
        &mut self,
        world: &mut World,
        _gpu: &GpuShared,
        _view: &ViewData,
        _settings: &RenderSettings,
    ) {
        let Some(mut frame) = world.get_resource_mut::<EguiFrame>() else {
            return;
        };
        self.primitives = std::mem::take(&mut frame.primitives);
        self.textures.append(std::mem::take(&mut frame.textures));
        self.pixels_per_point = frame.pixels_per_point.max(0.5);
    }

    fn overlay(&mut self, encoder: &mut wgpu::CommandEncoder, ctx: &FrameContext) {
        let Some(renderer) = &mut self.renderer else {
            return;
        };
        let gpu = ctx.gpu;
        let textures = std::mem::take(&mut self.textures);
        for (id, delta) in &textures.set {
            renderer.update_texture(&gpu.device, &gpu.queue, *id, delta);
        }
        let screen = egui_wgpu::ScreenDescriptor {
            size_in_pixels: [ctx.view.window.x.max(1), ctx.view.window.y.max(1)],
            pixels_per_point: self.pixels_per_point,
        };
        if !self.primitives.is_empty() {
            let extra = renderer.update_buffers(
                &gpu.device,
                &gpu.queue,
                encoder,
                &self.primitives,
                &screen,
            );
            if !extra.is_empty() {
                gpu.queue.submit(extra);
            }
            let pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("editor ui"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: ctx.ldr,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Load,
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            renderer.render(&mut pass.forget_lifetime(), &self.primitives, &screen);
        }
        for id in &textures.free {
            renderer.free_texture(id);
        }
    }
}
