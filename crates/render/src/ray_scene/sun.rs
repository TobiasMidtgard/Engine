//! The sun's trace (`trial_cut_more::RAY_SUN`, ray_sun.wgsl): one ray toward a point of the
//! sun's disc from each pixel of the prepass that wants one, a dispatch of the frame's one ray
//! pass (renderer.rs) against the scene's still structure; then the filter (pass "ray shadows
//! filter", ray_filters.wgsl), which keeps the rays over the frames in a history of their own,
//! and the light pass's group 1, with which screen_light.wgsl joins that history with the shadow
//! maps (`sun_joined`). Its textures are made when the switch turns on and dropped with this
//! when it turns off.

use bytemuck::{Pod, Zeroable};

use crate::features::GpuShared;
use crate::gpu_counts::GpuCounts;
use crate::renderer::{fullscreen_pass, fullscreen_pipeline, layout_entry};
use crate::targets::{Targets, Tex};

/// `sun_raw` (r: the sun's visibility, g: the flags, b: how far a stopped ray went;
/// ray_sun.wgsl). 4 bytes a pixel, 14.7 MB at 2560x1440.
const RAW_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8Unorm;

/// How far a ray of the trace looks for something between its pixel and the sun (meters,
/// `RayUniform::sun` y): what stands farther up-sun is left to the shadow maps.
pub(crate) const RAY_LENGTH: f32 = 64.0;

/// The trace's history (ray_filters.wgsl: r the visibility, g the frames it holds, b the view
/// depth, a the half width of the penumbra in pixels). 8 bytes a pixel, two of them: 59 MB at
/// 2560x1440.
pub(crate) const HISTORY_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba16Float;

/// `SunJoin` in screen_light.wgsl and ray_filters.wgsl.
#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct JoinUniform {
    /// 1 when joined; the reach of the traced pixels, the distance from which the rays meet
    /// the plants, the rays' length (`RayUniform::sun`).
    params: [f32; 4],
    /// One depth step of the shadow maps, where the join ends (meters), the tangent of the
    /// radius of the sun's disc the rays aim at, and 1 when the filter may read last frame's
    /// history.
    depth: [f32; 4],
}

/// The size of [`JoinUniform`], for the stand-in of a view without the join (effects.rs).
pub(crate) const JOIN_UNIFORM_SIZE: u64 = std::mem::size_of::<JoinUniform>() as u64;

/// What the join takes from the frame ([`SunTrace::filter`]).
pub(crate) struct JoinFrame<'a> {
    /// The shadow maps' still images, one layer a map, kept this frame.
    pub still_images: &'a wgpu::TextureView,
    /// The prepass's motion (`Targets::motion`).
    pub motion: &'a wgpu::TextureView,
    /// `RayUniform::sun` as [`super::RayScene::finish`] filled it this frame, in the order the
    /// join's uniform takes it: the traced pixels' reach, the distance from which the rays meet
    /// the swaying plants, the rays' length, the tangent of the radius of the sun's disc.
    pub sun: [f32; 4],
    /// Where the join ends (meters): the traced pixels' reach, short of the cascades' last
    /// tenth (where they blend into the far map).
    pub end: f32,
    /// One depth step of the shadow maps (their restore by drawing may round by one).
    pub depth_step: f32,
    /// The view goes on from last frame's (no camera cut).
    pub carry: bool,
}

/// ray_sun.wgsl after the ray library, in group 1 as every pipeline of the ray pass has it.
fn shader_source() -> String {
    format!(
        "{}\n{}",
        super::shader_lib(1),
        include_str!("../shaders/ray_sun.wgsl")
    )
}

/// The trace's program, its output and the count of its rays; the filter, its history and the
/// light pass's group.
pub(crate) struct SunTrace {
    pipeline: wgpu::ComputePipeline,
    /// Group 2: `sun_raw`, the counts, the still images.
    layout: wgpu::BindGroupLayout,
    /// What the rays found, at the view's size (made again when that changes).
    raw: Option<Tex>,
    /// Rays, rays stopped, "missed" and "false" (`render.ray.sun_*`), counted while
    /// `shade_skip::COUNT_MESH_RUNS` is set.
    counts: GpuCounts,
    /// A 1x1 stand-in for the still images, where the view has none.
    no_images: wgpu::TextureView,
    /// This frame's group 2 ([`begin`](Self::begin)), and whether the frame counts.
    group: Option<wgpu::BindGroup>,
    counting: bool,
    /// The filter (ray_filters.wgsl `fs_sun_temporal`) and its group 1: `sun_raw`, last
    /// frame's history, the motion, the join's uniform.
    filter: wgpu::RenderPipeline,
    filter_layout: wgpu::BindGroupLayout,
    /// The light pass's group 1 (effects.rs `sun_join_layout`) and the uniform both read.
    join_layout: wgpu::BindGroupLayout,
    join: wgpu::Buffer,
    /// The history at the view's size, two of them: the filter writes `history[current]`
    /// from the other.
    history: Option<[Tex; 2]>,
    current: usize,
    /// `history[current]` holds the last frame's filter (not after a frame without it, nor at
    /// a new size).
    history_ready: bool,
    /// This frame's group 1 of the light pass, once the filter ran ([`filter`](Self::filter)).
    join_group: Option<wgpu::BindGroup>,
    /// The last frame joined, and its filter read the frame before's history
    /// (`render.ray.sun_joined`, `render.ray.sun_history`).
    joined: bool,
    carried: bool,
}

