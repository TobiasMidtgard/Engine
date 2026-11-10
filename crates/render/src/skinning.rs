//! GPU skinning for the standard meshes ([`GpuSkin`]): a module gives an entity's mesh skin
//! weights and, each frame, its bones' pose; a compute pass skins the mesh's vertices where the
//! mesh pool keeps them, so the view, the shadow maps and everything else that draws the mesh
//! draws it posed, and only the bones go to the GPU each frame.
//!
//! After Unreal's GPU skin cache (a compute pass writes the skinned vertices once a frame for
//! every pass to read) and Unity's compute skinning (`MeshDeformation.GPUBatched`), with dual
//! quaternion blending (Kavan, Collins, Žára and O'Sullivan, "Skinning with Dual Quaternions",
//! I3D 2007): no candy-wrapper twists or collapsing joints. Linear blending is the option.
//!
//! How: the first frame a skinned mesh is in the pool, its rest vertices are copied (on the GPU)
//! to a buffer of their own; each frame its pose changed, the bones are written and one dispatch
//! reads the rest vertices, weights and bones and writes the posed positions and normals over
//! the pool's copy. The mesh's bounds follow the pose (each bone's box of the vertices it moves
//! most, posed), so culling stays right. A pose that didn't change costs nothing.
//!
//! Motion vectors follow the skin (characters week R1): the dispatch that poses a vertex first
//! keeps where it was posed last frame in [`Skins::motion`] (one `vec4` per vertex, each mesh
//! at its own offset), and the depth prepass moves it back there (`vs_prepass`, mesh.wgsl), so
//! temporal anti-aliasing and motion blur follow swinging legs and tails instead of smearing
//! them. A pose that stops changing is skinned once more, so last frame's place catches up.
//!
//! Limits: each skinned entity needs a mesh of its own (the pool's copy is posed for it);
//! skinned meshes get no levels of detail, clusters or impostors.

use std::sync::Arc;

use bytemuck::{Pod, Zeroable};
use engine_core::prelude::*;

use crate::components::{NoMeshLod, NoVirtualGeometry};
use crate::features::GpuShared;
use crate::mesh::Mesh;
use crate::mesh_pool::{MeshPool, PoolMesh};

/// Skin a mesh on the GPU. Put it on an entity with a [`crate::MeshRenderer`] whose mesh is the
/// entity's own (in its bind pose) and set [`bones`](Self::bones) every frame it moves.
#[derive(Component, Debug, Clone)]
#[require(NoVirtualGeometry, NoMeshLod)]
pub struct GpuSkin {
    /// Each vertex's bones and weights (as many as the mesh has vertices).
    pub weights: Arc<SkinWeights>,
    /// Each bone's move from the bind pose to now, in the mesh's space.
    pub bones: Vec<SkinBone>,
    pub blend: SkinBlend,
}

impl GpuSkin {
    pub fn new(weights: Arc<SkinWeights>) -> Self {
        Self {
            weights,
            bones: Vec::new(),
            blend: SkinBlend::default(),
        }
    }
}

/// Bones and weights per vertex. Weights should add up to 1; a vertex with none stays put.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct SkinWeights {
    pub joints: Vec<[u16; 4]>,
    pub weights: Vec<[f32; 4]>,
}

/// How a vertex's bones are blended.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SkinBlend {
    /// Dual quaternions: keeps volume at twisting and bending joints.
    #[default]
    DualQuaternion,
    /// Linear blending of the bones' matrices (cheaper look-alike of most engines' default).
    Linear,
}

/// A bone's rigid move as a unit dual quaternion: `real` the rotation (x, y, z, w), `dual` half
/// the translation times it.
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Pod, Zeroable)]
pub struct SkinBone {
    pub real: [f32; 4],
    pub dual: [f32; 4],
}

impl Default for SkinBone {
    fn default() -> Self {
        Self::IDENTITY
    }
}

impl SkinBone {
    pub const IDENTITY: Self = Self {
        real: [0.0, 0.0, 0.0, 1.0],
        dual: [0.0; 4],
    };

    /// Rotate by `rotation`, then move by `translation`.
    pub fn new(rotation: Quat, translation: Vec3) -> Self {
        let r = rotation.normalize();
        let d = Quat::from_xyzw(translation.x, translation.y, translation.z, 0.0) * r * 0.5;
        Self {
            real: r.to_array(),
            dual: d.to_array(),
        }
    }

