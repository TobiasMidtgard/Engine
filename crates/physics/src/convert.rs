//! Translating contract types (`engine_world::physics`) into Rapier types and back.
//!
//! Nothing outside this crate sees Rapier: every conversion between the contract and the
//! solver lives here or in [`crate::sim`].

use engine_core::math::{Quat, Vec3};
use engine_core::prelude::*;
use engine_world::SurfaceId;
use engine_world::physics::{ColliderShape, CollisionLayers, JointKind, JointMotor};
use rapier3d::parry::shape::{HeightFieldFlags, TriMeshFlags};
use rapier3d::parry::utils::Array2;
use rapier3d::prelude::*;

/// Smallest size any shape dimension is clamped to, meters (degenerate shapes break the solver).
const MIN_SIZE: f32 = 1.0e-3;

/// Build a Rapier shape for `shape` scaled by `scale` (the collider entity's world scale).
///
/// Spheres, capsules and cylinders can only scale uniformly around their axis, so they use the
/// largest relevant scale component. Returns `None` for shapes that cannot be built (a convex
/// hull of fewer than four non-coplanar points, an empty mesh); the caller logs and skips them.
pub fn shape(shape: &ColliderShape, scale: Vec3) -> Option<SharedShape> {
    let s = scale.abs();
    match shape {
        ColliderShape::Sphere { radius } => {
            Some(SharedShape::ball((radius * s.max_element()).max(MIN_SIZE)))
        }
        ColliderShape::Cuboid { half_extents } => {
            let h = (*half_extents * s).max(Vec3::splat(MIN_SIZE));
            Some(SharedShape::cuboid(h.x, h.y, h.z))
        }
        ColliderShape::Capsule {
            radius,
            half_height,
        } => Some(SharedShape::capsule_y(
            (half_height * s.y).max(0.0),
            (radius * s.x.max(s.z)).max(MIN_SIZE),
        )),
        ColliderShape::Cylinder {
            radius,
            half_height,
        } => Some(SharedShape::cylinder(
            (half_height * s.y).max(MIN_SIZE),
            (radius * s.x.max(s.z)).max(MIN_SIZE),
        )),
        ColliderShape::ConvexHull { points } => {
            let pts: Vec<Vec3> = points.iter().map(|p| *p * scale).collect();
            SharedShape::convex_hull(&pts)
        }
        ColliderShape::TriMesh { vertices, indices } => {
            if vertices.is_empty() || indices.is_empty() {
                return None;
            }
            let verts: Vec<Vec3> = vertices.iter().map(|p| *p * scale).collect();
            SharedShape::trimesh_with_flags(
                verts,
                indices.clone(),
                TriMeshFlags::FIX_INTERNAL_EDGES | TriMeshFlags::MERGE_DUPLICATE_VERTICES,
            )
            .ok()
        }
        ColliderShape::Heightfield {
            heights,
            rows,
            cols,
            size,
        } => {
            let (rows, cols) = (*rows as usize, *cols as usize);
            if rows < 2 || cols < 2 || heights.len() != rows * cols {
                return None;
            }
            // The contract stores heights row-major with rows along Z and columns along X;
            // parry wants a column-major matrix with the same row/column meaning.
            let mut data = vec![0.0; rows * cols];
            for r in 0..rows {
                for c in 0..cols {
                    data[r + c * rows] = heights[r * cols + c];
                }
            }
            Some(SharedShape::heightfield_with_flags(
                Array2::new(rows, cols, data),
                Vec3::new(size[0] * s.x, s.y, size[1] * s.z),
                HeightFieldFlags::FIX_INTERNAL_EDGES,
            ))
        }
        ColliderShape::Compound(parts) => {
            let mut out = Vec::with_capacity(parts.len());
            flatten_compound(parts, Pose::IDENTITY, scale, &mut out);
            (!out.is_empty()).then(|| SharedShape::compound(out))
        }
    }
}

