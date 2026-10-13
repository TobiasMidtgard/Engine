//! Auto-exposure metering: a GPU luminance histogram of the HDR image, read back a frame or two
//! later without stalling, turned into a measured EV100.
//!
//! The histogram is center-weighted and averages the upper-middle of the distribution (it
//! ignores the darkest 55% and the brightest 3% of pixels), so a few lamps at night don't darken
//! the image, while black space or deep shadow don't wash out what is lit. Near-black pixels
//! (below `BLACK_LOG2`) are not metered at all: a sunlit planet or moon against space exposes
//! for the lit surface, and a view of almost nothing but black has no reading (the renderer
//! then exposes for the sun).

use std::sync::Arc;
use std::sync::atomic::{AtomicU8, Ordering};

use bytemuck::{Pod, Zeroable};

use crate::features::GpuShared;

/// Histogram bins over the luminance range below.
pub const BINS: usize = 128;
/// log2 of the darkest metered luminance (cd/m²): 2^-12 ≈ 0.0002, starlight.
pub const MIN_LOG2: f32 = -12.0;
/// Stops covered by the histogram (up to 2^22 ≈ 4 million cd/m²).
pub const RANGE_LOG2: f32 = 34.0;
/// Fractions of the weighted pixel population ignored at the dark and bright ends.
pub const LOW_PERCENT: f32 = 0.55;
pub const HIGH_PERCENT: f32 = 0.97;
/// Pixels darker than 2^this cd/m² (≈ 0.002) count as black and aren't metered.
pub const BLACK_LOG2: f32 = -9.0;
/// Below this fraction of metered (non-black) pixels there is no reading.
pub const MIN_COVERAGE: f64 = 0.02;
/// How much brighter than the meter reading auto exposure sets the image, in EV. The meter
/// averages the brighter part of the image, which outdoors is mostly sky; like a photographer
/// exposing for the land rather than the sky, the exposure opens up from there. Calibrated on
/// the showcase: a sunny day then lands near the photographic "sunny 16" (EV100 ≈ 15).
pub const METER_BIAS_EV: f32 = 1.5;
/// Pixels are sampled on a grid with this spacing (in pixels) to keep metering cheap.
const STRIDE: u32 = 4;
const WORKGROUP: u32 = 16;
const READBACKS: usize = 3;

const FREE: u8 = 0;
const IN_FLIGHT: u8 = 1;
const MAPPING: u8 = 2;
const MAPPED: u8 = 3;
const FAILED: u8 = 4;

#[repr(C)]
#[derive(Debug, Clone, Copy, Pod, Zeroable)]
struct MeterParams {
    /// min log2, 1 / range log2, 1 / pre-exposure, stride
    a: [f32; 4],
    /// width, height, bins, unused
    b: [u32; 4],
}

struct Readback {
    buffer: wgpu::Buffer,
    state: Arc<AtomicU8>,
}

/// Owns the histogram pass and its readbacks.
pub struct ExposureMeter {
    pipeline: wgpu::ComputePipeline,
    layout: wgpu::BindGroupLayout,
    params: wgpu::Buffer,
    histogram: wgpu::Buffer,
    group: Option<wgpu::BindGroup>,
    readbacks: Vec<Readback>,
    /// Readback written by this frame's encoder, waiting for `after_submit`.
    pending: Option<usize>,
    /// Latest measurement (EV100 of the metered average, without compensation).
    pub measured_ev100: Option<f32>,
}

