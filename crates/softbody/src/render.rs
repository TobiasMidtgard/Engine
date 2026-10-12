//! Drawing soft bodies: one render feature for cloth, soft volumes, rope tubes and hair.
//!
//! Every frame the CPU interpolates each body between its last two simulation steps
//! (`FixedTime::alpha`), relative to the camera, and builds:
//!
//! - **surfaces** (cloth, jelly) with area-weighted normals, the stretch along the pattern's
//!   u and v (from the rest shape) that drives fine wrinkles, and burn state;
//! - **tubes** swept around each rope chain (Catmull-Rom, parallel-transport frames, capped
//!   ends, twisted-ply pattern);
//! - **hair guides**: the simulated guide points, from which the vertex shader grows every
//!   drawn strand (see `shaders/strand_common.wgsl` for the format). Strands thin out with
//!   distance and get thicker to keep the same coverage.
//!
//! Hair and fur use their own strand drawing here until the shared strand renderer (owned by
//! Rendering and lighting) exists; [`Geometry::add_hair`] produces exactly the buffers it needs.

use std::ops::Range;

use bytemuck::{Pod, Zeroable};
use engine_core::prelude::*;
use engine_render::shadows::{MAX_CASCADES, SKY_OCCLUSION_VIEW};
use engine_render::{
    FeatureStats, FrameContext, GpuShared, GrowBuffer, RenderFeature, RenderPhase, RenderSettings,
    ViewData, wgpu,
};

use crate::body::{SoftBodySim, Surface, Tube};
use crate::settings::SoftMaterial;

/// Cloth, jelly and rope surfaces (needs `lib_frame`).
pub const SURFACE_SHADER: &str = include_str!("shaders/surface.wgsl");
/// Shadow casting for surfaces (`lib_types` only).
pub const SHADOW_SHADER: &str = include_str!("shaders/shadow.wgsl");
/// Strand expansion shared by both hair passes.
pub const STRAND_COMMON: &str = include_str!("shaders/strand_common.wgsl");
/// Hair color pass (needs `lib_frame`, after [`STRAND_COMMON`]).
pub const STRAND_SHADER: &str = include_str!("shaders/strands.wgsl");
/// Hair shadow pass (after [`STRAND_COMMON`]).
pub const STRAND_SHADOW_SHADER: &str = include_str!("shaders/strands_shadow.wgsl");

/// Sides around a rope tube.
const TUBE_SIDES: usize = 8;
/// Drawn rings per simulated rope segment.
const TUBE_SUBDIV: usize = 3;
/// Projected radius (px) at which hair is drawn with every strand.
const HAIR_FULL_DENSITY_PX: f32 = 120.0;

/// One surface vertex. Must match `VertexIn` in `surface.wgsl` and `shadow.wgsl`.
#[repr(C)]
#[derive(Debug, Clone, Copy, Default, PartialEq, Pod, Zeroable)]
pub struct SoftVertex {
    /// Camera-relative position.
    pub position: [f32; 3],
    pub normal: [f32; 3],
    pub uv: [f32; 2],
    /// Strain along u and v (negative = compressed).
    pub tension: [f32; 2],
    /// Char, glow.
    pub burn: [f32; 2],
    pub material: u32,
}

impl SoftVertex {
    const ATTRIBUTES: [wgpu::VertexAttribute; 6] = wgpu::vertex_attr_array![
        0 => Float32x3, 1 => Float32x3, 2 => Float32x2, 3 => Float32x2, 4 => Float32x2, 5 => Uint32
    ];

    fn layout() -> wgpu::VertexBufferLayout<'static> {
        wgpu::VertexBufferLayout {
            array_stride: std::mem::size_of::<Self>() as u64,
            step_mode: wgpu::VertexStepMode::Vertex,
            attributes: &Self::ATTRIBUTES,
        }
    }
}

/// Must match `struct SoftMaterial` in `surface.wgsl`.
#[repr(C)]
#[derive(Debug, Clone, Copy, Default, PartialEq, Pod, Zeroable)]
pub struct GpuSoftMaterial {
    pub color: [f32; 4],
    pub color2: [f32; 4],
    pub color3: [f32; 4],
    pub emissive: [f32; 4],
    pub params: [f32; 4],
    pub scale: [f32; 4],
    pub pattern: [u32; 4],
}

impl GpuSoftMaterial {
    pub fn new(m: &SoftMaterial, wetness: f32, uv_scale: Vec2) -> Self {
        let (id, param) = m.pattern.encode();
        let [r, g, b] = m.color;
        let [r2, g2, b2] = m.color2;
        let [r3, g3, b3] = m.color3;
        let [er, eg, eb] = m.emissive;
        let scale = if uv_scale.min_element() > 0.0 {
            uv_scale
        } else {
            Vec2::ONE
        };
        Self {
            color: [r, g, b, m.alpha.clamp(0.0, 1.0)],
            color2: [r2, g2, b2, m.wrinkles.max(0.0)],
            color3: [r3, g3, b3, wetness.clamp(0.0, 1.0)],
            emissive: [er, eg, eb, m.sheen.max(0.0)],
            params: [
                m.roughness.clamp(0.02, 1.0),
                m.metallic.clamp(0.0, 1.0),
                m.subsurface.clamp(0.0, 1.0),
                0.0,
            ],
            scale: [scale.x, scale.y, 0.0, 0.0],
            pattern: [id, param, 0, 0],
        }
    }
}

