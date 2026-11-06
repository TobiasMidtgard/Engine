//! Targeted jumps: "jumping between elements works flawlessly" (research-mechanics.md §2).
//!
//! A jump pressed on the ground, off a perch, a pole, a bar, a line or a rope looks for the
//! element it is meant for, and if it finds one flies an arc that lands exactly on it:
//!
//! 1. **Candidates** in a ±40° cone around the stick (the camera's way with the stick idle),
//!    up to [`REACH_RUN`] away running or [`REACH_STAND`] from a standstill, from 12 m below to
//!    3 m above: perch tops, poles, bars, lines to land on, rock and ice faces, awnings, hay.
//! 2. **Score**: `3·stick + 1·camera + 1.5·height + 1·momentum + 1·kind − dist/reach`, the
//!    target chosen last time +15% so the choice doesn't flicker (Assassin's Creed's parkour up
//!    prefers high, crouch held prefers low).
//! 3. **Arc**: a ballistic arc to the catch point, rising under the profile's rising gravity
//!    and falling under its falling gravity, accepted when its horizontal speed is within what
//!    the body can give ([`Leap`]).
//! 4. **In flight** the velocity is nudged (at most [`CORRECT`] m/s²) to keep the arc on the
//!    target, as Dying Light 2's landing prediction does, and over the last [`WARP_SECS`] the
//!    body is warped onto the catch point (motion warping), so a jump that was good arrives.
//! 5. **Arrival** hands the body to the element's state ([`arrive`]).

use engine_core::math::{DVec3, Vec3};
use engine_core::prelude::*;
use engine_world::LocomotionMode;

use crate::impact::ImpactKind;
use crate::locomotion::{Body, Outputs, Probe};
use crate::mover::{Mover, flat};
use crate::profile::MovementProfile;
use crate::traverse::{Element, ElementKind, nearest_on_segment};
use crate::{balance, bar, face, perch, pole};

/// Farthest target from a run, and from a standstill (or a perch, a bar), m.
pub const REACH_RUN: f32 = 8.0;
pub const REACH_STAND: f32 = 5.5;
/// Half-angle of the cone targets are looked for in, rad (off an element), and from the ground.
pub const CONE: f32 = 0.7;
pub const GROUND_CONE: f32 = 0.45;
/// Lowest and highest target below and above the feet, m.
pub const BELOW: f32 = 12.0;
pub const ABOVE: f32 = 3.0;
/// Fastest the stick may build a launch above what the body has, as a multiple.
const SPEED_SLACK: f32 = 1.15;
/// Launch speed a perch, bar, pole or line adds to a standstill, m/s.
const SPRING: f32 = 2.5;
/// Largest in-flight correction, m/s² (horizontal; half that vertical).
pub const CORRECT: f32 = 6.0;
/// The last part of the flight warped onto the catch point, s.
pub const WARP_SECS: f32 = 0.12;
/// Farther than this off the target when the warp starts, the leap is given up, m.
pub const WARP_GAP: f32 = 0.5;
/// The target chosen last time scores this much more (hysteresis).
const KEEP_BONUS: f32 = 0.15;

/// What a leap flies to.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Aim {
    /// Crouch on a perch's top.
    Perch,
    /// Grab an upright pole with the hands `s` m above its foot.
    Pole { s: f32 },
    /// Catch a bar (or a washing line) `s` m along it.
    Bar { s: f32 },
    /// Land on a line to walk, `s` m along it.
    Line { s: f32 },
    /// Catch a rock or ice face at the body's position.
    Face,
    /// Land on an awning or in hay.
    Soft,
}

/// A leap in flight to an element.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Leap {
    pub element: Entity,
    pub aim: Aim,
    /// Where the body's center should be on arrival.
    pub to: DVec3,
    /// Own seconds flown, and the flight's planned length.
    pub secs: f32,
    pub total: f32,
    /// Gravity rising and falling, m/s².
    pub g_up: f32,
    pub g_down: f32,
    /// How high above the higher end the arc peaks, m (raised to clear a ledge on the way).
    pub extra: f32,
}

