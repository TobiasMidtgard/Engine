//! Contact manifolds with a cached separating axis: a pair found apart last step, and still
//! apart along the same axis, skips its convex test.
//!
//! Rapier runs a full convex test (GJK, then polygon clipping) every step on every pair whose
//! boxes meet, and Parry on every triangle of a mesh under the other shape's box (grown by a
//! tenth, at most 0.1 m). Pairs in contact reuse their points while they barely move, but pairs
//! that are near and apart get the whole GJK again each step: boulders lying on a fine mesh
//! (0.25 m cells, as voxel cliffs are) spent 58% of the physics step there
//! (`mesh_rocks_bench`, callgrind, round 24 of the Physics thread).
//!
//! Parry already keeps GJK's last separating direction in an empty manifold's normal, as its
//! first guess. Here it is first tried as a separating axis: if the two shapes' extents along
//! it (two support points each way) are still more than the prediction distance apart, the
//! exact test would also leave the manifold empty, so it is left as it is. This is the cached
//! separating axis of Bullet's and PhysX's convex tests (van den Bergen, "Collision Detection
//! in Interactive 3D Environments", 2003, 4.3.3) and of Box2D's `b2SimplexCache`. Meshes, height
//! fields and compounds are dispatched as Parry does, with this dispatcher handed on so their
//! per-part tests come back here; everything else goes to Parry.

use rapier3d::geometry::{ContactData, ContactManifoldData};
use rapier3d::math::{Pose, Real, Vector};
use rapier3d::parry::query::details::{
    ContactManifold, ContactManifoldsWorkspace, NormalConstraints,
    contact_manifolds_composite_shape_composite_shape, contact_manifolds_composite_shape_shape,
    contact_manifolds_heightfield_shape_shapes, contact_manifolds_trimesh_shape_shapes,
};
use rapier3d::parry::query::{
    ClosestPoints, Contact, DefaultQueryDispatcher, NonlinearRigidMotion,
    PersistentQueryDispatcher, QueryDispatcher, ShapeCastHit, ShapeCastOptions, ShapeDistance,
    ShapeIntersection, Unsupported,
};
use rapier3d::parry::shape::{Shape, ShapeType, SupportMap};

/// How far beyond the prediction distance a cached axis must hold a pair apart to skip it
/// (m): a pair this close runs the exact test, so a contact always starts from a fresh GJK
/// direction, as without the cache. A barrel rolling at 8 m/s over terrain is chaotic enough
/// that the direction GJK starts from shows (`rolling.rs`, `barrels_roll_flat_...`): skipping
/// right up to the prediction distance let it veer 0.41 m in 3 s, against 0.16 m with
/// Parry's own; from 0.5 to 3 cm it veers 0.23 m and bounces 1.6 mm (4.9 mm with Parry's own).
const FRESH_M: Real = 0.01;

/// Skip pairs a cached axis holds apart? On unless `PHYSICS_AXIS_CACHE=0` (for an A/B, e.g.
/// with the `mesh_rocks_bench` example).
pub(crate) fn enabled() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| std::env::var("PHYSICS_AXIS_CACHE").map_or(true, |v| v != "0"))
}

/// The narrow phase's dispatcher: Parry's, with pairs a cached axis holds apart skipped.
#[derive(Clone, Copy, Default)]
pub(crate) struct MeshContacts;

/// Does `axis` (in shape 1's space; any length) still hold `shape1` and `shape2` more than
/// `margin` apart, either way along it? A pair GJK found apart last step keeps the axis it
/// found in its empty manifold's normal (Parry's own cache for GJK's first guess); while that
/// axis separates them, so does the exact test, and its answer would be the same empty manifold.
pub(crate) fn still_apart(
    pos12: &Pose,
    shape1: &dyn SupportMap,
    shape2: &dyn SupportMap,
    axis: Vector,
    margin: Real,
) -> bool {
    let to2 = pos12.rotation.inverse();
    let reach = |d: Vector| -> (Real, Real) {
        // Shape 1's farthest point along `d`, and shape 2's nearest.
        let far1 = shape1.local_support_point(d).dot(d);
        let near2 = (*pos12 * shape2.local_support_point(to2 * -d)).dot(d);
        (far1, near2)
    };
    let len = axis.length();
    if len < 1.0e-6 {
        return false;
    }
    let d = axis / len;
    let (far1, near2) = reach(d);
    if near2 - far1 > margin {
        return true;
    }
    let (far1, near2) = reach(-d);
    near2 - far1 > margin
}

impl QueryDispatcher for MeshContacts {
    fn intersection_test(
        &self,
        pos12: &Pose,
        g1: &dyn Shape,
        g2: &dyn Shape,
    ) -> Result<ShapeIntersection, Unsupported> {
        DefaultQueryDispatcher.intersection_test(pos12, g1, g2)
    }

    fn distance(
        &self,
        pos12: &Pose,
        g1: &dyn Shape,
        g2: &dyn Shape,
    ) -> Result<ShapeDistance, Unsupported> {
        DefaultQueryDispatcher.distance(pos12, g1, g2)
    }

