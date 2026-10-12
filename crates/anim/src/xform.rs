//! Bone transforms: translation, rotation, scale with the composition rules animation needs.

use engine_core::math::{Mat4, Quat, Vec3};
use engine_world::animation::BonePose;
use serde::{Deserialize, Serialize};

/// A bone transform (local to its parent, or in model space). Meters and radians.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Xf {
    pub t: Vec3,
    pub r: Quat,
    pub s: Vec3,
}

impl Default for Xf {
    fn default() -> Self {
        Self::IDENTITY
    }
}

impl Xf {
    pub const IDENTITY: Xf = Xf {
        t: Vec3::ZERO,
        r: Quat::IDENTITY,
        s: Vec3::ONE,
    };

    pub fn from_t(t: Vec3) -> Self {
        Self {
            t,
            ..Self::IDENTITY
        }
    }

    pub fn from_tr(t: Vec3, r: Quat) -> Self {
        Self { t, r, s: Vec3::ONE }
    }

    /// `self ∘ child`: the child's transform expressed in this transform's space.
    #[inline]
    pub fn mul(&self, child: &Xf) -> Xf {
        Xf {
            t: self.t + self.r * (self.s * child.t),
            r: (self.r * child.r).normalize(),
            s: self.s * child.s,
        }
    }

    /// Inverse (exact for uniform scale, which is what skeletons use).
    #[inline]
    pub fn inverse(&self) -> Xf {
        let r = self.r.conjugate();
        let s = Vec3::ONE / self.s;
        Xf {
            t: -(s * (r * self.t)),
            r,
            s,
        }
    }

    #[inline]
    pub fn transform_point(&self, p: Vec3) -> Vec3 {
        self.t + self.r * (self.s * p)
    }

    #[inline]
    pub fn transform_vector(&self, v: Vec3) -> Vec3 {
        self.r * (self.s * v)
    }

    /// Blend toward `other` by `w` (0 = self, 1 = other); rotations take the shortest path.
    #[inline]
    pub fn lerp(&self, other: &Xf, w: f32) -> Xf {
        Xf {
            t: self.t.lerp(other.t, w),
            r: nlerp(self.r, other.r, w),
            s: self.s.lerp(other.s, w),
        }
    }

    pub fn to_mat4(&self) -> Mat4 {
        Mat4::from_scale_rotation_translation(self.s, self.r, self.t)
    }

    pub fn from_mat4(m: &Mat4) -> Xf {
        let (s, r, t) = m.to_scale_rotation_translation();
        Xf { t, r, s }
    }

    pub fn to_bone_pose(&self) -> BonePose {
        BonePose {
            translation: self.t,
            rotation: self.r,
            scale: self.s,
        }
    }

    pub fn from_bone_pose(b: &BonePose) -> Xf {
        Xf {
            t: b.translation,
            r: b.rotation,
            s: b.scale,
        }
    }
}

/// Normalized linear quaternion interpolation along the shortest arc. Cheap and accurate
/// enough for blending animation poses.
#[inline]
pub fn nlerp(a: Quat, b: Quat, w: f32) -> Quat {
    let b = if a.dot(b) < 0.0 { -b } else { b };
    let v = a.to_array();
    let u = b.to_array();
    let q = Quat::from_xyzw(
        v[0] + (u[0] - v[0]) * w,
        v[1] + (u[1] - v[1]) * w,
        v[2] + (u[2] - v[2]) * w,
        v[3] + (u[3] - v[3]) * w,
    );
    let len = q.length();
    if len > 1e-8 { q / len } else { a }
}

/// Signed rotation angle of `q` about the unit `axis` (swing–twist decomposition), radians.
#[inline]
pub fn twist_angle(q: Quat, axis: Vec3) -> f32 {
    let v = Vec3::new(q.x, q.y, q.z);
    let p = v.dot(axis);
    let a = 2.0 * p.atan2(q.w);
    // Wrap to (-π, π].
    if a > std::f32::consts::PI {
        a - std::f32::consts::TAU
    } else if a < -std::f32::consts::PI {
        a + std::f32::consts::TAU
    } else {
        a
    }
}

/// Smallest rotation turning unit vector `from` onto unit vector `to`.
#[inline]
pub fn rotation_between(from: Vec3, to: Vec3) -> Quat {
    let d = from.dot(to);
    if d > 0.999_999 {
        return Quat::IDENTITY;
    }
    if d < -0.999_999 {
        let axis = from.any_orthonormal_vector();
        return Quat::from_axis_angle(axis, std::f32::consts::PI);
    }
    Quat::from_rotation_arc(from, to)
}

/// Exponential smoothing factor for a time constant (seconds): `x += (target - x) * k`.
#[inline]
pub fn smoothing(dt: f32, tau: f32) -> f32 {
    if tau <= 1e-6 {
        1.0
    } else {
        1.0 - (-dt / tau).exp()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mul_and_inverse_round_trip() {
        let a = Xf {
            t: Vec3::new(1.0, 2.0, 3.0),
            r: Quat::from_rotation_y(0.7) * Quat::from_rotation_x(0.3),
            s: Vec3::splat(1.5),
        };
        let p = Vec3::new(0.3, -0.2, 0.9);
        let back = a.inverse().transform_point(a.transform_point(p));
        assert!(back.abs_diff_eq(p, 1e-5), "{back:?}");
        let id = a.mul(&a.inverse());
        assert!(id.t.length() < 1e-5 && id.r.angle_between(Quat::IDENTITY) < 1e-4);
    }

    #[test]
    fn twist_reads_rotation_about_axis() {
        let q = Quat::from_axis_angle(Vec3::X, 0.8) * Quat::from_axis_angle(Vec3::Y, 0.2);
        let a = twist_angle(q, Vec3::X);
        assert!((a - 0.8).abs() < 0.05, "{a}");
        assert!((twist_angle(Quat::from_rotation_z(-1.2), Vec3::Z) + 1.2).abs() < 1e-5);
    }

    #[test]
    fn nlerp_takes_shortest_path() {
        let a = Quat::from_rotation_y(0.1);
        let b = -Quat::from_rotation_y(0.3);
        let m = nlerp(a, b, 0.5);
        assert!(m.angle_between(Quat::from_rotation_y(0.2)) < 1e-3);
    }
}
