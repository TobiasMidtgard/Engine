//! Vaults: over a low, shallow obstacle (a railing, a concrete barrier, a crate, a car's hood)
//! in one movement, without slowing down. Taller or deeper obstacles are climbed onto (a
//! mantle) or hung from.
//!
//! * **What vaults.** An obstacle between [`KNEE`] and the profile's
//!   [`vault_max_height`](MovementProfile::vault_max_height) above the feet, whose top ends
//!   within [`vault_max_depth`](MovementProfile::vault_max_depth) without rising on the way,
//!   with room over it and ground to land on at least a little below its top beyond it. Its
//!   face may lean back (a precast barrier) and it may be a bare rail on posts.
//! * **When.** Running at it (within 45° of straight on) at [`MIN_SPEED`] or more, on a
//!   jump, sprinting into it, or running into it in the air; not out of a slide or a crouch.
//!   A sprinter takes off as late as the rise allows, so the vault costs almost no speed; a
//!   jump takes off a little earlier. Speeds count against the obstacle: one coming at the
//!   runner is vaulted sooner, and the runner keeps its own speed over the ground.
//! * **How.** The body rises just enough to clear the top as it reaches it, goes over it
//!   with the left hand planted on the top, skimming it, and drops down the far side running.
//!
//! The mode is [`LocomotionMode::Mantle`] (the contract has no vault of its own). The trigger
//! `vault` fires at the take-off, and a hand [`GroundContact`] (`ContactKind::Body`) goes out
//! on the top as the hand plants, on the top's surface.

use engine_core::math::{DVec3, Vec3};
use engine_core::prelude::*;
use engine_world::physics::Hit;
use engine_world::{ContactKind, GroundContact, LocomotionMode, SurfaceId};

use crate::locomotion::{Body, Outputs, Probe};
use crate::mover::{Mover, Vault, flat};
use crate::profile::MovementProfile;

/// Lowest obstacle vaulted, m above the feet (lower ones are stepped or jumped over).
pub const KNEE: f32 = 0.45;
/// In the air, an obstacle's top may be this little above the feet, m.
const AIR_LOW: f32 = 0.05;
/// Face rays are at most this far apart in height, m (a bare rail is about 5 cm thick, a
/// railing's top rail sits anywhere up to the profile's limit).
const FACE_STEP: f32 = 0.2;
/// Slowest run toward an obstacle that vaults it, m/s.
pub const MIN_SPEED: f32 = 3.5;
/// The run at most this far off straight at the obstacle's face (cosine).
const STRAIGHT_ON: f32 = 0.7;
/// The feet clear the top by this much going over, m.
pub const CLEAR: f32 = 0.08;
/// Rising speed a sprinter's take-off is timed for, m/s…
pub const LIFT: f32 = 7.5;
/// …and the range a take-off rises at (a jump a little early rises slower, one right at
/// the face faster), m/s.
const LIFT_RANGE: (f32, f32) = (3.5, 10.0);
/// A jump vaults an obstacle this much further away than a sprint takes off for, m.
const JUMP_EARLY: f32 = 0.6;
/// Obstacles coming at the runner are looked for this much further ahead (the closing speed
/// is only known once one is found).
const REACH_SLACK: f32 = 2.0;
/// Speed kept over the obstacle.
pub const KEEP: f32 = 0.95;
/// Pushing off the hand at the far side: speed down, m/s.
pub const DROP: f32 = 2.0;
/// Landing at most this far below the top, m (no vaulting off a roof's edge).
const MAX_DROP: f32 = 3.0;
/// The far side's ground at least this far below the top (else it's a step up onto
/// something: climb onto it instead), m.
const MIN_DROP: f32 = 0.3;
/// Depth sampling along the top, m.
const STEP: f32 = 0.15;
/// How far past the face the top is looked for, m: right behind it first (a thin panel),
/// then further in (a face that leans back puts its top edge behind the face's lower part).
const INSETS: [f32; 5] = [0.015, 0.04, 0.08, 0.15, 0.3];
/// The longest a vault takes before letting go, s.
const MAX_SECS: f32 = 1.0;
/// Where the left hand plants, left of the body's line, m.
pub const HAND_LEFT: f32 = 0.22;
/// Share of the body's weight on the planted hand.
const HAND_LOAD: f32 = 0.5;

/// What the step knows about the input this step.
pub(crate) struct Held {
    /// Jump pressed (or still buffered).
    pub jump: bool,
    pub sprint: bool,
}

/// An obstacle to vault.
struct Obstacle {
    /// The near edge of its top, in line with the body.
    edge: DVec3,
    /// Its top above the feet, m.
    height: f32,
    depth: f32,
    /// From the capsule's front to the face, m.
    gap: f32,
    surface: SurfaceId,
    entity: Entity,
}

/// Taking off for a vault.
pub(crate) enum TakeOff {
    /// Took off: the velocity to move with.
    Go(Vec3),
    /// An obstacle to vault ahead, not yet in reach of the take-off: don't climb onto it or
    /// catch its top meanwhile.
    Ahead,
    /// Nothing to vault.
    No,
}

