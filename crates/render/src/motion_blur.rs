//! Motion blur: what a camera's shutter does to things that move on screen.
//!
//! Put a [`MotionBlur`] on the camera entity. Each pixel's velocity comes from the depth
//! prepass's motion vectors, which include moving objects (meshes report last frame's
//! transform), or, for whatever the prepass doesn't draw (the sky, features without a prepass),
//! from the camera's motion and the depth. The blur is the reconstruction filter of McGuire et
//! al. 2012 with Jimenez's 2014 refinements (tile max and neighbor max velocities, a
//! depth-aware gather): see `shaders/motion_blur.wgsl`.
//!
//! The blur follows the motion shown on screen. Its length is the on-screen speed times the
//! exposure time `shutter_fraction / reference_fps`: the speed is this frame's motion over the
//! real time since the last frame, so the same on-screen speed blurs the same at 30 or 144 fps,
//! and in slow motion (time dilation) things move less per frame and blur less. A frame after a
//! camera cut (or the first frame) is not blurred, and a still image is left exactly as it is.
//!
//! It runs after TAA (whose history stays sharp, and whose neighborhood clamp would otherwise
//! cut the streaks) and after depth of field, before bloom (a streaking light's glow follows
//! the streak) and tonemapping (on linear HDR, so bright lights streak with their full energy).

use bytemuck::{Pod, Zeroable};
use engine_core::prelude::*;

use crate::depth_of_field::post_pipeline;
use crate::features::GpuShared;
use crate::frame::ViewData;
use crate::renderer::{fullscreen_pass, layout_entry};
use crate::targets::{Targets, Tex};

/// Motion blur for the camera on this entity (off without it). See the [module docs](self).
///
/// ```ignore
/// commands.spawn((Camera::perspective(60.0), MotionBlur::default(), Transform::default()));
/// ```
#[derive(Component, Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct MotionBlur {
    /// Share of the frame interval the shutter is open at `reference_fps`: 0.5 is a 180°
    /// shutter, 1.0 a 360° one. The exposure is `shutter_fraction / reference_fps` seconds.
    pub shutter_fraction: f32,
    /// The frame rate the shutter is defined at (Hz), so the blur of a given on-screen speed
    /// doesn't change with the actual frame rate (0.5 at 30 = 1/60 s, a film-like look at 60
    /// fps). 0 uses the actual frame interval instead (the blur then grows as the frame rate
    /// drops).
    pub reference_fps: f32,
    /// Longest blur, pixels of the output image from the pixel to either end of its streak
    /// (1..96).
    pub max_radius_px: f32,
    /// Most samples per pixel along the streak (4..64): fewer is cheaper and noisier.
    pub samples: u32,
}

impl Default for MotionBlur {
    fn default() -> Self {
        Self {
            shutter_fraction: 0.5,
            reference_fps: 30.0,
            max_radius_px: 32.0,
            samples: 24,
        }
    }
}

impl MotionBlur {
    /// A shutter open for `shutter_fraction` of the frame interval at `reference_fps`.
    pub fn new(shutter_fraction: f32, reference_fps: f32) -> Self {
        Self {
            shutter_fraction,
            reference_fps,
            ..Default::default()
        }
    }

    /// Exposure time (s) for a frame that took `frame_secs` of real time.
    pub fn exposure_secs(&self, frame_secs: f32) -> f32 {
        let shutter = self.shutter_fraction.clamp(0.0, 4.0);
        if self.reference_fps > 0.0 {
            shutter / self.reference_fps
        } else {
            shutter * frame_secs.max(0.0)
        }
    }

    /// The blur's length over this frame's on-screen motion, for a frame that took
    /// `frame_secs` of real time: the exposure over the frame interval.
    pub fn blur_scale(&self, frame_secs: f32) -> f32 {
        if self.reference_fps > 0.0 && frame_secs > 1e-4 {
            self.exposure_secs(frame_secs) / frame_secs
        } else {
            self.shutter_fraction.clamp(0.0, 4.0)
        }
    }
}

/// Largest blur radius the passes support (output pixels).
const MAX_RADIUS_PX: f32 = 96.0;
/// Pixels per tile side.
const TILE_PX: u32 = 16;
/// Velocity (half blur vector, px) and view depth (m).
const VELOCITY_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba16Float;

