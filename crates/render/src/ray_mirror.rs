//! The mirrors' pictures traced in the frame's ray pass (`trial_cut_more::RAY_MIRROR_PICTURES`,
//! step 12 of the ray-traced lighting round, the water's mirror ray): every
//! [`crate::PlanarReflection`] the main camera sees from above gets its picture from the frame's
//! rays in place of the extra view that draws it (planar_reflection.rs).
//!
//! The picture is fully told by where the mirror's view camera goes, which
//! [`crate::planar_reflection::place_reflection_views`] works out as before: the mirrored eye,
//! its off-axis frustum around the part of the screen the mirror covers, its near plane, its
//! size. Each texel's ray from the mirrored eye meets the plane at a point, and past it is
//! exactly the main view's ray through that point reflected in the plane. So each texel is
//! every pixel's reflection (ray_reflect.wgsl `reflect_at`) at roughness 0 from that point: the
//! screen first, then a ray 384 m long through the frame's ray scene, its hit lit as every
//! pixel's reflection lights its hits (ray_hit_light.wgsl `reflected_hit_light`, or taken from
//! last frame's picture where the camera sees it) and, where the ray lights it, fogged over its
//! whole mirrored path from the mirrored eye (`reflected_path_fog`, where every pixel's
//! reflection fogs from the camera), then what the screen shows farther out, then the sky
//! (ray_mirror.wgsl).
//! Its depth is what the drawn view's would be (reverse-Z, the near plane over the depth along
//! the mirrored view), so the picture's readers (the water's `mirror_reflection`) find what
//! lies beyond the plane and what under it as before.
//!
//! With temporal anti-aliasing each frame's ray goes through another point of its texel, and
//! the picture keeps a history of its own, as a drawn view does (mirror_resolve.wgsl): each
//! texel blends this frame's ray into what it held where last frame's mirrored view saw its
//! point, so a still picture holds still and its outlines are smooth. The main view's history
//! can't do it for the picture, which the water reads through the unjittered view: the picture
//! would move under it by a share of a texel, a pixel or more across.
//!
//! Per mirror the frame keeps a [`TracedPicture`]: the colour's mip chain (the history writes
//! level 0, "ray mirror levels" filters the rest), this frame's rays and last frame's level 0
//! for the history, the depth the trace writes as `R32Float` and its copy into a depth texture
//! (readers bind a depth texture, and depth formats can't be written by a compute shader), and
//! the trace's and the history's uniforms. Their views stay the same from frame to frame, so
//! readers that cache bind groups by view keep theirs.
//!
//! What lies past the rays' reach that the screen doesn't show (a ridge above the top edge, in
//! the lake) is the sky in the picture, unless `trial_cut_more::RAY_MIRROR_FAR` draws the
//! mirror's view as the picture's far view: through the picture's frustum, smaller, with its
//! near plane a tenth short of where the trace's shortest rays end
//! (`planar_reflection::far_near`, [`MIRROR_FAR_OVERLAP`]). Past its own ray's reach a texel
//! takes what the view drew there, by its coverage, over what the screen shows or the sky: only
//! what lies past where its ray begins to give way to the view (drawn from one near plane, the
//! view also holds what the rays hit, which a neighbouring texel would fringe). A hit in the
//! reach's last tenth blends into it by distance where the view drew the same thing. Where the
//! screen doesn't show the sky there, the sky is the one the view drew (read clear of what it
//! drew round it, whose outline its anti-aliasing blends into the sky texels beside it), its
//! clouds and fog as a drawn view's, in place of the environment map's (a 256 × 128 picture,
//! unfogged, 1.4° a texel, the weather's clouds marched into it more cheaply: the small ones
//! were lost). The view asks the features for the picture's detail
//! ([`crate::ViewData::detail`]), so the weather marches its clouds as finely in it as in a
//! drawn mirror's view. The picture never takes the light's disc (the sun's, or the moon's when
//! it is the light: `frame.sun_dir`) from the screen or the view: the water's glitter is that
//! light's reflection, so a disc would show it a second time. Within three times its radius of
//! the light (and two of the far view's texels more with the far view, for its anti-aliasing)
//! the sky is the one just past that, drawn out toward the light, or taken from beside it where
//! that would lie under the plane (under a low sun, where the ground is); what the screen or
//! the view drew there, along the texel's ray or along the camera's to it (they part by the
//! parallax between the eye and the mirrored eye), is held to half again the environment map's
//! sky, as their anti-aliasing blends the disc into an outline in front of it. A hit the rays
//! light themselves holds no disc and keeps its light.

