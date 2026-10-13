//! Exact capsule-against-box queries for characters.
//!
//! Parry answers capsule-box casts and contacts with GJK, which slips in two common cases: a
//! capsule within a few millimeters of a box's center axis, where a cast that starts inside the
//! controller's 2 cm gap can miss and a contact can report the wrong distance, and boxes
//! hundreds of meters across, where both lose centimeters to tens of centimeters. Either lets a
//! character sink into the floor (20 cm on a 25 m box, meters on a 500 m one).
//!
//! A capsule is a segment with a radius, and the distance from a point to a box is a clamp, so
//! here these queries are exact: the closest points between the segment and the box minimize a
//! piecewise quadratic, and the time of impact comes from Newton steps on that distance, which
//! is convex in time, so a step never passes the hit. Everything else (and a capsule whose core
//! is already inside the box) goes to Parry's own dispatcher.

use engine_core::math::DVec3;
use rapier3d::math::{Pose, Real, Vector};
use rapier3d::parry::query::{
    ClosestPoints, Contact, DefaultQueryDispatcher, NonlinearRigidMotion, QueryDispatcher,
    ShapeCastHit, ShapeCastOptions, ShapeCastStatus, ShapeDistance, ShapeIntersection, Unsupported,
};
use rapier3d::parry::shape::{Capsule, Shape};

/// Queries for characters: exact for capsules against boxes, Parry's own for the rest.
pub(crate) fn dispatcher() -> impl QueryDispatcher {
    CapsuleBox.chain(DefaultQueryDispatcher)
}

/// Answers capsule-box casts and contacts; [`Unsupported`] for everything else.
pub(crate) struct CapsuleBox;

/// A capsule seen from a box's frame: its core segment, radius, and pose there.
struct InBox {
    a: DVec3,
    b: DVec3,
    radius: f64,
    half: DVec3,
    /// The capsule's pose in the box's frame.
    pose: Pose,
}

impl InBox {
    /// `pose` places the capsule in the box's frame.
    fn new(capsule: &Capsule, half: Vector, pose: Pose) -> Self {
        Self {
            a: pose.transform_point(capsule.segment.a).as_dvec3(),
            b: pose.transform_point(capsule.segment.b).as_dvec3(),
            radius: capsule.radius as f64,
            half: half.as_dvec3(),
            pose,
        }
    }

    /// A point given in the box's frame, in the capsule's frame after it moved by `moved`.
    fn to_capsule(&self, p: DVec3, moved: DVec3) -> Vector {
        self.pose.inverse_transform_point((p - moved).as_vec3())
    }

    fn dir_to_capsule(&self, d: DVec3) -> Vector {
        self.pose.rotation.inverse() * d.as_vec3()
    }
}

/// The closest points between a capsule's core and a box, in the box's frame.
#[derive(Clone, Copy, Debug)]
struct Closest {
    core: DVec3,
    on_box: DVec3,
    dist: f64,
}

impl Closest {
    /// From the box toward the capsule's core.
    fn normal(&self) -> DVec3 {
        (self.core - self.on_box) / self.dist
    }
}

/// Squared distance from `p` to the box with half extents `h` centered at the origin.
fn dist2(p: DVec3, h: DVec3) -> f64 {
    (p - p.clamp(-h, h)).length_squared()
}

/// The closest points between segment `a`-`b` and the box with half extents `h` centered at the
/// origin. Along the segment the squared distance is a convex piecewise quadratic whose pieces
/// change where a coordinate crosses a face's plane, so each piece's minimum is exact.
fn segment_box(a: DVec3, b: DVec3, h: DVec3) -> Closest {
    let e = b - a;
    let mut breaks = [0.0f64; 8];
    breaks[1] = 1.0;
    let mut n = 2;
    for i in 0..3 {
        if e[i].abs() > 1e-12 {
            for plane in [-h[i], h[i]] {
                let s = (plane - a[i]) / e[i];
                if s > 0.0 && s < 1.0 {
                    breaks[n] = s;
                    n += 1;
                }
            }
        }
    }
    let breaks = &mut breaks[..n];
    breaks.sort_by(f64::total_cmp);
    let (mut best_d2, mut best_s) = (f64::MAX, 0.0);
    for w in breaks.windows(2) {
        let (s0, s1) = (w[0], w[1]);
        let mid = a + e * (0.5 * (s0 + s1));
        // On this piece the squared distance is qa s² + qb s + const.
        let (mut qa, mut qb) = (0.0, 0.0);
        for i in 0..3 {
            let off = if mid[i] > h[i] {
                a[i] - h[i]
            } else if mid[i] < -h[i] {
                a[i] + h[i]
            } else {
                continue;
            };
            qa += e[i] * e[i];
            qb += 2.0 * e[i] * off;
        }
        let s = if qa > 0.0 {
            (-qb / (2.0 * qa)).clamp(s0, s1)
        } else {
            s0
        };
        let d2 = dist2(a + e * s, h);
        if d2 < best_d2 {
            (best_d2, best_s) = (d2, s);
        }
    }
    let core = a + e * best_s;
    Closest {
        core,
        on_box: core.clamp(-h, h),
        dist: best_d2.sqrt(),
    }
}

