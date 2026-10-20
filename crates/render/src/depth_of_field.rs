//! Depth of field: a physically based thin lens on the camera, with a gather-based bokeh.
//!
//! Put a [`DepthOfField`] on the camera entity. The lens's focal length follows from the
//! camera's vertical field of view and the sensor height (a 27° field of view on a full-frame
//! sensor is a 50 mm lens), and the f-number sets the aperture, so a 50 mm f/1.4 lens blurs the
//! background as much as a real one would, and stopping down to f/16 brings it back into focus.
//! The circle of confusion of a point at distance `z` with the lens focused at `s` is
//! `c = f² (1/s − 1/z) / (N (1 − f/s))` on the sensor (diameter; negative in front of the focus
//! plane), scaled to pixels by the image height over the sensor height.
//!
//! Autofocus measures what is at the screen center every frame on the GPU (the weighted median
//! of a grid of depths, sky ignored), and pulls the focus toward it at `autofocus_speed`; after
//! a camera cut it snaps. `Diagnostics` reports the focus distance as `render.dof_focus_m` (read
//! back a frame or two later).
//!
//! It runs on the scene before TAA, and before bloom and tonemapping (see
//! `shaders/depth_of_field.wgsl` for the passes). The circle of confusion comes from this
//! frame's depth, which TAA's jitter moves by a fraction of a pixel every frame: on TAA's
//! resolved image a silhouette between something in focus and a blurred background would flip
//! between sharp and blurred from one frame to the next. Before TAA, color and depth agree, and
//! TAA anti-aliases the silhouettes of the defocused image and smooths what is left of the
//! gather's sample pattern like any other detail. The fog is part of the radiance that reaches
//! the lens, so it is blurred with what it covers; bloom (glare inside the lens) spreads around
//! the defocused image, as in a camera; and working on linear pre-exposed HDR keeps the energy
//! of small bright lights, which open into bright discs. What the order costs: next frame's
//! reflections and bounce light, which read the finished image, see it defocused (much as the
//! lens sees the reflection of a defocused background anyway).

use std::sync::Arc;
use std::sync::atomic::{AtomicU8, Ordering};

use bytemuck::{Pod, Zeroable};
use engine_core::prelude::*;

use crate::features::GpuShared;
use crate::frame::ViewData;
use crate::renderer::{fullscreen_pass, layout_entry};
use crate::targets::{Targets, Tex};

/// Depth of field for the camera on this entity (off without it). See the [module
/// docs](self).
///
/// ```ignore
/// // A 50 mm lens at f/1.4 on a full-frame camera, focused on whatever is at the screen center.
/// commands.spawn((
///     Camera::perspective(DepthOfField::fov_y_deg_for_lens(50.0, 24.0)),
///     DepthOfField::autofocus(1.4),
///     Transform::from_xyz(0.0, 1.6, 0.0),
/// ));
/// ```
#[derive(Component, Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct DepthOfField {
    /// Distance to the plane in focus along the view direction (m). Ignored while `autofocus`
    /// is on.
    pub focus_distance_m: f32,
    /// Aperture as an f-number: f/1.4 blurs a lot, f/16 hardly at all.
    pub f_stop: f32,
    /// Height of the sensor (film gate), mm: 24 full frame, 15.6 APS-C, 13.5 Super 35 at 2.39:1.
    /// With the camera's vertical field of view it gives the focal length.
    pub sensor_height_mm: f32,
    /// Focus on what is at the screen center instead of `focus_distance_m`.
    pub autofocus: bool,
    /// How quickly autofocus follows (1/s): the focus closes about 63% of the way (in
    /// diopters) every `1 / autofocus_speed` seconds of real time. It snaps after a camera cut.
    pub autofocus_speed: f32,
    /// Size of the square at the screen center that autofocus looks at, as a fraction of the
    /// image height.
    pub autofocus_area: f32,
    /// Largest blur radius, pixels of the output image (1..96): the cost and the softest blur.
    pub max_radius_px: f32,
}

impl Default for DepthOfField {
    fn default() -> Self {
        Self {
            focus_distance_m: 10.0,
            f_stop: 2.8,
            sensor_height_mm: 24.0,
            autofocus: false,
            autofocus_speed: 4.0,
            autofocus_area: 0.12,
            max_radius_px: 32.0,
        }
    }
}

