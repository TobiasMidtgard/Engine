//! Ragdolls: the [`Ragdoll`] contract. Its weight says how physical a body is: 0 = animated,
//! 1 = limp; in between, the animation drives the physical body with torques (stumbles, hit
//! reactions). Built only from physics contract components (`RigidBody`, `Collider`,
//! `Joint`, `ExternalForce`), so it works with whichever physics module is installed, and a
//! body without one stays animated.
//!
//! * Bodies whose skeleton has a [`RagdollDef`] (the humanoid and every person fitted to it)
//!   get one rigid capsule per part when their weight rises above 0: placed on the animated
//!   pose and moving as it moved (a runner falls forward), jointed at the bones' pivots with
//!   hinges at elbows and knees and ball joints elsewhere, within the ranges a body bends
//!   (hard limits in the joints, soft ones by torque), with some joint friction so limbs
//!   don't flail like rope.
//! * Each frame the pose follows the bodies: part bones take their body's transform, bones
//!   between parts (collarbones) keep their pose from when the body went physical, and the
//!   rest (hands, head on the neck, toes, hair) keep animating on top.
//! * Below weight 1 every joint is pulled toward the animated pose, holding the limbs up
//!   against gravity, and the hips toward the animated hips; the closer to 1, the weaker.
//! * Back at 0 the bodies go and the pose blends back to the animation over
//!   [`BLEND_OUT_SECS`].
//! * Parts are on [`RAGDOLL_LAYER`] and don't collide with each other; while a body is
//!   physical its own character controller ignores them.
//! * A part hitting something hard reports a `GroundContact` of kind `Body` (body falls for
//!   sounds and prints in snow) and a `NoiseEvent`.

use std::collections::HashMap;
use std::sync::Arc;

use engine_core::prelude::*;
use engine_world::animation::{Ragdoll, SkeletonPose};
use engine_world::physics::{
    CharacterController, Collider, ColliderShape, CollisionLayers, ContactEvent, ContactPhase,
    ExternalForce, Joint, JointKind, PhysicsBackend, RigidBody, Velocity,
};
use engine_world::{ContactKind, Gravity, Ground, GroundContact, NoiseEvent, SurfaceMaterials};

use crate::animator::Animator;
use crate::skeleton::Skeleton;
use crate::xform::{Xf, rotation_between};

/// Collision layer of ragdoll parts (a bit of `CollisionLayers::member`). Parts collide with
/// everything except other parts.
pub const RAGDOLL_LAYER: u32 = 1 << 29;
/// How long the pose takes to blend back to the animation when the weight returns to 0, s.
pub const BLEND_OUT_SECS: f32 = 0.45;
/// Faster than this between two frames is a teleport, not motion, m/s.
const TELEPORT_SPEED: f32 = 60.0;
/// How long the animated motion a body keeps when it turns physical is averaged over, s (so
/// a body stopped on the frame it goes limp still falls the way it was going).
const MOMENTUM_SECS: f32 = 0.08;
/// Joint friction of a limp body: relative spin decays at this rate, 1/s.
const TONE: f32 = 4.0;
/// Natural frequency of the soft joint limits, rad/s.
const LIMIT_FREQUENCY: f32 = 40.0;
/// Natural frequency of the pull of each part toward its animated pose at weight 0, rad/s.
const DRIVE_FREQUENCY: f32 = 30.0;
/// Damping ratio of that pull.
const DRIVE_DAMPING: f32 = 0.9;
/// Slower than this into a hard surface makes no sound, m/s.
const IMPACT_SPEED: f32 = 1.2;

/// How a part hangs on its parent part.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum JointDef {
    /// Ball and socket. `limits` bound the part's rotation from its rest pose relative to its
    /// parent, per model axis at rest (rotation-vector components, rad): for a limb hanging
    /// down, +x swings it forward, y twists it about the vertical, +z swings it toward +X.
    Ball { limits: [[f32; 2]; 3] },
    /// Hinge about `axis` (model space at rest) within `limits` (rad, right-handed about the
    /// axis, 0 at rest).
    Hinge { axis: Vec3, limits: [f32; 2] },
}

/// One rigid part of a ragdoll: a capsule that moves one bone.
#[derive(Debug, Clone, PartialEq)]
pub struct PartDef {
    /// The bone this part moves; the part pivots about the bone's origin.
    pub bone: String,
    /// The capsule's axis, model space at rest, meters.
    pub segment: [Vec3; 2],
    pub radius: f32,
    pub mass_kg: f32,
    /// The part this one hangs from (`None` for the root part, which must come first), and
    /// how.
    pub parent: Option<usize>,
    pub joint: JointDef,
}

/// The parts and joints of a skeleton's ragdoll.
#[derive(Debug, Clone, PartialEq)]
pub struct RagdollDef {
    pub parts: Vec<PartDef>,
    /// The model axis joints twist about freely in the physics backend; their hard swing
    /// limits are across it (the vertical for upright bodies, so hips and shoulders swing
    /// within a cone about the hanging limb).
    pub twist: Vec3,
    /// Bone index of each part.
    bones: Vec<usize>,
    /// The part each bone is moved by, if any.
    part_of_bone: Vec<Option<usize>>,
    /// Bones between parts (collarbones): held at their pose from when the body went
    /// physical, so the joints and the skin agree.
    between: Vec<bool>,
    /// Rest model rotation of each part's bone, and its pivot.
    rest: Vec<Xf>,
    /// Each part's center of mass in its body's frame.
    center: Vec<Vec3>,
    /// Mass of each part with everything hanging from it, kg.
    subtree_mass: Vec<f32>,
    /// Each part's moments of inertia about its center (along and across its capsule, kg·m²)
    /// and its capsule axis in its body's frame.
    inertia: Vec<(f32, f32, Vec3)>,
    /// Joints each part takes torques from (its own and its children's).
    joint_count: Vec<f32>,
    /// Parts hanging from each part.
    children: Vec<Vec<usize>>,
    /// Body frame at rest: turns +X onto `twist`.
    frame: Quat,
}

