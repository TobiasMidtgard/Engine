//! Hanging ropes to climb and swing on (Assassin's Creed's ropes, Tomb Raider's swing):
//! a rope hangs from an anchor (a beam, a bracket, a bell tower's frame); jump into it and the
//! hands catch it.
//!
//! * **A rope** is an entity with a [`ClimbRope`] and a `Transform` at its anchor; it hangs
//!   [`ClimbRope::length`] straight down at rest. [`crate::look::climbrope`] draws it, bent
//!   where it is held.
//! * **Catching it.** An airborne character with
//!   [`ledge_grab`](MovementProfile::ledge_grab) whose hand (see [`zipline::hand`]) comes within
//!   [`GRAB_RADIUS`] of the rope catches it. Its speed across the rope becomes the rope's
//!   swing, so a running jump into a rope swings out on it.
//! * **Climbing.** The stick forward climbs ([`CLIMB_UP`]), back climbs down ([`CLIMB_DOWN`]);
//!   holding crouch slides down ([`SLIDE_DOWN`]). At the top, pushing up steps off onto a
//!   ledge in reach (the beam's roof). Off the bottom end, the climber drops.
//! * **Swinging.** The rope and climber swing as a pendulum about the anchor. Sideways on the
//!   stick, or any way with sprint held, pumps the swing.
//! * **Letting go.** Jump lets go with the swing's velocity and a hop, steered by the stick;
//!   a hard hit or something in the way tears the climber off. A rope let go of isn't caught
//!   again before the feet touch the ground.

use engine_core::math::{DVec3, Vec3};
use engine_core::prelude::*;
use engine_world::LocomotionMode;

use crate::locomotion::{Body, Outputs, Probe};
use crate::mover::{Mover, RopeHold, flat};
use crate::profile::MovementProfile;
use crate::zipline;

/// How close the hand must come to a rope to catch it, m.
pub const GRAB_RADIUS: f32 = 0.6;
/// The rope runs this far in front of the climber's axis, m (between the hands).
pub const ROPE_AHEAD: f32 = crate::ladder::RUNG_AHEAD;
/// Climbing speeds, m/s (Assassin's Creed and Uncharted climb ropes at about these).
pub const CLIMB_UP: f32 = 1.5;
pub const CLIMB_DOWN: f32 = 2.5;
/// Sliding down with crouch held, m/s.
pub const SLIDE_DOWN: f32 = 5.0;
/// How fast the climbing speed changes, m/s².
const CLIMB_ACCEL: f32 = 10.0;
/// Pumping the swing: acceleration of the hands along the stick, m/s².
const PUMP: f32 = 4.5;
/// Damping of the swing, 1/s.
const SWING_DAMPING: f32 = 0.25;
/// Widest swing from straight down, rad.
const MAX_SWING: f32 = 1.2;
/// A catch keeps this much of the speed across the rope as swing.
const CATCH_KEEP: f32 = 0.8;
/// Fastest pull of the body onto the rope after a catch or a bump, m/s.
const SNAP_SPEED: f32 = 6.0;
/// Held this far off where the body should hang: torn off, m.
const TORN_OFF: f32 = 1.2;
/// After letting go, no catching a rope for this long, s.
const REGRAB_SECS: f32 = 0.4;
/// Letting go hops up this high, m, and the stick adds this much speed, m/s.
const RELEASE_HOP: f32 = 0.6;
const RELEASE_PUSH: f32 = 3.0;

/// A rope hanging from this entity's translation (the anchor), `length` meters long.
#[derive(Component, Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[require(Transform)]
#[serde(default)]
pub struct ClimbRope {
    /// Anchor to free end, m.
    pub length: f32,
    /// The hands stop this far below the anchor (the knot, the beam's underside), m.
    pub top_clear: f32,
    /// Rope radius, m (for the look).
    pub radius: f32,
}

impl Default for ClimbRope {
    fn default() -> Self {
        Self {
            length: 8.0,
            top_clear: 0.35,
            radius: 0.022,
        }
    }
}

/// The rope's anchor in world space.
pub(crate) fn anchor(t: &Transform) -> DVec3 {
    t.translation
}

/// Where the body's center is with the hands `s` meters down a rope from `anchor` running
/// along `dir`, facing `facing`: under the hands along the rope, behind it.
pub fn body_at(anchor: DVec3, dir: Vec3, s: f32, facing: Vec3, up: Vec3, height: f32) -> DVec3 {
    let hands = anchor + (dir * s).as_dvec3();
    let back = flat(facing, up).normalize_or_zero() * ROPE_AHEAD;
    hands + (dir * (height * zipline::GRIP_ABOVE) - back).as_dvec3()
}

