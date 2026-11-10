//! Screen-space subsurface scattering for skin: light that enters the skin comes out around
//! where it went in, red farthest, which softens shading, warms shadow edges and lets ears
//! glow (Jimenez et al. 2015, "Separable Subsurface Scattering").
//!
//! A feature opts in per pixel. Its prepass writes `prepass_output_subsurface(…, strength)`
//! (lib_frame.wgsl) instead of `prepass_output`, which marks the pixel and how strongly it
//! scatters, and its scene pass writes the pixel's specular light to alpha (pre-exposed, the
//! same units as the color) so highlights stay sharp. The feature also answers
//! `RenderFeature::needs_subsurface` with true while it draws such pixels. Right after the
//! opaque scene, two passes blur the marked pixels' diffuse light along x and then y with the
//! skin's diffusion profile (Burley's normalized diffusion, red reaching six times as far as
//! green, twelve as blue), only within marked pixels and not across steps in depth, and add the
//! specular back. The reach follows `RenderSettings::subsurface_width` (meters at strength 1),
//! so a face scatters the same at any distance; far away it covers less than a pixel and the
//! pass leaves it be.
//!
//! Blurring lit, textured skin also blurs its texture (freckles, pores) and greys fair skin
//! ("pre-scatter" texturing). A feature can instead light the skin untextured and draw its
//! color with `RenderFeature::draw_subsurface_color` into an sRGB target (cleared to white) that
//! the second pass multiplies into the scattered diffuse light ("post-scatter", Jimenez et al.
//! 2015 section 4; Unity HDRP's texturing modes, Golubev 2018).

use bytemuck::{Pod, Zeroable};

use crate::depth_of_field::post_pipeline;
use crate::features::GpuShared;
use crate::frame::ViewData;
use crate::renderer::{fullscreen_pass, layout_entry};
use crate::targets::{Targets, Tex};

/// The post-scatter color target (`RenderFeature::draw_subsurface_color`).
pub(crate) const COLOR_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8UnormSrgb;

/// Kernel samples per pass, the center included (`SSS_SAMPLES` in subsurface.wgsl).
const SAMPLES: usize = 17;
/// Samples bunch toward the center by this power (the profile is peaked).
const EXPONENT: f32 = 2.0;
/// Each channel's Burley shape distance `d`, as a share of the kernel's reach: red eight times
/// within it, so all but a twentieth of red's light lands inside. Green and blue in the ratio of
/// measured skin's diffuse mean free paths (Jensen et al. 2001, "skin1": about 3.7, 1.4 and
/// 0.7 mm) over Burley's albedo scaling for skin's surface albedo (0.91, 0.34, 0.27): red
/// spreads six times as far as green and twelve times as far as blue, so shadow edges warm
/// and the texture's detail survives in green and blue.
const SHAPE: [f32; 3] = [1.0 / 8.0, 0.17 / 8.0, 0.08 / 8.0];

#[repr(C)]
#[derive(Debug, Clone, Copy, Pod, Zeroable)]
struct SssUniform {
    kernel: [[f32; 4]; SAMPLES],
    params: [f32; 4],
    dir: [f32; 4],
}

/// Burley's normalized diffusion profile (Christensen and Burley 2015, as Unreal and HDRP
/// scatter skin) at radius `r` for shape distance `d`: the light leaving the surface at `r`
/// from where it entered, per unit area; it integrates to 1 over the plane.
fn burley(r: f32, d: f32) -> f32 {
    ((-r / d).exp() + (-r / (3.0 * d)).exp()) / (8.0 * std::f32::consts::PI * d * r)
}

/// The share of channel `c`'s light the profile lands in the strip `lo..hi` along one axis
/// (reach units): its marginal, so that the x and then y passes spread it over the plane as
/// the profile does (Jimenez et al. 2015 section 3).
fn strip_share(c: usize, lo: f32, hi: f32) -> f32 {
    let d = SHAPE[c];
    // Midpoints in x, and in y = ymax t² (dense near the axis, finer there than blue's shape
    // distance; the singular center is never sampled at r = 0), both halves of y.
    let (nx, nt) = (48, 160);
    let ymax = 1.5f32;
    let dx = (hi - lo) / nx as f32;
    let dt = 1.0 / nt as f32;
    let mut sum = 0.0f64;
    for i in 0..nx {
        let x = lo + (i as f32 + 0.5) * dx;
        for j in 0..nt {
            let t = (j as f32 + 0.5) * dt;
            let y = ymax * t * t;
            let dy = 2.0 * ymax * t * dt;
            sum += 2.0 * (burley((x * x + y * y).sqrt(), d) * dy) as f64;
        }
    }
    (sum * dx as f64) as f32
}