    fn contact(
        &self,
        pos12: &Pose,
        g1: &dyn Shape,
        g2: &dyn Shape,
        prediction: Real,
    ) -> Result<Option<Contact>, Unsupported> {
        DefaultQueryDispatcher.contact(pos12, g1, g2, prediction)
    }

    fn closest_points(
        &self,
        pos12: &Pose,
        g1: &dyn Shape,
        g2: &dyn Shape,
        max_dist: Real,
    ) -> Result<ClosestPoints, Unsupported> {
        DefaultQueryDispatcher.closest_points(pos12, g1, g2, max_dist)
    }

    fn cast_shapes(
        &self,
        pos12: &Pose,
        local_vel12: Vector,
        g1: &dyn Shape,
        g2: &dyn Shape,
        options: ShapeCastOptions,
    ) -> Result<Option<ShapeCastHit>, Unsupported> {
        DefaultQueryDispatcher.cast_shapes(pos12, local_vel12, g1, g2, options)
    }

    fn cast_shapes_nonlinear(
        &self,
        motion1: &NonlinearRigidMotion,
        g1: &dyn Shape,
        motion2: &NonlinearRigidMotion,
        g2: &dyn Shape,
        start_time: Real,
        end_time: Real,
        stop_at_penetration: bool,
    ) -> Result<Option<ShapeCastHit>, Unsupported> {
        DefaultQueryDispatcher.cast_shapes_nonlinear(
            motion1,
            g1,
            motion2,
            g2,
            start_time,
            end_time,
            stop_at_penetration,
        )
    }
}

impl PersistentQueryDispatcher<ContactManifoldData, ContactData> for MeshContacts {
    fn contact_manifolds(
        &self,
        pos12: &Pose,
        shape1: &dyn Shape,
        shape2: &dyn Shape,
        prediction: Real,
        manifolds: &mut Vec<ContactManifold<ContactManifoldData, ContactData>>,
        workspace: &mut Option<ContactManifoldsWorkspace>,
    ) -> Result<(), Unsupported> {
        // Parry's own order, with this dispatcher handed on, so the convex tests inside
        // meshes, height fields and compounds come back here.
        let (c1, c2) = (shape1.as_composite_shape(), shape2.as_composite_shape());
        if let (Some(c1), Some(c2)) = (c1, c2) {
            contact_manifolds_composite_shape_composite_shape(
                self, pos12, c1, c2, prediction, manifolds, workspace,
            );
            return Ok(());
        }
        let (t1, t2) = (shape1.shape_type(), shape2.shape_type());
        if t1 == ShapeType::TriMesh || t2 == ShapeType::TriMesh {
            contact_manifolds_trimesh_shape_shapes(
                self, pos12, shape1, shape2, prediction, manifolds, workspace,
            );
            return Ok(());
        }
        if (t1 == ShapeType::HeightField && c2.is_none())
            || (t2 == ShapeType::HeightField && c1.is_none())
        {
            contact_manifolds_heightfield_shape_shapes(
                self, pos12, shape1, shape2, prediction, manifolds, workspace,
            );
            return Ok(());
        }
        let plain = |t: ShapeType| !matches!(t, ShapeType::HeightField | ShapeType::Voxels);
        if plain(t1) && plain(t2) {
            if let Some(c1) = c1 {
                contact_manifolds_composite_shape_shape(
                    self, pos12, c1, shape2, prediction, manifolds, workspace, false,
                );
                return Ok(());
            }
            if let Some(c2) = c2 {
                contact_manifolds_composite_shape_shape(
                    self,
                    &pos12.inverse(),
                    c2,
                    shape1,
                    prediction,
                    manifolds,
                    workspace,
                    true,
                );
                return Ok(());
            }
            // Two plain convex shapes: the broad phase already paired them by their boxes,
            // so a skip saves one test, and it would change which way the next GJK starts.
            // That alone can tip a chaotic pile (a ragdoll knocked over) into rocking rather
            // than sleeping, so these stay exactly Parry's.
        }
        DefaultQueryDispatcher
            .contact_manifolds(pos12, shape1, shape2, prediction, manifolds, workspace)
    }

