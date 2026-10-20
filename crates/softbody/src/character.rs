//! Collision bodies for animated characters: capsules on the bones of a `SkeletonPose` that
//! capes, skirts, hair and fur collide with (and that push curtains and rope bridges around
//! when the character walks into them).

use engine_core::prelude::*;
use engine_world::animation::SkeletonPose;

use crate::collide::LocalShape;

/// A capsule from bone `bone` to bone `to`, or a sphere around `bone` (plus `offset` in the
/// bone's space) when `to` is `None`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct BoneCapsule {
    pub bone: String,
    pub to: Option<String>,
    /// Meters.
    pub radius: f32,
    /// Offset of the sphere center (or both capsule ends) in the bone's space, meters.
    pub offset: [f32; 3],
}

impl Default for BoneCapsule {
    fn default() -> Self {
        Self {
            bone: String::new(),
            to: None,
            radius: 0.1,
            offset: [0.0; 3],
        }
    }
}

impl BoneCapsule {
    pub fn capsule(bone: &str, to: &str, radius: f32) -> Self {
        Self {
            bone: bone.into(),
            to: Some(to.into()),
            radius,
            offset: [0.0; 3],
        }
    }

    pub fn sphere(bone: &str, radius: f32, offset: [f32; 3]) -> Self {
        Self {
            bone: bone.into(),
            to: None,
            radius,
            offset,
        }
    }
}

/// Collision capsules on a character's bones. Put it on the entity with the `SkeletonPose`.
#[derive(Component, Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct BoneColliders {
    pub capsules: Vec<BoneCapsule>,
    /// Friction of cloth and hair against the body.
    pub friction: f32,
}

impl BoneColliders {
    /// Capsules for a humanoid, under either naming: the conventional names (`pelvis`,
    /// `chest`, `neck`, `head`, `upper_arm_l`, `forearm_l`, `hand_l`, `thigh_l`, `shin_l`,
    /// `foot_l`) or the animation module's (`hips`, `lower_arm_l`, `upper_leg_l`,
    /// `lower_leg_l`), and the `_r` side. Capsules naming bones the skeleton lacks are skipped.
    pub fn humanoid() -> Self {
        let mut capsules = vec![
            BoneCapsule::capsule("pelvis", "chest", 0.15),
            BoneCapsule::capsule("hips", "chest", 0.15),
            BoneCapsule::capsule("chest", "neck", 0.16),
            BoneCapsule::capsule("neck", "head", 0.06),
            BoneCapsule::sphere("head", 0.105, [0.0, 0.07, 0.0]),
        ];
        for side in ["l", "r"] {
            let b = |n: &str| format!("{n}_{side}");
            for (forearm, thigh, shin) in [
                ("forearm", "thigh", "shin"),
                ("lower_arm", "upper_leg", "lower_leg"),
            ] {
                capsules.push(BoneCapsule::capsule(&b("upper_arm"), &b(forearm), 0.055));
                capsules.push(BoneCapsule::capsule(&b(forearm), &b("hand"), 0.045));
                capsules.push(BoneCapsule::capsule(&b(thigh), &b(shin), 0.08));
                capsules.push(BoneCapsule::capsule(&b(shin), &b("foot"), 0.06));
            }
            capsules.push(BoneCapsule::capsule("chest", &b("upper_arm"), 0.07));
        }
        Self {
            capsules,
            friction: 0.4,
        }
    }

    /// Resolve every capsule to a world shape: (shape, center, rotation). Capsules naming
    /// bones the skeleton doesn't have are skipped.
    pub fn resolve(
        &self,
        transform: &GlobalTransform,
        pose: &SkeletonPose,
        out: &mut Vec<(LocalShape, DVec3, Quat)>,
    ) {
        let scale = transform.scale().max_element();
        for c in &self.capsules {
            let Some(a) = pose.index_of(&c.bone) else {
                continue;
            };
            let Some((pa, ra)) = pose.bone_world(transform, a) else {
                continue;
            };
            let offset = (ra * Vec3::from(c.offset) * scale).as_dvec3();
            let radius = c.radius * scale;
            match &c.to {
                None => out.push((LocalShape::Sphere { radius }, pa + offset, Quat::IDENTITY)),
                Some(to) => {
                    let Some(b) = pose.index_of(to) else {
                        continue;
                    };
                    let Some((pb, _)) = pose.bone_world(transform, b) else {
                        continue;
                    };
                    let axis = (pb - pa).as_vec3();
                    let len = axis.length();
                    let rot = if len > 1e-5 {
                        Quat::from_rotation_arc(Vec3::Y, axis / len)
                    } else {
                        Quat::IDENTITY
                    };
                    out.push((
                        LocalShape::Capsule {
                            half_height: len * 0.5,
                            radius,
                        },
                        (pa + pb) * 0.5 + offset,
                        rot,
                    ));
                }
            }
        }
    }
}
