//! Meshes work out their material once a pixel ([`trial_cut::MESHES_ONCE`], a trial cut).
//!
//! The usual way draws every opaque mesh twice: the depth prepass runs the whole material
//! (textures, triplanar blends, parallax, detail, weather) for the normals and roughness, at
//! every layer it happens to draw, and the scene pass runs it again at every pixel before
//! lighting it, once for each triangle that touches the pixel's 2x2 block.
//!
//! With the cut on, the opaque meshes leave the shared prepass for a pass of their own right
//! after it ([`RenderFeature::prepass_own`]). It writes what the shared prepass writes (depth,
//! normals, roughness, motion) and stores the rest of the material beside it (mesh.wgsl
//! `fs_once`, [`FORMATS`]). The scene pass draws none of them: one triangle over the screen
//! lights each stored pixel once (mesh_once.wgsl). So the material is worked out in one pass
//! instead of two, the meshes' vertices are drawn once instead of twice, and the lighting runs
//! once a pixel.
//!
//! Impostors and virtual geometry keep the usual way, as do extra views and debug views.
//!
//! [`trial_cut::MESHES_ONCE`]: crate::settings::trial_cut::MESHES_ONCE
//! [`RenderFeature::prepass_own`]: crate::features::RenderFeature::prepass_own

use crate::features::{GpuShared, PrepassTargets};

/// What the meshes' own prepass stores beside the shared prepass's normals and motion, in the
/// order of mesh.wgsl's `OnceOutput`: albedo and occlusion; emissive light and parallax
/// self-shadowing; the smooth normal and the side in view; metallic, subsurface, reflectance
/// and alpha; the depth drawn and the kind of surface.
pub(crate) const FORMATS: [wgpu::TextureFormat; 5] = [
    wgpu::TextureFormat::Rgba16Float,
    wgpu::TextureFormat::Rgba16Float,
    wgpu::TextureFormat::Rgba16Float,
    wgpu::TextureFormat::Rgba8Unorm,
    wgpu::TextureFormat::Rg32Float,
];

/// Bytes a pixel of the pass's seven color targets together (normals 8, motion 4, then
/// [`FORMATS`]), as wgpu counts them against `Limits::max_color_attachment_bytes_per_sample`
/// (32 unless the GPU gives more).
const BYTES: u32 = 48;

/// Whether the device can run the pass at all.
pub(crate) fn supported(gpu: &GpuShared) -> bool {
    let limits = gpu.device.limits();
    limits.max_color_attachment_bytes_per_sample >= BYTES && limits.max_color_attachments >= 7
}

/// The color targets of a mesh pipeline drawn in the pass.
pub(crate) fn color_targets(gpu: &GpuShared) -> Vec<Option<wgpu::ColorTargetState>> {
    let mut targets = gpu.prepass_targets().to_vec();
    targets.extend(FORMATS.map(|format| {
        Some(wgpu::ColorTargetState {
            format,
            blend: None,
            write_mask: wgpu::ColorWrites::ALL,
        })
    }));
    targets
}

struct Stored {
    size: (u32, u32),
    views: [wgpu::TextureView; 5],
    group: wgpu::BindGroup,
}

pub(crate) struct MeshOnce {
    layout: wgpu::BindGroupLayout,
    light: wgpu::RenderPipeline,
    stored: Option<Stored>,
    /// The meshes' pipelines for the pass, indexed like `MeshFeature::pipelines`.
    pub(crate) pipelines: Vec<Option<wgpu::RenderPipeline>>,
    /// With `trial_cut::MESHES_DEPTH_FIRST`: the pipelines that draw their depth only, and
    /// those that then draw the material where the depth is equal.
    pub(crate) depth: Vec<Option<wgpu::RenderPipeline>>,
    pub(crate) at_depth: Vec<Option<wgpu::RenderPipeline>>,
}

/// What a pipeline of the pass draws.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum Draws {
    /// Depth and material together (`fs_once`).
    Whole,
    /// Depth only, first (`fs_once_depth`; the color targets are not written).
    Depth,
    /// The material where the depth drawn first is equal, tested before the program
    /// (`fs_once_early`).
    AtDepth,
}

impl Draws {
    pub(crate) fn fragment(self) -> &'static str {
        match self {
            Draws::Whole => "fs_once",
            Draws::Depth => "fs_once_depth",
            Draws::AtDepth => "fs_once_early",
        }
    }

    /// The pass's color targets as a pipeline of this kind writes them.
    pub(crate) fn targets(
        self,
        targets: &[Option<wgpu::ColorTargetState>],
    ) -> Vec<Option<wgpu::ColorTargetState>> {
        let mut targets = targets.to_vec();
        if self == Draws::Depth {
            for t in targets.iter_mut().flatten() {
                t.write_mask = wgpu::ColorWrites::empty();
            }
        }
        targets
    }

    /// Its depth state: `write` is the prepass's (test and write), `equal` the opaque scene
    /// pass's (equal, nothing written).
    pub(crate) fn depth(
        self,
        write: &wgpu::DepthStencilState,
        equal: &wgpu::DepthStencilState,
    ) -> wgpu::DepthStencilState {
        if self == Draws::AtDepth {
            equal.clone()
        } else {
            write.clone()
        }
    }
}

