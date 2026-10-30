//! Rigid glTF models driven by a procedural rig (the paddock car): every mesh of the file
//! becomes a [`BoneAttachment`] of one bone, so a body rolls on its springs and wheels steer
//! and spin while the file's own materials and detail stay as authored.

use engine_assets::{Assets, Handle};
use engine_core::prelude::*;
use engine_render::{GltfScene, MaterialRef, MeshRenderer, StandardMaterial};

use crate::animator::Animator;
use crate::attach::BoneAttachment;
use crate::xform::Xf;

/// A glTF scene worn by this entity's [`Animator`]. Once the file has loaded, each mesh is
/// spawned as its own entity following a bone; they despawn with this entity.
#[derive(Component, Debug, Clone)]
pub struct RigModel {
    pub scene: Handle<GltfScene>,
    /// The file's frame in the rig's model space (where the file's origin sits, and how it
    /// turns: a file facing +Z on a rig facing −Z turns half a circle).
    pub placement: Xf,
    /// Bone that meshes follow unless [`follow`](Self::follow) says otherwise.
    pub bone: String,
    /// glTF node name → bone: the node and everything under it follow that bone (the most
    /// specific entry wins, so a brake caliper inside a wheel can follow the hub instead).
    pub follow: Vec<(String, String)>,
    /// Nodes posed in the file (a wheel modeled steered and turned) whose own rotation is
    /// dropped, so the rig's bone alone turns them.
    pub upright: Vec<String>,
    /// Node name → how its meshes look instead of the file's material.
    pub looks: Vec<(String, PartLook)>,
    /// glTF material index → a change to the file's material for every mesh that uses it
    /// (glass that needs transmission, which the renderer draws as blended glass instead; a
    /// normal map tiled with a texture transform the loader doesn't apply).
    pub materials: Vec<(usize, MaterialEdit)>,
    pub(crate) spawned: bool,
}

/// A change to one of a [`RigModel`]'s file materials.
pub type MaterialEdit = fn(&mut StandardMaterial);

/// How a node of a [`RigModel`] is drawn.
#[derive(Debug, Clone, PartialEq)]
pub enum PartLook {
    /// Not drawn (a decal or logo that must not show).
    Hidden,
    Material(StandardMaterial),
}

impl RigModel {
    pub fn new(scene: Handle<GltfScene>, bone: &str) -> Self {
        Self {
            scene,
            placement: Xf::IDENTITY,
            bone: bone.into(),
            follow: Vec::new(),
            upright: Vec::new(),
            looks: Vec::new(),
            materials: Vec::new(),
            spawned: false,
        }
    }

    /// The bone a node follows, given the bone its parent follows.
    fn bone_for<'a>(&'a self, node: Option<&str>, inherited: &'a str) -> &'a str {
        node.and_then(|n| self.follow.iter().find(|(name, _)| name == n))
            .map_or(inherited, |(_, bone)| bone.as_str())
    }

    /// Every drawn mesh of `scene`: (node name, bone, rest pose in the rig's model space,
    /// scale, renderer). `file_material(i)` is the file's material `i` as loaded.
    fn parts(
        &self,
        scene: &GltfScene,
        file_material: impl Fn(&str) -> Option<StandardMaterial>,
    ) -> Vec<(String, String, Xf, Vec3, MeshRenderer)> {
        let mut out = Vec::new();
        let mut stack: Vec<(usize, Xf, Vec3, String)> = scene
            .roots
            .iter()
            .rev()
            .map(|&r| (r, self.placement, Vec3::ONE, self.bone.clone()))
            .collect();
        let mut visited = vec![false; scene.nodes.len()];
        while let Some((index, parent, parent_scale, inherited)) = stack.pop() {
            let Some(node) = scene.nodes.get(index) else {
                continue;
            };
            if std::mem::replace(&mut visited[index], true) {
                continue;
            }
            let name = node.name.as_deref();
            let bone = self.bone_for(name, &inherited).to_string();
            let upright = name.is_some_and(|n| self.upright.iter().any(|u| u == n));
            let local = Xf::from_tr(
                node.transform.translation.as_vec3() * parent_scale,
                if upright {
                    Quat::IDENTITY
                } else {
                    node.transform.rotation
                },
            );
            let pose = parent.mul(&local);
            let scale = parent_scale * node.transform.scale;
            let look = name.and_then(|n| self.looks.iter().find(|(l, _)| l == n));
            for primitive in &node.primitives {
                let mut renderer = primitive.renderer();
                match look {
                    Some((_, PartLook::Hidden)) => continue,
                    Some((_, PartLook::Material(m))) => renderer.material = MaterialRef::Inline(*m),
                    None => {
                        let key = primitive.material.as_deref().unwrap_or_default();
                        let index = key
                            .rsplit_once("#Material")
                            .and_then(|(_, i)| i.parse::<usize>().ok());
                        if let Some((_, edit)) =
                            self.materials.iter().find(|(i, _)| Some(*i) == index)
                        {
                            let mut m = file_material(key).unwrap_or_default();
                            edit(&mut m);
                            renderer.material = MaterialRef::Inline(m);
                        }
                    }
                }
                out.push((
                    name.unwrap_or("part").to_string(),
                    bone.clone(),
                    pose,
                    scale,
                    renderer,
                ));
            }
            for &child in node.children.iter().rev() {
                stack.push((child, pose, scale, bone.clone()));
            }
        }
        out
    }
}

