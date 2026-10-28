//! Screen-space effects built on the depth prepass: ground-truth ambient occlusion (GTAO),
//! screen-space reflections (SSR), screen-space global illumination (SSGI) and temporal
//! anti-aliasing (TAA).
//!
//! ```text
//! prepass (depth, normals, motion) → copy depth → GTAO → edge-aware blur → mip chain of the
//! previous frame's image → SSR (traces the prepass depth, gathers from the previous frame's
//! image and its mips) → SSGI trace (same inputs) → temporal blend
//! → blur → [scene shading reads the AO, SSR and GI] … transparent → TAA resolve into the
//! history → copy back to the HDR target (without TAA, the HDR image is copied into the history
//! for next frame's reflections and bounce light)
//! ```

use bytemuck::{Pod, Zeroable};
use engine_core::prelude::*;

use crate::features::GpuShared;
use crate::renderer::{fullscreen_pass, fullscreen_pipeline, layout_entry};
use crate::targets::{Targets, Tex};

/// Jitter sequence length (Halton 2, 3).
pub const JITTER_SAMPLES: u64 = 8;
/// Smallest weight of the current frame once the history has converged.
const TAA_MIN_ALPHA: f32 = 0.1;
/// Levels of the blurred previous-frame chain for glossy reflections (half resolution and down).
const SSR_CHAIN_LEVELS: u32 = 6;

/// Sub-pixel jitter (pixels, in −0.5..0.5) for frame `index`: the Halton (2, 3) sequence.
pub fn jitter(index: u64) -> Vec2 {
    let i = (index % JITTER_SAMPLES) + 1;
    Vec2::new(halton(i, 2) - 0.5, halton(i, 3) - 0.5)
}

/// Element `i` of the Halton low-discrepancy sequence in `base` (0..1).
pub(crate) fn halton(mut i: u64, base: u64) -> f32 {
    let mut f = 1.0f32;
    let mut r = 0.0f32;
    while i > 0 {
        f /= base as f32;
        r += f * (i % base) as f32;
        i /= base;
    }
    r
}

#[repr(C)]
#[derive(Debug, Clone, Copy, Pod, Zeroable)]
struct TaaUniform {
    params: [f32; 4],
}

/// GPU resources of the screen-space effects. Bind groups follow the targets (rebuilt on
/// resize).
pub struct ScreenEffects {
    ao_layout: wgpu::BindGroupLayout,
    gtao: wgpu::RenderPipeline,
    blur: wgpu::RenderPipeline,
    gtao_group: wgpu::BindGroup,
    blur_group: wgpu::BindGroup,
    ssr_layout: wgpu::BindGroupLayout,
    ssr: wgpu::RenderPipeline,
    /// SSR input groups, indexed by the history written this frame (they read the other one).
    ssr_groups: [wgpu::BindGroup; 2],
    /// Downsampling into the blurred mip chain glossy reflections gather from.
    chain_layout: wgpu::BindGroupLayout,
    chain_pipeline: wgpu::RenderPipeline,
    chain: ReflectionChain,
    gi_layout: wgpu::BindGroupLayout,
    gi_trace: wgpu::RenderPipeline,
    gi_temporal: wgpu::RenderPipeline,
    gi_blur_near: wgpu::RenderPipeline,
    gi_blur: wgpu::RenderPipeline,
    /// GI groups: trace (indexed like `ssr_groups`), temporal and blur (indexed by the GI
    /// history written this frame).
    gi_groups: GiGroups,
    /// GI history slot written this frame.
    gi_current: usize,
    /// Whether the GI history holds last frame's bounce light. False after a frame without it
    /// (a camera cut, GI off), when the history still holds an older view's.
    gi_history_ready: bool,
    taa_layout: wgpu::BindGroupLayout,
    taa: wgpu::RenderPipeline,
    taa_params: wgpu::Buffer,
    /// TAA input groups, indexed by the history written this frame.
    taa_groups: [wgpu::BindGroup; 2],
    /// The reactive mask: where the transparent phase drew something thin (taa_reactive.wgsl).
    reactive_layout: wgpu::BindGroupLayout,
    reactive: wgpu::RenderPipeline,
    reactive_group: wgpu::BindGroup,
    /// History slot written this frame.
    current: usize,
    /// Frames blended into the history since the last reset.
    accumulated: u32,
    /// Whether the history holds a previous frame of the same size.
    history_ready: bool,
}