    /// The move from a bone's bind pose to its pose now (both in the mesh's space).
    pub fn from_poses(bind: (Quat, Vec3), now: (Quat, Vec3)) -> Self {
        let inv = bind.0.inverse();
        // now * bind⁻¹: p -> now.r * (inv * (p - bind.t)) + now.t
        let rotation = now.0 * inv;
        Self::new(rotation, now.1 - rotation * bind.1)
    }

    pub fn rotation(&self) -> Quat {
        Quat::from_array(self.real)
    }

    pub fn translation(&self) -> Vec3 {
        let (r, d) = (Vec4::from(self.real), Vec4::from(self.dual));
        2.0 * (r.w * d.truncate() - d.w * r.truncate() + r.truncate().cross(d.truncate()))
    }

    /// Where it moves `p` (the shader's blend of a single bone).
    pub fn apply(&self, p: Vec3) -> Vec3 {
        self.rotation() * p + self.translation()
    }
}

const WORKGROUP: u32 = 64;

fn entry(binding: u32, buffer: &wgpu::Buffer) -> wgpu::BindGroupEntry<'_> {
    wgpu::BindGroupEntry {
        binding,
        resource: buffer.as_entire_binding(),
    }
}

/// One skinned mesh's GPU state (by mesh index).
pub(crate) struct Skin {
    /// The weights it was built with (a new `Arc` rebuilds it).
    weights: Arc<SkinWeights>,
    joints_buffer: wgpu::Buffer,
    weights_buffer: wgpu::Buffer,
    bones: wgpu::Buffer,
    bone_count: usize,
    params: wgpu::Buffer,
    /// The rest vertices, copied from the pool on its first frame there.
    rest: wgpu::Buffer,
    /// Where the pool held the rest vertices it copied (a new upload copies again).
    copied: Option<(PoolMesh, u64)>,
    /// Its bind group, with the pool page and that page's vertex buffer generation
    /// (`MeshPool::generation`) it was made over.
    group: Option<((u16, u32), wgpu::BindGroup)>,
    /// Each bone's bind-space box of the vertices it moves most (none: no vertex).
    boxes: Vec<Option<(Vec3, Vec3)>>,
    /// The pose last skinned, to skip one that didn't change.
    skinned: Vec<SkinBone>,
    blend: SkinBlend,
    /// Skin this frame.
    due: bool,
    /// Skinned last frame for a new pose: skin once more after it stops, so the motion
    /// buffer's last-frame positions catch up with it.
    moving: bool,
    /// Where its vertices start in [`Skins::motion`].
    pub motion_offset: u32,
    /// Its motion entries hold nothing yet: the next dispatch sets last frame's place to now.
    fresh: bool,
    pub last_seen: u64,
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct Params {
    base_vertex: u32,
    count: u32,
    linear: u32,
    bones: u32,
    /// Where the mesh's entries start in the motion buffer.
    motion: u32,
    /// 1: no last frame yet (its place last frame is its place now).
    fresh: u32,
    pad: [u32; 2],
}

/// Fewest motion entries the buffer holds (`vec4` each).
const MOTION_MIN: u32 = 1024;

/// Every skinned mesh's GPU state and the pass that skins them.
#[derive(Default)]
pub(crate) struct Skins {
    pipeline: Option<(wgpu::ComputePipeline, wgpu::BindGroupLayout)>,
    pub skins: bevy_platform::collections::HashMap<u32, Skin>,
    /// Dispatches this frame.
    pub skinned: u32,
    /// Vertices they posed.
    pub skinned_vertices: u64,
    /// Every skinned vertex's posed position last frame (model space, `w` 1), each mesh's at
    /// its `motion_offset`: the prepass reads it (mesh materials group, binding 2).
    pub motion: Option<wgpu::Buffer>,
    /// Entries `motion` holds.
    motion_capacity: u32,
    /// The skins changed: offsets to hand out again.
    repack: bool,
    /// `motion` is a new buffer (the groups that bind it are made again).
    motion_replaced: bool,
}

impl Skins {
    /// Before the gather: for each skinned mesh `index` (canonical) drawn this frame with
    /// `skin`, write the bones of a changed pose and return its posed bounds (center, half
    /// extents) for the mesh's culling. `mesh` is its asset (bind pose).
    pub fn prepare(
        &mut self,
        gpu: &GpuShared,
        index: u32,
        mesh: &Mesh,
        skin: &GpuSkin,
        frame: u64,
    ) -> Option<(Vec3, Vec3)> {
        let vertices = mesh.vertices.len();
        if skin.weights.joints.len() != vertices || skin.weights.weights.len() != vertices {
            static WARNED: std::sync::Once = std::sync::Once::new();
            WARNED.call_once(|| {
                tracing::warn!(
                    "GpuSkin weights for {} vertices on a mesh of {vertices}; not skinned",
                    skin.weights.joints.len()
                )
            });
            return None;
        }
        let rebuild = self.skins.get(&index).is_none_or(|s| {
            !Arc::ptr_eq(&s.weights, &skin.weights) || s.bone_count < skin.bones.len().max(1)
        });
        if rebuild {
            self.skins.insert(index, Skin::new(gpu, mesh, skin));
            self.repack = true;
        }
        let state = self.skins.get_mut(&index)?;
        state.last_seen = frame;
        let changed = state.skinned != skin.bones || state.blend != skin.blend;
        if !changed && state.moving {
            // Stopped: skin the same pose once more, so last frame's place is this one.
            state.moving = false;
            state.due = true;
        }
        if changed {
            state.moving = true;
            state.skinned.clone_from(&skin.bones);
            state.blend = skin.blend;
            let mut bones = skin.bones.clone();
            bones.resize(state.bone_count, SkinBone::IDENTITY);
            gpu.queue
                .write_buffer(&state.bones, 0, bytemuck::cast_slice(&bones));
            state.due = true;
        }
        Some(state.bounds())
    }

