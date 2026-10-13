//! Ladders: anything with a [`Climbable`](engine_world::locomotion::Climbable), placed by any
//! module (a ladder prop, rungs up a tower or a ship's side, a hatch). A `Climbable` is a climb
//! line: where a climber's feet are on its lowest and top rungs, which way the climber faces
//! and where it steps off at the top.
//!
//! * **Getting on.** Walking into the bottom of the line toward the ladder grabs it, and so
//!   does jumping or falling into it anywhere along it. Walking back over the top toward the
//!   ladder, from its exit, grabs it at the top (the climber turns to face it). Grabbing gives
//!   the air jump back.
//! * **Climbing.** Forward climbs and back climbs down (the [`MoveIntent`] direction against the
//!   way the ladder faces) at the profile's
//!   [`ladder_speed`](MovementProfile::ladder_speed), half as fast again sprinting. Holding
//!   crouch slides down with the hands and feet on the rails.
//! * **Getting off.** Climbing past the top rung steps over onto the exit (a short mantle);
//!   climbing below the lowest rung, or onto the ground, steps off. Jump kicks off backward,
//!   steered sideways by the stick; a dash or a hard hit throws the climber off. A ladder whose
//!   component goes (shot away, say) drops its climbers.
//!
//! The body is held on the line, so it moves and turns with the ladder's entity (a ladder on
//! a moving ship carries its climber). A rung [`GroundContact`](engine_world::GroundContact)
//! goes out every [`RUNG`] climbed, on the ladder's collider surface (else metal), so audio
//! can ring the rungs.
//!
//! [`MoveIntent`]: engine_world::locomotion::MoveIntent

use engine_core::math::{DVec3, Vec3};
use engine_core::prelude::*;
use engine_world::locomotion::ClimbLine;
use engine_world::{ContactKind, GroundContact, LocomotionMode};

use crate::locomotion::{Body, Outputs, Probe};
use crate::mover::{Ladder, Mantle, Mover, flat};
use crate::profile::MovementProfile;

/// Rung spacing the climb is counted in (rung contacts, the hands and feet of the demo's
/// fighters), m.
pub const RUNG: f32 = 0.3;
/// Rungs are this far in front of the climb line (toward the ladder), m.
pub const RUNG_AHEAD: f32 = 0.2;
/// Grabbing: how far in front of the climb line the capsule's front may be, m.
const GRAB_FRONT: f32 = 0.3;
/// Grabbing: how far past the line (into the ladder) the feet may be, m.
const GRAB_BEHIND: f32 = 0.3;
/// Grabbing: how far beyond the ladder's half width the body may be to the side, m.
const GRAB_SIDE: f32 = 0.15;
/// Grabbing from the ground: the feet may be this far below the lowest rung, m (it is
/// usually a step up off the ground).
const GRAB_BELOW: f32 = 0.6;
/// Grabbing out of the air: the feet may be this far below the lowest rung, m (the hands
/// catch it and pull up).
const AIR_GRAB_BELOW: f32 = 0.9;
/// Grabbing from the top: the feet within this of the top rung's height, m.
const TOP_GRAB: f32 = 0.35;
/// How fast the climbing speed changes, m/s².
const ACCEL: f32 = 14.0;
/// Sliding down: fastest speed, m/s, and how fast it gets there, m/s².
const SLIDE_SPEED: f32 = 7.0;
const SLIDE_ACCEL: f32 = 24.0;
/// Sprinting climbs this much faster.
const SPRINT: f32 = 1.5;
/// Fastest pull onto the line after a grab (or a bump), m/s.
const SNAP_SPEED: f32 = 5.0;
/// No climbing while the body is further than this from the line across it (still being
/// pulled onto it, from the floor at the top, say), m.
const BOARDING: f32 = 0.15;
/// Held this far off the line (something is in the way): let go, m.
const TORN_OFF: f32 = 1.0;
/// A hit pushing harder than this throws a climber off, m/s.
pub(crate) const KNOCKED_OFF: f32 = 5.0;
/// After letting go, no grabbing a ladder for this long, s.
const REGRAB_SECS: f32 = 0.35;
/// Kicking off: speed away from the ladder, m/s.
const KICK_SPEED: f32 = 4.0;

/// A climb line seen from a climber standing with `up` up.
#[derive(Debug, Clone, Copy)]
struct Frame {
    line: ClimbLine,
    /// Toward the ladder, horizontal unit.
    facing: Vec3,
    /// To the climber's right.
    right: Vec3,
    /// Lowest to top rung, m.
    len: f32,
}

impl Frame {
    fn new(line: ClimbLine, up: Vec3) -> Option<Self> {
        let facing = flat(line.facing, up).try_normalize()?;
        let len = (line.top - line.bottom).length() as f32;
        (len > 0.05).then_some(Self {
            line,
            facing,
            right: facing.cross(up),
            len,
        })
    }

    /// Where the feet are `climbed` meters up.
    fn feet(&self, climbed: f32) -> DVec3 {
        self.line.at((climbed / self.len) as f64)
    }
}

