//! [`PerfOverlay`]: an on-screen performance panel, toggled with **F3**.
//!
//! It shows frames per second, frame time and a graph of the last [`HISTORY`] frames, the GPU
//! time of each render pass and the CPU time of each render step (from the renderer's
//! `render.gpu.*` / `render.cpu.*` diagnostics), GPU memory, and, when the game was started
//! with `--profile`, the slowest systems of the last second ([`ProfileSnapshot`]).
//!
//! Text uses the public-domain 8×8 bitmap font from `font8x8`, drawn from a tiny atlas as
//! screen-space quads onto the final image after every other overlay, so the panel looks the
//! same in any scene and costs a few thousand vertices.

use std::collections::VecDeque;

use bevy_ecs::prelude::*;
use bytemuck::{Pod, Zeroable};
use engine_core::diagnostics::Diagnostics;
use engine_core::profile::ProfileSnapshot;
use engine_core::time::Time;
use engine_input::{Binding, ButtonInput, InputMap, KeyCode};
use engine_render::{
    FeatureStats, FrameContext, GpuShared, GrowBuffer, RenderFeature, RenderSettings, ViewData,
};
use engine_world::{CellState, InteractTarget, StreamedCells, WindField, WindOverride};
use serde::{Deserialize, Serialize};

/// Shader source (standalone, no frame bindings).
pub const SHADER: &str = include_str!("shaders/perf_overlay.wgsl");

/// Frames kept for the frame-time graph.
pub const HISTORY: usize = 240;

/// The performance panel's switch. Toggled with F3; also settable through the registry
/// (`PerfOverlay`), so an AI agent can turn it on before taking a screenshot.
#[derive(Resource, Debug, Clone, Copy, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct PerfOverlay {
    pub enabled: bool,
    /// Text size in pixels per font pixel; 0 picks one from the window height.
    pub scale: u32,
}

/// Real frame times in milliseconds, oldest first.
#[derive(Resource, Debug, Clone, Default)]
pub struct FrameHistory {
    pub frames_ms: VecDeque<f32>,
}

impl FrameHistory {
    pub fn push(&mut self, ms: f32) {
        if self.frames_ms.len() >= HISTORY {
            self.frames_ms.pop_front();
        }
        self.frames_ms.push_back(ms);
    }

    /// Mean and worst of the kept frames.
    pub fn summary(&self) -> (f32, f32) {
        if self.frames_ms.is_empty() {
            return (0.0, 0.0);
        }
        let sum: f32 = self.frames_ms.iter().sum();
        let worst = self.frames_ms.iter().copied().fold(0.0, f32::max);
        (sum / self.frames_ms.len() as f32, worst)
    }
}

/// `First`: F3 toggles the panel; every frame's real time goes into [`FrameHistory`]. While
/// the panel is open, PageUp / PageDown raise and lower the wind a step ([`WindOverride`]) and
/// Home gives it back to the weather.
pub fn perf_overlay_system(
    time: Res<Time>,
    keys: Option<Res<ButtonInput<KeyCode>>>,
    wind: Option<Res<WindField>>,
    test_wind: Option<ResMut<WindOverride>>,
    mut overlay: ResMut<PerfOverlay>,
    mut history: ResMut<FrameHistory>,
) {
    if let Some(keys) = keys {
        if keys.just_pressed(KeyCode::F3) {
            overlay.enabled = !overlay.enabled;
        }
        if overlay.enabled
            && let Some(mut test_wind) = test_wind
        {
            let now = test_wind
                .speed_m_s
                .or(wind.map(|w| w.prevailing.length()))
                .unwrap_or(0.0);
            if keys.just_pressed(KeyCode::PageUp) {
                test_wind.speed_m_s = Some(WindOverride::step(now, true));
            } else if keys.just_pressed(KeyCode::PageDown) {
                test_wind.speed_m_s = Some(WindOverride::step(now, false));
            } else if keys.just_pressed(KeyCode::Home) && test_wind.speed_m_s.is_some() {
                test_wind.speed_m_s = None;
            }
        }
    }
    let dt = time.real_delta_secs();
    if dt > 0.0 {
        history.push(dt * 1000.0);
    }
}

