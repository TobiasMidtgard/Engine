//! Selection outlines, as in Unreal, Unity and Blender: the selected objects' silhouettes drawn
//! as a line over the scene view, full strength where they can be seen and faint through
//! whatever hides them, so a selected model reads at a glance even half behind a wall. The
//! active selection is light orange, the rest of the selection a deeper orange (Blender's
//! active and selected colors); a model's or demo's children are outlined with it.
//!
//! A render overlay in two passes, under the editor UI:
//! 1. Mask: the selected meshes (rigs in the pose they are drawn in) rasterized into an RGBA8
//!    mask over the window: r = the active selection covers the pixel, g = and is in front of
//!    the scene there (tested against the scene depth copy), b and a the same for the rest of
//!    the selection.
//! 2. Composite: a pixel near the mask finds the nearest covered pixel within the line width and
//!    blends the outline color, faded over its last pixel by the distance. Brute force, which
//!    Golus ("The Quest for Very Wide Outlines") measured as fine for lines a few pixels wide;
//!    the pass is clipped to the selection's screen rectangle.

use std::collections::HashMap;

use engine::prelude::*;
use engine::render::wgpu::util::DeviceExt;
use engine::render::{
    FrameContext, GpuShared, GrowBuffer, Mesh, RenderFeature, RenderSettings, ResolvedMesh,
    ViewData, wgpu,
};
use engine_anim::skin::{SkinPose, SkinnedMesh, SkinnedMeshRenderer, skin_on_cpu};

use crate::editor::Editor;
use crate::scene_ops::EditorOnly;

/// Line width in points (scaled by the UI's pixels per point).
const WIDTH_POINTS: f32 = 2.0;
/// Opacity of the outline where the selection is hidden behind something.
const HIDDEN_ALPHA: f32 = 0.4;
/// Opacity of the tint over the hidden parts of the selection.
const HIDDEN_FILL: f32 = 0.08;
/// Linear colors: the active selection, then the rest.
const ACTIVE: [f32; 3] = [1.0, 0.45, 0.06];
const OTHER: [f32; 3] = [0.85, 0.16, 0.01];
/// Stop gathering past this many triangles (a whole selected landscape, say).
const MAX_TRIANGLES: usize = 6_000_000;

/// One mesh to outline.
pub struct Part {
    pub source: Source,
    /// Mesh space to world.
    pub global: GlobalTransform,
    /// Mesh-space bounds.
    pub bounds: (Vec3, Vec3),
    pub active: bool,
}

/// Where a part's triangles come from.
pub enum Source {
    /// A static mesh asset at this version.
    Mesh(Handle<Mesh>, u64),
    /// A rig, skinned on the CPU into its current pose.
    Posed(Vec<Vec3>, Vec<u32>),
}

impl Part {
    fn triangles(&self, world: &World) -> usize {
        match &self.source {
            Source::Mesh(h, _) => world
                .resource::<Assets<Mesh>>()
                .get(*h)
                .map_or(0, |m| m.indices.len() / 3),
            Source::Posed(_, indices) => indices.len() / 3,
        }
    }
}

/// The meshes of the selection and everything under it: children, and a demo's spawned
/// entities. The active selection's parts come first and claim shared children.
pub fn selected_parts(world: &World) -> Vec<Part> {
    let Some(editor) = world.get_resource::<Editor>() else {
        return Vec::new();
    };
    if editor.game_view || !editor.show_outline {
        return Vec::new();
    }
    let mut roots: Vec<(Entity, bool)> = editor.selection.iter().map(|&e| (e, false)).collect();
    if let Some(last) = roots.pop() {
        roots.insert(0, (last.0, true));
    }
    let mut seen = std::collections::HashSet::new();
    let mut parts = Vec::new();
    let mut triangles = 0;
    for (root, active) in roots {
        let mut stack = vec![root];
        while let Some(e) = stack.pop() {
            if !seen.insert(e)
                || world.get_entity(e).is_err()
                || world.get::<EditorOnly>(e).is_some()
            {
                continue;
            }
            if let Some(children) = world.get::<Children>(e) {
                stack.extend(children.iter());
            }
            if let Some(demo) = world.get::<crate::demos::DemoInstance>(e) {
                stack.extend(demo.entities.iter().copied());
            }
            let Some(&global) = world.get::<GlobalTransform>(e) else {
                continue;
            };
            if let Some(part) = part_of(world, e, global, active) {
                triangles += part.triangles(world);
                if triangles > MAX_TRIANGLES {
                    return parts;
                }
                parts.push(part);
            }
        }
    }
    parts
}