    /// Skin the meshes whose pose changed (or whose rest copy is new), in the pool's buffers.
    pub fn encode(
        &mut self,
        gpu: &GpuShared,
        encoder: &mut wgpu::CommandEncoder,
        pool: &MeshPool,
        resident: impl Fn(u32) -> Option<(PoolMesh, u64)>,
    ) {
        self.skinned = 0;
        self.skinned_vertices = 0;
        if self.skins.is_empty() {
            return;
        }
        let device = &gpu.device;
        let (pipeline, layout) = self.pipeline.get_or_insert_with(|| {
            let module = gpu.shader("gpu skinning", SHADER, false);
            let storage = |binding, read_only| wgpu::BindGroupLayoutEntry {
                binding,
                visibility: wgpu::ShaderStages::COMPUTE,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Storage { read_only },
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            };
            let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: Some("gpu skinning"),
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
                    storage(1, true),
                    storage(2, true),
                    storage(3, true),
                    storage(4, true),
                    storage(5, false),
                    storage(6, false),
                ],
            });
            let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some("gpu skinning"),
                bind_group_layouts: &[Some(&layout)],
                immediate_size: 0,
            });
            let pipeline = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
                label: Some("gpu skinning"),
                layout: Some(&pipeline_layout),
                module: &module,
                entry_point: Some("cs_skin"),
                compilation_options: Default::default(),
                cache: None,
            });
            (pipeline, layout)
        });
        let Some(motion) = &self.motion else {
            return;
        };
        let max_binding = device.limits().max_storage_buffer_binding_size;
        let mut jobs = Vec::new();
        for (index, state) in &mut self.skins {
            let Some((pool_mesh, version)) = resident(*index) else {
                continue;
            };
            let Some((vertex_buffer, _)) = pool.buffers(pool_mesh.page) else {
                continue;
            };
            if vertex_buffer.size() > max_binding {
                continue;
            }
            let bytes = pool_mesh.vertex_count as u64 * crate::mesh_pool::VERTEX_BYTES;
            if state.copied != Some((pool_mesh, version)) {
                // A new upload holds the bind pose: keep it as the rest copy, then pose it.
                if state.rest.size() < bytes {
                    continue;
                }
                encoder.copy_buffer_to_buffer(
                    vertex_buffer,
                    pool_mesh.base_vertex as u64 * crate::mesh_pool::VERTEX_BYTES,
                    &state.rest,
                    0,
                    bytes,
                );
                state.copied = Some((pool_mesh, version));
                state.group = None;
                state.due = true;
                state.fresh = true;
            }
            let at = (pool_mesh.page, pool.generation(pool_mesh.page));
            if state.group.as_ref().is_some_and(|(made, _)| *made != at) {
                // The page's vertex buffer was replaced by a larger one: let go of the old
                // one (the group keeps it alive), and pose once more into the new one, which
                // holds what the old one did when it was copied.
                state.group = None;
                state.due = true;
            }
            if !state.due {
                continue;
            }
            state.due = false;
            let fresh = std::mem::take(&mut state.fresh);
            gpu.queue.write_buffer(
                &state.params,
                0,
                bytemuck::bytes_of(&Params {
                    base_vertex: pool_mesh.base_vertex,
                    count: pool_mesh.vertex_count,
                    linear: (state.blend == SkinBlend::Linear) as u32,
                    bones: state.bone_count as u32,
                    motion: state.motion_offset,
                    fresh: fresh as u32,
                    pad: [0; 2],
                }),
            );
            if state.group.is_none() {
                let group = device.create_bind_group(&wgpu::BindGroupDescriptor {
                    label: Some("gpu skinning"),
                    layout,
                    entries: &[
                        entry(0, &state.params),
                        entry(1, &state.rest),
                        entry(2, &state.joints_buffer),
                        entry(3, &state.weights_buffer),
                        entry(4, &state.bones),
                        entry(5, vertex_buffer),
                        entry(6, motion),
                    ],
                });
                state.group = Some((at, group));
            }
            jobs.push((*index, pool_mesh.vertex_count));
        }
        if jobs.is_empty() {
            return;
        }
        let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
            label: Some("gpu skinning"),
            timestamp_writes: None,
        });
        pass.set_pipeline(pipeline);
        for (index, count) in jobs {
            let Some((_, group)) = self.skins.get(&index).and_then(|s| s.group.as_ref()) else {
                continue;
            };
            pass.set_bind_group(0, group, &[]);
            pass.dispatch_workgroups(count.div_ceil(WORKGROUP), 1, 1);
            self.skinned += 1;
            self.skinned_vertices += count as u64;
        }
    }

    /// Forget skins not drawn for `frames` frames (their entities are gone or far).
    pub fn evict(&mut self, frame: u64, frames: u64) {
        let before = self.skins.len();
        self.skins.retain(|_, s| s.last_seen + frames >= frame);
        self.repack |= self.skins.len() != before;
    }

    /// After `prepare` and `evict`, once a frame: hand out the motion buffer's offsets again
    /// when the skins changed (every skin starts afresh), growing the buffer if they need it.
    /// The buffer exists from the first call on, so the mesh groups can always bind it.
    pub fn place_motion(&mut self, gpu: &GpuShared) {
        if self.repack {
            self.repack = false;
            let mut next = 0u32;
            for state in self.skins.values_mut() {
                state.motion_offset = next;
                state.fresh = true;
                state.due = true;
                state.group = None;
                next += state.rest.size() as u32 / crate::mesh_pool::VERTEX_BYTES as u32;
            }
            if next > self.motion_capacity {
                self.motion = None;
            }
            if self.motion.is_none() {
                self.motion_capacity = next.next_power_of_two().max(MOTION_MIN);
            }
        }
        if self.motion.is_none() {
            self.motion_capacity = self.motion_capacity.max(MOTION_MIN);
            self.motion = Some(gpu.device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("gpu skinning motion"),
                size: self.motion_capacity as u64 * 16,
                usage: wgpu::BufferUsages::STORAGE,
                mapped_at_creation: false,
            }));
            self.motion_replaced = true;
            for state in self.skins.values_mut() {
                state.group = None;
            }
        }
    }

    /// Whether the motion buffer was replaced since the last call.
    pub fn take_motion_replaced(&mut self) -> bool {
        std::mem::take(&mut self.motion_replaced)
    }

    /// Where mesh `index`'s entries start in the motion buffer, when it's skinned.
    pub fn motion_offset(&self, index: u32) -> Option<u32> {
        self.skins.get(&index).map(|s| s.motion_offset)
    }
}

