//! Bars: horizontal poles to hang and swing from (scaffold pipes, sign brackets, low branches)
//! and washing lines to go hand over hand along. Also the hang under a beam, a railing or a
//! tightrope after dropping off it.
//!
//! - **Catching**: a targeted leap, or the hands passing within [`GRAB_REACH`] of the bar in
//!   the air. The speed across the bar carries on as the swing.
//! - **Swinging**: a pendulum about the bar ([`RADIUS`] to the center of mass). The stick
//!   along the swing pumps it near the bottom ([`PUMP`]). Sideways the hands shimmy along
//!   ([`SHIMMY`]) when hanging still.
//! - **Jump** lets go with the swing's speed and a boost along it ([`RELEASE_BOOST`]); let
//!   go just past the bottom on the way up and it throws you farthest (a "perfect" with a kick
//!   of the view). A target in the cone bends the release onto it. Under a line to walk, jump
//!   without the stick climbs back up onto it.
//! - **Crouch** drops.

use engine_core::math::{DVec3, Vec3};
use engine_core::prelude::*;
use engine_world::LocomotionMode;

use crate::impact::ImpactKind;
use crate::locomotion::{Body, Outputs, Probe};
use crate::mover::{Mover, flat};
use crate::profile::MovementProfile;
use crate::traverse::{Element, ElementKind, Input, follow, nearest_on_segment};

/// Hands this close to the bar catch it, m.
pub const GRAB_REACH: f32 = 0.4;
/// From the bar to the body's center of mass, m (the pendulum).
pub const RADIUS: f32 = 1.1;
/// Pumping near the bottom, m/s² along the swing.
pub const PUMP: f32 = 3.0;
/// Shimmy speed along the bar, m/s.
pub const SHIMMY: f32 = 1.3;
/// Speed added along the swing on letting go, m/s.
pub const RELEASE_BOOST: f32 = 1.0;
/// The swing's damping, 1/s.
const DAMPING: f32 = 0.18;
/// Widest swing, rad.
const MAX_SWING: f32 = 1.6;

/// Hanging from a bar or a line.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BarHold {
    pub element: Entity,
    /// Hands this far along the bar from its `a` end, m.
    pub s: f32,
    /// Facing across the bar (horizontal unit): a swing forward goes this way.
    pub forward: Vec3,
    /// Swing angle from hanging straight down, rad (+ toward `forward`), and its rate.
    pub angle: f32,
    pub rate: f32,
    /// Shimmying along, m/s (+ toward `b`).
    pub along: f32,
    /// How far a cable is pressed down under the hands, m.
    pub load: f32,
    pub load_v: f32,
    pub secs: f32,
    /// The grip this step (for the look).
    pub at: DVec3,
}

impl BarHold {
    /// The grip (between the hands) on `e`, pressed down by its sag.
    pub fn grip(&self, e: &Element, up: Vec3) -> DVec3 {
        let (p, _) = (e.at(self.s), ());
        p - (up * (e.rest_sag(self.s) + self.load)).as_dvec3()
    }
}

/// The body's center hanging at `angle` from `grip`.
pub fn body_at(grip: DVec3, forward: Vec3, angle: f32, up: Vec3, height: f32) -> DVec3 {
    let r = height * crate::zipline::GRIP_ABOVE;
    let down = -up * angle.cos() + forward * angle.sin();
    grip + (down * r).as_dvec3()
}

/// Catches the bar (or line) `e` with the hands `s` m along it.
pub(crate) fn catch(
    m: &mut Mover,
    b: &Body,
    e: &Element,
    s: f32,
    v: Vec3,
    out: &mut Outputs,
) -> Vec3 {
    let up = b.up;
    let along = e.dir(Vec3::X);
    let across = flat(up.cross(along), up).normalize_or(m.facing);
    let vh = flat(v, up);
    // Face the way the body crosses the bar (else the way it faces).
    let forward = if vh.dot(across).abs() > 0.5 {
        across * vh.dot(across).signum()
    } else if m.facing.dot(across) >= 0.0 {
        across
    } else {
        -across
    };
    let speed_across = vh.dot(forward);
    let rate = (speed_across / RADIUS).clamp(-4.0, 4.0) * 0.9;
    m.leave_elements();
    m.bar = Some(BarHold {
        element: e.entity,
        s: s.clamp(0.45, (e.length() - 0.45).max(0.45)),
        forward,
        angle: 0.0,
        rate,
        along: 0.0,
        load: 0.0,
        load_v: if e.sag > 0.0 {
            -v.dot(up).min(0.0) * 0.12
        } else {
            0.0
        },
        secs: 0.0,
        at: e.at(s),
    });
    m.air_jumps_left = 0;
    m.set_mode(LocomotionMode::Climb);
    out.trigger(b.entity, "bar_grab");
    out.impact(b.entity, ImpactKind::Catch, e.at(s), -up, v.length());
    vh * 0.3
}