/// A solved arc.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Arc {
    pub v0: Vec3,
    pub total: f32,
}

/// The arc from `from` to `to` peaking `extra` m above the higher end (rising at `g_up`,
/// falling at `g_down`).
pub fn solve(from: DVec3, to: DVec3, up: Vec3, g_up: f32, g_down: f32, extra: f32) -> Arc {
    let d = (to - from).as_vec3();
    let dy = d.dot(up);
    let dh = d - up * dy;
    let apex = dy.max(0.0) + extra.max(0.05);
    let vy = (2.0 * g_up * apex).sqrt();
    let t_up = vy / g_up;
    let t_down = (2.0 * (apex - dy) / g_down).sqrt();
    let total = (t_up + t_down).max(0.1);
    Arc {
        v0: dh / total + up * vy,
        total,
    }
}

/// How high above the higher end an arc of horizontal length `dh` and drop `dy` peaks, m.
pub(crate) fn extra_for(dh: f32, dy: f32) -> f32 {
    if dy < -1.0 {
        0.3 + 0.04 * dh
    } else {
        (0.35 + 0.1 * dh).clamp(0.45, 1.1)
    }
}

/// Where an arc launched at `v0` from `from` is after `t` s.
fn arc_at(from: DVec3, v0: Vec3, up: Vec3, g_up: f32, g_down: f32, t: f32) -> DVec3 {
    let vy = v0.dot(up);
    from + ((v0 - up * vy) * t + up * predict_drop(vy, t, g_up, g_down)).as_dvec3()
}

/// Whether the feet of an arc pass over everything on the way down (the takeoff roof's
/// edge, a cornice, a parapet), up to the last stretch where it meets its target.
fn clears(probe: &Probe, from: DVec3, arc: Arc, up: Vec3, g_up: f32, g_down: f32) -> bool {
    const STEPS: usize = 10;
    // The bottom of the capsule (body center is 0.9 m above the soles, the capsule 0.35 m
    // round) at its leading edge, middle and trailing edge, a little low for margin.
    let along = flat(arc.v0, up).normalize_or_zero();
    let at = |t: f32| arc_at(from, arc.v0, up, g_up, g_down, t);
    for off in [-0.35, 0.0, 0.35] {
        let feet = |p: DVec3| p + (along * off - up * 0.95).as_dvec3();
        // (From the first sample on: the takeoff point itself is on the ground.)
        let mut last = feet(at(arc.total / STEPS as f32));
        for i in 2..=(STEPS * 4 / 5) {
            let next = feet(at(arc.total * i as f32 / STEPS as f32));
            let d = (next - last).as_vec3();
            if probe.ray(last, d.normalize_or_zero(), d.length()).is_some() {
                return false;
            }
            last = next;
        }
    }
    true
}

/// The arc from `from` to `to`, peaking `extra` m up, or higher if that is what it takes to
/// clear the ledge it leaps from (a leap down off a roof arcs out over the edge instead of
/// touching down on it). Returns the peak height used.
fn clear_arc(
    probe: &Probe,
    from: DVec3,
    to: DVec3,
    up: Vec3,
    g_up: f32,
    g_down: f32,
    extra: f32,
) -> (f32, Arc) {
    let base = solve(from, to, up, g_up, g_down, extra);
    if (to - from).as_vec3().dot(up) > -1.0 || clears(probe, from, base, up, g_up, g_down) {
        return (extra, base);
    }
    for raise in [0.3, 0.6, 1.0, 1.4] {
        let arc = solve(from, to, up, g_up, g_down, extra + raise);
        if clears(probe, from, arc, up, g_up, g_down) {
            return (extra + raise, arc);
        }
    }
    (extra, base)
}