fn part_of(world: &World, e: Entity, global: GlobalTransform, active: bool) -> Option<Part> {
    if let (Some(renderer), Some(resolved)) =
        (world.get::<MeshRenderer>(e), world.get::<ResolvedMesh>(e))
    {
        let meshes = world.resource::<Assets<Mesh>>();
        let mesh = meshes.get(resolved.mesh).filter(|_| renderer.visible)?;
        let aabb = mesh.aabb();
        return Some(Part {
            source: Source::Mesh(resolved.mesh, meshes.version(resolved.mesh)),
            global,
            bounds: (aabb.min, aabb.max),
            active,
        });
    }
    let renderer = world.get::<SkinnedMeshRenderer>(e).filter(|r| r.visible)?;
    let mesh = world
        .get_resource::<Assets<SkinnedMesh>>()?
        .get(renderer.mesh?)?;
    let points = match world.get::<SkinPose>(e) {
        Some(pose) if !pose.joints.is_empty() => skin_on_cpu(mesh, pose).0,
        _ => mesh.vertices.iter().map(|v| v.position).collect(),
    };
    let bounds = points.iter().fold(
        (Vec3::splat(f32::INFINITY), Vec3::splat(f32::NEG_INFINITY)),
        |(lo, hi), p| (lo.min(*p), hi.max(*p)),
    );
    (!points.is_empty()).then(|| Part {
        source: Source::Posed(points, mesh.indices.clone()),
        global,
        bounds,
        active,
    })
}

/// `global` relative to the camera at `origin`, as the f32 matrix the GPU draws with.
fn camera_relative(global: &GlobalTransform, origin: DVec3) -> Mat4 {
    let m = global.0.matrix3.as_mat3();
    let t = (global.0.translation - origin).as_vec3();
    Mat4::from_cols(
        m.x_axis.extend(0.0),
        m.y_axis.extend(0.0),
        m.z_axis.extend(0.0),
        t.extend(1.0),
    )
}

/// The window rectangle (x, y, w, h in pixels) the parts cover, grown by `margin`; `None` when
/// none is in front of the camera. A part reaching behind the camera covers the whole scene.
pub fn screen_rect(parts: &[Part], view: &ViewData, margin: f32) -> Option<[u32; 4]> {
    let origin = view.viewport_origin.as_vec2();
    let size = view.viewport_size.as_vec2();
    let (mut lo, mut hi) = (Vec2::splat(f32::INFINITY), Vec2::splat(f32::NEG_INFINITY));
    for part in parts {
        let m = view.unjittered_view_proj * camera_relative(&part.global, view.origin);
        let (a, b) = part.bounds;
        for i in 0..8 {
            let corner = Vec3::new(
                if i & 1 == 0 { a.x } else { b.x },
                if i & 2 == 0 { a.y } else { b.y },
                if i & 4 == 0 { a.z } else { b.z },
            );
            let clip = m * corner.extend(1.0);
            if clip.w <= 1e-4 {
                lo = Vec2::ZERO.min(lo);
                hi = Vec2::splat(f32::INFINITY);
                continue;
            }
            let ndc = Vec2::new(clip.x, clip.y) / clip.w;
            let p = origin + (ndc * Vec2::new(0.5, -0.5) + 0.5) * size;
            lo = lo.min(p);
            hi = hi.max(p);
        }
    }
    let lo = (lo - margin).max(origin).floor();
    let hi = (hi + margin).min(origin + size).ceil();
    (hi.x > lo.x && hi.y > lo.y).then_some([
        lo.x as u32,
        lo.y as u32,
        (hi.x - lo.x) as u32,
        (hi.y - lo.y) as u32,
    ])
}

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct Instance {
    model: [[f32; 4]; 4],
    /// x: 1 for the active selection.
    params: [f32; 4],
}

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct Style {
    active: [f32; 4],
    other: [f32; 4],
    /// Line width (px), hidden line opacity, hidden fill opacity.
    params: [f32; 4],
}

struct GpuMesh {
    vertices: wgpu::Buffer,
    indices: wgpu::Buffer,
    count: u32,
    used: bool,
}

enum Draw {
    Cached(u32, u64),
    Posed { first: u32, count: u32, base: i32 },
}

