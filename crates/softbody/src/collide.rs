//! Collision shapes as the solver sees them: signed distance functions in a moving frame.
//!
//! Every collider a soft body can touch (physics colliders, character capsules, bone capsules
//! on animated characters, the ground) becomes a [`LocalCollider`] in the body's own
//! coordinates for one fixed step. Its pose is interpolated from where it was at the start of
//! the step to where it is at the end, substep by substep, so a body that dashes at 30 m/s
//! sweeps through the cape instead of teleporting past it.

use std::sync::Arc;

use engine_core::math::{Quat, Vec3};

use crate::field::{DistanceField, HeightGrid};

/// Shape of a [`LocalCollider`], in the collider's own space (meters).
#[derive(Debug, Clone)]
pub enum LocalShape {
    Sphere {
        radius: f32,
    },
    /// Capsule along local Y: segment from `-half_height` to `+half_height`, plus `radius`.
    Capsule {
        half_height: f32,
        radius: f32,
    },
    /// Box with half extents.
    Cuboid {
        half: Vec3,
    },
    /// Cylinder along local Y.
    Cylinder {
        half_height: f32,
        radius: f32,
    },
    /// A triangle mesh baked into a distance grid.
    Field(Arc<DistanceField>),
    /// A terrain height grid.
    Height(Arc<HeightGrid>),
}

impl LocalShape {
    /// Radius of a sphere around the shape's center that contains it.
    pub fn bounding_radius(&self) -> f32 {
        match *self {
            LocalShape::Sphere { radius } => radius,
            LocalShape::Capsule {
                half_height,
                radius,
            } => half_height + radius,
            LocalShape::Cuboid { half } => half.length(),
            LocalShape::Cylinder {
                half_height,
                radius,
            } => (half_height * half_height + radius * radius).sqrt(),
            LocalShape::Field(ref f) => f.bounding_radius(),
            LocalShape::Height(ref g) => g.bounding_radius(),
        }
    }

    /// Half extents of a box around the shape's center, in shape space.
    pub fn half_extents(&self) -> Vec3 {
        match *self {
            LocalShape::Sphere { radius } => Vec3::splat(radius),
            LocalShape::Capsule {
                half_height,
                radius,
            } => Vec3::new(radius, half_height + radius, radius),
            LocalShape::Cuboid { half } => half,
            LocalShape::Cylinder {
                half_height,
                radius,
            } => Vec3::new(radius, half_height, radius),
            LocalShape::Field(_) | LocalShape::Height(_) => Vec3::splat(self.bounding_radius()),
        }
    }

    /// Signed distance from `p` (shape space) to the surface, and the outward normal there.
    /// Negative inside. Grids answer infinity where they know nothing (far from the surface).
    pub fn sdf(&self, p: Vec3) -> (f32, Vec3) {
        match *self {
            LocalShape::Sphere { radius } => {
                let len = p.length();
                let n = if len > 1e-9 { p / len } else { Vec3::Y };
                (len - radius, n)
            }
            LocalShape::Capsule {
                half_height,
                radius,
            } => {
                let q = Vec3::new(p.x, p.y - p.y.clamp(-half_height, half_height), p.z);
                let len = q.length();
                let n = if len > 1e-9 {
                    q / len
                } else {
                    // On the axis: push out sideways, any direction works.
                    Vec3::X
                };
                (len - radius, n)
            }
            LocalShape::Cuboid { half } => {
                let q = p.abs() - half;
                if q.x > 0.0 || q.y > 0.0 || q.z > 0.0 {
                    let outside = q.max(Vec3::ZERO);
                    let d = outside.length();
                    let n = (outside * p.signum()) / d.max(1e-9);
                    (d, n)
                } else if q.x >= q.y && q.x >= q.z {
                    (q.x, Vec3::X * p.x.signum())
                } else if q.y >= q.z {
                    (q.y, Vec3::Y * p.y.signum())
                } else {
                    (q.z, Vec3::Z * p.z.signum())
                }
            }
            LocalShape::Cylinder {
                half_height,
                radius,
            } => {
                let rxz = (p.x * p.x + p.z * p.z).sqrt();
                let radial = if rxz > 1e-9 {
                    Vec3::new(p.x / rxz, 0.0, p.z / rxz)
                } else {
                    Vec3::X
                };
                let dx = rxz - radius;
                let dy = p.y.abs() - half_height;
                let up = Vec3::Y * p.y.signum();
                if dx > 0.0 && dy > 0.0 {
                    let d = (dx * dx + dy * dy).sqrt();
                    (d, (radial * dx + up * dy) / d)
                } else if dx > dy {
                    (dx, radial)
                } else {
                    (dy, up)
                }
            }
            LocalShape::Field(ref f) => f.sample(p).unwrap_or((f32::INFINITY, Vec3::ZERO)),
            LocalShape::Height(ref g) => g.sample(p).unwrap_or((f32::INFINITY, Vec3::ZERO)),
        }
    }
}