/// Below this the capsule's core is inside the box: no normal to trust here.
const CORE_INSIDE: f64 = 1e-9;

/// How the capsule moving at `vel` (box frame) meets the box: `Ok(None)` if it doesn't within
/// `options`, the time and closest points if it does, [`Unsupported`] if its core starts inside.
fn cast(
    c: &InBox,
    vel: DVec3,
    options: &ShapeCastOptions,
) -> Result<Option<(f64, Closest)>, Unsupported> {
    let reach = c.radius + options.target_distance as f64;
    let max_t = options.max_time_of_impact as f64;
    let mut t = 0.0;
    let mut reached = None;
    for i in 0..64 {
        let moved = vel * t;
        let cl = segment_box(c.a + moved, c.b + moved, c.half);
        if cl.dist < CORE_INSIDE {
            // Only possible at the start: the steps stop `reach` short of the core.
            return if i == 0 {
                Err(Unsupported)
            } else {
                Ok(reached)
            };
        }
        reached = Some((t, cl));
        let gap = cl.dist - reach;
        let closing = -cl.normal().dot(vel);
        if gap <= 1e-6 {
            // Starting within reach and moving away is not a hit unless asked to stop there.
            if i == 0 && !options.stop_at_penetration && closing <= 0.0 {
                return Ok(None);
            }
            return Ok(reached);
        }
        if closing <= 1e-12 {
            return Ok(None);
        }
        // The distance is convex in time, so this tangent step lands at or before the hit.
        t += gap / closing;
        if t > max_t {
            return Ok(None);
        }
    }
    // Out of steps (not seen in practice): the last point reached, just short of the hit.
    Ok(reached)
}

/// The cast's hit with the box as the first shape (box frame) and the capsule second.
fn hit_box_first(
    c: &InBox,
    vel: DVec3,
    t: f64,
    cl: Closest,
    options: &ShapeCastOptions,
) -> ShapeCastHit {
    let n = cl.normal();
    let status = if t > 0.0 {
        ShapeCastStatus::Converged
    } else if cl.dist - c.radius < options.target_distance as f64 {
        ShapeCastStatus::PenetratingOrWithinTargetDist
    } else {
        ShapeCastStatus::Converged
    };
    ShapeCastHit {
        time_of_impact: t as Real,
        witness1: cl.on_box.as_vec3(),
        witness2: c.to_capsule(cl.core - n * c.radius, vel * t),
        normal1: n.as_vec3(),
        normal2: c.dir_to_capsule(-n),
        status,
        subshape1: 0,
        subshape2: 0,
    }
}

fn contact_box_first(c: &InBox, prediction: Real) -> Result<Option<Contact>, Unsupported> {
    let cl = segment_box(c.a, c.b, c.half);
    if cl.dist < CORE_INSIDE {
        return Err(Unsupported);
    }
    let dist = cl.dist - c.radius;
    if dist > prediction as f64 {
        return Ok(None);
    }
    let n = cl.normal();
    Ok(Some(Contact {
        point1: cl.on_box.as_vec3(),
        point2: c.to_capsule(cl.core - n * c.radius, DVec3::ZERO),
        normal1: n.as_vec3(),
        normal2: c.dir_to_capsule(-n),
        dist: dist as Real,
        subshape1: 0,
        subshape2: 0,
    }))
}

impl QueryDispatcher for CapsuleBox {
    fn intersection_test(
        &self,
        _: &Pose,
        _: &dyn Shape,
        _: &dyn Shape,
    ) -> Result<ShapeIntersection, Unsupported> {
        Err(Unsupported)
    }

    fn distance(
        &self,
        _: &Pose,
        _: &dyn Shape,
        _: &dyn Shape,
    ) -> Result<ShapeDistance, Unsupported> {
        Err(Unsupported)
    }