#[repr(C)]
#[derive(Debug, Clone, Copy, Pod, Zeroable)]
struct MotionBlurUniform {
    inv_view_proj: [[f32; 4]; 4],
    view_proj: [[f32; 4]; 4],
    prev_view_proj: [[f32; 4]; 4],
    camera_delta: [f32; 4],
    params: [f32; 4],
    size: [f32; 4],
}

/// Pipelines and size-independent resources.
pub(crate) struct MotionBlurPass {
    layout: wgpu::BindGroupLayout,
    params: wgpu::Buffer,
    velocity: wgpu::RenderPipeline,
    tiles: wgpu::RenderPipeline,
    neighbors: wgpu::RenderPipeline,
    gather: wgpu::RenderPipeline,
    targets: Option<MotionBlurTargets>,
}

struct MotionBlurTargets {
    size: (u32, u32),
    velocity: Tex,
    tiles: Tex,
    neighbors: Tex,
    /// Velocity, tiles, neighbors, gather.
    groups: [wgpu::BindGroup; 4],
}

impl MotionBlurPass {
    pub fn new(shared: &GpuShared) -> Self {
        use wgpu::ShaderStages as S;
        let device = &shared.device;
        let float = |binding| {
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
        let depth = |binding| {
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
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("motion blur"),
            entries: &[
                layout_entry(
                    0,
                    S::FRAGMENT,
                    wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                ),
                layout_entry(
                    1,
                    S::FRAGMENT,
                    wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                ),
                float(2),
                float(3),
                float(4),
                depth(5),
                depth(6),
                float(7),
                float(8),
            ],
        });
        let module = shared.shader(
            "motion blur",
            include_str!("shaders/motion_blur.wgsl"),
            false,
        );
        let pipeline = |label, entry, format| {
            post_pipeline(
                shared,
                label,
                &module,
                "vs_motion_blur",
                entry,
                &layout,
                &[format],
            )
        };
        Self {
            velocity: pipeline("motion blur velocity", "fs_velocity", VELOCITY_FORMAT),
            tiles: pipeline("motion blur tiles", "fs_tiles", VELOCITY_FORMAT),
            neighbors: pipeline("motion blur neighbors", "fs_neighbors", VELOCITY_FORMAT),
            gather: pipeline("motion blur gather", "fs_gather", shared.formats.hdr),
            params: device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("motion blur params"),
                size: std::mem::size_of::<MotionBlurUniform>() as u64,
                usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            }),
            layout,
            targets: None,
        }
    }

    /// The output size changed: drop the size-dependent targets.
    pub fn resize(&mut self) {
        self.targets = None;
    }

    /// Record motion blur of `targets.hdr` into `out`.
    #[allow(clippy::too_many_arguments)]
    pub fn encode(
        &mut self,
        shared: &GpuShared,
        targets: &Targets,
        encoder: &mut wgpu::CommandEncoder,
        view: &ViewData,
        settings: &MotionBlur,
        real_dt: f32,
        out: &wgpu::TextureView,
    ) {
        let size = (targets.width, targets.height);
        if self.targets.as_ref().is_none_or(|t| t.size != size) {
            self.targets = Some(MotionBlurTargets::new(
                shared,
                targets,
                &self.layout,
                &self.params,
            ));
        }
        let max_radius = settings.max_radius_px.clamp(1.0, MAX_RADIUS_PX);
        let delta = view.camera_delta();
        let u = MotionBlurUniform {
            inv_view_proj: view.view_proj.inverse().to_cols_array_2d(),
            view_proj: view.unjittered_view_proj.to_cols_array_2d(),
            prev_view_proj: view.prev_view_proj.to_cols_array_2d(),
            camera_delta: [delta.x, delta.y, delta.z, 0.0],
            params: [
                settings.blur_scale(real_dt),
                max_radius,
                settings.samples.clamp(4, 64) as f32,
                view.near.max(1e-4),
            ],
            size: [
                targets.width as f32,
                targets.height as f32,
                (max_radius / TILE_PX as f32).ceil(),
                (view.frame_index % 64) as f32,
            ],
        };
        shared
            .queue
            .write_buffer(&self.params, 0, bytemuck::bytes_of(&u));
        let t = self.targets.as_ref().expect("motion blur targets");
        let passes = [
            (
                "motion blur velocity",
                &self.velocity,
                &t.groups[0],
                &t.velocity.view,
            ),
            (
                "motion blur tiles",
                &self.tiles,
                &t.groups[1],
                &t.tiles.view,
            ),
            (
                "motion blur neighbors",
                &self.neighbors,
                &t.groups[2],
                &t.neighbors.view,
            ),
        ];
        for (label, pipeline, group, target) in
            passes
                .into_iter()
                .chain([("motion blur gather", &self.gather, &t.groups[3], out)])
        {
            let mut pass = fullscreen_pass(encoder, label, target, true);
            pass.set_pipeline(pipeline);
            pass.set_bind_group(0, group, &[]);
            pass.draw(0..3, 0..1);
        }
    }
}

