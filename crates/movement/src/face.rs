//! Rock and ice faces: climbing hold by hold "like real life" (Uncharted 4's reach ring, Jusant's
//! one limb at a time) and with ice picks (Tomb Raider's axes).
//!
//! - **Rock** ([`ElementKind::Rock`]): the face has generated holds ([`crate::traverse::rock_holds`]:
//!   jugs, crimps, cracks, foot holds). The stick picks a direction on the face; the hand that
//!   leads that way reaches for the best hold in an annulus around it ([`REACH`]) and the body
//!   shifts toward the other supports while it moves; the feet follow onto holds about a body
//!   length below the hands. One limb moves at a time ([`HAND_SECS`], [`FOOT_SECS`]): shift,
//!   reach, plant (a small dip and a puff of chalk), settle.
//! - **Dyno**: jump with the stick up throws both hands to a hold up to [`DYNO_REACH`] higher.
//! - **Ice** ([`ElementKind::Ice`]): every point takes a pick. Pick, foot, foot (the "T"
//!   technique): each pick is cocked, swung and bites with a short body-only hitstop, ice chips
//!   and a bump of the view ([`PICK_SECS`]).
//! - **Exits**: at the top, climbing on mantles over the edge; jump without the stick up kicks
//!   off the face (or leaps to a target); crouch lets go; at the bottom the feet step down.

use engine_core::math::{DVec3, Vec3};
use engine_core::prelude::*;
use engine_world::LocomotionMode;

use crate::impact::ImpactKind;
use crate::locomotion::{Body, Outputs, Probe};
use crate::mover::{Mover, flat};
use crate::profile::MovementProfile;
use crate::traverse::{Element, ElementKind, HoldKind, Input, follow, rock_holds};

/// A hand reaches holds this far from where it is, m (min, max).
pub const REACH: (f32, f32) = (0.22, 0.85);
/// A dyno reaches this much higher, m.
pub const DYNO_REACH: f32 = 1.5;
/// How long a hand move and a foot move take, s.
pub const HAND_SECS: f32 = 0.22;
pub const FOOT_SECS: f32 = 0.16;
/// How long an ice pick takes (cock, swing, bite), s, and how far each pick reaches, m.
pub const PICK_SECS: f32 = 0.28;
pub const PICK_REACH: f32 = 0.6;
/// The body's center stands this far off the face, m.
pub const OFF_FACE: f32 = 0.42;
/// The feet hold about this far below the hands, m.
const FEET_BELOW: f32 = 1.05;

/// A limb on the move.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LimbMove {
    /// 0 left hand, 1 right hand, 2 left foot, 3 right foot, 4 both hands (a dyno).
    pub limb: u8,
    pub from: [DVec3; 2],
    pub to: [DVec3; 2],
    pub t: f32,
    pub secs: f32,
}

/// Climbing a rock or ice face.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FaceClimb {
    pub element: Entity,
    pub ice: bool,
    /// Where the hands and feet hold (left, right).
    pub hands: [DVec3; 2],
    pub feet: [DVec3; 2],
    pub moving: Option<LimbMove>,
    /// The hand that moved last (the other leads next), and the moves since the feet did.
    pub last_hand: u8,
    pub hand_moves: u8,
    pub secs: f32,
    /// The body's center, eased toward where the holds put it.
    pub center: DVec3,
}

impl FaceClimb {
    /// Where the body's center belongs for the holds (and any limb on the move), off a face
    /// with outward normal `n`.
    pub fn rest_center(&self, n: Vec3, up: Vec3) -> DVec3 {
        let (hands, feet) = self.contacts();
        let hm = (hands[0] + hands[1]) * 0.5;
        let fm = (feet[0] + feet[1]) * 0.5;
        // Between the feet and the hands, nearer the hands (arms long, weight on the feet).
        let mut c = fm + (hm - fm) * 0.5;
        // Hands high above the feet: the body hangs from them.
        let span = (hm - fm).dot(up.as_dvec3()) as f32;
        c += (up * (span - 1.7).clamp(-0.5, 0.3) * -0.2).as_dvec3();
        // Shift toward the supports while a limb moves.
        if let Some(mv) = self.moving {
            let shift = (mv.t * std::f32::consts::PI).sin() * 0.06;
            let side = match mv.limb {
                0 => (hands[1] - hands[0]).normalize_or_zero(),
                1 => (hands[0] - hands[1]).normalize_or_zero(),
                _ => DVec3::ZERO,
            };
            c += side * shift as f64;
        }
        let on = c - n.as_dvec3() * (c - hm).dot(n.as_dvec3());
        on + (n * OFF_FACE).as_dvec3()
    }