/// Going over the obstacle being vaulted. Returns the velocity to move with, or `None` when
/// not vaulting (or knocked out of the vault).
pub(crate) fn over(
    m: &mut Mover,
    p: &MovementProfile,
    b: &Body,
    knocked: bool,
    dt: f32,
    out: &mut Outputs,
) -> Option<Vec3> {
    let mut vt = m.vault?;
    if knocked {
        m.vault = None;
        return None;
    }
    let up = b.up;
    let along = (b.pos - vt.plant).dot(vt.forward.as_dvec3()) as f32;
    let above = (b.feet() - vt.plant).dot(up.as_dvec3()) as f32;
    vt.secs += dt;
    if vt.rising && above >= CLEAR {
        vt.rising = false;
        plant(p, b, &vt, out);
    }
    if along >= vt.depth + b.radius * 0.9 || vt.secs > MAX_SECS {
        m.vault = None;
        m.set_mode(LocomotionMode::Air);
        return Some(vt.forward * vt.speed - up * DROP);
    }
    m.vault = Some(vt);
    m.set_mode(LocomotionMode::Mantle);
    let lift = if vt.rising {
        vt.lift
    } else {
        // Skim the top: hold the feet just over it.
        ((CLEAR + 0.03 - above) * 12.0).clamp(-1.5, 3.0)
    };
    Some(vt.forward * vt.speed + up * lift)
}

/// Taking off for an obstacle in reach.
#[allow(clippy::too_many_arguments)]
pub(crate) fn take_off(
    m: &mut Mover,
    p: &MovementProfile,
    b: &Body,
    probe: &Probe,
    v: Vec3,
    wish: Vec3,
    held: Held,
    grounded: bool,
    out: &mut Outputs,
) -> TakeOff {
    // Out of a slide or a crouch the capsule is short: what clears it wouldn't clear the
    // standing body it gets up into.
    if !p.vault || m.crouched || m.mode == LocomotionMode::Slide {
        return TakeOff::No;
    }
    let up = b.up;
    let vh = flat(v, up);
    let Some(dir) = wish
        .try_normalize()
        .filter(|_| wish.length() > 0.3)
        .or_else(|| vh.try_normalize())
    else {
        return TakeOff::No;
    };
    let speed = vh.dot(dir);
    if speed < MIN_SPEED {
        return TakeOff::No;
    }
    let jump = grounded && held.jump;
    if grounded && !jump && !held.sprint {
        return TakeOff::No;
    }
    let early = if jump { JUMP_EARLY } else { 0.0 };
    let reach = REACH_SLACK * speed * (p.vault_max_height + CLEAR) / LIFT + early + 0.1;
    let Some(o) = find(p, b, probe, dir, reach, grounded) else {
        return TakeOff::No;
    };
    // Against the obstacle: it moves (in this character's seconds) relative to the ground
    // under the runner.
    let deck = m.platform.map_or(Vec3::ZERO, |d| d.velocity);
    let moving = flat(
        probe.velocity_at(o.entity, o.edge).unwrap_or(Vec3::ZERO) - deck,
        up,
    );
    let closing = speed - moving.dot(dir);
    if closing < MIN_SPEED {
        return TakeOff::No;
    }
    // Take off once rising at `LIFT` (or faster, rising already) just clears the top at the
    // face.
    let rise = o.height + CLEAR;
    if o.gap > closing * rise / LIFT.max(v.dot(up)) + early + 0.1 {
        return TakeOff::Ahead;
    }
    let reaching = (o.gap / closing).max(1e-3);
    let lift = (rise / reaching)
        .clamp(LIFT_RANGE.0, LIFT_RANGE.1)
        .max(v.dot(up));
    if jump {
        m.jump_buffer = 0.0;
        m.coyote = 0.0;
    }
    let from = (b.pos - o.edge).dot(dir.as_dvec3()) as f32;
    // Over it the body moves with the obstacle (it is held, like a ledge): the speed over it
    // is relative to it.
    let vt = Vault {
        plant: o.edge,
        forward: dir,
        depth: o.depth,
        from,
        speed: closing * KEEP,
        lift,
        rising: true,
        secs: 0.0,
        surface: Some(o.surface),
        on: probe.pin(o.entity, o.edge, dir),
    };
    m.vault = Some(vt);
    m.set_mode(LocomotionMode::Mantle);
    out.trigger(b.entity, "vault");
    out.noise(b.entity, b.pos, 5.0);
    // This step still moves relative to the ground under the runner.
    TakeOff::Go(dir * vt.speed + moving + up * lift)
}

/// The left hand plants on the top.
fn plant(p: &MovementProfile, b: &Body, vt: &Vault, out: &mut Outputs) {
    let left = b.up.cross(vt.forward).normalize_or_zero();
    out.contact(GroundContact {
        position: vt.plant + (left * HAND_LEFT).as_dvec3(),
        normal: b.up,
        size: [0.1, 0.18],
        yaw: GroundContact::yaw_of(vt.forward),
        load_kg: p.mass_kg * HAND_LOAD,
        speed: vt.speed,
        kind: ContactKind::Body,
        surface: vt.surface,
        entity: Some(b.entity),
    });
}