impl DepthOfField {
    /// Focused at `focus_distance_m` with the aperture at `f_stop`.
    pub fn new(focus_distance_m: f32, f_stop: f32) -> Self {
        Self {
            focus_distance_m,
            f_stop,
            ..Default::default()
        }
    }

    /// Autofocus on the screen center with the aperture at `f_stop`.
    pub fn autofocus(f_stop: f32) -> Self {
        Self {
            autofocus: true,
            f_stop,
            ..Default::default()
        }
    }

    pub fn with_sensor_height_mm(mut self, mm: f32) -> Self {
        self.sensor_height_mm = mm;
        self
    }

    pub fn with_max_radius_px(mut self, px: f32) -> Self {
        self.max_radius_px = px;
        self
    }

    /// Vertical field of view (degrees) of a lens of `focal_length_mm` on a sensor
    /// `sensor_height_mm` tall: what to give `Camera::perspective` for "a 50 mm lens".
    pub fn fov_y_deg_for_lens(focal_length_mm: f32, sensor_height_mm: f32) -> f32 {
        2.0 * (0.5 * sensor_height_mm / focal_length_mm.max(1e-3))
            .atan()
            .to_degrees()
    }

    /// Focal length (m) of the lens that gives a vertical field of view of `fov_y` radians on
    /// this sensor.
    pub fn focal_length_m(&self, fov_y: f32) -> f32 {
        0.5 * self.sensor_height_mm.max(1.0) * 1e-3 / (0.5 * fov_y.clamp(1e-3, 3.1)).tan()
    }

    /// Signed circle of confusion radius in pixels of a point `distance_m` along the view
    /// direction, with the lens focused at `focus_m`, for an image `image_height_px` tall seen
    /// through a vertical field of view of `fov_y` radians: negative in front of the focus
    /// plane, positive behind it, not limited to `max_radius_px`.
    pub fn coc_radius_px(
        &self,
        distance_m: f32,
        focus_m: f32,
        fov_y: f32,
        image_height_px: f32,
    ) -> f32 {
        let lens = self.lens(fov_y, image_height_px);
        let focus = (1.0 / focus_m.max(1e-3)).min(lens.closest_focus);
        lens.coc_scale * (focus - 1.0 / distance_m.max(1e-6)) / (1.0 - lens.focal_length * focus)
    }

    fn lens(&self, fov_y: f32, image_height_px: f32) -> Lens {
        let f = self.focal_length_m(fov_y);
        let n = self.f_stop.max(0.5);
        let sensor = self.sensor_height_mm.max(1.0) * 1e-3;
        Lens {
            focal_length: f,
            // Radius = diameter / 2, from the sensor to pixels.
            coc_scale: f * f / (2.0 * n * sensor) * image_height_px,
            // No closer than twice the focal length (1:1 macro).
            closest_focus: 0.5 / f,
        }
    }
}

struct Lens {
    focal_length: f32,
    coc_scale: f32,
    closest_focus: f32,
}

/// Largest blur radius the passes support (output pixels).
const MAX_RADIUS_PX: f32 = 96.0;
/// Full-res pixels per tile side (8 half-res texels).
const TILE_PX: u32 = 16;
const READBACKS: usize = 3;
const FREE: u8 = 0;
const IN_FLIGHT: u8 = 1;
const MAPPING: u8 = 2;
const MAPPED: u8 = 3;
const FAILED: u8 = 4;

#[repr(C)]
#[derive(Debug, Clone, Copy, Pod, Zeroable)]
struct DofUniform {
    lens: [f32; 4],
    size: [f32; 4],
    focus: [f32; 4],
    misc: [f32; 4],
}

/// Pipelines and size-independent resources.
pub(crate) struct DofPass {
    layout: wgpu::BindGroupLayout,
    params: wgpu::Buffer,
    focus: wgpu::RenderPipeline,
    setup: wgpu::RenderPipeline,
    tiles: wgpu::RenderPipeline,
    dilate: wgpu::RenderPipeline,
    prefilter: wgpu::RenderPipeline,
    gather: wgpu::RenderPipeline,
    postfilter: wgpu::RenderPipeline,
    composite: wgpu::RenderPipeline,
    /// The focus (diopters) carried between frames, and this frame's.
    focus_state: Tex,
    focus_next: Tex,
    readbacks: Vec<FocusReadback>,
    pending: Option<usize>,
    targets: Option<DofTargets>,
    /// Camera and whether depth of field ran last frame (autofocus snaps when either changes).
    last_camera: Option<Entity>,
    was_active: bool,
    /// Frames encoded, and the frame `focus_distance_m` was measured in: readings land a
    /// varying number of frames late and can land out of order, so the newest one wins.
    frame: u64,
    read_frame: u64,
    /// Latest focus distance read back from the GPU (m).
    pub focus_distance_m: Option<f32>,
}