/// One collider in a soft body's coordinates for one fixed step.
#[derive(Debug, Clone)]
pub struct LocalCollider {
    pub shape: LocalShape,
    /// Pose at the start of the step (body-local position, world rotation).
    pub from_pos: Vec3,
    pub from_rot: Quat,
    /// Pose at the end of the step.
    pub to_pos: Vec3,
    pub to_rot: Quat,
    /// Coulomb friction coefficient against cloth and flesh.
    pub friction: f32,
    /// Index into the world collider list, for reaction impulses.
    pub id: u32,
    /// Whether reaction impulses are collected for this collider (dynamic rigid bodies).
    pub dynamic: bool,
}

impl LocalCollider {
    /// A collider that doesn't move during the step.
    pub fn fixed(shape: LocalShape, pos: Vec3, rot: Quat) -> Self {
        Self {
            shape,
            from_pos: pos,
            from_rot: rot,
            to_pos: pos,
            to_rot: rot,
            friction: 0.5,
            id: u32::MAX,
            dynamic: false,
        }
    }

    /// Center and radius of a sphere containing the collider over the whole step.
    pub fn swept_bounds(&self) -> (Vec3, f32) {
        let center = (self.from_pos + self.to_pos) * 0.5;
        let r = self.shape.bounding_radius() + (self.to_pos - self.from_pos).length() * 0.5;
        (center, r)
    }
}

/// A collider posed at one instant of a substep, ready for particle tests against its
/// [`LocalCollider`]'s shape.
#[derive(Debug, Clone, Copy)]
pub(crate) struct PosedCollider {
    pub pos: Vec3,
    pub rot: Quat,
    pub inv_rot: Quat,
    pub bound2: f32,
    /// Linear velocity over the step, m/s.
    pub lin_vel: Vec3,
    /// Angular velocity over the step, rad/s.
    pub ang_vel: Vec3,
    pub friction: f32,
}

impl PosedCollider {
    pub(crate) fn at(c: &LocalCollider, t: f32, dt: f32, margin: f32) -> Self {
        let pos = c.from_pos.lerp(c.to_pos, t);
        let rot = c.from_rot.slerp(c.to_rot, t).normalize();
        let inv_dt = if dt > 0.0 { 1.0 / dt } else { 0.0 };
        let delta = c.to_rot * c.from_rot.conjugate();
        let (axis, mut angle) = delta.to_axis_angle();
        if angle > std::f32::consts::PI {
            angle -= std::f32::consts::TAU;
        }
        let r = c.shape.bounding_radius() + margin;
        Self {
            pos,
            rot,
            inv_rot: rot.conjugate(),
            bound2: r * r,
            lin_vel: (c.to_pos - c.from_pos) * inv_dt,
            ang_vel: if angle.abs() > 1e-6 {
                axis * (angle * inv_dt)
            } else {
                Vec3::ZERO
            },
            friction: c.friction,
        }
    }