impl RagdollDef {
    /// A ragdoll from parts for `skeleton`. Fails when a bone is missing or a parent doesn't
    /// come before its child.
    pub fn new(skeleton: &Skeleton, parts: Vec<PartDef>, twist: Vec3) -> Result<Self, String> {
        if parts.is_empty() || parts[0].parent.is_some() {
            return Err("the first ragdoll part must be the root (no parent)".into());
        }
        let mut bones = Vec::with_capacity(parts.len());
        let mut part_of_bone = vec![None; skeleton.len()];
        for (i, p) in parts.iter().enumerate() {
            let b = skeleton
                .index_of(&p.bone)
                .ok_or_else(|| format!("skeleton {} has no bone {}", skeleton.name, p.bone))?;
            if part_of_bone[b].is_some() {
                return Err(format!("two ragdoll parts move bone {}", p.bone));
            }
            if i > 0 && p.parent.is_none_or(|q| q >= i) {
                return Err(format!(
                    "ragdoll part {} needs a parent part listed before it",
                    p.bone
                ));
            }
            part_of_bone[b] = Some(i);
            bones.push(b);
        }
        // Bones with a part above and a part below them.
        let n = skeleton.len();
        let mut has_part_above = vec![false; n];
        for b in 0..n {
            if let Some(p) = skeleton.parents[b] {
                let p = p as usize;
                has_part_above[b] = has_part_above[p] || part_of_bone[p].is_some();
            }
        }
        let mut has_part_below = vec![false; n];
        for b in (0..n).rev() {
            if let Some(p) = skeleton.parents[b] {
                let p = p as usize;
                has_part_below[p] |= has_part_below[b] || part_of_bone[b].is_some();
            }
        }
        let between = (0..n)
            .map(|b| part_of_bone[b].is_none() && has_part_above[b] && has_part_below[b])
            .collect();
        let frame = rotation_between(Vec3::X, twist.normalize_or(Vec3::NEG_Y));
        let rest: Vec<Xf> = bones.iter().map(|&b| skeleton.rest_model[b]).collect();
        let center = parts
            .iter()
            .zip(&rest)
            .map(|(p, r)| frame.inverse() * ((p.segment[0] + p.segment[1]) * 0.5 - r.t))
            .collect();
        let mut children = vec![Vec::new(); parts.len()];
        for (i, p) in parts.iter().enumerate() {
            if let Some(q) = p.parent {
                children[q].push(i);
            }
        }
        let mut def = Self {
            parts,
            twist,
            bones,
            part_of_bone,
            between,
            rest,
            center,
            subtree_mass: Vec::new(),
            inertia: Vec::new(),
            joint_count: Vec::new(),
            children,
            frame,
        };
        for i in 0..def.parts.len() {
            let below = def.subtree(i);
            def.subtree_mass
                .push(below.iter().map(|&j| def.parts[j].mass_kg).sum());
            // A solid capsule as a cylinder with its caps folded in.
            let p = &def.parts[i];
            let axis = def.frame.inverse() * (p.segment[1] - p.segment[0]);
            let (r, len) = (p.radius, axis.length() + p.radius * 4.0 / 3.0);
            def.inertia.push((
                p.mass_kg * r * r * 0.5,
                p.mass_kg * (3.0 * r * r + len * len) / 12.0,
                axis.normalize_or(Vec3::Y),
            ));
            def.joint_count
                .push((def.children[i].len() + usize::from(p.parent.is_some())).max(1) as f32);
        }
        Ok(def)
    }

    /// The ragdoll for a skeleton, by its family (the name before any `.` or `@`): the
    /// humanoid and people fitted to it, dogs of any size, and horses (the imported horse
    /// too).
    pub fn for_skeleton(skeleton: &Skeleton) -> Option<Self> {
        let family = skeleton.name.split(['.', '@']).next().unwrap_or_default();
        let def = match family {
            "humanoid" => Self::humanoid(skeleton),
            "dog" => Self::dog(skeleton),
            "horse" => Self::horse(skeleton),
            _ => return None,
        };
        def.map_err(|e| tracing::warn!("no ragdoll for {}: {e}", skeleton.name))
            .ok()
    }