/// What the panel shows, gathered from the world each frame it is on.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct PanelData {
    pub frames_ms: Vec<f32>,
    pub gpu_ms: Option<f64>,
    pub gpu_memory_mb: Option<f64>,
    /// The process's resident memory, MB.
    pub ram_mb: Option<f64>,
    /// Streamed cells: (loaded, queued, all).
    pub cells: Option<(usize, usize, usize)>,
    /// `(pass, ms)`, slowest first.
    pub gpu_passes: Vec<(String, f64)>,
    /// `(step, ms)`, slowest first.
    pub cpu_steps: Vec<(String, f64)>,
    /// `(system, ms per frame)`, slowest first; empty without `--profile`.
    pub systems: Vec<(String, f64)>,
    /// Average cores kept busy by systems (profiling only).
    pub cores: Option<f64>,
    pub profiling: bool,
    /// Prevailing wind, m/s, and whether a [`WindOverride`] sets it.
    pub wind: Option<(f32, bool)>,
}

impl PanelData {
    pub fn gather(world: &World) -> Self {
        let history = world.get_resource::<FrameHistory>();
        let mut data = PanelData {
            frames_ms: history
                .map(|h| h.frames_ms.iter().copied().collect())
                .unwrap_or_default(),
            profiling: engine_core::profile::enabled(),
            wind: world.get_resource::<WindField>().map(|w| {
                let set = world
                    .get_resource::<WindOverride>()
                    .is_some_and(|t| t.speed_m_s.is_some());
                (w.prevailing.length(), set)
            }),
            cells: world
                .get_resource::<StreamedCells>()
                .filter(|c| !c.cells.is_empty())
                .map(|c| {
                    let loaded = c.cells.iter().filter(|c| c.state == CellState::Loaded);
                    (loaded.count(), c.waking().count(), c.cells.len())
                }),
            ..Default::default()
        };
        if let Some(diag) = world.get_resource::<Diagnostics>() {
            data.gpu_ms = diag.get("render.gpu_ms");
            data.gpu_memory_mb = diag.get("render.gpu_memory_mb");
            data.ram_mb = diag.get(engine_core::diagnostics::PROCESS_RAM_MB);
            for (name, value) in &diag.metrics {
                if let Some(pass) = name.strip_prefix("render.gpu.") {
                    data.gpu_passes.push((pass.to_string(), *value));
                } else if let Some(step) = name.strip_prefix("render.cpu.") {
                    data.cpu_steps.push((step.to_string(), *value));
                }
            }
        }
        let slowest_first = |a: &(String, f64), b: &(String, f64)| b.1.total_cmp(&a.1);
        data.gpu_passes.sort_by(slowest_first);
        data.cpu_steps.sort_by(slowest_first);
        if let Some(snap) = world.get_resource::<ProfileSnapshot>()
            && snap.frame_ms > 0.0
        {
            data.systems = snap.systems.clone();
            data.cores = Some(snap.busy_ms / snap.frame_ms);
        }
        data
    }
}

/// One overlay vertex: clip-space position, atlas coordinates and display color.
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Pod, Zeroable)]
pub struct OverlayVertex {
    pub position: [f32; 2],
    pub uv: [f32; 2],
    pub color: [f32; 4],
}

impl OverlayVertex {
    const ATTRIBUTES: [wgpu::VertexAttribute; 3] =
        wgpu::vertex_attr_array![0 => Float32x2, 1 => Float32x2, 2 => Float32x4];

    pub fn layout() -> wgpu::VertexBufferLayout<'static> {
        wgpu::VertexBufferLayout {
            array_stride: std::mem::size_of::<Self>() as u64,
            step_mode: wgpu::VertexStepMode::Vertex,
            attributes: &Self::ATTRIBUTES,
        }
    }
}

/// Glyph atlas: 16 × 8 cells of 8 × 8 texels, ASCII 0..128 (cell 0 is solid, for rectangles).
pub const ATLAS_SIZE: [u32; 2] = [128, 64];

/// The atlas texels (R8, 255 = ink).
pub fn atlas_texels() -> Vec<u8> {
    let [w, h] = ATLAS_SIZE;
    let mut texels = vec![0u8; (w * h) as usize];
    for (code, rows) in font8x8::legacy::BASIC_LEGACY.iter().enumerate() {
        let (cx, cy) = ((code % 16) as u32 * 8, (code / 16) as u32 * 8);
        for (y, row) in rows.iter().enumerate() {
            for x in 0..8 {
                // Bit 0 is the leftmost pixel; cell 0 is all ink.
                if code == 0 || row & (1 << x) != 0 {
                    texels[((cy + y as u32) * w + cx + x) as usize] = 255;
                }
            }
        }
    }
    texels
}