use std::collections::HashMap;

use bytemuck::{Pod, Zeroable};
use engine_core::prelude::*;

use crate::features::GpuShared;
use crate::frame::ViewData;
use crate::planar_reflection::{PlanarReflection, PlanarReflectionPicture, TracedPlacement};

/// How far a texel's ray goes on from where it meets the mirror's plane (m): every pixel's
/// reflection's reach at roughness 0 (ray_mirror.wgsl `MIRROR_T_MAX`).
pub(crate) const MIRROR_RAY_M: f32 = 384.0;

/// The share of that reach, at its end, over which a texel's hits blend into the picture's far
/// view by distance (`trial_cut_more::RAY_MIRROR_FAR`), so that a ridge across the reach shows
/// no line where the rays end: the far view's near plane comes this much short of the shortest
/// ray's end (`planar_reflection::far_near`).
pub(crate) const MIRROR_FAR_OVERLAP: f32 = 0.1;

/// `MirrorTrace` in ray_mirror.wgsl: the mirrored view, camera-relative.
#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct GpuMirrorTrace {
    /// Its right axis; w: its near plane (m).
    right: [f32; 4],
    up: [f32; 4],
    /// It looks along -back.
    back: [f32; 4],
    /// The mirrored eye relative to the main view's; w: a texel's angle (its tangent's span).
    eye: [f32; 4],
    /// The picture's left, right, bottom and top edges as tangents (x / -z, y / -z).
    tangents: [f32; 4],
    /// The plane's unit normal (its reflecting side); w: `n · (point on it - main eye)`.
    plane: [f32; 4],
    /// The picture's far view (`trial_cut_more::RAY_MIRROR_FAR`): its exposure (0 without
    /// one this frame), its near plane (m), and how far along a texel's ray its hits begin to
    /// blend into the view (m, [`MIRROR_FAR_OVERLAP`] short of the reach).
    far: [f32; 4],
}

/// `MirrorResolve` in mirror_resolve.wgsl: this frame's mirrored view and last frame's, both
/// relative to this frame's main eye.
#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct GpuMirrorResolve {
    /// As the trace's; w: its near plane (m).
    right: [f32; 4],
    /// w: 1 where last frame's picture holds this one's history, else 0.
    up: [f32; 4],
    /// w: this frame's exposure over last frame's.
    back: [f32; 4],
    eye: [f32; 4],
    tangents: [f32; 4],
    prev_right: [f32; 4],
    prev_up: [f32; 4],
    prev_back: [f32; 4],
    prev_eye: [f32; 4],
    prev_tangents: [f32; 4],
}

/// Where a picture's mirrored view was, for next frame's history.
#[derive(Clone, Copy)]
struct MirrorPose {
    rotation: Quat,
    eye: DVec3,
    tangents: Vec4,
    exposure: f32,
}

/// A mirror's traced picture: what the trace writes and its readers sample.
struct TracedPicture {
    size: UVec2,
    mips: u32,
    /// HDR colour (`Rgba16Float`, pre-exposed by the main view's exposure), `mips` levels; level
    /// 0's alpha holds the frames its history holds (readers take rgb).
    chain: wgpu::Texture,
    /// Every level, for readers.
    color: wgpu::TextureView,
    /// This frame's rays, which the trace writes.
    raw: crate::targets::Tex,
    /// Last frame's level 0, copied for the history to read.
    history: crate::targets::Tex,
    /// `GpuMirrorResolve`.
    resolve_uniform: wgpu::Buffer,
    resolve_group: wgpu::BindGroup,
    /// Where this picture's view was when its level 0 was last written (none before the first
    /// time): `RayMirrors::finish`, once the frame traced it.
    pose: Option<MirrorPose>,
    /// Where it is this frame, which `finish` makes `pose`.
    pose_now: Option<MirrorPose>,
    /// Whether this frame's history reads last frame's level 0.
    with_history: bool,
    /// Reverse-Z depth as the trace writes it (0 for the sky).
    depth_raw: crate::targets::Tex,
    /// Its copy in the depth format readers bind.
    depth: crate::targets::Tex,
    /// `GpuMirrorTrace`.
    uniform: wgpu::Buffer,
    /// `depth_raw` for the depth copy.
    copy_group: wgpu::BindGroup,
    /// This frame's far view (`trial_cut_more::RAY_MIRROR_FAR`): its colour, depth and size.
    far: Option<(wgpu::TextureView, wgpu::TextureView, UVec2)>,
}