    fn contact(
        &self,
        pos12: &Pose,
        g1: &dyn Shape,
        g2: &dyn Shape,
        prediction: Real,
    ) -> Result<Option<Contact>, Unsupported> {
        if let (Some(cuboid), Some(capsule)) = (g1.as_cuboid(), g2.as_capsule()) {
            contact_box_first(
                &InBox::new(capsule, cuboid.half_extents, *pos12),
                prediction,
            )
        } else if let (Some(capsule), Some(cuboid)) = (g1.as_capsule(), g2.as_cuboid()) {
            let c = InBox::new(capsule, cuboid.half_extents, pos12.inverse());
            Ok(contact_box_first(&c, prediction)?.map(Contact::flipped))
        } else {
            Err(Unsupported)
        }
    }

    fn closest_points(
        &self,
        _: &Pose,
        _: &dyn Shape,
        _: &dyn Shape,
        _: Real,
    ) -> Result<ClosestPoints, Unsupported> {
        Err(Unsupported)
    }

    fn cast_shapes(
        &self,
        pos12: &Pose,
        local_vel12: Vector,
        g1: &dyn Shape,
        g2: &dyn Shape,
        options: ShapeCastOptions,
    ) -> Result<Option<ShapeCastHit>, Unsupported> {
        if let (Some(cuboid), Some(capsule)) = (g1.as_cuboid(), g2.as_capsule()) {
            // The capsule moves at `local_vel12` in the box's frame.
            let c = InBox::new(capsule, cuboid.half_extents, *pos12);
            let vel = local_vel12.as_dvec3();
            Ok(cast(&c, vel, &options)?.map(|(t, cl)| hit_box_first(&c, vel, t, cl, &options)))
        } else if let (Some(capsule), Some(cuboid)) = (g1.as_capsule(), g2.as_cuboid()) {
            // The box moves at `local_vel12` in the capsule's frame: the capsule moves the
            // other way in the box's.
            let c = InBox::new(capsule, cuboid.half_extents, pos12.inverse());
            let vel = (c.pose.rotation * -local_vel12).as_dvec3();
            Ok(cast(&c, vel, &options)?
                .map(|(t, cl)| hit_box_first(&c, vel, t, cl, &options).swapped()))
        } else {
            Err(Unsupported)
        }
    }

