//! Articulations: joints between dynamic bodies solved in reduced coordinates.
//!
//! A Rapier multibody keeps each link's pose as coordinates along its joint's free axes under
//! its parent, so a ragdoll's elbow can only bend and a chain link can only swing: joints never
//! stretch, however hard the landing or long the chain (constraint joints pulled together by an
//! iterative solver open up for a frame when a ragdoll hits the ground). Which groups of jointed
//! bodies are articulated is decided in `Simulation::create_joint` (free-floating ones whose
//! masses are close).
//!
//! The price is that the multibody owns its links' poses and velocities: anything written to a
//! link body directly (a teleport, a new velocity, an impulse, a character's push, a world
//! shift, rolling resistance) would be overwritten by the next step. [`before_step`] folds such
//! writes into the articulation's coordinates: a link's written pose sets its joint's
//! coordinates (projected onto the free axes; the root's pose moves the whole articulation),
//! and velocity changes act as the impulses that would cause them, spread over the
//! articulation through its mass matrix (so an impulse on a hand moves the whole ragdoll
//! correctly). [`after_step`] records what the step left, so the next step can tell what
//! changed.

use std::collections::HashMap;

use engine_core::math::{Mat3, Vec3};
use engine_world::physics::{Joint, JointKind};
use rapier3d::na::{DMatrix, DVector};
use rapier3d::prelude::{
    Multibody, MultibodyLinkId, PhysicsWorld, Pose, RigidBody, RigidBodyHandle, RigidBodySet,
};

use crate::convert::{self, JointFrames};
use crate::sim::JointHandle;

/// Heaviest body over lightest one in a group of bodies jointed together that may be
/// articulated. Rapier's articulations blow up on contact when a far heavier body hangs from
/// light links (a 17 t wrecking ball on 12 kg chain links hitting a wall), whether the heavy body
/// is a link itself or held by a constraint joint, so such groups keep constraint joints.
const MAX_MASS_RATIO: f32 = 100.0;

/// Whether a jointed group with these masses may be articulated ([`MAX_MASS_RATIO`]).
pub(crate) fn close_masses(masses: impl IntoIterator<Item = f32>) -> bool {
    let (lightest, heaviest) = masses
        .into_iter()
        .fold((f32::MAX, 0.0f32), |(lo, hi), m| (lo.min(m), hi.max(m)));
    lightest > 0.0 && heaviest <= lightest * MAX_MASS_RATIO
}

/// Dry friction in the joints of a free-floating group (a ragdoll, a loose chain), as a
/// fraction of the load its limb's weight puts on it (see [`friction_of`]).
const JOINT_FRICTION: f32 = 0.1;

/// Shortest lever [`friction_of`] weighs a link's friction torque with, meters (a link centered on
/// its joint still has some size).
const MIN_LEVER: f32 = 0.05;

/// Standard gravity, m/s²: joint friction scales with the weight a link would have on Earth,
/// so it stays the same in free fall and in space.
const G: f32 = 9.81;

/// A written link this far from its joint was moved, not posed, meters.
const TORN: f32 = 0.01;

/// A link body as the last step left it.
#[derive(Clone, Copy, Debug)]
pub(crate) struct LinkState {
    /// The articulation's root body and this link's place in it (a link that moved to another
    /// articulation, or changed place in one, has coordinates that no longer match its body).
    root: RigidBodyHandle,
    id: usize,
    pose: Pose,
    linvel: Vec3,
    angvel: Vec3,
}

/// What the last step left in a region's articulations.
#[derive(Default)]
pub(crate) struct Links {
    /// Every link's body, by body.
    bodies: HashMap<RigidBodyHandle, LinkState>,
    /// Every articulation's generalized velocities, by root body. Rapier zeroes them when it
    /// splits or joins articulations, which its link bodies don't show.
    speeds: HashMap<RigidBodyHandle, DVector<f32>>,
}

