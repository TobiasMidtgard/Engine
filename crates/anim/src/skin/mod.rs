//! Skinned meshes and GPU skinning.
//!
//! A [`SkinnedMesh`] is a triangle mesh whose vertices follow up to four joints each (linear
//! blend skinning), plus sparse morph targets (muscle bulges, corrective shapes, faces). Put
//! one on a character with [`SkinnedMeshRenderer`]; the [`SkinningFeature`](feature::SkinningFeature)
//! render feature deforms it on the GPU every frame:
//!
//! 1. **Skin pass** (compute): morph targets, then joints, into a per-instance vertex buffer.
//! 2. **Tension pass** (compute): compares each vertex's edges with the rest pose. Negative
//!    tension = compressed (cloth bunches and folds), positive = stretched (fabric pulls tight).
//! 3. **Draw**: shaded with the engine's `shade_surface`, so characters are lit exactly like
//!    everything else; cloth regions get wrinkles from tension, skin regions get veins and
//!    a flush from muscle activation. Shadows use the same skinned vertices.
//!
//! Detail work (morphs, tension) only runs near the camera; far characters skip it.

pub mod feature;

use std::sync::Arc;

use engine_assets::{Asset, Assets, Handle};
use engine_core::math::Aabb;
use engine_core::prelude::*;
use engine_render::StandardMaterial;

use crate::animator::Animator;
use crate::muscle::MorphWeights;
use crate::retarget::Retarget;

/// One vertex of a [`SkinnedMesh`].
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SkinVertex {
    pub position: Vec3,
    pub normal: Vec3,
    pub uv: Vec2,
    /// Linear RGBA, multiplied with the material tint.
    pub color: Vec4,
    /// Joint indices into [`SkinnedMesh::joints`].
    pub joints: [u16; 4],
    /// Joint weights (sum to 1).
    pub weights: [f32; 4],
    /// Stable coordinates for procedural detail (wrinkles, veins); the bind-pose position.
    pub pattern: Vec3,
    /// Direction along the limb or body part; folds run across it, veins along it.
    pub axis: Vec3,
    /// x: cloth (wrinkles and bunching), y: skin (subsurface, flush), z: veins, w: sheen.
    pub mask: Vec4,
}

impl Default for SkinVertex {
    fn default() -> Self {
        Self {
            position: Vec3::ZERO,
            normal: Vec3::Y,
            uv: Vec2::ZERO,
            color: Vec4::ONE,
            joints: [0; 4],
            weights: [1.0, 0.0, 0.0, 0.0],
            pattern: Vec3::ZERO,
            axis: Vec3::Y,
            mask: Vec4::ZERO,
        }
    }
}

/// A sparse offset of some vertices: a muscle bulge, a jiggle direction, a facial shape.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct MorphTarget {
    pub name: String,
    pub deltas: Vec<MorphDelta>,
}

/// One vertex's offset in a [`MorphTarget`] at weight 1.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct MorphDelta {
    pub vertex: u32,
    pub position: Vec3,
    pub normal: Vec3,
    /// How much this vertex counts as "in the muscle" (0..1) for vein and flush detail.
    pub influence: f32,
}

/// A mesh deformed by a skeleton.
#[derive(Debug, Clone, Default)]
pub struct SkinnedMesh {
    pub vertices: Vec<SkinVertex>,
    /// Counter-clockwise triangles.
    pub indices: Vec<u32>,
    /// Bone name of each joint index used by the vertices.
    pub joints: Vec<String>,
    /// Mesh space → joint space at bind time, per joint.
    pub inverse_bind: Vec<Mat4>,
    pub morph_targets: Vec<MorphTarget>,
    /// Index ranges drawn with their own surface (see [`SkinnedMeshRenderer::surfaces`]).
    /// Empty: the whole mesh is one surface.
    pub submeshes: Vec<Submesh>,
}

/// A range of a mesh's triangles drawn with one textured surface.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Submesh {
    /// First index into [`SkinnedMesh::indices`].
    pub first_index: u32,
    pub index_count: u32,
    /// Index into [`SkinnedMeshRenderer::surfaces`].
    pub surface: usize,
}

impl Asset for SkinnedMesh {}

impl SkinnedMesh {
    pub fn bounds(&self) -> Aabb {
        Aabb::from_points(self.vertices.iter().map(|v| v.position))
    }

