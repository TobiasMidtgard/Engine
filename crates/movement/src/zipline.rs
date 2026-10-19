//! Zip lines, after Titanfall 2's and Apex Legends': a steel cable strung between two anchors.
//! Jump into one and the free hand catches it; the rider slides along hanging below it, faster
//! downhill, and lets go near the far end to fly on with the speed.
//!
//! * **A line** is an entity with a [`Zipline`] and a `Transform` at its near anchor; `to`
//!   reaches the far anchor in the entity's frame, so a line moves and turns with its transform
//!   (and follows world shifts). The cable sags [`Zipline::sag`] at mid-span and dips a little
//!   more under a rider ([`Cable::dip`]). [`crate::look::zipline`] draws it.
//! * **Catching it.** An airborne character with
//!   [`zipline`](MovementProfile::zipline) whose hand (just above its head, see [`GRIP_ABOVE`])
//!   comes within [`GRAB_RADIUS`] of a cable catches it, away from the anchors. It rides the way
//!   it was going along the line (or, barely moving, the way it faces), and gets its air jump
//!   back.
//! * **Riding.** The speed eases toward [`zip_speed`](MovementProfile::zip_speed), plus what
//!   the slope gives downhill (minus uphill); holding forward rides up to a quarter faster,
//!   holding back as much slower. The rider faces along the line.
//! * **Letting go.** Jump lets go with a jump, crouch drops off, and [`Zipline::end_clear`]
//!   before an anchor the rider lets go with a small hop ([`Zipline::exit_hop`]) and flies
//!   on. A rider held back by something on the way (it falls behind the cable) or touching the
//!   ground lets go too. A line let go of isn't caught again before the feet touch the ground.

use engine_core::math::{DVec3, Vec3};
use engine_core::prelude::*;

use crate::locomotion::{Body, Outputs, Probe};
use crate::mover::{Mover, Zip, flat};
use crate::profile::MovementProfile;

/// How close the hand must come to a cable to catch it, m.
pub const GRAB_RADIUS: f32 = 0.6;
/// The hand rides this far above the body's center, as a fraction of the capsule's height
/// (a 1.8 m pilot hangs with its feet 1.95 m below the cable).
pub const GRIP_ABOVE: f32 = 0.58;
/// No catching a line within this of either anchor, m.
const ANCHOR_CLEAR: f32 = 0.8;
/// How fast the rider's speed eases toward the line's speed, 1/s.
const SPEED_PULL: f32 = 1.0;
/// Slowest and fastest ride, m/s.
const MIN_SPEED: f32 = 2.0;
const MAX_SPEED: f32 = 30.0;
/// A rider starts at least this fast, m/s.
const ENTRY_SPEED: f32 = 4.0;
/// Fastest pull of the body under the cable after a catch, m/s.
const SNAP_SPEED: f32 = 6.0;
/// Held this far off where it should hang (something is in the way): let go, m.
const TORN_OFF: f32 = 1.2;
/// After letting go, no catching a line for this long, s.
const REGRAB_SECS: f32 = 0.4;
/// Deepest dip of the cable under a rider at mid-span, m (short lines dip less).
const LOAD_DIP: f32 = 0.35;

/// A zip line from this entity's translation (the near anchor) to `to` (the far anchor).
#[derive(Component, Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[require(Transform)]
#[serde(default)]
pub struct Zipline {
    /// The far anchor, from the near one, in the entity's frame (its rotation applies, its
    /// scale doesn't), m.
    pub to: Vec3,
    /// How far the cable sags below the straight line at mid-span, m.
    pub sag: f32,
    /// Riders let go this far before either anchor, m (room for the hardware and to land).
    pub end_clear: f32,
    /// Letting go at an end lifts the rider at this speed, m/s (over a ledge at the end, say).
    pub exit_hop: f32,
}

impl Default for Zipline {
    fn default() -> Self {
        Self {
            to: Vec3::new(20.0, -5.0, 0.0),
            sag: 0.4,
            end_clear: 1.5,
            exit_hop: 2.0,
        }
    }
}