/// Make `desc` (between dynamic bodies `h1` and `h2`) an articulation link. A body has one
/// parent, so the link goes from whichever body can be the parent; `None` when neither can (the
/// joint would close a loop): the caller makes a constraint joint instead.
pub(crate) fn link(
    world: &mut PhysicsWorld,
    h1: RigidBodyHandle,
    h2: RigidBodyHandle,
    desc: &Joint,
    frames: JointFrames,
) -> Option<JointHandle> {
    let set = &mut world.multibody_joints;
    let (l1, l2) = (
        set.rigid_body_link(h1).copied(),
        set.rigid_body_link(h2).copied(),
    );
    if let (Some(a), Some(b)) = (l1, l2)
        && a.multibody == b.multibody
    {
        return None;
    }
    // Only insert what Rapier accepts (a refused insert leaves an empty multibody behind).
    let root = |l: Option<MultibodyLinkId>| l.is_none_or(|l| l.id == 0);
    let collide = desc.collide_connected;
    let link = if root(l2) {
        set.insert(
            h1,
            h2,
            convert::articulation(&desc.kind, frames, collide),
            true,
        )?
    } else if root(l1) {
        let kind = convert::swapped(&desc.kind);
        set.insert(
            h2,
            h1,
            convert::articulation(&kind, frames.swapped(), collide),
            true,
        )?
    } else {
        return None;
    };
    let cone = match desc.kind {
        JointKind::Spherical {
            cone_limit: Some(limit),
        } => Some(world.impulse_joints.insert(
            h1,
            h2,
            convert::cone_limit(limit, frames, collide),
            true,
        )),
        _ => None,
    };
    Some(JointHandle::Articulated { link, cone })
}

/// Before a step: fold writes to link bodies since the last step into each articulation.
pub(crate) fn before_step(world: &mut PhysicsWorld, last: &Links) {
    for root in roots(world) {
        let Some(index) = world
            .multibody_joints
            .rigid_body_link(root)
            .map(|l| l.multibody)
        else {
            continue;
        };
        if let Some(mb) = world.multibody_joints.get_multibody_mut(index) {
            sync(mb, &mut world.bodies, last);
        }
    }
}

/// After a step: put every link where its coordinates say (Rapier's sweep against fast impacts
/// clamps link bodies like free bodies; the next step would snap them back) and record it.
pub(crate) fn after_step(world: &mut PhysicsWorld, last: &mut Links) {
    last.bodies.clear();
    last.speeds.clear();
    for root in roots(world) {
        let Some(index) = world
            .multibody_joints
            .rigid_body_link(root)
            .map(|l| l.multibody)
        else {
            continue;
        };
        let Some(mb) = world.multibody_joints.get_multibody_mut(index) else {
            continue;
        };
        if world.bodies.get(root).is_some_and(|rb| !rb.is_sleeping()) {
            mb.forward_kinematics(&world.bodies, true);
            // Only touch the clamped ones: a body written to wakes Rapier's bookkeeping.
            for link in mb.links() {
                let h = link.rigid_body_handle();
                let at = *link.local_to_world();
                if world
                    .bodies
                    .get(h)
                    .is_some_and(|rb| !same_pose(rb.position(), &at))
                    && let Some(rb) = world.bodies.get_mut(h)
                {
                    rb.set_position(at, false);
                }
            }
        }
        for (id, link) in mb.links().enumerate() {
            let h = link.rigid_body_handle();
            if let Some(rb) = world.bodies.get(h) {
                last.bodies.insert(
                    h,
                    LinkState {
                        root,
                        id,
                        pose: *rb.position(),
                        linvel: rb.linvel(),
                        angvel: rb.angvel(),
                    },
                );
            }
        }
        last.speeds
            .insert(root, mb.generalized_velocity().clone_owned());
    }
}

/// The root body of every articulation with more than one link.
fn roots(world: &PhysicsWorld) -> Vec<RigidBodyHandle> {
    world
        .multibody_joints
        .multibodies()
        .filter(|mb| mb.num_links() > 1)
        .map(|mb| mb.root().rigid_body_handle())
        .collect()
}

