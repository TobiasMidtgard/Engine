//! Where the frame time goes, in the log every few seconds (`RenderSettings::perf_log_secs`):
//!
//! ```text
//! perf: 24.1 fps (41.5 ms, worst 63.0); render CPU 9.8 ms (acquiring the window image 22.3 ms),
//! everything else 9.4 ms; GPU 30.6 ms; GPU memory 5.81 GB in use, 6.40 GB reserved;
//! 1934 draws, 7.2 M triangles
//! ```
//!
//! So one run on a player's machine tells a frame held up by the CPU (render CPU or everything
//! else, the game's systems, near the frame time) from one held up by the GPU (GPU time near
//! it, the wait for the window image long) and from one where the GPU waits on memory (frame
//! long, GPU time short, memory near the card's size). The GPU time is the renderer's main
//! command buffer between two timestamps (`TIMESTAMP_QUERY`; "GPU n/a" without it), read back
//! a few frames late without waiting; GPU memory is wgpu's allocator report (left out where the
//! backend has none). The same numbers go to `Diagnostics` as `render.frame_ms`,
//! `render.acquire_ms`, `render.gpu_ms` and `render.gpu_memory_mb`.
//!
//! The frame is also split into steps: on the CPU each step of the render system (a
//! `tracing` span, target `engine_render`, name `render_step`, field `name`), on the GPU each
//! pass (a timestamp where it starts). Their latest times go to `Diagnostics` as
//! `render.cpu.<step>` and `render.gpu.<pass>` (milliseconds), and with the frame profiler on
//! (`--profile`) the log gets a table of both with every report. The profiler also splits the
//! scene pass by feature (`render.gpu.scene: <feature>`, then `render.gpu.scene: sky phase`),
//! so its cost can be put on what draws it; `render.gpu.scene` stays their sum.

use std::borrow::Cow;
use std::collections::HashMap;
use std::fmt::Write as _;
use std::sync::Arc;
use std::sync::atomic::{AtomicU8, Ordering};
use std::time::Instant;

use engine_core::prelude::Diagnostics;

const FREE: u8 = 0;
const IN_FLIGHT: u8 = 1;
const MAPPING: u8 = 2;
const MAPPED: u8 = 3;
const FAILED: u8 = 4;

/// Timestamp readbacks in flight at once.
const SLOTS: usize = 4;
/// Most timestamps in one frame (one where each pass starts, one at the end); passes past it
/// count towards the one before.
const MAX_MARKS: usize = 128;
/// Bytes of one frame's timestamps (a multiple of the 256 query resolves must start on).
const SLOT_BYTES: u64 = (MAX_MARKS * 8) as u64;
const _: () = assert!(SLOT_BYTES.is_multiple_of(wgpu::QUERY_RESOLVE_BUFFER_ALIGNMENT));

type Label = Cow<'static, str>;

struct Slot {
    buffer: wgpu::Buffer,
    state: Arc<AtomicU8>,
    /// The pass each timestamp starts (the last one ends the frame).
    labels: Vec<Label>,
}

/// GPU time of the frame's command buffer and of each pass in it, from timestamps.
struct GpuTimer {
    queries: wgpu::QuerySet,
    resolve: wgpu::Buffer,
    slots: Vec<Slot>,
    /// The slot this frame writes, between `begin` and `after_submit`.
    pending: Option<usize>,
    /// Nanoseconds per timestamp tick.
    period_ns: f32,
}

/// One finished frame's GPU times, milliseconds.
struct GpuFrame {
    total: f32,
    passes: Vec<(Label, f32)>,
}