struct Pipelines {
    mask: wgpu::RenderPipeline,
    composite: wgpu::RenderPipeline,
    depth_layout: wgpu::BindGroupLayout,
    composite_layout: wgpu::BindGroupLayout,
    style: wgpu::Buffer,
    instances: GrowBuffer,
    posed_vertices: GrowBuffer,
    posed_indices: GrowBuffer,
}

/// Draws the selection outline. Added by the editor plugin.
#[derive(Default)]
pub struct OutlineFeature {
    gpu: Option<Pipelines>,
    meshes: HashMap<(u32, u64), GpuMesh>,
    draws: Vec<Draw>,
    mask: Option<(wgpu::Texture, wgpu::TextureView, UVec2)>,
    rect: Option<[u32; 4]>,
    triangles: u64,
}

impl RenderFeature for OutlineFeature {
    fn name(&self) -> &str {
        "editor selection outline"
    }

    fn order(&self) -> i32 {
        // Under the debug lines (gizmo helpers) and the editor UI.
        900
    }

    /// The selection outlines in every scene view, not just the main one.
    fn overlay_in_extra_views(&self) -> bool {
        true
    }

    fn init(&mut self, gpu: &GpuShared) {
        let device = &gpu.device;
        let depth_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("outline scene depth"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Texture {
                    sample_type: wgpu::TextureSampleType::Depth,
                    view_dimension: wgpu::TextureViewDimension::D2,
                    multisampled: false,
                },
                count: None,
            }],
        });
        let composite_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("outline composite"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: false },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
            ],
        });
        let mask_shader = gpu.shader("outline mask", MASK_SHADER, true);
        let composite_shader = gpu.shader("outline composite", COMPOSITE_SHADER, false);
        let max = wgpu::BlendComponent {
            src_factor: wgpu::BlendFactor::One,
            dst_factor: wgpu::BlendFactor::One,
            operation: wgpu::BlendOperation::Max,
        };
        let instance_attributes = wgpu::vertex_attr_array![
            1 => Float32x4, 2 => Float32x4, 3 => Float32x4, 4 => Float32x4, 5 => Float32x4
        ];
        let mask = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("outline mask"),
            layout: Some(
                &device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                    label: Some("outline mask"),
                    bind_group_layouts: &[Some(&gpu.frame_layout), Some(&depth_layout)],
                    immediate_size: 0,
                }),
            ),
            vertex: wgpu::VertexState {
                module: &mask_shader,
                entry_point: Some("vs_mask"),
                compilation_options: Default::default(),
                buffers: &[
                    Some(wgpu::VertexBufferLayout {
                        array_stride: 12,
                        step_mode: wgpu::VertexStepMode::Vertex,
                        attributes: &wgpu::vertex_attr_array![0 => Float32x3],
                    }),
                    Some(wgpu::VertexBufferLayout {
                        array_stride: std::mem::size_of::<Instance>() as u64,
                        step_mode: wgpu::VertexStepMode::Instance,
                        attributes: &instance_attributes,
                    }),
                ],
            },
            primitive: wgpu::PrimitiveState {
                cull_mode: None,
                ..Default::default()
            },
            depth_stencil: None,
            multisample: Default::default(),
            fragment: Some(wgpu::FragmentState {
                module: &mask_shader,
                entry_point: Some("fs_mask"),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format: wgpu::TextureFormat::Rgba8Unorm,
                    blend: Some(wgpu::BlendState {
                        color: max,
                        alpha: max,
                    }),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            multiview_mask: None,
            cache: None,
        });
        let composite = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("outline composite"),
            layout: Some(
                &device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                    label: Some("outline composite"),
                    bind_group_layouts: &[Some(&composite_layout)],
                    immediate_size: 0,
                }),
            ),
            vertex: wgpu::VertexState {
                module: &composite_shader,
                entry_point: Some("vs_full"),
                compilation_options: Default::default(),
                buffers: &[],
            },
            primitive: Default::default(),
            depth_stencil: None,
            multisample: Default::default(),
            fragment: Some(wgpu::FragmentState {
                module: &composite_shader,
                entry_point: Some("fs_composite"),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format: gpu.formats.ldr,
                    blend: Some(wgpu::BlendState::ALPHA_BLENDING),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            multiview_mask: None,
            cache: None,
        });
        let style = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("outline style"),
            size: std::mem::size_of::<Style>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let grow = |label, usage| GrowBuffer::new(device, label, usage, 64 * 1024);
        self.gpu = Some(Pipelines {
            mask,
            composite,
            depth_layout,
            composite_layout,
            style,
            instances: grow("outline instances", wgpu::BufferUsages::VERTEX),
            posed_vertices: grow("outline posed vertices", wgpu::BufferUsages::VERTEX),
            posed_indices: grow("outline posed indices", wgpu::BufferUsages::INDEX),
        });
    }

    fn prepare(
        &mut self,
        world: &mut World,
        gpu: &GpuShared,
        view: &ViewData,
        _settings: &RenderSettings,
    ) {
        // Every view prepares and submits in turn (extra views first), so this view's draws and
        // buffers are the ones its overlay uses.
        self.draws.clear();
        self.rect = None;
        self.triangles = 0;
        let Some(pipes) = &mut self.gpu else {
            return;
        };
        let parts = selected_parts(world);
        let ppp = world
            .get_resource::<crate::egui_bridge::EguiInput>()
            .map_or(1.0, |f| f.pixels_per_point.max(0.5));
        let width = (WIDTH_POINTS * ppp).clamp(1.0, 4.0);
        self.rect = screen_rect(&parts, view, width + 2.0);
        if self.rect.is_none() {
            return;
        }
        let meshes = world.resource::<Assets<Mesh>>();
        let mut instances = Vec::with_capacity(parts.len());
        let (mut posed_vertices, mut posed_indices) = (Vec::<Vec3>::new(), Vec::<u32>::new());
        for mesh in self.meshes.values_mut() {
            mesh.used = false;
        }
        for part in &parts {
            let draw = match &part.source {
                Source::Mesh(handle, version) => {
                    let key = (handle.index(), *version);
                    let Some(mesh) = meshes.get(*handle) else {
                        continue;
                    };
                    let entry = self.meshes.entry(key).or_insert_with(|| upload(gpu, mesh));
                    entry.used = true;
                    self.triangles += entry.count as u64 / 3;
                    Draw::Cached(key.0, key.1)
                }
                Source::Posed(points, indices) => {
                    let draw = Draw::Posed {
                        first: posed_indices.len() as u32,
                        count: indices.len() as u32,
                        base: posed_vertices.len() as i32,
                    };
                    posed_vertices.extend_from_slice(points);
                    posed_indices.extend_from_slice(indices);
                    self.triangles += indices.len() as u64 / 3;
                    draw
                }
            };
            instances.push(Instance {
                model: camera_relative(&part.global, view.origin).to_cols_array_2d(),
                params: [if part.active { 1.0 } else { 0.0 }, 0.0, 0.0, 0.0],
            });
            self.draws.push(draw);
        }
        self.meshes.retain(|_, m| m.used);
        if !posed_vertices.is_empty() {
            pipes.posed_vertices.write(
                &gpu.device,
                &gpu.queue,
                bytemuck::cast_slice(&posed_vertices),
            );
            pipes.posed_indices.write(
                &gpu.device,
                &gpu.queue,
                bytemuck::cast_slice(&posed_indices),
            );
        }
        if !instances.is_empty() {
            pipes
                .instances
                .write(&gpu.device, &gpu.queue, bytemuck::cast_slice(&instances));
        }
        let style = Style {
            active: [ACTIVE[0], ACTIVE[1], ACTIVE[2], 1.0],
            other: [OTHER[0], OTHER[1], OTHER[2], 1.0],
            params: [width, HIDDEN_ALPHA, HIDDEN_FILL, 0.0],
        };
        gpu.queue
            .write_buffer(&pipes.style, 0, bytemuck::bytes_of(&style));
        // The mask covers the window.
        let size = view.window.max(UVec2::ONE);
        if self.mask.as_ref().is_none_or(|m| m.2 != size) {
            let texture = gpu.device.create_texture(&wgpu::TextureDescriptor {
                label: Some("outline mask"),
                size: wgpu::Extent3d {
                    width: size.x,
                    height: size.y,
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: wgpu::TextureFormat::Rgba8Unorm,
                usage: wgpu::TextureUsages::RENDER_ATTACHMENT
                    | wgpu::TextureUsages::TEXTURE_BINDING,
                view_formats: &[],
            });
            let view = texture.create_view(&Default::default());
            self.mask = Some((texture, view, size));
        }
    }

    fn needs_scene_copy(&self) -> bool {
        !self.draws.is_empty()
    }

    fn overlay(&mut self, encoder: &mut wgpu::CommandEncoder, ctx: &FrameContext) {
        let (Some(pipes), Some((_, mask, size)), Some(rect)) = (&self.gpu, &self.mask, self.rect)
        else {
            return;
        };
        if self.draws.is_empty() || ctx.view.window.max(UVec2::ONE) != *size {
            return;
        }
        let device = &ctx.gpu.device;
        let depth = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("outline scene depth"),
            layout: &pipes.depth_layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: wgpu::BindingResource::TextureView(ctx.scene_depth_copy),
            }],
        });
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("outline mask"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: mask,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                ..Default::default()
            });
            ctx.view.set_scene_viewport(&mut pass);
            pass.set_pipeline(&pipes.mask);
            pass.set_bind_group(0, ctx.frame_group, &[]);
            pass.set_bind_group(1, &depth, &[]);
            pass.set_vertex_buffer(1, pipes.instances.buffer.slice(..));
            for (i, draw) in self.draws.iter().enumerate() {
                let i = i as u32;
                match draw {
                    Draw::Cached(handle, version) => {
                        let Some(mesh) = self.meshes.get(&(*handle, *version)) else {
                            continue;
                        };
                        pass.set_vertex_buffer(0, mesh.vertices.slice(..));
                        pass.set_index_buffer(mesh.indices.slice(..), wgpu::IndexFormat::Uint32);
                        pass.draw_indexed(0..mesh.count, 0, i..i + 1);
                    }
                    Draw::Posed { first, count, base } => {
                        pass.set_vertex_buffer(0, pipes.posed_vertices.buffer.slice(..));
                        pass.set_index_buffer(
                            pipes.posed_indices.buffer.slice(..),
                            wgpu::IndexFormat::Uint32,
                        );
                        pass.draw_indexed(*first..first + count, *base, i..i + 1);
                    }
                }
            }
        }
        let group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("outline composite"),
            layout: &pipes.composite_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(mask),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: pipes.style.as_entire_binding(),
                },
            ],
        });
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("outline composite"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: ctx.ldr,
                depth_slice: None,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Load,
                    store: wgpu::StoreOp::Store,
                },
            })],
            ..Default::default()
        });
        let [x, y, w, h] = rect;
        let (x, y) = (x.min(size.x - 1), y.min(size.y - 1));
        pass.set_scissor_rect(x, y, w.min(size.x - x), h.min(size.y - y));
        pass.set_pipeline(&pipes.composite);
        pass.set_bind_group(0, &group, &[]);
        pass.draw(0..3, 0..1);
    }

    fn describe(&self) -> serde_json::Value {
        serde_json::json!({
            "parts": self.draws.len(),
            "triangles": self.triangles,
            "cached_meshes": self.meshes.len(),
            "rect": self.rect,
        })
    }
}

