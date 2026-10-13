//! [`MarksFeature`]: draws [`Marks`] as decals over the opaque scene (transparent phase,
//! right after the weather's ground pass), from `shaders/marks.wgsl`.

use bytemuck::{Pod, Zeroable};
use engine_core::math::Vec3;
use engine_core::prelude::*;
use engine_render::wgpu;
use engine_render::{
    FeatureStats, FrameContext, GpuShared, GrowBuffer, RenderFeature, RenderPhase, RenderSettings,
    ViewData,
};

use crate::emitter::VfxSettings;
use crate::marks::{Mark, Marks};

/// The decal shader (without the generated box table).
pub const SHADER_MARKS: &str = include_str!("shaders/marks.wgsl");

/// Marks farther than this from the camera are not drawn (the shader fades them out before).
const DRAW_DISTANCE: f64 = 95.0;

/// One mark on the GPU (96 bytes); matches `MarkInstance` in `shaders/marks.wgsl`.
#[repr(C)]
#[derive(Debug, Clone, Copy, Default, PartialEq, Pod, Zeroable)]
pub struct GpuMark {
    /// Camera-relative center, kind code.
    pub center: [f32; 4],
    /// Unit right, half width (m).
    pub right: [f32; 4],
    /// Unit forward, half length (m).
    pub forward: [f32; 4],
    /// Unit normal, half thickness of the projection box (m).
    pub normal: [f32; 4],
    /// Age (s), life (s), strength, seed.
    pub life: [f32; 4],
    /// Ember glow (s), unused.
    pub extra: [f32; 4],
}

impl GpuMark {
    /// `mark` relative to the camera at `origin`.
    pub fn new(mark: &Mark, origin: engine_core::math::DVec3) -> Self {
        let c = (mark.position - origin).as_vec3();
        let n = mark.normal.normalize_or(Vec3::Y);
        let f = crate::marks::along_surface(mark.forward, n);
        let r = f.cross(n).normalize_or(Vec3::X);
        let [hw, hl] = mark.half_size;
        let size = hw.max(hl);
        // Deep enough to reach uneven ground, thin enough to stay off whatever stands in it.
        let thickness = (size * 0.35).clamp(0.06, 0.6);
        Self {
            center: [c.x, c.y, c.z, mark.kind.code()],
            right: [r.x, r.y, r.z, hw.max(0.01)],
            forward: [f.x, f.y, f.z, hl.max(0.01)],
            normal: [n.x, n.y, n.z, thickness],
            life: [
                mark.age_secs,
                mark.life_secs.max(1e-3),
                mark.strength.clamp(0.0, 1.0),
                (mark.seed % 65_521) as f32,
            ],
            extra: [mark.glow_secs, 0.0, 0.0, 0.0],
        }
    }
}

/// Corner indices (bit 0: right, bit 1: forward, bit 2: normal) of the box's 12 triangles,
/// counter-clockwise seen from outside.
pub fn box_triangles() -> [u32; 36] {
    // Each face: the axis it faces along, its side, and its four corners in a ring.
    let mut out = [0u32; 36];
    let mut i = 0;
    for axis in 0..3u32 {
        for side in [0u32, 1] {
            let (a, b) = ((axis + 1) % 3, (axis + 2) % 3);
            let corner = |u: u32, v: u32| (side << axis) | (u << a) | (v << b);
            let ring = [corner(0, 0), corner(1, 0), corner(1, 1), corner(0, 1)];
            // (a, b, axis) is right-handed, so this ring is counter-clockwise seen from +axis.
            let ring = if side == 1 {
                ring
            } else {
                [ring[0], ring[3], ring[2], ring[1]]
            };
            for t in [[0, 1, 2], [0, 2, 3]] {
                for k in t {
                    out[i] = ring[k];
                    i += 1;
                }
            }
        }
    }
    out
}

/// The full decal shader source: the box table, then `marks.wgsl`.
pub fn marks_shader() -> String {
    let list = box_triangles()
        .iter()
        .map(|k| format!("{k}u"))
        .collect::<Vec<_>>()
        .join(", ");
    format!("const MARK_BOX: array<u32, 36> = array<u32, 36>({list});\n{SHADER_MARKS}")
}

/// Multiply blend: the destination times the shader's color (a ratio around 1).
const MULTIPLY: wgpu::BlendState = wgpu::BlendState {
    color: wgpu::BlendComponent {
        src_factor: wgpu::BlendFactor::Dst,
        dst_factor: wgpu::BlendFactor::Zero,
        operation: wgpu::BlendOperation::Add,
    },
    alpha: wgpu::BlendComponent {
        src_factor: wgpu::BlendFactor::Zero,
        dst_factor: wgpu::BlendFactor::One,
        operation: wgpu::BlendOperation::Add,
    },
};