impl TracedPicture {
    fn new(
        gpu: &GpuShared,
        (copy, resolve): (&DepthCopy, &Resolve),
        size: UVec2,
        mips: u32,
    ) -> Self {
        let device = &gpu.device;
        let chain = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("ray mirror picture"),
            size: wgpu::Extent3d {
                width: size.x,
                height: size.y,
                depth_or_array_layers: 1,
            },
            mip_level_count: mips,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba16Float,
            usage: wgpu::TextureUsages::TEXTURE_BINDING
                | wgpu::TextureUsages::STORAGE_BINDING
                | wgpu::TextureUsages::RENDER_ATTACHMENT
                | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        let level0 = chain.create_view(&wgpu::TextureViewDescriptor {
            label: Some("ray mirror picture level 0"),
            base_mip_level: 0,
            mip_level_count: Some(1),
            ..Default::default()
        });
        let color = chain.create_view(&Default::default());
        let raw = crate::targets::Tex::new(
            device,
            "ray mirror rays",
            size.x,
            size.y,
            wgpu::TextureFormat::Rgba16Float,
            wgpu::TextureUsages::STORAGE_BINDING | wgpu::TextureUsages::TEXTURE_BINDING,
        );
        let history = crate::targets::Tex::new(
            device,
            "ray mirror history",
            size.x,
            size.y,
            wgpu::TextureFormat::Rgba16Float,
            wgpu::TextureUsages::COPY_DST | wgpu::TextureUsages::TEXTURE_BINDING,
        );
        let depth_raw = crate::targets::Tex::new(
            device,
            "ray mirror depth (traced)",
            size.x,
            size.y,
            wgpu::TextureFormat::R32Float,
            wgpu::TextureUsages::STORAGE_BINDING | wgpu::TextureUsages::TEXTURE_BINDING,
        );
        let depth = crate::targets::Tex::new(
            device,
            "ray mirror depth",
            size.x,
            size.y,
            gpu.formats.depth,
            wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
        );
        let uniform = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("ray mirror trace"),
            size: std::mem::size_of::<GpuMirrorTrace>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let copy_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("ray mirror depth copy"),
            layout: &copy.layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: wgpu::BindingResource::TextureView(&depth_raw.view),
            }],
        });
        let resolve_uniform = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("ray mirror history"),
            size: std::mem::size_of::<GpuMirrorResolve>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let view = wgpu::BindingResource::TextureView;
        let resolve_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("ray mirror history"),
            layout: &resolve.layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: resolve_uniform.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: view(&raw.view),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: view(&depth_raw.view),
                },
                wgpu::BindGroupEntry {
                    binding: 3,
                    resource: view(&history.view),
                },
                wgpu::BindGroupEntry {
                    binding: 4,
                    resource: wgpu::BindingResource::Sampler(&gpu.linear_clamp),
                },
                wgpu::BindGroupEntry {
                    binding: 5,
                    resource: view(&level0),
                },
            ],
        });
        Self {
            size,
            mips,
            chain,
            color,
            raw,
            history,
            resolve_uniform,
            resolve_group,
            pose: None,
            pose_now: None,
            with_history: false,
            depth_raw,
            depth,
            uniform,
            copy_group,
            far: None,
        }
    }

    /// Bytes on the GPU: the chain (a third over its level 0), this frame's rays and last
    /// frame's level 0, the two depths.
    fn bytes(&self) -> u64 {
        let texels = self.size.x as u64 * self.size.y as u64;
        texels * 8 * 4 / 3 + texels * 8 * 2 + texels * 4 * 2
    }
}

/// The history: each texel's ray blended into what the texel held (mirror_resolve.wgsl).
struct Resolve {
    pipeline: wgpu::ComputePipeline,
    layout: wgpu::BindGroupLayout,
}