    /// Signed distance from `shape` posed here and world normal at body-local point `p`, or
    /// `None` when the point is clearly outside the bounding sphere.
    #[inline]
    pub(crate) fn query(&self, shape: &LocalShape, p: Vec3) -> Option<(f32, Vec3)> {
        let rel = p - self.pos;
        if rel.length_squared() > self.bound2 {
            return None;
        }
        let (d, n) = shape.sdf(self.inv_rot * rel);
        Some((d, self.rot * n))
    }

    /// Velocity of the collider's surface at body-local point `p`.
    #[inline]
    pub(crate) fn velocity_at(&self, p: Vec3) -> Vec3 {
        self.lin_vel + self.ang_vel.cross(p - self.pos)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn check_outward(shape: &LocalShape, p: Vec3, expect_d: f32, expect_n: Vec3) {
        let (d, n) = shape.sdf(p);
        assert!(
            (d - expect_d).abs() < 1e-4,
            "{shape:?} at {p}: d {d} != {expect_d}"
        );
        assert!(
            n.abs_diff_eq(expect_n, 1e-4),
            "{shape:?} at {p}: n {n} != {expect_n}"
        );
    }

    #[test]
    fn signed_distances_are_correct() {
        let s = LocalShape::Sphere { radius: 1.0 };
        check_outward(&s, Vec3::new(0.0, 2.0, 0.0), 1.0, Vec3::Y);
        check_outward(&s, Vec3::new(0.5, 0.0, 0.0), -0.5, Vec3::X);

        let c = LocalShape::Capsule {
            half_height: 1.0,
            radius: 0.5,
        };
        check_outward(&c, Vec3::new(1.0, 0.5, 0.0), 0.5, Vec3::X);
        check_outward(&c, Vec3::new(0.0, 2.0, 0.0), 0.5, Vec3::Y);

        let b = LocalShape::Cuboid {
            half: Vec3::new(1.0, 2.0, 3.0),
        };
        check_outward(&b, Vec3::new(0.0, 3.0, 0.0), 1.0, Vec3::Y);
        check_outward(&b, Vec3::new(0.9, 0.0, 0.0), -0.1, Vec3::X);
        check_outward(&b, Vec3::new(0.0, 0.0, -2.5), -0.5, Vec3::NEG_Z);
        let (d, _) = b.sdf(Vec3::new(2.0, 3.0, 0.0));
        assert!((d - 2f32.sqrt()).abs() < 1e-4);

        let y = LocalShape::Cylinder {
            half_height: 1.0,
            radius: 1.0,
        };
        check_outward(&y, Vec3::new(2.0, 0.0, 0.0), 1.0, Vec3::X);
        check_outward(&y, Vec3::new(0.0, -1.5, 0.0), 0.5, Vec3::NEG_Y);
        check_outward(&y, Vec3::new(0.0, 0.8, 0.0), -0.2, Vec3::Y);
    }

    #[test]
    fn posed_collider_reports_rotation_velocity() {
        let c = LocalCollider {
            shape: LocalShape::Sphere { radius: 1.0 },
            from_pos: Vec3::ZERO,
            from_rot: Quat::IDENTITY,
            to_pos: Vec3::new(1.0, 0.0, 0.0),
            to_rot: Quat::from_rotation_y(0.1),
            friction: 0.5,
            id: 0,
            dynamic: false,
        };
        let p = PosedCollider::at(&c, 0.5, 0.1, 0.0);
        assert!(p.pos.abs_diff_eq(Vec3::new(0.5, 0.0, 0.0), 1e-5));
        assert!(p.lin_vel.abs_diff_eq(Vec3::new(10.0, 0.0, 0.0), 1e-4));
        assert!((p.ang_vel.y - 1.0).abs() < 1e-3, "{:?}", p.ang_vel);
    }
}