impl ScreenEffects {
    pub fn new(shared: &GpuShared, targets: &Targets) -> Self {
        use wgpu::ShaderStages as S;
        let device = &shared.device;
        let float_tex = |binding| {
            layout_entry(
                binding,
                S::FRAGMENT,
                wgpu::BindingType::Texture {
                    sample_type: wgpu::TextureSampleType::Float { filterable: true },
                    view_dimension: wgpu::TextureViewDimension::D2,
                    multisampled: false,
                },
            )
        };
        let depth_tex = |binding| {
            layout_entry(
                binding,
                S::FRAGMENT,
                wgpu::BindingType::Texture {
                    sample_type: wgpu::TextureSampleType::Depth,
                    view_dimension: wgpu::TextureViewDimension::D2,
                    multisampled: false,
                },
            )
        };
        let ao_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("ambient occlusion inputs"),
            entries: &[float_tex(0), float_tex(1), depth_tex(2)],
        });
        let ssr_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("screen-space reflection inputs"),
            entries: &[
                float_tex(0),
                depth_tex(1),
                float_tex(2),
                float_tex(3),
                float_tex(4),
                float_tex(5),
            ],
        });
        let chain_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("reflection chain input"),
            entries: &[
                float_tex(0),
                layout_entry(
                    1,
                    S::FRAGMENT,
                    wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                ),
            ],
        });
        let gi_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("screen-space gi inputs"),
            entries: &[
                float_tex(0),
                depth_tex(1),
                float_tex(2),
                float_tex(3),
                float_tex(4),
                float_tex(5),
            ],
        });
        let taa_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("taa inputs"),
            entries: &[
                float_tex(0),
                float_tex(1),
                float_tex(2),
                depth_tex(3),
                layout_entry(
                    4,
                    S::FRAGMENT,
                    wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                ),
                float_tex(5),
            ],
        });
        let reactive_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("taa reactive inputs"),
            entries: &[float_tex(0), float_tex(1)],
        });
        let gtao_module = shared.shader("gtao", include_str!("shaders/gtao.wgsl"), true);
        let taa_module = shared.shader("taa", include_str!("shaders/taa.wgsl"), true);
        let ao_layouts = [&shared.frame_layout, &ao_layout];
        let gtao = fullscreen_pipeline(
            shared,
            "gtao",
            &gtao_module,
            "vs_ao",
            "fs_gtao",
            &ao_layouts,
            shared.formats.ao,
            None,
        );
        let blur = fullscreen_pipeline(
            shared,
            "gtao blur",
            &gtao_module,
            "vs_ao",
            "fs_blur",
            &ao_layouts,
            shared.formats.ao,
            None,
        );
        let ssr_module = shared.shader("ssr", include_str!("shaders/ssr.wgsl"), true);
        let ssr = fullscreen_pipeline(
            shared,
            "screen-space reflections",
            &ssr_module,
            "vs_ssr",
            "fs_ssr",
            &[&shared.frame_layout, &ssr_layout],
            shared.formats.hdr,
            None,
        );
        let chain_module =
            shared.shader("ssr_chain", include_str!("shaders/ssr_chain.wgsl"), false);
        let chain_pipeline = fullscreen_pipeline(
            shared,
            "reflection chain",
            &chain_module,
            "vs_chain",
            "fs_chain",
            &[&chain_layout],
            shared.formats.hdr,
            None,
        );
        let gi_module = shared.shader("ssgi", include_str!("shaders/ssgi.wgsl"), true);
        let gi_pipeline = |label, entry| {
            fullscreen_pipeline(
                shared,
                label,
                &gi_module,
                "vs_gi",
                entry,
                &[&shared.frame_layout, &gi_layout],
                shared.formats.hdr,
                None,
            )
        };
        let gi_trace = gi_pipeline("screen-space gi trace", "fs_trace");
        let gi_temporal = gi_pipeline("screen-space gi temporal", "fs_temporal");
        let gi_blur_near = gi_pipeline("screen-space gi blur near", "fs_blur_near");
        let gi_blur = gi_pipeline("screen-space gi blur", "fs_blur");
        let taa = fullscreen_pipeline(
            shared,
            "taa resolve",
            &taa_module,
            "vs_taa",
            "fs_taa",
            &[&shared.frame_layout, &taa_layout],
            shared.formats.hdr,
            None,
        );
        let reactive_module = shared.shader(
            "taa_reactive",
            include_str!("shaders/taa_reactive.wgsl"),
            false,
        );
        let reactive = fullscreen_pipeline(
            shared,
            "taa reactive mask",
            &reactive_module,
            "vs_reactive",
            "fs_reactive",
            &[&reactive_layout],
            shared.formats.ao,
            None,
        );
        let taa_params = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("taa params"),
            size: std::mem::size_of::<TaaUniform>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let (gtao_group, blur_group, taa_groups) =
            Self::groups(shared, targets, &ao_layout, &taa_layout, &taa_params);
        let chain = ReflectionChain::new(shared, targets, &chain_layout);
        let ssr_groups = Self::ssr_groups(shared, targets, &ssr_layout, &chain.view);
        let gi_groups = GiGroups::new(shared, targets, &gi_layout);
        let reactive_group = Self::reactive_group(shared, targets, &reactive_layout);
        Self {
            ao_layout,
            gtao,
            blur,
            gtao_group,
            blur_group,
            ssr_layout,
            ssr,
            ssr_groups,
            chain_layout,
            chain_pipeline,
            chain,
            gi_layout,
            gi_trace,
            gi_temporal,
            gi_blur_near,
            gi_blur,
            gi_groups,
            gi_current: 0,
            gi_history_ready: false,
            taa_layout,
            taa,
            taa_params,
            taa_groups,
            reactive_layout,
            reactive,
            reactive_group,
            current: 0,
            accumulated: 0,
            history_ready: false,
        }
    }

    fn reactive_group(
        shared: &GpuShared,
        t: &Targets,
        layout: &wgpu::BindGroupLayout,
    ) -> wgpu::BindGroup {
        shared.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("taa reactive inputs"),
            layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(&t.hdr.view),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::TextureView(&t.color_copy.view),
                },
            ],
        })
    }

    #[allow(clippy::type_complexity)]
    fn groups(
        shared: &GpuShared,
        t: &Targets,
        ao_layout: &wgpu::BindGroupLayout,
        taa_layout: &wgpu::BindGroupLayout,
        taa_params: &wgpu::Buffer,
    ) -> (wgpu::BindGroup, wgpu::BindGroup, [wgpu::BindGroup; 2]) {
        let device = &shared.device;
        fn view(v: &wgpu::TextureView) -> wgpu::BindingResource<'_> {
            wgpu::BindingResource::TextureView(v)
        }
        let ao_group = |label: &str, input: &wgpu::TextureView| {
            device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some(label),
                layout: ao_layout,
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: view(input),
                    },
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: view(&t.prepass_normal.view),
                    },
                    wgpu::BindGroupEntry {
                        binding: 2,
                        resource: view(&t.prepass_depth.view),
                    },
                ],
            })
        };
        // The main pass doesn't read binding 0; any float texture that isn't its target will do.
        let gtao_group = ao_group("gtao inputs", &t.ao.view);
        let blur_group = ao_group("gtao blur inputs", &t.ao_raw.view);
        let taa_groups = [0, 1].map(|current| {
            device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("taa inputs"),
                layout: taa_layout,
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: view(&t.hdr.view),
                    },
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: view(&t.history[1 - current].view),
                    },
                    wgpu::BindGroupEntry {
                        binding: 2,
                        resource: view(&t.motion.view),
                    },
                    wgpu::BindGroupEntry {
                        binding: 3,
                        resource: view(&t.depth.view),
                    },
                    wgpu::BindGroupEntry {
                        binding: 4,
                        resource: taa_params.as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 5,
                        resource: view(&t.reactive.view),
                    },
                ],
            })
        });
        (gtao_group, blur_group, taa_groups)
    }

    fn ssr_groups(
        shared: &GpuShared,
        t: &Targets,
        layout: &wgpu::BindGroupLayout,
        chain: &wgpu::TextureView,
    ) -> [wgpu::BindGroup; 2] {
        fn view(v: &wgpu::TextureView) -> wgpu::BindingResource<'_> {
            wgpu::BindingResource::TextureView(v)
        }
        [0, 1].map(|current| {
            shared.device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("screen-space reflection inputs"),
                layout,
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: view(&t.prepass_normal.view),
                    },
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: view(&t.prepass_depth.view),
                    },
                    wgpu::BindGroupEntry {
                        binding: 2,
                        resource: view(&t.motion.view),
                    },
                    wgpu::BindGroupEntry {
                        binding: 3,
                        resource: view(&t.history[1 - current].view),
                    },
                    wgpu::BindGroupEntry {
                        binding: 4,
                        resource: view(chain),
                    },
                    wgpu::BindGroupEntry {
                        binding: 5,
                        resource: view(&t.world_reflection.view),
                    },
                ],
            })
        })
    }

    /// The targets were recreated: rebuild bind groups and drop the history.
    pub fn resize(&mut self, shared: &GpuShared, targets: &Targets) {
        let (gtao_group, blur_group, taa_groups) = Self::groups(
            shared,
            targets,
            &self.ao_layout,
            &self.taa_layout,
            &self.taa_params,
        );
        self.gtao_group = gtao_group;
        self.blur_group = blur_group;
        self.taa_groups = taa_groups;
        self.reactive_group = Self::reactive_group(shared, targets, &self.reactive_layout);
        self.chain = ReflectionChain::new(shared, targets, &self.chain_layout);
        self.ssr_groups = Self::ssr_groups(shared, targets, &self.ssr_layout, &self.chain.view);
        self.gi_groups = GiGroups::new(shared, targets, &self.gi_layout);
        self.history_ready = false;
        self.gi_history_ready = false;
        self.accumulated = 0;
    }

    /// Forget the history (camera cut, anti-aliasing turned off).
    pub fn reset_history(&mut self) {
        self.history_ready = false;
        self.gi_history_ready = false;
        self.accumulated = 0;
    }

    /// Note a frame rendered without bounce light: the GI history now lags behind, so the next
    /// `encode_gi` starts it over.
    pub fn skip_gi(&mut self) {
        self.gi_history_ready = false;
    }

    /// Record ambient occlusion: GTAO into `ao_raw`, blurred into `ao`. `prepass_group` is the
    /// frame group whose screen-space maps are stand-ins (the passes write the real ones).
    pub fn encode_ao(
        &self,
        encoder: &mut wgpu::CommandEncoder,
        targets: &Targets,
        prepass_group: &wgpu::BindGroup,
    ) {
        {
            let mut pass = fullscreen_pass(encoder, "gtao", &targets.ao_raw.view, true);
            pass.set_pipeline(&self.gtao);
            pass.set_bind_group(0, prepass_group, &[]);
            pass.set_bind_group(1, &self.gtao_group, &[]);
            pass.draw(0..3, 0..1);
        }
        let mut pass = fullscreen_pass(encoder, "gtao blur", &targets.ao.view, true);
        pass.set_pipeline(&self.blur);
        pass.set_bind_group(0, prepass_group, &[]);
        pass.set_bind_group(1, &self.blur_group, &[]);
        pass.draw(0..3, 0..1);
    }

    /// Whether the history holds the previous frame's image (for screen-space reflections).
    pub fn has_previous(&self) -> bool {
        self.history_ready
    }

    /// Record screen-space reflections into `targets.ssr` from the prepass and the previous
    /// frame's image (`has_previous` must be true), after blurring that image into a mip chain
    /// for glossy reflections. `prepass_group` as for `encode_ao`.
    pub fn encode_ssr(
        &self,
        encoder: &mut wgpu::CommandEncoder,
        targets: &Targets,
        prepass_group: &wgpu::BindGroup,
    ) {
        for (level, target) in self.chain.levels.iter().enumerate() {
            let group = match level {
                0 => &self.chain.first[self.current],
                _ => &self.chain.rest[level - 1],
            };
            let mut pass = fullscreen_pass(encoder, "reflection chain", target, true);
            pass.set_pipeline(&self.chain_pipeline);
            pass.set_bind_group(0, group, &[]);
            pass.draw(0..3, 0..1);
        }
        let mut pass =
            fullscreen_pass(encoder, "screen-space reflections", &targets.ssr.view, true);
        pass.set_pipeline(&self.ssr);
        pass.set_bind_group(0, prepass_group, &[]);
        pass.set_bind_group(1, &self.ssr_groups[self.current], &[]);
        pass.draw(0..3, 0..1);
    }

    /// Record screen-space global illumination into `targets.gi`: trace from the prepass into
    /// the previous frame's image, blend with the GI history, blur. Same requirements as
    /// `encode_ssr`.
    pub fn encode_gi(
        &mut self,
        encoder: &mut wgpu::CommandEncoder,
        targets: &Targets,
        prepass_group: &wgpu::BindGroup,
    ) {
        let g = self.gi_current;
        if !self.gi_history_ready {
            // The history read this frame is from before a frame without bounce light, such as
            // the view before a camera cut; where its depths happen to match it would ghost in
            // (a daylit tree over a dark cave). Depth 0 in alpha disables the blend.
            fullscreen_pass(
                encoder,
                "screen-space gi history reset",
                &targets.gi_history[1 - g].view,
                true,
            );
        }
        let passes = [
            (
                "screen-space gi trace",
                &self.gi_trace,
                &self.gi_groups.trace[self.current],
                &targets.gi_raw.view,
            ),
            (
                "screen-space gi temporal",
                &self.gi_temporal,
                &self.gi_groups.temporal[g],
                &targets.gi_history[g].view,
            ),
            // The trace's target is free again: the near blur goes there, and the wide blur
            // reads it through the temporal pass's inputs.
            (
                "screen-space gi blur near",
                &self.gi_blur_near,
                &self.gi_groups.blur[g],
                &targets.gi_raw.view,
            ),
            (
                "screen-space gi blur",
                &self.gi_blur,
                &self.gi_groups.temporal[g],
                &targets.gi.view,
            ),
        ];
        for (label, pipeline, group, out) in passes {
            let mut pass = fullscreen_pass(encoder, label, out, true);
            pass.set_pipeline(pipeline);
            pass.set_bind_group(0, prepass_group, &[]);
            pass.set_bind_group(1, group, &[]);
            pass.draw(0..3, 0..1);
        }
        self.gi_current = 1 - g;
        self.gi_history_ready = true;
    }

    /// Without TAA: keep the finished HDR image as the previous frame for next frame's
    /// reflections and bounce light.
    pub fn keep_frame(&mut self, encoder: &mut wgpu::CommandEncoder, targets: &Targets) {
        encoder.copy_texture_to_texture(
            targets.hdr.texture.as_image_copy(),
            targets.history[self.current].texture.as_image_copy(),
            wgpu::Extent3d {
                width: targets.width,
                height: targets.height,
                depth_or_array_layers: 1,
            },
        );
        self.current = 1 - self.current;
        self.history_ready = true;
    }

    /// Record the TAA resolve of `targets.hdr` into the history and copy the result back into
    /// `targets.hdr`. `history_valid` is false after a camera cut.
    /// Mark where the transparent phase just drew something thin, from the scene before it
    /// (`targets.color_copy`) and after (`targets.hdr`), for `encode_taa`.
    pub fn encode_reactive(&self, encoder: &mut wgpu::CommandEncoder, targets: &Targets) {
        let mut pass = fullscreen_pass(encoder, "taa reactive mask", &targets.reactive.view, true);
        pass.set_pipeline(&self.reactive);
        pass.set_bind_group(0, &self.reactive_group, &[]);
        pass.draw(0..3, 0..1);
    }

    /// `reactive`: how much the mask from `encode_reactive` this frame counts (0 when it
    /// wasn't drawn).
    pub fn encode_taa(
        &mut self,
        shared: &GpuShared,
        encoder: &mut wgpu::CommandEncoder,
        targets: &Targets,
        scene_group: &wgpu::BindGroup,
        history_valid: bool,
        reactive: f32,
    ) {
        if !history_valid || !self.history_ready {
            self.accumulated = 0;
        }
        let valid = history_valid && self.history_ready;
        let u = TaaUniform {
            params: [
                if valid { 1.0 } else { 0.0 },
                self.accumulated as f32,
                TAA_MIN_ALPHA,
                reactive,
            ],
        };
        shared
            .queue
            .write_buffer(&self.taa_params, 0, bytemuck::bytes_of(&u));
        let out = &targets.history[self.current];
        {
            let mut pass = fullscreen_pass(encoder, "taa resolve", &out.view, true);
            pass.set_pipeline(&self.taa);
            pass.set_bind_group(0, scene_group, &[]);
            pass.set_bind_group(1, &self.taa_groups[self.current], &[]);
            pass.draw(0..3, 0..1);
        }
        encoder.copy_texture_to_texture(
            out.texture.as_image_copy(),
            targets.hdr.texture.as_image_copy(),
            wgpu::Extent3d {
                width: targets.width,
                height: targets.height,
                depth_or_array_layers: 1,
            },
        );
        self.current = 1 - self.current;
        self.accumulated = (self.accumulated + 1).min(255);
        self.history_ready = true;
    }
}