impl Resolve {
    fn new(gpu: &GpuShared) -> Self {
        let device = &gpu.device;
        let module = gpu.shader(
            "ray mirror history",
            include_str!("shaders/mirror_resolve.wgsl"),
            false,
        );
        let texture = |binding: u32, filterable: bool| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: wgpu::ShaderStages::COMPUTE,
            ty: wgpu::BindingType::Texture {
                sample_type: wgpu::TextureSampleType::Float { filterable },
                view_dimension: wgpu::TextureViewDimension::D2,
                multisampled: false,
            },
            count: None,
        };
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("ray mirror history"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::COMPUTE,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
                texture(1, false),
                texture(2, false),
                texture(3, true),
                wgpu::BindGroupLayoutEntry {
                    binding: 4,
                    visibility: wgpu::ShaderStages::COMPUTE,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 5,
                    visibility: wgpu::ShaderStages::COMPUTE,
                    ty: wgpu::BindingType::StorageTexture {
                        access: wgpu::StorageTextureAccess::WriteOnly,
                        format: wgpu::TextureFormat::Rgba16Float,
                        view_dimension: wgpu::TextureViewDimension::D2,
                    },
                    count: None,
                },
            ],
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("ray mirror history"),
            bind_group_layouts: &[Some(&layout)],
            immediate_size: 0,
        });
        let pipeline = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
            label: Some("ray mirror history"),
            layout: Some(&pipeline_layout),
            module: &module,
            entry_point: Some("cs_mirror_resolve"),
            compilation_options: Default::default(),
            cache: None,
        });
        Self { pipeline, layout }
    }
}

/// The depth copy: a full-screen triangle writing each texel's traced depth (mirror_depth.wgsl).
struct DepthCopy {
    pipeline: wgpu::RenderPipeline,
    layout: wgpu::BindGroupLayout,
}

impl DepthCopy {
    fn new(gpu: &GpuShared) -> Self {
        let device = &gpu.device;
        let module = gpu.shader(
            "ray mirror depth",
            include_str!("shaders/mirror_depth.wgsl"),
            false,
        );
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("ray mirror depth copy"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Texture {
                    sample_type: wgpu::TextureSampleType::Float { filterable: false },
                    view_dimension: wgpu::TextureViewDimension::D2,
                    multisampled: false,
                },
                count: None,
            }],
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("ray mirror depth copy"),
            bind_group_layouts: &[Some(&layout)],
            immediate_size: 0,
        });
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("ray mirror depth copy"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &module,
                entry_point: Some("vs_full"),
                compilation_options: Default::default(),
                buffers: &[],
            },
            primitive: Default::default(),
            depth_stencil: Some(wgpu::DepthStencilState {
                format: gpu.formats.depth,
                depth_write_enabled: Some(true),
                depth_compare: Some(wgpu::CompareFunction::Always),
                stencil: Default::default(),
                bias: Default::default(),
            }),
            multisample: Default::default(),
            fragment: Some(wgpu::FragmentState {
                module: &module,
                entry_point: Some("fs_depth"),
                compilation_options: Default::default(),
                targets: &[],
            }),
            multiview_mask: None,
            cache: None,
        });
        Self { pipeline, layout }
    }
}

/// What the trace of one mirror's picture binds (group 2 of `cs_mirror_picture`, besides the
/// inputs every pixel's reflection reads): [`RayMirrors::due`].
pub(crate) struct MirrorTarget<'a> {
    pub uniform: &'a wgpu::Buffer,
    pub color: &'a wgpu::TextureView,
    pub depth: &'a wgpu::TextureView,
    pub size: UVec2,
    /// The picture's far view this frame: its colour and depth.
    pub far: Option<(&'a wgpu::TextureView, &'a wgpu::TextureView)>,
}

/// The main view's traced mirror pictures, the renderer's.
#[derive(Default)]
pub(crate) struct RayMirrors {
    pictures: HashMap<Entity, TracedPicture>,
    /// The mirrors traced this frame, in the order they dispatch.
    due: Vec<Entity>,
    copy: Option<(DepthCopy, Resolve)>,
}

impl RayMirrors {
    /// Whether `settings` ask for the mirrors' pictures from the rays on this device: their
    /// switch, and the frame's ray scene ([`crate::ray_scene::wanted`]).
    pub fn wanted(settings: &crate::RenderSettings, shared: &GpuShared) -> bool {
        settings.trial_cuts_more & crate::settings::trial_cut_more::RAY_MIRROR_PICTURES != 0
            && crate::ray_scene::wanted(settings, shared)
    }