    /// A person: pelvis, belly, chest, neck and head, upper arms, forearms with the hands,
    /// thighs, shins and feet, with segment masses from biomechanics tables (Winter) for a
    /// 75 kg adult scaled by size.
    pub fn humanoid(s: &Skeleton) -> Result<Self, String> {
        let at = bone_at(s);
        let rest_rot = |name: &str| {
            s.index_of(name)
                .map_or(Quat::IDENTITY, |i| s.rest_model[i].r)
        };
        let (hips, spine, chest, neck, head) = (
            at("hips")?,
            at("spine")?,
            at("chest")?,
            at("neck")?,
            at("head")?,
        );
        let ankle = 0.5 * (at("foot_l")?.y + at("foot_r")?.y);
        // Size relative to the generated person (head bone 1.52 m above the ankles).
        let k = ((head.y - ankle) / 1.52).clamp(0.2, 5.0);
        let kg = 75.0 * k.powi(3);
        let across = |c: Vec3, half: f32| [c - Vec3::X * half, c + Vec3::X * half];
        let hip_width = (at("upper_leg_r")?.x - at("upper_leg_l")?.x).abs();
        let mut parts = vec![
            PartDef {
                bone: "hips".into(),
                segment: across(hips - Vec3::Y * 0.02 * k, hip_width * 0.38),
                radius: 0.105 * k,
                mass_kg: 0.142 * kg,
                parent: None,
                joint: ball([0.0; 2], [0.0; 2], [0.0; 2]),
            },
            PartDef {
                bone: "spine".into(),
                segment: across((spine + chest) * 0.5, 0.05 * k),
                radius: 0.105 * k,
                mass_kg: 0.139 * kg,
                parent: Some(0),
                joint: ball([-0.5, 0.25], [-0.35, 0.35], [-0.3, 0.3]),
            },
            PartDef {
                bone: "chest".into(),
                segment: across(chest.lerp(neck, 0.45), 0.075 * k),
                radius: 0.125 * k,
                mass_kg: 0.216 * kg,
                parent: Some(1),
                joint: ball([-0.4, 0.25], [-0.35, 0.35], [-0.25, 0.25]),
            },
            PartDef {
                bone: "neck".into(),
                segment: [
                    neck + Vec3::Y * 0.05 * k,
                    head + Vec3::new(0.0, 0.085, -0.01) * k,
                ],
                radius: 0.09 * k,
                mass_kg: 0.081 * kg,
                parent: Some(2),
                joint: ball([-0.7, 0.5], [-1.0, 1.0], [-0.5, 0.5]),
            },
        ];
        for (side, sign) in [("l", -1.0f32), ("r", 1.0)] {
            let bone = |b: &str| format!("{b}_{side}");
            let (ua, la, hand) = (
                at(&bone("upper_arm"))?,
                at(&bone("lower_arm"))?,
                at(&bone("hand"))?,
            );
            let (ul, ll, foot, toe) = (
                at(&bone("upper_leg"))?,
                at(&bone("lower_leg"))?,
                at(&bone("foot"))?,
                at(&bone("toe"))?,
            );
            // Mirror a [lo, hi] range for the left side.
            let side_range = |lo: f32, hi: f32| if sign > 0.0 { [lo, hi] } else { [-hi, -lo] };
            let upper_arm = parts.len();
            parts.push(PartDef {
                bone: bone("upper_arm"),
                segment: [ua, la],
                radius: 0.048 * k,
                mass_kg: 0.028 * kg,
                parent: Some(2),
                joint: ball([-0.9, 2.6], [-1.2, 1.2], side_range(-0.5, 2.3)),
            });
            parts.push(PartDef {
                bone: bone("lower_arm"),
                segment: [la, hand + (hand - la).normalize_or(Vec3::NEG_Y) * 0.085 * k],
                radius: 0.04 * k,
                mass_kg: 0.022 * kg,
                parent: Some(upper_arm),
                joint: JointDef::Hinge {
                    axis: rest_rot(&bone("upper_arm")) * Vec3::X,
                    limits: [-0.05, 2.5],
                },
            });
            let upper_leg = parts.len();
            parts.push(PartDef {
                bone: bone("upper_leg"),
                segment: [ul, ll],
                radius: 0.072 * k,
                mass_kg: 0.1 * kg,
                parent: Some(0),
                joint: ball([-0.45, 1.9], [-0.6, 0.6], side_range(-0.3, 0.75)),
            });
            parts.push(PartDef {
                bone: bone("lower_leg"),
                segment: [ll, foot],
                radius: 0.052 * k,
                mass_kg: 0.0465 * kg,
                parent: Some(upper_leg),
                joint: JointDef::Hinge {
                    axis: rest_rot(&bone("upper_leg")) * Vec3::X,
                    limits: [-2.4, 0.03],
                },
            });
            // The sole on the ground (y = 0 at rest).
            let r = 0.035 * k;
            parts.push(PartDef {
                bone: bone("foot"),
                segment: [
                    Vec3::new(foot.x, r, foot.z + 0.02 * k),
                    Vec3::new(toe.x, r, toe.z - 0.02 * k),
                ],
                radius: r,
                mass_kg: 0.0145 * kg,
                parent: Some(upper_leg + 1),
                joint: ball([-0.7, 0.35], [-0.25, 0.25], [-0.3, 0.3]),
            });
        }
        Self::new(s, parts, Vec3::NEG_Y)
    }

    /// A dog: chest, loin, neck and head, and legs of upper arm or thigh, forearm or shin,
    /// and cannon with the paw. 30 kg for a 56 cm Labrador, scaled by size.
    pub fn dog(s: &Skeleton) -> Result<Self, String> {
        let at = bone_at(s);
        let (body, loin, neck, head, jaw, tail) = (
            at("body")?,
            at("loin")?,
            at("neck_1")?,
            at("head")?,
            at("jaw")?,
            at("tail_1")?,
        );
        let k = (body.y / 0.44).clamp(0.2, 5.0);
        let mut parts = vec![
            PartDef {
                bone: "body".into(),
                segment: [
                    Vec3::new(0.0, body.y - 0.03 * k, neck.z + 0.02 * k),
                    Vec3::new(0.0, body.y - 0.03 * k, loin.z - 0.12 * k),
                ],
                radius: 0.115 * k,
                mass_kg: 0.36,
                parent: None,
                joint: ball([0.0; 2], [0.0; 2], [0.0; 2]),
            },
            PartDef {
                bone: "loin".into(),
                segment: [
                    Vec3::new(0.0, loin.y - 0.03 * k, loin.z - 0.06 * k),
                    Vec3::new(0.0, loin.y - 0.02 * k, tail.z - 0.07 * k),
                ],
                radius: 0.085 * k,
                mass_kg: 0.2,
                parent: Some(0),
                joint: ball([-0.45, 0.45], [-0.35, 0.35], [-0.25, 0.25]),
            },
            PartDef {
                bone: "neck_1".into(),
                segment: [neck, neck.lerp(head, 0.8)],
                radius: 0.055 * k,
                mass_kg: 0.05,
                parent: Some(0),
                joint: ball([-1.0, 0.6], [-0.9, 0.9], [-0.4, 0.4]),
            },
            PartDef {
                bone: "head".into(),
                segment: [
                    head + Vec3::new(0.0, 0.01, 0.03) * k,
                    jaw + Vec3::new(0.0, 0.0, -0.1) * k,
                ],
                radius: 0.06 * k,
                mass_kg: 0.07,
                parent: Some(2),
                joint: ball([-0.6, 0.5], [-0.6, 0.6], [-0.4, 0.4]),
            },
        ];
        let legs = Legs {
            radius: [[0.035, 0.025, 0.02], [0.045, 0.03, 0.022]].map(|r| r.map(|r| r * k)),
            mass: [[0.028, 0.014, 0.008], [0.045, 0.018, 0.009]],
            parent: [0, 1],
            foot: "paw",
        };
        legs.add(s, &mut parts)?;
        Self::with_mass(s, parts, 30.0 * k.powi(3))
    }