/// Climbs the ladder held, or grabs one in reach. Returns the velocity to move with when the
/// character climbed (or just got off) a ladder this step; `None` leaves the step to the rest
/// of the locomotion.
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
    m.ladder_cooldown = (m.ladder_cooldown - dt).max(0.0);
    if grounded {
        m.ladder_last = None;
    }
    if m.ladder.is_none() {
        if !p.ladder || m.ladder_cooldown > 0.0 || m.grapple.is_some_and(|g| g.attached) {
            return None;
        }
        grab(m, p, b, probe, v, wish, grounded, out)?;
    }
    Some(climb(m, p, b, probe, v, wish, held, grounded, dt, out))
}

/// The buttons that matter on a ladder this step.
#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct Held {
    pub jump_pressed: bool,
    pub crouch: bool,
    pub sprint: bool,
    /// Hit hard enough to be thrown off.
    pub knocked: bool,
}

/// Grabs the nearest ladder in reach. Returns `None` when there is none.
#[allow(clippy::too_many_arguments)]
fn grab(
    m: &mut Mover,
    p: &MovementProfile,
    b: &Body,
    probe: &Probe,
    v: Vec3,
    wish: Vec3,
    grounded: bool,
    out: &mut Outputs,
) -> Option<()> {
    let up = b.up;
    let feet = b.feet();
    let mut best: Option<(Entity, Frame, f32, f32)> = None;
    for (e, line) in probe.climb_lines() {
        if m.ladder_last == Some(e) {
            continue;
        }
        let Some(f) = Frame::new(line, up) else {
            continue;
        };
        let reach_side = line.half_width + GRAB_SIDE;
        // Along the climb (from below or out of the air), walking or flying into it.
        let t = line.along(feet) as f32;
        let off = (feet - line.at(t as f64)).as_vec3();
        let (depth, side, rise) = (off.dot(f.facing), off.dot(f.right).abs(), off.dot(up));
        let toward = wish.dot(f.facing) > 0.5 || (!grounded && v.dot(f.facing) > 1.0);
        let below = if grounded { GRAB_BELOW } else { AIR_GRAB_BELOW };
        let height = if t <= 0.0 {
            rise >= -below
        } else {
            t < 1.0 || rise <= 0.1
        };
        let on_line = toward
            && height
            && side <= reach_side
            && depth >= -(b.radius + GRAB_FRONT)
            && depth <= GRAB_BEHIND;
        // From the top: on the floor by the exit, walking back to the ladder.
        let top = (feet - line.top).as_vec3();
        let exit = (line.exit - line.top).as_vec3().dot(f.facing).max(0.6);
        let from_top = grounded
            && wish.dot(-f.facing) > 0.5
            && top.dot(f.right).abs() <= reach_side
            && (-0.1..=exit + 0.2).contains(&top.dot(f.facing))
            && top.dot(up).abs() <= TOP_GRAB;
        let (t, miss) = if on_line {
            (t, depth.abs() + side)
        } else if from_top {
            (1.0, top.dot(f.facing).abs() + top.dot(f.right).abs())
        } else {
            continue;
        };
        if best.is_none_or(|(_, _, _, bm)| miss < bm) {
            best = Some((e, f, t, miss));
        }
    }
    let (e, f, t, _) = best?;
    let climbed = t * f.len;
    // Out of the air, some of the jump's rise carries on up the ladder.
    let speed = if grounded {
        0.0
    } else {
        v.dot(up).clamp(-p.ladder_speed, p.ladder_speed)
    };
    m.ladder = Some(Ladder {
        entity: Some(e),
        t,
        climbed,
        speed,
        facing: f.facing,
        sliding: false,
        secs: 0.0,
        rung: (climbed / RUNG).round() as i32,
    });
    m.jump_buffer = 0.0;
    m.air_jumps_left = p.air_jumps;
    m.set_mode(LocomotionMode::Climb);
    out.trigger(b.entity, "ladder_grab");
    out.noise(b.entity, feet, 4.0);
    Some(())
}