fn sync(mb: &mut Multibody, bodies: &mut RigidBodySet, last: &Links) {
    let root = mb.root().rigid_body_handle();
    let Some(root_body) = bodies.get(root) else {
        return;
    };
    // Coordinates Rapier reset since the last step: their links' bodies still move as they did.
    let reset = last
        .speeds
        .get(&root)
        .is_none_or(|q| q.len() != mb.ndofs() || *q != mb.generalized_velocity());
    let asleep = root_body.is_sleeping();
    if asleep {
        // Asleep: the bodies are still, so are the coordinates (Rapier only zeroes the bodies).
        mb.generalized_velocity_mut().fill(0.0);
    }
    let n = mb.num_links();
    let mut known = Vec::with_capacity(n);
    let mut poses = Vec::with_capacity(n);
    let mut written = Vec::with_capacity(n);
    for (id, link) in mb.links().enumerate() {
        let h = link.rigid_body_handle();
        let state = last
            .bodies
            .get(&h)
            .filter(|s| s.root == root && s.id == id)
            .copied();
        let pose = bodies.get(h).map_or(Pose::IDENTITY, |rb| *rb.position());
        written.push(state.is_none_or(|s| !same_pose(&s.pose, &pose)));
        known.push(state);
        poses.push(pose);
    }
    // Asleep and untouched since the last step (no pose or velocity written): nothing to fold
    // in, and its damping and friction can wait until it wakes. A heap of sleeping ragdolls
    // then costs next to nothing here.
    if asleep
        && !written.contains(&true)
        && mb.links().zip(&known).all(|(link, s)| {
            let rb = bodies.get(link.rigid_body_handle());
            s.zip(rb)
                .is_some_and(|(s, rb)| rb.linvel() == s.linvel && rb.angvel() == s.angvel)
        })
    {
        return;
    }

    // Poses written since the last step. A written root, or a written link torn from its
    // unwritten parent (a teleport, not a bend), carries the rest of the articulation with it:
    // moving any one part moves the whole body. Otherwise each written link takes its pose as
    // far as its joint allows (an animation pose bends the joints).
    if written.contains(&true) {
        let moved = |i: usize| known[i].map(|s| poses[i] * s.pose.inverse());
        let torn = |i: usize| {
            let link = mb.link(i).unwrap();
            let p = link.parent_id().unwrap();
            let data = &link.joint.data;
            written[i]
                && !written[p]
                && (poses[p] * data.local_frame1.translation)
                    .distance(poses[i] * data.local_frame2.translation)
                    > TORN
        };
        let carry = if written[0] {
            moved(0)
        } else {
            (1..n)
                .find(|&i| known[i].is_some() && torn(i))
                .and_then(moved)
        };
        if let Some(carry) = carry {
            for i in 0..n {
                if !written[i]
                    && let Some(s) = known[i]
                {
                    poses[i] = carry * s.pose;
                }
            }
            if !written[0]
                && let Some(rb) = bodies.get_mut(root)
            {
                // Rapier places the articulation from its root's body.
                rb.set_position(poses[0], false);
            }
        }
        match_poses(mb, &poses);
    }
    mb.forward_kinematics(bodies, true);

    // Velocity changes since the last step: against what the body had then, or (for a newcomer,
    // or after a reset) what the coordinates give it.
    let qdot = mb.generalized_velocity().clone_owned();
    let mut changes: Vec<(usize, [f32; 6])> = Vec::new();
    for (id, state) in known.iter().enumerate() {
        let link = mb.link(id).unwrap();
        let Some(rb) = bodies.get(link.rigid_body_handle()) else {
            continue;
        };
        let (v, w) = match state.filter(|_| !reset) {
            Some(s) => (s.linvel, s.angvel),
            None => {
                let j = mb.body_jacobian(id) * &qdot;
                (Vec3::new(j[0], j[1], j[2]), Vec3::new(j[3], j[4], j[5]))
            }
        };
        let (dv, dw) = (rb.linvel() - v, rb.angvel() - w);
        if dv != Vec3::ZERO || dw != Vec3::ZERO {
            changes.push((id, [dv.x, dv.y, dv.z, dw.x, dw.y, dw.z]));
        }
    }
    if !changes.is_empty() {
        apply_velocity_changes(mb, bodies, &changes);
    }
    damp(mb, bodies);
    joint_friction(mb, bodies);
}