/// Must match `struct StrandGuide` in `strand_common.wgsl`.
#[repr(C)]
#[derive(Debug, Clone, Copy, Default, PartialEq, Pod, Zeroable)]
pub struct GpuStrandGuide {
    /// First point in the point buffer, and point count (root first).
    pub first: u32,
    pub count: u32,
    pub groom: u32,
    pub seed: u32,
    /// Root normal (world), w = guide length (m).
    pub normal: [f32; 4],
    /// Root tangent (world): the strand offsets are placed around the guide with it.
    pub tangent: [f32; 4],
}

/// Must match `struct Groom` in `strand_common.wgsl`.
#[repr(C)]
#[derive(Debug, Clone, Copy, Default, PartialEq, Pod, Zeroable)]
pub struct GpuGroom {
    /// Linear rgb albedo, root thickness (m).
    pub color: [f32; 4],
    /// Spread (m), clumping (0..1), curl radius (m), curl turns per meter.
    pub shape: [f32; 4],
    /// First guide, strands per guide, first strand (instance), drawn segments per strand.
    pub counts: [u32; 4],
    /// Length variation (0..1), brightness at the root (self-shadowing in the hair).
    pub extra: [f32; 4],
}

impl GpuGroom {
    /// Filler after the last groom: never matched by any instance.
    const UNUSED: Self = Self {
        color: [0.0; 4],
        shape: [0.0; 4],
        counts: [0, 1, u32::MAX, 1],
        extra: [0.0; 4],
    };
}

/// One hair groom to draw: strands are instances, each a strip of `vertices`.
#[derive(Debug, Clone, PartialEq)]
pub struct HairBatch {
    pub vertices: u32,
    pub instances: Range<u32>,
}

/// CPU-side geometry for one frame. Pure data, so it is tested without a GPU.
#[derive(Default)]
pub struct Geometry {
    pub vertices: Vec<SoftVertex>,
    pub indices: Vec<u32>,
    pub materials: Vec<GpuSoftMaterial>,
    /// Hair guide points, camera-relative (w unused).
    pub points: Vec<[f32; 4]>,
    pub guides: Vec<GpuStrandGuide>,
    pub grooms: Vec<GpuGroom>,
    pub strands: u32,
    particles: Vec<Vec3>,
    positions: Vec<Vec3>,
    normals: Vec<Vec3>,
    tension: Vec<Vec3>,
    ring: Vec<(Vec3, Vec3, Vec3, f32)>,
}

impl Geometry {
    pub fn clear(&mut self) {
        self.vertices.clear();
        self.indices.clear();
        self.materials.clear();
        self.points.clear();
        self.guides.clear();
        self.grooms.clear();
        self.strands = 0;
    }

    fn interpolate(&mut self, body: &SoftBodySim, base: Vec3, alpha: f32) {
        self.particles.clear();
        self.particles
            .extend((0..body.sim.len()).map(|i| base + body.sim.interpolated(i, alpha)));
    }

    /// Append a body's surface and rope tubes; `base` is the body origin relative to the
    /// camera. Returns its index range (empty for bodies with nothing to draw).
    pub fn add_body(&mut self, body: &SoftBodySim, base: Vec3, alpha: f32) -> Range<u32> {
        let start = self.indices.len() as u32;
        if body.surface.is_none() && body.tubes.is_empty() {
            return start..start;
        }
        let material = self.materials.len() as u32;
        let wetness = body.cloth.as_ref().map_or(0.0, |c| c.wetness);
        let uv_scale = body.surface.as_ref().map_or(Vec2::ONE, |s| s.uv_scale);
        self.materials
            .push(GpuSoftMaterial::new(&body.material, wetness, uv_scale));
        self.interpolate(body, base, alpha);
        if let Some(surface) = &body.surface {
            self.add_surface(body, surface, material);
        }
        for tube in &body.tubes {
            self.add_tube(tube, material);
        }
        start..self.indices.len() as u32
    }

