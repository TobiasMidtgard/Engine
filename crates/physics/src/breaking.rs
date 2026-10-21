//! Joints that snap when bent or twisted too hard, not only when pulled.
//!
//! `engine_world::physics::Joint::break_force` breaks a joint on the force it carries. A pegged
//! timber frame, a glued shelf or a welded bracket mostly fails by bending: the beam's weight
//! or a blow turns it about the joint, and a fixed joint resists that with torque, which a force
//! limit never sees. PhysX (`PxConstraint::setBreakForce(force, torque)`) and Unity
//! (`Joint.breakForce` and `Joint.breakTorque`) break a joint on either, and so does this
//! backend: put [`BreakTorque`] on a joint's entity next to its `Joint`.
//!
//! A breakable fixed joint gives way at its strength: it is held by stiff motors capped at its
//! limits, not locked (`convert::yielding`), so a blow passes on at most what the joint can
//! carry and the rest moves the body it strikes; the joint breaks once a motor runs at its cap.
//! A locked joint took the whole blow within the solve and broke only afterwards: a catapult
//! ball bounced off a pegged post as off a wall. Breakable joints are never articulated.

use engine_core::prelude::*;

/// Break this joint above this torque, N·m (with or without a `break_force`). Read when the
/// joint is made: insert it with the `Joint`.
#[derive(Component, Debug, Clone, Copy, PartialEq)]
pub struct BreakTorque(pub f32);
