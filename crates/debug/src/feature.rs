//! [`DebugLinesFeature`]: draws the lines in [`DebugDraw`] in the transparent phase.
//!
//! Each frame the `f64` world-space segments are converted to camera-relative `f32`
//! (`ViewData::origin`), written into one reused CPU vector and uploaded into one growing
//! vertex buffer. Two line-list pipelines draw them: depth-tested (reverse-Z `GreaterEqual`,
//! no depth write) and always on top (`Always`). Both alpha-blend into the HDR target.
//!
//! Lines lying exactly on a surface (a ground grid, a footprint) would z-fight with it at
//! grazing angles, so the depth-tested pipeline refines the hardware test in its fragment
//! shader against the scene depth copy (see `shaders/debug_lines.wgsl`). The renderer makes
//! that copy only on frames that have depth-tested lines.

use bevy_ecs::prelude::*;
use bytemuck::{Pod, Zeroable};
use engine_core::diagnostics::Diagnostics;
use engine_core::math::DVec3;
use engine_render::{
    FeatureStats, FrameContext, GpuShared, GrowBuffer, RenderFeature, RenderPhase, RenderSettings,
    ViewData,
};

use crate::draw::{DebugDraw, DepthMode};

/// Shader source (composed with `lib_types` + `lib_frame`).
pub const SHADER: &str = include_str!("shaders/debug_lines.wgsl");

/// One line endpoint on the GPU: camera-relative position (meters) and linear RGBA color.
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Pod, Zeroable)]
pub struct LineVertex {
    pub position: [f32; 3],
    pub color: [f32; 4],
}

impl LineVertex {
    const ATTRIBUTES: [wgpu::VertexAttribute; 2] =
        wgpu::vertex_attr_array![0 => Float32x3, 1 => Float32x4];

    pub fn layout() -> wgpu::VertexBufferLayout<'static> {
        wgpu::VertexBufferLayout {
            array_stride: std::mem::size_of::<Self>() as u64,
            step_mode: wgpu::VertexStepMode::Vertex,
            attributes: &Self::ATTRIBUTES,
        }
    }
}

/// Convert every line visible now into camera-relative vertices, depth-tested lines first.
/// Returns the vertex count of each [`DepthMode`] (indexed like [`DepthMode::ALL`]).
///
/// The subtraction happens in `f64`, so a line 1e8 m from the world origin is as precise as
/// one next to it, as long as the camera is nearby.
pub fn build_vertices(draw: &DebugDraw, origin: DVec3, out: &mut Vec<LineVertex>) -> [u32; 2] {
    out.clear();
    out.reserve(draw.line_count() * 2);
    let mut counts = [0u32; 2];
    for (i, depth) in DepthMode::ALL.into_iter().enumerate() {
        let start = out.len();
        for line in draw.lines(depth) {
            let a = (line.a - origin).as_vec3();
            let b = (line.b - origin).as_vec3();
            if !(a.is_finite() && b.is_finite()) {
                continue;
            }
            out.push(LineVertex {
                position: a.to_array(),
                color: line.color,
            });
            out.push(LineVertex {
                position: b.to_array(),
                color: line.color,
            });
        }
        counts[i] = (out.len() - start) as u32;
    }
    counts
}

/// Draws [`DebugDraw`] lines. Added by [`crate::DebugPlugin`].
#[derive(Default)]
pub struct DebugLinesFeature {
    /// Indexed like [`DepthMode::ALL`]: depth-tested, on top.
    pipelines: Vec<wgpu::RenderPipeline>,
    depth_layout: Option<wgpu::BindGroupLayout>,
    /// Scene depth copy for the depth-tested pipeline; rebuilt after a resize.
    depth_group: Option<wgpu::BindGroup>,
    vertices: Option<GrowBuffer>,
    scratch: Vec<LineVertex>,
    counts: [u32; 2],
    stats: FeatureStats,
}

impl RenderFeature for DebugLinesFeature {
    fn name(&self) -> &str {
        "debug_lines"
    }

    /// Late in the transparent phase, so lines are drawn over water, particles and clouds.
    fn order(&self) -> i32 {
        1000
    }