/// The separable kernel: per sample the weight per channel and the offset (-1..1 of the
/// reach), center first; each channel's weights add up to 1 (what the profile carries past
/// the reach is given back to the samples in proportion).
fn kernel() -> [[f32; 4]; SAMPLES] {
    static KERNEL: std::sync::OnceLock<[[f32; 4]; SAMPLES]> = std::sync::OnceLock::new();
    *KERNEL.get_or_init(build_kernel)
}

fn build_kernel() -> [[f32; 4]; SAMPLES] {
    let half = (SAMPLES / 2) as f32;
    let mut k = [[0.0f32; 4]; SAMPLES];
    for (i, s) in k.iter_mut().enumerate() {
        let o = (i as f32 - half) / half;
        s[3] = o.signum() * o.abs().powf(EXPONENT);
    }
    let offsets: Vec<f32> = k.iter().map(|s| s[3]).collect();
    for (i, s) in k.iter_mut().enumerate() {
        // Each sample stands for the strip half-way to its neighbours.
        let lo = if i > 0 {
            (offsets[i] + offsets[i - 1]) / 2.0
        } else {
            -1.0
        };
        let hi = if i + 1 < SAMPLES {
            (offsets[i] + offsets[i + 1]) / 2.0
        } else {
            1.0
        };
        for (c, w) in s.iter_mut().take(3).enumerate() {
            *w = strip_share(c, lo, hi);
        }
    }
    for c in 0..3 {
        let sum: f32 = k.iter().map(|s| s[c]).sum();
        for s in &mut k {
            s[c] /= sum;
        }
    }
    // Center first.
    k.swap(0, SAMPLES / 2);
    k
}

/// The two blur passes and their scratch target.
pub(crate) struct SubsurfacePass {
    layout: wgpu::BindGroupLayout,
    pipeline: wgpu::RenderPipeline,
    /// Along x (scene color → scratch), along y (scratch → scene color).
    params: [wgpu::Buffer; 2],
    kernel: [[f32; 4]; SAMPLES],
    targets: Option<SubsurfaceTargets>,
}

struct SubsurfaceTargets {
    size: (u32, u32),
    scratch: Tex,
    /// Post-scatter color, multiplied in by the second pass.
    color: Tex,
    groups: [wgpu::BindGroup; 2],
}

impl SubsurfacePass {
    pub fn new(shared: &GpuShared) -> Self {
        use wgpu::ShaderStages as S;
        let device = &shared.device;
        let layout = shared.bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("subsurface"),
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
                layout_entry(
                    2,
                    S::FRAGMENT,
                    wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                ),
                layout_entry(
                    3,
                    S::FRAGMENT,
                    wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Depth,
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                ),
                layout_entry(
                    4,
                    S::FRAGMENT,
                    wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                ),
                layout_entry(
                    5,
                    S::FRAGMENT,
                    wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                ),
            ],
        });
        let module = shared.shader("subsurface", include_str!("shaders/subsurface.wgsl"), false);
        let pipeline = post_pipeline(
            shared,
            "subsurface",
            &module,
            "vs_subsurface",
            "fs_subsurface",
            &layout,
            &[shared.formats.hdr],
        );
        let buffer = |label| {
            device.create_buffer(&wgpu::BufferDescriptor {
                label: Some(label),
                size: std::mem::size_of::<SssUniform>() as u64,
                usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            })
        };
        Self {
            layout,
            pipeline,
            params: [buffer("subsurface x"), buffer("subsurface y")],
            kernel: kernel(),
            targets: None,
        }
    }

    /// The output size changed: drop the scratch target.
    pub fn resize(&mut self) {
        self.targets = None;
    }

    fn targets(&mut self, shared: &GpuShared, targets: &Targets) -> &SubsurfaceTargets {
        let size = (targets.width, targets.height);
        if self.targets.as_ref().is_none_or(|t| t.size != size) {
            self.targets = Some(SubsurfaceTargets::new(
                shared,
                targets,
                &self.layout,
                &self.params,
            ));
        }
        self.targets.as_ref().expect("subsurface targets")
    }

    /// The pass the features draw their post-scatter color in: the color target cleared to
    /// white, on the scene's depth (read only), group 0 the frame group.
    pub fn color_pass<'e>(
        &mut self,
        shared: &GpuShared,
        targets: &Targets,
        encoder: &'e mut wgpu::CommandEncoder,
        frame_group: &wgpu::BindGroup,
    ) -> wgpu::RenderPass<'e> {
        let t = self.targets(shared, targets);
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("subsurface color"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: &t.color.view,
                depth_slice: None,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Clear(wgpu::Color::WHITE),
                    store: wgpu::StoreOp::Store,
                },
            })],
            depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                view: &targets.depth.view,
                depth_ops: None,
                stencil_ops: None,
            }),
            ..Default::default()
        });
        pass.set_bind_group(0, frame_group, &[]);
        pass
    }

    /// Blur the marked pixels of `targets.hdr` in place (through the scratch target), then
    /// multiply in the post-scatter color. Only for perspective views.
    pub fn encode(
        &mut self,
        shared: &GpuShared,
        targets: &Targets,
        encoder: &mut wgpu::CommandEncoder,
        view: &ViewData,
        width_m: f32,
    ) {
        self.targets(shared, targets);
        // Pixels per meter at 1 m: the projection's vertical scale over half the height.
        let pixels_per_meter = view.unjittered_proj.y_axis.y * targets.height as f32 * 0.5;
        for (i, (dir, first)) in [([1.0, 0.0], 1.0), ([0.0, 1.0], 0.0)].iter().enumerate() {
            let u = SssUniform {
                kernel: self.kernel,
                params: [pixels_per_meter, view.near.max(1e-4), width_m.max(0.0), 0.0],
                dir: [dir[0], dir[1], *first, 0.0],
            };
            shared
                .queue
                .write_buffer(&self.params[i], 0, bytemuck::bytes_of(&u));
        }
        let t = self.targets.as_ref().expect("subsurface targets");
        for (i, (label, out)) in [
            ("subsurface x", &t.scratch.view),
            ("subsurface y", &targets.hdr.view),
        ]
        .into_iter()
        .enumerate()
        {
            // Unmarked pixels are discarded: the scene color keeps them.
            let mut pass = fullscreen_pass(encoder, label, out, false);
            pass.set_pipeline(&self.pipeline);
            pass.set_bind_group(0, &t.groups[i], &[]);
            pass.draw(0..3, 0..1);
        }
    }
}

