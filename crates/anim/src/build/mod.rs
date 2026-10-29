//! Procedural demo characters and vehicles: skeletons, meshes, muscles, rigs and clips.
//!
//! Everything here is generated from code (no art files), so the showcase works out of the
//! box and every piece doubles as an example of the API: [`humanoid`] (locomotion, heavy
//! swing, block, reach; muscles and clothing), [`horse`] (four-legged gait with flank
//! muscles), [`dog`] (a furred dog of any size with a rotary gallop), [`spider`] (a furred
//! tarantula on eight procedural legs) and [`car`] (suspension, steering, wheel spin), plus
//! [`rock`] boulders for the paddock.

pub mod car;
pub mod dog;
pub mod horse;
pub mod humanoid;
pub mod rock;
pub mod sdf;
pub mod spider;
pub mod weapons;

use engine_core::math::{EulerRot, Quat, Vec3};

use crate::skeleton::{BoneDef, Skeleton};
use crate::xform::Xf;

/// A bone with a rest rotation (`BoneDef::new` has none).
pub(crate) fn bone(name: &str, parent: Option<&str>, offset: Vec3, rotation: Quat) -> BoneDef {
    BoneDef {
        name: name.into(),
        parent: parent.map(Into::into),
        rest: Xf::from_tr(offset, rotation),
    }
}

/// Builds one key pose for [`AnimationClip::from_fn`](crate::clip::AnimationClip::from_fn):
/// rotations relative to each bone's rest orientation.
pub(crate) struct PoseBuilder<'a> {
    skeleton: &'a Skeleton,
    out: Vec<(&'static str, Xf)>,
}

impl<'a> PoseBuilder<'a> {
    pub fn new(skeleton: &'a Skeleton) -> Self {
        Self {
            skeleton,
            out: Vec::new(),
        }
    }

    fn rest(&self, bone: &str) -> Xf {
        let i = self
            .skeleton
            .index_of(bone)
            .unwrap_or_else(|| panic!("bone {bone} is not in skeleton {}", self.skeleton.name));
        self.skeleton.rest[i]
    }

    /// Rotate `bone` in its rest frame: degrees about X, Y and Z, applied as Z·X·Y (twist
    /// first, then bend, then swing sideways).
    pub fn rot(&mut self, bone: &'static str, x: f32, y: f32, z: f32) -> &mut Self {
        let rest = self.rest(bone);
        let q = Quat::from_euler(
            EulerRot::ZXY,
            z.to_radians(),
            x.to_radians(),
            y.to_radians(),
        );
        self.out.push((
            bone,
            Xf {
                t: Vec3::ZERO,
                r: rest.r * q,
                s: Vec3::ONE,
            },
        ));
        self
    }

    /// Rotate `bone` by `q` in its rest frame.
    pub fn turn(&mut self, bone: &'static str, q: Quat) -> &mut Self {
        let rest = self.rest(bone);
        self.out.push((
            bone,
            Xf {
                t: Vec3::ZERO,
                r: rest.r * q,
                s: Vec3::ONE,
            },
        ));
        self
    }

    /// Rotate and move `bone` (offset from its rest position, parent space).
    pub fn rot_move(
        &mut self,
        bone: &'static str,
        offset: Vec3,
        x: f32,
        y: f32,
        z: f32,
    ) -> &mut Self {
        let rest = self.rest(bone);
        let q = Quat::from_euler(
            EulerRot::ZXY,
            z.to_radians(),
            x.to_radians(),
            y.to_radians(),
        );
        let mut t = rest.t + offset;
        if t == Vec3::ZERO {
            // `from_fn` treats a zero translation as "no translation key".
            t = Vec3::splat(1e-7);
        }
        self.out.push((
            bone,
            Xf {
                t,
                r: rest.r * q,
                s: Vec3::ONE,
            },
        ));
        self
    }

    pub fn take(&mut self) -> Vec<(&'static str, Xf)> {
        std::mem::take(&mut self.out)
    }
}

/// Piecewise smooth interpolation through `(phase, value)` keys (sorted by phase).
pub(crate) fn keyed(phase: f32, keys: &[(f32, f32)]) -> f32 {
    let Some(first) = keys.first() else {
        return 0.0;
    };
    if phase <= first.0 {
        return first.1;
    }
    for w in keys.windows(2) {
        let (a, b) = (w[0], w[1]);
        if phase <= b.0 {
            let t = ((phase - a.0) / (b.0 - a.0).max(1e-6)).clamp(0.0, 1.0);
            let s = t * t * (3.0 - 2.0 * t);
            return a.1 + (b.1 - a.1) * s;
        }
    }
    keys.last().map_or(0.0, |k| k.1)
}

/// A smooth periodic bump at `center` (phase units, wraps at 1) of half-width `width`.
pub(crate) fn bump(phase: f32, center: f32, width: f32) -> f32 {
    let mut d = (phase - center).rem_euclid(1.0);
    if d > 0.5 {
        d -= 1.0;
    }
    (-(d / width.max(1e-4)).powi(2)).exp()
}

/// cos(2π·phase)
pub(crate) fn cycle(phase: f32) -> f32 {
    (phase * std::f32::consts::TAU).cos()
}

/// sin(2π·phase)
pub(crate) fn cycle_sin(phase: f32) -> f32 {
    (phase * std::f32::consts::TAU).sin()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keyed_and_bump_shapes() {
        let keys = [(0.0, 0.0), (0.5, 1.0), (1.0, 0.0)];
        assert_eq!(keyed(0.0, &keys), 0.0);
        assert!((keyed(0.5, &keys) - 1.0).abs() < 1e-6);
        assert!((keyed(0.25, &keys) - 0.5).abs() < 1e-6);
        assert!((bump(0.98, 0.02, 0.05) - bump(0.06, 0.02, 0.05)).abs() < 1e-5);
    }
}