    fn add_surface(&mut self, body: &SoftBodySim, s: &Surface, material: u32) {
        let n = s.vertex_count();
        self.positions.clear();
        match &s.embed {
            None => {
                if self.particles.len() < n {
                    return;
                }
                self.positions.extend_from_slice(&self.particles[..n]);
            }
            Some(e) => {
                let rot = body
                    .sim
                    .shapes
                    .first()
                    .map_or(Quat::IDENTITY, |m| m.rotation);
                for k in 0..n.min(e.particles.len()) {
                    let mut p = rot * e.offsets[k];
                    for (&i, &w) in e.particles[k].iter().zip(&e.weights[k]) {
                        if w != 0.0 {
                            p += self.particles[i as usize] * w;
                        }
                    }
                    self.positions.push(p);
                }
            }
        }
        let n = self.positions.len();
        let p = &self.positions;

        self.normals.clear();
        self.normals.resize(n, Vec3::ZERO);
        for t in &s.triangles {
            let [a, b, c] = t.map(|i| i as usize);
            if a >= n || b >= n || c >= n {
                continue;
            }
            let fn_ = (p[b] - p[a]).cross(p[c] - p[a]);
            self.normals[a] += fn_;
            self.normals[b] += fn_;
            self.normals[c] += fn_;
        }

        // Stretch along the pattern's u and v: F = [x1-x0, x2-x0] · Dm⁻¹. z counts triangles.
        self.tension.clear();
        self.tension.resize(n, Vec3::ZERO);
        if s.embed.is_none() && s.rest_inv.len() == s.triangles.len() {
            for (t, dm_inv) in s.triangles.iter().zip(&s.rest_inv) {
                let [a, b, c] = t.map(|i| i as usize);
                if a >= n || b >= n || c >= n {
                    continue;
                }
                let e1 = p[b] - p[a];
                let e2 = p[c] - p[a];
                let fu = e1 * dm_inv.x_axis.x + e2 * dm_inv.x_axis.y;
                let fv = e1 * dm_inv.y_axis.x + e2 * dm_inv.y_axis.y;
                let strain = Vec3::new(fu.length() - 1.0, fv.length() - 1.0, 1.0);
                self.tension[a] += strain;
                self.tension[b] += strain;
                self.tension[c] += strain;
            }
        }

        let (char, glow) = match &body.cloth {
            Some(c) if c.char.len() >= n && c.glow.len() >= n => (&c.char[..], &c.glow[..]),
            _ => (&[][..], &[][..]),
        };
        let first = self.vertices.len() as u32;
        for i in 0..n {
            let normal = self.normals[i].try_normalize().unwrap_or(Vec3::Y);
            let t = self.tension[i];
            let tension = if t.z > 0.0 {
                [t.x / t.z, t.y / t.z]
            } else {
                [0.0; 2]
            };
            let burn = if char.is_empty() {
                [0.0; 2]
            } else {
                [char[i], glow[i]]
            };
            self.vertices.push(SoftVertex {
                position: p[i].to_array(),
                normal: normal.to_array(),
                uv: s.uv.get(i).copied().unwrap_or_default().to_array(),
                tension,
                burn,
                material,
            });
        }
        for t in &s.triangles {
            if t.iter().all(|&i| (i as usize) < n) {
                self.indices.extend(t.iter().map(|&i| first + i));
            }
        }
    }

    fn add_tube(&mut self, tube: &Tube, material: u32) {
        let pts: Vec<Vec3> = tube
            .particles
            .iter()
            .filter_map(|&i| self.particles.get(i as usize).copied())
            .collect();
        if pts.len() < 2 {
            return;
        }
        // Centerline (Catmull-Rom through the particles) with tangents and arc length.
        self.ring.clear();
        let last = pts.len() - 1;
        let mut along = 0.0;
        let mut prev = pts[0];
        for i in 0..last {
            let p0 = pts[i.saturating_sub(1)];
            let p1 = pts[i];
            let p2 = pts[i + 1];
            let p3 = pts[(i + 2).min(last)];
            let steps = if i + 1 == last {
                TUBE_SUBDIV + 1
            } else {
                TUBE_SUBDIV
            };
            for k in 0..steps {
                let u = k as f32 / TUBE_SUBDIV as f32;
                let (c, tan) = catmull_rom(p0, p1, p2, p3, u);
                along += c.distance(prev);
                prev = c;
                self.ring.push((c, tan, Vec3::ZERO, along));
            }
        }
        // Parallel-transport frames, so the tube doesn't twist where the rope bends.
        let mut t_prev = self.ring[0].1.try_normalize().unwrap_or(Vec3::Y);
        let mut normal = t_prev.any_orthonormal_vector();
        for r in &mut self.ring {
            let t = r.1.try_normalize().unwrap_or(t_prev);
            normal = Quat::from_rotation_arc(t_prev, t) * normal;
            normal = (normal - t * normal.dot(t))
                .try_normalize()
                .unwrap_or(normal);
            r.1 = t;
            r.2 = normal;
            t_prev = t;
        }

        let radius = tube.radius;
        let first = self.vertices.len() as u32;
        let stride = (TUBE_SIDES + 1) as u32;
        for &(c, t, n, v) in &self.ring {
            let b = t.cross(n);
            for k in 0..=TUBE_SIDES {
                let a = k as f32 / TUBE_SIDES as f32 * std::f32::consts::TAU;
                let dir = n * a.cos() + b * a.sin();
                self.vertices.push(SoftVertex {
                    position: (c + dir * radius).to_array(),
                    normal: dir.to_array(),
                    uv: [k as f32 / TUBE_SIDES as f32, v],
                    tension: [0.0; 2],
                    burn: [0.0; 2],
                    material,
                });
            }
        }
        for r in 0..self.ring.len() as u32 - 1 {
            for k in 0..TUBE_SIDES as u32 {
                let a = first + r * stride + k;
                let b = a + stride;
                self.indices.extend([a, b, a + 1, a + 1, b, b + 1]);
            }
        }
        // End caps (cut ends stay closed).
        for (end, sign) in [(0, -1.0), (self.ring.len() - 1, 1.0)] {
            let (c, t, n, v) = self.ring[end];
            let b = t.cross(n);
            let normal = (t * sign).to_array();
            let center = self.vertices.len() as u32;
            let vertex = |p: Vec3, uv: [f32; 2]| SoftVertex {
                position: p.to_array(),
                normal,
                uv,
                tension: [0.0; 2],
                burn: [0.0; 2],
                material,
            };
            self.vertices.push(vertex(c, [0.5, v]));
            for k in 0..TUBE_SIDES {
                let a = k as f32 / TUBE_SIDES as f32 * std::f32::consts::TAU;
                let dir = n * a.cos() + b * a.sin();
                self.vertices
                    .push(vertex(c + dir * radius, [k as f32 / TUBE_SIDES as f32, v]));
            }
            for k in 0..TUBE_SIDES as u32 {
                let a = center + 1 + k;
                let b = center + 1 + (k + 1) % TUBE_SIDES as u32;
                self.indices.extend([center, a, b]);
            }
        }
    }