    /// A horse: barrel, neck in two, head, and legs of upper arm or thigh, forearm or gaskin,
    /// and cannon with the pastern and hoof. 500 kg at the generated horse's size.
    pub fn horse(s: &Skeleton) -> Result<Self, String> {
        let at = bone_at(s);
        let (body, neck_1, neck_2, head, tail) = (
            at("body")?,
            at("neck_1")?,
            at("neck_2")?,
            at("head")?,
            at("tail_1")?,
        );
        let k = (body.y / 1.25).clamp(0.2, 5.0);
        let mut parts = vec![
            PartDef {
                bone: "body".into(),
                segment: [
                    Vec3::new(0.0, body.y - 0.04 * k, neck_1.z * 0.75),
                    Vec3::new(0.0, body.y - 0.02 * k, tail.z * 0.62),
                ],
                radius: 0.33 * k,
                mass_kg: 0.62,
                parent: None,
                joint: ball([0.0; 2], [0.0; 2], [0.0; 2]),
            },
            PartDef {
                bone: "neck_1".into(),
                segment: [neck_1, neck_2],
                radius: 0.17 * k,
                mass_kg: 0.05,
                parent: Some(0),
                joint: ball([-0.9, 0.5], [-0.7, 0.7], [-0.3, 0.3]),
            },
            PartDef {
                bone: "neck_2".into(),
                segment: [neck_2, head],
                radius: 0.12 * k,
                mass_kg: 0.025,
                parent: Some(1),
                joint: ball([-0.6, 0.5], [-0.6, 0.6], [-0.3, 0.3]),
            },
            PartDef {
                bone: "head".into(),
                segment: [
                    head + Vec3::new(0.0, -0.02, 0.02) * k,
                    head + Vec3::new(0.0, -0.33, -0.3) * k,
                ],
                radius: 0.1 * k,
                mass_kg: 0.045,
                parent: Some(2),
                joint: ball([-0.6, 0.6], [-0.5, 0.5], [-0.3, 0.3]),
            },
        ];
        let legs = Legs {
            radius: [[0.09, 0.07, 0.045], [0.11, 0.075, 0.05]].map(|r| r.map(|r| r * k)),
            mass: [[0.025, 0.016, 0.008], [0.045, 0.018, 0.01]],
            parent: [0, 0],
            foot: "hoof",
        };
        legs.add(s, &mut parts)?;
        Self::with_mass(s, parts, 500.0 * k.powi(3))
    }

    /// A ragdoll whose part masses are shares of `kg`.
    fn with_mass(s: &Skeleton, mut parts: Vec<PartDef>, kg: f32) -> Result<Self, String> {
        let total: f32 = parts.iter().map(|p| p.mass_kg).sum();
        for p in &mut parts {
            p.mass_kg *= kg / total;
        }
        Self::new(s, parts, Vec3::NEG_Y)
    }

    /// Total mass, kg.
    pub fn mass(&self) -> f32 {
        self.subtree_mass.first().copied().unwrap_or(0.0)
    }

    /// Part `i` and every part hanging from it.
    fn subtree(&self, i: usize) -> Vec<usize> {
        let mut out = vec![i];
        let mut k = 0;
        while k < out.len() {
            out.extend(self.children[out[k]].iter().copied());
            k += 1;
        }
        out
    }

    /// Inverse inertia tensor (world axes) of part `i` with body rotation `rot`, at entity
    /// scale `scale`.
    fn inverse_inertia(&self, i: usize, rot: Quat, scale: f32) -> Mat3 {
        let (along, across, axis) = self.inertia[i];
        let s5 = scale.powi(5);
        let a = rot * axis;
        let aa = Mat3::from_cols(a * a.x, a * a.y, a * a.z);
        (Mat3::IDENTITY - aa) * (1.0 / (across * s5)) + aa * (1.0 / (along * s5))
    }

    /// Body pose (world) of part `i` for bone model transform `m`, on an entity at `gt`.
    fn body_pose(&self, i: usize, m: &Xf, gt: &GlobalTransform) -> (DVec3, Quat) {
        let e = gt.rotation();
        let delta = m.r * self.rest[i].r.inverse();
        (
            gt.translation() + (e * (m.t * gt.scale())).as_dvec3(),
            (e * delta * self.frame).normalize(),
        )
    }

    /// The bone's model transform for part `i`'s body at `pos`, `rot` (world).
    fn bone_model(&self, i: usize, pos: DVec3, rot: Quat, gt: &GlobalTransform) -> Xf {
        let inv = gt.rotation().inverse();
        let scale = gt.scale().max_element().max(1e-6);
        Xf::from_tr(
            inv * ((pos - gt.translation()).as_vec3() / scale),
            (inv * rot * self.frame.inverse() * self.rest[i].r).normalize(),
        )
    }

    /// Rotation of each part from its rest pose (model frame) for body rotations `rot`.
    fn deltas(&self, rot: &[Quat], gt: &GlobalTransform) -> Vec<Quat> {
        let inv = gt.rotation().inverse();
        rot.iter()
            .map(|r| (inv * *r * self.frame.inverse()).normalize())
            .collect()
    }

    /// The physics collider of part `i` at entity scale `scale`.
    fn collider(&self, i: usize, scale: f32, surface: engine_world::SurfaceId) -> Collider {
        let p = &self.parts[i];
        let to_local = |v: Vec3| self.frame.inverse() * v;
        let axis = to_local(p.segment[1] - p.segment[0]);
        let rotation = rotation_between(Vec3::Y, axis.normalize_or(Vec3::Y));
        Collider {
            shape: ColliderShape::Compound(vec![(
                self.center[i] * scale,
                rotation,
                ColliderShape::Capsule {
                    radius: p.radius * scale,
                    half_height: axis.length() * 0.5 * scale,
                },
            )]),
            surface,
            density: 1050.0,
            sensor: false,
            layers: CollisionLayers {
                member: RAGDOLL_LAYER,
                collides_with: !RAGDOLL_LAYER,
            },
        }
    }

    /// The physics joint of part `i` (which has a parent) for its anchor on the parent.
    fn joint(&self, i: usize, parent: Entity, child: Entity, anchor: Vec3) -> Joint {
        let kind = match self.parts[i].joint {
            JointDef::Ball { limits } => {
                // The backend limits swings across the twist axis: keep that a backstop
                // beyond the soft limits (it can't tell forward from back).
                let reach = [limits[0], limits[2]]
                    .iter()
                    .flatten()
                    .fold(0.0f32, |m, v| m.max(v.abs()));
                JointKind::Spherical {
                    cone_limit: (reach + 0.3 < 2.8).then_some(reach + 0.3),
                }
            }
            JointDef::Hinge { axis, limits } => JointKind::Revolute {
                axis: self.frame.inverse() * axis,
                limits: Some(limits),
                motor: None,
            },
        };
        Joint {
            kind,
            body_a: parent,
            body_b: Some(child),
            anchor_a: anchor,
            anchor_b: DVec3::ZERO,
            break_force: None,
            collide_connected: false,
        }
    }
}