    fn contact_manifold_convex_convex(
        &self,
        pos12: &Pose,
        shape1: &dyn Shape,
        shape2: &dyn Shape,
        normal_constraints1: Option<&dyn NormalConstraints>,
        normal_constraints2: Option<&dyn NormalConstraints>,
        prediction: Real,
        manifold: &mut ContactManifold<ContactManifoldData, ContactData>,
    ) -> Result<(), Unsupported> {
        if manifold.points.is_empty()
            && manifold.local_n1 != Vector::ZERO
            && let (Some(s1), Some(s2)) = (shape1.as_support_map(), shape2.as_support_map())
            && still_apart(pos12, s1, s2, manifold.local_n1, prediction + FRESH_M)
        {
            return Ok(());
        }
        DefaultQueryDispatcher.contact_manifold_convex_convex(
            pos12,
            shape1,
            shape2,
            normal_constraints1,
            normal_constraints2,
            prediction,
            manifold,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rapier3d::parry::shape::{ConvexPolyhedron, Cuboid, Triangle};

    #[test]
    fn an_axis_holds_shapes_apart_only_when_they_are_farther_than_the_margin() {
        // Against Parry's exact distance, for many hulls and triangles posed at random: an
        // axis never says apart for a pair within the margin, and the axis GJK finds says
        // apart for nearly every pair beyond it.
        let margin = 0.02;
        let mut seed = 0x2545_f491_u32;
        let mut r = || {
            seed ^= seed << 13;
            seed ^= seed >> 17;
            seed ^= seed << 5;
            seed as f32 / u32::MAX as f32 * 2.0 - 1.0
        };
        let (mut far, mut held) = (0, 0);
        for _ in 0..5_000 {
            let points: Vec<Vector> = (0..14)
                .map(|_| Vector::new(r(), r() * 0.6, r() * 0.8).normalize() * 0.7)
                .collect();
            let hull = ConvexPolyhedron::from_convex_hull(&points).unwrap();
            let c = Vector::new(r(), r(), r()) * 1.5;
            let t = Triangle::new(
                c + Vector::new(r(), r(), r()) * 0.5,
                c + Vector::new(r(), r(), r()) * 0.5,
                c + Vector::new(r(), r(), r()) * 0.5,
            );
            let pos12 = Pose::from_parts(
                Vector::new(r(), r(), r()) * 0.3,
                rapier3d::math::Rotation::from_scaled_axis(Vector::new(r(), r(), r())),
            );
            let dist = rapier3d::parry::query::distance(&Pose::IDENTITY, &t, &pos12, &hull)
                .unwrap()
                .distance;
            // Any axis, and the one from the closest points when they are apart.
            for axis in [Vector::new(r(), r(), r()), Vector::Y] {
                if dist <= margin {
                    assert!(!still_apart(&pos12, &t, &hull, axis, margin), "{dist}");
                }
            }
            if dist > margin * 2.0 {
                far += 1;
                let cp = rapier3d::parry::query::closest_points(
                    &Pose::IDENTITY,
                    &t,
                    &pos12,
                    &hull,
                    10.0,
                )
                .unwrap();
                if let rapier3d::parry::query::ClosestPoints::WithinMargin(p1, p2) = cp {
                    held += usize::from(still_apart(&pos12, &t, &hull, p2 - p1, margin));
                }
            }
        }
        assert!(far > 500, "{far}");
        assert!(held * 10 > far * 9, "{held} of {far}");
    }

    #[test]
    fn a_box_resting_on_a_mesh_keeps_its_contacts_and_skips_the_triangles_beside_it() {
        // The same manifolds as Parry's own, step after step, for a box sliding over a floor
        // of triangles.
        use rapier3d::parry::shape::TriMesh;
        let mut vertices = Vec::new();
        let mut indices = Vec::new();
        let n = 12;
        for j in 0..=n {
            for i in 0..=n {
                vertices.push(Vector::new(i as f32 * 0.5 - 3.0, 0.0, j as f32 * 0.5 - 3.0));
            }
        }
        let at = |i: u32, j: u32| j * (n + 1) + i;
        for j in 0..n {
            for i in 0..n {
                indices.push([at(i, j), at(i, j + 1), at(i + 1, j)]);
                indices.push([at(i + 1, j), at(i, j + 1), at(i + 1, j + 1)]);
            }
        }
        let floor = TriMesh::new(vertices, indices).unwrap();
        let cube = Cuboid::new(Vector::new(0.6, 0.3, 0.4));
        let (mut ours, mut theirs) = (Vec::new(), Vec::new());
        let (mut ws1, mut ws2) = (None, None);
        for k in 0..60 {
            let pos12 = Pose::from_parts(
                Vector::new(-1.5 + k as f32 * 0.05, 0.29 + 0.002 * (k % 3) as f32, 0.2),
                rapier3d::math::Rotation::from_scaled_axis(Vector::new(0.0, 0.01 * k as f32, 0.02)),
            );
            MeshContacts
                .contact_manifolds(&pos12, &floor, &cube, 0.02, &mut ours, &mut ws1)
                .unwrap();
            DefaultQueryDispatcher
                .contact_manifolds(&pos12, &floor, &cube, 0.02, &mut theirs, &mut ws2)
                .unwrap();
            let points = |m: &Vec<ContactManifold<ContactManifoldData, ContactData>>| {
                let mut p: Vec<(u32, i32)> = m
                    .iter()
                    .flat_map(|m| {
                        m.points
                            .iter()
                            .map(move |p| (m.subshape1, (p.dist * 1e4) as i32))
                    })
                    .collect();
                p.sort();
                p
            };
            assert_eq!(points(&ours), points(&theirs), "step {k}");
            assert!(!points(&ours).is_empty());
        }
    }
}