    /// Append a body's hair guides and groom. `density` (0..1] thins the strands out (they
    /// get thicker to keep the same coverage) and `detail` (0..1] lowers the drawn segments.
    pub fn add_hair(
        &mut self,
        body: &SoftBodySim,
        base: Vec3,
        alpha: f32,
        density: f32,
        detail: f32,
    ) -> Option<HairBatch> {
        let s = body.strands.as_ref()?;
        if s.guides.is_empty() {
            return None;
        }
        self.interpolate(body, base, alpha);
        let groom = self.grooms.len() as u32;
        let first_guide = self.guides.len() as u32;
        let mut max_points = 2;
        for (k, g) in s.guides.iter().enumerate() {
            let range = g.first as usize..(g.first + g.count) as usize;
            let Some(pts) = self.particles.get(range) else {
                continue;
            };
            let first = self.points.len() as u32;
            self.points.extend(pts.iter().map(|p| [p.x, p.y, p.z, 0.0]));
            let length: f32 = pts.windows(2).map(|w| w[0].distance(w[1])).sum();
            let rot = body
                .sim
                .frames
                .get(g.frame as usize)
                .map_or(Quat::IDENTITY, |f| f.pose(alpha).1);
            let n = rot * g.normal;
            let t = rot * g.tangent;
            self.guides.push(GpuStrandGuide {
                first,
                count: g.count,
                groom,
                seed: k as u32,
                normal: [n.x, n.y, n.z, length],
                tangent: [t.x, t.y, t.z, 0.0],
            });
            max_points = max_points.max(g.count);
        }
        let guides = self.guides.len() as u32 - first_guide;
        if guides == 0 {
            return None;
        }
        let full = s.style.strands_per_guide.max(1);
        let spg = ((full as f32 * density.clamp(0.0, 1.0)).ceil() as u32).clamp(1, full);
        let thickness = s.style.thickness * (full as f32 / spg as f32).sqrt();
        let segments =
            ((((max_points - 1) * 3) as f32 * detail.clamp(0.25, 1.0)) as u32).clamp(3, 32);
        let [r, g, b] = s.color;
        let first_strand = self.strands;
        let count = guides * spg;
        self.grooms.push(GpuGroom {
            color: [r, g, b, thickness],
            shape: [
                s.style.spread,
                s.style.clumping,
                s.style.curl,
                s.style.curl_frequency,
            ],
            counts: [first_guide, spg, first_strand, segments],
            // Long hair is dark deep inside; short fur lets light through to the skin.
            extra: [
                s.style.length_variation,
                0.3 + 0.45 * ((0.15 - s.style.length) / 0.12).clamp(0.0, 1.0),
                0.0,
                0.0,
            ],
        });
        self.strands += count;
        Some(HairBatch {
            vertices: 2 * (segments + 1),
            instances: first_strand..first_strand + count,
        })
    }
}

/// Catmull-Rom point and tangent between `p1` and `p2`.
fn catmull_rom(p0: Vec3, p1: Vec3, p2: Vec3, p3: Vec3, u: f32) -> (Vec3, Vec3) {
    let a = 2.0 * p1;
    let b = p2 - p0;
    let c = 2.0 * p0 - 5.0 * p1 + 4.0 * p2 - p3;
    let d = -p0 + 3.0 * p1 - 3.0 * p2 + p3;
    (
        0.5 * (a + b * u + c * u * u + d * u * u * u),
        0.5 * (b + 2.0 * c * u + 3.0 * d * u * u),
    )
}

/// Add `r` to a list of index ranges, merging it with the last one when they touch.
fn push_range(list: &mut Vec<Range<u32>>, r: Range<u32>) {
    if r.is_empty() {
        return;
    }
    match list.last_mut() {
        Some(last) if last.end == r.start => last.end = r.end,
        _ => list.push(r),
    }
}

struct HairDraw {
    batch: HairBatch,
    in_view: bool,
    /// Bit per shadow cascade.
    cascades: u8,
}

