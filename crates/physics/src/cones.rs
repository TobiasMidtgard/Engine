//! Round cone limits on ball joints. Rapier limits each angular axis on its own: a limit on
//! two axes is a square cone around body A's X axis, and it measures swing and twist mixed
//! together, so a ragdoll's arm twisted half a turn about itself slips out of it, and limbs held
//! against it flail on the ground forever. Here the limit is round and twist is free: before
//! every substep, each cone's joint frames are turned so that their Z axis is the axis the limb
//! swings about, and only that angle is limited.
//!
//! The cone's axis is the limb's: body B's direction from the joint to its center of mass when
//! the joint was made (so an arm can swing `cone_limit` radians away from where it hung, in
//! body A's frame). A joint whose body B is centered on it, or that is pinned to the world,
//! uses body A's direction from its center through the joint.
//!
//! A limb held against its limit also rubs there: sliding along the cone's rim meets friction
//! in proportion to how hard the limit pushes back ([`STOP_FRICTION`]), like a real joint
//! pressed into its stop. Without it a head lolling against its neck limit rocks along the rim
//! for half a minute (the limit itself does no work along the rim).

use engine_core::math::{Mat3, Quat, Vec3};
use engine_world::physics::JointKind;
use rapier3d::prelude::{ImpulseJointHandle, Pose};

use crate::sim::{JointHandle, RegionId, Simulation};

/// Friction coefficient of a limb sliding along its cone's rim while pressed against it.
const STOP_FRICTION: f32 = 0.5;

/// The cone-limited joints in region `rid`: their Rapier joint and rest pose.
fn cones(sim: &Simulation, rid: RegionId) -> Vec<(ImpulseJointHandle, Quat)> {
    sim.joints
        .values()
        .filter(|j| {
            matches!(
                j.desc.kind,
                JointKind::Spherical {
                    cone_limit: Some(_)
                }
            )
        })
        .filter_map(|j| {
            let (region, handle) = j.created?;
            let cone = match handle {
                JointHandle::Impulse(h) => h,
                JointHandle::Articulated { cone, .. } => cone?,
            };
            (region == rid).then_some((cone, j.rest.unwrap_or(Quat::IDENTITY)))
        })
        .collect()
}

/// Turn the frames of every cone-limited joint in region `rid` to the limb's swing plane.
pub(crate) fn orient(sim: &mut Simulation, rid: RegionId) {
    let cones = cones(sim, rid);
    if cones.is_empty() {
        return;
    }
    let world = &mut sim.region_mut(rid).world;
    for (h, rest) in cones {
        let Some(joint) = world.impulse_joints.get(h) else {
            continue;
        };
        let (Some(rb1), Some(rb2)) = (
            world.bodies.get(joint.body1()),
            world.bodies.get(joint.body2()),
        ) else {
            continue;
        };
        let (anchor1, anchor2) = (
            joint.data.local_frame1.translation,
            joint.data.local_frame2.translation,
        );
        // `rest` is body 1's rotation seen from body 2: a direction `v` in body 2 is
        // `rest⁻¹ · v` in body 1.
        let limb = rb2.mass_properties().local_mprops.local_com - anchor2;
        let (axis1, axis2) = if rb2.is_dynamic() && limb.length() > 1e-4 {
            let a2 = limb.normalize();
            (rest.inverse() * a2, a2)
        } else {
            let out = anchor1 - rb1.mass_properties().local_mprops.local_com;
            let a1 = out.try_normalize().unwrap_or(Vec3::X);
            (a1, rest * a1)
        };
        let (r1, r2) = (*rb1.rotation(), *rb2.rotation());
        let (x1, x2) = (r1 * axis1, r2 * axis2);
        let swing = x1
            .cross(x2)
            .try_normalize()
            .unwrap_or_else(|| x1.any_orthonormal_vector());
        // Both frames: X along the limb, Z the swing axis. Body 2's is body 1's turned about Z
        // by the swing angle, the one angle the joint limits.
        let frame = |x: Vec3| Quat::from_mat3(&Mat3::from_cols(x, swing.cross(x), swing));
        let local1 = (r1.inverse() * frame(x1)).normalize();
        let local2 = (r2.inverse() * frame(x2)).normalize();
        if let Some(joint) = world.impulse_joints.get_mut(h, false) {
            joint
                .data
                .set_local_frame1(Pose::from_parts(anchor1, local1));
            joint
                .data
                .set_local_frame2(Pose::from_parts(anchor2, local2));
        }
    }
}

/// After a substep: friction for every limb that slid along its cone's rim while the limit
/// pushed back (the limit's impulse is the rim's normal force).
pub(crate) fn rub(sim: &mut Simulation, rid: RegionId) {
    let cones = cones(sim, rid);
    if cones.is_empty() {
        return;
    }
    let world = &mut sim.region_mut(rid).world;
    for (h, _) in cones {
        let Some(joint) = world.impulse_joints.get(h) else {
            continue;
        };
        // The swing limit is the joint's angular Z limit.
        let pressed = joint.data.limits[5].impulse.abs();
        if pressed <= 0.0 {
            continue;
        }
        let (b1, b2) = (joint.body1(), joint.body2());
        let (Some(rb1), Some(rb2)) = (world.bodies.get(b1), world.bodies.get(b2)) else {
            continue;
        };
        let swing = *rb1.rotation() * (joint.data.local_frame1.rotation * Vec3::Z);
        let rel = rb2.angvel() - rb1.angvel();
        let slide = rel - swing * rel.dot(swing);
        let rate = slide.length();
        if rate < 1e-6 {
            continue;
        }
        let axis = slide / rate;
        let inv = |rb: &rapier3d::prelude::RigidBody| {
            if rb.is_dynamic() {
                rb.mass_properties().effective_world_inv_inertia * axis
            } else {
                Vec3::ZERO
            }
        };
        let (i1, i2) = (inv(rb1), inv(rb2));
        let w = axis.dot(i1 + i2);
        if w <= 1e-12 {
            continue;
        }
        // Never more than stops the slide.
        let j = (STOP_FRICTION * pressed).min(rate / w);
        for (b, dw) in [(b1, i1 * j), (b2, -i2 * j)] {
            if dw != Vec3::ZERO
                && let Some(rb) = world.bodies.get_mut(b)
            {
                let w = rb.angvel() + dw;
                rb.set_angvel(w, false);
            }
        }
    }
}
