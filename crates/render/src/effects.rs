//! Screen-space effects built on the depth prepass: ground-truth ambient occlusion (GTAO),
//! screen-space reflections (SSR) and temporal anti-aliasing (TAA).
//!
//! ```text
//! prepass (depth, normals, motion) → copy depth → GTAO → edge-aware blur → SSR (traces the
//! prepass depth, reads the previous frame's image) → [scene shading reads the AO and SSR] …
//! transparent → TAA resolve into the history → copy back to the HDR target (without TAA, the
//! HDR image is copied into the history for next frame's reflections)
//! ```

use bytemuck::{Pod, Zeroable};
use engine_core::prelude::*;

use crate::features::GpuShared;
use crate::renderer::{fullscreen_pass, fullscreen_pipeline, layout_entry};
use crate::targets::Targets;

/// Jitter sequence length (Halton 2, 3).
pub const JITTER_SAMPLES: u64 = 8;
/// Smallest weight of the current frame once the history has converged.
const TAA_MIN_ALPHA: f32 = 0.1;

/// Sub-pixel jitter (pixels, in −0.5..0.5) for frame `index`: the Halton (2, 3) sequence.
pub fn jitter(index: u64) -> Vec2 {
    let i = (index % JITTER_SAMPLES) + 1;
    Vec2::new(halton(i, 2) - 0.5, halton(i, 3) - 0.5)
}

fn halton(mut i: u64, base: u64) -> f32 {
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
    taa_layout: wgpu::BindGroupLayout,
    taa: wgpu::RenderPipeline,
    taa_params: wgpu::Buffer,
    /// TAA input groups, indexed by the history written this frame.
    taa_groups: [wgpu::BindGroup; 2],
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
            entries: &[float_tex(0), depth_tex(1), float_tex(2), float_tex(3)],
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
            ],
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
        let taa_params = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("taa params"),
            size: std::mem::size_of::<TaaUniform>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let (gtao_group, blur_group, taa_groups) =
            Self::groups(shared, targets, &ao_layout, &taa_layout, &taa_params);
        let ssr_groups = Self::ssr_groups(shared, targets, &ssr_layout);
        Self {
            ao_layout,
            gtao,
            blur,
            gtao_group,
            blur_group,
            ssr_layout,
            ssr,
            ssr_groups,
            taa_layout,
            taa,
            taa_params,
            taa_groups,
            current: 0,
            accumulated: 0,
            history_ready: false,
        }
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
                ],
            })
        });
        (gtao_group, blur_group, taa_groups)
    }

    fn ssr_groups(
        shared: &GpuShared,
        t: &Targets,
        layout: &wgpu::BindGroupLayout,
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
        self.ssr_groups = Self::ssr_groups(shared, targets, &self.ssr_layout);
        self.history_ready = false;
        self.accumulated = 0;
    }

    /// Forget the history (camera cut, anti-aliasing turned off).
    pub fn reset_history(&mut self) {
        self.history_ready = false;
        self.accumulated = 0;
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
    /// frame's image (`has_previous` must be true). `prepass_group` as for `encode_ao`.
    pub fn encode_ssr(
        &self,
        encoder: &mut wgpu::CommandEncoder,
        targets: &Targets,
        prepass_group: &wgpu::BindGroup,
    ) {
        let mut pass =
            fullscreen_pass(encoder, "screen-space reflections", &targets.ssr.view, true);
        pass.set_pipeline(&self.ssr);
        pass.set_bind_group(0, prepass_group, &[]);
        pass.set_bind_group(1, &self.ssr_groups[self.current], &[]);
        pass.draw(0..3, 0..1);
    }

    /// Without TAA: keep the finished HDR image as the previous frame for next frame's
    /// reflections.
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
    pub fn encode_taa(
        &mut self,
        shared: &GpuShared,
        encoder: &mut wgpu::CommandEncoder,
        targets: &Targets,
        scene_group: &wgpu::BindGroup,
        history_valid: bool,
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
                0.0,
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

/// Whether the camera jumped so far between two frames that the previous image is useless (a
/// teleport or a cut to another camera), as opposed to fast but continuous motion such as a
/// spaceship at 10 km/s. `prev_step` is the camera's movement the frame before.
pub fn is_camera_cut(
    prev_origin: DVec3,
    prev_rotation: Quat,
    origin: DVec3,
    rotation: Quat,
    prev_step: f64,
) -> bool {
    let step = (origin - prev_origin).length();
    let turned = prev_rotation.angle_between(rotation);
    turned > 45f32.to_radians() || (step > 25.0 && step > 4.0 * prev_step.max(1.0))
}

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
        // Teleport and a cut to a camera facing the other way.
        assert!(is_camera_cut(a, q, DVec3::new(500.0, 0.0, 0.0), q, 0.0));
        assert!(is_camera_cut(a, q, a, Quat::from_rotation_y(3.0), 0.0));
    }
}