    fn cast_shapes_nonlinear(
        &self,
        _: &NonlinearRigidMotion,
        _: &dyn Shape,
        _: &NonlinearRigidMotion,
        _: &dyn Shape,
        _: Real,
        _: Real,
        _: bool,
    ) -> Result<Option<ShapeCastHit>, Unsupported> {
        Err(Unsupported)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use engine_core::math::{EulerRot, Quat};
    use rapier3d::parry::query;
    use rapier3d::parry::shape::Cuboid;

    fn options(max: Real, target: Real) -> ShapeCastOptions {
        ShapeCastOptions {
            max_time_of_impact: max,
            target_distance: target,
            stop_at_penetration: false,
            compute_impact_geometry_on_penetration: true,
        }
    }

    #[test]
    fn segment_box_distances_are_exact() {
        let h = DVec3::new(1.0, 2.0, 3.0);
        // Above the top face, over the center axis.
        let c = segment_box(DVec3::new(0.0, 2.5, 0.0), DVec3::new(0.0, 4.0, 0.0), h);
        assert!((c.dist - 0.5).abs() < 1e-12 && c.on_box.y == 2.0);
        // Crossing past an edge diagonally.
        let c = segment_box(DVec3::new(2.0, 3.0, -1.0), DVec3::new(2.0, 3.0, 1.0), h);
        assert!((c.dist - 2f64.sqrt()).abs() < 1e-12, "{c:?}");
        // Parallel to a face, beside it.
        let c = segment_box(DVec3::new(-5.0, 0.0, 3.25), DVec3::new(5.0, 0.0, 3.25), h);
        assert!((c.dist - 0.25).abs() < 1e-12, "{c:?}");
        // Pointing at a corner from far away.
        let c = segment_box(DVec3::new(4.0, 5.0, 6.0), DVec3::new(10.0, 10.0, 10.0), h);
        assert!((c.dist - 27f64.sqrt()).abs() < 1e-12, "{c:?}");
    }

    /// A capsule over a box's top, cast down, lands with the target gap from any box size and
    /// any spot, including right over the center and on boxes kilometers across.
    #[test]
    fn capsules_land_on_top_of_boxes_of_any_size() {
        let capsule = Capsule::new_y(0.55, 0.35);
        let dispatcher = dispatcher();
        for half in [0.5f32, 5.0, 25.0, 500.0, 5000.0] {
            let cuboid = Cuboid::new(Vector::new(half, 0.5, half));
            let box_pos = Pose::from_translation(Vector::new(0.0, -0.5, 0.0));
            for off in [0.0f32, 0.002, 0.01, 0.37] {
                for gap in [0.3f32, 0.05, 0.021, 0.0199, 0.01, 0.0, -0.01] {
                    let at = Pose::from_translation(Vector::new(off * 0.9, 0.9 + gap, off));
                    // Capsule first, as Parry's `cast_shapes` would, and box first, as Rapier's
                    // query pipeline does.
                    let opts = options(0.5, 0.02);
                    let down = Vector::new(0.0, -1.0, 0.0);
                    let first = dispatcher
                        .cast_shapes(&at.inv_mul(&box_pos), -down, &capsule, &cuboid, opts)
                        .unwrap()
                        .expect("capsule-first cast missed");
                    let second = dispatcher
                        .cast_shapes(&box_pos.inv_mul(&at), down, &cuboid, &capsule, opts)
                        .unwrap()
                        .expect("box-first cast missed");
                    let want = (gap - 0.02).max(0.0);
                    for hit in [first, second.swapped()] {
                        assert!(
                            (hit.time_of_impact - want).abs() < 1e-4,
                            "half {half} off {off} gap {gap}: toi {} want {want}",
                            hit.time_of_impact
                        );
                        assert!(hit.normal1.y < -0.999, "{hit:?}");
                        assert!((hit.witness1.y - -0.9).abs() < 1e-3, "{hit:?}");
                    }
                    let c = dispatcher
                        .contact(&at.inv_mul(&box_pos), &capsule, &cuboid, 1.0)
                        .unwrap()
                        .unwrap();
                    assert!((c.dist - gap).abs() < 1e-4, "{c:?} want {gap}");
                    assert!(c.normal1.y < -0.999, "{c:?}");
                }
            }
        }
    }

    /// Away from the degenerate spots, the exact queries agree with Parry's GJK (to its own
    /// tolerance: 2.4 m apart it can be 1.5 mm off, where the exact distance matches sampling
    /// the segment a million times).
    #[test]
    fn exact_queries_agree_with_gjk_on_ordinary_poses() {
        let capsule = Capsule::new_y(0.4, 0.3);
        let cuboid = Cuboid::new(Vector::new(1.0, 0.25, 2.0));
        let exact = dispatcher();
        let mut checked = 0;
        for i in 0..200 {
            let k = i as f32;
            let box_pos = Pose::from_parts(
                Vector::new(0.3 * (k * 0.7).sin(), 0.1 * k.cos(), 0.0),
                Quat::from_euler(
                    EulerRot::XYZ,
                    (k * 0.37).sin(),
                    k * 0.11,
                    (k * 0.53).cos() * 0.6,
                ),
            );
            let at = Pose::from_translation(Vector::new(
                2.5 * (k * 1.3).sin(),
                1.5 + (k * 0.9).cos(),
                2.5 * (k * 0.4).cos(),
            ));
            let vel = -at.translation.normalize() + Vector::new(0.1 * k.sin(), 0.0, 0.1);
            let opts = options(10.0, 0.0);
            let pos12 = at.inv_mul(&box_pos);
            let ours = exact
                .cast_shapes(&pos12, -vel, &capsule, &cuboid, opts)
                .unwrap();
            let gjk = query::cast_shapes(&at, vel, &capsule, &box_pos, Vector::ZERO, &cuboid, opts)
                .unwrap();
            match (ours, gjk) {
                (Some(a), Some(b)) => {
                    assert!(
                        (a.time_of_impact - b.time_of_impact).abs() < 3e-3,
                        "{i}: {a:?} vs {b:?}"
                    );
                    assert!(a.normal1.dot(b.normal1) > 0.99, "{i}: {a:?} vs {b:?}");
                    checked += 1;
                }
                (None, None) => {}
                (a, b) => panic!("{i}: {a:?} vs {b:?}"),
            }
            let ours = exact.contact(&pos12, &capsule, &cuboid, 10.0).unwrap();
            let gjk = query::contact(&at, &capsule, &box_pos, &cuboid, 10.0).unwrap();
            if let (Some(a), Some(b)) = (ours, gjk) {
                assert!((a.dist - b.dist).abs() < 3e-3, "{i}: {a:?} vs {b:?}");
                assert!(a.normal1.dot(b.normal1) > 0.99, "{i}: {a:?} vs {b:?}");
            }
        }
        assert!(checked > 50, "only {checked} hits");
    }
}