/// A mesh's positions and indices on the GPU.
fn upload(gpu: &GpuShared, mesh: &Mesh) -> GpuMesh {
    let positions: Vec<[f32; 3]> = mesh.vertices.iter().map(|v| v.position).collect();
    let vertices = gpu
        .device
        .create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("outline mesh vertices"),
            contents: bytemuck::cast_slice(&positions),
            usage: wgpu::BufferUsages::VERTEX,
        });
    let indices = gpu
        .device
        .create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("outline mesh indices"),
            contents: bytemuck::cast_slice(&mesh.indices),
            usage: wgpu::BufferUsages::INDEX,
        });
    GpuMesh {
        vertices,
        indices,
        count: mesh.indices.len() as u32,
        used: true,
    }
}

/// Mask pass (with lib_types + lib_frame; overlay matrices are unjittered).
const MASK_SHADER: &str = r#"
// Relative depth tolerance (reverse-Z: larger is closer). The scene depth was drawn jittered
// and by the module's own renderer, so the selection's own surface may sit a little behind it.
const DEPTH_TOLERANCE: f32 = 1.002;

@group(1) @binding(0) var scene_depth: texture_depth_2d;

struct MaskIn {
    @location(0) position: vec3<f32>,
    @location(1) m0: vec4<f32>,
    @location(2) m1: vec4<f32>,
    @location(3) m2: vec4<f32>,
    @location(4) m3: vec4<f32>,
    @location(5) params: vec4<f32>,
};