#[derive(Default)]
struct Draws {
    opaque: Vec<Range<u32>>,
    transparent: Vec<Range<u32>>,
    /// Per shadow cascade, plus the sky occlusion map last.
    shadow: [Vec<Range<u32>>; MAX_CASCADES + 1],
    hair: Vec<HairDraw>,
}

impl Draws {
    fn clear(&mut self) {
        self.opaque.clear();
        self.transparent.clear();
        self.shadow.iter_mut().for_each(Vec::clear);
        self.hair.clear();
    }
}

struct Gpu {
    opaque: wgpu::RenderPipeline,
    transparent: wgpu::RenderPipeline,
    shadow: wgpu::RenderPipeline,
    strands: wgpu::RenderPipeline,
    strands_shadow: wgpu::RenderPipeline,
    material_layout: wgpu::BindGroupLayout,
    strand_layout: wgpu::BindGroupLayout,
    vertices: GrowBuffer,
    indices: GrowBuffer,
    materials: GrowBuffer,
    points: GrowBuffer,
    guides: GrowBuffer,
    grooms: GrowBuffer,
    material_group: wgpu::BindGroup,
    strand_group: wgpu::BindGroup,
}

fn storage_layout(device: &wgpu::Device, label: &str, count: u32) -> wgpu::BindGroupLayout {
    let entries: Vec<_> = (0..count)
        .map(|binding| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: wgpu::ShaderStages::VERTEX | wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Buffer {
                ty: wgpu::BufferBindingType::Storage { read_only: true },
                has_dynamic_offset: false,
                min_binding_size: None,
            },
            count: None,
        })
        .collect();
    device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some(label),
        entries: &entries,
    })
}

fn storage_group(
    device: &wgpu::Device,
    label: &str,
    layout: &wgpu::BindGroupLayout,
    buffers: &[&GrowBuffer],
) -> wgpu::BindGroup {
    let entries: Vec<_> = buffers
        .iter()
        .enumerate()
        .map(|(i, b)| wgpu::BindGroupEntry {
            binding: i as u32,
            resource: b.buffer.as_entire_binding(),
        })
        .collect();
    device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some(label),
        layout,
        entries: &entries,
    })
}

struct PipelineSpec<'a> {
    label: &'a str,
    module: &'a wgpu::ShaderModule,
    vs: &'a str,
    fs: Option<&'a str>,
    layouts: &'a [Option<&'a wgpu::BindGroupLayout>],
    vertex: bool,
    strip: bool,
    blend: bool,
    shadow: bool,
}

fn pipeline(gpu: &GpuShared, s: PipelineSpec) -> wgpu::RenderPipeline {
    let device = &gpu.device;
    let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: Some(s.label),
        bind_group_layouts: s.layouts,
        immediate_size: 0,
    });
    let buffers = [Some(SoftVertex::layout())];
    let targets = [Some(wgpu::ColorTargetState {
        format: gpu.formats.hdr,
        blend: s.blend.then_some(wgpu::BlendState::ALPHA_BLENDING),
        write_mask: wgpu::ColorWrites::ALL,
    })];
    device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some(s.label),
        layout: Some(&layout),
        vertex: wgpu::VertexState {
            module: s.module,
            entry_point: Some(s.vs),
            compilation_options: Default::default(),
            buffers: if s.vertex { &buffers } else { &[] },
        },
        primitive: wgpu::PrimitiveState {
            topology: if s.strip {
                wgpu::PrimitiveTopology::TriangleStrip
            } else {
                wgpu::PrimitiveTopology::TriangleList
            },
            cull_mode: None,
            unclipped_depth: s.shadow && gpu.features.contains(wgpu::Features::DEPTH_CLIP_CONTROL),
            ..Default::default()
        },
        depth_stencil: Some(if s.shadow {
            gpu.shadow_depth_state()
        } else {
            gpu.depth_state(!s.blend)
        }),
        multisample: Default::default(),
        fragment: s.fs.map(|fs| wgpu::FragmentState {
            module: s.module,
            entry_point: Some(fs),
            compilation_options: Default::default(),
            targets: &targets,
        }),
        multiview_mask: None,
        cache: None,
    })
}

/// Draws every [`SoftBodySim`]: cloth, jelly, rope tubes and hair, with shadows.
#[derive(Default)]
pub struct SoftBodyFeature {
    gpu: Option<Gpu>,
    query: Option<QueryState<&'static SoftBodySim>>,
    geometry: Geometry,
    draws: Draws,
    stats: FeatureStats,
    bodies: u32,
}

impl RenderFeature for SoftBodyFeature {
    fn name(&self) -> &str {
        "softbody"
    }