    /// In the main view's extract, once the frame's ray scene is settled and before the
    /// features prepare: each mirror placed to be traced (`placed`) gets this frame's picture,
    /// which the frame's ray pass then traces, while `can_trace` (the scene is there and the
    /// rays trace it); without, it has none this frame. `wanted`: [`Self::wanted`] (the
    /// pictures are freed when not); `taa`: whether the main view has temporal anti-aliasing,
    /// and with it the pictures their history. Also every frame without mirrors.
    pub fn publish(
        &mut self,
        world: &mut World,
        gpu: &GpuShared,
        view: &ViewData,
        placed: Vec<TracedPlacement>,
        (wanted, can_trace, taa): (bool, bool, bool),
    ) {
        self.due.clear();
        if !wanted {
            self.pictures.clear();
        }
        self.pictures
            .retain(|mirror, _| world.get::<PlanarReflection>(*mirror).is_some());
        for p in placed {
            if !can_trace {
                crate::planar_reflection::drop_traced(world, p.mirror);
                continue;
            }
            let size = p.view.size.max(UVec2::ONE);
            let mips = crate::planar_reflection::picture_mips(size);
            let (copy, resolve) = &*self
                .copy
                .get_or_insert_with(|| (DepthCopy::new(gpu), Resolve::new(gpu)));
            let picture = match self.pictures.entry(p.mirror) {
                std::collections::hash_map::Entry::Occupied(e) => {
                    let picture = e.into_mut();
                    if picture.size != size || picture.mips != mips {
                        *picture = TracedPicture::new(gpu, (copy, resolve), size, mips);
                    }
                    picture
                }
                std::collections::hash_map::Entry::Vacant(e) => {
                    e.insert(TracedPicture::new(gpu, (copy, resolve), size, mips))
                }
            };
            let rot = p.at.rotation();
            let (right, up, back) = (rot * Vec3::X, rot * Vec3::Y, rot * Vec3::Z);
            let t = p.view.tangents;
            let texel = ((t.y - t.x) / size.x as f32).max((t.w - t.z) / size.y as f32);
            let n = p.view.plane_normal.normalize();
            let plane = n.as_dvec3().dot(p.view.plane_point - view.origin) as f32;
            // Its far view, drawn this frame before the main view (`RAY_MIRROR_FAR`): through
            // the same frustum from the same eye, so a texel's place in it is the same.
            let far = world
                .get_resource::<crate::planar_reflection::MirrorPictures>()
                .and_then(|pictures| pictures.far.get(&p.mirror))
                .filter(|far| far.camera == p.camera)
                .map(|far| {
                    (
                        far.color.clone(),
                        far.depth.clone(),
                        far.size,
                        far.exposure,
                        far.near,
                    )
                });
            let trace = GpuMirrorTrace {
                right: right.extend(p.view.near).to_array(),
                up: up.extend(0.0).to_array(),
                back: back.extend(0.0).to_array(),
                eye: view.relative(p.at.translation()).extend(texel).to_array(),
                tangents: t.to_array(),
                plane: n.extend(plane).to_array(),
                far: far.as_ref().map_or([0.0; 4], |f| {
                    let blend = MIRROR_RAY_M * (1.0 - MIRROR_FAR_OVERLAP);
                    [f.3.max(f32::MIN_POSITIVE), f.4, blend, 0.0]
                }),
            };
            picture.far = far.map(|(color, depth, size, ..)| (color, depth, size));
            gpu.queue
                .write_buffer(&picture.uniform, 0, bytemuck::bytes_of(&trace));
            // The history, from where the view was when level 0 was last written: none after a
            // camera cut, or without temporal anti-aliasing (the picture is then this frame's
            // rays, as a drawn view's is its frame).
            let pose = MirrorPose {
                rotation: rot,
                eye: p.at.translation(),
                tangents: t,
                exposure: view.exposure,
            };
            let last = picture
                .pose
                .filter(|_| taa && view.history_valid)
                .unwrap_or(pose);
            picture.with_history = taa && view.history_valid && picture.pose.is_some();
            let (last_right, last_up, last_back) = (
                last.rotation * Vec3::X,
                last.rotation * Vec3::Y,
                last.rotation * Vec3::Z,
            );
            let resolve = GpuMirrorResolve {
                right: trace.right,
                up: up
                    .extend(if picture.with_history { 1.0 } else { 0.0 })
                    .to_array(),
                back: back
                    .extend(view.exposure / last.exposure.max(f32::MIN_POSITIVE))
                    .to_array(),
                eye: trace.eye,
                tangents: trace.tangents,
                prev_right: last_right.extend(0.0).to_array(),
                prev_up: last_up.extend(0.0).to_array(),
                prev_back: last_back.extend(0.0).to_array(),
                prev_eye: view.relative(last.eye).extend(0.0).to_array(),
                prev_tangents: last.tangents.to_array(),
            };
            gpu.queue
                .write_buffer(&picture.resolve_uniform, 0, bytemuck::bytes_of(&resolve));
            picture.pose_now = Some(pose);
            crate::planar_reflection::keep_traced(
                world,
                p.mirror,
                PlanarReflectionPicture {
                    camera: p.camera,
                    color: picture.color.clone(),
                    depth: picture.depth.view.clone(),
                    mips,
                    size,
                    view_proj: crate::planar_reflection::mirror_view_proj(&p.view, &p.at),
                    origin: p.at.translation(),
                    near: p.view.near,
                    // (Pre-exposed by the main view's exposure: readers' ratio is 1.)
                    exposure: view.exposure,
                    plane_point: p.view.plane_point,
                    plane_normal: n,
                },
            );
            self.due.push(p.mirror);
        }
    }