/// Give the coordinates the damping the link bodies ask for (Rapier ignores a link body's
/// damping): each coordinate gets the drag its links would feel moving along it,
/// `Σ Jᵢᵀ diag(mᵢ·linear, Iᵢ·angular) Jᵢ` (the diagonal: Rapier damps each coordinate on its
/// own). A whole ragdoll drifting slows like one body would.
fn damp(mb: &mut Multibody, bodies: &RigidBodySet) {
    let n = mb.ndofs();
    let mut out = DVector::<f32>::zeros(n);
    for id in 0..mb.num_links() {
        let Some(rb) = mb.link(id).and_then(|l| bodies.get(l.rigid_body_handle())) else {
            continue;
        };
        let (lin, ang) = (rb.linear_damping(), rb.angular_damping());
        if lin == 0.0 && ang == 0.0 {
            continue;
        }
        let props = &rb.mass_properties().local_mprops;
        let r = Mat3::from_quat(*rb.rotation() * props.principal_inertia_local_frame);
        let inertia = r * Mat3::from_diagonal(props.principal_inertia()) * r.transpose();
        let jac = mb.body_jacobian(id);
        for k in 0..n {
            let v = Vec3::new(jac[(0, k)], jac[(1, k)], jac[(2, k)]);
            let w = Vec3::new(jac[(3, k)], jac[(4, k)], jac[(5, k)]);
            out[k] += rb.mass() * lin * v.length_squared() + ang * w.dot(inertia * w);
        }
    }
    mb.damping_mut().copy_from(&out);
}

/// Give every joint of the articulation a little dry friction, like a real joint's (see
/// [`friction_of`]). A bent arm then comes to rest instead of rocking about its free twist for
/// minutes, while a falling limb barely notices. The root's free coordinates, and coordinates a
/// motor drives, get none.
fn joint_friction(mb: &mut Multibody, bodies: &RigidBodySet) {
    let mut out = DVector::<f32>::zeros(mb.ndofs());
    for id in 1..mb.num_links() {
        let Some(link) = mb.link(id) else {
            continue;
        };
        let parent = link.parent_id().and_then(|p| mb.link(p));
        let (Some(rb2), Some(rb1)) = (
            bodies.get(link.rigid_body_handle()),
            parent.and_then(|p| bodies.get(p.rigid_body_handle())),
        ) else {
            continue;
        };
        let joint = &link.joint.data;
        let (torque, force) = friction_of(
            (rb1, joint.local_frame1.translation),
            (rb2, joint.local_frame2.translation),
        );
        let mut k = link.assembly_id();
        for axis in 0..6 {
            if joint.locked_axes.bits() & (1 << axis) == 0 {
                if joint.motor_axes.bits() & (1 << axis) == 0 {
                    out[k] = if axis < 3 { force } else { torque };
                }
                k += 1;
            }
        }
    }
    mb.frictions_mut().copy_from(&out);
}

/// The dry friction of a joint between two bodies (each with the joint's anchor in it), as
/// torque and force: [`JOINT_FRICTION`] of the load the lighter body (the limb) puts on it by
/// its weight, at the distance of its center from the joint.
pub(crate) fn friction_of(a: (&RigidBody, Vec3), b: (&RigidBody, Vec3)) -> (f32, f32) {
    let ((limb, anchor), _) = if a.0.mass() < b.0.mass() {
        (a, b)
    } else {
        (b, a)
    };
    let weight = limb.mass() * G;
    let lever = (limb.mass_properties().local_mprops.local_com - anchor)
        .length()
        .max(MIN_LEVER);
    (JOINT_FRICTION * weight * lever, JOINT_FRICTION * weight)
}

fn same_pose(a: &Pose, b: &Pose) -> bool {
    a.translation == b.translation && a.rotation == b.rotation
}