    fn init(&mut self, gpu: &GpuShared) {
        let device = &gpu.device;
        let shader = gpu.shader("debug lines", SHADER, true);
        let depth_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("debug lines scene depth"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Texture {
                    sample_type: wgpu::TextureSampleType::Depth,
                    view_dimension: wgpu::TextureViewDimension::D2,
                    multisampled: false,
                },
                count: None,
            }],
        });
        let tested_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("debug lines (depth tested)"),
            bind_group_layouts: &[Some(&gpu.frame_layout), Some(&depth_layout)],
            immediate_size: 0,
        });
        let on_top_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("debug lines (on top)"),
            bind_group_layouts: &[Some(&gpu.frame_layout)],
            immediate_size: 0,
        });
        let buffers = [Some(LineVertex::layout())];
        self.pipelines = DepthMode::ALL
            .into_iter()
            .map(|mode| {
                let (label, layout, compare, fragment) = match mode {
                    // Reverse-Z: greater is closer.
                    DepthMode::Tested => (
                        "debug lines (depth tested)",
                        &tested_layout,
                        wgpu::CompareFunction::GreaterEqual,
                        "fs_line_tested",
                    ),
                    DepthMode::OnTop => (
                        "debug lines (on top)",
                        &on_top_layout,
                        wgpu::CompareFunction::Always,
                        "fs_line",
                    ),
                };
                device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                    label: Some(label),
                    layout: Some(layout),
                    vertex: wgpu::VertexState {
                        module: &shader,
                        entry_point: Some("vs_line"),
                        compilation_options: Default::default(),
                        buffers: &buffers,
                    },
                    primitive: wgpu::PrimitiveState {
                        topology: wgpu::PrimitiveTopology::LineList,
                        cull_mode: None,
                        ..Default::default()
                    },
                    depth_stencil: Some(wgpu::DepthStencilState {
                        format: gpu.formats.depth,
                        depth_write_enabled: Some(false),
                        depth_compare: Some(compare),
                        stencil: Default::default(),
                        bias: Default::default(),
                    }),
                    multisample: Default::default(),
                    fragment: Some(wgpu::FragmentState {
                        module: &shader,
                        entry_point: Some(fragment),
                        compilation_options: Default::default(),
                        targets: &[Some(wgpu::ColorTargetState {
                            format: gpu.formats.hdr,
                            blend: Some(wgpu::BlendState::ALPHA_BLENDING),
                            write_mask: wgpu::ColorWrites::ALL,
                        })],
                    }),
                    multiview_mask: None,
                    cache: None,
                })
            })
            .collect();
        self.vertices = Some(GrowBuffer::new(
            device,
            "debug lines",
            wgpu::BufferUsages::VERTEX,
            16 * 1024 * std::mem::size_of::<LineVertex>() as u64,
        ));
        self.depth_layout = Some(depth_layout);
        self.depth_group = None;
    }

    fn resize(&mut self, _gpu: &GpuShared, _width: u32, _height: u32) {
        // The scene depth copy is recreated with the other size-dependent targets.
        self.depth_group = None;
    }

    fn prepare(
        &mut self,
        world: &mut World,
        gpu: &GpuShared,
        view: &ViewData,
        _settings: &RenderSettings,
    ) {
        self.counts = [0, 0];
        self.stats = FeatureStats::default();
        let Some(draw) = world.get_resource::<DebugDraw>() else {
            return;
        };
        if draw.enabled {
            self.counts = build_vertices(draw, view.origin, &mut self.scratch);
        } else {
            self.scratch.clear();
        }
        let (lines, dropped) = (self.scratch.len() / 2, draw.dropped());
        if let Some(mut diag) = world.get_resource_mut::<Diagnostics>() {
            diag.set("debug.lines", lines as f64);
            diag.set("debug.dropped_lines", dropped as f64);
        }
        if self.scratch.is_empty() {
            return;
        }
        if let Some(buffer) = &mut self.vertices {
            buffer.write(&gpu.device, &gpu.queue, bytemuck::cast_slice(&self.scratch));
        }
        // Line counts go to `describe()` and `Diagnostics`, not to the instance counter.
        self.stats.draw_calls = self.counts.iter().filter(|c| **c > 0).count() as u32;
    }

    fn compute(&mut self, _encoder: &mut wgpu::CommandEncoder, ctx: &FrameContext) {
        if self.counts[0] > 0
            && self.depth_group.is_none()
            && let Some(layout) = &self.depth_layout
        {
            self.depth_group = Some(
                ctx.gpu
                    .device
                    .create_bind_group(&wgpu::BindGroupDescriptor {
                        label: Some("debug lines scene depth"),
                        layout,
                        entries: &[wgpu::BindGroupEntry {
                            binding: 0,
                            resource: wgpu::BindingResource::TextureView(ctx.scene_depth_copy),
                        }],
                    }),
            );
        }
    }

    fn needs_scene_copy(&self) -> bool {
        self.counts[0] > 0
    }

    fn draw(&self, phase: RenderPhase, pass: &mut wgpu::RenderPass<'_>, _ctx: &FrameContext) {
        if phase != RenderPhase::Transparent || self.counts == [0, 0] {
            return;
        }
        let Some(buffer) = &self.vertices else {
            return;
        };
        pass.set_vertex_buffer(0, buffer.buffer.slice(..));
        let [tested, on_top] = self.counts;
        if tested > 0
            && let (Some(pipeline), Some(group)) = (self.pipelines.first(), &self.depth_group)
        {
            pass.set_pipeline(pipeline);
            pass.set_bind_group(1, group, &[]);
            pass.draw(0..tested, 0..1);
        }
        if on_top > 0
            && let Some(pipeline) = self.pipelines.get(1)
        {
            pass.set_pipeline(pipeline);
            pass.draw(tested..tested + on_top, 0..1);
        }
    }

    fn stats(&self) -> FeatureStats {
        self.stats
    }

    fn describe(&self) -> serde_json::Value {
        serde_json::json!({
            "depth_tested_lines": self.counts[0] / 2,
            "on_top_lines": self.counts[1] / 2,
            "buffer_bytes": self.vertices.as_ref().map(|b| b.capacity()),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::color::*;

    #[test]
    fn shader_validates() {
        let code = engine_render::compose_shader(SHADER, true);
        engine_render::validate_wgsl("debug_lines", &code).unwrap();
    }

    #[test]
    fn vertex_layout_matches_struct() {
        assert_eq!(std::mem::size_of::<LineVertex>(), 28);
        assert_eq!(LineVertex::layout().array_stride, 28);
    }

    #[test]
    fn vertices_are_camera_relative_and_precise_far_from_origin() {
        let mut draw = DebugDraw::default();
        draw.begin_frame(0.0);
        let base = DVec3::new(1.0e8, 2.0e3, -1.0e8);
        // A 1 mm segment and a box 100 million meters from the origin.
        draw.line(base, base + DVec3::new(0.001, 0.0, 0.0), RED);
        draw.aabb(base + DVec3::splat(1.0), base + DVec3::splat(1.25), GREEN)
            .on_top();
        let camera = base + DVec3::new(-3.0, 1.5, 4.0);
        let mut out = Vec::new();
        let counts = build_vertices(&draw, camera, &mut out);
        assert_eq!(counts, [2, 24]);
        let a = engine_core::math::Vec3::from(out[0].position);
        let b = engine_core::math::Vec3::from(out[1].position);
        assert!(
            (a - engine_core::math::Vec3::new(3.0, -1.5, -4.0)).length() < 1e-6,
            "{a:?}"
        );
        assert!(
            ((b - a).x - 0.001).abs() < 1e-6,
            "1 mm survives: {:?}",
            b - a
        );
        // Box corners are exactly 0.25 m apart even though the box is at 1e8 m.
        let xs: Vec<f32> = out[2..].iter().map(|v| v.position[0]).collect();
        let (min, max) = xs
            .iter()
            .fold((f32::MAX, f32::MIN), |(lo, hi), x| (lo.min(*x), hi.max(*x)));
        assert!((max - min - 0.25).abs() < 1e-5, "{min}..{max}");
        assert_eq!(out[2].color, GREEN);
    }

    #[test]
    fn non_finite_lines_are_skipped() {
        let mut draw = DebugDraw::default();
        draw.line(DVec3::ZERO, DVec3::new(f64::NAN, 0.0, 0.0), RED);
        draw.line(DVec3::ZERO, DVec3::X, RED);
        let mut out = Vec::new();
        assert_eq!(build_vertices(&draw, DVec3::ZERO, &mut out), [2, 0]);
    }
}