    /// The contacts as they are now (a limb on the move partway along its arc).
    pub fn contacts(&self) -> ([DVec3; 2], [DVec3; 2]) {
        let (mut hands, mut feet) = (self.hands, self.feet);
        if let Some(mv) = self.moving {
            let k = smooth(mv.t) as f64;
            match mv.limb {
                0 | 1 => hands[mv.limb as usize] = mv.from[0].lerp(mv.to[0], k),
                2 | 3 => feet[mv.limb as usize - 2] = mv.from[0].lerp(mv.to[0], k),
                _ => {
                    hands[0] = mv.from[0].lerp(mv.to[0], k);
                    hands[1] = mv.from[1].lerp(mv.to[1], k);
                }
            }
        }
        (hands, feet)
    }
}

fn smooth(t: f32) -> f32 {
    let t = t.clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// A point on the face `e` near `p`, kept inside the patch (`margin` m in from its edges).
fn on_face(e: &Element, up: Vec3, p: DVec3, margin: f32) -> DVec3 {
    let (origin, across, fu, w, h) = e.face_frame(up);
    let rel = (p - origin).as_vec3();
    let x = rel
        .dot(across)
        .clamp(margin.min(w * 0.5), (w - margin).max(w * 0.5));
    let y = rel.dot(fu).clamp(0.0, h);
    origin + (across * x + fu * y).as_dvec3()
}

/// The hold of kind not `Foot` (hands) or any (feet) on rock face `e` best for a limb at
/// `from` going `dir` (on the face), within `reach`, kept within `span` of `other`.
#[allow(clippy::too_many_arguments)]
fn best_hold(
    holds: &[crate::traverse::Hold],
    from: DVec3,
    dir: Vec3,
    reach: (f32, f32),
    other: DVec3,
    span: f32,
    feet: bool,
) -> Option<DVec3> {
    let mut best: Option<(f32, DVec3)> = None;
    for h in holds {
        if !feet && h.kind == HoldKind::Foot {
            continue;
        }
        let d = (h.at - from).as_vec3();
        let dist = d.length();
        if dist < reach.0 || dist > reach.1 || (h.at - other).length() as f32 > span {
            continue;
        }
        let align = d.normalize_or_zero().dot(dir);
        if align < 0.35 {
            continue;
        }
        let kind = match h.kind {
            HoldKind::Jug => 0.25,
            HoldKind::Crack => 0.15,
            _ => 0.0,
        };
        let score = align * 1.2 + dist / reach.1 * 0.6 + kind;
        if best.is_none_or(|(s, _)| score > s) {
            best = Some((score, h.at));
        }
    }
    best.map(|(_, p)| p)
}

/// The nearest hold to `p` (for the catch).
fn nearest_hold(holds: &[crate::traverse::Hold], p: DVec3, feet: bool) -> Option<DVec3> {
    holds
        .iter()
        .filter(|h| feet || h.kind != HoldKind::Foot)
        .min_by(|a, b| {
            (a.at - p)
                .length_squared()
                .total_cmp(&(b.at - p).length_squared())
        })
        .map(|h| h.at)
}

/// Catches the face `e` where the body is.
pub(crate) fn catch(
    m: &mut Mover,
    _p: &MovementProfile,
    b: &Body,
    _probe: &Probe,
    e: &Element,
    v: Vec3,
    out: &mut Outputs,
) -> Vec3 {
    let up = b.up;
    let n = e.normal;
    let (_, across, ..) = e.face_frame(up);
    let ice = e.kind == ElementKind::Ice;
    let reach = b.pos + (up * 0.95).as_dvec3();
    let center = on_face(e, up, reach, 0.3);
    let left = center - (across * 0.25).as_dvec3();
    let right = center + (across * 0.25).as_dvec3();
    let feet_c = center - (up * FEET_BELOW).as_dvec3();
    let fl = feet_c - (across * 0.2).as_dvec3();
    let fr = feet_c + (across * 0.2).as_dvec3();
    let (hands, feet) = if ice {
        (
            [on_face(e, up, left, 0.2), on_face(e, up, right, 0.2)],
            [on_face(e, up, fl, 0.2), on_face(e, up, fr, 0.2)],
        )
    } else {
        let holds = rock_holds(e, up);
        let h = |p: DVec3, feet: bool| nearest_hold(&holds, p, feet).unwrap_or(p);
        (
            [h(left, false), h(right, false)],
            [h(fl, true), h(fr, true)],
        )
    };
    let mut f = FaceClimb {
        element: e.entity,
        ice,
        hands,
        feet,
        moving: None,
        last_hand: 0,
        hand_moves: 0,
        secs: 0.0,
        center: b.pos,
    };
    f.center = b.pos;
    m.leave_elements();
    m.face = Some(f);
    m.air_jumps_left = 0;
    m.set_mode(LocomotionMode::Climb);
    out.trigger(b.entity, if ice { "pick" } else { "face_grab" });
    out.impact(
        b.entity,
        if ice {
            ImpactKind::Pick
        } else {
            ImpactKind::Catch
        },
        hands[1],
        n,
        v.length(),
    );
    let _ = across;
    Vec3::ZERO
}

/// In the air near a face, or walking into one, catches it.
#[allow(clippy::too_many_arguments)]
pub(crate) fn capture(
    m: &mut Mover,
    p: &MovementProfile,
    b: &Body,
    probe: &Probe,
    v: Vec3,
    wish: Vec3,
    grounded: bool,
    out: &mut Outputs,
) -> Option<Vec3> {
    let up = b.up;
    for e in probe.elements_near(b.pos, 1.0) {
        if !e.kind.is_face() || Some(e.entity) == m.elem_last {
            continue;
        }
        let n = e.normal;
        let (origin, across, fu, w, h) = e.face_frame(up);
        let rel = (b.pos - origin).as_vec3();
        let (x, y, off) = (rel.dot(across), rel.dot(fu), rel.dot(n));
        if x < 0.2 || x > w - 0.2 || y < -1.0 || y > h - 1.2 || !(0.0..0.9).contains(&off) {
            continue;
        }
        let into = flat(wish, up).dot(-n);
        let toward = v.dot(-n);
        if (grounded && into > 0.7) || (!grounded && (toward > 0.5 || into > 0.5)) {
            return Some(catch(m, p, b, probe, &e, v, out));
        }
    }
    None
}

/// One step on the face.
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
    let Some(mut f) = m.face else {
        return v;
    };
    let Some(e) = probe.element(f.element) else {
        crate::traverse::let_go(m, None, 0.0);
        return v;
    };
    f.secs += dt;
    let n = e.normal;
    let (_, across, fu, _, height) = e.face_frame(up);
    if input.knocked || (input.crouch_pressed && f.secs > 0.2 && f.moving.is_none()) {
        crate::traverse::let_go(m, Some(f.element), 0.5);
        out.trigger(b.entity, "let_go");
        return n * 1.0;
    }
    // The stick on the face: into the wall (forward) is up, sideways is across, pulled back
    // (or crouch held) is down.
    let wish = flat(input.wish, up);
    let mut dir2 = (wish.dot(across), wish.dot(-n));
    if input.crouch {
        dir2.1 = -1.0;
    }
    let len2 = (dir2.0 * dir2.0 + dir2.1 * dir2.1).sqrt();
    let dir = if len2 > 0.3 {
        (across * dir2.0 + fu * dir2.1).normalize_or_zero()
    } else {
        Vec3::ZERO
    };

    if input.jump_pressed && f.moving.is_none() {
        let hm = (f.hands[0] + f.hands[1]) * 0.5;
        if dir.dot(fu) > 0.6 && !f.ice {
            // Dyno: both hands to a hold up to DYNO_REACH higher.
            let holds = rock_holds(&e, up);
            let target = |hand: DVec3| {
                best_hold(
                    &holds,
                    hand,
                    fu,
                    (0.8, DYNO_REACH),
                    hm + (fu * 1.2).as_dvec3(),
                    1.2,
                    false,
                )
            };
            if let (Some(l), Some(r)) = (target(f.hands[0]), target(f.hands[1])) {
                f.moving = Some(LimbMove {
                    limb: 4,
                    from: f.hands,
                    to: [l, r],
                    t: 0.0,
                    secs: 0.4,
                });
                m.face = Some(f);
                out.trigger(b.entity, "dyno");
                return fu * 4.0;
            }
        } else if dir.dot(fu) < 0.6 {
            // Kick off the face (or leap to a target in reach).
            let push = n * 4.5 + up * p.jump_speed(0.9, b.g) + dir * 1.5;
            out.impact(b.entity, ImpactKind::Kick, f.feet[0], n, 4.0);
            return crate::target::jump_off(
                m,
                p,
                b,
                probe,
                input,
                Some(f.element),
                push,
                push,
                out,
            );
        }
    }

    // Advance the limb on the move; plant it at the end.
    if let Some(mut mv) = f.moving {
        mv.t += dt / mv.secs;
        if mv.t >= 1.0 {
            match mv.limb {
                0 | 1 => f.hands[mv.limb as usize] = mv.to[0],
                2 | 3 => f.feet[mv.limb as usize - 2] = mv.to[0],
                _ => f.hands = mv.to,
            }
            f.moving = None;
            let at = mv.to[0];
            if mv.limb < 2 || mv.limb == 4 {
                f.last_hand = if mv.limb == 4 { f.last_hand } else { mv.limb };
                f.hand_moves = f.hand_moves.saturating_add(1);
                let kind = if f.ice {
                    ImpactKind::Pick
                } else if mv.limb == 4 {
                    ImpactKind::Catch
                } else {
                    ImpactKind::Hold
                };
                out.impact(b.entity, kind, at, n, if mv.limb == 4 { 5.0 } else { 2.0 });
                out.trigger(b.entity, if f.ice { "pick" } else { "hold" });
            } else {
                f.hand_moves = 0;
                out.impact(b.entity, ImpactKind::Hold, at, n, 1.0);
                out.trigger(b.entity, if f.ice { "crampon" } else { "foothold" });
            }
        } else {
            f.moving = Some(mv);
        }
    }

    // At the top, climbing on: over the edge.
    let hm = (f.hands[0] + f.hands[1]) * 0.5;
    let (origin, ..) = e.face_frame(up);
    let hands_y = (hm - origin).as_vec3().dot(fu);
    if f.moving.is_none()
        && dir.dot(fu) > 0.5
        && hands_y > height - 0.55
        && let Some(v) = crate::locomotion::step_off_rope(m, p, b, probe, -n, out)
    {
        m.face = None;
        m.elem_last = Some(f.element);
        m.elem_cooldown = 0.5;
        return v;
    }
    // At the bottom, climbing down: step off.
    let fm = (f.feet[0] + f.feet[1]) * 0.5;
    if f.moving.is_none()
        && dir.dot(fu) < -0.5
        && (grounded || (fm - origin).as_vec3().dot(fu) < 0.25)
    {
        crate::traverse::let_go(m, Some(f.element), 0.6);
        m.set_mode(LocomotionMode::Idle);
        return n * 1.0;
    }

    // Start the next limb.
    if f.moving.is_none() && dir != Vec3::ZERO {
        start_move(&mut f, &e, up, dir, fu, across);
    }

    m.face = Some(f);
    m.trav_face = Some(flat(-n, up).normalize_or(m.facing));
    m.set_mode(LocomotionMode::Climb);
    let want = f.rest_center(n, up);
    // Ease the body toward where the holds put it (the hips lag the hands: weight).
    let k = 1.0 - (-7.0 * dt).exp();
    let target = f.center + (want - f.center) * k as f64;
    let moving = (target - f.center).as_vec3() / dt.max(1e-4);
    if let Some(face) = &mut m.face {
        face.center = target;
    }
    follow(b.pos, target, moving, 5.0, dt)
}