impl ExposureMeter {
    pub fn new(shared: &GpuShared) -> Self {
        let device = &shared.device;
        let module = shared.shader(
            "exposure histogram",
            include_str!("shaders/exposure.wgsl"),
            false,
        );
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("exposure meter"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::COMPUTE,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: false },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::COMPUTE,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 2,
                    visibility: wgpu::ShaderStages::COMPUTE,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Storage { read_only: false },
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
            ],
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("exposure meter"),
            bind_group_layouts: &[Some(&layout)],
            immediate_size: 0,
        });
        let pipeline = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
            label: Some("exposure histogram"),
            layout: Some(&pipeline_layout),
            module: &module,
            entry_point: Some("cs_histogram"),
            compilation_options: Default::default(),
            cache: None,
        });
        let params = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("exposure params"),
            size: std::mem::size_of::<MeterParams>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let size = (BINS * 4) as u64;
        let histogram = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("exposure histogram"),
            size,
            usage: wgpu::BufferUsages::STORAGE
                | wgpu::BufferUsages::COPY_SRC
                | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let readbacks = (0..READBACKS)
            .map(|_| Readback {
                buffer: device.create_buffer(&wgpu::BufferDescriptor {
                    label: Some("exposure readback"),
                    size,
                    usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
                    mapped_at_creation: false,
                }),
                state: Arc::new(AtomicU8::new(FREE)),
            })
            .collect();
        Self {
            pipeline,
            layout,
            params,
            histogram,
            group: None,
            readbacks,
            pending: None,
            measured_ev100: None,
        }
    }

    /// Rebind after the HDR target changes size.
    pub fn set_source(&mut self, device: &wgpu::Device, hdr: &wgpu::TextureView) {
        self.group = Some(device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("exposure meter"),
            layout: &self.layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(hdr),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: self.params.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: self.histogram.as_entire_binding(),
                },
            ],
        }));
    }

    /// Record the histogram pass for this frame's HDR image (call after all scene drawing).
    pub fn encode(
        &mut self,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        width: u32,
        height: u32,
        pre_exposure: f32,
    ) {
        let Some(group) = &self.group else { return };
        let Some(slot) = self
            .readbacks
            .iter()
            .position(|r| r.state.load(Ordering::Acquire) == FREE)
        else {
            return; // all readbacks still in flight: skip metering this frame
        };
        let params = MeterParams {
            a: [
                MIN_LOG2,
                1.0 / RANGE_LOG2,
                1.0 / pre_exposure.max(1e-12),
                STRIDE as f32,
            ],
            b: [width, height, BINS as u32, 0],
        };
        queue.write_buffer(&self.params, 0, bytemuck::bytes_of(&params));
        encoder.clear_buffer(&self.histogram, 0, None);
        {
            let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
                label: Some("exposure histogram"),
                timestamp_writes: None,
            });
            pass.set_pipeline(&self.pipeline);
            pass.set_bind_group(0, group, &[]);
            let gx = width.div_ceil(STRIDE).div_ceil(WORKGROUP);
            let gy = height.div_ceil(STRIDE).div_ceil(WORKGROUP);
            pass.dispatch_workgroups(gx, gy, 1);
        }
        let rb = &self.readbacks[slot];
        encoder.copy_buffer_to_buffer(&self.histogram, 0, &rb.buffer, 0, (BINS * 4) as u64);
        rb.state.store(IN_FLIGHT, Ordering::Release);
        self.pending = Some(slot);
    }

    /// Start mapping this frame's readback (call right after `queue.submit`).
    pub fn after_submit(&mut self) {
        let Some(slot) = self.pending.take() else {
            return;
        };
        let rb = &self.readbacks[slot];
        rb.state.store(MAPPING, Ordering::Release);
        let state = rb.state.clone();
        rb.buffer
            .slice(..)
            .map_async(wgpu::MapMode::Read, move |r| {
                state.store(if r.is_ok() { MAPPED } else { FAILED }, Ordering::Release);
            });
    }

    /// Collect finished readbacks (non-blocking) and update `measured_ev100`.
    pub fn collect(&mut self, device: &wgpu::Device) {
        let _ = device.poll(wgpu::PollType::Poll);
        for rb in &self.readbacks {
            match rb.state.load(Ordering::Acquire) {
                MAPPED => {
                    let bins: Vec<u32> = match rb.buffer.slice(..).get_mapped_range() {
                        Ok(view) => bytemuck::cast_slice(&view).to_vec(),
                        Err(_) => Vec::new(),
                    };
                    rb.buffer.unmap();
                    rb.state.store(FREE, Ordering::Release);
                    if let Some(ev) = ev100_from_histogram(&bins) {
                        self.measured_ev100 = Some(ev);
                    }
                }
                FAILED => {
                    rb.buffer.unmap();
                    rb.state.store(FREE, Ordering::Release);
                }
                _ => {}
            }
        }
    }
}