impl GpuTimer {
    fn new(device: &wgpu::Device, queue: &wgpu::Queue) -> Option<Self> {
        if !device.features().contains(wgpu::Features::TIMESTAMP_QUERY) {
            return None;
        }
        let queries = device.create_query_set(&wgpu::QuerySetDescriptor {
            label: Some("frame timestamps"),
            ty: wgpu::QueryType::Timestamp,
            count: (MAX_MARKS * SLOTS) as u32,
        });
        let resolve = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("frame timestamps resolve"),
            size: SLOT_BYTES * SLOTS as u64,
            usage: wgpu::BufferUsages::QUERY_RESOLVE | wgpu::BufferUsages::COPY_SRC,
            mapped_at_creation: false,
        });
        let slots = (0..SLOTS)
            .map(|_| Slot {
                buffer: device.create_buffer(&wgpu::BufferDescriptor {
                    label: Some("frame timestamps readback"),
                    size: SLOT_BYTES,
                    usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
                    mapped_at_creation: false,
                }),
                state: Arc::new(AtomicU8::new(FREE)),
                labels: Vec::with_capacity(MAX_MARKS),
            })
            .collect();
        Some(Self {
            queries,
            resolve,
            slots,
            pending: None,
            period_ns: queue.get_timestamp_period(),
        })
    }

    /// An empty compute pass that writes a timestamp when the work before it is done.
    fn stamp(&self, encoder: &mut wgpu::CommandEncoder, index: u32) {
        let writes = wgpu::ComputePassTimestampWrites {
            query_set: &self.queries,
            beginning_of_pass_write_index: Some(index),
            end_of_pass_write_index: None,
        };
        drop(encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
            label: Some("timestamp"),
            timestamp_writes: Some(writes),
        }));
    }

    fn begin(&mut self, encoder: &mut wgpu::CommandEncoder, first: Label) {
        self.pending = self
            .slots
            .iter()
            .position(|s| s.state.load(Ordering::Acquire) == FREE);
        if let Some(i) = self.pending {
            self.slots[i].labels.clear();
            self.mark(encoder, first);
        }
    }

    /// Pass `label` starts here.
    fn mark(&mut self, encoder: &mut wgpu::CommandEncoder, label: Label) {
        let Some(i) = self.pending else {
            return;
        };
        // Keep the last timestamp for the end of the frame.
        let n = self.slots[i].labels.len();
        if n + 1 >= MAX_MARKS {
            return;
        }
        self.stamp(encoder, (i * MAX_MARKS + n) as u32);
        self.slots[i].labels.push(label);
    }

    fn end(&mut self, encoder: &mut wgpu::CommandEncoder) {
        let Some(i) = self.pending else {
            return;
        };
        let first = (i * MAX_MARKS) as u32;
        let n = self.slots[i].labels.len() as u32;
        self.stamp(encoder, first + n);
        let count = n + 1;
        let offset = SLOT_BYTES * i as u64;
        encoder.resolve_query_set(&self.queries, first..first + count, &self.resolve, offset);
        encoder.copy_buffer_to_buffer(
            &self.resolve,
            offset,
            &self.slots[i].buffer,
            0,
            count as u64 * 8,
        );
        self.slots[i].state.store(IN_FLIGHT, Ordering::Release);
    }

    fn after_submit(&mut self) {
        let Some(i) = self.pending.take() else {
            return;
        };
        let slot = &self.slots[i];
        if slot.state.load(Ordering::Acquire) != IN_FLIGHT {
            return;
        }
        slot.state.store(MAPPING, Ordering::Release);
        let state = slot.state.clone();
        slot.buffer
            .slice(..)
            .map_async(wgpu::MapMode::Read, move |r| {
                state.store(if r.is_ok() { MAPPED } else { FAILED }, Ordering::Release);
            });
    }

    /// Finished frames' GPU times (non-blocking; the renderer polls the device each frame).
    fn collect(&mut self, out: &mut Vec<GpuFrame>) {
        let ms = self.period_ns * 1e-6;
        for slot in &mut self.slots {
            match slot.state.load(Ordering::Acquire) {
                MAPPED => {
                    let n = slot.labels.len();
                    let ticks: Option<Vec<u64>> = match slot.buffer.slice(..).get_mapped_range() {
                        Ok(view) => {
                            let t: &[u64] = bytemuck::cast_slice(&view);
                            Some(t[..=n].to_vec())
                        }
                        Err(_) => None,
                    };
                    slot.buffer.unmap();
                    slot.state.store(FREE, Ordering::Release);
                    let Some(t) = ticks else {
                        continue;
                    };
                    // Timestamps that go backwards (a reset GPU clock) spoil the frame.
                    let Some(total) = t[n].checked_sub(t[0]) else {
                        continue;
                    };
                    let mut passes: Vec<(Label, f32)> = slot
                        .labels
                        .drain(..)
                        .zip(t.windows(2))
                        .map(|(label, w)| (label, w[1].saturating_sub(w[0]) as f32 * ms))
                        .collect();
                    // A scene split by feature (profiler) still reports the whole scene.
                    let split: Vec<f32> = passes
                        .iter()
                        .filter(|(label, _)| label.starts_with("scene: "))
                        .map(|&(_, ms)| ms)
                        .collect();
                    if !split.is_empty() {
                        passes.push(("scene".into(), split.iter().sum()));
                    }
                    out.push(GpuFrame {
                        total: total as f32 * ms,
                        passes,
                    });
                }
                FAILED => {
                    slot.buffer.unmap();
                    slot.state.store(FREE, Ordering::Release);
                }
                _ => {}
            }
        }
    }
}

