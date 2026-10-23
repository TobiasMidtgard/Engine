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

use std::sync::Arc;
use std::sync::atomic::{AtomicU8, Ordering};
use std::time::Instant;

const FREE: u8 = 0;
const IN_FLIGHT: u8 = 1;
const MAPPING: u8 = 2;
const MAPPED: u8 = 3;
const FAILED: u8 = 4;

/// Timestamp readbacks in flight at once.
const SLOTS: usize = 4;
/// Query resolves must start on this many bytes.
const RESOLVE_STRIDE: u64 = wgpu::QUERY_RESOLVE_BUFFER_ALIGNMENT;

struct Slot {
    buffer: wgpu::Buffer,
    state: Arc<AtomicU8>,
}

/// GPU time of the frame's command buffer from two timestamps.
struct GpuTimer {
    queries: wgpu::QuerySet,
    resolve: wgpu::Buffer,
    slots: Vec<Slot>,
    /// The slot this frame writes, between `begin` and `after_submit`.
    pending: Option<usize>,
    /// Nanoseconds per timestamp tick.
    period_ns: f32,
}

impl GpuTimer {
    fn new(device: &wgpu::Device, queue: &wgpu::Queue) -> Option<Self> {
        if !device.features().contains(wgpu::Features::TIMESTAMP_QUERY) {
            return None;
        }
        let queries = device.create_query_set(&wgpu::QuerySetDescriptor {
            label: Some("frame timestamps"),
            ty: wgpu::QueryType::Timestamp,
            count: 2 * SLOTS as u32,
        });
        let resolve = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("frame timestamps resolve"),
            size: RESOLVE_STRIDE * SLOTS as u64,
            usage: wgpu::BufferUsages::QUERY_RESOLVE | wgpu::BufferUsages::COPY_SRC,
            mapped_at_creation: false,
        });
        let slots = (0..SLOTS)
            .map(|_| Slot {
                buffer: device.create_buffer(&wgpu::BufferDescriptor {
                    label: Some("frame timestamps readback"),
                    size: 16,
                    usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
                    mapped_at_creation: false,
                }),
                state: Arc::new(AtomicU8::new(FREE)),
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

    /// An empty compute pass that writes timestamp `index` (one at the start of the frame's
    /// commands, one at the end).
    fn stamp(&self, encoder: &mut wgpu::CommandEncoder, index: u32, at_start: bool) {
        let writes = wgpu::ComputePassTimestampWrites {
            query_set: &self.queries,
            beginning_of_pass_write_index: at_start.then_some(index),
            end_of_pass_write_index: (!at_start).then_some(index),
        };
        drop(encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
            label: Some(if at_start { "frame start" } else { "frame end" }),
            timestamp_writes: Some(writes),
        }));
    }

    fn begin(&mut self, encoder: &mut wgpu::CommandEncoder) {
        self.pending = self
            .slots
            .iter()
            .position(|s| s.state.load(Ordering::Acquire) == FREE);
        if let Some(i) = self.pending {
            self.stamp(encoder, 2 * i as u32, true);
        }
    }

    fn end(&mut self, encoder: &mut wgpu::CommandEncoder) {
        let Some(i) = self.pending else {
            return;
        };
        let first = 2 * i as u32;
        self.stamp(encoder, first + 1, false);
        let offset = RESOLVE_STRIDE * i as u64;
        encoder.resolve_query_set(&self.queries, first..first + 2, &self.resolve, offset);
        encoder.copy_buffer_to_buffer(&self.resolve, offset, &self.slots[i].buffer, 0, 16);
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

    /// Finished frames' GPU times in milliseconds (non-blocking; the renderer polls the device
    /// each frame).
    fn collect(&mut self, out: &mut Vec<f32>) {
        for slot in &self.slots {
            match slot.state.load(Ordering::Acquire) {
                MAPPED => {
                    let ticks = match slot.buffer.slice(..).get_mapped_range() {
                        Ok(view) => {
                            let t: &[u64] = bytemuck::cast_slice(&view);
                            t[1].checked_sub(t[0])
                        }
                        Err(_) => None,
                    };
                    slot.buffer.unmap();
                    slot.state.store(FREE, Ordering::Release);
                    if let Some(ticks) = ticks {
                        out.push(ticks as f32 * self.period_ns * 1e-6);
                    }
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
}

/// The renderer's frame timing (see the module docs).
pub(crate) struct PerfLog {
    timer: Option<GpuTimer>,
    last_frame: Option<Instant>,
    window_start: Instant,
    window: Window,
    gpu_samples: Vec<f32>,
    /// Latest values, for `Diagnostics`.
    pub frame_ms: f32,
    pub acquire_ms: f32,
    pub gpu_ms: Option<f32>,
    pub gpu_memory_mb: Option<f32>,
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
        }
    }

    /// Call with the frame's first commands: stamps the start of its GPU time.
    pub fn begin(&mut self, encoder: &mut wgpu::CommandEncoder) {
        if let Some(t) = &mut self.timer {
            t.begin(encoder);
        }
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
    /// part of it spent waiting for the window image) and log a report every `every_secs`
    /// (0 = never). Polls nothing itself: the GPU times arrive as the renderer polls.
    pub fn frame(
        &mut self,
        device: &wgpu::Device,
        stats: &crate::RenderStats,
        acquire_ms: f32,
        every_secs: f32,
    ) {
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
        if let Some(&ms) = self.gpu_samples.last() {
            self.gpu_ms = Some(ms);
        }
        let w = &mut self.window;
        if self.frame_ms > 0.0 {
            w.frames += 1;
            w.frame_ms += self.frame_ms;
            w.worst_ms = w.worst_ms.max(self.frame_ms);
            w.cpu_ms += stats.cpu_ms;
            w.acquire_ms += acquire_ms;
        }
        w.gpu_ms += self.gpu_samples.iter().sum::<f32>();
        w.gpu_frames += self.gpu_samples.len() as u32;
        if every_secs <= 0.0 || (now - self.window_start).as_secs_f32() < every_secs {
            return;
        }
        let report = device.generate_allocator_report();
        let mb = |bytes: u64| bytes as f32 / (1024.0 * 1024.0);
        self.gpu_memory_mb = report.as_ref().map(|r| mb(r.total_allocated_bytes));
        let reserved_mb = report.as_ref().map(|r| mb(r.total_reserved_bytes));
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
        }
        self.window = Window::default();
        self.window_start = now;
    }
}