    fn init(&mut self, gpu: &GpuShared) {
        let device = &gpu.device;
        let material_layout = storage_layout(device, "softbody materials", 1);
        let strand_layout = storage_layout(device, "softbody strands", 3);
        let surface = gpu.shader("softbody surface", SURFACE_SHADER, true);
        let shadow = gpu.shader("softbody shadow", SHADOW_SHADER, false);
        let strands = gpu.shader(
            "softbody strands",
            &format!("{STRAND_COMMON}\n{STRAND_SHADER}"),
            true,
        );
        let strands_shadow = gpu.shader(
            "softbody strands shadow",
            &format!("{STRAND_COMMON}\n{STRAND_SHADOW_SHADER}"),
            false,
        );
        let scene = [Some(&gpu.frame_layout), Some(&material_layout)];
        let surface_spec = |label, blend| PipelineSpec {
            label,
            module: &surface,
            vs: "vs_main",
            fs: Some("fs_main"),
            layouts: &scene,
            vertex: true,
            strip: false,
            blend,
            shadow: false,
        };
        let opaque = pipeline(gpu, surface_spec("softbody opaque", false));
        let transparent = pipeline(gpu, surface_spec("softbody transparent", true));
        let shadow = pipeline(
            gpu,
            PipelineSpec {
                label: "softbody shadow",
                module: &shadow,
                vs: "vs_shadow",
                fs: None,
                layouts: &[Some(&gpu.shadow_view_layout)],
                vertex: true,
                strip: false,
                blend: false,
                shadow: true,
            },
        );
        let strands = pipeline(
            gpu,
            PipelineSpec {
                label: "softbody strands",
                module: &strands,
                vs: "vs_strand",
                fs: Some("fs_strand"),
                layouts: &[Some(&gpu.frame_layout), Some(&strand_layout)],
                vertex: false,
                strip: true,
                blend: false,
                shadow: false,
            },
        );
        let strands_shadow = pipeline(
            gpu,
            PipelineSpec {
                label: "softbody strands shadow",
                module: &strands_shadow,
                vs: "vs_strand_shadow",
                fs: None,
                layouts: &[Some(&gpu.shadow_view_layout), Some(&strand_layout)],
                vertex: false,
                strip: true,
                blend: false,
                shadow: true,
            },
        );
        let buffer = |label, usage, size| GrowBuffer::new(device, label, usage, size);
        let storage = wgpu::BufferUsages::STORAGE;
        let vertices = buffer("softbody vertices", wgpu::BufferUsages::VERTEX, 1 << 20);
        let indices = buffer("softbody indices", wgpu::BufferUsages::INDEX, 1 << 20);
        let materials = buffer("softbody materials", storage, 64 * 112);
        let points = buffer("softbody strand points", storage, 1 << 16);
        let guides = buffer("softbody strand guides", storage, 1 << 14);
        let grooms = buffer("softbody grooms", storage, 1 << 10);
        let material_group = storage_group(
            device,
            "softbody materials",
            &material_layout,
            &[&materials],
        );
        let strand_group = storage_group(
            device,
            "softbody strands",
            &strand_layout,
            &[&points, &guides, &grooms],
        );
        self.gpu = Some(Gpu {
            opaque,
            transparent,
            shadow,
            strands,
            strands_shadow,
            material_layout,
            strand_layout,
            vertices,
            indices,
            materials,
            points,
            guides,
            grooms,
            material_group,
            strand_group,
        });
    }

    fn prepare(
        &mut self,
        world: &mut World,
        gpu: &GpuShared,
        view: &ViewData,
        _settings: &RenderSettings,
    ) {
        let Some(g) = self.gpu.as_mut() else {
            return;
        };
        self.geometry.clear();
        self.draws.clear();
        self.stats = FeatureStats::default();
        let alpha = world.get_resource::<FixedTime>().map_or(1.0, |f| f.alpha());
        let query = self
            .query
            .get_or_insert_with(|| world.query::<&SoftBodySim>());

        // Which bodies the camera or a shadow map can see.
        struct Seen<'a> {
            body: &'a SoftBodySim,
            base: Vec3,
            dist: f32,
            px: f32,
            in_view: bool,
            /// Bit per cascade, bit MAX_CASCADES for the sky occlusion map.
            mask: u8,
        }
        let mut seen: Vec<Seen> = Vec::new();
        for body in query.iter(world) {
            if body.sim.is_empty() {
                continue;
            }
            let center = view.relative(body.center);
            let radius = body.radius + 0.05;
            let dist = center.length();
            let px = view.projected_radius_px(radius, dist);
            let in_view = view.frustum.intersects_sphere(center, radius) && px > 0.3;
            let mut mask = 0u8;
            for c in 0..view.cascades.count as usize {
                if view.cascades.sphere_in_cascade(c, center, radius) {
                    mask |= 1 << c;
                }
            }
            if body.strands.is_none()
                && view.sky_occlusion.enabled
                && view.sky_occlusion.contains_sphere(center, radius)
            {
                mask |= 1 << SKY_OCCLUSION_VIEW;
            }
            if !in_view && mask == 0 {
                self.stats.culled += 1;
                continue;
            }
            seen.push(Seen {
                body,
                base: (body.sim.origin - view.origin).as_vec3(),
                dist,
                px,
                in_view,
                mask,
            });
        }
        // Opaque first, then see-through bodies back to front.
        let transparent = |s: &Seen| s.body.material.alpha < 0.999;
        seen.sort_by(|a, b| {
            transparent(a).cmp(&transparent(b)).then(if transparent(a) {
                b.dist.total_cmp(&a.dist)
            } else {
                std::cmp::Ordering::Equal
            })
        });