/// Picks the next limb to move toward `dir` (on the face) and where it goes.
fn start_move(f: &mut FaceClimb, e: &Element, up: Vec3, dir: Vec3, fu: Vec3, across: Vec3) {
    let hm = (f.hands[0] + f.hands[1]) * 0.5;
    let fm = (f.feet[0] + f.feet[1]) * 0.5;
    let span = (hm - fm).dot(up.as_dvec3()) as f32;
    let going_up = dir.dot(fu) > 0.3;
    let going_down = dir.dot(fu) < -0.3;
    // Feet first when the hands have got far above them (going up), or going down.
    let feet_turn = (going_up && (span > FEET_BELOW + 0.3 || f.hand_moves >= 2))
        || (going_down && span < FEET_BELOW + 0.35);
    let holds = if f.ice { Vec::new() } else { rock_holds(e, up) };
    if feet_turn {
        // The lower foot (going up), or the higher (going down), to under its hand.
        let i = if going_down {
            if f.feet[0].dot(up.as_dvec3()) > f.feet[1].dot(up.as_dvec3()) {
                0
            } else {
                1
            }
        } else if f.feet[0].dot(up.as_dvec3()) < f.feet[1].dot(up.as_dvec3()) {
            0
        } else {
            1
        };
        let side = if i == 0 { -0.2 } else { 0.2 };
        let want =
            hm - (up * FEET_BELOW).as_dvec3() + (across * side).as_dvec3() + (dir * 0.2).as_dvec3();
        // A foothold near under the hand, else any edge a little farther, else the foot
        // smears on the rock itself (friction, as climbers do on slabs).
        let to = if f.ice {
            Some(on_face(e, up, want, 0.15))
        } else {
            nearest_hold_within(&holds, want, 0.45)
                .or_else(|| nearest_hold_within(&holds, want, 0.75))
                .or_else(|| Some(on_face(e, up, want, 0.15)))
        };
        if let Some(to) = to {
            f.moving = Some(LimbMove {
                limb: 2 + i,
                from: [f.feet[i as usize]; 2],
                to: [to; 2],
                t: 0.0,
                secs: FOOT_SECS,
            });
            return;
        }
        f.hand_moves = 0;
    }
    // A hand: the one that leads the way (right going right), else the other than last.
    let i: u8 = if dir.dot(across) > 0.5 {
        1
    } else if dir.dot(across) < -0.5 {
        0
    } else if going_down {
        // The higher hand comes down.
        if f.hands[0].dot(up.as_dvec3()) > f.hands[1].dot(up.as_dvec3()) {
            0
        } else {
            1
        }
    } else {
        1 - f.last_hand
    };
    let from = f.hands[i as usize];
    let other = f.hands[1 - i as usize];
    let to = if f.ice {
        Some(on_face(e, up, from + (dir * PICK_REACH).as_dvec3(), 0.15))
            .filter(|p| (*p - from).length() > 0.15)
    } else {
        best_hold(&holds, from, dir, REACH, other, 1.05, false).or_else(|| {
            best_hold(
                &holds,
                from,
                dir,
                (REACH.0, REACH.1 + 0.15),
                other,
                1.3,
                false,
            )
        })
    };
    // Nothing for this hand: the other one tries (once).
    let (i, from, to) = match to {
        None if !f.ice => {
            let j = 1 - i;
            let from = f.hands[j as usize];
            let to = best_hold(
                &holds,
                from,
                dir,
                (REACH.0, REACH.1 + 0.15),
                f.hands[i as usize],
                1.3,
                false,
            );
            (j, from, to)
        }
        _ => (i, from, to),
    };
    if let Some(to) = to {
        f.moving = Some(LimbMove {
            limb: i,
            from: [from; 2],
            to: [to; 2],
            t: 0.0,
            secs: if f.ice { PICK_SECS } else { HAND_SECS },
        });
    }
}

fn nearest_hold_within(holds: &[crate::traverse::Hold], p: DVec3, r: f32) -> Option<DVec3> {
    nearest_hold(holds, p, true).filter(|h| (*h - p).length() < r as f64)
}
