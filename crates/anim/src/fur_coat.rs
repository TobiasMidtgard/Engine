//! Furred skins exported for the one fur renderer (characters week S1): every animal whose
//! [`SkinMaterial`] grows fur ([`SkinMaterial::fur_length`] > 0) gets an
//! `engine_world::FurCoat` built from its mesh, which the hair module grows strands on and
//! draws through the shared strand renderer. The coat is built once per mesh and fur look (a
//! pack shares its arrays), and the animal's own fur shells stop once it wears one. Nothing is
//! exported without a fur drawer (`engine_world::FurCoatDrawer`), so the shells stay then.

use std::collections::HashMap;
use std::sync::Arc;

use engine_assets::{Assets, Handle};
use engine_core::prelude::*;
use engine_world::animation::{BonePose, SkeletonPose};
use engine_world::{FurCoat, FurCoatDrawer, FurStyle};

use crate::skin::{SkinMaterial, SkinnedMesh, SkinnedMeshRenderer};

/// On an entity wearing a [`FurCoat`] exported from its mesh: which mesh.
#[derive(Component, Debug, Clone, Copy, PartialEq)]
pub struct FurCoatFrom(pub Handle<SkinnedMesh>);

/// The coat of `mesh` worn by a skeleton like `pose`, its fur as `material` says (`None`
/// without fur).
pub fn coat_of(
    mesh: &SkinnedMesh,
    material: &SkinMaterial,
    pose: &SkeletonPose,
) -> Option<FurCoat> {
    if material.fur_length <= 0.0 || mesh.vertices.is_empty() {
        return None;
    }
    // Mesh joints by the pose's bone index; bind poses (model space) for the bones they name.
    let bone_of: Vec<u16> = mesh
        .joints
        .iter()
        .map(|n| pose.index_of(n).unwrap_or(0) as u16)
        .collect();
    let mut grown_on: Vec<BonePose> = pose.bones.clone();
    for (j, inv) in mesh.inverse_bind.iter().enumerate() {
        let (scale, rotation, translation) = inv.inverse().to_scale_rotation_translation();
        if let Some(b) = bone_of.get(j).and_then(|b| grown_on.get_mut(*b as usize)) {
            *b = BonePose {
                translation,
                rotation,
                scale,
            };
        }
    }
    let v = &mesh.vertices;
    let tint = Vec3::new(material.tint[0], material.tint[1], material.tint[2]);
    Some(FurCoat {
        version: 1,
        positions: v.iter().map(|v| v.position).collect::<Vec<_>>().into(),
        normals: v.iter().map(|v| v.normal).collect::<Vec<_>>().into(),
        indices: mesh.indices.clone().into(),
        joints: v
            .iter()
            .map(|v| {
                v.joints
                    .map(|j| bone_of.get(j as usize).copied().unwrap_or(0))
            })
            .collect::<Vec<_>>()
            .into(),
        weights: v.iter().map(|v| v.weights).collect::<Vec<_>>().into(),
        grown_on: grown_on.into(),
        color: v
            .iter()
            .map(|v| (v.color.truncate() * tint).to_array())
            .collect::<Vec<_>>()
            .into(),
        amount: v
            .iter()
            .map(|v| smoothstep(0.02, 0.2, v.fur))
            .collect::<Vec<_>>()
            .into(),
        length: v.iter().map(|v| v.fur.max(0.25)).collect::<Vec<_>>().into(),
        comb: Arc::from([]),
        style: FurStyle {
            length: material.fur_length,
            density: 300.0,
            tip: material.fur_tip,
            radius: 0.00005,
            stiffness: (0.95 - 0.5 * material.fur_droop).clamp(0.2, 0.95),
            curl: 0.0,
        },
    })
}

/// Give every furred animal its coat (once per mesh), and stop its own shells.
#[allow(clippy::type_complexity)]
pub fn export_fur_coats(
    mut commands: Commands,
    meshes: Option<Res<Assets<SkinnedMesh>>>,
    mut q: Query<(
        Entity,
        &mut SkinnedMeshRenderer,
        &SkeletonPose,
        Option<&FurCoatFrom>,
    )>,
    drawer: Option<Res<FurCoatDrawer>>,
    mut made: Local<HashMap<(Handle<SkinnedMesh>, [u32; 5]), Option<FurCoat>>>,
) {
    let (Some(meshes), Some(_)) = (meshes, drawer) else {
        return;
    };
    for (e, mut renderer, pose, from) in &mut q {
        let Some(handle) = renderer.mesh else {
            continue;
        };
        if from.is_some_and(|f| f.0 == handle) || renderer.material.fur_length <= 0.0 {
            continue;
        }
        let Some(mesh) = meshes.get(handle) else {
            continue;
        };
        let m = &renderer.material;
        let look = [m.fur_length, m.fur_droop, m.tint[0], m.tint[1], m.tint[2]].map(f32::to_bits);
        let coat = made
            .entry((handle, look))
            .or_insert_with(|| coat_of(mesh, &renderer.material, pose))
            .clone();
        let Some(coat) = coat else {
            continue;
        };
        if coat.grown_on.len() != pose.bones.len() {
            continue;
        }
        commands.entity(e).insert((coat, FurCoatFrom(handle)));
        renderer.material.fur_shells = 0;
    }
}
