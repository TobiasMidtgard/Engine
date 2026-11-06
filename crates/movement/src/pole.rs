//! Upright poles: lamp-post shafts, drainpipes, flagpoles, thin trunks.
//!
//! - **Catching** one: a targeted leap to it, or a hand coming within [`GRAB_REACH`] of its
//!   shaft in the air. Caught at speed, the body **swings round** it (Assassin's Creed's pole
//!   swing): the run carries on as a turn about the pole ([`SPIN_RADIUS`]), and jump lets go
//!   along the swing with all its speed.
//! - **Climbing**: the stick toward the pole climbs ([`CLIMB_UP`]), away climbs down, crouch
//!   slides ([`SLIDE`]); sideways turns round it. At the top, climbing on steps up onto a
//!   perch there (a lamp-post head).
//! - **Jump** leaps to what the stick or camera points at, else kicks off away from it.

use engine_core::math::{DVec3, Quat, Vec3};
use engine_core::prelude::*;
use engine_world::LocomotionMode;

use crate::impact::ImpactKind;
use crate::locomotion::{Body, Outputs, Probe};
use crate::mover::{Mover, flat};
use crate::profile::MovementProfile;
use crate::traverse::{Element, ElementKind, Input, follow, nearest_on_segment};

/// A hand this close to the shaft (beyond its radius) catches it, m.
pub const GRAB_REACH: f32 = 0.45;
/// Climbing speeds, m/s.
pub const CLIMB_UP: f32 = 1.5;
pub const CLIMB_DOWN: f32 = 2.0;
pub const SLIDE: f32 = 4.0;
/// Turning round the pole with the stick sideways, rad/s.
const TURN: f32 = 2.4;
/// The body's center swings round the pole this far from its axis, m.
pub const SPIN_RADIUS: f32 = 0.55;
/// Caught faster than this (across the pole), the body swings round it, m/s.
const SPIN_MIN: f32 = 3.5;
/// A swing round the pole slows by this, rad/s².
const SPIN_DRAG: f32 = 2.0;

/// Holding an upright pole.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PoleHold {
    pub element: Entity,
    /// The hands' height above the pole's foot, m.
    pub s: f32,
    /// From the pole's axis out to the body, horizontal unit.
    pub out: Vec3,
    /// Climbing speed, m/s (up positive).
    pub speed: f32,
    /// Swinging round the pole, rad/s (+ counterclockwise seen from above).
    pub spin: f32,
    pub secs: f32,
    /// The pole's axis at the hands, and its radius (for the look), m.
    pub axis: DVec3,
    pub radius: f32,
}

/// Where the body's center is holding pole `e` at hands height `s`, out along `out`.
pub fn body_at(e: &Element, s: f32, out: Vec3, up: Vec3, height: f32, spin: bool) -> DVec3 {
    let axis = e.a + (e.dir(up) * s).as_dvec3();
    // Clear of the pole's collider (the capsule's radius and a little).
    let hold = e.width * 0.5 + 0.37;
    let r = if spin { SPIN_RADIUS.max(hold) } else { hold };
    axis - (up * height * crate::zipline::GRIP_ABOVE).as_dvec3() + (out * r).as_dvec3()
}

/// Catches the pole `e` with the hands `s` m up it.
pub(crate) fn catch(
    m: &mut Mover,
    b: &Body,
    e: &Element,
    s: f32,
    v: Vec3,
    out: &mut Outputs,
) -> Vec3 {
    let up = b.up;
    let axis = e.a + (e.dir(up) * s).as_dvec3();
    let away = flat((b.pos - axis).as_vec3(), up)
        .try_normalize()
        .unwrap_or(-m.facing);
    let vh = flat(v, up);
    // Across the pole, the run turns into a swing round it.
    let tangent = up.cross(away);
    let across = vh.dot(tangent);
    let spin = if across.abs() > SPIN_MIN {
        across / SPIN_RADIUS
    } else {
        0.0
    };
    m.leave_elements();
    m.pole = Some(PoleHold {
        element: e.entity,
        s: s.clamp(0.0, e.length()),
        out: away,
        speed: 0.0,
        spin,
        secs: 0.0,
        axis,
        radius: e.width * 0.5,
    });
    m.air_jumps_left = 0;
    m.set_mode(LocomotionMode::Climb);
    out.trigger(
        b.entity,
        if spin != 0.0 {
            "pole_swing"
        } else {
            "pole_grab"
        },
    );
    out.impact(b.entity, ImpactKind::Catch, axis, away, vh.length());
    vh * 0.2
}

