//! Rig maths: bone transforms, two-bone IK and a few curves.

use engine_core::math::{Mat3, Quat, Vec3};

/// A bone's placement (rotation then translation) in its walker's root space.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Xf {
    pub r: Quat,
    pub t: Vec3,
}

impl Default for Xf {
    fn default() -> Self {
        Self::IDENTITY
    }
}

impl Xf {
    pub const IDENTITY: Xf = Xf {
        r: Quat::IDENTITY,
        t: Vec3::ZERO,
    };

    pub fn new(r: Quat, t: Vec3) -> Self {
        Self { r, t }
    }

    /// `self` followed by a child placed at `o` in `self`'s frame.
    pub fn mul(self, o: Xf) -> Xf {
        Xf {
            r: self.r * o.r,
            t: self.t + self.r * o.t,
        }
    }

    pub fn point(self, p: Vec3) -> Vec3 {
        self.t + self.r * p
    }

    /// The placement that undoes this one.
    pub fn inverse(self) -> Xf {
        let r = self.r.inverse();
        Xf {
            r,
            t: -(r * self.t),
        }
    }
}

/// Rotation whose local −Y runs along `down` and whose local −Z leans toward `front`.
pub fn aim_down(down: Vec3, front: Vec3) -> Quat {
    let y = -down.normalize_or(Vec3::NEG_Y);
    let mut z = -(front - y * y.dot(front));
    if z.length_squared() < 1e-8 {
        z = y.any_orthonormal_vector();
    }
    let z = z.normalize();
    let x = y.cross(z);
    Quat::from_mat3(&Mat3::from_cols(x, y, z)).normalize()
}

/// Two-bone IK: the middle joint for a chain from `root` toward `target` with bone lengths
/// `a` and `b`, bending toward `pole`. Returns the joint and the end point actually reached
/// (the target, pulled in if it is out of reach).
pub fn two_bone(root: Vec3, target: Vec3, a: f32, b: f32, pole: Vec3) -> (Vec3, Vec3) {
    let to = target - root;
    let dist = to.length().max(1e-5);
    let dir = to / dist;
    let d = dist.clamp((a - b).abs() + 1e-4, (a + b) * 0.9999);
    let along = (a * a - b * b + d * d) / (2.0 * d);
    let h = (a * a - along * along).max(0.0).sqrt();
    let mut bend = pole - dir * dir.dot(pole);
    if bend.length_squared() < 1e-8 {
        bend = dir.any_orthonormal_vector();
    }
    let bend = bend.normalize();
    let joint = root + dir * along + bend * h;
    (joint, root + dir * d)
}

/// Piecewise Catmull-Rom through evenly spaced `keys` over 0..=1.
pub fn curve(keys: &[f32], t: f32) -> f32 {
    let n = keys.len();
    if n == 1 {
        return keys[0];
    }
    let x = t.clamp(0.0, 1.0) * (n - 1) as f32;
    let i = (x.floor() as usize).min(n - 2);
    let f = x - i as f32;
    let k = |j: isize| keys[j.clamp(0, n as isize - 1) as usize];
    let (a, b, c, d) = (
        k(i as isize - 1),
        k(i as isize),
        k(i as isize + 1),
        k(i as isize + 2),
    );
    let f2 = f * f;
    let f3 = f2 * f;
    0.5 * (2.0 * b
        + (-a + c) * f
        + (2.0 * a - 5.0 * b + 4.0 * c - d) * f2
        + (-a + 3.0 * b - 3.0 * c + d) * f3)
}

/// Smooth minimum (never above `min(a, b)`), blending over `k`.
pub fn soft_min(a: f32, b: f32, k: f32) -> f32 {
    let h = 0.5 + 0.5 * (b - a) / k;
    if h >= 1.0 {
        // Also keeps `b = f32::MAX` (no limit) from cancelling `a` away.
        a
    } else if h <= 0.0 {
        b
    } else {
        b + (a - b) * h - k * h * (1.0 - h)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn two_bone_reaches_and_bends_toward_the_pole() {
        let (j, end) = two_bone(
            Vec3::ZERO,
            Vec3::new(0.0, -0.8, -0.1),
            0.45,
            0.43,
            Vec3::NEG_Z,
        );
        assert!(end.distance(Vec3::new(0.0, -0.8, -0.1)) < 1e-4);
        assert!((j.length() - 0.45).abs() < 1e-4);
        assert!((j.distance(end) - 0.43).abs() < 1e-4);
        assert!(j.z < -0.05, "knee forward: {j}");
        // Out of reach: straight toward the target.
        let (_, end) = two_bone(
            Vec3::ZERO,
            Vec3::new(0.0, -2.0, 0.0),
            0.45,
            0.43,
            Vec3::NEG_Z,
        );
        assert!((end.length() - 0.88).abs() < 1e-3);
    }

    #[test]
    fn aim_down_points_the_bone() {
        let q = aim_down(Vec3::new(0.0, -1.0, -1.0), Vec3::NEG_Z);
        assert!((q * Vec3::NEG_Y).distance(Vec3::new(0.0, -1.0, -1.0).normalize()) < 1e-5);
        assert!((q * Vec3::NEG_Z).z < 0.0);
        assert!((q * Vec3::X).distance(Vec3::X) < 1e-5);
    }

    #[test]
    fn soft_min_never_exceeds_min() {
        for (a, b) in [(0.0, 0.0), (0.1, 0.0), (0.0, 0.3), (-1.0, 2.0)] {
            assert!(soft_min(a, b, 0.02) <= a.min(b) + 1e-6);
        }
        assert!((soft_min(0.0, 1.0, 0.02)).abs() < 1e-6);
        assert_eq!(soft_min(0.4, f32::MAX, 0.03), 0.4);
    }

    #[test]
    fn curve_passes_through_keys() {
        let k = [0.0, 1.0, 0.5, 2.0];
        for (i, v) in k.iter().enumerate() {
            assert!((curve(&k, i as f32 / 3.0) - v).abs() < 1e-5);
        }
    }
}