/// Parry compounds can't nest, so nested compounds are flattened into one list of parts.
fn flatten_compound(
    parts: &[(Vec3, Quat, ColliderShape)],
    parent: Pose,
    scale: Vec3,
    out: &mut Vec<(Pose, SharedShape)>,
) {
    for (offset, rotation, part) in parts {
        let pose = parent * Pose::from_parts(*offset * scale, *rotation);
        match part {
            ColliderShape::Compound(inner) => flatten_compound(inner, pose, scale, out),
            _ => {
                if let Some(s) = shape(part, scale) {
                    out.push((pose, s));
                }
            }
        }
    }
}

/// Rapier's damping coefficient for "fraction of velocity lost per second".
///
/// Rapier damps continuously (`v' = -c·v`), so losing fraction `f` in one second means
/// `c = -ln(1 - f)`.
pub fn damping(fraction_per_second: f32) -> f32 {
    -(1.0 - fraction_per_second.clamp(0.0, 0.999)).ln()
}

/// Collision layers as Rapier interaction groups.
pub fn groups(layers: &CollisionLayers) -> InteractionGroups {
    InteractionGroups::new(
        Group::from_bits_retain(layers.member),
        Group::from_bits_retain(layers.collides_with),
        InteractionTestMode::And,
    )
}

/// Collider user data: the entity in the low 64 bits, the surface id above it and a sensor flag,
/// so query hits resolve to an entity and a surface without a map lookup.
pub fn pack_user_data(entity: Entity, surface: SurfaceId, sensor: bool) -> u128 {
    entity.to_bits() as u128 | (surface.0 as u128) << 64 | (sensor as u128) << 80
}

/// The entity a collider belongs to.
pub fn entity_of(user_data: u128) -> Entity {
    Entity::from_bits(user_data as u64)
}

/// The surface material of a collider.
pub fn surface_of(user_data: u128) -> SurfaceId {
    SurfaceId((user_data >> 64) as u16)
}

/// Rotation that turns +Y into `up` (characters stand along local "up" on planets).
pub fn up_rotation(up: Vec3) -> Quat {
    Quat::from_rotation_arc(Vec3::Y, up.try_normalize().unwrap_or(Vec3::Y))
}

/// The two attachment frames of a joint, in each body's local space.
#[derive(Debug, Clone, Copy)]
pub struct JointFrames {
    pub anchor1: Vec3,
    pub anchor2: Vec3,
    /// Joint axis (hinge or slide) in body 1's local space.
    pub axis: Vec3,
    /// Body 1's rotation seen from body 2 at rest (`rot2⁻¹ · rot1` when the joint was made):
    /// body 2's frame is body 1's turned by it, so the rest pose is the joint's zero (hinge
    /// angle 0, slide 0, a fixed joint holds the bodies as they were, a cone is centered on it).
    pub rest: Quat,
}

impl JointFrames {
    /// The same joint seen from body 2.
    pub fn swapped(self) -> Self {
        Self {
            anchor1: self.anchor2,
            anchor2: self.anchor1,
            axis: self.rest * self.axis,
            rest: self.rest.inverse(),
        }
    }
}

/// Build a Rapier joint for `kind` between two bodies.
pub fn joint(kind: &JointKind, frames: JointFrames, collide_connected: bool) -> GenericJoint {
    build(kind, frames, collide_connected, true)
}

/// Build the articulation link for `kind` (see [`articulates`]): the same joint without a cone
/// limit, which [`cone_limit`] adds as a separate constraint (a ball joint's articulation
/// coordinates integrate the spin, so they drift from the true swing angle).
pub fn articulation(
    kind: &JointKind,
    frames: JointFrames,
    collide_connected: bool,
) -> GenericJoint {
    build(kind, frames, collide_connected, false)
}

/// Whether `kind` can be an articulation link (ropes and springs are constraints by nature).
pub fn articulates(kind: &JointKind) -> bool {
    matches!(
        kind,
        JointKind::Fixed
            | JointKind::Spherical { .. }
            | JointKind::Revolute { .. }
            | JointKind::Prismatic { .. }
    )
}

/// A ball joint's cone limit on its own: frees every axis and limits the swing about Z
/// (`crate::cones` turns the frames to the swing plane before every substep).
pub fn cone_limit(limit: f32, frames: JointFrames, collide_connected: bool) -> GenericJoint {
    let l = limit.abs();
    let mut joint = GenericJointBuilder::new(JointAxesMask::empty())
        .limits(JointAxis::AngZ, [-l, l])
        .contacts_enabled(collide_connected)
        .build();
    place(&mut joint, frames, None);
    joint
}

