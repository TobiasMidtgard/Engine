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
//! screen first, then a ray 384 m long through the frame's ray scene, lit as a world
//! reflection's hit, then what the screen shows farther out, then the sky (ray_mirror.wgsl).
//! Its depth is what the drawn view's would be (reverse-Z, the near plane over the depth along
//! the mirrored view), so the picture's readers (the water's `mirror_reflection`) find what
//! lies beyond the plane and what under it as before.
//!
//! Per mirror the frame keeps a [`TracedPicture`]: the colour's mip chain (the trace writes
//! level 0, "ray mirror levels" filters the rest), the depth the trace writes as `R32Float` and
//! its copy into a depth texture (readers bind a depth texture, and depth formats can't be
//! written by a compute shader), and the trace's uniform. Their views stay the same from frame
//! to frame, so readers that cache bind groups by view keep theirs.

use std::collections::HashMap;

use bytemuck::{Pod, Zeroable};
use engine_core::prelude::*;

use crate::features::GpuShared;
use crate::frame::ViewData;
use crate::planar_reflection::{PlanarReflection, PlanarReflectionPicture, TracedPlacement};

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
}

/// A mirror's traced picture: what the trace writes and its readers sample.
struct TracedPicture {
    size: UVec2,
    mips: u32,
    /// HDR colour (`Rgba16Float`, pre-exposed by the main view's exposure), `mips` levels.
    chain: wgpu::Texture,
    /// Level 0, which the trace writes.
    level0: wgpu::TextureView,
    /// Every level, for readers.
    color: wgpu::TextureView,
    /// Reverse-Z depth as the trace writes it (0 for the sky).
    depth_raw: crate::targets::Tex,
    /// Its copy in the depth format readers bind.
    depth: crate::targets::Tex,
    /// `GpuMirrorTrace`.
    uniform: wgpu::Buffer,
    /// `depth_raw` for the depth copy.
    copy_group: wgpu::BindGroup,
}

impl TracedPicture {
    fn new(gpu: &GpuShared, copy: &DepthCopy, size: UVec2, mips: u32) -> Self {
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
                | wgpu::TextureUsages::RENDER_ATTACHMENT,
            view_formats: &[],
        });
        let level0 = chain.create_view(&wgpu::TextureViewDescriptor {
            label: Some("ray mirror picture level 0"),
            base_mip_level: 0,
            mip_level_count: Some(1),
            ..Default::default()
        });
        let color = chain.create_view(&Default::default());
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
        Self {
            size,
            mips,
            chain,
            level0,
            color,
            depth_raw,
            depth,
            uniform,
            copy_group,
        }
    }

    /// Bytes on the GPU: the chain (a third over its level 0), the two depths.
    fn bytes(&self) -> u64 {
        let texels = self.size.x as u64 * self.size.y as u64;
        texels * 8 * 4 / 3 + texels * 4 * 2
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
}

/// The main view's traced mirror pictures, the renderer's.
#[derive(Default)]
pub(crate) struct RayMirrors {
    pictures: HashMap<Entity, TracedPicture>,
    /// The mirrors traced this frame, in the order they dispatch.
    due: Vec<Entity>,
    copy: Option<DepthCopy>,
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
    /// pictures are freed when not). Also every frame without mirrors.
    pub fn publish(
        &mut self,
        world: &mut World,
        gpu: &GpuShared,
        view: &ViewData,
        placed: Vec<TracedPlacement>,
        (wanted, can_trace): (bool, bool),
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
            let copy = self.copy.get_or_insert_with(|| DepthCopy::new(gpu));
            let picture = match self.pictures.entry(p.mirror) {
                std::collections::hash_map::Entry::Occupied(e) => {
                    let picture = e.into_mut();
                    if picture.size != size || picture.mips != mips {
                        *picture = TracedPicture::new(gpu, copy, size, mips);
                    }
                    picture
                }
                std::collections::hash_map::Entry::Vacant(e) => {
                    e.insert(TracedPicture::new(gpu, copy, size, mips))
                }
            };
            let rot = p.at.rotation();
            let (right, up, back) = (rot * Vec3::X, rot * Vec3::Y, rot * Vec3::Z);
            let t = p.view.tangents;
            let texel = ((t.y - t.x) / size.x as f32).max((t.w - t.z) / size.y as f32);
            let n = p.view.plane_normal.normalize();
            let plane = n.as_dvec3().dot(p.view.plane_point - view.origin) as f32;
            let trace = GpuMirrorTrace {
                right: right.extend(p.view.near).to_array(),
                up: up.extend(0.0).to_array(),
                back: back.extend(0.0).to_array(),
                eye: view.relative(p.at.translation()).extend(texel).to_array(),
                tangents: t.to_array(),
                plane: n.extend(plane).to_array(),
            };
            gpu.queue
                .write_buffer(&picture.uniform, 0, bytemuck::bytes_of(&trace));
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
                color: &p.level0,
                depth: &p.depth_raw.view,
                size: p.size,
            })
        })
    }

    /// After the pass that traced them: each picture's levels, and its depth copied into the
    /// depth texture readers bind.
    pub fn finish(&self, gpu: &GpuShared, encoder: &mut wgpu::CommandEncoder) {
        let Some(copy) = &self.copy else {
            return;
        };
        for p in self.due.iter().filter_map(|m| self.pictures.get(m)) {
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
    /// `mirror_mb` (what the kept pictures hold on the GPU).
    pub fn publish_diagnostics(&self, d: &mut Diagnostics) {
        let texels: u64 = self
            .due
            .iter()
            .filter_map(|m| self.pictures.get(m))
            .map(|p| p.size.x as u64 * p.size.y as u64)
            .sum();
        let bytes: u64 = self.pictures.values().map(TracedPicture::bytes).sum();
        d.set("render.ray.mirror_pictures", self.due.len() as f64);
        d.set("render.ray.mirror_texels", texels as f64);
        d.set("render.ray.mirror_mb", bytes as f64 / (1024.0 * 1024.0));
    }

    /// Once after the frame's ray scene went: its rows read zero.
    pub fn publish_none(d: &mut Diagnostics) {
        for row in ["mirror_pictures", "mirror_texels", "mirror_mb"] {
            d.set(&format!("render.ray.{row}"), 0.0);
        }
    }
}
