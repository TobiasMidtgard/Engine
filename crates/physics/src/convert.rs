//! Translating contract types (`engine_world::physics`) into Rapier types and back.
//!
//! Nothing outside this crate sees Rapier: every conversion between the contract and the
//! solver lives here or in [`crate::sim`].

use engine_core::math::{Quat, Vec3};
use engine_core::prelude::*;
use engine_world::SurfaceId;
use engine_world::physics::{Collider, ColliderShape, CollisionLayers, JointKind, JointMotor};
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
                indices.to_vec(),
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
            // (Cached: a scaled hull reuses its unscaled one's triangles, and identical parts
            // of many compounds, a rock kit's few hulls, are built once.)
            _ => {
                if let Some(s) = shape_cached(part, scale) {
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

/// A breakable fixed joint's spring rate (as an acceleration per meter or radian, Rapier's
/// `AccelerationBased` motors) and damping: 60 Hz, critically damped, as stiff as the contacts
/// (a 0.07 mm sag under gravity).
const YIELD_STIFFNESS: f32 =
    (2.0 * std::f32::consts::PI * 60.0) * (2.0 * std::f32::consts::PI * 60.0);
const YIELD_DAMPING: f32 = 2.0 * 2.0 * std::f32::consts::PI * 60.0;

/// Turn a fixed joint into one that gives way at its strength: its six degrees of freedom are
/// held by stiff motors whose force (and torque) is capped at `force` (and `torque`), instead of
/// locked. A locked joint takes whatever a blow puts through it within the solve and only
/// breaks afterwards, so a ball thrown at a pegged post bounced off as from a wall and the post
/// never moved. Capped, the joint passes on at most its strength and the rest of the blow moves
/// the body; [`crate::step`] breaks it once a motor runs at its cap (`crate::breaking`).
pub fn yielding(joint: &mut GenericJoint, force: f32, torque: f32) {
    joint.locked_axes = JointAxesMask::empty();
    for (axes, cap) in [
        ([JointAxis::LinX, JointAxis::LinY, JointAxis::LinZ], force),
        ([JointAxis::AngX, JointAxis::AngY, JointAxis::AngZ], torque),
    ] {
        for axis in axes {
            joint.set_motor_model(axis, MotorModel::AccelerationBased);
            joint.set_motor(axis, 0.0, 0.0, YIELD_STIFFNESS, YIELD_DAMPING);
            joint.set_motor_max_force(axis, cap);
        }
    }
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
    fn big_meshes_of_the_same_points_and_scale_share_one_shape() {
        let n = 30u32;
        let mut vertices = Vec::new();
        let mut indices = Vec::new();
        for z in 0..=n {
            for x in 0..=n {
                vertices.push(Vec3::new(x as f32, (x * z) as f32 * 0.001, z as f32));
            }
        }
        for z in 0..n {
            for x in 0..n {
                let i = z * (n + 1) + x;
                indices.push([i, i + n + 1, i + 1]);
            }
        }
        let mesh = ColliderShape::TriMesh {
            vertices: vertices.into(),
            indices: indices.into(),
        };
        let a = shape_cached(&mesh, Vec3::ONE).unwrap();
        let b = shape_cached(&mesh, Vec3::ONE).unwrap();
        let scaled = shape_cached(&mesh, Vec3::splat(2.0)).unwrap();
        assert!(
            std::sync::Arc::ptr_eq(&a.0, &b.0),
            "the same mesh was built twice"
        );
        assert!(
            !std::sync::Arc::ptr_eq(&a.0, &scaled.0),
            "another scale shared the shape"
        );
        let small = ColliderShape::ConvexHull {
            points: vec![Vec3::ZERO, Vec3::X, Vec3::Y, Vec3::Z].into(),
        };
        let (c, d) = (
            shape_cached(&small, Vec3::ONE).unwrap(),
            shape_cached(&small, Vec3::ONE).unwrap(),
        );
        assert!(
            !std::sync::Arc::ptr_eq(&c.0, &d.0),
            "small shapes aren't cached"
        );
    }

    #[test]
    fn a_hull_scaled_from_its_unscaled_hull_is_the_hull_of_the_scaled_points() {
        let rock: Vec<Vec3> = (0..120)
            .map(|k| {
                let (a, b) = (k as f32 * 2.399_963, (k as f32 + 0.5) / 120.0);
                let y = 1.0 - 2.0 * b;
                let r = (1.0 - y * y).sqrt();
                Vec3::new(a.cos() * r, y, a.sin() * r) * (0.8 + 0.2 * (k as f32 * 7.3).sin())
            })
            .collect();
        for scale in [Vec3::new(0.4, 0.3, 0.37), Vec3::new(-0.5, 0.3, 0.6)] {
            let desc = ColliderShape::ConvexHull {
                points: rock.clone().into(),
            };
            let fast = hull_scaled(&desc, scale).unwrap();
            let pts: Vec<Vec3> = rock.iter().map(|p| *p * scale).collect();
            let slow = SharedShape::convex_hull(&pts).unwrap();
            for k in 0..200 {
                let (a, b) = (k as f32 * 0.731, k as f32 * 1.917);
                let p = Vec3::new(a.cos() * b.sin(), b.cos(), a.sin() * b.sin()) * 0.45;
                let pose = Pose::IDENTITY;
                assert_eq!(
                    fast.contains_point(&pose, p),
                    slow.contains_point(&pose, p),
                    "{p} at scale {scale}"
                );
                // Normals point out: a ray from outside toward the center meets the surface.
                let from = p.normalize_or(Vec3::Y) * 2.0;
                let ray = rapier3d::prelude::Ray::new(from, -from.normalize());
                let (hit_fast, hit_slow) = (
                    fast.cast_ray_and_get_normal(&pose, &ray, 10.0, true)
                        .unwrap(),
                    slow.cast_ray_and_get_normal(&pose, &ray, 10.0, true)
                        .unwrap(),
                );
                assert!((hit_fast.time_of_impact - hit_slow.time_of_impact).abs() < 1e-4);
                assert!(hit_fast.normal.dot(hit_slow.normal) > 0.999, "{scale}");
            }
        }
    }

    #[test]
    fn the_cache_forgets_only_shapes_nothing_uses() {
        let hull = |i: usize| ColliderShape::ConvexHull {
            points: (0..CACHE_MAX_POINTS / 4)
                .map(|k| {
                    let a = k as f32 * 0.37 + i as f32;
                    Vec3::new(a.cos(), (k % 7) as f32 * 0.1, a.sin())
                })
                .collect(),
        };
        let (kept, dropped) = (hull(101), hull(102));
        let held = shape_cached(&kept, Vec3::ONE).unwrap();
        drop(shape_cached(&dropped, Vec3::ONE));
        // Fill well past the cap with shapes nothing holds.
        for i in 0..6 {
            drop(shape_cached(&hull(200 + i), Vec3::ONE));
        }
        let again = shape_cached(&kept, Vec3::ONE).unwrap();
        assert!(
            std::sync::Arc::ptr_eq(&held.0, &again.0),
            "a shape in use was forgotten"
        );
        let c = cache().lock();
        assert!(!c.shapes.contains_key(&shape_key(&dropped, Vec3::ONE)));
        let idle: usize = c
            .shapes
            .values()
            .filter(|(s, _)| std::sync::Arc::strong_count(&s.0) == 1)
            .map(|(_, p)| *p)
            .sum();
        assert!(
            idle <= CACHE_MAX_POINTS,
            "{idle} points of unused shapes kept"
        );
    }

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
        assert!(
            shape(
                &ColliderShape::ConvexHull {
                    points: vec![].into()
                },
                Vec3::ONE
            )
            .is_none()
        );
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

/// `desc` without its shape, for what keeps a collider's settings beside its built shape (the
/// simulation's entries, a job's): a big mesh's points copied for each cost as much as
/// building it.
pub(crate) fn shapeless(desc: &Collider) -> Collider {
    Collider {
        shape: ColliderShape::default(),
        surface: desc.surface,
        density: desc.density,
        sensor: desc.sensor,
        layers: desc.layers,
    }
}

/// Shapes of this many points or more are kept in [`shape_cached`]'s cache: hashing one costs
/// about a microsecond per 100 points, building it 50-500 (a debris hull of 176 points took
/// 0.5 ms, and a zone's 302 of them 155 ms in one sync on the VM).
const CACHE_MIN_POINTS: usize = 64;
/// Points of shapes no collider uses the cache keeps at most (about 30 MB of meshes); the
/// oldest go first. Shapes in use cost nothing to keep (their colliders hold them anyway), so
/// they stay, and a zone streaming back in finds its shapes.
const CACHE_MAX_POINTS: usize = if cfg!(test) { 4_000 } else { 500_000 };

/// The points a shape is built from, if it is cached ([`CACHE_MIN_POINTS`]).
fn cache_points(shape: &ColliderShape) -> usize {
    match shape {
        ColliderShape::TriMesh { vertices, .. } => vertices.len(),
        ColliderShape::ConvexHull { points } => points.len(),
        ColliderShape::Compound(parts) => parts.iter().map(|(_, _, s)| cache_points(s)).sum(),
        _ => 0,
    }
}

/// A 64-bit hash of a shape and its scale (vertex bits, indices, offsets).
fn shape_key(shape: &ColliderShape, scale: Vec3) -> u64 {
    // FNV-1a over 32-bit words, finished with a 64-bit mix: fast enough to hash a 40,000
    // vertex mesh in well under a millisecond.
    struct H(u64);
    impl H {
        fn word(&mut self, w: u32) {
            self.0 = (self.0 ^ w as u64).wrapping_mul(0x0000_0100_0000_01b3);
        }
        fn vec(&mut self, v: Vec3) {
            self.word(v.x.to_bits());
            self.word(v.y.to_bits());
            self.word(v.z.to_bits());
        }
    }
    fn go(h: &mut H, shape: &ColliderShape) {
        match shape {
            ColliderShape::TriMesh { vertices, indices } => {
                h.word(1);
                h.word(vertices.len() as u32);
                vertices.iter().for_each(|v| h.vec(*v));
                for t in indices.iter() {
                    t.iter().for_each(|i| h.word(*i));
                }
            }
            ColliderShape::ConvexHull { points } => {
                h.word(2);
                h.word(points.len() as u32);
                points.iter().for_each(|v| h.vec(*v));
            }
            ColliderShape::Compound(parts) => {
                h.word(3);
                h.word(parts.len() as u32);
                for (offset, rotation, part) in parts {
                    h.vec(*offset);
                    for c in rotation.to_array() {
                        h.word(c.to_bits());
                    }
                    go(h, part);
                }
            }
            other => {
                // (Primitives are never cached; hash their debug form to stay total.)
                for b in format!("{other:?}").bytes() {
                    h.word(b as u32);
                }
            }
        }
    }
    let mut h = H(0xcbf2_9ce4_8422_2325);
    h.vec(scale);
    go(&mut h, shape);
    let mut x = h.0;
    x ^= x >> 33;
    x = x.wrapping_mul(0xff51_afd7_ed55_8ccd);
    x ^= x >> 33;
    x
}

#[derive(Default)]
struct ShapeCache {
    shapes: std::collections::HashMap<u64, (SharedShape, usize)>,
    order: std::collections::VecDeque<u64>,
    points: usize,
}

impl ShapeCache {
    /// Forget the oldest shapes no collider uses until those left unused hold at most
    /// three quarters of [`CACHE_MAX_POINTS`] (so a sweep isn't needed on every insert).
    fn evict(&mut self) {
        // (The cache's own handle is the only one of a shape nothing uses.)
        let unused = |s: &SharedShape| std::sync::Arc::strong_count(&s.0) == 1;
        let mut idle: usize = self
            .shapes
            .values()
            .filter(|(s, _)| unused(s))
            .map(|(_, p)| *p)
            .sum();
        let goal = CACHE_MAX_POINTS * 3 / 4;
        let shapes = &mut self.shapes;
        let mut freed = 0;
        self.order.retain(|k| {
            if idle <= goal {
                return true;
            }
            match shapes.get(k) {
                Some((s, p)) if unused(s) => {
                    idle -= p;
                    freed += p;
                    shapes.remove(k);
                    false
                }
                Some(_) => true,
                None => false,
            }
        });
        self.points -= freed;
    }
}

fn cache() -> &'static parking_lot::Mutex<ShapeCache> {
    static CACHE: std::sync::OnceLock<parking_lot::Mutex<ShapeCache>> = std::sync::OnceLock::new();
    CACHE.get_or_init(Default::default)
}

/// A hull of `points` at `scale`, from the hull of the unscaled points worked out once: rocks
/// are a few shapes each scaled its own way (the rockfall's 300 are 8), and scaling a hull is
/// a quarter of the cost of finding it. (Parry's `ConvexPolyhedron::scaled` turns face normals
/// by the scale, not its inverse, wrong for a scale that isn't uniform, so the scaled points
/// are joined again by the unscaled hull's triangles.)
fn hull_scaled(hull_desc: &ColliderShape, scale: Vec3) -> Option<SharedShape> {
    let ColliderShape::ConvexHull { points } = hull_desc else {
        return None;
    };
    type Hull = std::sync::Arc<(Vec<Vec3>, Vec<[u32; 3]>)>;
    static HULLS: std::sync::OnceLock<parking_lot::Mutex<std::collections::HashMap<u64, Hull>>> =
        std::sync::OnceLock::new();
    /// Hulls kept at most (a few MB); all are forgotten when there are more.
    const MAX_HULLS: usize = 4096;
    if scale.min_element().abs() < 1e-6 || !scale.is_finite() {
        return None;
    }
    let key = shape_key(hull_desc, Vec3::ONE);
    let hulls = HULLS.get_or_init(Default::default);
    let found = hulls.lock().get(&key).cloned();
    let hull = match found {
        Some(h) => h,
        None => {
            let (vertices, indices) =
                rapier3d::parry::transformation::try_convex_hull(points).ok()?;
            let h: Hull = std::sync::Arc::new((vertices, indices));
            let mut map = hulls.lock();
            if map.len() >= MAX_HULLS {
                map.clear();
            }
            map.insert(key, h.clone());
            h
        }
    };
    let (vertices, indices) = &*hull;
    let scaled: Vec<Vec3> = vertices.iter().map(|p| *p * scale).collect();
    // A mirroring scale turns the triangles inside out: wind them back.
    if scale.x * scale.y * scale.z < 0.0 {
        let flipped: Vec<[u32; 3]> = indices.iter().map(|t| [t[0], t[2], t[1]]).collect();
        SharedShape::convex_mesh(scaled, &flipped)
    } else {
        SharedShape::convex_mesh(scaled, indices)
    }
}

/// [`shape`], with big meshes, hulls and compounds shared: the same mesh at the same scale is
/// built once (a zone's jetty each time the zone streams back in, a building's repeated walls).
/// Parry shapes are immutable, so colliders can share one, as Jolt and PhysX share a mesh
/// between the bodies that use it.
pub fn shape_cached(shape_desc: &ColliderShape, scale: Vec3) -> Option<SharedShape> {
    let points = cache_points(shape_desc);
    if points < CACHE_MIN_POINTS {
        return shape(shape_desc, scale);
    }
    let key = shape_key(shape_desc, scale);
    if let Some((s, _)) = cache().lock().shapes.get(&key) {
        return Some(s.clone());
    }
    let built = match shape_desc {
        ColliderShape::ConvexHull { .. } => {
            hull_scaled(shape_desc, scale).or_else(|| shape(shape_desc, scale))
        }
        _ => shape(shape_desc, scale),
    }?;
    let mut c = cache().lock();
    if !c.shapes.contains_key(&key) && points <= CACHE_MAX_POINTS {
        c.shapes.insert(key, (built.clone(), points));
        c.order.push_back(key);
        c.points += points;
        if c.points > CACHE_MAX_POINTS {
            c.evict();
        }
    }
    Some(built)
}