/// `kind` as seen from body B: hinge and slide coordinates change sign.
pub fn swapped(kind: &JointKind) -> JointKind {
    let flip = |limits: &Option<[f32; 2]>, motor: &Option<JointMotor>| {
        (
            limits.map(|[lo, hi]| [-hi, -lo]),
            motor.map(|m| JointMotor {
                target_velocity: -m.target_velocity,
                target_position: m.target_position.map(|p| -p),
                ..m
            }),
        )
    };
    match kind {
        JointKind::Revolute {
            axis,
            limits,
            motor,
        } => {
            let (limits, motor) = flip(limits, motor);
            JointKind::Revolute {
                axis: *axis,
                limits,
                motor,
            }
        }
        JointKind::Prismatic {
            axis,
            limits,
            motor,
        } => {
            let (limits, motor) = flip(limits, motor);
            JointKind::Prismatic {
                axis: *axis,
                limits,
                motor,
            }
        }
        other => other.clone(),
    }
}

/// Set both frames: anchors, body 1's frame along `axis` (when the joint has one) and body
/// 2's frame matching it at rest.
fn place(joint: &mut GenericJoint, frames: JointFrames, axis: Option<Vec3>) {
    if let Some(axis) = axis {
        joint.set_local_axis1(axis.try_normalize().unwrap_or(Vec3::X));
    }
    let f1 = joint.local_frame1.rotation;
    joint.set_local_frame1(Pose::from_parts(frames.anchor1, f1));
    joint.set_local_frame2(Pose::from_parts(
        frames.anchor2,
        (frames.rest * f1).normalize(),
    ));
}

fn build(
    kind: &JointKind,
    frames: JointFrames,
    collide_connected: bool,
    cone: bool,
) -> GenericJoint {
    let mut joint: GenericJoint = match kind {
        JointKind::Fixed => FixedJointBuilder::new().build().into(),
        JointKind::Spherical { cone_limit } => {
            let mut b = SphericalJointBuilder::new();
            if let Some(limit) = cone_limit.filter(|_| cone) {
                // The swing about Z: `crate::cones` turns the frames to the swing plane
                // before every substep, which makes the cone round.
                let l = limit.abs();
                b = b.limits(JointAxis::AngZ, [-l, l]);
            }
            b.build().into()
        }
        JointKind::Revolute { limits, motor, .. } => {
            let mut j = RevoluteJointBuilder::new(Vec3::X).build();
            if let Some([lo, hi]) = limits {
                j.data.set_limits(JointAxis::AngX, [*lo, *hi]);
            }
            if let Some(m) = motor {
                apply_motor(&mut j.data, JointAxis::AngX, m);
            }
            j.data
        }
        JointKind::Prismatic { limits, motor, .. } => {
            let mut j = PrismaticJointBuilder::new(Vec3::X).build();
            if let Some([lo, hi]) = limits {
                j.data.set_limits(JointAxis::LinX, [*lo, *hi]);
            }
            if let Some(m) = motor {
                apply_motor(&mut j.data, JointAxis::LinX, m);
            }
            j.data
        }
        JointKind::Distance { limits } => {
            let mut j = RopeJointBuilder::new(limits[1].max(0.0)).build();
            j.data
                .set_limits(JointAxis::LinX, [limits[0].max(0.0), limits[1].max(0.0)]);
            j.data
        }
        JointKind::Spring {
            rest_length,
            stiffness,
            damping,
        } => SpringJointBuilder::new(rest_length.max(0.0), *stiffness, *damping)
            .spring_model(MotorModel::ForceBased)
            .build()
            .into(),
    };
    let axis = match kind {
        JointKind::Revolute { .. } | JointKind::Prismatic { .. } => Some(frames.axis),
        _ => None,
    };
    place(&mut joint, frames, axis);
    joint.set_contacts_enabled(collide_connected);
    joint
}