/// Hands passing near a bar in the air catch it.
pub(crate) fn capture(
    m: &mut Mover,
    b: &Body,
    probe: &Probe,
    v: Vec3,
    out: &mut Outputs,
) -> Option<Vec3> {
    let up = b.up;
    let hand = crate::zipline::hand(b.pos, up, b.height);
    for e in probe.elements_near(hand, 1.0) {
        if !matches!(e.kind, ElementKind::PoleH | ElementKind::Clothesline)
            || Some(e.entity) == m.elem_last
        {
            continue;
        }
        let (on, t) = nearest_on_segment(e.a, e.b, hand);
        let s = t * e.length();
        let on = on - (up * e.rest_sag(s)).as_dvec3();
        if (on - hand).length() < GRAB_REACH as f64 {
            return Some(catch(m, b, &e, s, v, out));
        }
    }
    None
}

/// One step hanging from the bar held.
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
    let Some(mut h) = m.bar else {
        return v;
    };
    let Some(e) = probe.element(h.element) else {
        crate::traverse::let_go(m, None, 0.0);
        return v;
    };
    h.secs += dt;
    let g = b.g;
    // The swing's tangent at the body (the way it moves for a growing angle), and its speed.
    let tangent = up * h.angle.sin() + h.forward * h.angle.cos();
    let r = b.height * crate::zipline::GRIP_ABOVE;
    let swing_v = tangent * h.rate * r;
    if input.knocked || (input.crouch_pressed && h.secs > 0.15) {
        crate::traverse::let_go(m, Some(h.element), 0.5);
        out.trigger(b.entity, "bar_release");
        return swing_v;
    }
    if input.jump_pressed {
        // Under a line to walk with the stick idle: climb back up onto it.
        if e.kind.is_walkable_line() && input.wish.length() < 0.3 && h.angle.abs() < 0.3 {
            out.trigger(b.entity, "mantle");
            let s = h.s;
            return crate::balance::land(m, b, &e, s, up * 2.0, out);
        }
        // Just past the bottom on the way up throws farthest.
        let perfect = h.rate > 1.0 && (0.2..0.8).contains(&h.angle);
        let release = swing_v + tangent * h.rate.signum() * RELEASE_BOOST + up * 1.2;
        let mut v = crate::target::jump_off(
            m,
            p,
            b,
            probe,
            input,
            Some(h.element),
            release,
            release,
            out,
        );
        if perfect {
            v += flat(v, up).normalize_or_zero() * 0.8;
            out.trigger(b.entity, "perfect");
        }
        return v;
    }
    // The pendulum: gravity, pumping near the bottom, damping.
    let pump = if h.angle.abs() < 0.45 {
        input.wish.dot(h.forward) * PUMP / RADIUS
    } else {
        0.0
    };
    let acc = -(g / RADIUS) * h.angle.sin() + pump - DAMPING * h.rate;
    h.rate += acc * dt;
    h.angle += h.rate * dt;
    if h.angle.abs() > MAX_SWING {
        h.angle = h.angle.clamp(-MAX_SWING, MAX_SWING);
        h.rate = 0.0;
    }
    // Shimmy along the bar while hanging fairly still.
    let along_dir = e.dir(Vec3::X);
    let side = if h.angle.abs() < 0.35 {
        input.wish.dot(along_dir)
    } else {
        0.0
    };
    let want = if side.abs() > 0.3 {
        SHIMMY * side.clamp(-1.0, 1.0)
    } else {
        0.0
    };
    h.along += (want - h.along).clamp(-8.0 * dt, 8.0 * dt);
    let len = e.length();
    h.s = (h.s + h.along * dt).clamp(0.45, (len - 0.45).max(0.45));
    // A cable gives under the weight and bounces.
    if e.sag > 0.0 {
        let u = (h.s / len.max(0.1)).clamp(0.0, 1.0);
        let rest = 0.25 * 4.0 * u * (1.0 - u);
        let a = -60.0 * (h.load - rest) - 2.0 * 0.25 * 7.7 * h.load_v;
        h.load_v += a * dt;
        h.load += h.load_v * dt;
    }
    let grip = h.grip(&e, up);
    h.at = grip;
    m.bar = Some(h);
    m.trav_face = Some(h.forward);
    m.set_mode(LocomotionMode::Climb);
    let want = body_at(grip, h.forward, h.angle, up, b.height);
    let tangent = up * h.angle.sin() + h.forward * h.angle.cos();
    let moving = tangent * h.rate * r + along_dir * h.along;
    follow(b.pos, want, moving, 8.0, dt)
}
