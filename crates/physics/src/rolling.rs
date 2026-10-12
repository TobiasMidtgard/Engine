//! Rolling resistance. Rapier has none, so a ball, barrel or wheel on flat ground would roll
//! forever. Real ones squash a little where they touch, and the squashed patch pushes back
//! against the roll with a torque of about `Crr · N · r` (N the contact force, r the lever from
//! the center of mass to the contact). Hard ground gives `PhysicsSettings::rolling_resistance`;
//! soft ground (mud, sand, snow, grass) adds what sinking in costs, the same amounts the vehicle
//! tires use. Only round shapes roll: a box tipping over a hard edge squashes nothing.

use engine_core::math::Vec3;
use engine_world::{Deformation, SurfaceMaterial};
use rapier3d::prelude::{ColliderHandle, RigidBodyHandle, ShapeType};

use crate::convert;
use crate::sim::{RegionId, Simulation};

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

fn rolls(shape: ShapeType) -> bool {
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

/// One contact's pushback: body handles, the turn axis and the angular impulse it may take.
struct Resist {
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

/// Take rolling resistance out of every rolling contact touched in the last substep.
pub(crate) fn resist_rolling(sim: &mut Simulation, rid: RegionId) {
    let base = sim.settings.rolling_resistance.max(0.0);
    let materials = &sim.materials;
    let Some(region) = sim.regions[rid as usize].as_mut() else {
        return;
    };
    let world = &mut region.world;
    let mut pushes = Vec::new();
    for pair in world.narrow_phase.contact_pairs() {
        if !pair.has_any_active_contact() {
            continue;
        }
        let (Some(c1), Some(c2)) = (
            world.colliders.get(pair.collider1),
            world.colliders.get(pair.collider2),
        ) else {
            continue;
        };
        let (round1, round2) = (
            rolls(c1.shape().shape_type()),
            rolls(c2.shape().shape_type()),
        );
        if !(round1 || round2) {
            continue;
        }
        let (a, b) = (c1.parent(), c2.parent());
        let dynamic = |h: Option<RigidBodyHandle>| {
            h.and_then(|h| world.bodies.get(h))
                .is_some_and(|rb| rb.is_dynamic() && !rb.is_sleeping())
        };
        if !(dynamic(a) || dynamic(b)) {
            continue;
        }
        let crr = base
            + materials
                .rolling(convert::surface_of(c1.user_data))
                .max(materials.rolling(convert::surface_of(c2.user_data)));
        if crr <= 0.0 {
            continue;
        }
        // Normal impulse this substep, where it acted and along which normal.
        let (mut jn, mut point, mut normal) = (0.0f32, Vec3::ZERO, Vec3::ZERO);
        for m in pair.manifolds() {
            for c in &m.points {
                if c.data.impulse > 0.0 {
                    jn += c.data.impulse;
                    point += c1.position().transform_point(c.local_p1) * c.data.impulse;
                    normal += m.data.normal * c.data.impulse;
                }
            }
        }
        if jn <= 1e-9 {
            continue;
        }
        point /= jn;
        let normal = normal.normalize_or_zero();
        let state = |h: Option<RigidBodyHandle>| {
            h.and_then(|h| world.bodies.get(h))
                .map(|rb| (rb.angvel(), Some(rb.center_of_mass())))
                .unwrap_or((Vec3::ZERO, None))
        };
        let ((w1, com1), (w2, com2)) = (state(a), state(b));
        let rel = w1 - w2;
        // Spinning about the normal is twist friction's job (Rapier has it).
        let roll = rel - normal * rel.dot(normal);
        let rate = roll.length();
        if rate < 1e-6 {
            continue;
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
            continue;
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
    // A fixed order, so the sums come out bit-identical every run.
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