/// Set every joint's coordinates from the poses its two links should have (projected onto the
/// joint's free axes). The root's pose, which Rapier reads from its body, places the whole.
fn match_poses(mb: &mut Multibody, poses: &[Pose]) {
    let mut disp = vec![0.0; mb.ndofs()];
    for id in 1..mb.num_links() {
        let link = mb.link(id).unwrap();
        let parent = link.parent_id().unwrap();
        let joint = &link.joint;
        let rel = poses[parent].inverse() * poses[id];
        let target = joint.data.local_frame1.inverse() * rel * joint.data.local_frame2;
        let locked = joint.data.locked_axes.bits();
        let coords = joint.coords();
        let mut k = link.assembly_id();
        for (axis, coord) in coords.iter().take(3).enumerate() {
            if locked & (1 << axis) == 0 {
                disp[k] = target.translation[axis] - coord;
                k += 1;
            }
        }
        let free = !(locked >> 3) & 0b111;
        match free.count_ones() {
            1 => {
                let d = free.trailing_zeros() as usize;
                let q = target.rotation;
                let mut angle = 2.0 * q.to_array()[d].atan2(q.w);
                let tau = std::f32::consts::TAU;
                angle += tau * ((coords[3 + d] - angle) / tau).round();
                disp[k] = angle - coords[3 + d];
            }
            3 => {
                let mut r = target.rotation * joint.joint_rot().inverse();
                if r.w < 0.0 {
                    r = -r;
                }
                let turn = r.to_scaled_axis();
                disp[k..k + 3].copy_from_slice(&turn.to_array());
            }
            _ => {}
        }
    }
    mb.apply_displacements(&disp);
}

/// Change the articulation's generalized velocities by the impulses that would change link
/// `id`'s velocity by `dv` (linear, angular) as a free body: `Δq̇ = M⁻¹ Σ Jᵢᵀ Mᵢ Δvᵢ`. A change
/// the joints allow (the whole ragdoll moving at once) comes through exactly; one they don't
/// (a hand pushed sideways) is shared with the rest of the body as the push would be.
fn apply_velocity_changes(
    mb: &mut Multibody,
    bodies: &RigidBodySet,
    changes: &[(usize, [f32; 6])],
) {
    let n = mb.ndofs();
    if n == 0 {
        return;
    }
    // In f64: a heavy link on light ones (a wrecking ball on its chain) makes the mass matrix
    // too ill-conditioned for f32.
    let spatial = |id: usize| -> Option<DMatrix<f64>> {
        let rb = bodies.get(mb.link(id)?.rigid_body_handle())?;
        let props = &rb.mass_properties().local_mprops;
        let r = Mat3::from_quat(*rb.rotation() * props.principal_inertia_local_frame);
        let inertia = r * Mat3::from_diagonal(props.principal_inertia()) * r.transpose();
        let mut out = DMatrix::<f64>::zeros(6, 6);
        for i in 0..3 {
            out[(i, i)] = rb.mass() as f64;
            for j in 0..3 {
                out[(3 + i, 3 + j)] = inertia.col(j)[i] as f64;
            }
        }
        Some(out)
    };
    let jacobian = |id: usize| mb.body_jacobian(id).map(|x| x as f64);
    // Generalized impulse Σ Jᵢᵀ Mᵢ Δvᵢ over the links that changed, and M = Σ Jᵢᵀ Mᵢ Jᵢ.
    let mut rhs = DVector::<f64>::zeros(n);
    let mut mass = DMatrix::<f64>::zeros(n, n);
    for id in 0..mb.num_links() {
        let Some(mi) = spatial(id) else {
            continue;
        };
        let jac = jacobian(id);
        let mj = &mi * &jac;
        if let Some((_, d)) = changes.iter().find(|(i, _)| *i == id) {
            let dv = DVector::from_iterator(6, d.iter().map(|x| *x as f64));
            rhs += mj.transpose() * dv;
        }
        mass += jac.transpose() * mj;
    }
    let dq = mass.cholesky().map(|c| c.solve(&rhs).map(|x| x as f32));
    if let Some(dq) = dq.filter(|d| d.iter().all(|x| x.is_finite())) {
        let mut qdot = mb.generalized_velocity_mut();
        qdot += dq;
    }
}