/// The rope step: climbs, swings or lets go of the rope held, or catches one in reach.
/// `None` leaves the step to the rest of the locomotion.
#[allow(clippy::too_many_arguments)]
pub(crate) fn step(
    m: &mut Mover,
    p: &MovementProfile,
    b: &Body,
    probe: &Probe,
    v: Vec3,
    wish: Vec3,
    held: Held,
    grounded: bool,
    dt: f32,
    out: &mut Outputs,
) -> Option<Vec3> {
    m.rope_cooldown = (m.rope_cooldown - dt).max(0.0);
    if grounded {
        m.rope_last = None;
    }
    if m.rope.is_none() {
        if !p.ledge_grab || grounded || m.rope_cooldown > 0.0 {
            return None;
        }
        catch(m, b, probe, v, out)?;
        m.air_jumps_left = p.air_jumps;
    }
    Some(hold(m, p, b, probe, v, wish, held, grounded, dt, out))
}

/// Inputs the rope reads this step.
#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct Held {
    pub jump_pressed: bool,
    pub crouch: bool,
    pub sprint: bool,
    pub knocked: bool,
    /// The wall the body is pressed against, if any (it stops the swing).
    pub wall: Option<Vec3>,
}

/// Catches the nearest rope in reach of the hand.
fn catch(m: &mut Mover, b: &Body, probe: &Probe, v: Vec3, out: &mut Outputs) -> Option<()> {
    let up = b.up;
    let hand = zipline::hand(b.pos, up, b.height);
    let mut best: Option<(Entity, DVec3, f32, f32)> = None;
    for (rope, at, r) in probe.climb_ropes() {
        if m.rope_last == Some(rope) {
            continue;
        }
        let s = (at - hand).dot(up.as_dvec3()) as f32;
        if s < r.top_clear || s > r.length - 0.2 {
            continue;
        }
        let d = flat((hand - at).as_vec3(), up).length();
        if d > GRAB_RADIUS || best.is_some_and(|(.., bd)| bd <= d) {
            continue;
        }
        best = Some((rope, at, s, d));
    }
    let (rope, at, s, _) = best?;
    let dir = (hand - at).as_vec3().try_normalize().unwrap_or(-up);
    // Across the rope, the run carries on as swing.
    let swing = flat(v, up) * CATCH_KEEP;
    m.rope = Some(RopeHold {
        rope: Some(rope),
        s,
        dir,
        swing: swing - dir * swing.dot(dir),
        climbed: 0.0,
        speed: 0.0,
        secs: 0.0,
    });
    m.set_mode(LocomotionMode::Climb);
    out.trigger(b.entity, "rope_grab");
    out.noise(b.entity, hand, 3.0);
    Some(())
}