/// What the jumper wants: where the stick points (else the camera, else the facing), and
/// how it moves.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Want {
    pub wish: Vec3,
    pub look: Option<Vec3>,
    /// Crouch held: parkour down (prefer low targets).
    pub down: bool,
    /// Standing on or hanging from this element: not a target.
    pub from: Option<Entity>,
    /// Launching off a perch, a bar, a pole or a line (they add a spring to a standstill).
    pub sprung: bool,
    /// Half-angle of the cone around the stick, rad.
    pub cone: f32,
}

impl Want {
    /// A jump from the ground (or just off it): only along the stick, a narrower cone (a
    /// jump that isn't meant for an element stays a free jump).
    pub(crate) fn ground(wish: Vec3, intent: &engine_world::locomotion::MoveIntent) -> Self {
        Self {
            wish,
            look: intent.look,
            down: intent.crouch,
            from: None,
            sprung: false,
            cone: GROUND_CONE,
        }
    }
}

/// A candidate's catch point (body center) for element `e`, for a jumper at `pos` wanting
/// to go along `dir`.
fn catch_point(e: &Element, b: &Body, dir: Vec3) -> Option<(Aim, DVec3)> {
    let up = b.up;
    let half = b.height * 0.5;
    let grip = b.height * crate::zipline::GRIP_ABOVE;
    // A point along a segment element: where the line the jumper looks along passes nearest.
    let along = |e: &Element| {
        let ahead = b.pos + (dir * 3.0).as_dvec3();
        let (p, t) = nearest_on_segment(e.a, e.b, ahead);
        // Also the nearest to the body itself: prefer whichever is ahead.
        let (q, u) = nearest_on_segment(e.a, e.b, b.pos);
        if (q - b.pos).as_vec3().dot(dir) > 0.5 && (q - b.pos).length() < (p - b.pos).length() {
            (q, u)
        } else {
            (p, t)
        }
    };
    match e.kind {
        ElementKind::Perch => Some((Aim::Perch, e.a + (up * half).as_dvec3())),
        ElementKind::PoleV => {
            let len = e.length();
            // Hands at about the height of a standing reach above the take-off, on the shaft.
            // Hands up the shaft about a jump's height above a standing reach, the feet well
            // off the ground.
            let hands = (b.pos - e.a).dot(up.as_dvec3()) as f32 + 1.6;
            let s = hands.clamp(2.4_f32.min(len), (len - 0.3).max(0.0));
            let axis = e.a + (e.dir(up) * s).as_dvec3();
            let back = flat((b.pos - axis).as_vec3(), up).normalize_or(-dir);
            let center = axis - (up * grip).as_dvec3() + (back * (e.width * 0.5 + 0.37)).as_dvec3();
            Some((Aim::Pole { s }, center))
        }
        ElementKind::PoleH | ElementKind::Clothesline => {
            let (_, t) = along(e);
            // Clear of whatever holds its ends up.
            let len = e.length();
            let s = (t * len).clamp(0.6_f32.min(len * 0.5), (len - 0.6).max(len * 0.5));
            let p = e.at(s);
            let sag = e.rest_sag(s);
            let center = p - (up * (grip + sag)).as_dvec3();
            Some((Aim::Bar { s }, center))
        }
        k if k.is_walkable_line() => {
            let (p, t) = along(e);
            let s = t * e.length();
            let top = e.top_at(s, e.rest_sag(s), up);
            let _ = p;
            Some((Aim::Line { s }, top + (up * half).as_dvec3()))
        }
        ElementKind::Rock | ElementKind::Ice => {
            // On the face at about head height above the take-off, in line with the body.
            let (origin, across, fu, w, h) = e.face_frame(up);
            let rel = (b.pos - origin).as_vec3();
            let x = rel.dot(across).clamp(0.4, (w - 0.4).max(0.4));
            let y = (rel.dot(fu) + 1.2).clamp(1.0_f32.min(h), (h - 0.6).max(0.0));
            let on = origin + (across * x + fu * y).as_dvec3();
            Some((Aim::Face, on + (e.normal * 0.35 - up * 0.35).as_dvec3()))
        }
        ElementKind::Awning | ElementKind::Haystack => {
            let c = if e.kind == ElementKind::Awning {
                e.a + (e.b - e.a) * 0.5
            } else {
                e.a
            };
            Some((Aim::Soft, c + (up * half).as_dvec3()))
        }
        _ => None,
    }
}

