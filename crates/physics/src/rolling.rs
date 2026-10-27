//! Rolling resistance. Rapier has none, so a ball, barrel or wheel on flat ground would roll
//! forever. Real ones squash a little where they touch, and the squashed patch pushes back
//! against the roll with a torque of about `Crr · N · r` (N the contact force, r the lever from
//! the center of mass to the contact). Hard ground gives `PhysicsSettings::rolling_resistance`;
//! soft ground (mud, sand, snow, grass) adds what sinking in costs, the same amounts the vehicle
//! tires use. Only round shapes roll: a box tipping over a hard edge squashes nothing.
//!
//! Spinning in place about the contact normal is held back the same way: by friction over the
//! contact patch, about `μ · N · SPIN_LEVER`. Rapier's twist friction only covers flat
//! multi-point contacts between plain bodies, so without this a ball spinning on its spot, or a
//! ragdoll's head (articulation links get no twist friction), would spin forever.

use engine_core::math::Vec3;
use engine_world::{Deformation, SurfaceMaterial};
use rapier3d::prelude::{
    Collider, ColliderHandle, ContactPair, PhysicsWorld, RigidBody, RigidBodyHandle, ShapeType,
};

use crate::convert;
use crate::sim::MaterialCache;

/// Extra rolling resistance coefficient from sinking into a soft surface.
pub(crate) fn sinkage(m: &SurfaceMaterial) -> f32 {
    match m.deformation {
        Deformation::Plastic { depth_m } => depth_m * 2.5,
        Deformation::Granular { depth_m } => depth_m * 3.0,
        Deformation::Snow => 0.05,
        Deformation::Flatten { .. } => 0.02,
        Deformation::None => 0.0,
    }
}

pub(crate) fn rolls(shape: ShapeType) -> bool {
    matches!(
        shape,
        ShapeType::Ball
            | ShapeType::Capsule
            | ShapeType::Cylinder
            | ShapeType::Cone
            | ShapeType::RoundCylinder
            | ShapeType::RoundCone
    )
}

/// How `collider` (on `body`) spins about its own axis of symmetry: the angular velocity about
/// that axis, a point on the axis, and the shape's radius. Spinning like this (a ball rolling,
/// a barrel or wheel turning on its axle) moves no part of the surface. `None` for shapes that
/// aren't round, and for fixed and kinematic bodies.
pub(crate) fn symmetric_spin(collider: &Collider, body: &RigidBody) -> Option<(Vec3, Vec3, f32)> {
    if !body.is_dynamic() {
        return None;
    }
    let shape = collider.shape();
    // The axis (`None` for a ball: any axis through its center) and a point on it, locally.
    let (axis, on_axis, radius) = match shape.shape_type() {
        ShapeType::Ball => (None, Vec3::ZERO, shape.as_ball()?.radius),
        ShapeType::Capsule => {
            let c = shape.as_capsule()?;
            let axis = (c.segment.b - c.segment.a).normalize_or(Vec3::Y);
            (Some(axis), c.segment.a, c.radius)
        }
        ShapeType::Cylinder => (Some(Vec3::Y), Vec3::ZERO, shape.as_cylinder()?.radius),
        ShapeType::Cone => (Some(Vec3::Y), Vec3::ZERO, shape.as_cone()?.radius),
        ShapeType::RoundCylinder => {
            let s = shape.as_round_cylinder()?;
            let r = s.inner_shape.radius + s.border_radius;
            (Some(Vec3::Y), Vec3::ZERO, r)
        }
        ShapeType::RoundCone => {
            let s = shape.as_round_cone()?;
            let r = s.inner_shape.radius + s.border_radius;
            (Some(Vec3::Y), Vec3::ZERO, r)
        }
        _ => return None,
    };
    let pose = collider.position();
    let w = body.angvel();
    let spin = match axis {
        None => w,
        Some(local) => {
            let axis = pose.rotation * local;
            axis * w.dot(axis)
        }
    };
    Some((spin, pose.transform_point(on_axis), radius))
}

/// Lever of spin friction, meters: two thirds of the radius of a hard contact's patch (1.5 cm).
const SPIN_LEVER: f32 = 0.01;

/// Spin slower than this fraction of the roll is left alone (tan 3°: how far a contact normal
/// can be off).
const SPIN_SLACK: f32 = 0.05;

/// One contact's pushback: body handles, the turn axis and the angular impulse it may take.
pub(crate) struct Resist {
    pair: (ColliderHandle, ColliderHandle),
    a: Option<RigidBodyHandle>,
    b: Option<RigidBodyHandle>,
    /// Unit axis of `a`'s roll relative to `b`.
    axis: Vec3,
    /// Relative roll rate, rad/s.
    rate: f32,
    /// Angular impulse the contact can take out this substep, N·m·s.
    budget: f32,
}

