//! Sky: physically based atmosphere with sun, stars and a cloud layer, plus the environment
//! map every lit surface uses for ambient light and reflections.
//!
//! The volumetric cloud feature replaces the simple cloud layer later; it draws into the same
//! phases (sky + environment), so nothing else changes.

use crate::features::{FrameContext, GpuShared, RenderFeature, RenderPhase};

#[derive(Default)]
pub struct SkyFeature {
    sky: Option<wgpu::RenderPipeline>,
    env: Option<wgpu::RenderPipeline>,
}

impl RenderFeature for SkyFeature {
    fn name(&self) -> &str {
        "sky"
    }

    fn order(&self) -> i32 {
        -100
    }

    fn init(&mut self, gpu: &GpuShared) {
        let device = &gpu.device;
        let shader = gpu.shader("sky", include_str!("shaders/sky.wgsl"), true);
        let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("sky"),
            bind_group_layouts: &[Some(&gpu.frame_layout)],
            immediate_size: 0,
        });
        let pipeline = |label: &str, vs: &str, fs: &str, format: wgpu::TextureFormat, depth: Option<wgpu::DepthStencilState>| {
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some(label),
                layout: Some(&layout),
                vertex: wgpu::VertexState {
                    module: &shader,
                    entry_point: Some(vs),
                    compilation_options: Default::default(),
                    buffers: &[],
                },
                primitive: Default::default(),
                depth_stencil: depth,
                multisample: Default::default(),
                fragment: Some(wgpu::FragmentState {
                    module: &shader,
                    entry_point: Some(fs),
                    compilation_options: Default::default(),
                    targets: &[Some(wgpu::ColorTargetState {
                        format,
                        blend: None,
                        write_mask: wgpu::ColorWrites::ALL,
                    })],
                }),
                multiview_mask: None,
                cache: None,
            })
        };
        // The sky is drawn at the far plane (depth 0 in reverse-Z) where nothing else was drawn.
        self.sky = Some(pipeline("sky", "vs_sky", "fs_sky", gpu.formats.hdr, Some(gpu.depth_state(false))));
        self.env = Some(pipeline("environment", "vs_env", "fs_env", gpu.formats.env, None));
    }

    fn draw(&self, phase: RenderPhase, pass: &mut wgpu::RenderPass<'_>, _ctx: &FrameContext) {
        if phase == RenderPhase::Sky
            && let Some(p) = &self.sky
        {
            pass.set_pipeline(p);
            pass.draw(0..3, 0..1);
        }
    }

    fn draw_environment(&self, pass: &mut wgpu::RenderPass<'_>, _ctx: &FrameContext) {
        if let Some(p) = &self.env {
            pass.set_pipeline(p);
            pass.draw(0..3, 0..1);
        }
    }
}