/// The render system's CPU steps this frame: each a `tracing` span (target `engine_render`,
/// name `render_step`, field `name`) and a time for the profile. Local to a frame, as the
/// entered span stays on its thread.
#[derive(Default)]
pub(crate) struct CpuSteps {
    current: Option<(Label, Instant, tracing::span::EnteredSpan)>,
    done: Vec<(Label, f32)>,
}

impl CpuSteps {
    /// End the current step and start `name`.
    pub fn step(&mut self, name: impl Into<Label>) {
        self.finish();
        let name = name.into();
        let span = tracing::info_span!(target: "engine_render", "render_step", name = %name);
        self.current = Some((name, Instant::now(), span.entered()));
    }

    /// End the current step.
    pub fn finish(&mut self) {
        if let Some((name, started, span)) = self.current.take() {
            drop(span);
            self.done
                .push((name, started.elapsed().as_secs_f32() * 1000.0));
        }
    }

    /// End the current step and keep the times, to carry them to another thread (the entered
    /// span can't go along).
    pub fn park(mut self) -> ParkedSteps {
        self.finish();
        ParkedSteps(self.done)
    }

    /// Go on with steps parked on another thread.
    pub fn resume(parked: ParkedSteps) -> Self {
        Self {
            current: None,
            done: parked.0,
        }
    }
}

/// [`CpuSteps`]' finished steps, between threads.
pub(crate) struct ParkedSteps(Vec<(Label, f32)>);

/// Sums over the current report window.
#[derive(Default)]
struct Window {
    frames: u32,
    frame_ms: f32,
    worst_ms: f32,
    cpu_ms: f32,
    acquire_ms: f32,
    gpu_ms: f32,
    gpu_frames: u32,
    /// Milliseconds per CPU step and per GPU pass, summed over the window.
    cpu_steps: HashMap<Label, f32>,
    gpu_passes: HashMap<Label, f32>,
}

/// The renderer's frame timing (see the module docs).
pub(crate) struct PerfLog {
    timer: Option<GpuTimer>,
    last_frame: Option<Instant>,
    window_start: Instant,
    window: Window,
    gpu_samples: Vec<GpuFrame>,
    /// Latest values, for `Diagnostics`.
    pub frame_ms: f32,
    pub acquire_ms: f32,
    pub gpu_ms: Option<f32>,
    pub gpu_memory_mb: Option<f32>,
    /// What the GPU allocator holds from the driver, in use or not (MB; same source).
    pub gpu_reserved_mb: Option<f32>,
    cpu_steps: Vec<(Label, f32)>,
    gpu_passes: Vec<(Label, f32)>,
    /// Each feature's draws and triangles in the latest frame, for the profile table.
    features: Vec<(String, crate::FeatureStats)>,
}

impl PerfLog {
    pub fn new(device: &wgpu::Device, queue: &wgpu::Queue) -> Self {
        Self {
            timer: GpuTimer::new(device, queue),
            last_frame: None,
            window_start: Instant::now(),
            window: Window::default(),
            gpu_samples: Vec::new(),
            frame_ms: 0.0,
            acquire_ms: 0.0,
            gpu_ms: None,
            gpu_memory_mb: None,
            gpu_reserved_mb: None,
            cpu_steps: Vec::new(),
            gpu_passes: Vec::new(),
            features: Vec::new(),
        }
    }