/// The pushback rolling resistance and spin friction owe a contact pair with a round or
/// articulated side, touched in the last substep (found while [`crate::sim::Region::touches`]
/// reads the pairs, so none is looked up twice). `base` is the hard-ground rolling resistance.
pub(crate) fn resist(
    world: &PhysicsWorld,
    pair: &ContactPair,
    base: f32,
    materials: &MaterialCache,
    pushes: &mut Vec<Resist>,
) {
    let (Some(c1), Some(c2)) = (
        world.colliders.get(pair.collider1),
        world.colliders.get(pair.collider2),
    ) else {
        return;
    };
    let (a, b) = (c1.parent(), c2.parent());
    let state = |h: Option<RigidBodyHandle>| {
        h.and_then(|h| world.bodies.get(h))
            .map(|rb| {
                (
                    rb.angvel(),
                    Some(rb.center_of_mass()),
                    rb.is_dynamic() && !rb.is_sleeping(),
                )
            })
            .unwrap_or((Vec3::ZERO, None, false))
    };
    let ((w1, com1, live1), (w2, com2, live2)) = (state(a), state(b));
    // Nothing turns against the other (a ball at rest in its cup): no pushback, and no need to
    // read the contact points.
    let rel = w1 - w2;
    if !(live1 || live2) || rel.length_squared() < 1e-12 {
        return;
    }
    let (round1, round2) = (
        rolls(c1.shape().shape_type()),
        rolls(c2.shape().shape_type()),
    );
    let crr = base
        + materials
            .rolling(convert::surface_of(c1.user_data))
            .max(materials.rolling(convert::surface_of(c2.user_data)));
    // Rapier's default rule for combining two colliders' friction.
    let mu = (c1.friction() + c2.friction()) * 0.5;
    // Normal impulse this substep, where it acted and along which normal. The impulses
    // live in the manifolds the solver saw: on terrain, meshes and compounds (one manifold
    // per triangle or part) those are merged clusters, and the plain ones hold none.
    let (mut jn, mut point, mut normal) = (0.0f32, Vec3::ZERO, Vec3::ZERO);
    for m in pair.solver_manifolds() {
        for c in &m.points {
            if c.data.impulse > 0.0 {
                jn += c.data.impulse;
                point += c1.position().transform_point(c.local_p1) * c.data.impulse;
                normal += m.data.normal * c.data.impulse;
            }
        }
    }
    if jn <= 1e-9 {
        return;
    }
    point /= jn;
    let normal = normal.normalize_or_zero();
    let spin = rel.dot(normal);
    let roll = rel - normal * spin;
    let rate = roll.length();
    // A normal a little off (an edge of the ground's triangles) shows some of a fast roll
    // as spin: leave that much alone, or a rolling barrel gets steered.
    let spinning = spin.abs() - rate * SPIN_SLACK;
    if mu > 0.0 && spinning > 1e-6 {
        pushes.push(Resist {
            pair: (pair.collider1, pair.collider2),
            a,
            b,
            axis: normal * spin.signum(),
            rate: spinning,
            budget: mu * jn * SPIN_LEVER,
        });
    }
    if !(round1 || round2) || crr <= 0.0 || rate < 1e-6 {
        return;
    }
    let axis = roll / rate;
    // How far the contact is from the axis the body rolls about: the radius, also when a
    // barrel rocks onto one rim.
    let radius = |com: Option<Vec3>| {
        com.map(|c| {
            let d = point - c;
            (d - axis * d.dot(axis)).length()
        })
        .unwrap_or(f32::INFINITY)
    };
    let lever = match (round1, round2) {
        (true, true) => radius(com1).min(radius(com2)),
        (true, false) => radius(com1),
        _ => radius(com2),
    };
    if !lever.is_finite() {
        return;
    }
    pushes.push(Resist {
        pair: (pair.collider1, pair.collider2),
        a,
        b,
        axis,
        rate,
        budget: crr * jn * lever,
    });
}

/// Take rolling resistance and spin friction out of the contacts touched in the last substep
/// ([`resist`]).
pub(crate) fn resist_rolling(world: &mut PhysicsWorld, mut pushes: Vec<Resist>) {
    // A fixed order, so the sums come out bit-identical every run (a contact's spin push comes
    // before its roll push).
    pushes.sort_by_key(|p| (p.pair.0.into_raw_parts(), p.pair.1.into_raw_parts()));
    for p in pushes {
        let inv = |h: Option<RigidBodyHandle>| {
            h.and_then(|h| world.bodies.get(h))
                .filter(|rb| rb.is_dynamic())
                .map(|rb| rb.mass_properties().effective_world_inv_inertia * p.axis)
                .unwrap_or(Vec3::ZERO)
        };
        let (ia, ib) = (inv(p.a), inv(p.b));
        let w = p.axis.dot(ia + ib);
        if w <= 1e-12 {
            continue;
        }
        // Never more than stops the roll.
        let j = p.budget.min(p.rate / w);
        for (h, dw) in [(p.a, -ia * j), (p.b, ib * j)] {
            if dw != Vec3::ZERO
                && let Some(rb) = h.and_then(|h| world.bodies.get_mut(h))
            {
                let w = rb.angvel() + dw;
                rb.set_angvel(w, false);
            }
        }
    }
}