impl Skin {
    fn new(gpu: &GpuShared, mesh: &Mesh, skin: &GpuSkin) -> Self {
        use wgpu::util::DeviceExt;
        let device = &gpu.device;
        let w = &skin.weights;
        let joints: Vec<[u32; 2]> = w
            .joints
            .iter()
            .map(|j| {
                [
                    j[0] as u32 | (j[1] as u32) << 16,
                    j[2] as u32 | (j[3] as u32) << 16,
                ]
            })
            .collect();
        let storage = wgpu::BufferUsages::STORAGE;
        let init = |label, contents: &[u8], usage| {
            device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some(label),
                contents,
                usage,
            })
        };
        let bone_count = skin.bones.len().max(
            w.joints
                .iter()
                .flatten()
                .map(|&j| j as usize + 1)
                .max()
                .unwrap_or(1),
        );
        let bones = vec![SkinBone::IDENTITY; bone_count];
        // Each bone's box of the vertices it moves most, in the bind pose.
        let mut boxes: Vec<Option<(Vec3, Vec3)>> = vec![None; bone_count];
        for (v, (j, wt)) in mesh.vertices.iter().zip(w.joints.iter().zip(&w.weights)) {
            let main = (0..4)
                .max_by(|&a, &b| wt[a].total_cmp(&wt[b]))
                .map_or(0, |k| j[k] as usize);
            let p = Vec3::from(v.position);
            if let Some(b) = boxes.get_mut(main) {
                *b = Some(b.map_or((p, p), |(lo, hi)| (lo.min(p), hi.max(p))));
            }
        }
        Self {
            weights: w.clone(),
            joints_buffer: init(
                "gpu skinning joints",
                bytemuck::cast_slice(&joints),
                storage,
            ),
            weights_buffer: init(
                "gpu skinning weights",
                bytemuck::cast_slice(&w.weights),
                storage,
            ),
            bones: init(
                "gpu skinning bones",
                bytemuck::cast_slice(&bones),
                storage | wgpu::BufferUsages::COPY_DST,
            ),
            bone_count,
            params: device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("gpu skinning params"),
                size: std::mem::size_of::<Params>() as u64,
                usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            }),
            rest: device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("gpu skinning rest vertices"),
                size: (mesh.vertices.len().max(1) * crate::mesh_pool::VERTEX_BYTES as usize) as u64,
                usage: storage | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            }),
            copied: None,
            group: None,
            boxes,
            skinned: Vec::new(),
            blend: skin.blend,
            due: true,
            moving: false,
            motion_offset: 0,
            fresh: true,
            last_seen: 0,
        }
    }

    /// The mesh's posed bounds (center, half extents): each bone's box moved by its bone (the
    /// identity for bones without a pose yet), with a margin for the blends between bones.
    fn bounds(&self) -> (Vec3, Vec3) {
        let (mut lo, mut hi) = (Vec3::splat(f32::MAX), Vec3::splat(f32::MIN));
        let mut margin = 0.0f32;
        for (b, bbox) in self.boxes.iter().enumerate() {
            let Some((min, max)) = bbox else {
                continue;
            };
            let bone = self.skinned.get(b).copied().unwrap_or(SkinBone::IDENTITY);
            for i in 0..8 {
                let corner = Vec3::new(
                    if i & 1 == 0 { min.x } else { max.x },
                    if i & 2 == 0 { min.y } else { max.y },
                    if i & 4 == 0 { min.z } else { max.z },
                );
                let p = bone.apply(corner);
                lo = lo.min(p);
                hi = hi.max(p);
            }
            margin = margin.max((*max - *min).max_element() * 0.1);
        }
        if lo.x > hi.x {
            return (Vec3::ZERO, Vec3::ZERO);
        }
        ((lo + hi) * 0.5, (hi - lo) * 0.5 + Vec3::splat(margin))
    }
}