/// An obstacle to vault ahead along `dir` (horizontal unit), its face within `reach` of the
/// capsule's front. In the air its top may be just above the feet.
fn find(
    p: &MovementProfile,
    b: &Body,
    probe: &Probe,
    dir: Vec3,
    reach: f32,
    grounded: bool,
) -> Option<Obstacle> {
    let up = b.up;
    let upd = up.as_dvec3();
    let feet = b.feet();
    let max = p.vault_max_height;
    // Its face: the nearest hit from the knees (the toes, in the air) up to the highest top
    // vaulted.
    let low = if grounded { KNEE } else { AIR_LOW };
    let rays = ((max - low) / FACE_STEP).ceil().max(1.0) as usize;
    let face = (0..=rays)
        .map(|i| low + (max - low) * i as f32 / rays as f32)
        .filter_map(|h| probe.ray(feet + upd * h as f64, dir, b.radius + reach))
        .filter(|h| h.normal.dot(up).abs() < 0.35 && h.normal.dot(dir) < -STRAIGHT_ON)
        .min_by(|a, c| a.distance.total_cmp(&c.distance))?;
    // Its top, past the face: straight behind it, or further in where the face leans back.
    let high = max + 0.2;
    let height_of = |h: &Hit| (h.point - feet).dot(upd) as f32;
    let top_at = |inset: f32| {
        let near = face.point + (dir * inset).as_dvec3();
        let from = near + upd * ((feet - near).dot(upd) + high as f64);
        probe.ray(from, -up, high - low + 0.15)
    };
    let mut found = None;
    for (i, &inset) in INSETS.iter().enumerate() {
        let Some(hit) = top_at(inset) else {
            continue;
        };
        // Starting inside it: taller than anything vaulted.
        if hit.distance < 0.01 {
            return None;
        }
        // The face itself, leaning back: look further in.
        if hit.normal.dot(up) < 0.7 {
            continue;
        }
        // Past the back of a thin panel, down to the ground: nothing to plant a hand on.
        let h = height_of(&hit);
        if h < low - 0.1 || h > max {
            return None;
        }
        found = Some((i, hit, h));
        break;
    }
    let (i, top, height) = found?;
    // Where the top's near edge is: the face's hit, or between the last inset that came down
    // on the face and the first that found the top.
    let edge_in = if i == 0 {
        0.0
    } else {
        let (mut lo, mut hi) = (INSETS[i - 1], INSETS[i]);
        for _ in 0..4 {
            let mid = (lo + hi) * 0.5;
            let on_top = top_at(mid)
                .is_some_and(|h| h.normal.dot(up) >= 0.7 && (height_of(&h) - height).abs() < 0.05);
            if on_top {
                hi = mid;
            } else {
                lo = mid;
            }
        }
        hi
    };
    let near = face.point + (dir * edge_in).as_dvec3();
    let edge = near + upd * (top.point - near).dot(upd);
    // Its depth: where the top ends (it must, within reach of the hand plant), without rising
    // on the way (a top that steps up is climbed onto instead).
    let top_above = |s: f32| {
        let at = edge + (dir * s).as_dvec3() + upd * 0.3;
        probe
            .ray(at, -up, 0.55)
            .filter(|h| h.normal.dot(up) > 0.5 && h.distance > 0.01)
            .map(|h| 0.3 - h.distance)
    };
    let mut rise: f32 = 0.0;
    let mut depth = None;
    for s in (1..)
        .map(|k| 0.03 + STEP * k as f32)
        .take_while(|&s| s <= p.vault_max_depth + STEP)
    {
        match top_above(s) {
            Some(r) => rise = rise.max(r),
            None => {
                depth = Some(s - STEP * 0.5);
                break;
            }
        }
    }
    let depth = depth.filter(|&d| d <= p.vault_max_depth)?;
    if rise > CLEAR {
        return None;
    }
    // Room to go over, the hips skimming the top and the head clear.
    let behind = edge - (dir * b.radius).as_dvec3();
    let across = depth + b.radius * 3.0;
    for h in [CLEAR + 0.3, CLEAR + b.height - 0.2] {
        if probe.ray(behind + upd * h as f64, dir, across).is_some() {
            return None;
        }
    }
    if probe
        .ray(b.pos, up, b.height * 0.5 + height + CLEAR)
        .is_some()
    {
        return None;
    }
    // Ground to land on beyond it, below its top.
    let beyond = edge + (dir * (depth + b.radius + 0.3)).as_dvec3() + upd * 0.3;
    let land = probe.ray(beyond, -up, 0.3 + MAX_DROP)?;
    let drop = (edge - land.point).dot(upd) as f32;
    if land.normal.dot(up) < 0.64 || drop < MIN_DROP {
        return None;
    }
    Some(Obstacle {
        edge,
        height,
        depth,
        gap: (face.distance - b.radius).max(0.0) + edge_in,
        surface: top.surface,
        entity: top.entity,
    })
}
