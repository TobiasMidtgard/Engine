//! Perches: small tops to land and crouch on (lamp-post heads, bollards, chimney caps, branch
//! forks), Assassin's Creed's lamp post to lamp post and Spider-Man's perch launches.
//!
//! - **Landing** on one (a targeted leap, or falling onto it within [`CAPTURE`] of its center)
//!   crouches on it, absorbing the landing in a squat ([`SQUAT_SECS`]).
//! - **Jump** leaps to the next element the stick or the camera points at
//!   ([`crate::target`]); pressed within [`PERFECT_SECS`] of touching down it launches
//!   [`PERFECT_BONUS`] faster with a kick of the view (a "perfect").
//! - **Holding the stick** a moment walks off the edge; **crouch** drops straight down.
//! - Crouched on a perch with something to hunt below, attack drops onto it (see
//!   [`crate::takedown`]).

use engine_core::math::{DVec3, Vec3};
use engine_core::prelude::*;
use engine_world::LocomotionMode;

use crate::impact::ImpactKind;
use crate::locomotion::{Body, Outputs, Probe};
use crate::mover::{Mover, flat};
use crate::profile::MovementProfile;
use crate::traverse::{Element, ElementKind, Input, follow};

/// Falling onto a perch within this of its top's center (horizontally) lands on it, m.
pub const CAPTURE: f32 = 0.45;
/// The landing squat lasts this long, s.
pub const SQUAT_SECS: f32 = 0.3;
/// A jump this soon after touching down is a perfect launch, s, and how much faster it goes.
pub const PERFECT_SECS: f32 = 0.12;
pub const PERFECT_BONUS: f32 = 0.15;
/// Holding the stick this long steps off, s.
const STEP_OFF_SECS: f32 = 0.3;

/// Crouched on a perch.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Perch {
    pub element: Entity,
    /// The top's center.
    pub top: DVec3,
    /// Own seconds on it.
    pub secs: f32,
    /// How hard it was landed on, m/s (the squat's depth).
    pub landed: f32,
    /// How long the stick has been held, s.
    held: f32,
}

impl Perch {
    /// 0..1: how much of the landing is still being absorbed.
    pub fn squat(&self) -> f32 {
        let t = self.secs / SQUAT_SECS;
        if t >= 1.0 {
            return 0.0;
        }
        (1.0 - t) * (0.4 + self.landed / 12.0).min(1.0)
    }
}

/// Lands on the perch `e`.
pub(crate) fn land(m: &mut Mover, b: &Body, e: &Element, v: Vec3, out: &mut Outputs) -> Vec3 {
    let speed = v.length();
    m.leave_elements();
    m.perch = Some(Perch {
        element: e.entity,
        top: e.a,
        secs: 0.0,
        landed: speed,
        held: 0.0,
    });
    m.air_secs = 0.0;
    m.fall_speed = 0.0;
    m.grounded = true;
    m.air_jumps_left = 0;
    m.set_mode(LocomotionMode::Crouch);
    out.trigger(b.entity, "perch");
    out.impact(b.entity, ImpactKind::Perch, e.a, b.up, speed);
    out.noise(b.entity, e.a, 3.0);
    let want = e.a + (b.up * b.height * 0.5).as_dvec3();
    follow(b.pos, want, Vec3::ZERO, 12.0, 1.0 / 60.0)
}

/// Falling onto a perch under the feet catches it.
pub(crate) fn capture(
    m: &mut Mover,
    b: &Body,
    probe: &Probe,
    v: Vec3,
    out: &mut Outputs,
) -> Option<Vec3> {
    if v.dot(b.up) > 0.5 {
        return None;
    }
    let feet = b.feet();
    for e in probe.elements_near(feet, 1.0) {
        if e.kind != ElementKind::Perch || Some(e.entity) == m.elem_last {
            continue;
        }
        let d = (feet - e.a).as_vec3();
        let above = d.dot(b.up);
        if flat(d, b.up).length() < CAPTURE && (-0.2..0.45).contains(&above) {
            return Some(land(m, b, &e, v, out));
        }
    }
    None
}

/// One step on the perch held.
#[allow(clippy::too_many_arguments)]
pub(crate) fn step(
    m: &mut Mover,
    p: &MovementProfile,
    b: &Body,
    probe: &Probe,
    input: &Input,
    v: Vec3,
    dt: f32,
    out: &mut Outputs,
) -> Vec3 {
    let up = b.up;
    let Some(mut h) = m.perch else {
        return v;
    };
    let Some(e) = probe.element(h.element) else {
        crate::traverse::let_go(m, None, 0.0);
        return v;
    };
    h.top = e.a;
    h.secs += dt;
    if input.knocked {
        crate::traverse::let_go(m, Some(h.element), 0.6);
        out.trigger(b.entity, "let_go");
        return v;
    }
    if input.jump_pressed {
        let perfect = h.secs < PERFECT_SECS;
        let dir = if input.wish.length() > 0.3 {
            input.wish.normalize()
        } else {
            input
                .look
                .map(|l| flat(l, up))
                .and_then(|l| l.try_normalize())
                .unwrap_or(m.facing)
        };
        // Without a target: a standing jump off it toward the stick.
        let hop = dir * p.run_speed * 0.8 + up * p.jump_speed(p.jump_height, b.g);
        let mut launched =
            crate::target::jump_off(m, p, b, probe, input, Some(h.element), Vec3::ZERO, hop, out);
        if perfect {
            launched += flat(launched, up) * PERFECT_BONUS;
            if let Some(l) = &mut m.leap {
                // The arc lands sooner (faster along it): shorten the flight to match.
                l.total /= 1.0 + PERFECT_BONUS;
            }
            out.trigger(b.entity, "perfect");
        }
        return launched;
    }
    if input.crouch_pressed && h.secs > 0.15 {
        // Drop straight down off it.
        crate::traverse::let_go(m, Some(h.element), 0.6);
        out.trigger(b.entity, "drop");
        return m.facing * 0.8 + up * 0.5;
    }
    // The stick along a branch or a beam starting by the perch: out onto it.
    if input.wish.length() > 0.6 {
        let wish = flat(input.wish, up).normalize_or_zero();
        let onto = probe.elements_near(h.top, 1.0).find_map(|l| {
            if !l.kind.is_walkable_line() {
                return None;
            }
            let dir = l.dir(Vec3::X);
            let len = l.length();
            [(l.a, 0.0, dir), (l.b, len, -dir)]
                .into_iter()
                .find(|(end, _, way)| {
                    (*end - h.top).length() < 0.8
                        && flat(*way, up).normalize_or_zero().dot(wish) > 0.6
                })
                .map(|(_, s, way)| (l, s, way))
        });
        if let Some((l, s, way)) = onto {
            out.trigger(b.entity, "step_onto");
            return crate::balance::land(m, b, &l, s, way * 1.5, out);
        }
    }
    // The stick held a moment walks off the edge.
    if input.wish.length() > 0.6 {
        h.held += dt;
        if h.held > STEP_OFF_SECS {
            crate::traverse::let_go(m, Some(h.element), 0.5);
            return input.wish.normalize() * 2.5 + up * 1.8;
        }
    } else {
        h.held = 0.0;
    }
    m.perch = Some(h);
    m.grounded = true;
    m.air_secs = 0.0;
    m.set_mode(LocomotionMode::Crouch);
    let want = h.top + (up * b.height * 0.5).as_dvec3();
    follow(b.pos, want, Vec3::ZERO, 8.0, dt)
}
