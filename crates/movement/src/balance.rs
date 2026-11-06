//! Lines to walk with balance: beams, railings, tightropes, branches (Tomb Raider's rope walks,
//! Assassin's Creed's beams).
//!
//! - **Getting on**: a targeted leap, falling onto the line within [`CAPTURE`] of it, or
//!   walking onto it from its end.
//! - **Walking**: the stick along the line walks it; across does nothing. The speed is
//!   capped by how narrow it is ([`speed_cap`]): a beam is run, a railing walked briskly, a
//!   rope stepped carefully (sprint held a little faster).
//! - **Balance**: an inverted pendulum ([`LineWalk::lean`]) disturbed by speed, landing and
//!   how narrow the line is, kept up by an automatic controller (research §5: cosmetic by
//!   default): the arms go out and correct, the hips counter the lean.
//! - **The line moves**: a cable or a branch gives under the walker and bounces on landing
//!   ([`LineWalk::load`]), the cable sagging where the walker stands.
//! - **Exits**: jump leaps (to a target, or on along the line), crouch drops to hang from it
//!   (a bar: see [`crate::bar`]), walking off an end onto ground steps off.

use engine_core::math::{DVec3, Vec3};
use engine_core::prelude::*;
use engine_world::LocomotionMode;

use crate::impact::ImpactKind;
use crate::locomotion::{Body, Outputs, Probe};
use crate::mover::{Mover, flat};
use crate::profile::MovementProfile;
use crate::traverse::{Element, ElementKind, Input, follow, nearest_on_segment};

/// Falling onto a line within this of it (horizontally) lands on it, m.
pub const CAPTURE: f32 = 0.4;
/// How quickly the walking speed changes, m/s².
const ACCEL: f32 = 9.0;

/// Fastest along a line of this kind and width (sprint held: `sprint`), m/s.
pub fn speed_cap(kind: ElementKind, width: f32, sprint: bool) -> f32 {
    match kind {
        ElementKind::Tightrope => {
            if sprint {
                3.5
            } else {
                2.0
            }
        }
        ElementKind::Branch => 3.0,
        _ if width >= 0.25 => {
            if sprint {
                6.5
            } else {
                5.0
            }
        }
        _ => {
            if sprint {
                4.5
            } else {
                3.5
            }
        }
    }
}

/// How hard a line of this kind is to balance on, 0 (a wide beam) .. 1 (a rope).
pub fn narrowness(kind: ElementKind, width: f32) -> f32 {
    match kind {
        ElementKind::Tightrope => 1.0,
        ElementKind::Branch => 0.6,
        _ => (1.0 - (width - 0.05) / 0.3).clamp(0.0, 0.8),
    }
}

/// Walking a line.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LineWalk {
    pub element: Entity,
    /// The feet this far along from the line's `a` end, m.
    pub s: f32,
    /// Speed along it, m/s (+ toward `b`).
    pub speed: f32,
    /// The body's lean, rad (+ toward the right of `a`→`b`), and its rate.
    pub lean: f32,
    pub lean_v: f32,
    /// How far the line is pressed down under the feet beyond its rest sag, m, and how fast.
    pub load: f32,
    pub load_v: f32,
    pub secs: f32,
    /// Narrowness 0..1 (see [`narrowness`]).
    pub narrow: f32,
    /// The way the walker faces along the line (+1 toward `b`).
    pub heading: f32,
}

impl LineWalk {
    /// Where the feet are on `e`.
    pub fn feet(&self, e: &Element, up: Vec3) -> DVec3 {
        e.top_at(self.s, e.rest_sag(self.s) + self.load, up)
    }
}

/// Lands on the line `e`, `s` m along it.
pub(crate) fn land(
    m: &mut Mover,
    b: &Body,
    e: &Element,
    s: f32,
    v: Vec3,
    out: &mut Outputs,
) -> Vec3 {
    let up = b.up;
    let dir = e.dir(Vec3::X);
    let along = v.dot(dir);
    let narrow = narrowness(e.kind, e.width);
    let heading = if along.abs() > 0.5 {
        along.signum()
    } else if m.facing.dot(dir) >= 0.0 {
        1.0
    } else {
        -1.0
    };
    let down = (-v.dot(up)).max(0.0);
    // Landing tips the body a little (the side it came from) and presses the line down.
    let right = dir.cross(up);
    let lean_v = -v.dot(right) * 0.4 + if down > 2.0 { 0.15 * narrow } else { 0.0 };
    let gives = matches!(e.kind, ElementKind::Tightrope | ElementKind::Branch);
    m.leave_elements();
    m.line = Some(LineWalk {
        element: e.entity,
        s: s.clamp(0.0, e.length()),
        speed: along.clamp(-3.0, 3.0),
        lean: 0.0,
        lean_v,
        load: 0.0,
        load_v: if gives { down * 0.09 } else { 0.0 },
        secs: 0.0,
        narrow,
        heading,
    });
    m.air_secs = 0.0;
    m.fall_speed = 0.0;
    m.grounded = true;
    m.air_jumps_left = 0;
    m.set_mode(LocomotionMode::Walk);
    out.trigger(b.entity, "line_land");
    out.impact(
        b.entity,
        ImpactKind::Perch,
        e.at(s),
        up,
        down.max(v.length() * 0.3),
    );
    dir * along.clamp(-3.0, 3.0)
}