/// A line's cable in world space.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Cable {
    /// Near and far anchors.
    pub a: DVec3,
    pub b: DVec3,
    /// Up (opposite gravity) the cable sags against.
    pub up: Vec3,
    pub sag: f32,
    pub end_clear: f32,
    pub exit_hop: f32,
}

impl Cable {
    pub fn new(t: &Transform, line: &Zipline, up: Vec3) -> Self {
        Self {
            a: t.translation,
            b: t.translation + (t.rotation * line.to).as_dvec3(),
            up,
            sag: line.sag.max(0.0),
            end_clear: line.end_clear.max(0.0),
            exit_hop: line.exit_hop,
        }
    }

    /// Anchor to anchor, m.
    pub fn span(&self) -> f32 {
        (self.b - self.a).length() as f32
    }

    /// The unloaded cable at `s` (0 = near anchor, 1 = far).
    pub fn at(&self, s: f32) -> DVec3 {
        let s = s.clamp(0.0, 1.0);
        self.a + (self.b - self.a) * s as f64
            - (self.up * (4.0 * self.sag * s * (1.0 - s))).as_dvec3()
    }

    /// d[`at`](Self::at)/ds: along the cable toward the far anchor, its length the meters per
    /// unit of `s`.
    pub fn tangent(&self, s: f32) -> Vec3 {
        (self.b - self.a).as_vec3() - self.up * (4.0 * self.sag * (1.0 - 2.0 * s))
    }

    /// How far a rider's weight at `s` pulls the cable down there, m.
    pub fn dip(&self, s: f32) -> f32 {
        let s = s.clamp(0.0, 1.0);
        LOAD_DIP.min(0.012 * self.span()) * 4.0 * s * (1.0 - s)
    }

    /// Where a rider's hand is at `s`: on the cable, pulled down by the rider.
    pub fn grip(&self, s: f32) -> DVec3 {
        self.at(s) - (self.up * self.dip(s)).as_dvec3()
    }

    /// The point of the unloaded cable closest to `p`: its `s` and the distance, m.
    pub fn closest(&self, p: DVec3) -> (f32, f32) {
        let n = ((self.span() / 0.5).ceil() as usize).clamp(8, 256);
        let mut best = (0.0, f64::MAX);
        let mut prev = self.at(0.0);
        for i in 1..=n {
            let (s0, s1) = ((i - 1) as f32 / n as f32, i as f32 / n as f32);
            let next = self.at(s1);
            let seg = next - prev;
            let len2 = seg.length_squared().max(1e-12);
            let k = ((p - prev).dot(seg) / len2).clamp(0.0, 1.0);
            let d = (prev + seg * k - p).length_squared();
            if d < best.1 {
                best = (s0 + (s1 - s0) * k as f32, d);
            }
            prev = next;
        }
        (best.0, best.1.sqrt() as f32)
    }
}

/// The hand that catches and holds a line: centered over the body (`pos`), `height` the
/// capsule's.
pub fn hand(pos: DVec3, up: Vec3, height: f32) -> DVec3 {
    pos + (up * (height * GRIP_ABOVE)).as_dvec3()
}

/// Where the body's center hangs from a hand at `grip`.
pub fn hanging(grip: DVec3, up: Vec3, height: f32) -> DVec3 {
    grip - (up * (height * GRIP_ABOVE)).as_dvec3()
}

/// Rides the line held, or catches one in reach. Returns the velocity to move with when the
/// character rode (or just let go of) a line this step; `None` leaves the step to the rest of
/// the locomotion.
#[allow(clippy::too_many_arguments)]
pub(crate) fn step(
    m: &mut Mover,
    p: &MovementProfile,
    b: &Body,
    probe: &Probe,
    v: Vec3,
    wish: Vec3,
    jump_pressed: bool,
    crouch_pressed: bool,
    grounded: bool,
    dt: f32,
    out: &mut Outputs,
) -> Option<Vec3> {
    m.zip_cooldown = (m.zip_cooldown - dt).max(0.0);
    if grounded {
        m.zip_last = None;
    }
    if m.zip.is_none() {
        if !p.zipline || grounded || m.zip_cooldown > 0.0 || m.grapple.is_some_and(|g| g.attached) {
            return None;
        }
        catch(m, p, b, probe, v, out)?;
    }
    Some(ride(
        m,
        p,
        b,
        probe,
        v,
        wish,
        jump_pressed,
        crouch_pressed,
        grounded,
        dt,
        out,
    ))
}