/// Bind groups of the screen-space GI passes.
/// The previous frame's image at half resolution and blurred further down a mip chain, which
/// glossy screen-space reflections gather from (see ssr_chain.wgsl).
struct ReflectionChain {
    _texture: wgpu::Texture,
    /// All levels, for sampling.
    view: wgpu::TextureView,
    /// One view per level, for drawing into.
    levels: Vec<wgpu::TextureView>,
    /// Input of level 0, indexed by the history written this frame (they read the other one).
    first: [wgpu::BindGroup; 2],
    /// Input of level `i + 1`: level `i`.
    rest: Vec<wgpu::BindGroup>,
}

impl ReflectionChain {
    fn new(shared: &GpuShared, t: &Targets, layout: &wgpu::BindGroupLayout) -> Self {
        let width = t.width.div_ceil(2).max(1);
        let height = t.height.div_ceil(2).max(1);
        let count = SSR_CHAIN_LEVELS.min(width.min(height).ilog2() + 1);
        let texture = shared.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("reflection chain"),
            size: wgpu::Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
            mip_level_count: count,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: shared.formats.hdr,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        });
        let view = texture.create_view(&Default::default());
        let levels: Vec<_> = (0..count)
            .map(|level| {
                texture.create_view(&wgpu::TextureViewDescriptor {
                    label: Some("reflection chain level"),
                    base_mip_level: level,
                    mip_level_count: Some(1),
                    ..Default::default()
                })
            })
            .collect();
        let group = |input: &wgpu::TextureView| {
            shared.device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("reflection chain input"),
                layout,
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: wgpu::BindingResource::TextureView(input),
                    },
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: wgpu::BindingResource::Sampler(&shared.linear_clamp),
                    },
                ],
            })
        };
        let first = [0, 1].map(|current| group(&t.history[1 - current].view));
        let rest = levels[..levels.len() - 1].iter().map(group).collect();
        Self {
            _texture: texture,
            view,
            levels,
            first,
            rest,
        }
    }
}

