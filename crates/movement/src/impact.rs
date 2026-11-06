//! The impact bus: every contact that should be felt (a landing, a catch, a pick biting ice,
//! a kick off a wall, a bounce) sends one [`Impact`], and everything that sells weight reacts to
//! it from the same number, so every new move feels heavy without its own effects code
//! (research: Sakurai's hitstop, Eiserloh's trauma shake, Titanfall's landing view spring).
//!
//! - the camera ([`crate::fx::CameraFx`]): trauma (rotational shake = trauma²), a landing dip
//!   on a spring, a field-of-view kick on launches;
//! - the puppet (`look::rig::Animator::impact`): a short body-only hitstop on heavy contacts
//!   and a compression pushed into the pelvis spring;
//! - dust, chips and straw ([`crate::look::effects`]);
//! - sound reads the [`AnimationTrigger`](engine_world::animation::AnimationTrigger)s the moves
//!   already send.

use engine_core::math::{DVec3, Vec3};
use engine_core::prelude::*;

/// What kind of contact.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum ImpactKind {
    /// Feet on the ground after a flight.
    Land,
    /// Came down hard (crouch landing, hands to the ground).
    HardLand,
    /// Came down rolling (crouch timed before contact).
    Roll,
    /// Hands caught a ledge, a bar, a rope or a pole.
    Catch,
    /// Landed on a perch, a beam or a line.
    Perch,
    /// An ice pick bit.
    Pick,
    /// A hand or foot planted on a climbing hold.
    Hold,
    /// A foot kicked off a wall (wall kick, run-up step).
    Kick,
    /// Bounced off an awning.
    Bounce,
    /// Landed in hay.
    Hay,
    /// A takeoff from a perch, a bar or a rope with good timing.
    Launch,
    /// Dropped onto someone from above.
    Takedown,
}

/// One felt contact. See the module docs.
#[derive(Message, Debug, Clone, Copy, PartialEq)]
pub struct Impact {
    /// Who made it.
    pub entity: Entity,
    pub kind: ImpactKind,
    /// Where, world.
    pub point: DVec3,
    /// Out of the surface hit.
    pub normal: Vec3,
    /// Speed into the surface, m/s.
    pub speed: f32,
}

impl Impact {
    /// How heavy, 0..1: what the camera, the puppet and the effects scale by.
    pub fn strength(&self) -> f32 {
        let s = self.speed.max(0.0);
        let k = match self.kind {
            ImpactKind::Land => (s - 4.0) / 10.0,
            ImpactKind::HardLand => 0.35 + (s - 7.0) / 12.0,
            ImpactKind::Roll => 0.25 + (s - 7.0) / 16.0,
            ImpactKind::Catch => 0.15 + s / 20.0,
            ImpactKind::Perch => 0.1 + s / 18.0,
            ImpactKind::Pick => 0.22,
            ImpactKind::Hold => 0.05 + s / 30.0,
            ImpactKind::Kick => 0.1 + s / 25.0,
            ImpactKind::Bounce => 0.15 + s / 25.0,
            ImpactKind::Hay => 0.2 + s / 30.0,
            ImpactKind::Launch => 0.15,
            ImpactKind::Takedown => 0.8,
        };
        k.clamp(0.0, 1.0)
    }

    /// Camera trauma to add (Eiserloh: shake = trauma²).
    pub fn trauma(&self) -> f32 {
        let k = self.strength();
        match self.kind {
            ImpactKind::Launch => 0.0,
            ImpactKind::Pick => 0.12,
            ImpactKind::Hold => k * 0.3,
            _ => k * 0.6,
        }
    }

    /// How far the view dips with it, m (a landing view spring, Titanfall-like).
    pub fn dip(&self) -> f32 {
        match self.kind {
            ImpactKind::Land
            | ImpactKind::HardLand
            | ImpactKind::Roll
            | ImpactKind::Perch
            | ImpactKind::Hay
            | ImpactKind::Bounce
            | ImpactKind::Takedown => 0.03 + 0.22 * self.strength(),
            ImpactKind::Catch => 0.04 + 0.1 * self.strength(),
            _ => 0.0,
        }
    }

    /// Body-only freeze of the puppet, seconds: heavy contacts only (30–60 ms), never the
    /// world or the camera.
    pub fn hitstop(&self) -> f32 {
        match self.kind {
            ImpactKind::Pick => 0.045,
            ImpactKind::Takedown => 0.08,
            ImpactKind::HardLand => 0.05,
            ImpactKind::Catch if self.speed > 6.0 => 0.04,
            ImpactKind::Land if self.speed > 9.0 => 0.04,
            _ => 0.0,
        }
    }

    /// Field-of-view kick, degrees (launches and bounces).
    pub fn fov_kick(&self) -> f32 {
        match self.kind {
            ImpactKind::Launch => 4.0,
            ImpactKind::Bounce => 3.0,
            ImpactKind::Takedown => -5.0,
            _ => 0.0,
        }
    }
}

/// A critically damped-ish spring step (semi-implicit), for the view's dip: `x` toward 0 at
/// angular frequency `w` and damping ratio `z`.
pub fn spring(x: &mut f32, v: &mut f32, w: f32, z: f32, dt: f32) {
    let a = -w * w * *x - 2.0 * z * w * *v;
    *v += a * dt;
    *x += *v * dt;
}

/// Smooth noise in -1..1 (sum of sines with odd periods): enough for a shake, no table.
pub fn shake_noise(t: f32, seed: f32) -> f32 {
    let a = (t * 23.1 + seed * 7.3).sin() * 0.5;
    let b = (t * 37.7 + seed * 3.1).sin() * 0.3;
    let c = (t * 61.3 + seed * 11.9).sin() * 0.2;
    a + b + c
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hit(kind: ImpactKind, speed: f32) -> Impact {
        Impact {
            entity: Entity::PLACEHOLDER,
            kind,
            point: DVec3::ZERO,
            normal: Vec3::Y,
            speed,
        }
    }

    #[test]
    fn harder_landings_are_felt_more() {
        let soft = hit(ImpactKind::Land, 4.0);
        let hard = hit(ImpactKind::Land, 10.0);
        assert!(soft.strength() < 0.05);
        assert!(hard.strength() > 0.5);
        assert!(hard.dip() > soft.dip() && hard.trauma() > soft.trauma());
        assert!(hard.hitstop() > 0.0 && soft.hitstop() == 0.0);
    }

    #[test]
    fn the_view_spring_settles() {
        let (mut x, mut v) = (-0.2, 0.0);
        for _ in 0..120 {
            spring(&mut x, &mut v, 18.0, 0.6, 1.0 / 60.0);
        }
        assert!(x.abs() < 1e-3 && v.abs() < 1e-2, "{x} {v}");
    }
}