/// Where `name` sits in `s`'s rest pose, or an error naming the missing bone.
fn bone_at(s: &Skeleton) -> impl Fn(&str) -> Result<Vec3, String> + '_ {
    move |name: &str| {
        s.index_of(name)
            .map(|i| s.rest_model[i].t)
            .ok_or_else(|| format!("skeleton {} has no bone {name}", s.name))
    }
}

fn ball(x: [f32; 2], y: [f32; 2], z: [f32; 2]) -> JointDef {
    JointDef::Ball { limits: [x, y, z] }
}

/// A four-legged animal's legs (front, then hind): radius and mass share of the upper,
/// lower and cannon parts, the trunk part each pair hangs from, and the foot bone's name.
struct Legs {
    radius: [[f32; 3]; 2],
    mass: [[f32; 3]; 2],
    parent: [usize; 2],
    foot: &'static str,
}

impl Legs {
    /// Swing at the shoulder or hip; the elbow and hock fold forward, the carpus and stifle
    /// back. The cannon reaches the ground at rest.
    fn add(&self, s: &Skeleton, parts: &mut Vec<PartDef>) -> Result<(), String> {
        let at = bone_at(s);
        for (side, sign) in [("l", -1.0f32), ("r", 1.0)] {
            let side_range = |lo: f32, hi: f32| if sign > 0.0 { [lo, hi] } else { [-hi, -lo] };
            for (pair, prefix) in ["front", "hind"].into_iter().enumerate() {
                let bone = |b: &str| format!("{prefix}_{b}_{side}");
                let (upper, lower, cannon, foot) = (
                    at(&bone("upper"))?,
                    at(&bone("lower"))?,
                    at(&bone("cannon"))?,
                    at(&bone(self.foot))?,
                );
                let [r_upper, r_lower, r_cannon] = self.radius[pair];
                let [m_upper, m_lower, m_cannon] = self.mass[pair];
                let (middle, low) = if pair == 0 {
                    ([-0.3, 1.6], [-2.0, 0.2])
                } else {
                    ([-1.4, 0.3], [-0.3, 1.8])
                };
                let first = parts.len();
                parts.push(PartDef {
                    bone: bone("upper"),
                    segment: [upper, lower],
                    radius: r_upper,
                    mass_kg: m_upper,
                    parent: Some(self.parent[pair]),
                    joint: ball([-0.8, 1.0], [-0.3, 0.3], side_range(-0.2, 0.5)),
                });
                parts.push(PartDef {
                    bone: bone("lower"),
                    segment: [lower, cannon],
                    radius: r_lower,
                    mass_kg: m_lower,
                    parent: Some(first),
                    joint: JointDef::Hinge {
                        axis: Vec3::X,
                        limits: middle,
                    },
                });
                parts.push(PartDef {
                    bone: bone("cannon"),
                    segment: [cannon, Vec3::new(foot.x, r_cannon, foot.z)],
                    radius: r_cannon,
                    mass_kg: m_cannon,
                    parent: Some(first + 1),
                    joint: JointDef::Hinge {
                        axis: Vec3::X,
                        limits: low,
                    },
                });
            }
        }
        Ok(())
    }
}

/// Ragdolls built so far, by skeleton name (`None`: that skeleton has none).
#[derive(Resource, Default)]
pub struct RagdollDefs(HashMap<String, Option<Arc<RagdollDef>>>);

impl RagdollDefs {
    pub fn get(&mut self, skeleton: &Skeleton) -> Option<Arc<RagdollDef>> {
        self.0
            .entry(skeleton.name.clone())
            .or_insert_with(|| RagdollDef::for_skeleton(skeleton).map(Arc::new))
            .clone()
    }
}

/// A rigid part of a ragdoll, on the part's physics body. A ray that hits it can push the
/// body with `ApplyImpulse` on this entity.
#[derive(Component, Debug, Clone, PartialEq)]
pub struct RagdollPart {
    pub owner: Entity,
    /// Index into the owner's [`RagdollDef::parts`].
    pub index: usize,
    pub bone: String,
}

/// A joint between two parts of a ragdoll, on the joint's entity.
#[derive(Component, Debug, Clone, Copy, PartialEq)]
pub struct RagdollJoint {
    pub owner: Entity,
}

/// The ragdoll of an animated body whose skeleton has one: its parts while physical, and
/// where they were last frame (so a body that turns physical keeps its motion).
#[derive(Component)]
pub struct RagdollRig {
    pub def: Arc<RagdollDef>,
    skeleton: String,
    /// Part bodies (in the def's order) while physical; empty while animated.
    pub parts: Vec<Entity>,
    joints: Vec<Entity>,
    /// Local transforms of the bones between parts when the body turned physical.
    held: Vec<Xf>,
    /// Where the animation put each part's body last frame, and that frame.
    last: Vec<(DVec3, Quat)>,
    last_frame: Option<u64>,
    /// Each part's animated velocity, averaged over [`MOMENTUM_SECS`].
    momentum: Vec<Velocity>,
    /// The frame the parts were spawned (they exist from the next one).
    spawned: u64,
    /// Pose to blend back from after the body stopped being physical, and seconds since.
    blend: Option<(Vec<Xf>, f32)>,
    /// Whether this took the ragdoll layer out of the owner's character controller.
    took_controller_bit: bool,
}

impl RagdollRig {
    pub fn new(def: Arc<RagdollDef>, skeleton: &str) -> Self {
        Self {
            def,
            skeleton: skeleton.into(),
            parts: Vec::new(),
            joints: Vec::new(),
            held: Vec::new(),
            last: Vec::new(),
            last_frame: None,
            momentum: Vec::new(),
            spawned: 0,
            blend: None,
            took_controller_bit: false,
        }
    }

    /// Whether the body is physical now (its parts exist or are being spawned).
    pub fn is_physical(&self) -> bool {
        !self.parts.is_empty()
    }

    /// Whether the pose is still blending back to the animation.
    pub fn is_blending_back(&self) -> bool {
        self.blend.is_some()
    }

    pub(crate) fn follow_shift(&mut self, s: &engine_world::WorldShift) {
        for (p, r) in &mut self.last {
            *p = s.point(*p);
            *r = s.orientation(*r);
        }
    }
}