/// Premultiplied over: alpha 0 adds light, alpha 1 covers.
const OVER: wgpu::BlendState = wgpu::BlendState {
    color: wgpu::BlendComponent {
        src_factor: wgpu::BlendFactor::One,
        dst_factor: wgpu::BlendFactor::OneMinusSrcAlpha,
        operation: wgpu::BlendOperation::Add,
    },
    alpha: wgpu::BlendComponent {
        src_factor: wgpu::BlendFactor::Zero,
        dst_factor: wgpu::BlendFactor::One,
        operation: wgpu::BlendOperation::Add,
    },
};

struct Gpu {
    shade: wgpu::RenderPipeline,
    cover: wgpu::RenderPipeline,
    layout: wgpu::BindGroupLayout,
    marks: GrowBuffer,
    group: Option<wgpu::BindGroup>,
    bound_size: (u32, u32),
}

/// Draws [`Marks`]. Added by [`crate::VfxPlugin`].
#[derive(Default)]
pub struct MarksFeature {
    gpu: Option<Gpu>,
    instances: Vec<GpuMark>,
    /// Instances drawn by the multiply pass, then by the over pass.
    shade: std::ops::Range<u32>,
    cover: std::ops::Range<u32>,
    stats: FeatureStats,
}

impl MarksFeature {
    /// Marks drawn this frame (multiply pass, over pass).
    pub fn drawn(&self) -> (u32, u32) {
        (self.shade.len() as u32, self.cover.len() as u32)
    }
}

impl RenderFeature for MarksFeature {
    fn name(&self) -> &str {
        "vfx_marks"
    }

    /// Just after the weather's ground pass (-1000), so marks land on its snow; before water
    /// surfaces and everything else that is see-through.
    fn order(&self) -> i32 {
        -900
    }