const PANEL: [f32; 4] = [0.0, 0.0, 0.0, 0.85];
const TEXT: [f32; 4] = [0.92, 0.94, 0.96, 1.0];
const DIM: [f32; 4] = [0.6, 0.64, 0.7, 1.0];
const HEAD: [f32; 4] = [0.55, 0.8, 1.0, 1.0];
const GOOD: [f32; 4] = [0.3, 0.85, 0.4, 1.0];
const OKAY: [f32; 4] = [0.95, 0.8, 0.25, 1.0];
const SLOW: [f32; 4] = [0.95, 0.3, 0.25, 1.0];
const GUIDE: [f32; 4] = [1.0, 1.0, 1.0, 0.25];

/// Frame time in ms the graph's full height stands for.
const GRAPH_MS: f32 = 50.0;
/// Rows listed per table.
const ROWS: usize = 8;
const SYSTEM_ROWS: usize = 12;
/// Characters per panel line.
const COLUMNS: usize = 64;

fn time_color(ms: f32) -> [f32; 4] {
    if ms <= 1000.0 / 60.0 + 0.5 {
        GOOD
    } else if ms <= 1000.0 / 30.0 + 0.5 {
        OKAY
    } else {
        SLOW
    }
}

/// Builds the quads in pixels, then converts them to clip space.
struct Painter {
    out: Vec<OverlayVertex>,
    width: f32,
    height: f32,
    scale: f32,
}

impl Painter {
    fn quad(&mut self, x: f32, y: f32, w: f32, h: f32, uv: [f32; 4], color: [f32; 4]) {
        let to_clip =
            |px: f32, py: f32| [px / self.width * 2.0 - 1.0, 1.0 - py / self.height * 2.0];
        let (x1, y1) = (x + w, y + h);
        let [u0, v0, u1, v1] = uv;
        let corners = [
            (to_clip(x, y), [u0, v0]),
            (to_clip(x1, y), [u1, v0]),
            (to_clip(x1, y1), [u1, v1]),
            (to_clip(x, y1), [u0, v1]),
        ];
        for i in [0, 1, 2, 0, 2, 3] {
            let (position, uv) = corners[i];
            self.out.push(OverlayVertex {
                position,
                uv,
                color,
            });
        }
    }

    fn rect(&mut self, x: f32, y: f32, w: f32, h: f32, color: [f32; 4]) {
        // The middle of the solid cell 0.
        let (u, v) = (4.0 / ATLAS_SIZE[0] as f32, 4.0 / ATLAS_SIZE[1] as f32);
        self.quad(x, y, w, h, [u, v, u, v], color);
    }

    fn text(&mut self, x: f32, y: f32, text: &str, color: [f32; 4]) {
        let g = 8.0 * self.scale;
        for (i, ch) in text.chars().enumerate() {
            let code = if ch.is_ascii() && ch != '\0' {
                ch as u32
            } else {
                '?' as u32
            };
            if code == ' ' as u32 {
                continue;
            }
            let (cx, cy) = ((code % 16) as f32 * 8.0, (code / 16) as f32 * 8.0);
            let [aw, ah] = ATLAS_SIZE.map(|s| s as f32);
            let uv = [cx / aw, cy / ah, (cx + 8.0) / aw, (cy + 8.0) / ah];
            self.quad(x + i as f32 * g, y, g, g, uv, color);
        }
    }
}

/// The prompt for what the player would use ([`InteractTarget`]): `E  Drive`, with the key
/// bound to `interact`. Until a UI draws prompts, the panel's font does (windowed runs only).
fn interact_prompt(world: &World) -> Option<String> {
    // Not in headless runs: tours and gallery stills stay clean.
    if world
        .get_resource::<engine_core::config::EngineConfig>()
        .is_some_and(|c| c.headless)
    {
        return None;
    }
    let target = world.get_resource::<InteractTarget>()?;
    target.target?;
    let key = world
        .get_resource::<InputMap>()
        .and_then(|m| m.actions.get("interact"))
        .and_then(|a| {
            a.buttons.iter().find_map(|b| match b {
                Binding::Key(k) => Some(format!("{k:?}")),
                _ => None,
            })
        })
        .unwrap_or_else(|| "E".into());
    Some(format!("{key}  {}", target.verb))
}