/// Give animated bodies whose skeleton has a ragdoll a [`RagdollRig`] (again when their
/// skeleton changes).
pub(crate) fn add_rigs(
    mut commands: Commands,
    mut defs: ResMut<RagdollDefs>,
    q: Query<(Entity, &Animator, Option<&RagdollRig>), Changed<Animator>>,
) {
    for (entity, animator, rig) in &q {
        if rig.is_some_and(|r| r.skeleton == animator.skeleton.name) {
            continue;
        }
        match defs.get(&animator.skeleton) {
            Some(def) => {
                commands
                    .entity(entity)
                    .insert(RagdollRig::new(def, &animator.skeleton.name));
            }
            None if rig.is_some() => {
                commands.entity(entity).remove::<RagdollRig>();
            }
            None => {}
        }
    }
}

type Owners<'w, 's> = Query<
    'w,
    's,
    (
        Entity,
        &'static mut Animator,
        &'static mut RagdollRig,
        &'static GlobalTransform,
        &'static SkeletonPose,
        Option<&'static Ragdoll>,
        Option<&'static Name>,
        Option<&'static mut CharacterController>,
    ),
    Without<RagdollPart>,
>;

type Parts<'w, 's> = Query<
    'w,
    's,
    (
        &'static Transform,
        &'static Velocity,
        &'static mut ExternalForce,
    ),
    With<RagdollPart>,
>;

/// Turn bodies physical and back as their weight changes, pose them from their parts, and
/// drive the parts toward the animation.
#[allow(clippy::too_many_arguments)]
pub(crate) fn ragdoll_system(
    mut commands: Commands,
    time: Res<Time>,
    backend: Option<Res<PhysicsBackend>>,
    gravity: Res<Gravity>,
    surfaces: Res<SurfaceMaterials>,
    mut owners: Owners,
    mut parts: Parts,
) {
    let frame = time.frame_count();
    let dt = time.delta_secs();
    let flesh = surfaces.id_or_default("flesh");
    for (entity, mut animator, mut rig, gt, published, ragdoll, name, controller) in &mut owners {
        let rig = &mut *rig;
        let def = rig.def.clone();
        let weight = ragdoll.map_or(0.0, |r| r.weight.clamp(0.0, 1.0));
        animator.refresh_model();
        let animated: Vec<(DVec3, Quat)> = (0..def.parts.len())
            .map(|i| def.body_pose(i, &animator.model[def.bones[i]], gt))
            .collect();
        if weight > 0.0 && !rig.is_physical() && backend.is_some() {
            spawn_parts(
                &mut commands,
                rig,
                entity,
                &animator,
                &animated,
                gt,
                name,
                frame,
                flesh,
            );
            if let Some(mut c) = controller {
                rig.took_controller_bit = c.layers.collides_with & RAGDOLL_LAYER != 0;
                c.layers.collides_with &= !RAGDOLL_LAYER;
            }
        } else if weight <= 0.0 && rig.is_physical() {
            // Blend back from the pose shown last frame.
            if published.bones.len() == animator.skeleton.len() && frame > rig.spawned + 1 {
                let snapshot = published.bones.iter().map(Xf::from_bone_pose).collect();
                rig.blend = Some((snapshot, 0.0));
            }
            for e in rig.parts.drain(..).chain(rig.joints.drain(..)) {
                commands.entity(e).try_despawn();
            }
            if let Some(mut c) = controller
                && rig.took_controller_bit
            {
                c.layers.collides_with |= RAGDOLL_LAYER;
            }
            rig.took_controller_bit = false;
        }

        if rig.is_physical() && frame > rig.spawned {
            follow_parts(
                &def,
                rig,
                &mut animator,
                (gt, &animated),
                weight,
                (frame, dt),
                &gravity,
                &mut parts,
            );
        } else if let Some((snapshot, t)) = &mut rig.blend {
            *t += dt;
            let w = (*t / BLEND_OUT_SECS).clamp(0.0, 1.0);
            let w = w * w * (3.0 - 2.0 * w);
            let skeleton = animator.skeleton.clone();
            let Animator { local, model, .. } = &mut *animator;
            for b in 0..skeleton.len() {
                model[b] = snapshot[b].lerp(&model[b], w);
                local[b] = match skeleton.parents[b] {
                    Some(p) => model[p as usize].inverse().mul(&model[b]),
                    None => model[b],
                };
            }
            if *t >= BLEND_OUT_SECS {
                rig.blend = None;
            }
        }
        // How the animated parts move (teleports don't count).
        let a = crate::xform::smoothing(dt, MOMENTUM_SECS);
        if rig.last_frame.is_some_and(|f| f + 1 == frame) && dt > 0.0 {
            rig.momentum.resize(animated.len(), Velocity::default());
            for ((m, &(p, r)), &(p0, r0)) in rig.momentum.iter_mut().zip(&animated).zip(&rig.last) {
                let linear = ((p - p0) / dt as f64).as_vec3();
                if linear.length() < TELEPORT_SPEED {
                    m.linear = m.linear.lerp(linear, a);
                    m.angular = m.angular.lerp(rotation_vector(r * r0.inverse()) / dt, a);
                }
            }
        } else {
            rig.momentum.clear();
        }
        rig.last = animated;
        rig.last_frame = Some(frame);
    }
}

/// Spawn the part bodies and joints on the animated pose, moving as it moved.
#[allow(clippy::too_many_arguments)]
fn spawn_parts(
    commands: &mut Commands,
    rig: &mut RagdollRig,
    owner: Entity,
    animator: &Animator,
    animated: &[(DVec3, Quat)],
    gt: &GlobalTransform,
    name: Option<&Name>,
    frame: u64,
    surface: engine_world::SurfaceId,
) {
    let def = rig.def.clone();
    let scale = gt.scale().max_element();
    let label = name.map_or_else(|| format!("{owner}"), |n| n.as_str().to_string());
    rig.parts = (0..def.parts.len())
        .map(|i| {
            let (pos, rot) = animated[i];
            let velocity = rig.momentum.get(i).copied().unwrap_or_default();
            let t = Transform {
                translation: pos,
                rotation: rot,
                ..Default::default()
            };
            let part = &def.parts[i];
            commands
                .spawn((
                    Name::new(format!("{label} ragdoll {}", part.bone)),
                    t,
                    GlobalTransform::from(t),
                    RigidBody {
                        mass_kg: Some(part.mass_kg * scale.powi(3)),
                        linear_damping: 0.02,
                        angular_damping: 0.1,
                        ccd: true,
                        ..RigidBody::dynamic()
                    },
                    def.collider(i, scale, surface),
                    velocity,
                    ExternalForce::default(),
                    RagdollPart {
                        owner,
                        index: i,
                        bone: part.bone.clone(),
                    },
                ))
                .id()
        })
        .collect();
    rig.joints = (1..def.parts.len())
        .filter_map(|i| {
            let p = def.parts[i].parent?;
            let (pp, pr) = animated[p];
            let anchor = pr.inverse() * (animated[i].0 - pp).as_vec3();
            Some(
                commands
                    .spawn((
                        Name::new(format!("{label} ragdoll joint {}", def.parts[i].bone)),
                        def.joint(i, rig.parts[p], rig.parts[i], anchor),
                        RagdollJoint { owner },
                    ))
                    .id(),
            )
        })
        .collect();
    rig.held = animator.local.clone();
    rig.blend = None;
    rig.spawned = frame;
}