/// How much each kind is wanted (perches first: lamp post to lamp post).
fn kind_priority(k: ElementKind) -> f32 {
    match k {
        ElementKind::Perch => 1.0,
        ElementKind::PoleV | ElementKind::PoleH => 0.8,
        ElementKind::Beam | ElementKind::Rail | ElementKind::Tightrope | ElementKind::Branch => 0.7,
        ElementKind::Clothesline => 0.5,
        ElementKind::Rock | ElementKind::Ice => 0.6,
        ElementKind::Awning | ElementKind::Haystack => 0.4,
    }
}

/// Picks the element a jump now is meant for, if any, and the arc to it.
pub(crate) fn pick(
    m: &Mover,
    p: &MovementProfile,
    b: &Body,
    probe: &Probe,
    v: Vec3,
    want: Want,
) -> Option<Leap> {
    let up = b.up;
    let vh = flat(v, up);
    let speed = vh.length();
    let look = want
        .look
        .map(|l| flat(l, up))
        .and_then(|l| l.try_normalize());
    let dir = if want.wish.length() > 0.3 {
        want.wish.normalize()
    } else {
        look.unwrap_or(m.facing)
    };
    let running = speed > p.run_speed * 0.6;
    let reach = if running || want.sprung {
        REACH_RUN
    } else {
        REACH_STAND
    };
    let g_up = b.g * p.rise_gravity;
    let g_down = b.g * p.fall_gravity;
    let vmax = speed.max(p.run_speed) * SPEED_SLACK + if want.sprung { SPRING } else { 0.0 };
    let vy_max = p.held_jump_speed(p.jump_height * 1.35, b.g);
    let mut best: Option<(f32, Leap)> = None;
    // (Out to the reach across and the deepest drop below: the precise limits are below.)
    for e in probe.elements_near(b.pos, (reach + 1.5).hypot(BELOW)) {
        if Some(e.entity) == want.from || Some(e.entity) == m.elem_last {
            continue;
        }
        let Some((aim, to)) = catch_point(&e, b, dir) else {
            continue;
        };
        let d = (to - b.pos).as_vec3();
        let dy = d.dot(up);
        let dh = d - up * dy;
        let dist = dh.length();
        if dist > reach || dy > ABOVE || dy < -BELOW {
            continue;
        }
        // Too close to need a jump (and straight up is a climb, not a leap).
        if dist < 0.6 && dy.abs() < 0.6 {
            continue;
        }
        let to_dir = dh.try_normalize().unwrap_or(dir);
        let stick = to_dir.dot(dir);
        if dist > 0.8 && stick < want.cone.cos() {
            continue;
        }
        let (extra, arc) = clear_arc(probe, b.pos, to, up, g_up, g_down, extra_for(dist, dy));
        let vy = arc.v0.dot(up);
        let need = flat(arc.v0, up).length();
        if need > vmax || vy > vy_max {
            continue;
        }
        let cam = look.map_or(0.0, |l| to_dir.dot(l));
        let height = if want.down { -dy } else { dy } / 5.0;
        let momentum = if speed > 1.0 {
            to_dir.dot(vh / speed)
        } else {
            0.0
        };
        let mut score =
            3.0 * stick + cam + 1.5 * height.clamp(-1.0, 1.0) + momentum + kind_priority(e.kind)
                - dist / reach;
        if m.target_last == Some(e.entity) {
            score *= 1.0 + KEEP_BONUS;
        }
        if best.is_none_or(|(s, _)| score > s) {
            best = Some((
                score,
                Leap {
                    element: e.entity,
                    aim,
                    to,
                    secs: 0.0,
                    total: arc.total,
                    g_up,
                    g_down,
                    extra,
                },
            ));
        }
    }
    best.map(|(_, l)| l)
}