struct MaskOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) depth_zw: vec2<f32>,
    @location(1) @interpolate(flat) lead: f32,
};

@vertex
fn vs_mask(in: MaskIn) -> MaskOut {
    let rel = mat4x4<f32>(in.m0, in.m1, in.m2, in.m3) * vec4<f32>(in.position, 1.0);
    let clip = frame.view_proj * rel;
    var out: MaskOut;
    out.clip = clip;
    out.depth_zw = clip.zw;
    out.lead = in.params.x;
    return out;
}

@fragment
fn fs_mask(in: MaskOut) -> @location(0) vec4<f32> {
    let size = vec2<i32>(textureDimensions(scene_depth));
    let p = vec2<i32>((in.clip.xy - frame.window.xy) * frame.viewport.xy / frame.window.zw);
    let depth = in.depth_zw.x / in.depth_zw.y;
    // In front when no farther than the farthest scene depth around the pixel, so the
    // selection's own surface counts as seen at any angle.
    var farthest = 1.0;
    for (var y = -1; y <= 1; y++) {
        for (var x = -1; x <= 1; x++) {
            let q = clamp(p + vec2<i32>(x, y), vec2<i32>(0), size - 1);
            farthest = min(farthest, textureLoad(scene_depth, q, 0));
        }
    }
    let seen = select(0.0, 1.0, depth * DEPTH_TOLERANCE >= farthest);
    if (in.lead > 0.5) {
        return vec4<f32>(1.0, seen, 0.0, 0.0);
    }
    return vec4<f32>(0.0, 0.0, 1.0, seen);
}
"#;