/// Rotation vector (axis × angle, shortest way round) of a rotation.
fn rotation_vector(q: Quat) -> Vec3 {
    let q = if q.w < 0.0 { -q } else { q };
    q.to_scaled_axis()
}

/// Pose the skeleton from the part bodies, and set the forces that keep the joints in range
/// and pull the parts toward the animated pose.
#[allow(clippy::too_many_arguments)]
fn follow_parts(
    def: &RagdollDef,
    rig: &RagdollRig,
    animator: &mut Animator,
    (gt, animated): (&GlobalTransform, &[(DVec3, Quat)]),
    weight: f32,
    (frame, dt): (u64, f32),
    gravity: &Gravity,
    parts: &mut Parts,
) {
    let n = def.parts.len();
    let mut pos = Vec::with_capacity(n);
    let mut rot = Vec::with_capacity(n);
    let mut spin = Vec::with_capacity(n);
    let mut vel = Vec::with_capacity(n);
    for &e in &rig.parts {
        let Ok((t, v, _)) = parts.get(e) else {
            return;
        };
        pos.push(t.translation);
        rot.push(t.rotation);
        spin.push(v.angular);
        vel.push(v.linear);
    }

    // The pose: part bones from their bodies, bones between parts as they were, the rest
    // animated.
    let skeleton = animator.skeleton.clone();
    let Animator { local, model, .. } = animator;
    for b in 0..skeleton.len() {
        let parent = skeleton.parents[b].map_or(Xf::IDENTITY, |p| model[p as usize]);
        if let Some(i) = def.part_of_bone[b] {
            let mut m = def.bone_model(i, pos[i], rot[i], gt);
            m.s = model[b].s;
            local[b] = parent.inverse().mul(&m);
            model[b] = m;
        } else {
            if def.between[b]
                && let Some(h) = rig.held.get(b)
            {
                local[b] = *h;
            }
            model[b] = parent.mul(&local[b]);
        }
    }

    // Joint torques keep the joints in range and add friction. Their gains are accelerations
    // scaled by the inertia a torque actually turns: a part between ball joints spins about
    // its own center against only its own inertia, shared among everything that pushes it.
    let drive = 1.0 - weight;
    // A spring and damper set once a frame (semi-implicit) stay stable while
    // (w·dt)² + 2·c·dt < 4: keep w·dt under 1 and c·dt under 1.4.
    let cap = |w: f32| w.min(1.0 / dt.max(1e-4));
    let max_damping = 1.4 / dt.max(1e-4);
    let w_limit = cap(LIMIT_FREQUENCY);
    let delta = def.deltas(&rot, gt);
    let e_rot = gt.rotation();
    let scale = gt.scale().max_element();
    let pushers = |i: usize| def.joint_count[i] + if drive > 0.0 { 1.0 } else { 0.0 };
    let inverse: Vec<Mat3> = (0..n)
        .map(|i| def.inverse_inertia(i, rot[i], scale) * pushers(i))
        .collect();
    let mut torque = vec![Vec3::ZERO; n];
    let mut force = vec![Vec3::ZERO; n];
    for i in 1..n {
        let p = def.parts[i].parent.unwrap_or(0);
        let inertia = (inverse[i] + inverse[p]).inverse();
        let mut accel = Vec3::ZERO;
        let mut damping = TONE;
        if let JointDef::Ball { limits } = def.parts[i].joint {
            let v = rotation_vector(delta[p].inverse() * delta[i]);
            let excess = Vec3::new(
                v.x - v.x.clamp(limits[0][0], limits[0][1]),
                v.y - v.y.clamp(limits[1][0], limits[1][1]),
                v.z - v.z.clamp(limits[2][0], limits[2][1]),
            );
            if excess != Vec3::ZERO {
                accel -= e_rot * delta[p] * excess * w_limit * w_limit;
                damping += w_limit;
            }
        }
        let t = inertia * (accel - damping.min(max_damping) * (spin[i] - spin[p]));
        torque[i] += t;
        torque[p] -= t;
    }

    // Below weight 1 each part is pulled toward where the animation has it and held up
    // against gravity: fully until the body is half limp, then less and less.
    if drive > 0.0 {
        let support = (drive * 2.0).min(1.0);
        let w = cap(DRIVE_FREQUENCY) * drive;
        let c = (2.0 * DRIVE_DAMPING * w).min(max_damping);
        let moving = rig.last_frame.is_some_and(|f| f + 1 == frame) && dt > 0.0;
        for i in 0..n {
            let (target, aim) = animated[i];
            let (target_velocity, target_spin) = match rig.last.get(i) {
                Some(&(p0, r0)) if moving => (
                    ((target - p0) / dt as f64).as_vec3(),
                    rotation_vector(aim * r0.inverse()) / dt,
                ),
                _ => (Vec3::ZERO, Vec3::ZERO),
            };
            let mass = def.parts[i].mass_kg * scale.powi(3);
            force[i] += mass
                * ((target - pos[i]).as_vec3() * w * w + (target_velocity - vel[i]) * c
                    - gravity.at(pos[i]) * support);
            let inertia = (def.inverse_inertia(i, rot[i], scale) * pushers(i)).inverse();
            torque[i] += inertia
                * (rotation_vector(aim * rot[i].inverse()) * w * w + (target_spin - spin[i]) * c);
        }
    }
    for (i, &e) in rig.parts.iter().enumerate() {
        if let Ok((_, _, mut f)) = parts.get_mut(e) {
            let new = ExternalForce {
                force: force[i],
                torque: torque[i],
                at: None,
            };
            if *f != new {
                *f = new;
            }
        }
    }
}