/// Catches the nearest line in reach of the hand. Returns `None` when there is none.
fn catch(
    m: &mut Mover,
    p: &MovementProfile,
    b: &Body,
    probe: &Probe,
    v: Vec3,
    out: &mut Outputs,
) -> Option<()> {
    let at = hand(b.pos, b.up, b.height);
    let mut best: Option<(Entity, Cable, f32, f32)> = None;
    for (line, cable) in probe.cables(b.up) {
        if m.zip_last == Some(line) {
            continue;
        }
        let span = cable.span();
        if span < 2.0 * ANCHOR_CLEAR {
            continue;
        }
        let (s, d) = cable.closest(at);
        let from_a = s * span;
        let from_b = (1.0 - s) * span;
        let clear = ANCHOR_CLEAR.max(cable.end_clear);
        // Away from the anchors (and from where a rider would let go at once).
        if d > GRAB_RADIUS || from_a < ANCHOR_CLEAR || from_b < ANCHOR_CLEAR {
            continue;
        }
        let dir = ride_dir(m, v, cable.tangent(s));
        let left = if dir > 0.0 { from_b } else { from_a };
        if left < clear + 0.5 {
            continue;
        }
        if best.is_none_or(|(_, _, _, bd)| d < bd) {
            best = Some((line, cable, s, d));
        }
    }
    let (line, cable, s, _) = best?;
    let along = cable.tangent(s).normalize_or_zero();
    let dir = ride_dir(m, v, along);
    m.zip = Some(Zip {
        line: Some(line),
        s,
        dir,
        speed: (v.dot(along) * dir).max(ENTRY_SPEED),
        heading: along * dir,
        secs: 0.0,
    });
    m.air_jumps_left = p.air_jumps;
    m.set_mode(engine_world::LocomotionMode::Air);
    out.trigger(b.entity, "zip_grab");
    out.noise(b.entity, at, 4.0);
    Some(())
}

/// Which way a catch rides along `along`: the way the character moves, or barely moving, the
/// way it faces (+1 toward the far anchor).
fn ride_dir(m: &Mover, v: Vec3, along: Vec3) -> f32 {
    let along = along.normalize_or_zero();
    if v.dot(along).abs() > 1.0 {
        v.dot(along).signum()
    } else if m.facing.dot(along).abs() > 0.1 {
        m.facing.dot(along).signum()
    } else {
        1.0
    }
}