        let geo = &mut self.geometry;
        for s in &seen {
            let range = geo.add_body(s.body, s.base, alpha);
            if !range.is_empty() {
                if s.in_view {
                    let list = if transparent(s) {
                        &mut self.draws.transparent
                    } else {
                        &mut self.draws.opaque
                    };
                    push_range(list, range.clone());
                    self.stats.triangles += (range.len() / 3) as u64;
                }
                for c in 0..=MAX_CASCADES {
                    if s.mask & (1 << c) != 0 {
                        push_range(&mut self.draws.shadow[c], range.clone());
                    }
                }
            }
            if s.body.strands.is_some() {
                let density = (s.px / HAIR_FULL_DENSITY_PX).clamp(0.06, 1.0);
                let detail = (s.px / (HAIR_FULL_DENSITY_PX * 0.5)).clamp(0.25, 1.0);
                if let Some(batch) = geo.add_hair(s.body, s.base, alpha, density, detail) {
                    if s.in_view {
                        self.stats.instances += batch.instances.len() as u32;
                        self.stats.triangles +=
                            batch.instances.len() as u64 * (batch.vertices as u64 - 2);
                    }
                    self.draws.hair.push(HairDraw {
                        batch,
                        in_view: s.in_view,
                        cascades: s.mask & !(1 << SKY_OCCLUSION_VIEW),
                    });
                }
            }
        }
        self.bodies = seen.len() as u32;
        self.stats.draw_calls = (self.draws.opaque.len()
            + self.draws.transparent.len()
            + self.draws.hair.iter().filter(|h| h.in_view).count())
            as u32;