    /// Call with the frame's first commands, naming the first pass: stamps the start of its
    /// GPU time.
    pub fn begin(
        &mut self,
        cpu: &mut CpuSteps,
        encoder: &mut wgpu::CommandEncoder,
        first: impl Into<Label>,
    ) {
        let first = first.into();
        if let Some(t) = &mut self.timer {
            t.begin(encoder, first.clone());
        }
        cpu.step(first);
    }

    /// Pass `name` starts here: a new CPU step and a GPU timestamp.
    pub fn pass(
        &mut self,
        cpu: &mut CpuSteps,
        encoder: &mut wgpu::CommandEncoder,
        name: impl Into<Label>,
    ) {
        let name = name.into();
        if let Some(t) = &mut self.timer {
            t.mark(encoder, name.clone());
        }
        cpu.step(name);
    }

    /// Call with the frame's last commands, before `finish`.
    pub fn end(&mut self, encoder: &mut wgpu::CommandEncoder) {
        if let Some(t) = &mut self.timer {
            t.end(encoder);
        }
    }

    /// Call right after `queue.submit`.
    pub fn after_submit(&mut self) {
        if let Some(t) = &mut self.timer {
            t.after_submit();
        }
    }

    /// Account for a finished frame (`cpu_ms`: the render system's time, `acquire_ms`: the
    /// part of it spent waiting for the window image, `cpu`: its steps) and log a report every
    /// `every_secs` (0 = never). Polls nothing itself: the GPU times arrive as the renderer
    /// polls.
    pub fn frame(
        &mut self,
        device: &wgpu::Device,
        stats: &crate::RenderStats,
        acquire_ms: f32,
        mut cpu: CpuSteps,
        every_secs: f32,
    ) {
        cpu.finish();
        self.cpu_steps = cpu.done;
        let now = Instant::now();
        self.frame_ms = self
            .last_frame
            .map_or(0.0, |t| (now - t).as_secs_f32() * 1000.0);
        self.last_frame = Some(now);
        self.acquire_ms = acquire_ms;
        self.gpu_samples.clear();
        if let Some(t) = &mut self.timer {
            t.collect(&mut self.gpu_samples);
        }
        let w = &mut self.window;
        if self.frame_ms > 0.0 {
            w.frames += 1;
            w.frame_ms += self.frame_ms;
            w.worst_ms = w.worst_ms.max(self.frame_ms);
            w.cpu_ms += stats.cpu_ms;
            w.acquire_ms += acquire_ms;
            for (name, ms) in &self.cpu_steps {
                *w.cpu_steps.entry(name.clone()).or_default() += ms;
            }
        }
        for frame in self.gpu_samples.drain(..) {
            w.gpu_ms += frame.total;
            w.gpu_frames += 1;
            for (name, ms) in &frame.passes {
                *w.gpu_passes.entry(name.clone()).or_default() += ms;
            }
            self.gpu_ms = Some(frame.total);
            self.gpu_passes = frame.passes;
        }
        if every_secs <= 0.0 || (now - self.window_start).as_secs_f32() < every_secs {
            return;
        }
        let report = device.generate_allocator_report();
        let mb = |bytes: u64| bytes as f32 / (1024.0 * 1024.0);
        self.gpu_memory_mb = report.as_ref().map(|r| mb(r.total_allocated_bytes));
        let reserved_mb = report.as_ref().map(|r| mb(r.total_reserved_bytes));
        self.gpu_reserved_mb = reserved_mb;
        let w = &self.window;
        if w.frames > 0 {
            let n = w.frames as f32;
            let frame = w.frame_ms / n;
            let cpu = w.cpu_ms / n;
            let gpu = if w.gpu_frames > 0 {
                format!("GPU {:.1} ms", w.gpu_ms / w.gpu_frames as f32)
            } else {
                "GPU n/a".to_string()
            };
            let memory = match (self.gpu_memory_mb, reserved_mb) {
                (Some(used), Some(reserved)) => format!(
                    "; GPU memory {:.2} GB in use, {:.2} GB reserved",
                    used / 1024.0,
                    reserved / 1024.0
                ),
                _ => String::new(),
            };
            tracing::info!(
                "perf: {:.1} fps ({:.1} ms, worst {:.1}); render CPU {:.1} ms (acquiring the \
                 window image {:.1} ms), everything else {:.1} ms; {gpu}{memory}; {} draws, \
                 {:.1} M triangles",
                1000.0 / frame.max(1e-3),
                frame,
                w.worst_ms,
                cpu,
                w.acquire_ms / n,
                (frame - cpu).max(0.0),
                stats.draw_calls,
                stats.triangles as f64 / 1e6,
            );
            if engine_core::profile::enabled() {
                tracing::info!("render profile\n{}", self.table());
                if let Some(report) = &report {
                    tracing::info!("gpu memory\n{}", memory_table(report));
                }
            }
        }
        self.window = Window::default();
        self.window_start = now;
    }