/// Composite pass onto the final image.
const COMPOSITE_SHADER: &str = r#"
struct Style {
    lead: vec4<f32>,
    rest: vec4<f32>,
    // Line width (px), hidden line opacity, hidden fill opacity.
    params: vec4<f32>,
};

@group(0) @binding(0) var mask: texture_2d<f32>;
@group(0) @binding(1) var<uniform> style: Style;

@vertex
fn vs_full(@builtin(vertex_index) i: u32) -> @builtin(position) vec4<f32> {
    let uv = vec2<f32>(f32((i << 1u) & 2u), f32(i & 2u));
    return vec4<f32>(uv * 2.0 - 1.0, 0.0, 1.0);
}

// Opacity of a line `width` px wide at `d` px (pixel centers) from the shape: full for the
// first `width` pixels, fading over the next one.
fn edge(d: f32, width: f32) -> f32 {
    return clamp(width + 1.0 - d, 0.0, 1.0);
}

// Opacity for one selection group: `covered` and `seen` at this pixel, `d_covered` and
// `d_seen` the distances to the nearest pixels that are.
fn group_alpha(covered: bool, seen: bool, d_covered: f32, d_seen: f32) -> f32 {
    if (seen) {
        return 0.0;
    }
    let width = style.params.x;
    // The seen part's silhouette, including where something in front cuts it off.
    let line = edge(d_seen, width);
    // The hidden part: a faint line around it and a faint tint over it.
    var hidden = style.params.z;
    if (!covered) {
        hidden = edge(d_covered, width) * style.params.y;
    }
    return max(line, hidden);
}

@fragment
fn fs_composite(@builtin(position) pos: vec4<f32>) -> @location(0) vec4<f32> {
    let size = vec2<i32>(textureDimensions(mask));
    let p = vec2<i32>(pos.xy);
    let r = i32(ceil(style.params.x)) + 1;
    // Distances to the nearest pixel with each mask channel set.
    var d = vec4<f32>(1.0e4);
    for (var y = -r; y <= r; y++) {
        for (var x = -r; x <= r; x++) {
            let m = textureLoad(mask, clamp(p + vec2<i32>(x, y), vec2<i32>(0), size - 1), 0);
            let dist = length(vec2<f32>(f32(x), f32(y)));
            d = select(d, min(d, vec4<f32>(dist)), m > vec4<f32>(0.5));
        }
    }
    let here = d < vec4<f32>(0.5);
    let a = group_alpha(here.x, here.y, d.x, d.y);
    let b = group_alpha(here.z, here.w, d.z, d.w);
    if (a <= 0.0 && b <= 0.0) {
        discard;
    }
    if (a >= b) {
        return vec4<f32>(style.lead.rgb, a);
    }
    return vec4<f32>(style.rest.rgb, b);
}
"#;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shaders_validate() {
        for (name, source, frame) in [
            ("outline mask", MASK_SHADER, true),
            ("outline composite", COMPOSITE_SHADER, false),
        ] {
            let code = engine::render::compose_shader(source, frame);
            engine::render::validate_wgsl(name, &code).unwrap();
        }
    }

    #[test]
    fn instances_match_the_vertex_layout() {
        assert_eq!(std::mem::size_of::<Instance>(), 80);
        assert_eq!(std::mem::size_of::<Style>(), 48);
    }
}
