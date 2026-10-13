//! Moving platforms. The physics backend carries a character standing on a moving body (a
//! turntable, a train car, a lift); movement keeps the deck's motion when the character leaves
//! it, so a jump from a moving train keeps the train's speed, and takes it off again on landing,
//! so a jump straight up comes down on the same spot of the deck. The character also turns with
//! a turning deck.
//!
//! The deck's velocity under the character comes from how the body's `Transform` moved between
//! steps, so it works however the platform is driven (`Velocity`, animation, scripts). Colliders
//! should sit on the moving entity itself, not on a child of it.

use engine_core::math::{DVec3, Quat, Vec3};
use engine_core::prelude::*;
use engine_world::WorldShift;

use crate::mover::{Mover, flat};

/// The body a character stands on, followed from step to step.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Platform {
    pub entity: Entity,
    /// Velocity of the deck under the character, m/s in the character's own seconds.
    pub velocity: Vec3,
    translation: DVec3,
    rotation: Quat,
    /// `velocity` is known (from a second step on the deck, or from the body's `Velocity`).
    known: bool,
    /// Came down onto the deck from the air and still carries the world's frame: take the
    /// deck's velocity off as soon as it is known.
    landing: bool,
}

impl Platform {
    pub(crate) fn shift(&mut self, s: &WorldShift) {
        self.translation = s.point(self.translation);
        self.rotation = (s.rotation.as_quat() * self.rotation).normalize();
        self.velocity = s.direction(self.velocity);
    }
}

/// What the character stands on this step: the body, its pose now, and its velocity at the
/// character's feet if the body reports one (m/s, own seconds).
pub(crate) struct Deck<'a> {
    pub entity: Entity,
    pub transform: &'a Transform,
    pub velocity: Option<Vec3>,
}