/// A one-line prompt in a dark box, centered a little below the middle of the screen.
pub fn build_prompt(text: &str, width: u32, height: u32, scale: u32) -> Vec<OverlayVertex> {
    let mut p = Painter {
        out: Vec::new(),
        width: width.max(1) as f32,
        height: height.max(1) as f32,
        scale: scale.max(1) as f32,
    };
    let s = p.scale;
    let pad = 4.0 * s;
    let w = text.chars().count() as f32 * 8.0 * s + 2.0 * pad;
    let h = 8.0 * s + 2.0 * pad;
    let x = ((p.width - w) * 0.5).max(0.0);
    let y = (p.height * 0.62).min(p.height - h);
    p.rect(x, y, w, h, PANEL);
    p.text(x + pad, y + pad, text, TEXT);
    p.out
}

fn clip(s: &str, n: usize) -> String {
    if s.chars().count() <= n {
        s.to_string()
    } else {
        let mut cut: String = s.chars().take(n.saturating_sub(1)).collect();
        cut.push('~');
        cut
    }
}

/// Text size for a window `height` pixels tall: 1× up to ~1000 px, 2× at 1080p and 1440p,
/// 3× at 4K.
pub fn auto_scale(height: u32) -> u32 {
    ((height as f32 / 700.0).round() as u32).max(1)
}

/// Lay the panel out for a `width` × `height` image. Empty when there is nothing to draw.
pub fn build_panel(data: &PanelData, width: u32, height: u32, scale: u32) -> Vec<OverlayVertex> {
    let mut p = Painter {
        out: Vec::new(),
        width: width.max(1) as f32,
        height: height.max(1) as f32,
        scale: scale.max(1) as f32,
    };
    let s = p.scale;
    let line = 10.0 * s;
    let margin = 6.0 * s;
    let panel_w = COLUMNS as f32 * 8.0 * s + 2.0 * margin;

    let mut lines: Vec<(String, [f32; 4])> = Vec::new();
    let (mean, worst) = {
        let n = data.frames_ms.len().max(1) as f32;
        let sum: f32 = data.frames_ms.iter().sum();
        (sum / n, data.frames_ms.iter().copied().fold(0.0, f32::max))
    };
    let fps = if mean > 0.0 { 1000.0 / mean } else { 0.0 };
    lines.push((
        format!("{fps:6.1} fps {mean:7.2} ms  worst {worst:7.2} ms"),
        time_color(mean),
    ));
    let mut second = String::new();
    if let Some(gpu) = data.gpu_ms {
        second.push_str(&format!("GPU {gpu:6.2} ms"));
    }
    let cpu: f64 = data.cpu_steps.iter().map(|(_, ms)| ms).sum();
    if !data.cpu_steps.is_empty() {
        second.push_str(&format!("  render CPU {cpu:6.2} ms"));
    }
    if let Some(mb) = data.ram_mb {
        second.push_str(&format!("  RAM {:.2} GB", mb / 1024.0));
    }
    if let Some(mb) = data.gpu_memory_mb {
        second.push_str(&format!("  VRAM {:.2} GB", mb / 1024.0));
    }
    if !second.is_empty() {
        lines.push((second, DIM));
    }
    if let Some((loaded, queued, all)) = data.cells {
        let waiting = if queued > 0 {
            format!(", {queued} waiting")
        } else {
            String::new()
        };
        lines.push((format!("cells {loaded}/{all} loaded{waiting}"), DIM));
    }
    if let Some((speed, set)) = data.wind {
        let source = if set {
            "set, Home: weather's"
        } else {
            "weather's"
        };
        lines.push((
            format!("wind {speed:4.1} m/s ({source})  PgUp/PgDn change"),
            DIM,
        ));
    }

    // Lines above the graph, then the graph, then the tables.
    let graph_h = 48.0 * s;
    let mut tables: Vec<(String, [f32; 4])> = Vec::new();
    let half = COLUMNS / 2;
    let rows = data.gpu_passes.len().max(data.cpu_steps.len()).min(ROWS);
    if rows > 0 {
        tables.push((
            format!(
                "{:<half$}{}",
                "GPU pass        ms", "render CPU step     ms"
            ),
            HEAD,
        ));
        for i in 0..rows {
            let cell = |list: &[(String, f64)]| {
                list.get(i)
                    .map(|(name, ms)| format!("{:<19}{ms:6.2}", clip(name, 18)))
                    .unwrap_or_default()
            };
            tables.push((
                format!("{:<half$}{}", cell(&data.gpu_passes), cell(&data.cpu_steps)),
                TEXT,
            ));
        }
    }
    if data.systems.is_empty() {
        let hint = if data.profiling {
            "systems: measuring..."
        } else {
            "start with --profile to time every system"
        };
        tables.push((hint.to_string(), DIM));
    } else {
        let cores = data
            .cores
            .map(|c| format!(" ({c:.2} cores busy)"))
            .unwrap_or_default();
        tables.push((format!("system{cores}"), HEAD));
        for (name, ms) in data.systems.iter().take(SYSTEM_ROWS) {
            tables.push((
                format!("{:<56}{ms:8.2}", clip(name, 55)),
                time_color(*ms as f32 * 4.0),
            ));
        }
    }

    let panel_h = margin * 2.0 + line * (lines.len() + tables.len()) as f32 + graph_h + line;
    let (x0, y0) = (margin, margin);
    p.rect(x0, y0, panel_w, panel_h, PANEL);
    let tx = x0 + margin;
    let mut y = y0 + margin;
    for (text, color) in &lines {
        p.text(tx, y, &clip(text, COLUMNS), *color);
        y += line;
    }

    // Frame-time graph: one bar per frame, newest on the right, with 60 and 30 fps guides.
    let graph_w = panel_w - 2.0 * margin;
    let bottom = y + graph_h;
    let bar_w = graph_w / HISTORY as f32;
    let skip = HISTORY.saturating_sub(data.frames_ms.len());
    for (i, ms) in data.frames_ms.iter().enumerate() {
        let h = (ms / GRAPH_MS).clamp(0.0, 1.0) * graph_h;
        let x = tx + (skip + i) as f32 * bar_w;
        p.rect(x, bottom - h, bar_w.max(1.0), h.max(1.0), time_color(*ms));
    }
    for guide in [1000.0 / 60.0, 1000.0 / 30.0] {
        let gy = bottom - guide / GRAPH_MS * graph_h;
        p.rect(tx, gy, graph_w, s.max(1.0), GUIDE);
    }
    y = bottom + line;
    for (text, color) in &tables {
        p.text(tx, y, &clip(text, COLUMNS), *color);
        y += line;
    }
    p.out
}