/// Falling onto a line under the feet, or walking onto one from its end, gets on it.
pub(crate) fn capture(
    m: &mut Mover,
    b: &Body,
    probe: &Probe,
    v: Vec3,
    wish: Vec3,
    grounded: bool,
    out: &mut Outputs,
) -> Option<Vec3> {
    let up = b.up;
    let feet = b.feet();
    if !grounded && v.dot(up) > 0.5 {
        return None;
    }
    for e in probe.elements_near(feet, 1.2) {
        if !e.kind.is_walkable_line() || Some(e.entity) == m.elem_last {
            continue;
        }
        let (on, t) = nearest_on_segment(e.a, e.b, feet);
        let len = e.length();
        let s = t * len;
        let top = e.top_at(s, e.rest_sag(s), up);
        let d = (feet - top).as_vec3();
        let above = d.dot(up);
        let side = flat(d, up).length();
        let _ = on;
        if grounded {
            // Walking onto it from an end: in line with it, stepping along it.
            let dir = e.dir(Vec3::X);
            let near_end = s < 0.6 || s > len - 0.6;
            let along = flat(wish, up).dot(dir).abs();
            if near_end && side < 0.25 && above.abs() < 0.35 && along > 0.7 {
                return Some(land(m, b, &e, s, flat(v, up), out));
            }
        } else if side < CAPTURE && (-0.25..0.5).contains(&above) {
            return Some(land(m, b, &e, s, v, out));
        }
    }
    None
}

/// One step on the line walked.
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
    let Some(mut w) = m.line else {
        return v;
    };
    let Some(e) = probe.element(w.element) else {
        crate::traverse::let_go(m, None, 0.0);
        return v;
    };
    w.secs += dt;
    let dir = e.dir(Vec3::X);
    let len = e.length();
    let moving = dir * w.speed;
    if input.knocked || w.lean.abs() > 0.6 {
        // Knocked or toppled: fall off (the ledge and bar catches may save it).
        crate::traverse::let_go(m, Some(w.element), 0.5);
        out.trigger(b.entity, "let_go");
        return moving + dir.cross(up) * w.lean.signum() * 1.5;
    }
    if input.jump_pressed {
        // Without a target: on along the line (or the way the stick says).
        let ahead = if input.wish.length() > 0.3 {
            input.wish.normalize()
        } else {
            dir * w.heading
        };
        let fallback =
            ahead * w.speed.abs().max(p.run_speed * 0.7) + up * p.jump_speed(p.jump_height, b.g);
        return crate::target::jump_off(
            m,
            p,
            b,
            probe,
            input,
            Some(w.element),
            moving,
            fallback,
            out,
        );
    }
    if input.crouch_pressed && w.secs > 0.2 {
        // Drop to hang from it.
        out.trigger(b.entity, "drop_hang");
        let s = w.s;
        return crate::bar::catch(m, b, &e, s, moving, out);
    }
    // Walk along it: only the stick's part along the line counts.
    let push = flat(input.wish, up).dot(dir);
    let cap = speed_cap(e.kind, e.width, input.sprint);
    let want = if push.abs() > 0.2 {
        cap * push.clamp(-1.0, 1.0)
    } else {
        0.0
    };
    w.speed += (want - w.speed).clamp(-ACCEL * dt, ACCEL * dt);
    if w.speed.abs() > 0.3 {
        w.heading = w.speed.signum();
    }
    w.s += w.speed * dt;
    if w.s < 0.0 || w.s > len {
        // Off an end: onto ground there, else stop at the end.
        let end = if w.s < 0.0 { e.a } else { e.b };
        let beyond = end + (dir * w.speed.signum() * 0.4).as_dvec3();
        let ground = probe.ray(beyond + (up * 0.5).as_dvec3(), -up, 1.0);
        if ground.is_some_and(|h| h.normal.dot(up) > 0.7) {
            crate::traverse::let_go(m, Some(w.element), 0.6);
            m.set_mode(LocomotionMode::Walk);
            return moving;
        }
        w.s = w.s.clamp(0.0, len);
        w.speed = 0.0;
    }
    // Balance: an inverted pendulum pushed about by speed and the line, held up by an
    // automatic controller.
    let t = w.secs;
    let noise = ((t * 2.3).sin() * 0.6 + (t * 3.7 + 1.3).sin() * 0.4) * (0.4 + w.speed.abs() * 0.5);
    let acc = 6.0 * w.lean + noise * (0.5 + 2.5 * w.narrow) - 28.0 * w.lean - 7.0 * w.lean_v;
    w.lean_v += acc * dt;
    w.lean += w.lean_v * dt;
    // A cable or a branch gives under the weight (more mid-span, at a branch's tip) and
    // bounces.
    let gives = matches!(e.kind, ElementKind::Tightrope | ElementKind::Branch);
    if gives {
        let u = (w.s / len.max(0.1)).clamp(0.0, 1.0);
        let rest = match e.kind {
            ElementKind::Branch => 0.2 * u * u,
            _ => 0.22 * 4.0 * u * (1.0 - u),
        };
        let step = (w.secs * 6.0).sin() * 0.01 * w.speed.abs();
        let a = -80.0 * (w.load - rest - step) - 2.0 * 0.22 * 9.0 * w.load_v;
        w.load_v += a * dt;
        w.load += w.load_v * dt;
    }
    m.line = Some(w);
    m.trav_face = Some(dir * w.heading);
    m.grounded = true;
    m.air_secs = 0.0;
    m.set_mode(if w.speed.abs() > 3.6 {
        LocomotionMode::Run
    } else if w.speed.abs() > 0.2 {
        LocomotionMode::Walk
    } else {
        LocomotionMode::Idle
    });
    if (w.secs * w.speed.abs() / 0.55).fract() < (w.speed.abs() * dt / 0.55) {
        out.trigger(b.entity, "line_step");
    }
    let want = w.feet(&e, up) + (up * b.height * 0.5).as_dvec3();
    let moving = dir * w.speed + up * -w.load_v;
    follow(b.pos, want, moving, 6.0, dt)
}