struct GiGroups {
    /// Indexed by the TAA history written this frame (they read the other one).
    trace: [wgpu::BindGroup; 2],
    /// Indexed by the GI history written this frame.
    temporal: [wgpu::BindGroup; 2],
    blur: [wgpu::BindGroup; 2],
}

impl GiGroups {
    fn new(shared: &GpuShared, t: &Targets, layout: &wgpu::BindGroupLayout) -> Self {
        // Bindings: prepass normal, depth, motion, previous frame, GI input, GI history. Each
        // pass reads only some of them; the rest get any texture it doesn't write.
        let group = |label, previous: &Tex, input: &Tex, history: &Tex| {
            let views = [
                &t.prepass_normal.view,
                &t.prepass_depth.view,
                &t.motion.view,
                &previous.view,
                &input.view,
                &history.view,
            ];
            let entries: Vec<_> = views
                .iter()
                .enumerate()
                .map(|(i, v)| wgpu::BindGroupEntry {
                    binding: i as u32,
                    resource: wgpu::BindingResource::TextureView(v),
                })
                .collect();
            shared.device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some(label),
                layout,
                entries: &entries,
            })
        };
        Self {
            trace: [0, 1].map(|current| {
                group(
                    "screen-space gi trace inputs",
                    &t.history[1 - current],
                    &t.gi,
                    &t.gi_history[0],
                )
            }),
            temporal: [0, 1].map(|g| {
                group(
                    "screen-space gi temporal inputs",
                    &t.history[0],
                    &t.gi_raw,
                    &t.gi_history[1 - g],
                )
            }),
            blur: [0, 1].map(|g| {
                group(
                    "screen-space gi blur inputs",
                    &t.history[0],
                    &t.gi_history[g],
                    &t.gi_history[1 - g],
                )
            }),
        }
    }
}