/// Dry friction on a constraint joint's free axes that no motor drives: up to `torque` (N·m)
/// about its turning axes and `force` (N) along a slide, holding the bodies still against each
/// other below that (a velocity motor toward rest that can't push harder).
pub fn add_friction(data: &mut GenericJoint, kind: &JointKind, torque: f32, force: f32) {
    let axes: &[JointAxis] = match kind {
        JointKind::Spherical { .. } => &[JointAxis::AngX, JointAxis::AngY, JointAxis::AngZ],
        JointKind::Revolute { motor: None, .. } => &[JointAxis::AngX],
        JointKind::Prismatic { motor: None, .. } => &[JointAxis::LinX],
        _ => &[],
    };
    for &axis in axes {
        let most = if axis == JointAxis::LinX {
            force
        } else {
            torque
        };
        data.set_motor_model(axis, MotorModel::ForceBased);
        data.set_motor_velocity(axis, 0.0, FRICTION_GRIP);
        data.set_motor_max_force(axis, most);
    }
}

/// How hard [`add_friction`]'s motors hold toward rest below their limit (N·m·s/rad): far
/// stiffer than any limb, so it acts like dry friction.
const FRICTION_GRIP: f32 = 1.0e4;

fn apply_motor(data: &mut GenericJoint, axis: JointAxis, m: &JointMotor) {
    data.set_motor_model(axis, MotorModel::ForceBased);
    match m.target_position {
        Some(target) => {
            data.set_motor(axis, target, m.target_velocity, m.stiffness, m.damping);
        }
        None => {
            // A velocity motor: `damping` is how hard it pulls toward the target speed.
            let factor = if m.damping > 0.0 { m.damping } else { 1.0 };
            data.set_motor_velocity(axis, m.target_velocity, factor);
        }
    }
    data.set_motor_max_force(axis, m.max_force.max(0.0));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn damping_matches_fraction_lost_per_second() {
        // Losing half the speed in a second: v(1) = e^-c = 0.5.
        let c = damping(0.5);
        assert!(((-c).exp() - 0.5).abs() < 1e-5);
        assert_eq!(damping(0.0), 0.0);
        assert!(damping(1.0).is_finite());
    }

    #[test]
    fn user_data_round_trips() {
        let e = Entity::from_bits((7u64 << 32) | 42);
        let data = pack_user_data(e, SurfaceId(11), true);
        assert_eq!(entity_of(data), e);
        assert_eq!(surface_of(data), SurfaceId(11));
        assert_eq!((data >> 80) & 1, 1);
    }

    #[test]
    fn shapes_scale_with_the_entity() {
        let s = shape(
            &ColliderShape::Cuboid {
                half_extents: Vec3::new(1.0, 2.0, 3.0),
            },
            Vec3::new(2.0, 1.0, 0.5),
        )
        .unwrap();
        let c = s.as_cuboid().unwrap();
        assert_eq!(c.half_extents, Vec3::new(2.0, 2.0, 1.5));
        assert!(shape(&ColliderShape::ConvexHull { points: vec![] }, Vec3::ONE).is_none());
    }

    #[test]
    fn heightfield_rows_run_along_z() {
        // 2 rows (z) × 3 columns (x); the last column is high.
        let hf = shape(
            &ColliderShape::Heightfield {
                heights: vec![0.0, 0.0, 5.0, 0.0, 0.0, 5.0],
                rows: 2,
                cols: 3,
                size: [2.0, 1.0],
            },
            Vec3::ONE,
        )
        .unwrap();
        let aabb = hf.compute_local_aabb();
        assert!((aabb.maxs.y - 5.0).abs() < 1e-5);
        // Height under +x is high, under -x low.
        use rapier3d::parry::query::{Ray, RayCast};
        let h = hf.as_heightfield().unwrap();
        let height_at = |x: f32| {
            let ray = Ray::new(Vec3::new(x, 10.0, 0.0), -Vec3::Y);
            10.0 - h.cast_local_ray(&ray, 20.0, true).unwrap()
        };
        assert!(height_at(0.95) > 4.0, "{}", height_at(0.95));
        assert!(height_at(-0.95) < 0.1, "{}", height_at(-0.95));
    }
}