/// One step on the rope held.
#[allow(clippy::too_many_arguments)]
fn hold(
    m: &mut Mover,
    p: &MovementProfile,
    b: &Body,
    probe: &Probe,
    v: Vec3,
    wish: Vec3,
    held: Held,
    grounded: bool,
    dt: f32,
    out: &mut Outputs,
) -> Vec3 {
    let up = b.up;
    let Some(mut r) = m.rope else {
        return v;
    };
    let Some((at, rope)) = r.rope.and_then(|e| probe.climb_rope(e)) else {
        let_go(m, b, out, "rope_release");
        return v;
    };
    let facing = flat(m.facing, up).normalize_or(Vec3::NEG_Z);
    let right = facing.cross(up);
    // The hands' velocity along the swing (the body follows them rigidly).
    let moving = r.swing;

    if held.knocked {
        let_go(m, b, out, "let_go");
        return v;
    }
    if held.jump_pressed {
        let_go(m, b, out, "rope_release");
        m.jump_buffer = 0.0;
        // Off with the swing, a hop and a push toward the stick.
        let launch = p.jump_speed(RELEASE_HOP, b.g);
        let v = flat(moving, up) + wish * RELEASE_PUSH + up * (moving.dot(up).max(0.0) + launch);
        m.air_cap = flat(v, up).length();
        return v;
    }

    // Climb (forward up, back down), or slide down with crouch.
    let climb = if held.sprint { 0.0 } else { wish.dot(facing) };
    let want = if held.crouch {
        -SLIDE_DOWN
    } else if climb > 0.2 {
        CLIMB_UP * climb.min(1.0)
    } else if climb < -0.2 {
        CLIMB_DOWN * climb.max(-1.0)
    } else {
        0.0
    };
    let accel = if held.crouch {
        CLIMB_ACCEL * 2.0
    } else {
        CLIMB_ACCEL
    };
    r.speed += (want - r.speed).clamp(-accel * dt, accel * dt);
    r.s -= r.speed * dt;
    r.climbed += r.speed * dt;
    r.secs += dt;
    if r.s > rope.length - 0.05 || (grounded && r.speed <= 0.0 && r.secs > 0.3) {
        // Off the bottom end (or down onto the ground).
        let_go(m, b, out, "rope_release");
        return moving;
    }
    if r.s <= rope.top_clear {
        r.s = rope.top_clear;
        r.speed = r.speed.min(0.0);
        // At the top, pushing on up: step off onto a ledge in reach.
        if climb > 0.3 || held.sprint && wish.length() > 0.3 {
            let toward = if wish.length() > 0.3 { wish } else { facing };
            if let Some(v) = crate::locomotion::step_off_rope(m, p, b, probe, toward, out) {
                m.rope = None;
                m.rope_last = r.rope;
                m.rope_cooldown = REGRAB_SECS;
                return v;
            }
        }
    }

    // A wall in the way stops the swing into it, and the rope runs through the hands where
    // the body really is.
    if let Some(n) = held.wall.filter(|n| n.dot(up).abs() < 0.7) {
        let into = r.swing.dot(n);
        if into < 0.0 {
            r.swing -= n * into;
        }
        let back = facing * ROPE_AHEAD;
        let hands = b.pos + (back - r.dir * (b.height * zipline::GRIP_ABOVE)).as_dvec3();
        r.dir = (hands - at).as_vec3().normalize_or(r.dir);
    }

    // The swing: a pendulum about the anchor, through the hands (Verlet on the sphere).
    let pump = if held.sprint {
        wish
    } else {
        right * wish.dot(right)
    };
    let mut swing = r.swing + (-up * b.g + flat(pump, up) * PUMP) * dt;
    swing *= 1.0 - (SWING_DAMPING * dt).min(1.0);
    let length = r.s.max(0.3);
    let hands = at + (r.dir * length).as_dvec3() + (swing * dt).as_dvec3();
    let mut dir = (hands - at).as_vec3().normalize_or(-up);
    // No wider than MAX_SWING from straight down.
    let angle = dir.dot(-up).clamp(-1.0, 1.0).acos();
    if angle > MAX_SWING {
        let side = flat(dir, up).normalize_or_zero();
        dir = -up * MAX_SWING.cos() + side * MAX_SWING.sin();
        let out_of = swing.dot(side).max(0.0);
        swing -= side * out_of;
    }
    swing -= dir * swing.dot(dir);
    r.dir = dir;
    r.swing = swing;

    let want = body_at(at, dir, r.s, facing, up, b.height);
    if r.secs > 0.3 && (b.pos - want).length() as f32 > TORN_OFF {
        let_go(m, b, out, "rope_release");
        return v;
    }
    m.rope = Some(r);
    m.set_mode(LocomotionMode::Climb);
    // Move with the hands, closing what gap is left (after the catch, or a bump) at up to
    // SNAP_SPEED.
    let moving = swing - dir * r.speed;
    let free = b.pos + (moving * dt).as_dvec3();
    let fix = (want - free).as_vec3().clamp_length_max(SNAP_SPEED * dt);
    moving + fix / dt
}

pub(crate) fn let_go(m: &mut Mover, b: &Body, out: &mut Outputs, trigger: &str) {
    m.rope_last = m.rope.take().and_then(|r| r.rope);
    m.rope_cooldown = REGRAB_SECS;
    m.set_mode(LocomotionMode::Air);
    out.trigger(b.entity, trigger);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_body_hangs_under_the_hands_behind_the_rope() {
        let at = DVec3::new(0.0, 10.0, 0.0);
        let c = body_at(at, Vec3::NEG_Y, 3.0, Vec3::X, Vec3::Y, 1.8);
        assert!((c.y - (10.0 - 3.0 - 1.8 * zipline::GRIP_ABOVE as f64)).abs() < 1e-4);
        assert!((c.x + ROPE_AHEAD as f64).abs() < 1e-4, "{c:?}");
    }
}
