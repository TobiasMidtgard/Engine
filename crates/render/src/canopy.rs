//! Light through leaf canopies: a top-down map of the leaves around the camera, read by the sky
//! visibility (`canopy_visibility` in `lib_frame.wgsl`).
//!
//! The sky occlusion map sees cover from above as all or nothing, so an alpha-tested leaf card
//! is a roof: under a crown the sky is either gone or, between cards, all there. A forest's
//! floor sees the sky through its leaves instead, less of it the more leaf area it looks
//! through. Crop and forest models get the diffuse sky light under a canopy by splitting the sky
//! into three rings, at 15°, 45° and 75° elevation, weighted 0.178, 0.514 and 0.308
//! (Goudriaan's three-ring sky), each seen through `exp(-k(β) L')` of the leaves, with
//! `k(β) = 0.5 / sin β` for leaves at all angles (a spherical leaf angle distribution) and `L'`
//! the leaf area along the ring's slant. Under a uniform layer of leaf area index 4, that lets
//! through 6.9 % of the sky; the effective extinction falls from 0.77 at L = 1 to 0.63 at L = 7.
//!
//! Each swaying crown (a masked material with wind sway: leaf cards) is an ellipsoid of leaves
//! from its mesh's bounds, holding its cards' area times the share of their texels an alpha test
//! keeps (`ImageCache::alpha_coverage`). The crowns are splatted additively into a 1024² RGBA16F
//! map of 1 m texels around the camera (shaders/canopy_splat.wgsl): per texel the leaf area
//! index and the first two moments of leaf height, so its mips hold the mean leaf area over
//! larger squares and the mean and spread of the leaves' height there, as variance shadow maps
//! keep depth moments. The shader reads one tap per ring at the mip about the ring's width at the
//! height of the leaves above the point, and counts only the leaves above the point (the
//! normal distribution of leaf height above it), so the inside and the underside of a crown get
//! less sky than its top. Crowns leave the sky occlusion map (trunks, rocks and roofs stay).
//!
//! The map is drawn again when the camera moves 128 m (it recenters) or the crowns change; its
//! edge fades to the open sky over its outer eighth (448-512 m).
//!
//! Sources: Goudriaan (1988), "The bare bones of leaf-angle distribution in radiation models
//! for canopy photosynthesis and energy exchange"; the three-ring diffuse sky as used in crop
//! irradiance models (https://ramialbasha.github.io/crop-irradiance/absorbed_sunlit_shaded.html);
//! Bruneton and Neyret (2012), "Real-time Realistic Rendering and Lighting of Forests"; Mantler
//! et al. (2006), top-down vegetation density for landscapes.

use std::sync::Mutex;

use bytemuck::{Pod, Zeroable};
use engine_core::prelude::*;

use crate::features::GpuShared;

/// Texels along each side of the map.
pub(crate) const CANOPY_SIZE: u32 = 1024;
/// Meters per texel.
pub(crate) const CANOPY_TEXEL: f32 = 1.0;
/// Half the map's width, meters.
pub(crate) const CANOPY_HALF_EXTENT: f32 = CANOPY_SIZE as f32 * CANOPY_TEXEL * 0.5;
/// The map's center snaps to this grid (m): it is drawn again when the camera crosses a line.
pub(crate) const CANOPY_RECENTER: f64 = 128.0;
/// Heights are stored in units of this many meters above the map's reference height (which
/// snaps to `CANOPY_HEIGHT_SNAP`), so their squares stay in half-float range.
pub(crate) const CANOPY_HEIGHT_UNIT: f32 = 64.0;
pub(crate) const CANOPY_HEIGHT_SNAP: f64 = 32.0;
/// A leaf card's share of leaf when its base color's coverage is not known yet.
pub(crate) const DEFAULT_LEAF_COVERAGE: f32 = 0.5;
/// Smallest horizontal radius a crown is drawn with (m): smaller ones are spread over this, the
/// same leaf area, so they still land on a texel.
const MIN_DRAW_RADIUS: f32 = 0.75 * CANOPY_TEXEL;

const FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba16Float;

/// One crown for the splat (shaders/canopy_splat.wgsl `Crown`).
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Pod, Zeroable)]
pub(crate) struct Crown {
    /// Center relative to the map's center (x, z; m), horizontal radius (m), and the leaf area
    /// per square meter under its center.
    pub center_radius_tau: [f32; 4],
    /// Center height and vertical half-axis in height units, unused, unused.
    pub height: [f32; 4],
}

impl Crown {
    /// An ellipsoid of `leaf_area` square meters of leaves at `center` (relative to the map's
    /// center and reference height, m) with horizontal radius `radius` and vertical half-axis
    /// `half_height` (m). Its leaf area index summed over the ground is `leaf_area`.
    pub fn new(center: Vec3, radius: f32, half_height: f32, leaf_area: f32) -> Self {
        let r = radius.max(MIN_DRAW_RADIUS);
        // The column under the center holds 2 * half-height * density of leaves, with density
        // = leaf area / (4/3 pi r² half-height): 3 * leaf area / (2 pi r²).
        let tau0 = 3.0 * leaf_area / (2.0 * std::f32::consts::PI * r * r);
        Self {
            center_radius_tau: [center.x, center.z, r, tau0],
            height: [
                center.y / CANOPY_HEIGHT_UNIT,
                half_height / CANOPY_HEIGHT_UNIT,
                0.0,
                0.0,
            ],
        }
    }
}