    pub fn morph_names(&self) -> Arc<[String]> {
        self.morph_targets
            .iter()
            .map(|t| t.name.clone())
            .collect::<Vec<_>>()
            .into()
    }

    pub fn triangle_count(&self) -> usize {
        self.indices.len() / 3
    }

    /// Recompute smooth vertex normals from the triangles.
    pub fn compute_normals(&mut self) {
        let mut n = vec![Vec3::ZERO; self.vertices.len()];
        for t in self.indices.as_chunks::<3>().0 {
            let [a, b, c] = [t[0] as usize, t[1] as usize, t[2] as usize];
            let pa = self.vertices[a].position;
            let face = (self.vertices[b].position - pa).cross(self.vertices[c].position - pa);
            n[a] += face;
            n[b] += face;
            n[c] += face;
        }
        for (v, n) in self.vertices.iter_mut().zip(n) {
            v.normal = n.normalize_or(Vec3::Y);
        }
    }

    /// Every vertex's unique neighbors with the rest-pose edge length (for cloth tension).
    pub fn neighbors(&self) -> Vec<Vec<(u32, f32)>> {
        let mut out: Vec<Vec<(u32, f32)>> = vec![Vec::new(); self.vertices.len()];
        for t in self.indices.as_chunks::<3>().0 {
            for (a, b) in [(t[0], t[1]), (t[1], t[2]), (t[2], t[0])] {
                let (pa, pb) = (
                    self.vertices[a as usize].position,
                    self.vertices[b as usize].position,
                );
                let l = (pa - pb).length();
                if l <= 1e-7 {
                    continue;
                }
                if !out[a as usize].iter().any(|n| n.0 == b) {
                    out[a as usize].push((b, l));
                }
                if !out[b as usize].iter().any(|n| n.0 == a) {
                    out[b as usize].push((a, l));
                }
            }
        }
        out
    }
}

/// Surface settings of a skinned character. Colors come from the mesh's vertex colors times
/// `tint`; the masks in each vertex say what is cloth, skin or veined.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct SkinMaterial {
    /// Linear RGBA multiplier.
    pub tint: [f32; 4],
    pub skin_roughness: f32,
    pub cloth_roughness: f32,
    /// Light wrap through skin (0..1).
    pub subsurface: f32,
    pub metallic: f32,
    /// Cloth wrinkle depth from tension (0 = smooth fabric).
    pub wrinkles: f32,
    /// Wrinkle frequency, folds per meter.
    pub wrinkle_frequency: f32,
    /// Vein and striation detail from muscle activation (0 = none).
    pub veins: f32,
    /// How far compressed cloth bunches outward, meters.
    pub bunch: f32,
    /// Linear RGB of veins.
    pub vein_color: [f32; 3],
    /// Fabric/fur sheen at grazing angles (0..1).
    pub sheen: f32,
    /// Linear RGB emission.
    pub emissive: [f32; 3],
    /// Emission luminance, cd/m².
    pub emissive_strength: f32,
}

impl Default for SkinMaterial {
    fn default() -> Self {
        Self {
            tint: [1.0; 4],
            skin_roughness: 0.5,
            cloth_roughness: 0.85,
            subsurface: 0.35,
            metallic: 0.0,
            wrinkles: 1.0,
            wrinkle_frequency: 28.0,
            veins: 1.0,
            bunch: 0.012,
            vein_color: [0.32, 0.38, 0.6],
            sheen: 0.3,
            emissive: [0.0; 3],
            emissive_strength: 0.0,
        }
    }
}

/// Draw a [`SkinnedMesh`] deformed by this entity's [`Animator`].
#[derive(Component, Debug, Clone, PartialEq, Serialize, Deserialize)]
#[require(Transform, SkinPose)]
#[serde(default)]
pub struct SkinnedMeshRenderer {
    pub mesh: Option<Handle<SkinnedMesh>>,
    pub material: SkinMaterial,
    /// Textured surfaces for the mesh's [`Submesh`]es (imported characters): base color,
    /// normal and metallic-roughness textures, alpha cutouts. The procedural detail of
    /// `material` (cloth folds, veins, flush) still applies on top, by the vertex mask.
    pub surfaces: Vec<StandardMaterial>,
    pub cast_shadows: bool,
    pub visible: bool,
    /// Beyond this distance from the camera, skip morphs and cloth tension, meters.
    pub detail_distance: f32,
}

