//! Exact counts of the meshes' shader work in the main view, for measuring cuts
//! ([`shade_skip::COUNT_MESH_RUNS`]): how many times the vertex program and the fragment
//! program ran, and how many triangles reached the rasterizer, in the meshes' draws of the
//! prepass, the late prepass and the scene pass. The GPU counts them (pipeline statistics
//! queries), so the numbers are the same kind on any card, and `render.stats` shows them under
//! the meshes' `mesh_runs`.
//!
//! What they answer: the prepass's fragment runs over the pixels the meshes end up covering is
//! how many layers of material the prepass works out a pixel; the scene pass's is how often a
//! pixel is shaded (a triangle that touches a 2x2 block of pixels runs the program for all
//! four).
//!
//! [`shade_skip::COUNT_MESH_RUNS`]: crate::settings::shade_skip::COUNT_MESH_RUNS

use crate::features::GpuShared;
use std::sync::Arc;
use std::sync::atomic::{AtomicU8, AtomicU32, Ordering};

/// The meshes' draws in the prepass (the shared one, or their own with
/// `trial_cut::MESHES_ONCE`).
pub(crate) const PREPASS: u32 = 0;
/// Their draws in the late prepass.
pub(crate) const LATE: u32 = 1;
/// Their draws in the scene pass's opaque phase.
pub(crate) const SCENE: u32 = 2;
const QUERIES: u32 = 3;
/// Numbers a query: vertex runs, triangles out of the clipper, fragment runs.
const WORDS: usize = 3;
/// Bytes from one query's numbers to the next in the buffers: a query is resolved at a
/// multiple of `QUERY_RESOLVE_BUFFER_ALIGNMENT`.
const STRIDE: u64 = wgpu::QUERY_RESOLVE_BUFFER_ALIGNMENT;
const BYTES: u64 = QUERIES as u64 * STRIDE;

const FREE: u8 = 0;
const IN_FLIGHT: u8 = 1;
const MAPPING: u8 = 2;
const MAPPED: u8 = 3;
const FAILED: u8 = 4;

struct Readback {
    buffer: wgpu::Buffer,
    state: Arc<AtomicU8>,
    /// The queries it holds (bits).
    holds: u32,
}

pub(crate) struct MeshCounts {
    set: wgpu::QuerySet,
    resolve: wgpu::Buffer,
    readbacks: Vec<Readback>,
    /// The readback this frame's commands write: mapped once they are submitted.
    written: Option<usize>,
    /// Queries begun this frame (bits).
    used: AtomicU32,
    /// The latest numbers read back, by query; zero for a pass that drew no meshes.
    latest: [[u64; WORDS]; QUERIES as usize],
}