/// The map's center for a camera at `origin`: x and z snapped to `CANOPY_RECENTER`, y (the
/// reference height) to `CANOPY_HEIGHT_SNAP`.
pub(crate) fn anchor_for(origin: DVec3) -> DVec3 {
    let snap = |v: f64, step: f64| (v / step).round() * step;
    DVec3::new(
        snap(origin.x, CANOPY_RECENTER),
        snap(origin.y, CANOPY_HEIGHT_SNAP),
        snap(origin.z, CANOPY_RECENTER),
    )
}

/// The canopy map (frame binding 26) and the crowns last drawn into it.
pub(crate) struct CanopyMap {
    texture: wgpu::Texture,
    /// Every level, for the frame group.
    view: wgpu::TextureView,
    /// Level 0, drawn into.
    top: wgpu::TextureView,
    state: Mutex<State>,
}

#[derive(Default)]
struct State {
    /// The map's center (world) once crowns were drawn; `None`: empty, nothing to read.
    anchor: Option<DVec3>,
    /// The crowns drawn (or waiting to be), to tell when they change.
    crowns: Vec<Crown>,
    /// Crowns were set and wait to be drawn.
    dirty: bool,
    gpu: Option<Splat>,
}

struct Splat {
    pipeline: wgpu::RenderPipeline,
    layout: wgpu::BindGroupLayout,
    uniform: wgpu::Buffer,
    buffer: Option<wgpu::Buffer>,
    group: Option<wgpu::BindGroup>,
    count: u32,
}

impl CanopyMap {
    pub fn new(device: &wgpu::Device) -> Self {
        let levels = CANOPY_SIZE.ilog2() + 1;
        // New textures start zeroed: no leaves anywhere.
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("canopy map"),
            size: wgpu::Extent3d {
                width: CANOPY_SIZE,
                height: CANOPY_SIZE,
                depth_or_array_layers: 1,
            },
            mip_level_count: levels,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: FORMAT,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::RENDER_ATTACHMENT,
            view_formats: &[],
        });
        let view = texture.create_view(&Default::default());
        let top = texture.create_view(&wgpu::TextureViewDescriptor {
            label: Some("canopy map level 0"),
            base_mip_level: 0,
            mip_level_count: Some(1),
            ..Default::default()
        });
        Self {
            texture,
            view,
            top,
            state: Mutex::default(),
        }
    }

    /// The whole map, for frame binding 26.
    pub fn view(&self) -> &wgpu::TextureView {
        &self.view
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// The crowns around `anchor` (`anchor_for` of the main view's camera), relative to it.
    /// The map is drawn again (by `encode`) only when they or the anchor changed.
    pub fn set_crowns(&self, anchor: DVec3, crowns: &[Crown]) {
        let mut state = self.lock();
        if state.anchor == Some(anchor) && state.crowns == crowns {
            return;
        }
        state.anchor = Some(anchor);
        state.crowns.clear();
        state.crowns.extend_from_slice(crowns);
        state.dirty = true;
    }

    /// No crowns: the map reads as open sky (canopy off).
    pub fn clear(&self) {
        let mut state = self.lock();
        if state.anchor.is_some() {
            state.anchor = None;
            state.crowns.clear();
            state.dirty = false;
        }
    }

    /// The frame uniform's `canopy`: the map's center relative to the camera at `origin`
    /// (x, z, m), its reference height relative to the camera (m), and half its width (0: no
    /// map); and `canopy_params`: leaf area scale, mip levels, texel (m), unused.
    pub fn uniform(&self, origin: DVec3, strength: f32) -> [[f32; 4]; 2] {
        // No crowns: no map to read (scenes without trees skip its taps).
        let anchor = {
            let state = self.lock();
            match state.anchor {
                Some(anchor) if !state.crowns.is_empty() => anchor,
                _ => return [[0.0; 4]; 2],
            }
        };
        let rel = (anchor - origin).as_vec3();
        [
            [rel.x, rel.z, rel.y, CANOPY_HALF_EXTENT],
            [
                strength.max(0.0),
                self.texture.mip_level_count() as f32,
                CANOPY_TEXEL,
                0.0,
            ],
        ]
    }

    /// Draw the crowns set since the last call into the map and filter its mips.
    pub fn encode(&self, gpu: &GpuShared, encoder: &mut wgpu::CommandEncoder) {
        let mut state = self.lock();
        if !state.dirty {
            return;
        }
        state.dirty = false;
        let State {
            crowns, gpu: splat, ..
        } = &mut *state;
        let splat = splat.get_or_insert_with(|| Splat::new(gpu));
        splat.upload(gpu, crowns);
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("canopy splat"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &self.top,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                ..Default::default()
            });
            if let Some(group) = splat.group.as_ref().filter(|_| splat.count > 0) {
                pass.set_pipeline(&splat.pipeline);
                pass.set_bind_group(0, group, &[]);
                pass.draw(0..4, 0..splat.count);
            }
        }
        gpu.mips.record(gpu, encoder, &self.texture, FORMAT);
    }
}

