//! Bone sockets: props, weapons and effects that follow a bone of an animated character.

use engine_core::prelude::*;

use crate::animator::Animator;
use crate::xform::Xf;

/// Make this entity follow a bone of `target` (a hammer in a hand, a hat on a head, a torch
/// flame at a wrist). Writes this entity's `Transform` (and `GlobalTransform`, so it is
/// exact this frame) every frame after the pose is final. Put it on a root entity (no parent).
#[derive(Component, Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct BoneAttachment {
    #[serde(skip)]
    pub target: Option<Entity>,
    pub bone: String,
    /// Offset in the bone's frame, meters.
    pub offset: Vec3,
    /// Rotation in the bone's frame.
    pub rotation: Quat,
}

impl Default for BoneAttachment {
    fn default() -> Self {
        Self {
            target: None,
            bone: String::new(),
            offset: Vec3::ZERO,
            rotation: Quat::IDENTITY,
        }
    }
}

impl BoneAttachment {
    pub fn new(target: Entity, bone: &str) -> Self {
        Self {
            target: Some(target),
            bone: bone.into(),
            ..Default::default()
        }
    }

    pub fn with_offset(mut self, offset: Vec3, rotation: Quat) -> Self {
        self.offset = offset;
        self.rotation = rotation;
        self
    }
}

pub(crate) fn attachment_system(
    characters: Query<(&Animator, &GlobalTransform), Without<BoneAttachment>>,
    mut q: Query<(&BoneAttachment, &mut Transform, &mut GlobalTransform)>,
) {
    for (attach, mut t, mut gt) in &mut q {
        let Some((animator, owner)) = attach.target.and_then(|e| characters.get(e).ok()) else {
            continue;
        };
        let Some(bone) = animator.skeleton.index_of(&attach.bone) else {
            continue;
        };
        let socket = animator.model[bone].mul(&Xf::from_tr(attach.offset, attach.rotation));
        let (pos, rot) = animator_world(owner, socket);
        t.translation = pos;
        t.rotation = rot;
        *gt = GlobalTransform::from(*t);
    }
}

fn animator_world(owner: &GlobalTransform, model: Xf) -> (DVec3, Quat) {
    let rot = owner.rotation();
    (
        owner.translation() + (rot * (model.t * owner.scale())).as_dvec3(),
        rot * model.r,
    )
}