const SHADER: &str = r#"
struct Params {
    base_vertex: u32,
    count: u32,
    linear: u32,
    bones: u32,
    motion: u32,
    fresh: u32,
    pad0: u32,
    pad1: u32,
}
struct Bone {
    real: vec4<f32>,
    dual: vec4<f32>,
}

@group(0) @binding(0) var<uniform> params: Params;
// The rest vertices as the mesh pool keeps them (`PoolVertex`): 9 words each, the position as
// three floats, then the normal as three signed 16-bit fractions in two words, uv and color.
@group(0) @binding(1) var<storage, read> rest: array<u32>;
@group(0) @binding(2) var<storage, read> joints: array<vec2<u32>>;
@group(0) @binding(3) var<storage, read> weights: array<vec4<f32>>;
@group(0) @binding(4) var<storage, read> bones: array<Bone>;
// The mesh pool page's vertices: the posed position and normal go over the mesh's range.
@group(0) @binding(5) var<storage, read_write> pool: array<u32>;
// Each vertex's posed position last frame (`Skins::motion`), for the prepass's motion vectors.
@group(0) @binding(6) var<storage, read_write> motion: array<vec4<f32>>;

fn rotate(q: vec4<f32>, v: vec3<f32>) -> vec3<f32> {
    let t = 2.0 * cross(q.xyz, v);
    return v + q.w * t + cross(q.xyz, t);
}

fn translation(b: Bone) -> vec3<f32> {
    return 2.0 * (b.real.w * b.dual.xyz - b.dual.w * b.real.xyz + cross(b.real.xyz, b.dual.xyz));
}