/// Starts the leap `l`: the launch velocity of its arc.
pub(crate) fn launch(m: &mut Mover, b: &Body, l: Leap, out: &mut Outputs) -> Vec3 {
    let arc = solve(b.pos, l.to, b.up, l.g_up, l.g_down, l.extra);
    m.leap = Some(Leap {
        total: arc.total,
        ..l
    });
    m.target_last = Some(l.element);
    m.jump_buffer = 0.0;
    m.coyote = 0.0;
    m.jumping = false;
    m.set_mode(LocomotionMode::Air);
    m.air_cap = flat(arc.v0, b.up).length();
    out.trigger(b.entity, "leap");
    out.noise(b.entity, b.feet(), 4.0);
    arc.v0
}

/// Jumping off the element `from` the body is on (perch, pole, bar, line, face): a leap to
/// the element meant if there is one, else `fallback` (a plain jump).
#[allow(clippy::too_many_arguments)]
pub(crate) fn jump_off(
    m: &mut Mover,
    p: &MovementProfile,
    b: &Body,
    probe: &Probe,
    input: &crate::traverse::Input,
    from: Option<Entity>,
    base: Vec3,
    fallback: Vec3,
    out: &mut Outputs,
) -> Vec3 {
    let want = Want {
        wish: input.wish,
        look: input.look,
        down: input.crouch,
        from,
        sprung: true,
        cone: CONE,
    };
    let leap = pick(m, p, b, probe, base, want);
    crate::traverse::let_go(m, from, 0.35);
    m.jump_buffer = 0.0;
    if let Some(l) = leap {
        out.impact(b.entity, ImpactKind::Launch, b.feet(), b.up, 0.0);
        return launch(m, b, l, out);
    }
    m.jumping = false;
    m.air_cap = flat(fallback, b.up).length();
    out.trigger(b.entity, "jump");
    fallback
}

/// One step of a leap in flight: the velocity, or `None` when it has arrived (the element's
/// state took over: its velocity is in `Some` of the second value) or was given up.
#[allow(clippy::too_many_arguments)]
pub(crate) fn fly(
    m: &mut Mover,
    p: &MovementProfile,
    b: &Body,
    probe: &Probe,
    v: Vec3,
    blocked: bool,
    dt: f32,
    out: &mut Outputs,
) -> Option<Vec3> {
    let mut l = m.leap?;
    let up = b.up;
    let Some(e) = probe.element(l.element) else {
        m.leap = None;
        return None;
    };
    l.secs += dt;
    let left = l.total - l.secs;
    let gap = (l.to - b.pos).as_vec3();
    // Arrived (or overdue and close): hand over to the element.
    if left <= dt * 0.5 || gap.length() < 0.12 {
        m.leap = None;
        return Some(arrive(m, p, b, probe, &e, l.aim, v, out));
    }
    if blocked && l.secs > 0.1 {
        // Something in the way: fall as a normal flight (the ledge catch may still save it).
        m.leap = None;
        return None;
    }
    m.leap = Some(l);
    m.set_mode(LocomotionMode::Air);
    if left < WARP_SECS {
        if gap.length() > WARP_GAP {
            // Held up on the way (snagged on something): no teleport, fall as a normal
            // flight.
            m.leap = None;
            return None;
        }
        // Warp: arrive exactly, the velocity that closes the gap in the time left.
        return Some(gap / left.max(dt));
    }
    // Keep the arc on the target: the horizontal velocity that gets there in the time left,
    // approached at most CORRECT m/s²; the vertical under gravity with a gentle trim.
    let gh = gap - up * gap.dot(up);
    let want_h = gh / left;
    let vh = flat(v, up);
    let vh = vh + (want_h - vh).clamp_length_max(CORRECT * dt);
    let vy = v.dot(up);
    let g = if vy > 0.0 { l.g_up } else { l.g_down };
    let mut vy = vy - g * dt;
    // Where gravity alone ends the flight, vertically; trim toward the target.
    let end = predict_drop(vy, left, l.g_up, l.g_down);
    let err = gap.dot(up) - end;
    vy += (err / left).clamp(-CORRECT * 0.5 * dt, CORRECT * 0.5 * dt);
    let _ = p;
    Some(vh + up * vy)
}