/// Draws the [`PerfOverlay`] panel. Added by [`crate::DebugPlugin`].
#[derive(Default)]
pub struct PerfOverlayFeature {
    pipeline: Option<wgpu::RenderPipeline>,
    group: Option<wgpu::BindGroup>,
    vertices: Option<GrowBuffer>,
    scratch: Vec<OverlayVertex>,
    count: u32,
}

impl RenderFeature for PerfOverlayFeature {
    fn name(&self) -> &str {
        "perf_overlay"
    }

    /// After every other overlay, game UI included.
    fn order(&self) -> i32 {
        i32::MAX - 16
    }

    fn init(&mut self, gpu: &GpuShared) {
        let device = &gpu.device;
        let shader = gpu.shader("perf overlay", SHADER, false);
        let [w, h] = ATLAS_SIZE;
        let atlas = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("perf overlay font"),
            size: wgpu::Extent3d {
                width: w,
                height: h,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::R8Unorm,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        gpu.queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &atlas,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            &atlas_texels(),
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(w),
                rows_per_image: Some(h),
            },
            wgpu::Extent3d {
                width: w,
                height: h,
                depth_or_array_layers: 1,
            },
        );
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("perf overlay font"),
            mag_filter: wgpu::FilterMode::Nearest,
            min_filter: wgpu::FilterMode::Nearest,
            ..Default::default()
        });
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("perf overlay"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
            ],
        });
        let view = atlas.create_view(&Default::default());
        self.group = Some(device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("perf overlay"),
            layout: &layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(&view),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::Sampler(&sampler),
                },
            ],
        }));
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("perf overlay"),
            bind_group_layouts: &[Some(&layout)],
            immediate_size: 0,
        });
        let buffers = [Some(OverlayVertex::layout())];
        self.pipeline = Some(
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some("perf overlay"),
                layout: Some(&pipeline_layout),
                vertex: wgpu::VertexState {
                    module: &shader,
                    entry_point: Some("vs_overlay"),
                    compilation_options: Default::default(),
                    buffers: &buffers,
                },
                primitive: wgpu::PrimitiveState {
                    topology: wgpu::PrimitiveTopology::TriangleList,
                    cull_mode: None,
                    ..Default::default()
                },
                depth_stencil: None,
                multisample: Default::default(),
                fragment: Some(wgpu::FragmentState {
                    module: &shader,
                    entry_point: Some("fs_overlay"),
                    compilation_options: Default::default(),
                    targets: &[Some(wgpu::ColorTargetState {
                        format: gpu.formats.ldr,
                        blend: Some(wgpu::BlendState::ALPHA_BLENDING),
                        write_mask: wgpu::ColorWrites::ALL,
                    })],
                }),
                multiview_mask: None,
                cache: None,
            }),
        );
        self.vertices = Some(GrowBuffer::new(
            device,
            "perf overlay",
            wgpu::BufferUsages::VERTEX,
            4096 * std::mem::size_of::<OverlayVertex>() as u64,
        ));
    }

    fn prepare(
        &mut self,
        world: &mut World,
        gpu: &GpuShared,
        view: &ViewData,
        _settings: &RenderSettings,
    ) {
        self.count = 0;
        let overlay = world
            .get_resource::<PerfOverlay>()
            .copied()
            .unwrap_or_default();
        let prompt = interact_prompt(world);
        if !overlay.enabled && prompt.is_none() {
            return;
        }
        // The panel covers the window, not the camera's viewport.
        let (width, height) = (view.window.x.max(1), view.window.y.max(1));
        let scale = match overlay.scale {
            0 => auto_scale(height),
            s => s,
        };
        self.scratch.clear();
        if overlay.enabled {
            let data = PanelData::gather(world);
            self.scratch = build_panel(&data, width, height, scale);
        }
        if let Some(text) = prompt {
            self.scratch
                .extend(build_prompt(&text, width, height, scale + 1));
        }
        if self.scratch.is_empty() {
            return;
        }
        if let Some(buffer) = &mut self.vertices {
            buffer.write(&gpu.device, &gpu.queue, bytemuck::cast_slice(&self.scratch));
            self.count = self.scratch.len() as u32;
        }
    }

    fn overlay(&mut self, encoder: &mut wgpu::CommandEncoder, ctx: &FrameContext) {
        let (Some(pipeline), Some(group), Some(buffer)) =
            (&self.pipeline, &self.group, &self.vertices)
        else {
            return;
        };
        if self.count == 0 {
            return;
        }
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("perf overlay"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: ctx.ldr,
                depth_slice: None,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Load,
                    store: wgpu::StoreOp::Store,
                },
            })],
            ..Default::default()
        });
        pass.set_pipeline(pipeline);
        pass.set_bind_group(0, group, &[]);
        pass.set_vertex_buffer(0, buffer.buffer.slice(..));
        pass.draw(0..self.count, 0..1);
    }

    fn stats(&self) -> FeatureStats {
        FeatureStats {
            draw_calls: (self.count > 0) as u32,
            triangles: self.count as u64 / 3,
            ..Default::default()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shader_validates() {
        let code = engine_render::compose_shader(SHADER, false);
        engine_render::validate_wgsl("perf_overlay", &code).unwrap();
    }

    #[test]
    fn atlas_has_a_solid_cell_and_real_glyphs() {
        let texels = atlas_texels();
        let w = ATLAS_SIZE[0] as usize;
        // Cell 0 is solid.
        assert!((0..8).all(|y| (0..8).all(|x| texels[y * w + x] == 255)));
        // 'A' (65) has ink, ' ' (32) has none.
        let ink = |code: usize| {
            let (cx, cy) = ((code % 16) * 8, (code / 16) * 8);
            (0..8)
                .flat_map(|y| (0..8).map(move |x| (x, y)))
                .filter(|(x, y)| texels[(cy + y) * w + cx + x] == 255)
                .count()
        };
        assert!(ink('A' as usize) > 10);
        assert_eq!(ink(' ' as usize), 0);
    }

    #[test]
    fn history_keeps_the_last_frames() {
        let mut h = FrameHistory::default();
        for i in 0..(HISTORY + 10) {
            h.push(i as f32);
        }
        assert_eq!(h.frames_ms.len(), HISTORY);
        assert_eq!(h.frames_ms[0], 10.0);
        let (_, worst) = h.summary();
        assert_eq!(worst, (HISTORY + 9) as f32);
    }

    #[test]
    fn panel_stays_on_screen_and_in_clip_space() {
        let data = PanelData {
            frames_ms: (0..HISTORY).map(|i| 10.0 + (i % 40) as f32).collect(),
            gpu_ms: Some(12.5),
            gpu_memory_mb: Some(4096.0),
            ram_mb: Some(6200.0),
            cells: Some((12, 2, 31)),
            gpu_passes: vec![("scene".into(), 9.8), ("shadows".into(), 1.7)],
            cpu_steps: vec![("prepare meshes".into(), 9.75)],
            systems: vec![("engine_physics::physics_step".into(), 3.2)],
            cores: Some(1.4),
            profiling: true,
            wind: Some((19.0, true)),
        };
        let verts = build_panel(&data, 2560, 1440, auto_scale(1440));
        assert!(!verts.is_empty());
        assert_eq!(verts.len() % 6, 0);
        for v in &verts {
            let [x, y] = v.position;
            assert!(
                (-1.0..=1.0).contains(&x) && (-1.0..=1.0).contains(&y),
                "{v:?}"
            );
        }
        // The panel sits in the top-left quarter of a 1440p screen.
        let max_x = verts.iter().map(|v| v.position[0]).fold(-1.0, f32::max);
        assert!(max_x < 0.0, "{max_x}");
        assert_eq!(auto_scale(1440), 2);
        assert_eq!(auto_scale(720), 1);
        assert_eq!(auto_scale(2160), 3);
    }

    #[test]
    fn prompt_sits_centered_below_the_middle() {
        let verts = build_prompt("E  Drive", 1920, 1080, 3);
        // A box and six letters (spaces draw nothing).
        assert_eq!(verts.len(), 6 * 7);
        let xs = verts.iter().map(|v| v.position[0]);
        let (min_x, max_x) = xs.fold((1.0f32, -1.0f32), |(a, b), x| (a.min(x), b.max(x)));
        assert!((min_x + max_x).abs() < 1e-3, "{min_x} {max_x}");
        assert!(
            verts
                .iter()
                .all(|v| v.position[1] < 0.0 && v.position[1] > -1.0)
        );
    }

    #[test]
    fn overlay_toggles_with_f3() {
        use engine_core::prelude::*;
        let mut app = App::new();
        app.add_plugins(engine_input::InputPlugin);
        app.init_resource::<PerfOverlay>()
            .init_resource::<FrameHistory>()
            .add_systems(First, perf_overlay_system);
        app.world
            .resource_mut::<ButtonInput<KeyCode>>()
            .press(KeyCode::F3);
        app.update();
        assert!(app.world.resource::<PerfOverlay>().enabled);
    }

    #[test]
    fn page_keys_step_the_wind_while_the_panel_is_open() {
        use engine_core::prelude::*;
        let mut app = App::new();
        app.add_plugins(engine_input::InputPlugin);
        app.init_resource::<PerfOverlay>()
            .init_resource::<FrameHistory>()
            .init_resource::<WindField>()
            .init_resource::<WindOverride>()
            .add_systems(First, perf_overlay_system);
        let press = |app: &mut App, key: KeyCode| {
            app.world.resource_mut::<ButtonInput<KeyCode>>().press(key);
            app.update();
            app.world
                .resource_mut::<ButtonInput<KeyCode>>()
                .release(key);
            app.update();
        };
        // Closed panel: the page keys are someone else's.
        press(&mut app, KeyCode::PageUp);
        assert_eq!(app.world.resource::<WindOverride>().speed_m_s, None);
        press(&mut app, KeyCode::F3);
        // The default wind is ~3.2 m/s; one step up is 4.5.
        press(&mut app, KeyCode::PageUp);
        assert_eq!(app.world.resource::<WindOverride>().speed_m_s, Some(4.5));
        press(&mut app, KeyCode::PageUp);
        assert_eq!(app.world.resource::<WindOverride>().speed_m_s, Some(7.0));
        press(&mut app, KeyCode::PageDown);
        assert_eq!(app.world.resource::<WindOverride>().speed_m_s, Some(4.5));
        press(&mut app, KeyCode::Home);
        assert_eq!(app.world.resource::<WindOverride>().speed_m_s, None);
    }
}