struct FocusReadback {
    buffer: wgpu::Buffer,
    state: Arc<AtomicU8>,
    /// The frame whose focus this buffer holds.
    frame: u64,
}

/// Size-dependent targets and the bind groups that read them.
struct DofTargets {
    size: (u32, u32),
    half: Tex,
    pre: Tex,
    far: Tex,
    near: Tex,
    tiles: Tex,
    dilated: Tex,
    groups: [wgpu::BindGroup; 8],
}

impl DofPass {
    pub fn new(shared: &GpuShared) -> Self {
        use wgpu::ShaderStages as S;
        let device = &shared.device;
        let float = |binding, filterable| {
            layout_entry(
                binding,
                S::FRAGMENT,
                wgpu::BindingType::Texture {
                    sample_type: wgpu::TextureSampleType::Float { filterable },
                    view_dimension: wgpu::TextureViewDimension::D2,
                    multisampled: false,
                },
            )
        };
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("depth of field"),
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
                float(2, true),
                float(3, true),
                float(4, true),
                layout_entry(
                    5,
                    S::FRAGMENT,
                    wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Depth,
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                ),
                float(6, false),
            ],
        });
        let module = shared.shader(
            "depth of field",
            include_str!("shaders/depth_of_field.wgsl"),
            false,
        );
        let half = shared.formats.hdr;
        let pipeline = |label, entry, formats: &[wgpu::TextureFormat]| {
            post_pipeline(shared, label, &module, "vs_dof", entry, &layout, formats)
        };
        let focus = pipeline("dof focus", "fs_focus", &[FOCUS_FORMAT]);
        let setup = pipeline("dof setup", "fs_setup", &[half]);
        let tiles = pipeline("dof tiles", "fs_tiles", &[TILE_FORMAT]);
        let dilate = pipeline("dof dilate", "fs_dilate", &[TILE_FORMAT]);
        let prefilter = pipeline("dof prefilter", "fs_prefilter", &[half]);
        let gather = pipeline("dof gather", "fs_gather", &[half, half]);
        let postfilter = pipeline("dof postfilter", "fs_postfilter", &[half, half]);
        let composite = pipeline("dof composite", "fs_composite", &[shared.formats.hdr]);
        let params = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("dof params"),
            size: std::mem::size_of::<DofUniform>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        use wgpu::TextureUsages as U;
        let focus_state = Tex::new(
            device,
            "dof focus state",
            1,
            1,
            FOCUS_FORMAT,
            U::TEXTURE_BINDING | U::COPY_DST,
        );
        let focus_next = Tex::new(
            device,
            "dof focus",
            1,
            1,
            FOCUS_FORMAT,
            U::RENDER_ATTACHMENT | U::COPY_SRC,
        );
        let readbacks = (0..READBACKS)
            .map(|_| FocusReadback {
                buffer: device.create_buffer(&wgpu::BufferDescriptor {
                    label: Some("dof focus readback"),
                    size: 256,
                    usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
                    mapped_at_creation: false,
                }),
                state: Arc::new(AtomicU8::new(FREE)),
                frame: 0,
            })
            .collect();
        Self {
            layout,
            params,
            focus,
            setup,
            tiles,
            dilate,
            prefilter,
            gather,
            postfilter,
            composite,
            focus_state,
            focus_next,
            readbacks,
            pending: None,
            targets: None,
            last_camera: None,
            was_active: false,
            frame: 0,
            read_frame: 0,
            focus_distance_m: None,
        }
    }

    /// The output size changed: drop the size-dependent targets.
    pub fn resize(&mut self) {
        self.targets = None;
    }

    /// Depth of field was off this frame: autofocus snaps when it comes back.
    pub fn skip(&mut self) {
        self.was_active = false;
    }

    /// Record depth of field from `targets.hdr` (and the final depth) into `out`.
    #[allow(clippy::too_many_arguments)]
    pub fn encode(
        &mut self,
        shared: &GpuShared,
        targets: &Targets,
        encoder: &mut wgpu::CommandEncoder,
        view: &ViewData,
        settings: &DepthOfField,
        real_dt: f32,
        out: &wgpu::TextureView,
    ) {
        let size = (targets.width, targets.height);
        if self.targets.as_ref().is_none_or(|t| t.size != size) {
            self.targets = Some(DofTargets::new(
                shared,
                targets,
                &self.layout,
                &self.params,
                &self.focus_state,
            ));
        }
        let snap = !self.was_active || !view.history_valid || self.last_camera != view.camera;
        self.was_active = true;
        self.last_camera = view.camera;
        self.frame += 1;

        let height = targets.height as f32;
        let lens = settings.lens(view.fov_y, height);
        let max_radius = settings.max_radius_px.clamp(1.0, MAX_RADIUS_PX);
        let blend = if snap {
            1.0
        } else {
            1.0 - (-settings.autofocus_speed.max(0.0) * real_dt.max(0.0)).exp()
        };
        let (half_w, half_h) = (targets.width.div_ceil(2), targets.height.div_ceil(2));
        let reach = (0.5 * max_radius / (TILE_PX / 2) as f32).ceil();
        let u = DofUniform {
            lens: [
                lens.coc_scale,
                lens.focal_length,
                view.near.max(1e-4),
                max_radius,
            ],
            size: [
                targets.width as f32,
                targets.height as f32,
                half_w as f32,
                half_h as f32,
            ],
            focus: [
                if settings.autofocus { 1.0 } else { 0.0 },
                if settings.autofocus { blend } else { 1.0 },
                (1.0 / settings.focus_distance_m.max(1e-3)).min(lens.closest_focus),
                0.5 * settings.autofocus_area.clamp(0.005, 1.0) * height,
            ],
            misc: [reach, lens.closest_focus, 0.0, 0.0],
        };
        shared
            .queue
            .write_buffer(&self.params, 0, bytemuck::bytes_of(&u));
        let t = self.targets.as_ref().expect("dof targets");
        let pass = |encoder: &mut wgpu::CommandEncoder,
                    label: &str,
                    pipeline: &wgpu::RenderPipeline,
                    group: &wgpu::BindGroup,
                    target: &wgpu::TextureView| {
            let mut pass = fullscreen_pass(encoder, label, target, true);
            pass.set_pipeline(pipeline);
            pass.set_bind_group(0, group, &[]);
            pass.draw(0..3, 0..1);
        };
        // The focus: measured into `focus_next`, then kept for next frame.
        pass(
            encoder,
            "dof focus",
            &self.focus,
            &t.groups[0],
            &self.focus_next.view,
        );
        encoder.copy_texture_to_texture(
            self.focus_next.texture.as_image_copy(),
            self.focus_state.texture.as_image_copy(),
            wgpu::Extent3d {
                width: 1,
                height: 1,
                depth_or_array_layers: 1,
            },
        );
        pass(
            encoder,
            "dof setup",
            &self.setup,
            &t.groups[1],
            &t.half.view,
        );
        pass(
            encoder,
            "dof tiles",
            &self.tiles,
            &t.groups[2],
            &t.tiles.view,
        );
        pass(
            encoder,
            "dof dilate",
            &self.dilate,
            &t.groups[3],
            &t.dilated.view,
        );
        pass(
            encoder,
            "dof prefilter",
            &self.prefilter,
            &t.groups[4],
            &t.pre.view,
        );
        // Both fields at once, then both filtered back into `half` and `pre`.
        let fields = |encoder: &mut wgpu::CommandEncoder,
                      label: &str,
                      pipeline: &wgpu::RenderPipeline,
                      group: &wgpu::BindGroup,
                      far: &wgpu::TextureView,
                      near: &wgpu::TextureView| {
            let attachment = |view| {
                Some(wgpu::RenderPassColorAttachment {
                    view,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),
                        store: wgpu::StoreOp::Store,
                    },
                })
            };
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some(label),
                color_attachments: &[attachment(far), attachment(near)],
                ..Default::default()
            });
            pass.set_pipeline(pipeline);
            pass.set_bind_group(0, group, &[]);
            pass.draw(0..3, 0..1);
        };
        fields(
            encoder,
            "dof gather",
            &self.gather,
            &t.groups[5],
            &t.far.view,
            &t.near.view,
        );
        fields(
            encoder,
            "dof postfilter",
            &self.postfilter,
            &t.groups[6],
            &t.half.view,
            &t.pre.view,
        );
        pass(encoder, "dof composite", &self.composite, &t.groups[7], out);
        self.read_back_focus(encoder);
    }

    fn read_back_focus(&mut self, encoder: &mut wgpu::CommandEncoder) {
        let Some(slot) = self
            .readbacks
            .iter()
            .position(|r| r.state.load(Ordering::Acquire) == FREE)
        else {
            return;
        };
        let rb = &mut self.readbacks[slot];
        rb.frame = self.frame;
        encoder.copy_texture_to_buffer(
            self.focus_next.texture.as_image_copy(),
            wgpu::TexelCopyBufferInfo {
                buffer: &rb.buffer,
                layout: wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(256),
                    rows_per_image: Some(1),
                },
            },
            wgpu::Extent3d {
                width: 1,
                height: 1,
                depth_or_array_layers: 1,
            },
        );
        rb.state.store(IN_FLIGHT, Ordering::Release);
        self.pending = Some(slot);
    }

    /// Start mapping this frame's focus readback (call right after `queue.submit`).
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

    /// How many frames ago `focus_distance_m` was measured (1: the frame before the latest).
    pub fn focus_lag_frames(&self) -> Option<u64> {
        self.focus_distance_m.map(|_| self.frame - self.read_frame)
    }

    /// Collect finished focus readbacks (non-blocking; the device was polled this frame).
    pub fn collect(&mut self) {
        for rb in &self.readbacks {
            match rb.state.load(Ordering::Acquire) {
                MAPPED => {
                    let diopters = match rb.buffer.slice(..).get_mapped_range() {
                        Ok(view) => Some(f32::from_le_bytes([view[0], view[1], view[2], view[3]])),
                        Err(_) => None,
                    };
                    rb.buffer.unmap();
                    rb.state.store(FREE, Ordering::Release);
                    if let Some(d) = diopters.filter(|d| d.is_finite())
                        && rb.frame > self.read_frame
                    {
                        self.read_frame = rb.frame;
                        self.focus_distance_m =
                            Some(if d > 1e-6 { 1.0 / d } else { f32::INFINITY });
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

/// The focus state: diopters in r.
const FOCUS_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::R32Float;
/// Tiles: largest near-field CoC, largest CoC (half-res texels).
const TILE_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba16Float;

impl DofTargets {
    fn new(
        shared: &GpuShared,
        targets: &Targets,
        layout: &wgpu::BindGroupLayout,
        params: &wgpu::Buffer,
        focus: &Tex,
    ) -> Self {
        use wgpu::TextureUsages as U;
        let device = &shared.device;
        let (w, h) = (targets.width, targets.height);
        let (hw, hh) = (w.div_ceil(2), h.div_ceil(2));
        let (tw, th) = (w.div_ceil(TILE_PX), h.div_ceil(TILE_PX));
        let usage = U::RENDER_ATTACHMENT | U::TEXTURE_BINDING;
        let hdr = shared.formats.hdr;
        let half = Tex::new(device, "dof half", hw, hh, hdr, usage);
        let pre = Tex::new(device, "dof prefiltered", hw, hh, hdr, usage);
        let far = Tex::new(device, "dof far field", hw, hh, hdr, usage);
        let near = Tex::new(device, "dof near field", hw, hh, hdr, usage);
        let tiles = Tex::new(device, "dof tiles", tw, th, TILE_FORMAT, usage);
        let dilated = Tex::new(device, "dof tiles dilated", tw, th, TILE_FORMAT, usage);
        let group = |label: &str, a: &Tex, b: &Tex, c: &Tex| {
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
                        resource: wgpu::BindingResource::TextureView(&a.view),
                    },
                    wgpu::BindGroupEntry {
                        binding: 3,
                        resource: wgpu::BindingResource::TextureView(&b.view),
                    },
                    wgpu::BindGroupEntry {
                        binding: 4,
                        resource: wgpu::BindingResource::TextureView(&c.view),
                    },
                    wgpu::BindGroupEntry {
                        binding: 5,
                        resource: wgpu::BindingResource::TextureView(&targets.depth.view),
                    },
                    wgpu::BindGroupEntry {
                        binding: 6,
                        resource: wgpu::BindingResource::TextureView(&focus.view),
                    },
                ],
            })
        };
        let scene = &targets.hdr;
        // Unused inputs hold any texture the pass doesn't draw into.
        let groups = [
            group("dof focus", scene, scene, scene),
            group("dof setup", scene, &pre, &pre),
            group("dof tiles", &half, &pre, &pre),
            group("dof dilate", &tiles, &pre, &pre),
            group("dof prefilter", &half, &dilated, &far),
            group("dof gather", &pre, &dilated, &half),
            group("dof postfilter", &far, &near, &dilated),
            // The filtered fields: far in `half`, near in `pre`.
            group("dof composite", scene, &half, &pre),
        ];
        Self {
            size: (w, h),
            half,
            pre,
            far,
            near,
            tiles,
            dilated,
            groups,
        }
    }
}

/// A fullscreen-triangle pipeline drawing into `formats` (one or more color targets).
pub(crate) fn post_pipeline(
    shared: &GpuShared,
    label: &str,
    module: &wgpu::ShaderModule,
    vs: &str,
    fs: &str,
    layout: &wgpu::BindGroupLayout,
    formats: &[wgpu::TextureFormat],
) -> wgpu::RenderPipeline {
    let pipeline_layout = shared
        .device
        .create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some(label),
            bind_group_layouts: &[Some(layout)],
            immediate_size: 0,
        });
    let targets: Vec<Option<wgpu::ColorTargetState>> = formats
        .iter()
        .map(|&format| {
            Some(wgpu::ColorTargetState {
                format,
                blend: None,
                write_mask: wgpu::ColorWrites::ALL,
            })
        })
        .collect();
    shared
        .device
        .create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some(label),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module,
                entry_point: Some(vs),
                compilation_options: Default::default(),
                buffers: &[],
            },
            primitive: wgpu::PrimitiveState::default(),
            depth_stencil: None,
            multisample: Default::default(),
            fragment: Some(wgpu::FragmentState {
                module,
                entry_point: Some(fs),
                compilation_options: Default::default(),
                targets: &targets,
            }),
            multiview_mask: None,
            cache: None,
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::features::{compose_shader, validate_wgsl};

    #[test]
    fn shader_validates() {
        validate_wgsl(
            "depth_of_field",
            &compose_shader(include_str!("shaders/depth_of_field.wgsl"), false),
        )
        .unwrap();
    }

    #[test]
    fn a_50mm_lens_on_full_frame() {
        let fov = DepthOfField::fov_y_deg_for_lens(50.0, 24.0);
        assert!((fov - 26.99).abs() < 0.05, "{fov}");
        let dof = DepthOfField::new(3.0, 1.4);
        let f = dof.focal_length_m(fov.to_radians());
        assert!((f - 0.05).abs() < 1e-5, "{f}");
    }

    #[test]
    fn circle_of_confusion_matches_the_thin_lens() {
        // 50 mm f/1.4 focused at 3 m, full frame, 1080 px tall. Far limit: c = f² / (N (s - f))
        // = 0.0025 / (1.4 * 2.95) = 0.605 mm on a 24 mm sensor: 27.2 px across, 13.6 px radius.
        let fov = DepthOfField::fov_y_deg_for_lens(50.0, 24.0).to_radians();
        let dof = DepthOfField::new(3.0, 1.4);
        let far = dof.coc_radius_px(1e9, 3.0, fov, 1080.0);
        assert!((far - 13.6).abs() < 0.1, "{far}");
        // In focus at the focus distance, negative in front of it.
        assert!(dof.coc_radius_px(3.0, 3.0, fov, 1080.0).abs() < 1e-3);
        let near = dof.coc_radius_px(1.0, 3.0, fov, 1080.0);
        assert!(near < -20.0, "{near}");
        // f/16 brings the background nearly into focus; twice the image height, twice the blur.
        let stopped = DepthOfField::new(3.0, 16.0).coc_radius_px(1e9, 3.0, fov, 1080.0);
        assert!(stopped < 1.5, "{stopped}");
        let big = dof.coc_radius_px(1e9, 3.0, fov, 2160.0);
        assert!((big - 2.0 * far).abs() < 1e-3);
    }

    #[test]
    fn deserializes_with_defaults() {
        let d: DepthOfField =
            serde_json::from_str(r#"{"f_stop": 2.0, "autofocus": true}"#).unwrap();
        assert_eq!(d.f_stop, 2.0);
        assert!(d.autofocus);
        assert_eq!(d.sensor_height_mm, 24.0);
    }
}