    /// Each feature's draws and triangles this frame (listed in the profile table).
    pub fn set_features(&mut self, features: impl Iterator<Item = (String, crate::FeatureStats)>) {
        self.features.clear();
        self.features.extend(features);
    }

    /// The window's CPU steps and GPU passes, slowest first, milliseconds per frame, and the
    /// latest frame's draws and triangles per feature.
    fn table(&self) -> String {
        let w = &self.window;
        let mut out = String::new();
        let mut section = |title: &str, sums: &HashMap<Label, f32>, frames: u32| {
            let mut rows: Vec<_> = sums.iter().collect();
            rows.sort_by(|a, b| b.1.total_cmp(a.1));
            let total: f32 = sums.values().sum();
            let frames = frames.max(1) as f32;
            let _ = writeln!(out, "{title:<40} {:>8.2} ms/frame", total / frames);
            for (name, ms) in rows {
                let _ = writeln!(out, "  {:<38} {:>8.2}", truncate(name, 38), ms / frames);
            }
        };
        section("render CPU steps", &w.cpu_steps, w.frames);
        if w.gpu_frames > 0 {
            section("GPU passes", &w.gpu_passes, w.gpu_frames);
        } else {
            out.push_str("GPU passes: no timestamps on this GPU\n");
        }
        let mut features: Vec<_> = self
            .features
            .iter()
            .filter(|(_, s)| s.draw_calls > 0 || s.triangles > 0)
            .collect();
        features.sort_by_key(|(_, s)| std::cmp::Reverse(s.triangles));
        let _ = writeln!(
            out,
            "{:<40} {:>8} {:>10} {:>8}",
            "features (last frame)", "draws", "triangles", "culled"
        );
        for (name, s) in features {
            let _ = writeln!(
                out,
                "  {:<38} {:>8} {:>10} {:>8}",
                truncate(name, 38),
                s.draw_calls,
                s.triangles,
                s.culled
            );
        }
        out
    }

    /// The latest step and pass times into `Diagnostics` (`render.cpu.<step>`,
    /// `render.gpu.<pass>`).
    pub fn publish(&self, d: &mut Diagnostics) {
        let mut key = String::new();
        for (prefix, rows) in [
            ("render.cpu.", &self.cpu_steps),
            ("render.gpu.", &self.gpu_passes),
        ] {
            for (name, ms) in rows {
                key.clear();
                key.push_str(prefix);
                key.push_str(name);
                d.set(&key, *ms as f64);
            }
        }
    }
}

/// One allocator block for [`memory_table`]: its size and its allocations (label, bytes).
struct Block<'a> {
    size: u64,
    allocations: Vec<(&'a str, u64)>,
}

impl Block<'_> {
    fn used(&self) -> u64 {
        self.allocations.iter().map(|a| a.1).sum()
    }
}

/// Where the allocator's memory goes (`--profile`): blocks by how full they are, the blocks with
/// the most free space and what sits in them, and the largest users by resource label. Reserved
/// but unused memory is free space inside blocks that still hold something.
pub(crate) fn memory_table(report: &wgpu::AllocatorReport) -> String {
    let blocks: Vec<Block> = report
        .blocks
        .iter()
        .map(|b| Block {
            size: b.size,
            allocations: report.allocations[b.allocations.clone()]
                .iter()
                .map(|a| (a.name.as_str(), a.size))
                .collect(),
        })
        .collect();
    blocks_table(&blocks)
}