impl SunTrace {
    /// `scene`: the ray scene's layout (group 1).
    pub(crate) fn new(shared: &GpuShared, scene: &wgpu::BindGroupLayout) -> Self {
        let device = &shared.device;
        let compute = wgpu::ShaderStages::COMPUTE;
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("ray shadow trace"),
            entries: &[
                layout_entry(
                    0,
                    compute,
                    wgpu::BindingType::StorageTexture {
                        access: wgpu::StorageTextureAccess::WriteOnly,
                        format: RAW_FORMAT,
                        view_dimension: wgpu::TextureViewDimension::D2,
                    },
                ),
                layout_entry(
                    1,
                    compute,
                    wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Storage { read_only: false },
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                ),
                layout_entry(
                    2,
                    compute,
                    wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Depth,
                        view_dimension: wgpu::TextureViewDimension::D2Array,
                        multisampled: false,
                    },
                ),
            ],
        });
        // The frame's group and the ray scene's as every pipeline of the ray pass has them (a
        // pipeline with another layout there would have wgpu walk the scene's instances again).
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("ray shadow trace"),
            bind_group_layouts: &[Some(&shared.frame_layout), Some(scene), Some(&layout)],
            immediate_size: 0,
        });
        let module = shared.shader("ray shadow trace", &shader_source(), true);
        let pipeline = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
            label: Some("cs_sun_trace"),
            layout: Some(&pipeline_layout),
            module: &module,
            entry_point: Some("cs_sun_trace"),
            compilation_options: Default::default(),
            cache: None,
        });
        let no_images = device
            .create_texture(&wgpu::TextureDescriptor {
                label: Some("ray shadow trace still images (none)"),
                size: wgpu::Extent3d {
                    width: 1,
                    height: 1,
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: wgpu::TextureFormat::Depth32Float,
                usage: wgpu::TextureUsages::TEXTURE_BINDING,
                view_formats: &[],
            })
            .create_view(&wgpu::TextureViewDescriptor {
                dimension: Some(wgpu::TextureViewDimension::D2Array),
                ..Default::default()
            });
        let float_tex = |binding| {
            layout_entry(
                binding,
                wgpu::ShaderStages::FRAGMENT,
                wgpu::BindingType::Texture {
                    sample_type: wgpu::TextureSampleType::Float { filterable: false },
                    view_dimension: wgpu::TextureViewDimension::D2,
                    multisampled: false,
                },
            )
        };
        let filter_layout = shared.bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("ray shadows filter"),
            entries: &[
                float_tex(0),
                float_tex(1),
                float_tex(2),
                layout_entry(
                    3,
                    wgpu::ShaderStages::FRAGMENT,
                    wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                ),
            ],
        });
        let filter_module = shared.shader(
            "ray shadows filter",
            include_str!("../shaders/ray_filters.wgsl"),
            true,
        );
        let filter = fullscreen_pipeline(
            shared,
            "ray shadows filter",
            &filter_module,
            "vs_sun_filter",
            "fs_sun_temporal",
            &[&shared.frame_layout, &filter_layout],
            HISTORY_FORMAT,
            None,
        );
        let join = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("ray shadows join"),
            size: JOIN_UNIFORM_SIZE,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        Self {
            pipeline,
            layout,
            raw: None,
            counts: GpuCounts::new(device, "ray shadow trace counts"),
            no_images,
            group: None,
            counting: false,
            filter,
            filter_layout,
            join_layout: crate::effects::sun_join_layout(shared),
            join,
            history: None,
            current: 0,
            history_ready: false,
            join_group: None,
            joined: false,
            carried: false,
        }
    }

    /// Before the frame's ray pass: this frame's group 2, with the counts that arrived taken in
    /// and this frame's cleared when `count` asks for them. `still_images`: the shadow maps'
    /// still images, one layer a map (`None`: the view has none).
    pub(crate) fn begin(
        &mut self,
        device: &wgpu::Device,
        encoder: &mut wgpu::CommandEncoder,
        targets: &Targets,
        still_images: Option<&wgpu::TextureView>,
        count: bool,
    ) {
        self.counts.take();
        if !count {
            self.counts.latest = None;
        }
        let size = (targets.width, targets.height);
        let raw = match &mut self.raw {
            Some(t) if (t.width, t.height) == size => t,
            slot => slot.insert(Tex::new(
                device,
                "ray shadow trace",
                size.0,
                size.1,
                RAW_FORMAT,
                wgpu::TextureUsages::STORAGE_BINDING | wgpu::TextureUsages::TEXTURE_BINDING,
            )),
        };
        self.group = Some(device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("ray shadow trace"),
            layout: &self.layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(&raw.view),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: self.counts.counts.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: wgpu::BindingResource::TextureView(
                        still_images.unwrap_or(&self.no_images),
                    ),
                },
            ],
        }));
        self.counting = count;
        if count {
            self.counts.clear(encoder);
        }
        self.join_group = None;
    }

    /// In the frame's ray pass (the frame's group and the scene's set): the trace, one
    /// invocation a pixel of the view.
    pub(crate) fn dispatch(&self, pass: &mut wgpu::ComputePass<'_>, targets: &Targets) {
        let Some(group) = &self.group else {
            return;
        };
        pass.set_bind_group(2, group, &[]);
        pass.set_pipeline(&self.pipeline);
        pass.dispatch_workgroups(targets.width.div_ceil(8), targets.height.div_ceil(8), 1);
    }

    /// After the frame's ray pass: the counts copied out, when the frame counted them.
    pub(crate) fn end(&mut self, encoder: &mut wgpu::CommandEncoder) {
        if self.group.take().is_some() {
            self.counts.copy(encoder, self.counting);
        }
    }

    /// The latest counts read back (rays, stopped, missed, false), while the frames count.
    pub(crate) fn counts(&self) -> Option<[u32; 4]> {
        self.counts.latest
    }

    /// After the frame's ray pass and before the screen light pass (`light_group` the light
    /// pass's group 0, with the prepass's depth): the filter, pass "ray shadows filter", from
    /// this frame's rays and last frame's history into this frame's, and the light pass's group
    /// 1 that joins it ([`join_group`](Self::join_group)).
    pub(crate) fn filter(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        light_group: &wgpu::BindGroup,
        targets: &Targets,
        frame: JoinFrame<'_>,
    ) {
        let Some(raw) = &self.raw else {
            self.skip_filter();
            return;
        };
        let size = (targets.width, targets.height);
        if self
            .history
            .as_ref()
            .is_none_or(|h| (h[0].width, h[0].height) != size)
        {
            let usage =
                wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING;
            self.history = Some(
                ["ray shadows history 0", "ray shadows history 1"]
                    .map(|label| Tex::new(device, label, size.0, size.1, HISTORY_FORMAT, usage)),
            );
            self.history_ready = false;
        }
        let history = self.history.as_ref().expect("made above");
        let carry = self.history_ready && frame.carry;
        self.current ^= 1;
        let (now, before) = (&history[self.current], &history[self.current ^ 1]);
        let [reach, plants_from, length, tan_radius] = frame.sun;
        queue.write_buffer(
            &self.join,
            0,
            bytemuck::bytes_of(&JoinUniform {
                params: [1.0, reach, plants_from, length],
                depth: [
                    frame.depth_step,
                    frame.end,
                    tan_radius,
                    f32::from(u8::from(carry)),
                ],
            }),
        );
        let view = wgpu::BindingResource::TextureView;
        let filter_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("ray shadows filter"),
            layout: &self.filter_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: view(&raw.view),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: view(&before.view),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: view(frame.motion),
                },
                wgpu::BindGroupEntry {
                    binding: 3,
                    resource: self.join.as_entire_binding(),
                },
            ],
        });
        {
            let mut pass = fullscreen_pass(encoder, "ray shadows filter", &now.view, true);
            pass.set_pipeline(&self.filter);
            pass.set_bind_group(0, light_group, &[]);
            pass.set_bind_group(1, &filter_group, &[]);
            pass.draw(0..3, 0..1);
        }
        self.join_group = Some(device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("ray shadows join"),
            layout: &self.join_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: view(&now.view),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: view(frame.still_images),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: self.join.as_entire_binding(),
                },
            ],
        }));
        self.history_ready = true;
        self.joined = true;
        self.carried = carry;
    }

    /// A frame the trace is not joined in (measured alone, the maps' still images not kept,
    /// the light pass off): the light pass takes the maps alone, and the next filter starts
    /// its history again.
    pub(crate) fn skip_filter(&mut self) {
        self.history_ready = false;
        self.join_group = None;
        self.joined = false;
        self.carried = false;
    }

    /// The light pass's group 1 this frame, when the filter ran.
    pub(crate) fn join_group(&self) -> Option<&wgpu::BindGroup> {
        self.join_group.as_ref()
    }

    /// Whether the last frame joined the trace, and whether its filter read the history.
    pub(crate) fn joined(&self) -> (bool, bool) {
        (self.joined, self.carried)
    }
}

#[cfg(test)]
mod tests {
    /// ray_sun.wgsl composes with the ray library and the frame's, and passes validation.
    #[test]
    fn the_suns_trace_validates() {
        let code = crate::features::compose_shader(&super::shader_source(), true);
        crate::features::validate_wgsl("ray_sun", &code).unwrap();
    }

    /// The filter composes with the frame's library alone (no ray queries: it is made on
    /// every device that traces) and passes validation.
    #[test]
    fn the_suns_filter_validates() {
        let code =
            crate::features::compose_shader(include_str!("../shaders/ray_filters.wgsl"), true);
        crate::features::validate_wgsl("ray_filters", &code).unwrap();
    }
}