/// Whether the camera jumped so far between two frames that the previous image is useless (a
/// teleport or a cut to another camera), as opposed to fast but continuous motion such as a
/// spaceship at 10 km/s. `prev_step` is the camera's movement the frame before.
///
/// A camera that stood still or crept along cuts once it jumps [`CUT_STEP_M`]: tour stops and
/// gallery shots are often only 10 to 20 m apart and face nearly the same way, and keeping the
/// last stop's history there smears a ghost of it over the new view.
pub fn is_camera_cut(
    prev_origin: DVec3,
    prev_rotation: Quat,
    origin: DVec3,
    rotation: Quat,
    prev_step: f64,
) -> bool {
    let step = (origin - prev_origin).length();
    let turned = prev_rotation.angle_between(rotation);
    turned > 45f32.to_radians() || step > CUT_STEP_M.max(4.0 * prev_step)
}

/// Smallest jump [`is_camera_cut`] calls a cut (meters in one frame).
pub const CUT_STEP_M: f64 = 5.0;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn jitter_covers_the_pixel_evenly() {
        let samples: Vec<Vec2> = (0..JITTER_SAMPLES).map(jitter).collect();
        for s in &samples {
            assert!(s.x.abs() <= 0.5 && s.y.abs() <= 0.5, "{s}");
            assert!(*s != Vec2::ZERO, "a zero jitter would look like TAA is off");
        }
        let mean = samples.iter().copied().sum::<Vec2>() / samples.len() as f32;
        assert!(mean.length() < 0.1, "biased jitter {mean}");
        // Quadrants are all visited.
        for (sx, sy) in [(1.0, 1.0), (1.0, -1.0), (-1.0, 1.0), (-1.0, -1.0)] {
            assert!(
                samples.iter().any(|s| s.x * sx > 0.0 && s.y * sy > 0.0),
                "quadrant {sx},{sy} never sampled: {samples:?}"
            );
        }
    }

    #[test]
    fn cuts_are_jumps_not_fast_motion() {
        let q = Quat::IDENTITY;
        let a = DVec3::ZERO;
        // Walking, driving, a spaceship cruising at 10 km/s (167 m per frame).
        assert!(!is_camera_cut(a, q, DVec3::new(0.1, 0.0, 0.0), q, 0.1));
        assert!(!is_camera_cut(a, q, DVec3::new(167.0, 0.0, 0.0), q, 166.0));
        // A sprint at a few frames per second, a car's camera speeding up.
        assert!(!is_camera_cut(a, q, DVec3::new(4.0, 0.0, 0.0), q, 0.0));
        assert!(!is_camera_cut(a, q, DVec3::new(11.0, 0.0, 0.0), q, 3.0));
        // Teleport and a cut to a camera facing the other way.
        assert!(is_camera_cut(a, q, DVec3::new(500.0, 0.0, 0.0), q, 0.0));
        assert!(is_camera_cut(a, q, a, Quat::from_rotation_y(3.0), 0.0));
        // The next tour stop, 15 m along and turned 30 degrees, from a still camera.
        let turn = Quat::from_rotation_y(30f32.to_radians());
        assert!(is_camera_cut(a, q, DVec3::new(15.0, 0.0, 0.0), turn, 0.0));
        assert!(is_camera_cut(a, q, DVec3::new(6.0, 0.0, 0.0), q, 0.02));
    }
}