/// Despawn parts and joints whose body is gone or no longer physical.
pub(crate) fn despawn_orphan_parts(
    mut commands: Commands,
    parts: Query<(Entity, &RagdollPart)>,
    joints: Query<(Entity, &RagdollJoint)>,
    rigs: Query<&RagdollRig>,
) {
    for (e, part) in &parts {
        if rigs
            .get(part.owner)
            .is_ok_and(|r| r.parts.get(part.index) == Some(&e))
        {
            continue;
        }
        commands.entity(e).try_despawn();
    }
    for (e, joint) in &joints {
        if rigs.get(joint.owner).is_ok_and(|r| r.joints.contains(&e)) {
            continue;
        }
        commands.entity(e).try_despawn();
    }
}

/// Parts hitting something hard: a `Body` ground contact (sounds, prints) and a noise.
pub(crate) fn ragdoll_impacts(
    ground: Res<Ground>,
    mut events: MessageReader<ContactEvent>,
    mut contacts: MessageWriter<GroundContact>,
    mut noises: MessageWriter<NoiseEvent>,
    parts: Query<(&RagdollPart, &Transform)>,
    rigs: Query<&RagdollRig>,
) {
    for ev in events.read() {
        if ev.phase != ContactPhase::Started || ev.sensor || ev.impulse <= 0.0 {
            continue;
        }
        let (part, t, normal) = match (parts.get(ev.a), parts.get(ev.b)) {
            (Ok((p, t)), Err(_)) => (p, t, -ev.normal),
            (Err(_), Ok((p, t))) => (p, t, ev.normal),
            _ => continue,
        };
        let Ok(rig) = rigs.get(part.owner) else {
            continue;
        };
        let Some(def) = rig.def.parts.get(part.index) else {
            continue;
        };
        let speed = ev.impulse / def.mass_kg.max(0.1);
        if speed < IMPACT_SPEED {
            continue;
        }
        let axis = t.rotation * (rig.def.frame.inverse() * (def.segment[1] - def.segment[0]));
        contacts.write(GroundContact {
            position: ev.point,
            normal,
            size: [def.radius * 2.0, axis.length() + def.radius * 2.0],
            yaw: GroundContact::yaw_of(axis),
            load_kg: def.mass_kg,
            speed,
            kind: ContactKind::Body,
            surface: ground.sample(ev.point).map(|s| s.surface),
            entity: Some(part.owner),
        });
        noises.write(NoiseEvent {
            position: ev.point,
            radius: (6.0 + speed * 3.0).min(30.0),
            source: Some(part.owner),
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::build::humanoid;

    #[test]
    fn a_person_has_fourteen_parts_weighing_seventy_five_kilos() {
        let s = humanoid::skeleton();
        let def = RagdollDef::for_skeleton(&s).expect("humanoid ragdoll");
        assert_eq!(def.parts.len(), 14);
        assert!((def.mass() - 75.0).abs() < 0.5, "{} kg", def.mass());
        // Collarbones sit between the chest and the upper arms; the head rides on the neck.
        assert!(def.between[s.bone("clavicle_l")]);
        assert!(!def.between[s.bone("head")]);
        assert!(!def.between[s.bone("root")]);
        // At rest every body has the same rotation, so every joint starts at its rest angle.
        let gt = GlobalTransform::from(Transform::from_xyz(3.0, 1.0, -2.0));
        let poses: Vec<_> = (0..def.parts.len())
            .map(|i| def.body_pose(i, &s.rest_model[def.bones[i]], &gt))
            .collect();
        for (_, r) in &poses {
            assert!(r.angle_between(poses[0].1) < 1e-4);
        }
        // Bodies map back onto their bones.
        for (i, &(p, r)) in poses.iter().enumerate() {
            let m = def.bone_model(i, p, r, &gt);
            let rest = s.rest_model[def.bones[i]];
            assert!(
                m.t.abs_diff_eq(rest.t, 1e-4),
                "{}: {:?}",
                def.parts[i].bone,
                m.t
            );
            assert!(m.r.angle_between(rest.r) < 1e-4);
        }
        // The feet stand on the ground and the head reaches about 1.75 m.
        let foot = def.parts.iter().find(|p| p.bone == "foot_l").unwrap();
        assert!((foot.segment[0].y - foot.radius).abs() < 1e-4);
        let head = def.parts.iter().find(|p| p.bone == "neck").unwrap();
        let top = head.segment[1].y + head.radius;
        assert!((1.7..1.85).contains(&top), "head top at {top}");
    }

    #[test]
    fn dogs_and_horses_have_ragdolls_of_their_weight() {
        use crate::build::{dog, horse};
        for (s, kg) in [
            (dog::skeleton(1.0), 30.0),
            (
                dog::skeleton(dog::scale_for_height(0.4)),
                30.0 * (0.4f32 / 0.56).powi(3),
            ),
            (horse::skeleton(), 500.0),
        ] {
            let def = RagdollDef::for_skeleton(&s).expect("a ragdoll");
            assert_eq!(def.parts.len(), 16, "{}", s.name);
            assert!(
                (def.mass() - kg).abs() < kg * 0.02,
                "{}: {} kg",
                s.name,
                def.mass()
            );
            // Every foot stands on the ground, and bodies map back onto their bones.
            for p in def.parts.iter().filter(|p| p.bone.contains("cannon")) {
                assert!((p.segment[1].y - p.radius).abs() < 1e-4, "{}", p.bone);
            }
            let gt = GlobalTransform::default();
            for i in 0..def.parts.len() {
                let rest = s.rest_model[def.bones[i]];
                let (p, r) = def.body_pose(i, &rest, &gt);
                let m = def.bone_model(i, p, r, &gt);
                assert!(m.t.abs_diff_eq(rest.t, 1e-4), "{}", def.parts[i].bone);
            }
        }
    }

    #[test]
    fn other_skeletons_have_no_ragdoll_yet() {
        let s = crate::build::spider::skeleton();
        assert!(RagdollDef::for_skeleton(&s).is_none());
    }
}