/// Follow the deck under the character (before the step), moving `m.velocity` between the
/// world's frame and the deck's as the character steps on or off. `pos` is the character's
/// position now; `dt` is its own step.
pub(crate) fn ride(m: &mut Mover, deck: Option<Deck>, pos: DVec3, dt: f32) {
    let on = deck.as_ref().map(|d| d.entity);
    if let Some(p) = m.platform
        && Some(p.entity) != on
    {
        // Left it (jumped, fell, walked off, or stepped onto something else).
        if p.known {
            m.velocity += p.velocity;
        }
        m.platform = None;
    }
    let Some(d) = deck else {
        return;
    };
    let t = d.transform;
    match &mut m.platform {
        Some(p) => {
            // The point of the deck under the character now: where it was last step.
            let local = t.rotation.inverse() * (pos - t.translation).as_vec3();
            let before = p.translation + (p.rotation * local).as_dvec3();
            let velocity = ((pos - before) / dt.max(1e-6) as f64).as_vec3();
            if p.landing {
                // Landed last step without knowing how fast the deck moves: take it off now.
                m.velocity -= velocity;
                p.landing = false;
            }
            let turn = t.rotation * p.rotation.inverse();
            if let Some(f) = flat(turn * m.facing, m.up).try_normalize() {
                m.facing = f;
            }
            p.velocity = velocity;
            p.translation = t.translation;
            p.rotation = t.rotation;
            p.known = true;
        }
        None => {
            // From the air, the character moves in the world's frame and lands relative to the
            // deck. Walking or spawning onto it, it is carried along as it is.
            let landing = m.air_secs > 0.0;
            if landing && let Some(v) = d.velocity {
                m.velocity -= v;
            }
            m.platform = Some(Platform {
                entity: d.entity,
                velocity: d.velocity.unwrap_or(Vec3::ZERO),
                translation: t.translation,
                rotation: t.rotation,
                known: d.velocity.is_some(),
                landing: landing && d.velocity.is_none(),
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const DT: f32 = 1.0 / 60.0;

    fn deck(e: Entity, t: &Transform) -> Option<Deck<'_>> {
        Some(Deck {
            entity: e,
            transform: t,
            velocity: None,
        })
    }

    #[test]
    fn a_jump_keeps_the_decks_speed_and_lands_on_the_same_spot() {
        let e = Entity::from_raw_u32(7).unwrap();
        let mut m = Mover::default();
        let mut t = Transform::from_xyz(0.0, 0.0, 0.0);
        let speed = DVec3::new(12.0, 0.0, 0.0);
        // Riding: physics carries the character, its own velocity is relative (zero).
        let mut pos = DVec3::new(1.0, 1.0, 0.0);
        for _ in 0..5 {
            ride(&mut m, deck(e, &t), pos, DT);
            assert!(m.velocity.length() < 1e-4);
            t.translation += speed * DT as f64;
            pos += speed * DT as f64;
        }
        // Jump: off the deck, the deck's speed is kept.
        m.velocity += Vec3::Y * 5.0;
        ride(&mut m, None, pos, DT);
        assert!(
            m.velocity.distance(Vec3::new(12.0, 5.0, 0.0)) < 1e-3,
            "{}",
            m.velocity
        );
        // Back down on the same deck: the landing step doesn't know its speed yet...
        m.velocity.y = -5.0;
        m.air_secs = 0.6;
        ride(&mut m, deck(e, &t), pos, DT);
        t.translation += speed * DT as f64;
        pos += speed * DT as f64;
        // ...the next one does, and the character is still relative to the deck.
        ride(&mut m, deck(e, &t), pos, DT);
        assert!(
            m.velocity.distance(Vec3::new(0.0, -5.0, 0.0)) < 1e-3,
            "{}",
            m.velocity
        );
    }

    #[test]
    fn a_turning_deck_turns_the_character_and_flings_it_along_the_rim() {
        let e = Entity::from_raw_u32(3).unwrap();
        let mut m = Mover {
            facing: Vec3::NEG_Z,
            ..Default::default()
        };
        let mut t = Transform::default();
        let spin = 0.5_f32; // rad/s about +Y
        let mut pos = DVec3::new(4.0, 0.3, 0.0);
        for _ in 0..30 {
            ride(&mut m, deck(e, &t), pos, DT);
            let q = Quat::from_rotation_y(spin * DT);
            t.rotation = q * t.rotation;
            pos = (q * pos.as_vec3()).as_dvec3();
        }
        ride(&mut m, deck(e, &t), pos, DT);
        // Half a second at 0.5 rad/s: facing turned by about 0.25 rad.
        let turned = Quat::from_rotation_y(spin * DT * 30.0) * Vec3::NEG_Z;
        assert!(m.facing.distance(turned) < 0.02, "{}", m.facing);
        // Stepping off at 4 m from the axis: 2 m/s along the rim.
        ride(&mut m, None, pos, DT);
        let rim = Vec3::Y.cross(pos.as_vec3()).normalize() * spin * 4.0;
        assert!(m.velocity.distance(rim) < 0.05, "{} vs {rim}", m.velocity);
    }

    #[test]
    fn a_body_that_reports_its_velocity_is_taken_off_on_the_landing_step() {
        let e = Entity::from_raw_u32(9).unwrap();
        let t = Transform::default();
        let mut m = Mover {
            velocity: Vec3::new(8.0, -3.0, 0.0),
            air_secs: 0.4,
            ..Default::default()
        };
        let d = Deck {
            entity: e,
            transform: &t,
            velocity: Some(Vec3::new(8.0, 0.0, 0.0)),
        };
        ride(&mut m, Some(d), DVec3::ZERO, DT);
        assert!(m.velocity.distance(Vec3::new(0.0, -3.0, 0.0)) < 1e-5);
    }

    #[test]
    fn standing_still_on_static_ground_changes_nothing() {
        let e = Entity::from_raw_u32(1).unwrap();
        let t = Transform::from_xyz(5.0, 0.0, 5.0);
        let mut m = Mover {
            velocity: Vec3::new(3.0, 0.0, 1.0),
            ..Default::default()
        };
        for _ in 0..3 {
            ride(&mut m, deck(e, &t), DVec3::new(5.0, 1.0, 5.0), DT);
        }
        ride(&mut m, None, DVec3::new(5.0, 2.0, 5.0), DT);
        assert!(m.velocity.distance(Vec3::new(3.0, 0.0, 1.0)) < 1e-6);
    }
}