impl MotionBlurTargets {
    fn new(
        shared: &GpuShared,
        targets: &Targets,
        layout: &wgpu::BindGroupLayout,
        params: &wgpu::Buffer,
    ) -> Self {
        use wgpu::TextureUsages as U;
        let device = &shared.device;
        let (w, h) = (targets.width, targets.height);
        let (tw, th) = (w.div_ceil(TILE_PX), h.div_ceil(TILE_PX));
        let usage = U::RENDER_ATTACHMENT | U::TEXTURE_BINDING;
        let velocity = Tex::new(device, "motion blur velocity", w, h, VELOCITY_FORMAT, usage);
        let tiles = Tex::new(device, "motion blur tiles", tw, th, VELOCITY_FORMAT, usage);
        let neighbors = Tex::new(
            device,
            "motion blur neighbors",
            tw,
            th,
            VELOCITY_FORMAT,
            usage,
        );
        let group = |label: &str, color: &Tex, data: &Tex, near: &Tex| {
            let views = [
                &color.view,
                &data.view,
                &near.view,
                &targets.depth.view,
                &targets.prepass_depth.view,
                &targets.prepass_normal.view,
                &targets.motion.view,
            ];
            let mut entries = vec![
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: params.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::Sampler(&shared.linear_clamp),
                },
            ];
            entries.extend(views.iter().enumerate().map(|(i, v)| wgpu::BindGroupEntry {
                binding: i as u32 + 2,
                resource: wgpu::BindingResource::TextureView(v),
            }));
            device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some(label),
                layout,
                entries: &entries,
            })
        };
        // Unused inputs hold any texture the pass doesn't draw into.
        let groups = [
            group("motion blur velocity", &targets.hdr, &tiles, &neighbors),
            group("motion blur tiles", &targets.hdr, &velocity, &neighbors),
            group("motion blur neighbors", &targets.hdr, &tiles, &velocity),
            group("motion blur gather", &targets.hdr, &velocity, &neighbors),
        ];
        Self {
            size: (w, h),
            velocity,
            tiles,
            neighbors,
            groups,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::features::{compose_shader, validate_wgsl};

    #[test]
    fn shader_validates() {
        validate_wgsl(
            "motion_blur",
            &compose_shader(include_str!("shaders/motion_blur.wgsl"), false),
        )
        .unwrap();
    }

    #[test]
    fn a_180_degree_shutter() {
        let mb = MotionBlur::default();
        // 0.5 at 30 fps: 1/60 s, whatever the frame rate.
        assert!((mb.exposure_secs(1.0 / 144.0) - 1.0 / 60.0).abs() < 1e-6);
        // At 30 fps the blur is half the motion per frame, at 60 fps all of it, at 120 twice it:
        // the same length for the same on-screen speed.
        assert!((mb.blur_scale(1.0 / 30.0) - 0.5).abs() < 1e-5);
        assert!((mb.blur_scale(1.0 / 60.0) - 1.0).abs() < 1e-5);
        assert!((mb.blur_scale(1.0 / 120.0) - 2.0).abs() < 1e-4);
        // Tied to the actual frame interval instead.
        let actual = MotionBlur::new(0.5, 0.0);
        assert_eq!(actual.blur_scale(1.0 / 120.0), 0.5);
        assert!((actual.exposure_secs(1.0 / 120.0) - 1.0 / 240.0).abs() < 1e-7);
        // No time passed (a re-render of the same frame): fall back to the shutter fraction.
        assert_eq!(mb.blur_scale(0.0), 0.5);
    }

    #[test]
    fn deserializes_with_defaults() {
        let m: MotionBlur = serde_json::from_str(r#"{"shutter_fraction": 0.25}"#).unwrap();
        assert_eq!(m.shutter_fraction, 0.25);
        assert_eq!(m.samples, 24);
    }
}
