//! The grapple hook, after Titanfall 2's: fire at a surface up to
//! [`grapple_range`](MovementProfile::grapple_range) away and the hook flies out, bites, and
//! the rope reels the character in.
//!
//! * **Firing.** Set [`Mover::pending_grapple`] to the point to hit (the player aims with
//!   the camera, a brain at its target). The hook flies at [`HOOK_SPEED`] from the shoulder;
//!   a shot at nothing flies to full range and is reeled back in.
//! * **Reeling in.** Once the hook bites, the rope pulls the character toward it at up to
//!   [`grapple_speed`](MovementProfile::grapple_speed) against gravity, never lets it swing
//!   further out than the rope (so a hook overhead swings it), and the stick steers across
//!   the rope. A character on the ground is lifted off it.
//! * **Moving targets.** A hook that bites a moving body (a ship, a lift, a train) moves with
//!   it, and the rope reels the character in relative to it.
//! * **Letting go.** Jump (an air jump, if one is left), crouch, or fire again lets go and
//!   keeps the momentum; so does arriving at the hook, the rope snagging on something, or
//!   holding on for [`MAX_SECS`]. A ledge in front at the end is climbed onto (or caught,
//!   when too high), which is how a pilot grapples up onto a roof. Then the hook recharges
//!   for [`grapple_cooldown_secs`](MovementProfile::grapple_cooldown_secs).

use engine_core::math::{DVec3, Vec3};
use engine_core::prelude::*;

use crate::locomotion::{Body, Outputs, Probe};
use crate::mover::{Grapple, Mover};
use crate::profile::MovementProfile;

/// How fast the hook flies, m/s.
pub const HOOK_SPEED: f32 = 90.0;
/// Longest the rope holds, own seconds.
pub const MAX_SECS: f32 = 3.0;
/// Recharge after a shot that hit nothing, s.
const MISS_COOLDOWN: f32 = 0.6;
/// Sideways steering on the rope, as a fraction of the air acceleration.
const STEER: f32 = 0.6;
/// Over the last meters to the hook the reel stops pulling once the body comes in faster
/// than it could stop at this deceleration, m/s², so arriving under a ledge doesn't fling it
/// high over it (a rope only ever pulls).
const ARRIVE_DECEL: f32 = 25.0;

/// Where the rope pulls on a body centered at `pos`: its shoulders.
pub fn shoulders(pos: DVec3, up: Vec3, height: f32) -> DVec3 {
    pos + (up * (height * 0.3)).as_dvec3()
}

pub(crate) fn origin(b: &Body) -> DVec3 {
    shoulders(b.pos, b.up, b.height)
}

/// Where the hook is now (it flies out from the [`shoulders`] at `from` toward its anchor).
pub fn hook_position(g: &Grapple, from: DVec3) -> DVec3 {
    let to = g.anchor - from;
    let dist = to.length();
    if g.attached || dist <= g.reach as f64 {
        g.anchor
    } else {
        from + to * (g.reach as f64 / dist.max(1e-6))
    }
}

/// Lets go of the hook (or calls it back) and starts the recharge.
pub(crate) fn release(m: &mut Mover, p: &MovementProfile, entity: Entity, out: &mut Outputs) {
    if m.grapple.take().is_some_and(|g| g.attached) {
        out.trigger(entity, "grapple_release");
    }
    m.grapple_cooldown = p.grapple_cooldown_secs;
}

fn fire(m: &mut Mover, p: &MovementProfile, b: &Body, probe: &Probe, at: DVec3, out: &mut Outputs) {
    let from = origin(b);
    let Some(dir) = (at - from).as_vec3().try_normalize() else {
        return;
    };
    let hit = probe.ray(from, dir, p.grapple_range);
    m.grapple = Some(match hit {
        Some(h) => Grapple {
            anchor: h.point,
            normal: h.normal,
            reach: 0.0,
            attached: false,
            miss: false,
            length: h.distance,
            secs: 0.0,
            on: probe.pin(h.entity, h.point, h.normal),
            anchor_velocity: Vec3::ZERO,
        },
        None => Grapple {
            anchor: from + (dir * p.grapple_range).as_dvec3(),
            normal: Vec3::ZERO,
            reach: 0.0,
            attached: false,
            miss: true,
            length: p.grapple_range,
            secs: 0.0,
            on: None,
            anchor_velocity: Vec3::ZERO,
        },
    });
    out.trigger(b.entity, "grapple_fire");
    out.noise(b.entity, from, 6.0);
}