impl SubsurfaceTargets {
    fn new(
        shared: &GpuShared,
        targets: &Targets,
        layout: &wgpu::BindGroupLayout,
        params: &[wgpu::Buffer; 2],
    ) -> Self {
        use wgpu::TextureUsages as U;
        let device = &shared.device;
        let scratch = Tex::new(
            device,
            "subsurface scratch",
            targets.width,
            targets.height,
            shared.formats.hdr,
            U::RENDER_ATTACHMENT | U::TEXTURE_BINDING,
        );
        let post = Tex::new(
            device,
            "subsurface color",
            targets.width,
            targets.height,
            COLOR_FORMAT,
            U::RENDER_ATTACHMENT | U::TEXTURE_BINDING,
        );
        let group = |label: &str, params: &wgpu::Buffer, color: &wgpu::TextureView| {
            device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some(label),
                layout,
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: params.as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: wgpu::BindingResource::Sampler(&shared.linear_clamp),
                    },
                    wgpu::BindGroupEntry {
                        binding: 2,
                        resource: wgpu::BindingResource::TextureView(color),
                    },
                    wgpu::BindGroupEntry {
                        binding: 3,
                        resource: wgpu::BindingResource::TextureView(&targets.prepass_depth.view),
                    },
                    wgpu::BindGroupEntry {
                        binding: 4,
                        resource: wgpu::BindingResource::TextureView(&targets.prepass_normal.view),
                    },
                    wgpu::BindGroupEntry {
                        binding: 5,
                        resource: wgpu::BindingResource::TextureView(&post.view),
                    },
                ],
            })
        };
        let groups = [
            group("subsurface x", &params[0], &targets.hdr.view),
            group("subsurface y", &params[1], &scratch.view),
        ];
        Self {
            size: (targets.width, targets.height),
            scratch,
            color: post,
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
            "subsurface",
            &compose_shader(include_str!("shaders/subsurface.wgsl"), false),
        )
        .unwrap();
    }

    #[test]
    fn the_kernel_keeps_the_light_and_red_reaches_farthest() {
        let k = kernel();
        assert_eq!(k[0][3], 0.0, "center first");
        for c in 0..3 {
            let sum: f32 = k.iter().map(|s| s[c]).sum();
            assert!((sum - 1.0).abs() < 1e-4, "channel {c} adds up to {sum}");
        }
        // Light that leaves the center: red most, blue least.
        let spread = |c: usize| 1.0 - k[0][c];
        assert!(spread(0) > spread(1) && spread(1) > spread(2));
        // Far samples carry more red than blue.
        let far = k
            .iter()
            .max_by(|a, b| a[3].abs().total_cmp(&b[3].abs()))
            .unwrap();
        assert!(far[0] > far[2]);
        assert!(k.iter().all(|s| s[3].abs() <= 1.0));
    }
}