fn bone(j: u32) -> Bone {
    return bones[min(j, params.bones - 1u)];
}

@compute @workgroup_size(64)
fn cs_skin(@builtin(global_invocation_id) id: vec3<u32>) {
    let i = id.x;
    if i >= params.count {
        return;
    }
    let r = i * 9u;
    let p = vec3<f32>(
        bitcast<f32>(rest[r]),
        bitcast<f32>(rest[r + 1u]),
        bitcast<f32>(rest[r + 2u]),
    );
    let n = vec3<f32>(unpack2x16snorm(rest[r + 3u]), unpack2x16snorm(rest[r + 4u]).x);
    let jp = joints[i];
    let js = array<u32, 4>(jp.x & 0xffffu, jp.x >> 16u, jp.y & 0xffffu, jp.y >> 16u);
    let w = weights[i];
    let ws = array<f32, 4>(w.x, w.y, w.z, w.w);
    var out_p = p;
    var out_n = n;
    let total = ws[0] + ws[1] + ws[2] + ws[3];
    if total > 1e-6 {
        if params.linear == 1u {
            // Blend the bones' moves of the point (linear blend skinning).
            var sp = vec3<f32>(0.0);
            var sn = vec3<f32>(0.0);
            for (var k = 0u; k < 4u; k++) {
                if ws[k] > 0.0 {
                    let b = bone(js[k]);
                    sp += ws[k] * (rotate(b.real, p) + translation(b));
                    sn += ws[k] * rotate(b.real, n);
                }
            }
            out_p = sp / total;
            out_n = sn;
        } else {
            // Blend the dual quaternions in the first one's hemisphere, then normalize.
            let first = bone(js[0]).real;
            var real = vec4<f32>(0.0);
            var dual = vec4<f32>(0.0);
            for (var k = 0u; k < 4u; k++) {
                if ws[k] > 0.0 {
                    let b = bone(js[k]);
                    let s = select(-ws[k], ws[k], dot(b.real, first) >= 0.0);
                    real += s * b.real;
                    dual += s * b.dual;
                }
            }
            let len = max(length(real), 1e-8);
            let blended = Bone(real / len, dual / len);
            out_p = rotate(blended.real, p) + translation(blended);
            out_n = rotate(blended.real, n);
        }
    }
    out_n = normalize(select(n, out_n, dot(out_n, out_n) > 1e-12));
    let o = (params.base_vertex + i) * 9u;
    // Where the pool posed it last frame (where it is now, on a mesh's first frame).
    let was = vec3<f32>(
        bitcast<f32>(pool[o]),
        bitcast<f32>(pool[o + 1u]),
        bitcast<f32>(pool[o + 2u]),
    );
    motion[params.motion + i] = vec4<f32>(select(was, out_p, params.fresh != 0u), 1.0);
    pool[o] = bitcast<u32>(out_p.x);
    pool[o + 1u] = bitcast<u32>(out_p.y);
    pool[o + 2u] = bitcast<u32>(out_p.z);
    pool[o + 3u] = pack2x16snorm(out_n.xy);
    // The upper half is not the normal's (`PoolVertex::normal`): it stays.
    pool[o + 4u] = (pack2x16snorm(vec2<f32>(out_n.z, 0.0)) & 0xffffu) | (rest[r + 4u] & 0xffff0000u);
}
"#;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_bone_moves_points_as_its_poses_say() {
        let bind = (Quat::from_rotation_z(0.3), Vec3::new(0.0, 1.0, 0.0));
        let now = (
            Quat::from_rotation_y(1.1) * Quat::from_rotation_z(0.3),
            Vec3::new(2.0, 1.5, -1.0),
        );
        let bone = SkinBone::from_poses(bind, now);
        // A point fixed to the bone: its bone-space spot, placed by each pose.
        let local = Vec3::new(0.2, 0.7, -0.4);
        let at_bind = bind.0 * local + bind.1;
        let at_now = now.0 * local + now.1;
        assert!(bone.apply(at_bind).distance(at_now) < 1e-5);
        assert!(SkinBone::IDENTITY.apply(at_bind).distance(at_bind) < 1e-6);
        let t = Vec3::new(-3.0, 0.5, 2.0);
        assert!(SkinBone::new(Quat::IDENTITY, t).translation().distance(t) < 1e-6);
    }

    #[test]
    fn shader_validates() {
        let code = crate::features::compose_shader(SHADER, false);
        crate::features::validate_wgsl("gpu skinning", &code).unwrap();
    }
}