impl MeshCounts {
    /// None on a device without pipeline statistics queries.
    pub(crate) fn new(gpu: &GpuShared) -> Option<Self> {
        if !gpu
            .features
            .contains(wgpu::Features::PIPELINE_STATISTICS_QUERY)
        {
            return None;
        }
        let device = &gpu.device;
        let set = device.create_query_set(&wgpu::QuerySetDescriptor {
            label: Some("mesh runs"),
            ty: wgpu::QueryType::PipelineStatistics(
                wgpu::PipelineStatisticsTypes::VERTEX_SHADER_INVOCATIONS
                    | wgpu::PipelineStatisticsTypes::CLIPPER_PRIMITIVES_OUT
                    | wgpu::PipelineStatisticsTypes::FRAGMENT_SHADER_INVOCATIONS,
            ),
            count: QUERIES,
        });
        let resolve = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("mesh runs"),
            size: BYTES,
            usage: wgpu::BufferUsages::QUERY_RESOLVE | wgpu::BufferUsages::COPY_SRC,
            mapped_at_creation: false,
        });
        let readbacks = (0..3)
            .map(|_| Readback {
                buffer: device.create_buffer(&wgpu::BufferDescriptor {
                    label: Some("mesh runs readback"),
                    size: BYTES,
                    usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
                    mapped_at_creation: false,
                }),
                state: Arc::new(AtomicU8::new(FREE)),
                holds: 0,
            })
            .collect();
        Some(Self {
            set,
            resolve,
            readbacks,
            written: None,
            used: AtomicU32::new(0),
            latest: [[0; WORDS]; QUERIES as usize],
        })
    }

    /// Start of a frame's commands (last frame's are submitted): map what last frame wrote
    /// and take in what has arrived.
    pub(crate) fn begin_frame(&mut self) {
        self.used.store(0, Ordering::Release);
        if let Some(slot) = self.written.take() {
            let rb = &self.readbacks[slot];
            rb.state.store(MAPPING, Ordering::Release);
            let state = rb.state.clone();
            rb.buffer
                .slice(..)
                .map_async(wgpu::MapMode::Read, move |r| {
                    state.store(if r.is_ok() { MAPPED } else { FAILED }, Ordering::Release);
                });
        }
        for rb in &self.readbacks {
            match rb.state.load(Ordering::Acquire) {
                MAPPED => {
                    if let Ok(view) = rb.buffer.slice(..).get_mapped_range() {
                        let words: &[u64] = bytemuck::cast_slice(&view);
                        for q in 0..QUERIES as usize {
                            for w in 0..WORDS {
                                self.latest[q][w] = if rb.holds & (1 << q) != 0 {
                                    words[q * (STRIDE as usize / 8) + w]
                                } else {
                                    0
                                };
                            }
                        }
                    }
                    rb.buffer.unmap();
                    rb.state.store(FREE, Ordering::Release);
                }
                FAILED => {
                    rb.buffer.unmap();
                    rb.state.store(FREE, Ordering::Release);
                }
                _ => {}
            }
        }
    }

    /// Count the draws that follow in `pass` as `query` (once a frame each). True when
    /// counting began: call [`end`](Self::end) on the same pass after the draws.
    pub(crate) fn begin(&self, pass: &mut wgpu::RenderPass<'_>, query: u32) -> bool {
        let bit = 1 << query;
        if self.used.fetch_or(bit, Ordering::AcqRel) & bit != 0 {
            return false;
        }
        pass.begin_pipeline_statistics_query(&self.set, query);
        true
    }

    pub(crate) fn end(pass: &mut wgpu::RenderPass<'_>) {
        pass.end_pipeline_statistics_query();
    }

    /// After the frame's last pass that counts: copy this frame's numbers out.
    pub(crate) fn end_frame(&mut self, encoder: &mut wgpu::CommandEncoder) {
        let used = self.used.swap(0, Ordering::AcqRel);
        if used == 0 || self.written.is_some() {
            return;
        }
        let Some(slot) = self
            .readbacks
            .iter()
            .position(|r| r.state.load(Ordering::Acquire) == FREE)
        else {
            return;
        };
        for q in (0..QUERIES).filter(|q| used & (1 << q) != 0) {
            encoder.resolve_query_set(&self.set, q..q + 1, &self.resolve, u64::from(q) * STRIDE);
        }
        let rb = &mut self.readbacks[slot];
        encoder.copy_buffer_to_buffer(&self.resolve, 0, &rb.buffer, 0, BYTES);
        rb.holds = used;
        rb.state.store(IN_FLIGHT, Ordering::Release);
        self.written = Some(slot);
    }

    /// For `render.stats`.
    pub(crate) fn describe(&self) -> serde_json::Value {
        let row = |q: u32| {
            let [vertex, triangles, fragment] = self.latest[q as usize];
            serde_json::json!({
                "vertex_runs": vertex,
                "triangles": triangles,
                "fragment_runs": fragment,
            })
        };
        serde_json::json!({
            "prepass": row(PREPASS),
            "prepass_late": row(LATE),
            "scene": row(SCENE),
        })
    }
}