        // Upload.
        let (device, queue) = (&gpu.device, &gpu.queue);
        g.vertices
            .write(device, queue, bytemuck::cast_slice(&geo.vertices));
        g.indices
            .write(device, queue, bytemuck::cast_slice(&geo.indices));
        if g.materials
            .write(device, queue, bytemuck::cast_slice(&geo.materials))
        {
            g.material_group = storage_group(
                device,
                "softbody materials",
                &g.material_layout,
                &[&g.materials],
            );
        }
        if !geo.grooms.is_empty() {
            // Fill the whole buffer: the shader finds a strand's groom by scanning all slots.
            let size = std::mem::size_of::<GpuGroom>();
            let slots = (geo.grooms.len() * size)
                .max(g.grooms.capacity() as usize)
                .max(256)
                .next_power_of_two()
                / size;
            geo.grooms.resize(slots, GpuGroom::UNUSED);
            let mut grown = g
                .points
                .write(device, queue, bytemuck::cast_slice(&geo.points));
            grown |= g
                .guides
                .write(device, queue, bytemuck::cast_slice(&geo.guides));
            grown |= g
                .grooms
                .write(device, queue, bytemuck::cast_slice(&geo.grooms));
            if grown {
                g.strand_group = storage_group(
                    device,
                    "softbody strands",
                    &g.strand_layout,
                    &[&g.points, &g.guides, &g.grooms],
                );
            }
        }
    }

    fn draw_shadow(&self, pass: &mut wgpu::RenderPass<'_>, cascade: usize, _ctx: &FrameContext) {
        let Some(g) = &self.gpu else {
            return;
        };
        let ranges = &self.draws.shadow[cascade.min(MAX_CASCADES)];
        if !ranges.is_empty() {
            pass.set_pipeline(&g.shadow);
            pass.set_vertex_buffer(0, g.vertices.buffer.slice(..));
            pass.set_index_buffer(g.indices.buffer.slice(..), wgpu::IndexFormat::Uint32);
            for r in ranges {
                pass.draw_indexed(r.clone(), 0, 0..1);
            }
        }
        if cascade < MAX_CASCADES {
            let mut bound = false;
            for h in self
                .draws
                .hair
                .iter()
                .filter(|h| h.cascades & (1 << cascade) != 0)
            {
                if !bound {
                    pass.set_pipeline(&g.strands_shadow);
                    pass.set_bind_group(1, &g.strand_group, &[]);
                    bound = true;
                }
                pass.draw(0..h.batch.vertices, h.batch.instances.clone());
            }
        }
    }

    fn draw(&self, phase: RenderPhase, pass: &mut wgpu::RenderPass<'_>, _ctx: &FrameContext) {
        let Some(g) = &self.gpu else {
            return;
        };
        let (pipeline, ranges) = match phase {
            RenderPhase::Opaque => (&g.opaque, &self.draws.opaque),
            RenderPhase::Transparent => (&g.transparent, &self.draws.transparent),
            RenderPhase::Sky => return,
        };
        if !ranges.is_empty() {
            pass.set_pipeline(pipeline);
            pass.set_bind_group(1, &g.material_group, &[]);
            pass.set_vertex_buffer(0, g.vertices.buffer.slice(..));
            pass.set_index_buffer(g.indices.buffer.slice(..), wgpu::IndexFormat::Uint32);
            for r in ranges {
                pass.draw_indexed(r.clone(), 0, 0..1);
            }
        }
        if phase == RenderPhase::Opaque {
            let mut bound = false;
            for h in self.draws.hair.iter().filter(|h| h.in_view) {
                if !bound {
                    pass.set_pipeline(&g.strands);
                    pass.set_bind_group(1, &g.strand_group, &[]);
                    bound = true;
                }
                pass.draw(0..h.batch.vertices, h.batch.instances.clone());
            }
        }
    }

    fn stats(&self) -> FeatureStats {
        self.stats
    }

    fn describe(&self) -> serde_json::Value {
        serde_json::json!({
            "bodies": self.bodies,
            "vertices": self.geometry.vertices.len(),
            "strands": self.geometry.strands,
            "guides": self.geometry.guides.len(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cloth::{self, Cloth};
    use crate::settings::SoftMaterial;
    use engine_render::{compose_shader, validate_wgsl};

    #[test]
    fn shaders_validate() {
        let cases = [
            ("surface", SURFACE_SHADER.to_string(), true),
            ("shadow", SHADOW_SHADER.to_string(), false),
            ("strands", format!("{STRAND_COMMON}\n{STRAND_SHADER}"), true),
            (
                "strands shadow",
                format!("{STRAND_COMMON}\n{STRAND_SHADOW_SHADER}"),
                false,
            ),
        ];
        for (label, src, frame) in cases {
            validate_wgsl(label, &compose_shader(&src, frame)).unwrap();
        }
    }

    #[test]
    fn gpu_structs_match_wgsl_sizes() {
        assert_eq!(std::mem::size_of::<SoftVertex>(), 52);
        assert_eq!(std::mem::size_of::<GpuSoftMaterial>(), 112);
        assert_eq!(std::mem::size_of::<GpuStrandGuide>(), 48);
        assert_eq!(std::mem::size_of::<GpuGroom>(), 64);
    }

    fn flat_cloth() -> SoftBodySim {
        let spec = Cloth {
            size: [2.0, 1.0],
            resolution: [9, 5],
            ..Cloth::flag([2.0, 1.0], SoftMaterial::hex(0xc03030))
        };
        cloth::build(
            &spec,
            Entity::PLACEHOLDER,
            &GlobalTransform::default(),
            None,
        )
        .unwrap()
    }

    #[test]
    fn cloth_vertices_follow_particles_and_report_compression() {
        let mut body = flat_cloth();
        let mut geo = Geometry::default();
        let range = geo.add_body(&body, Vec3::ZERO, 1.0);
        assert_eq!(geo.vertices.len(), body.sim.len());
        assert_eq!(
            range.len(),
            body.surface.as_ref().unwrap().triangles.len() * 3
        );
        for v in &geo.vertices {
            assert!(Vec3::from(v.normal).is_normalized());
            assert!(v.tension[0].abs() < 1e-3 && v.tension[1].abs() < 1e-3);
        }
        // Squeeze the cloth to 70% along its width (u): strain along u turns negative.
        let x0 = body.sim.pos.iter().map(|p| p.x).fold(f32::MAX, f32::min);
        for i in 0..body.sim.len() {
            body.sim.pos[i].x = x0 + (body.sim.pos[i].x - x0) * 0.7;
            body.sim.step_start[i] = body.sim.pos[i];
        }
        geo.clear();
        geo.add_body(&body, Vec3::ZERO, 1.0);
        for v in &geo.vertices {
            assert!((v.tension[0] + 0.3).abs() < 0.02, "{:?}", v.tension);
            assert!(v.tension[1].abs() < 0.02, "{:?}", v.tension);
        }
    }

    #[test]
    fn tubes_are_closed_rings_along_the_rope() {
        let mut body = flat_cloth();
        body.surface = None;
        body.tubes = vec![Tube {
            particles: (0..9).collect(),
            radius: 0.05,
        }];
        let mut geo = Geometry::default();
        let range = geo.add_body(&body, Vec3::new(10.0, 0.0, 0.0), 1.0);
        let rings = 8 * TUBE_SUBDIV + 1;
        assert_eq!(
            geo.vertices.len(),
            rings * (TUBE_SIDES + 1) + 2 * (TUBE_SIDES + 1)
        );
        assert_eq!(
            range.len(),
            (rings - 1) * TUBE_SIDES * 6 + 2 * TUBE_SIDES * 3
        );
        // Every ring vertex sits one radius from the rope, which runs through the particles.
        let first = geo.vertices[0].position;
        let p0 = Vec3::new(10.0, 0.0, 0.0) + body.sim.pos[0];
        assert!((Vec3::from(first).distance(p0) - 0.05).abs() < 1e-4);
        // Texture v runs in meters along the rope.
        let v_end = geo.vertices[(rings - 1) * (TUBE_SIDES + 1)].uv[1];
        let length = body.sim.pos[0].distance(body.sim.pos[8]);
        assert!((v_end - length).abs() < 0.01, "{v_end} vs {length}");
    }

    #[test]
    fn ranges_merge_when_adjacent() {
        let mut list = Vec::new();
        push_range(&mut list, 0..6);
        push_range(&mut list, 6..12);
        push_range(&mut list, 15..18);
        push_range(&mut list, 18..18);
        assert_eq!(list, vec![0..12, 15..18]);
    }
}