/// One step on the line held: let go, or slide along it. Returns the velocity.
#[allow(clippy::too_many_arguments)]
fn ride(
    m: &mut Mover,
    p: &MovementProfile,
    b: &Body,
    probe: &Probe,
    v: Vec3,
    wish: Vec3,
    jump_pressed: bool,
    crouch_pressed: bool,
    grounded: bool,
    dt: f32,
    out: &mut Outputs,
) -> Vec3 {
    let up = b.up;
    let Some(mut z) = m.zip else {
        return v;
    };
    let Some(cable) = z.line.and_then(|l| probe.cable(l, up)) else {
        // The line is gone: fall.
        let_go(m, b, out, "zip_release");
        return v;
    };
    let span = cable.span().max(1e-3);
    let tangent = cable.tangent(z.s);
    let per_s = tangent.length().max(1e-3);
    let heading = tangent / per_s * z.dir;
    let moving = heading * z.speed;

    if jump_pressed {
        let_go(m, b, out, "jump");
        // A jump off the line, like one off the ground: the speed along it stays.
        let launch = MovementProfile::launch_speed(p.jump_height, b.g);
        return flat(moving, up) + up * launch.max(moving.dot(up));
    }
    if crouch_pressed {
        let_go(m, b, out, "zip_release");
        return moving;
    }
    // Held back by something (or standing on the ground): the hand slips off.
    let hang = hanging(cable.grip(z.s), up, b.height);
    if z.secs > 0.25 && ((b.pos - hang).length() as f32 > TORN_OFF || grounded) {
        let_go(m, b, out, "zip_release");
        return v;
    }

    // Faster downhill, slower uphill; the stick asks for a little more or less.
    let downhill = -b.g * heading.dot(up);
    let ask = flat(heading, up)
        .try_normalize()
        .map_or(0.0, |h| wish.dot(h).clamp(-1.0, 1.0));
    let target = p.zip_speed * (1.0 + 0.25 * ask);
    z.speed =
        (z.speed + (downhill + (target - z.speed) * SPEED_PULL) * dt).clamp(MIN_SPEED, MAX_SPEED);
    z.s = (z.s + z.dir * z.speed * dt / per_s).clamp(0.0, 1.0);
    z.secs += dt;
    z.heading = heading;
    let moving = heading * z.speed;

    // Near the far end: let go with a hop and fly on.
    let left = if z.dir > 0.0 { 1.0 - z.s } else { z.s } * span;
    if left <= cable.end_clear.max(ANCHOR_CLEAR * 0.5) {
        let_go(m, b, out, "zip_release");
        return moving + up * cable.exit_hop;
    }
    m.zip = Some(z);
    m.set_mode(engine_world::LocomotionMode::Air);

    // Hang under the cable: move along it, and close what gap is left after the catch (or
    // after a bump) at up to SNAP_SPEED.
    let want = hanging(cable.grip(z.s), up, b.height);
    let free = b.pos + (moving * dt).as_dvec3();
    let fix = (want - free).as_vec3().clamp_length_max(SNAP_SPEED * dt);
    moving + fix / dt
}

pub(crate) fn let_go(m: &mut Mover, b: &Body, out: &mut Outputs, trigger: &str) {
    m.zip_last = m.zip.take().and_then(|z| z.line);
    m.zip_cooldown = REGRAB_SECS;
    m.set_mode(engine_world::LocomotionMode::Air);
    out.trigger(b.entity, trigger);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cable() -> Cable {
        Cable {
            a: DVec3::new(0.0, 10.0, 0.0),
            b: DVec3::new(20.0, 2.0, 0.0),
            up: Vec3::Y,
            sag: 0.5,
            end_clear: 1.5,
            exit_hop: 2.0,
        }
    }

    #[test]
    fn the_cable_sags_between_its_anchors() {
        let c = cable();
        assert_eq!(c.at(0.0), c.a);
        assert_eq!(c.at(1.0), c.b);
        // Mid-span: half way down, plus the sag.
        assert!((c.at(0.5).y - 5.5).abs() < 1e-6);
        // A rider dips it a little more, never at the anchors.
        assert!(c.grip(0.5).y < c.at(0.5).y - 0.2);
        assert_eq!(c.grip(0.0), c.a);
    }

    #[test]
    fn closest_point_finds_the_cable_under_a_hand() {
        let c = cable();
        let on = c.at(0.3);
        let (s, d) = c.closest(on + DVec3::new(0.0, -0.4, 0.2));
        assert!((s - 0.3).abs() < 0.02, "s {s}");
        assert!(
            (d - (0.4f32 * 0.4 + 0.2 * 0.2).sqrt()).abs() < 0.05,
            "d {d}"
        );
    }

    #[test]
    fn tangent_points_down_the_line() {
        let c = cable();
        let t = c.tangent(0.5).normalize();
        assert!(t.x > 0.9 && t.y < -0.3);
        // Steeper near the top than near the bottom (the sag).
        assert!(c.tangent(0.1).normalize().y < c.tangent(0.9).normalize().y);
    }
}