/// One step on the ladder held: get off, or climb. Returns the velocity.
#[allow(clippy::too_many_arguments)]
fn climb(
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
    let Some(mut l) = m.ladder else {
        return v;
    };
    let Some((e, f)) = l
        .entity
        .and_then(|e| Some((e, Frame::new(probe.climb_line(e)?, up)?)))
    else {
        // The ladder is gone: fall.
        let_go(m, b, out, "ladder_exit", false);
        return v;
    };
    if held.knocked {
        let_go(m, b, out, "ladder_exit", true);
        return v;
    }
    if held.jump_pressed {
        // Kick off backward, steered sideways by the stick.
        let_go(m, b, out, "jump", true);
        out.noise(b.entity, b.pos, 6.0);
        let side = wish.dot(f.right).clamp(-1.0, 1.0);
        let launch = MovementProfile::launch_speed(p.wall_jump_height, b.g);
        return -f.facing * KICK_SPEED + f.right * (side * p.run_speed * 0.6) + up * launch;
    }

    // Forward climbs, back climbs down; crouch slides. Not before the body is on the line.
    let ask = wish.dot(f.facing).clamp(-1.0, 1.0);
    let ask = if ask.abs() < 0.2 { 0.0 } else { ask };
    let across = flat((f.feet(l.climbed) - b.feet()).as_vec3(), up).length();
    l.sliding = held.crouch && l.climbed > 0.0;
    let (target, accel) = if across > BOARDING {
        (0.0, ACCEL * 2.0)
    } else if l.sliding {
        (-SLIDE_SPEED, SLIDE_ACCEL)
    } else {
        let sprint = if held.sprint { SPRINT } else { 1.0 };
        (ask * p.ladder_speed * sprint, ACCEL)
    };
    l.speed += (target - l.speed).clamp(-accel * dt, accel * dt);
    let next = l.climbed + l.speed * dt;
    l.secs += dt;
    m.air_secs = 0.0;
    m.fall_speed = 0.0;

    if next >= f.len && l.speed > 0.0 {
        // Over the top: step off onto the exit.
        let exit = f.line.exit;
        let_go(m, b, out, "ladder_exit", false);
        m.mantle = Some(Mantle {
            top: exit,
            forward: f.facing,
            rising: true,
            exit_speed: p.run_speed * 0.5,
            on: probe.ladder_pin(e, exit, f.facing),
        });
        m.set_mode(LocomotionMode::Mantle);
        return up * p.mantle_speed;
    }
    if l.speed < 0.0 && (next <= 0.0 || (grounded && l.secs > 0.2)) {
        // Off the bottom: step back onto the ground (or drop the last bit to it).
        let_go(m, b, out, "ladder_exit", false);
        let stick = if grounded { -up * 0.5 } else { Vec3::ZERO };
        return -f.facing * 0.8 + stick;
    }
    l.climbed = next.clamp(0.0, f.len);
    l.t = l.climbed / f.len;
    l.facing = f.facing;

    // A hand and a foot settle on a rung.
    let rung = (l.climbed / RUNG).round() as i32;
    if rung != l.rung {
        l.rung = rung;
        if !l.sliding && p.stride_footsteps {
            m.left_foot = !m.left_foot;
            let side = if m.left_foot { -0.12 } else { 0.12 };
            let at =
                f.feet(rung as f32 * RUNG) + (f.facing * RUNG_AHEAD + f.right * side).as_dvec3();
            out.contact(GroundContact {
                position: at,
                normal: up,
                size: [0.06, 0.1],
                yaw: GroundContact::yaw_of(f.facing),
                load_kg: p.mass_kg * 0.5,
                speed: 0.0,
                kind: ContactKind::Foot,
                surface: probe.ladder_surface(e),
                entity: Some(b.entity),
            });
            out.noise(b.entity, at, 3.0);
        }
    }

    // Held on the line, the feet on the rung reached.
    let want = f.feet(l.climbed) + (up * (b.height * 0.5)).as_dvec3();
    let gap = (want - b.pos).as_vec3();
    if l.secs > 0.3 && gap.length() > TORN_OFF {
        let_go(m, b, out, "ladder_exit", true);
        return v;
    }
    m.ladder = Some(l);
    m.set_mode(LocomotionMode::Climb);
    (gap / dt.max(1e-4)).clamp_length_max(SNAP_SPEED + l.speed.abs())
}

/// Lets go of the ladder held. `kicked`: not grabbed again before the feet touch the ground.
pub(crate) fn let_go(m: &mut Mover, b: &Body, out: &mut Outputs, trigger: &str, kicked: bool) {
    let l = m.ladder.take();
    if kicked {
        m.ladder_last = l.and_then(|l| l.entity);
    }
    m.ladder_cooldown = REGRAB_SECS;
    m.set_mode(LocomotionMode::Air);
    out.trigger(b.entity, trigger);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_frame_measures_the_climb() {
        let line = ClimbLine {
            bottom: DVec3::new(2.0, 0.4, 5.0),
            top: DVec3::new(2.0, 5.5, 5.0),
            exit: DVec3::new(2.0, 5.5, 4.0),
            facing: Vec3::NEG_Z,
            half_width: 0.25,
        };
        let f = Frame::new(line, Vec3::Y).unwrap();
        assert!((f.len - 5.1).abs() < 1e-5);
        assert_eq!(f.facing, Vec3::NEG_Z);
        // Facing −Z, the right is +X.
        assert!(f.right.distance(Vec3::X) < 1e-6);
        assert!(f.feet(2.55).distance(DVec3::new(2.0, 2.95, 5.0)) < 1e-6);
        // A ladder facing straight up (or down) has no way to face.
        assert!(
            Frame::new(
                ClimbLine {
                    facing: Vec3::Y,
                    ..line
                },
                Vec3::Y
            )
            .is_none()
        );
    }
}
