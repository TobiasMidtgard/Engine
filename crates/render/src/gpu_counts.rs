//! Four counters a compute pass adds to on the GPU (`array<atomic<u32>, 4>`), read back for
//! the CPU some frames later: probe GI's update and the sun's measuring rays (probe_gi), the
//! sun's trace (ray_scene/sun.rs).

use std::sync::Arc;
use std::sync::atomic::{AtomicU8, Ordering};

/// The counters' size: four `u32`.
pub(crate) const COUNTS_BYTES: u64 = 16;

/// Four counters a pass adds to on the GPU, and their copy for the CPU to read some frames
/// later: cleared before the pass, copied out after it, mapped once the copy was submitted.
pub(crate) struct GpuCounts {
    /// What the pass binds (`STORAGE`).
    pub(crate) counts: wgpu::Buffer,
    read: wgpu::Buffer,
    /// `COUNTS_*`.
    state: Arc<AtomicU8>,
    /// This frame's commands copy the counts out: map the copy once they are submitted.
    copied: bool,
    /// The latest counts read.
    pub(crate) latest: Option<[u32; 4]>,
}

const COUNTS_FREE: u8 = 0;
const COUNTS_MAPPING: u8 = 1;
const COUNTS_MAPPED: u8 = 2;
const COUNTS_FAILED: u8 = 3;

impl GpuCounts {
    pub(crate) fn new(device: &wgpu::Device, label: &str) -> Self {
        let buffer = |label: &str, usage| {
            device.create_buffer(&wgpu::BufferDescriptor {
                label: Some(label),
                size: COUNTS_BYTES,
                usage,
                mapped_at_creation: false,
            })
        };
        Self {
            counts: buffer(
                label,
                wgpu::BufferUsages::STORAGE
                    | wgpu::BufferUsages::COPY_SRC
                    | wgpu::BufferUsages::COPY_DST,
            ),
            read: buffer(
                &format!("{label} readback"),
                wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            ),
            state: Default::default(),
            copied: false,
            latest: None,
        }
    }

    /// Before the pass that counts: start from zero.
    pub(crate) fn clear(&self, encoder: &mut wgpu::CommandEncoder) {
        encoder.clear_buffer(&self.counts, 0, None);
    }

    /// After the pass: copy the counts out for the CPU, when they were counted and no earlier
    /// copy is still on its way.
    pub(crate) fn copy(&mut self, encoder: &mut wgpu::CommandEncoder, count: bool) {
        if count && !self.copied && self.state.load(Ordering::Acquire) == COUNTS_FREE {
            encoder.copy_buffer_to_buffer(&self.counts, 0, &self.read, 0, COUNTS_BYTES);
            self.copied = true;
        }
    }

    /// Take in the counts that arrived, and ask for those last frame's commands copied out
    /// (they are submitted by now).
    pub(crate) fn take(&mut self) {
        if std::mem::take(&mut self.copied) {
            self.state.store(COUNTS_MAPPING, Ordering::Release);
            let state = self.state.clone();
            self.read
                .slice(..)
                .map_async(wgpu::MapMode::Read, move |r| {
                    let to = if r.is_ok() {
                        COUNTS_MAPPED
                    } else {
                        COUNTS_FAILED
                    };
                    state.store(to, Ordering::Release);
                });
        }
        match self.state.load(Ordering::Acquire) {
            COUNTS_MAPPED => {
                if let Ok(view) = self.read.slice(..).get_mapped_range() {
                    let n = |at: usize| u32::from_le_bytes([0, 1, 2, 3].map(|b| view[at * 4 + b]));
                    self.latest = Some([n(0), n(1), n(2), n(3)]);
                }
                self.read.unmap();
                self.state.store(COUNTS_FREE, Ordering::Release);
            }
            COUNTS_FAILED => {
                self.read.unmap();
                self.state.store(COUNTS_FREE, Ordering::Release);
            }
            _ => {}
        }
    }
}

/// A buffer a pass writes on the GPU, of any size, and its copy for the CPU to read some
/// frames later, as [`GpuCounts`] reads its four counters: copied out after the pass, mapped
/// once the copy was submitted. Rendering's check of the probes reads its sums through one
/// (probe_gi, `ProbeCheck`).
pub(crate) struct GpuReadback {
    /// What the pass binds (`STORAGE`).
    pub(crate) buffer: wgpu::Buffer,
    read: wgpu::Buffer,
    size: u64,
    /// `COUNTS_*`.
    state: Arc<AtomicU8>,
    /// This frame's commands copy the buffer out: map the copy once they are submitted.
    copied: bool,
}

impl GpuReadback {
    pub(crate) fn new(device: &wgpu::Device, label: &str, size: u64) -> Self {
        let buffer = |label: &str, usage| {
            device.create_buffer(&wgpu::BufferDescriptor {
                label: Some(label),
                size,
                usage,
                mapped_at_creation: false,
            })
        };
        Self {
            buffer: buffer(
                label,
                wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
            ),
            read: buffer(
                &format!("{label} readback"),
                wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            ),
            size,
            state: Default::default(),
            copied: false,
        }
    }

    /// After the pass: copy the buffer out for the CPU, unless an earlier copy is still on its
    /// way (that frame's result is then not read).
    pub(crate) fn copy(&mut self, encoder: &mut wgpu::CommandEncoder) {
        if !self.copied && self.state.load(Ordering::Acquire) == COUNTS_FREE {
            encoder.copy_buffer_to_buffer(&self.buffer, 0, &self.read, 0, self.size);
            self.copied = true;
        }
    }

    /// What arrived since the last call, if anything; and ask for what last frame's commands
    /// copied out (they are submitted by now).
    pub(crate) fn take(&mut self) -> Option<Vec<u8>> {
        if std::mem::take(&mut self.copied) {
            self.state.store(COUNTS_MAPPING, Ordering::Release);
            let state = self.state.clone();
            self.read
                .slice(..)
                .map_async(wgpu::MapMode::Read, move |r| {
                    let to = if r.is_ok() {
                        COUNTS_MAPPED
                    } else {
                        COUNTS_FAILED
                    };
                    state.store(to, Ordering::Release);
                });
        }
        let mut arrived = None;
        match self.state.load(Ordering::Acquire) {
            COUNTS_MAPPED => {
                if let Ok(view) = self.read.slice(..).get_mapped_range() {
                    arrived = Some(view.to_vec());
                }
                self.read.unmap();
                self.state.store(COUNTS_FREE, Ordering::Release);
            }
            COUNTS_FAILED => {
                self.read.unmap();
                self.state.store(COUNTS_FREE, Ordering::Release);
            }
            _ => {}
        }
        arrived
    }
}