fn blocks_table(blocks: &[Block]) -> String {
    use std::fmt::Write;
    let mb = |bytes: u64| bytes as f64 / (1024.0 * 1024.0);
    let mut out = String::new();
    let reserved: u64 = blocks.iter().map(|b| b.size).sum();
    let in_use: u64 = blocks.iter().map(Block::used).sum();
    let fill = |b: &Block| b.used() as f64 / b.size.max(1) as f64;
    let quarter = blocks.iter().filter(|b| fill(b) < 0.25).count();
    let half = blocks.iter().filter(|b| fill(b) < 0.5).count();
    let _ = writeln!(
        out,
        "{} blocks, {:.0} MB reserved, {:.0} MB in use, {:.0} MB free inside blocks; {quarter} \
         blocks under 25% full, {half} under 50%",
        blocks.len(),
        mb(reserved),
        mb(in_use),
        mb(reserved.saturating_sub(in_use)),
    );
    // Block sizes.
    let mut sizes: std::collections::BTreeMap<u64, (u32, u64)> = Default::default();
    for b in blocks {
        let e = sizes.entry(b.size).or_default();
        e.0 += 1;
        e.1 += b.used();
    }
    let _ = writeln!(out, "blocks by size:");
    for (size, (count, used)) in sizes.iter().rev().take(8) {
        let _ = writeln!(
            out,
            "  {:>9.1} MB x {count:<4} {:>5.1}% full",
            mb(*size),
            100.0 * *used as f64 / (*size * *count as u64).max(1) as f64
        );
    }
    // The emptiest space, with each block's largest tenant.
    let mut emptiest: Vec<&Block> = blocks.iter().collect();
    emptiest.sort_by_key(|b| std::cmp::Reverse(b.size.saturating_sub(b.used())));
    let _ = writeln!(out, "most free space:");
    for b in emptiest.iter().take(10) {
        let largest = b.allocations.iter().max_by_key(|a| a.1);
        let _ = writeln!(
            out,
            "  {:>7.1} MB free of {:>6.1} MB, {:>4} allocations, largest {}",
            mb(b.size.saturating_sub(b.used())),
            mb(b.size),
            b.allocations.len(),
            largest.map_or("-".to_string(), |a| format!("{:?} {:.1} MB", a.0, mb(a.1))),
        );
    }
    // Users by label.
    let mut labels: HashMap<&str, (u32, u64)> = HashMap::new();
    for &(name, bytes) in blocks.iter().flat_map(|b| &b.allocations) {
        let e = labels.entry(name).or_default();
        e.0 += 1;
        e.1 += bytes;
    }
    let mut labels: Vec<_> = labels.into_iter().collect();
    labels.sort_by_key(|(_, (_, bytes))| std::cmp::Reverse(*bytes));
    let _ = writeln!(out, "largest users:");
    for (name, (count, bytes)) in labels.iter().take(20) {
        let name = if name.is_empty() { "(no label)" } else { name };
        let _ = writeln!(
            out,
            "  {:>8.1} MB  {count:>5} x {}",
            mb(*bytes),
            truncate(name, 48)
        );
    }
    out
}

fn truncate(s: &str, n: usize) -> &str {
    match s.char_indices().nth(n) {
        Some((i, _)) => &s[..i],
        None => s,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn memory_table_shows_free_space_and_its_tenants() {
        const MB: u64 = 1024 * 1024;
        // A full 64 MB block and a 256 MB block holding one 16 MB atlas.
        let blocks = [
            Block {
                size: 64 * MB,
                allocations: vec![("mesh vertices", 40 * MB), ("mesh indices", 24 * MB)],
            },
            Block {
                size: 256 * MB,
                allocations: vec![("impostor atlas", 16 * MB)],
            },
        ];
        let table = blocks_table(&blocks);
        eprintln!("{table}");
        assert!(table.contains("2 blocks, 320 MB reserved, 80 MB in use, 240 MB free"));
        assert!(table.contains("1 blocks under 25% full"));
        let free = table
            .lines()
            .skip_while(|l| !l.starts_with("most free"))
            .nth(1);
        assert!(
            free.unwrap().contains("240.0 MB free of  256.0 MB"),
            "{free:?}"
        );
        assert!(
            free.unwrap().contains("\"impostor atlas\" 16.0 MB"),
            "{free:?}"
        );
        let users = table
            .lines()
            .skip_while(|l| !l.starts_with("largest users"))
            .nth(1);
        assert!(
            users.unwrap().contains("40.0 MB      1 x mesh vertices"),
            "{users:?}"
        );
    }
}