/// The rope pulling this step.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct Reel {
    /// Velocity to move with, m/s (own time).
    pub velocity: Vec3,
    /// Reeled in all the way: let go (after a last look for a ledge to climb).
    pub arrived: bool,
}

/// Fires `shot` (the consumed [`Mover::pending_grapple`]), flies and reels in. Returns the
/// rope's pull while it holds; `None` while there is no hook out, it is still flying, or it
/// was just let go (the step goes on as usual).
#[allow(clippy::too_many_arguments)]
pub(crate) fn step(
    m: &mut Mover,
    p: &MovementProfile,
    b: &Body,
    probe: &Probe,
    shot: Option<DVec3>,
    v: Vec3,
    wish: Vec3,
    jump_pressed: bool,
    crouch_pressed: bool,
    grounded: bool,
    dt: f32,
    out: &mut Outputs,
) -> Option<Reel> {
    if let Some(at) = shot {
        if m.grapple.is_some() {
            release(m, p, b.entity, out);
            return None;
        }
        let busy = m.mantle.is_some() || m.hang.is_some() || m.dash.is_some();
        if p.grapple && m.grapple_cooldown <= 0.0 && !busy {
            fire(m, p, b, probe, at, out);
        }
    }
    let mut g = m.grapple?;
    let up = b.up;
    let from = origin(b);
    let to = (g.anchor - from).as_vec3();
    let dist = to.length();

    // ---- the hook in flight
    if !g.attached {
        g.reach += HOOK_SPEED * dt;
        if g.reach < dist {
            m.grapple = Some(g);
            return None;
        }
        if g.miss {
            // Out to full range and reeled back.
            m.grapple = None;
            m.grapple_cooldown = MISS_COOLDOWN;
            return None;
        }
        g.attached = true;
        g.length = dist;
        g.secs = 0.0;
        m.air_jumps_left = p.air_jumps;
        out.trigger(b.entity, "grapple_hit");
        out.noise(b.entity, g.anchor, 8.0);
    }

    // ---- letting go
    g.secs += dt;
    let dir = to / dist.max(1e-3);
    let snagged = dist > 1.0 && probe.ray(from, dir, dist - 0.5).is_some();
    if jump_pressed || crouch_pressed || snagged || g.secs > MAX_SECS {
        release(m, p, b.entity, out);
        return None;
    }

    // ---- reeling in, easing off near the hook. The rope pulls relative to the hook, which
    // moves with the body it bit into.
    let carry = g.anchor_velocity - m.platform.map_or(Vec3::ZERO, |d| d.velocity);
    let mut v = v - carry;
    let along = v.dot(dir);
    let speed = p
        .grapple_speed
        .min((2.0 * ARRIVE_DECEL * (dist - 0.5).max(0.0)).sqrt() + 2.0);
    v += dir * (speed - along).clamp(0.0, p.grapple_pull * dt);
    v -= up * (b.g * dt);
    if let Some(w) = wish.try_normalize() {
        let across = w - dir * w.dot(dir);
        v += across * (p.air_accel * STEER * wish.length().min(1.0) * dt);
    }
    // A taut rope: never further out than its length (a hook overhead swings the body).
    g.length = g.length.min(dist);
    if dist >= g.length - 0.02 {
        let away = v.dot(dir);
        if away < 0.0 {
            v -= dir * away;
        }
    }
    v = v.clamp_length_max(p.grapple_speed * 1.3) + carry;
    // Off the ground: the feet would otherwise stay pinned to it.
    if grounded && dir.dot(up) > 0.2 && v.dot(up) < 2.0 {
        v += up * (2.0 - v.dot(up));
    }
    m.grapple = Some(g);
    Some(Reel {
        velocity: v,
        arrived: dist < b.radius + 0.5,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_hook_flies_out_then_holds_at_its_anchor() {
        let mut g = Grapple {
            anchor: DVec3::new(0.0, 0.0, -10.0),
            normal: Vec3::Z,
            reach: 4.0,
            attached: false,
            miss: false,
            length: 10.0,
            secs: 0.0,
            on: None,
            anchor_velocity: Vec3::ZERO,
        };
        let from = DVec3::ZERO;
        assert!((hook_position(&g, from) - DVec3::new(0.0, 0.0, -4.0)).length() < 1e-9);
        g.reach = 12.0;
        assert_eq!(hook_position(&g, from), g.anchor);
        g.reach = 1.0;
        g.attached = true;
        assert_eq!(hook_position(&g, from), g.anchor);
    }
}