    fn init(&mut self, gpu: &GpuShared) {
        let device = &gpu.device;
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("vfx marks"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Storage { read_only: true },
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Depth,
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
            ],
        });
        let shader = gpu.shader("vfx marks", &marks_shader(), true);
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("vfx marks"),
            bind_group_layouts: &[Some(&gpu.frame_layout), Some(&layout)],
            immediate_size: 0,
        });
        // Every pixel inside a box, wherever the camera is: back faces, no depth test (the
        // shader tests the scene depth itself).
        let depth = wgpu::DepthStencilState {
            depth_compare: Some(wgpu::CompareFunction::Always),
            ..gpu.depth_state(false)
        };
        let pipeline = |label: &'static str, entry: &str, blend: wgpu::BlendState| {
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some(label),
                layout: Some(&pipeline_layout),
                vertex: wgpu::VertexState {
                    module: &shader,
                    entry_point: Some("vs_mark"),
                    compilation_options: Default::default(),
                    buffers: &[],
                },
                primitive: wgpu::PrimitiveState {
                    topology: wgpu::PrimitiveTopology::TriangleList,
                    front_face: wgpu::FrontFace::Ccw,
                    cull_mode: Some(wgpu::Face::Front),
                    ..Default::default()
                },
                depth_stencil: Some(depth.clone()),
                multisample: Default::default(),
                fragment: Some(wgpu::FragmentState {
                    module: &shader,
                    entry_point: Some(entry),
                    compilation_options: Default::default(),
                    targets: &[Some(wgpu::ColorTargetState {
                        format: gpu.formats.hdr,
                        blend: Some(blend),
                        write_mask: wgpu::ColorWrites::ALL,
                    })],
                }),
                multiview_mask: None,
                cache: None,
            })
        };
        self.gpu = Some(Gpu {
            shade: pipeline("vfx marks shade", "fs_mark_shade", MULTIPLY),
            cover: pipeline("vfx marks cover", "fs_mark_cover", OVER),
            layout,
            marks: GrowBuffer::new(
                device,
                "vfx marks",
                wgpu::BufferUsages::STORAGE,
                256 * std::mem::size_of::<GpuMark>() as u64,
            ),
            group: None,
            bound_size: (0, 0),
        });
    }

    fn resize(&mut self, _gpu: &GpuShared, _width: u32, _height: u32) {
        if let Some(g) = &mut self.gpu {
            g.group = None;
        }
    }

    fn prepare(
        &mut self,
        world: &mut World,
        gpu: &GpuShared,
        view: &ViewData,
        _settings: &RenderSettings,
    ) {
        self.instances.clear();
        self.shade = 0..0;
        self.cover = 0..0;
        self.stats = FeatureStats::default();
        let Some(g) = self.gpu.as_mut() else {
            return;
        };
        let enabled = world
            .get_resource::<VfxSettings>()
            .is_none_or(|s| s.enabled);
        let Some(marks) = world.get_resource::<Marks>() else {
            return;
        };
        if !enabled || marks.is_empty() || view.orthographic {
            return;
        }
        let visible = |m: &&Mark| {
            let reach = m.half_size[0].max(m.half_size[1]) as f64;
            let d = m.position - view.origin;
            d.length() < DRAW_DISTANCE + reach
                && view
                    .frustum
                    .intersects_sphere(d.as_vec3(), reach as f32 + 0.5)
        };
        for m in marks.iter().filter(visible).filter(|m| m.kind.shades()) {
            self.instances.push(GpuMark::new(m, view.origin));
        }
        let split = self.instances.len() as u32;
        for m in marks.iter().filter(visible).filter(|m| m.kind.covers()) {
            self.instances.push(GpuMark::new(m, view.origin));
        }
        let total = self.instances.len() as u32;
        self.shade = 0..split;
        self.cover = split..total;
        if total > 0
            && g.marks.write(
                &gpu.device,
                &gpu.queue,
                bytemuck::cast_slice(&self.instances),
            )
        {
            g.group = None;
        }
        self.stats = FeatureStats {
            draw_calls: u32::from(!self.shade.is_empty()) + u32::from(!self.cover.is_empty()),
            instances: total,
            triangles: total as u64 * 12,
            culled: marks.len() as u32 - total.min(marks.len() as u32),
        };
    }

    fn compute(&mut self, _encoder: &mut wgpu::CommandEncoder, ctx: &FrameContext) {
        let Some(g) = self.gpu.as_mut() else {
            return;
        };
        let size = (ctx.view.width, ctx.view.height);
        if g.bound_size != size {
            g.bound_size = size;
            g.group = None;
        }
        if g.group.is_none() {
            g.group = Some(
                ctx.gpu
                    .device
                    .create_bind_group(&wgpu::BindGroupDescriptor {
                        label: Some("vfx marks"),
                        layout: &g.layout,
                        entries: &[
                            wgpu::BindGroupEntry {
                                binding: 0,
                                resource: g.marks.buffer.as_entire_binding(),
                            },
                            wgpu::BindGroupEntry {
                                binding: 1,
                                resource: wgpu::BindingResource::TextureView(ctx.scene_depth_copy),
                            },
                        ],
                    }),
            );
        }
    }

    fn draw(&self, phase: RenderPhase, pass: &mut wgpu::RenderPass<'_>, _ctx: &FrameContext) {
        if phase != RenderPhase::Transparent {
            return;
        }
        let (Some(g), true) = (&self.gpu, !self.instances.is_empty()) else {
            return;
        };
        let Some(group) = &g.group else {
            return;
        };
        pass.set_bind_group(1, group, &[]);
        if !self.shade.is_empty() {
            pass.set_pipeline(&g.shade);
            pass.draw(0..36, self.shade.clone());
        }
        if !self.cover.is_empty() {
            pass.set_pipeline(&g.cover);
            pass.draw(0..36, self.cover.clone());
        }
    }

    fn needs_scene_copy(&self) -> bool {
        !self.instances.is_empty()
    }

    fn stats(&self) -> FeatureStats {
        self.stats
    }

    fn describe(&self) -> serde_json::Value {
        serde_json::json!({
            "shaded": self.shade.len(),
            "covered": self.cover.len(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn box_faces_point_outward() {
        let corner = |k: u32| {
            Vec3::new(
                (k & 1) as f32 * 2.0 - 1.0,
                ((k >> 1) & 1) as f32 * 2.0 - 1.0,
                ((k >> 2) & 1) as f32 * 2.0 - 1.0,
            )
        };
        let tris = box_triangles();
        for t in tris.chunks(3) {
            let [a, b, c] = [corner(t[0]), corner(t[1]), corner(t[2])];
            let n = (b - a).cross(c - a);
            let center = (a + b + c) / 3.0;
            assert!(n.dot(center) > 0.0, "triangle {t:?} faces inward");
        }
        // Every corner is used, every face has two triangles.
        let mut used = [0; 8];
        for k in tris {
            used[k as usize] += 1;
        }
        assert!(used.iter().all(|&n| n > 0));
    }

    #[test]
    fn mark_layout_matches_the_shader() {
        assert_eq!(std::mem::size_of::<GpuMark>(), 96);
        let code = engine_render::compose_shader(&marks_shader(), true);
        engine_render::validate_wgsl("marks", &code).unwrap();
    }
}