/// Metered EV100 from histogram bins: the weighted average log-luminance between the
/// `LOW_PERCENT` and `HIGH_PERCENT` points of the distribution.
pub fn ev100_from_histogram(bins: &[u32]) -> Option<f32> {
    let n = bins.len() as f32;
    let first = (((BLACK_LOG2 - MIN_LOG2) / RANGE_LOG2) * n).ceil().max(0.0) as usize;
    let everything: f64 = bins.iter().map(|&b| b as f64).sum();
    let bins = &bins[first.min(bins.len())..];
    let total: f64 = bins.iter().map(|&b| b as f64).sum();
    if total <= 0.0 || total < everything * MIN_COVERAGE {
        return None;
    }
    let lo = total * LOW_PERCENT as f64;
    let hi = total * HIGH_PERCENT as f64;
    let (mut seen, mut sum, mut weight) = (0.0f64, 0.0f64, 0.0f64);
    for (i, &b) in bins.iter().enumerate() {
        let count = b as f64;
        // Part of this bin inside [lo, hi].
        let start = seen.max(lo);
        let end = (seen + count).min(hi);
        seen += count;
        if end > start {
            let log2_l = MIN_LOG2 + ((first + i) as f32 + 0.5) / n * RANGE_LOG2;
            sum += log2_l as f64 * (end - start);
            weight += end - start;
        }
    }
    if weight <= 0.0 {
        return None;
    }
    let avg_log2 = (sum / weight) as f32;
    // EV100 = log2(L * S / K) with S = 100, K = 12.5.
    Some(avg_log2 + (100.0f32 / 12.5).log2())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bin_for(l: f32) -> usize {
        (((l.log2() - MIN_LOG2) / RANGE_LOG2) * BINS as f32) as usize
    }

    #[test]
    fn uniform_image_meters_its_luminance() {
        let mut bins = vec![0u32; BINS];
        bins[bin_for(1000.0)] = 5000;
        let ev = ev100_from_histogram(&bins).unwrap();
        let expected = (1000.0f32 * 8.0).log2();
        assert!((ev - expected).abs() < 0.3, "{ev} vs {expected}");
    }

    #[test]
    fn a_few_bright_lamps_do_not_dominate_a_dark_scene() {
        let mut bins = vec![0u32; BINS];
        bins[bin_for(0.05)] = 9700; // moonlit ground
        bins[bin_for(20_000.0)] = 300; // lamps (3%)
        let ev = ev100_from_histogram(&bins).unwrap();
        assert!(ev < 1.0, "night metered too bright: {ev}");
    }

    #[test]
    fn black_space_does_not_wash_out_a_lit_planet() {
        let mut bins = vec![0u32; BINS];
        bins[0] = 6000; // space
        bins[bin_for(5000.0)] = 4000; // sunlit planet
        let ev = ev100_from_histogram(&bins).unwrap();
        let expected = (5000.0f32 * 8.0).log2();
        assert!((ev - expected).abs() < 0.3, "{ev} vs {expected}");
    }

    #[test]
    fn almost_only_black_has_no_reading() {
        let mut bins = vec![0u32; BINS];
        bins[0] = 10_000;
        bins[bin_for(0.1)] = 50; // a few stars
        assert!(ev100_from_histogram(&bins).is_none());
    }

    #[test]
    fn empty_histogram_has_no_reading() {
        assert!(ev100_from_histogram(&vec![0; BINS]).is_none());
    }
}