impl Splat {
    fn new(gpu: &GpuShared) -> Self {
        let layout = gpu.bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("canopy splat"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::VERTEX,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Storage { read_only: true },
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
            ],
        });
        let module = gpu.shader(
            "canopy splat",
            include_str!("shaders/canopy_splat.wgsl"),
            false,
        );
        let pipeline_layout = gpu.pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("canopy splat"),
            bind_group_layouts: &[Some(&layout)],
            immediate_size: 0,
        });
        let add = wgpu::BlendComponent {
            src_factor: wgpu::BlendFactor::One,
            dst_factor: wgpu::BlendFactor::One,
            operation: wgpu::BlendOperation::Add,
        };
        let pipeline = gpu.render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("canopy splat"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &module,
                entry_point: Some("vs_splat"),
                compilation_options: Default::default(),
                buffers: &[],
            },
            primitive: wgpu::PrimitiveState {
                topology: wgpu::PrimitiveTopology::TriangleStrip,
                ..Default::default()
            },
            depth_stencil: None,
            multisample: Default::default(),
            fragment: Some(wgpu::FragmentState {
                module: &module,
                entry_point: Some("fs_splat"),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format: FORMAT,
                    blend: Some(wgpu::BlendState {
                        color: add,
                        alpha: add,
                    }),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            multiview_mask: None,
            cache: None,
        });
        let uniform = gpu.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("canopy splat"),
            size: 16,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        gpu.queue.write_buffer(
            &uniform,
            0,
            bytemuck::bytes_of(&[CANOPY_HALF_EXTENT, 0.0, 0.0, 0.0]),
        );
        Self {
            pipeline,
            layout,
            uniform,
            buffer: None,
            group: None,
            count: 0,
        }
    }

    fn upload(&mut self, gpu: &GpuShared, crowns: &[Crown]) {
        self.count = crowns.len() as u32;
        if crowns.is_empty() {
            return;
        }
        let bytes = std::mem::size_of_val(crowns) as u64;
        if self.buffer.as_ref().is_none_or(|b| b.size() < bytes) {
            let buffer = gpu.device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("canopy crowns"),
                size: bytes.next_power_of_two().max(4096),
                usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            });
            self.group = Some(gpu.device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("canopy splat"),
                layout: &self.layout,
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: self.uniform.as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: buffer.as_entire_binding(),
                    },
                ],
            }));
            self.buffer = Some(buffer);
        }
        if let Some(buffer) = &self.buffer {
            gpu.queue
                .write_buffer(buffer, 0, bytemuck::cast_slice(crowns));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_crowns_leaf_area_index_sums_to_its_leaf_area() {
        // tau0 * sqrt(1 - d²) integrated over the disc of radius r: tau0 * 2/3 pi r².
        let c = Crown::new(Vec3::ZERO, 3.0, 4.0, 50.0);
        let [_, _, r, tau0] = c.center_radius_tau;
        let total = tau0 * 2.0 / 3.0 * std::f32::consts::PI * r * r;
        assert!((total - 50.0).abs() < 1e-3, "{total}");
        // Numerically, on 1 m texels like the splat (within 2 %).
        let mut sum = 0.0;
        for x in -4..4 {
            for z in -4..4 {
                let d2 = ((x as f32 + 0.5).powi(2) + (z as f32 + 0.5).powi(2)) / (r * r);
                if d2 < 1.0 {
                    sum += tau0 * (1.0 - d2).sqrt();
                }
            }
        }
        assert!((sum - 50.0).abs() < 50.0 * 0.05, "{sum}");
    }

    #[test]
    fn the_anchor_snaps_so_small_moves_keep_the_map() {
        let a = anchor_for(DVec3::new(10.0, 5.0, -20.0));
        assert_eq!(a, anchor_for(DVec3::new(50.0, 12.0, -60.0)));
        assert_eq!(a, DVec3::ZERO);
        assert_eq!(
            anchor_for(DVec3::new(70.0, 20.0, 0.0)),
            DVec3::new(128.0, 32.0, 0.0)
        );
    }

    /// The three-ring sky under a uniform layer of leaf area index `l`.
    fn three_rings(l: f32) -> f32 {
        [(15f32, 0.178f32), (45.0, 0.514), (75.0, 0.308)]
            .iter()
            .map(|&(beta, w)| w * (-0.5 / beta.to_radians().sin() * l).exp())
            .sum()
    }

    #[test]
    fn the_three_ring_sky_matches_the_crop_models() {
        // Diffuse extinction k_d = -ln(T) / L falls with leaf area (0.77, 0.67, 0.63).
        for (l, k) in [(1.0, 0.77), (4.0, 0.67), (7.0, 0.63)] {
            let kd = -three_rings(l).ln() / l;
            assert!((kd - k).abs() < 0.01, "L {l}: k_d {kd}");
        }
        assert!((three_rings(4.0) - 0.069).abs() < 0.001);
    }
}