impl MeshOnce {
    pub(crate) fn new(gpu: &GpuShared, variants: usize) -> Self {
        let device = &gpu.device;
        let entries: Vec<_> = (0..FORMATS.len() as u32)
            .map(|binding| wgpu::BindGroupLayoutEntry {
                binding,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Texture {
                    sample_type: wgpu::TextureSampleType::Float { filterable: false },
                    view_dimension: wgpu::TextureViewDimension::D2,
                    multisampled: false,
                },
                count: None,
            })
            .collect();
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("meshes once"),
            entries: &entries,
        });
        let shader = gpu.shader("mesh once", include_str!("shaders/mesh_once.wgsl"), true);
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("meshes once"),
            bind_group_layouts: &[Some(&gpu.frame_layout), Some(&layout)],
            immediate_size: 0,
        });
        let light = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("meshes lit once"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs_once_light"),
                compilation_options: Default::default(),
                buffers: &[],
            },
            primitive: Default::default(),
            // The shader gives each pixel the prepass's depth: equal to the scene's depth
            // unless something has been drawn in front since.
            depth_stencil: Some(wgpu::DepthStencilState {
                depth_compare: Some(wgpu::CompareFunction::Equal),
                ..gpu.depth_state(false)
            }),
            multisample: Default::default(),
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs_once_light"),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format: gpu.formats.hdr,
                    blend: None,
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            multiview_mask: None,
            cache: None,
        });
        Self {
            layout,
            light,
            stored: None,
            pipelines: (0..variants).map(|_| None).collect(),
            depth: (0..variants).map(|_| None).collect(),
            at_depth: (0..variants).map(|_| None).collect(),
        }
    }

    /// Bytes of video memory the stored material takes now.
    pub(crate) fn bytes(&self) -> u64 {
        self.stored
            .as_ref()
            .map_or(0, |s| u64::from(s.size.0) * u64::from(s.size.1) * 36)
    }

    fn stored(&mut self, device: &wgpu::Device, size: (u32, u32)) -> &Stored {
        if self.stored.as_ref().is_none_or(|s| s.size != size) {
            let views = FORMATS.map(|format| {
                device
                    .create_texture(&wgpu::TextureDescriptor {
                        label: Some("meshes once"),
                        size: wgpu::Extent3d {
                            width: size.0.max(1),
                            height: size.1.max(1),
                            depth_or_array_layers: 1,
                        },
                        mip_level_count: 1,
                        sample_count: 1,
                        dimension: wgpu::TextureDimension::D2,
                        format,
                        usage: wgpu::TextureUsages::RENDER_ATTACHMENT
                            | wgpu::TextureUsages::TEXTURE_BINDING,
                        view_formats: &[],
                    })
                    .create_view(&Default::default())
            });
            let entries: Vec<_> = views
                .iter()
                .enumerate()
                .map(|(i, view)| wgpu::BindGroupEntry {
                    binding: i as u32,
                    resource: wgpu::BindingResource::TextureView(view),
                })
                .collect();
            let group = device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("meshes once"),
                layout: &self.layout,
                entries: &entries,
            });
            self.stored = Some(Stored { size, views, group });
        }
        self.stored.as_ref().expect("made above")
    }

    /// The meshes' own prepass over the shared prepass's targets (kept) and the stored
    /// material (cleared, or kept when `late`), with the prepass's group 0 bound.
    pub(crate) fn pass<'e>(
        &mut self,
        device: &wgpu::Device,
        encoder: &'e mut wgpu::CommandEncoder,
        shared: &PrepassTargets,
        late: bool,
    ) -> wgpu::RenderPass<'e> {
        let stored = self.stored(device, shared.size);
        let keep = wgpu::Operations {
            load: wgpu::LoadOp::Load,
            store: wgpu::StoreOp::Store,
        };
        let own = wgpu::Operations {
            load: if late {
                wgpu::LoadOp::Load
            } else {
                wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT)
            },
            store: wgpu::StoreOp::Store,
        };
        let mut colors: Vec<Option<wgpu::RenderPassColorAttachment>> = Vec::with_capacity(7);
        for view in [shared.normal, shared.motion] {
            colors.push(Some(wgpu::RenderPassColorAttachment {
                view,
                depth_slice: None,
                resolve_target: None,
                ops: keep,
            }));
        }
        for view in &stored.views {
            colors.push(Some(wgpu::RenderPassColorAttachment {
                view,
                depth_slice: None,
                resolve_target: None,
                ops: own,
            }));
        }
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some(if late {
                "prepass: meshes once (late)"
            } else {
                "prepass: meshes once"
            }),
            color_attachments: &colors,
            depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                view: shared.depth,
                depth_ops: Some(wgpu::Operations {
                    load: wgpu::LoadOp::Load,
                    store: wgpu::StoreOp::Store,
                }),
                stencil_ops: None,
            }),
            ..Default::default()
        });
        pass.set_bind_group(0, shared.group, &[]);
        pass
    }

    /// In the scene pass (frame group bound): light every stored pixel. Leaves its own group
    /// bound at 1.
    pub(crate) fn light(&self, pass: &mut wgpu::RenderPass<'_>) {
        let Some(stored) = &self.stored else {
            return;
        };
        pass.set_pipeline(&self.light);
        pass.set_bind_group(1, &stored.group, &[]);
        pass.draw(0..3, 0..1);
    }
}