impl Default for SkinnedMeshRenderer {
    fn default() -> Self {
        Self {
            mesh: None,
            material: SkinMaterial::default(),
            surfaces: Vec::new(),
            cast_shadows: true,
            visible: true,
            detail_distance: 45.0,
        }
    }
}

impl SkinnedMeshRenderer {
    pub fn new(mesh: Handle<SkinnedMesh>, material: SkinMaterial) -> Self {
        Self {
            mesh: Some(mesh),
            material,
            ..Default::default()
        }
    }
}

/// Joint matrices and morph weights ready for the GPU (written every frame from the
/// [`Animator`]'s final pose).
#[derive(Component, Debug, Clone, Default)]
pub struct SkinPose {
    /// Per mesh joint: rows of the 3×4 model-space skinning matrix.
    pub joints: Vec<[[f32; 4]; 3]>,
    pub morph_weights: Vec<f32>,
    /// Mesh joint → skeleton bone, resolved by name.
    map: Vec<Option<JointSource>>,
    map_version: u64,
}

/// Where a mesh joint's transform comes from.
#[derive(Debug, Clone, Copy, PartialEq)]
enum JointSource {
    /// The animator's skeleton.
    Logic(u16),
    /// The retargeted skeleton of a [`Retarget`].
    Target(u16),
}

pub(crate) fn update_skin_poses(
    meshes: Option<Res<Assets<SkinnedMesh>>>,
    mut q: Query<(
        &Animator,
        &SkinnedMeshRenderer,
        &mut SkinPose,
        Option<&MorphWeights>,
        Option<&Retarget>,
    )>,
) {
    let Some(meshes) = meshes else {
        return;
    };
    q.par_iter_mut()
        .for_each(|(animator, renderer, mut pose, morphs, retarget)| {
            let Some(handle) = renderer.mesh else {
                return;
            };
            let Some(mesh) = meshes.get(handle) else {
                return;
            };
            let version = meshes.version(handle);
            // A retargeted mesh's joints name the target skeleton first, then logic bones.
            let target = retarget.map(|r| &r.retargeter.target);
            let resolved_for_target = pose
                .map
                .first()
                .is_some_and(|m| matches!(m, Some(JointSource::Target(_))) == target.is_some());
            if pose.map_version != version
                || pose.map.len() != mesh.joints.len()
                || !resolved_for_target
            {
                pose.map = mesh
                    .joints
                    .iter()
                    .map(|j| {
                        target
                            .and_then(|t| t.index_of(j))
                            .map(|i| JointSource::Target(i as u16))
                            .or_else(|| {
                                animator
                                    .skeleton
                                    .index_of(j)
                                    .map(|i| JointSource::Logic(i as u16))
                            })
                    })
                    .collect();
                pose.map_version = version;
            }
            let pose = &mut *pose;
            pose.joints.clear();
            for (j, bone) in pose.map.iter().enumerate() {
                let x = match bone {
                    Some(JointSource::Logic(b)) => animator.model.get(*b as usize),
                    Some(JointSource::Target(b)) => retarget.and_then(|r| r.model.get(*b as usize)),
                    None => None,
                };
                let m = match x {
                    Some(x) => {
                        x.to_mat4() * mesh.inverse_bind.get(j).copied().unwrap_or(Mat4::IDENTITY)
                    }
                    None => Mat4::IDENTITY,
                };
                pose.joints.push([
                    m.row(0).to_array(),
                    m.row(1).to_array(),
                    m.row(2).to_array(),
                ]);
            }
            pose.morph_weights.clear();
            pose.morph_weights.resize(mesh.morph_targets.len(), 0.0);
            if let Some(m) = morphs {
                for (w, v) in pose.morph_weights.iter_mut().zip(&m.weights) {
                    *w = *v;
                }
            }
        });
}