/// A mesh spawned for a [`RigModel`].
#[derive(Component, Debug, Clone, Copy)]
pub struct RigModelPart {
    pub owner: Entity,
}

/// Spawn the meshes of every [`RigModel`] whose file has loaded.
pub(crate) fn spawn_rig_models(
    mut commands: Commands,
    scenes: Option<Res<Assets<GltfScene>>>,
    materials: Option<Res<Assets<StandardMaterial>>>,
    mut q: Query<(Entity, &mut RigModel, &Animator, &GlobalTransform)>,
) {
    let Some(scenes) = scenes else {
        return;
    };
    for (entity, mut model, animator, gt) in &mut q {
        if model.spawned {
            continue;
        }
        let Some(scene) = scenes.get(model.scene) else {
            continue;
        };
        model.spawned = true;
        let skeleton = &animator.skeleton;
        let file_material = |key: &str| {
            let m = materials.as_deref()?;
            m.get(m.loaded_by_key(key)?).copied()
        };
        for (name, bone, pose, scale, renderer) in model.parts(scene, file_material) {
            let Some(b) = skeleton.index_of(&bone) else {
                tracing::warn!("rig model: no bone {bone} for {name}");
                continue;
            };
            let socket = skeleton.rest_model[b].inverse().mul(&pose);
            let t = Transform {
                translation: gt.translation() + (gt.rotation() * pose.t).as_dvec3(),
                rotation: gt.rotation() * pose.r,
                scale,
            };
            commands.spawn((
                Name::new(name),
                t,
                GlobalTransform::from(t),
                renderer,
                BoneAttachment {
                    target: Some(entity),
                    bone,
                    offset: socket.t,
                    rotation: socket.r,
                    hang: None,
                    fist: false,
                },
                RigModelPart { owner: entity },
            ));
        }
    }
}

/// Despawn the meshes of rig models that are gone.
pub(crate) fn despawn_orphan_parts(
    mut commands: Commands,
    parts: Query<(Entity, &RigModelPart)>,
    owners: Query<(), With<RigModel>>,
) {
    for (e, part) in &parts {
        if owners.get(part.owner).is_err() {
            commands.entity(e).despawn();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use engine_render::{GltfNode, GltfPrimitive};

    fn node(name: &str, t: Vec3, r: Quat, children: Vec<usize>, mesh: bool) -> GltfNode {
        GltfNode {
            name: Some(name.into()),
            transform: Transform::from_translation(t.as_dvec3()).with_rotation(r),
            children,
            primitives: if mesh {
                vec![GltfPrimitive {
                    mesh: format!("car.glb#{name}"),
                    material: Some("car.glb#Material3".into()),
                }]
            } else {
                Vec::new()
            },
            mesh: None,
            skin: None,
        }
    }

    #[test]
    fn parts_follow_their_bones_and_posed_wheels_stand_upright() {
        let steered = Quat::from_rotation_y(0.5) * Quat::from_rotation_x(1.0);
        let scene = GltfScene {
            name: None,
            source: "car.glb".into(),
            nodes: vec![
                node("Body", Vec3::ZERO, Quat::IDENTITY, vec![1, 3], true),
                node("Wheel", Vec3::new(0.9, 0.35, -1.4), steered, vec![2], false),
                node(
                    "Caliper",
                    Vec3::new(0.05, 0.0, 0.0),
                    Quat::IDENTITY,
                    vec![],
                    true,
                ),
                node(
                    "Logo",
                    Vec3::new(0.0, 0.5, 2.0),
                    Quat::IDENTITY,
                    vec![],
                    true,
                ),
            ],
            roots: vec![0],
        };
        let mut model = RigModel::new(Handle::from_index(0), "body");
        model.placement = Xf::from_tr(
            Vec3::new(0.0, 0.1, 0.0),
            Quat::from_rotation_y(std::f32::consts::PI),
        );
        model.follow = vec![
            ("Wheel".into(), "wheel".into()),
            ("Caliper".into(), "hub".into()),
        ];
        model.upright = vec!["Wheel".into()];
        model.looks = vec![("Logo".into(), PartLook::Hidden)];
        model.materials = vec![(3, |m| m.perceptual_roughness = 0.05)];
        let parts = model.parts(&scene, |key| {
            (key == "car.glb#Material3").then(|| StandardMaterial::default().with_metallic(1.0))
        });
        let names: Vec<_> = parts.iter().map(|p| (p.0.as_str(), p.1.as_str())).collect();
        assert_eq!(names, [("Body", "body"), ("Caliper", "hub")]);
        // The caliper sits beside the wheel's hub, turned with the file but not with the
        // wheel's modeled steer and spin.
        let caliper = &parts[1].2;
        let want = Vec3::new(-0.95, 0.45, 1.4);
        assert!(caliper.t.distance(want) < 1e-5, "{}", caliper.t);
        assert!(
            caliper
                .r
                .angle_between(Quat::from_rotation_y(std::f32::consts::PI))
                < 1e-5
        );
        // The file's material, changed.
        assert!(matches!(&parts[0].4.material,
            MaterialRef::Inline(m) if m.perceptual_roughness == 0.05 && m.metallic == 1.0));
    }
}