/// A hand in the air near a pole's shaft catches it.
pub(crate) fn capture(
    m: &mut Mover,
    b: &Body,
    probe: &Probe,
    v: Vec3,
    out: &mut Outputs,
) -> Option<Vec3> {
    let up = b.up;
    let hand = crate::zipline::hand(b.pos, up, b.height);
    let chest = b.pos + (up * 0.4).as_dvec3();
    for e in probe.elements_near(chest, 1.5) {
        if e.kind != ElementKind::PoleV || Some(e.entity) == m.elem_last {
            continue;
        }
        let (on, t) = nearest_on_segment(e.a, e.b, chest);
        let gap = flat((chest - on).as_vec3(), up).length() - e.width * 0.5;
        let s = (t * e.length()).max((hand - e.a).dot(up.as_dvec3()) as f32 - 0.2);
        if gap < GRAB_REACH && s > 1.0 && s < e.length() - 0.1 {
            let s = s.min(e.length() - 0.2);
            return Some(catch(m, b, &e, s, v, out));
        }
    }
    None
}

/// One step on the pole held.
#[allow(clippy::too_many_arguments)]
pub(crate) fn step(
    m: &mut Mover,
    p: &MovementProfile,
    b: &Body,
    probe: &Probe,
    input: &Input,
    v: Vec3,
    grounded: bool,
    dt: f32,
    out: &mut Outputs,
) -> Vec3 {
    let up = b.up;
    let Some(mut h) = m.pole else {
        return v;
    };
    let Some(e) = probe.element(h.element) else {
        crate::traverse::let_go(m, None, 0.0);
        return v;
    };
    h.secs += dt;
    if input.knocked {
        crate::traverse::let_go(m, Some(h.element), 0.6);
        out.trigger(b.entity, "let_go");
        return v;
    }
    let spinning = h.spin.abs() > 0.5;
    let tangent = up.cross(h.out);
    if input.jump_pressed {
        // Off along the swing with its speed, or away from the pole.
        let fallback = if spinning {
            tangent * (h.spin * SPIN_RADIUS) * 1.1 + h.out * 2.0 + up * p.jump_speed(0.9, b.g)
        } else {
            h.out * 4.0 + up * p.jump_speed(0.8, b.g)
        };
        let base = if spinning {
            tangent * h.spin * SPIN_RADIUS
        } else {
            Vec3::ZERO
        };
        return crate::target::jump_off(
            m,
            p,
            b,
            probe,
            input,
            Some(h.element),
            base,
            fallback,
            out,
        );
    }
    if spinning {
        // Swinging round: the stick doesn't climb; the swing slows.
        let turn = h.spin * dt;
        h.out = (Quat::from_axis_angle(up, turn) * h.out).normalize();
        let slow = SPIN_DRAG * dt;
        h.spin = if h.spin > 0.0 {
            (h.spin - slow).max(0.0)
        } else {
            (h.spin + slow).min(0.0)
        };
        if h.spin.abs() <= 0.5 {
            h.spin = 0.0;
        }
    } else {
        let toward = input.wish.dot(-h.out);
        let side = input.wish.dot(tangent);
        let want = if input.crouch {
            -SLIDE
        } else if toward > 0.3 {
            CLIMB_UP * toward.min(1.0)
        } else if toward < -0.3 {
            CLIMB_DOWN * toward.max(-1.0)
        } else {
            0.0
        };
        h.speed += (want - h.speed).clamp(-10.0 * dt, 10.0 * dt);
        h.s += h.speed * dt;
        if side.abs() > 0.3 {
            h.out = (Quat::from_axis_angle(up, side * TURN * dt) * h.out).normalize();
        }
    }
    let len = e.length();
    let top = len - 0.15;
    if h.s >= top {
        h.s = top;
        h.speed = h.speed.min(0.0);
        // Climbing on at the top: up onto a perch there.
        if input.wish.dot(-h.out) > 0.3 || input.wish.length() < 0.1 && h.speed >= 0.0 {
            let head = e.b;
            if input.wish.dot(-h.out) > 0.3
                && let Some(perch) = probe.elements_near(head, 1.2).find(|x| {
                    x.kind == ElementKind::Perch && (x.a - head).dot(up.as_dvec3()) > -0.2
                })
            {
                out.trigger(b.entity, "pole_top");
                return crate::perch::land(m, b, &perch, up * 2.0, out);
            }
        }
    }
    // Down at the foot: let go onto the ground.
    let feet_on_ground = grounded && h.speed < 0.0;
    if h.s < 1.2 || feet_on_ground {
        crate::traverse::let_go(m, Some(h.element), 0.5);
        m.set_mode(LocomotionMode::Idle);
        out.trigger(b.entity, "pole_release");
        return h.out * 1.0;
    }
    h.axis = e.a + (e.dir(up) * h.s).as_dvec3();
    h.radius = e.width * 0.5;
    m.pole = Some(h);
    m.trav_face = Some(-h.out);
    m.set_mode(LocomotionMode::Climb);
    let want = body_at(&e, h.s, h.out, up, b.height, h.spin != 0.0);
    let moving = up * h.speed + up.cross(h.out) * h.spin * SPIN_RADIUS;
    follow(b.pos, want, moving, 7.0, dt)
}