/// Deform a mesh on the CPU exactly like the GPU skin pass (morphs, then joints). Used by
/// tests, picking and tools; returns positions and normals.
pub fn skin_on_cpu(mesh: &SkinnedMesh, pose: &SkinPose) -> (Vec<Vec3>, Vec<Vec3>) {
    let mut p: Vec<Vec3> = mesh.vertices.iter().map(|v| v.position).collect();
    let mut n: Vec<Vec3> = mesh.vertices.iter().map(|v| v.normal).collect();
    for (t, target) in mesh.morph_targets.iter().enumerate() {
        let w = pose.morph_weights.get(t).copied().unwrap_or(0.0);
        if w == 0.0 {
            continue;
        }
        for d in &target.deltas {
            p[d.vertex as usize] += d.position * w;
            n[d.vertex as usize] += d.normal * w;
        }
    }
    for (i, v) in mesh.vertices.iter().enumerate() {
        let mut m = Mat4::ZERO;
        for k in 0..4 {
            let w = v.weights[k];
            if w <= 0.0 {
                continue;
            }
            let rows = pose.joints.get(v.joints[k] as usize).copied().unwrap_or([
                [1.0, 0.0, 0.0, 0.0],
                [0.0, 1.0, 0.0, 0.0],
                [0.0, 0.0, 1.0, 0.0],
            ]);
            let jm = Mat4::from_cols_array_2d(&[rows[0], rows[1], rows[2], [0.0, 0.0, 0.0, 1.0]])
                .transpose();
            m += jm * w;
        }
        p[i] = m.transform_point3(p[i]);
        n[i] = m.transform_vector3(n[i]).normalize_or(Vec3::Y);
    }
    (p, n)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::skeleton::{BoneDef, Skeleton};

    fn bar() -> (Arc<Skeleton>, SkinnedMesh) {
        let s = Arc::new(
            Skeleton::new(
                "bar",
                vec![
                    BoneDef::new("a", None, Vec3::ZERO),
                    BoneDef::new("b", Some("a"), Vec3::Y),
                ],
            )
            .unwrap(),
        );
        let mut mesh = SkinnedMesh {
            joints: vec!["a".into(), "b".into()],
            inverse_bind: s.inverse_bind.clone(),
            ..Default::default()
        };
        for (y, w) in [(0.0, 0.0), (1.0, 0.5), (2.0, 1.0)] {
            mesh.vertices.push(SkinVertex {
                position: Vec3::new(0.1, y, 0.0),
                joints: [0, 1, 0, 0],
                weights: [1.0 - w, w, 0.0, 0.0],
                ..Default::default()
            });
        }
        mesh.morph_targets.push(MorphTarget {
            name: "bulge".into(),
            deltas: vec![MorphDelta {
                vertex: 1,
                position: Vec3::X * 0.05,
                normal: Vec3::ZERO,
                influence: 1.0,
            }],
        });
        (s, mesh)
    }

    #[test]
    fn cpu_skinning_follows_joints_and_morphs() {
        let (s, mesh) = bar();
        let mut a = Animator::procedural(s);
        a.local[1].r = Quat::from_rotation_z(std::f32::consts::FRAC_PI_2);
        a.refresh_model();
        let mut pose = SkinPose::default();
        for j in 0..2 {
            let m = a.model[j].to_mat4() * mesh.inverse_bind[j];
            pose.joints.push([
                m.row(0).to_array(),
                m.row(1).to_array(),
                m.row(2).to_array(),
            ]);
        }
        let (flat, _) = skin_on_cpu(&mesh, &pose);
        pose.morph_weights = vec![1.0];
        let (p, _) = skin_on_cpu(&mesh, &pose);
        // The top vertex follows bone b fully: (0.1, 2) rotates about (0, 1) to (-1, 1.1).
        assert!(
            p[2].abs_diff_eq(Vec3::new(-1.0, 1.1, 0.0), 1e-4),
            "{:?}",
            p[2]
        );
        // The bottom vertex stays.
        assert!(p[0].abs_diff_eq(Vec3::new(0.1, 0.0, 0.0), 1e-5));
        // The middle vertex got its bulge before skinning (half follows each bone).
        assert!(
            flat[1].abs_diff_eq(Vec3::new(0.05, 1.05, 0.0), 1e-5),
            "{:?}",
            flat[1]
        );
        assert!(
            p[1].abs_diff_eq(Vec3::new(0.075, 1.075, 0.0), 1e-5),
            "{:?}",
            p[1]
        );
    }

    #[test]
    fn neighbors_have_rest_lengths() {
        let mut mesh = SkinnedMesh::default();
        for p in [Vec3::ZERO, Vec3::X, Vec3::Y] {
            mesh.vertices.push(SkinVertex {
                position: p,
                ..Default::default()
            });
        }
        mesh.indices = vec![0, 1, 2];
        let n = mesh.neighbors();
        assert_eq!(n[0].len(), 2);
        assert!(
            n[1].iter()
                .any(|(v, l)| *v == 2 && (l - 2f32.sqrt()).abs() < 1e-6)
        );
    }
}