/// Height gained over `t` seconds starting at vertical speed `vy`, rising at `g_up` and
/// falling at `g_down`.
fn predict_drop(vy: f32, t: f32, g_up: f32, g_down: f32) -> f32 {
    if vy > 0.0 {
        let t_up = (vy / g_up).min(t);
        let rise = vy * t_up - 0.5 * g_up * t_up * t_up;
        let t2 = t - t_up;
        let v2 = vy - g_up * t_up;
        rise + v2 * t2 - 0.5 * g_down * t2 * t2
    } else {
        vy * t - 0.5 * g_down * t * t
    }
}

/// The leap arrived at `e`: the element's state takes the body.
#[allow(clippy::too_many_arguments)]
pub(crate) fn arrive(
    m: &mut Mover,
    p: &MovementProfile,
    b: &Body,
    probe: &Probe,
    e: &Element,
    aim: Aim,
    v: Vec3,
    out: &mut Outputs,
) -> Vec3 {
    let speed = v.length();
    match aim {
        Aim::Perch => perch::land(m, b, e, v, out),
        Aim::Pole { s } => pole::catch(m, b, e, s, v, out),
        Aim::Bar { s } => bar::catch(m, b, e, s, v, out),
        Aim::Line { s } => balance::land(m, b, e, s, v, out),
        Aim::Face => face::catch(m, p, b, probe, e, v, out),
        Aim::Soft if e.kind == ElementKind::Awning => {
            m.set_mode(LocomotionMode::Air);
            crate::soft::bounce(m, b, e, v, out)
        }
        Aim::Soft => {
            // Hay: on into it (the landing finds it soft).
            let _ = speed;
            m.set_mode(LocomotionMode::Air);
            v
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_arc_lands_on_its_target() {
        let up = Vec3::Y;
        let from = DVec3::new(0.0, 1.0, 0.0);
        for to in [
            DVec3::new(4.0, 1.0, 0.0),
            DVec3::new(3.0, 3.5, 2.0),
            DVec3::new(5.0, -4.0, 0.0),
        ] {
            let (g_up, g_down) = (14.7, 20.6);
            let d = (to - from).as_vec3();
            let arc = solve(
                from,
                to,
                up,
                g_up,
                g_down,
                extra_for(flat(d, up).length(), d.y),
            );
            // Integrate it.
            let mut p = from;
            let mut v = arc.v0;
            let dt = 1.0 / 240.0;
            let mut t = 0.0;
            while t < arc.total - 1e-4 {
                let g = if v.y > 0.0 { g_up } else { g_down };
                v.y -= g * dt;
                p += (v * dt).as_dvec3();
                t += dt;
            }
            assert!((p - to).length() < 0.15, "{to:?} → {p:?}");
        }
    }

    #[test]
    fn the_drop_prediction_matches_the_integration() {
        let (g_up, g_down) = (14.7, 20.6);
        let mut y = 0.0f32;
        let mut v = 4.0f32;
        let dt = 1.0 / 1000.0;
        for _ in 0..600 {
            let g = if v > 0.0 { g_up } else { g_down };
            v -= g * dt;
            y += v * dt;
        }
        assert!((predict_drop(4.0, 0.6, g_up, g_down) - y).abs() < 0.03);
    }
}
