//! Bone sockets: props, weapons and effects that follow a bone of an animated character.

use engine_core::prelude::*;

use crate::animator::Animator;
use crate::xform::{Xf, rotation_between};

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
    /// Let a held prop hang from the hand while no clip holds it up.
    pub hang: Option<Hang>,
}

/// A prop held loosely, like a sledgehammer carried at the side: while none of `curves` is
/// on, it swings about `pivot` (the fist, in the bone's frame) until `axis` (its long axis in
/// the bone's frame) points along `toward` (model space). As a curve comes on, the prop turns
/// back into the hand's frame (two hands on a swing, a hammer braced on the shoulder).
/// Several attachments with the same hang move as one rigid prop.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Hang {
    pub pivot: Vec3,
    pub axis: Vec3,
    pub toward: Vec3,
    pub curves: Vec<String>,
}

impl Default for Hang {
    fn default() -> Self {
        Self {
            pivot: Vec3::ZERO,
            axis: Vec3::NEG_Y,
            toward: Vec3::NEG_Y,
            curves: Vec::new(),
        }
    }
}

impl Default for BoneAttachment {
    fn default() -> Self {
        Self {
            target: None,
            bone: String::new(),
            offset: Vec3::ZERO,
            rotation: Quat::IDENTITY,
            hang: None,
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

    pub fn with_hang(mut self, hang: Hang) -> Self {
        self.hang = Some(hang);
        self
    }

    /// Where the attachment is this frame, in the animator's model space.
    pub fn socket(&self, animator: &Animator) -> Option<Xf> {
        let bone = animator.model[animator.skeleton.index_of(&self.bone)?];
        let Some(hang) = &self.hang else {
            return Some(bone.mul(&Xf::from_tr(self.offset, self.rotation)));
        };
        let held = hang
            .curves
            .iter()
            .map(|c| animator.curve(c))
            .fold(0.0, f32::max)
            .clamp(0.0, 1.0);
        let axis = (bone.r * hang.axis).normalize_or(Vec3::NEG_Y);
        let swing = rotation_between(axis, hang.toward.normalize_or(Vec3::NEG_Y));
        let r = (Quat::IDENTITY.slerp(swing, 1.0 - held) * bone.r).normalize();
        let pivot = bone.transform_point(hang.pivot);
        Some(Xf::from_tr(
            pivot + r * (self.offset - hang.pivot),
            (r * self.rotation).normalize(),
        ))
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
        let Some(socket) = attach.socket(animator) else {
            continue;
        };
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

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::*;
    use crate::skeleton::{BoneDef, Skeleton};

    #[test]
    fn a_loose_prop_hangs_until_a_curve_holds_it() {
        let skeleton = Skeleton::new(
            "arm",
            vec![
                BoneDef::new("root", None, Vec3::ZERO),
                BoneDef::new("hand", Some("root"), Vec3::new(0.3, 0.8, 0.0)),
            ],
        )
        .unwrap();
        let mut animator = Animator::procedural(Arc::new(skeleton));
        animator.model[1] = Xf::from_tr(Vec3::new(0.3, 0.8, 0.0), Quat::from_rotation_x(0.7));
        let grip = Vec3::new(0.0, -0.08, 0.0);
        let handle = Vec3::new(0.0, -0.34, -0.94).normalize();
        let head = BoneAttachment {
            bone: "hand".into(),
            offset: grip + handle * 0.6,
            ..Default::default()
        }
        .with_hang(Hang {
            pivot: grip,
            axis: handle,
            toward: Vec3::NEG_Y,
            curves: vec!["grip".into()],
        });
        let fist = animator.model[1].transform_point(grip);
        let hanging = head.socket(&animator).unwrap();
        assert!(
            hanging.t.distance(fist + Vec3::NEG_Y * 0.6) < 1e-4,
            "{}",
            hanging.t
        );

        animator.curves.push(("grip".into(), 1.0));
        let held = head.socket(&animator).unwrap();
        let in_hand = animator.model[1].mul(&Xf::from_tr(head.offset, Quat::IDENTITY));
        assert!(held.t.distance(in_hand.t) < 1e-4);
        assert!(held.r.angle_between(in_hand.r) < 1e-3);
    }
}