    /// The pictures this frame traces, as their trace binds them.
    pub fn due(&self) -> impl Iterator<Item = MirrorTarget<'_>> {
        self.due.iter().filter_map(|mirror| {
            let p = self.pictures.get(mirror)?;
            Some(MirrorTarget {
                uniform: &p.uniform,
                color: &p.raw.view,
                depth: &p.depth_raw.view,
                size: p.size,
                far: p.far.as_ref().map(|(color, depth, _)| (color, depth)),
            })
        })
    }

    /// After the pass that traced them: each picture's history (its level 0), its levels, and
    /// its depth copied into the depth texture readers bind.
    pub fn finish(&mut self, gpu: &GpuShared, encoder: &mut wgpu::CommandEncoder) {
        let Some((copy, resolve)) = &self.copy else {
            return;
        };
        for mirror in &self.due {
            let Some(p) = self.pictures.get_mut(mirror) else {
                continue;
            };
            // (A frame that published the picture but didn't trace it leaves its level 0, and so
            // where it was seen from, as it was.)
            p.pose = p.pose_now.take();
            if p.with_history {
                encoder.copy_texture_to_texture(
                    p.chain.as_image_copy(),
                    p.history.texture.as_image_copy(),
                    wgpu::Extent3d {
                        width: p.size.x,
                        height: p.size.y,
                        depth_or_array_layers: 1,
                    },
                );
            }
            {
                let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
                    label: Some("ray mirror history"),
                    timestamp_writes: None,
                });
                pass.set_pipeline(&resolve.pipeline);
                pass.set_bind_group(0, &p.resolve_group, &[]);
                pass.dispatch_workgroups(p.size.x.div_ceil(8), p.size.y.div_ceil(8), 1);
            }
            gpu.mips
                .record(gpu, encoder, &p.chain, wgpu::TextureFormat::Rgba16Float);
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("ray mirror depth"),
                color_attachments: &[],
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                    view: &p.depth.view,
                    depth_ops: Some(wgpu::Operations {
                        load: wgpu::LoadOp::Clear(0.0),
                        store: wgpu::StoreOp::Store,
                    }),
                    stencil_ops: None,
                }),
                ..Default::default()
            });
            pass.set_pipeline(&copy.pipeline);
            pass.set_bind_group(0, &p.copy_group, &[]);
            pass.draw(0..3, 0..1);
        }
    }

    /// `render.ray.mirror_pictures` (traced this frame), `mirror_texels` (their level 0s'),
    /// `mirror_mb` (what the kept pictures hold on the GPU), `mirror_far` (their far views
    /// this frame, `trial_cut_more::RAY_MIRROR_FAR`) and `mirror_far_texels`.
    pub fn publish_diagnostics(&self, d: &mut Diagnostics) {
        let due = || self.due.iter().filter_map(|m| self.pictures.get(m));
        let texels: u64 = due().map(|p| p.size.x as u64 * p.size.y as u64).sum();
        let far = || due().filter_map(|p| p.far.as_ref().map(|f| f.2));
        let far_texels: u64 = far().map(|s| s.x as u64 * s.y as u64).sum();
        let bytes: u64 = self.pictures.values().map(TracedPicture::bytes).sum();
        d.set("render.ray.mirror_pictures", self.due.len() as f64);
        d.set("render.ray.mirror_texels", texels as f64);
        d.set("render.ray.mirror_mb", bytes as f64 / (1024.0 * 1024.0));
        d.set("render.ray.mirror_far", far().count() as f64);
        d.set("render.ray.mirror_far_texels", far_texels as f64);
    }

    /// Once after the frame's ray scene went: its rows read zero.
    pub fn publish_none(d: &mut Diagnostics) {
        for row in [
            "mirror_pictures",
            "mirror_texels",
            "mirror_mb",
            "mirror_far",
            "mirror_far_texels",
        ] {
            d.set(&format!("render.ray.{row}"), 0.0);
        }
    }
}
