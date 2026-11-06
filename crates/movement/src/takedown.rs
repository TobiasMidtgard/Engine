//! Takedowns from above: crouched on a perch, a branch or a beam (or hanging, or in the air)
//! with an enemy below, attack drops onto it (Assassin's Creed's air assassination, the
//! hunter in the tree).
//!
//! - **Who**: the nearest enemy [`Fighter`] (another team, still standing) [`DROP_MIN`] to
//!   [`DROP_MAX`] below and within [`REACH`] across, in front of the attacker (or almost
//!   straight below).
//! - **The leap** is an arc onto the target's shoulders that follows it if it moves.
//! - **The hit** lands with a heavy impact (shake, hitstop), sends the target a
//!   [`DamageEvent`] that takes it down, and pins it a moment before the attacker rises.
//!
//! [`begin_takedowns`] picks targets from `MoveIntent::attack` (before the combat system starts
//! a normal swing) and delivers the damage; [`step`] flies the body (in the locomotion step).

use engine_core::math::{DVec3, Vec3};
use engine_core::prelude::*;
use engine_world::locomotion::{LocomotionMode, MoveIntent};
use engine_world::{DamageEvent, DamageKind};

use crate::combat::{CombatState, Fighter};
use crate::impact::ImpactKind;
use crate::locomotion::{Body, Outputs};
use crate::mover::{Mover, flat};
use crate::profile::MovementProfile;
use crate::traverse::follow;

/// Across to the target, at most, m.
pub const REACH: f32 = 5.5;
/// How far below the target must be (its center under the attacker's), m.
pub const DROP_MIN: f32 = 1.5;
pub const DROP_MAX: f32 = 10.0;
/// Pinned on the target after the hit, s.
pub const PIN_SECS: f32 = 0.75;
/// The hit lands with this much (kills an ordinary enemy).
pub const DAMAGE: f32 = 500.0;

/// A takedown under way.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Takedown {
    pub target: Entity,
    /// The target's center (kept up to date: the leap follows it).
    pub to: DVec3,
    pub secs: f32,
    /// The leap's planned flight time, s.
    pub total: f32,
    /// Landed on it: seconds pinning it.
    pub pinned: Option<f32>,
    /// The hit landed and the damage is still to be sent.
    pub(crate) strike: bool,
}

impl Takedown {
    /// 0..1 through the whole move (the leap, then the pin), for the look.
    pub fn t(&self) -> f32 {
        match self.pinned {
            None => 0.5 * (self.secs / self.total.max(0.05)).min(1.0),
            Some(p) => 0.5 + 0.5 * (p / PIN_SECS).min(1.0),
        }
    }
}

/// Starts takedowns asked for with attack from above, keeps each leap on its target and
/// sends the hit's damage (`FixedUpdate`, before combat starts attacks).
#[allow(clippy::type_complexity)]
pub(crate) fn begin_takedowns(
    mut attackers: Query<(
        Entity,
        &Transform,
        &mut Mover,
        &mut MoveIntent,
        &Fighter,
        &MovementProfile,
    )>,
    targets: Query<(Entity, &Transform, &Fighter, &CombatState), Without<Mover>>,
    others: Query<(Entity, &Transform, &Fighter, &CombatState), With<Mover>>,
    mut damage: MessageWriter<DamageEvent>,
    mut snaps: Local<Vec<(Entity, DVec3, u32)>>,
) {
    snaps.clear();
    for (e, t, f, cs) in targets.iter().chain(others.iter()) {
        if !cs.defeated && f.health > 0.0 {
            snaps.push((e, t.translation, f.team));
        }
    }
    for (me, t, mut m, mut intent, f, _p) in &mut attackers {
        let up = m.up;
        if let Some(mut td) = m.takedown {
            if let Some(&(_, pos, _)) = snaps.iter().find(|s| s.0 == td.target) {
                td.to = pos;
            }
            if td.strike {
                td.strike = false;
                damage.write(DamageEvent {
                    target: td.target,
                    amount: DAMAGE,
                    kind: DamageKind::Blunt,
                    point: td.to + (up * 0.6).as_dvec3(),
                    impulse: -up * 400.0 + flat(m.facing, up) * 150.0,
                    source: Some(me),
                });
            }
            m.takedown = Some(td);
            // (A takedown under way swallows attack presses.)
            intent.attack = None;
            continue;
        }
        if intent.attack.is_none() {
            continue;
        }
        let high = m.perch.is_some()
            || m.line.is_some()
            || m.pole.is_some()
            || m.bar.is_some()
            || m.hang.is_some()
            || !m.grounded;
        if !high {
            continue;
        }
        let from = t.translation;
        let look = intent
            .look
            .map(|l| flat(l, up))
            .and_then(|l| l.try_normalize())
            .unwrap_or(m.facing);
        let best = snaps
            .iter()
            .filter(|s| s.0 != me && s.2 != f.team)
            .filter_map(|&(e, pos, _)| {
                let d = (pos - from).as_vec3();
                let below = -d.dot(up);
                let across = flat(d, up);
                let dist = across.length();
                if !(DROP_MIN..=DROP_MAX).contains(&below) || dist > REACH {
                    return None;
                }
                let ahead = across.normalize_or_zero().dot(look);
                if dist > 1.5 && ahead < 0.2 {
                    return None;
                }
                Some((dist - ahead * 2.0, e, pos))
            })
            .min_by(|a, b| a.0.total_cmp(&b.0));
        let Some((_, target, pos)) = best else {
            continue;
        };
        intent.attack = None;
        m.leave_elements();
        m.takedown = Some(Takedown {
            target,
            to: pos,
            secs: 0.0,
            total: 0.0,
            pinned: None,
            strike: false,
        });
    }
}

/// Where the attacker lands: on the target's shoulders, a little behind (from where it came).
fn landing(td: &Takedown, b: &Body) -> DVec3 {
    let back = flat((b.pos - td.to).as_vec3(), b.up).normalize_or_zero();
    td.to + (b.up * 0.9 + back * 0.35).as_dvec3()
}

/// One step of the takedown (in the locomotion step): the leap, the hit, the pin.
pub(crate) fn step(
    m: &mut Mover,
    p: &MovementProfile,
    b: &Body,
    v: Vec3,
    dt: f32,
    out: &mut Outputs,
) -> Option<Vec3> {
    let mut td = m.takedown?;
    let up = b.up;
    td.secs += dt;
    if let Some(pin) = td.pinned {
        let pin = pin + dt;
        if pin >= PIN_SECS {
            m.takedown = None;
            m.set_mode(LocomotionMode::Idle);
            return Some(Vec3::ZERO);
        }
        td.pinned = Some(pin);
        m.takedown = Some(td);
        m.set_mode(LocomotionMode::Crouch);
        // Riding the target down to the ground.
        let ground = td.to - (up * 0.2).as_dvec3();
        return Some(follow(b.pos, ground, Vec3::ZERO, 4.0, dt));
    }
    let to = landing(&td, b);
    if td.total == 0.0 {
        // The leap: an arc onto it under the body's own gravity.
        let g_up = b.g * p.rise_gravity;
        let g_down = b.g * p.fall_gravity;
        let d = (to - b.pos).as_vec3();
        let arc = crate::target::solve(
            b.pos,
            to,
            up,
            g_up,
            g_down,
            crate::target::extra_for(flat(d, up).length(), d.dot(up)),
        );
        td.total = arc.total;
        td.secs = 0.0;
        m.takedown = Some(td);
        m.set_mode(LocomotionMode::Air);
        out.trigger(b.entity, "takedown_leap");
        out.impact(b.entity, ImpactKind::Launch, b.feet(), up, arc.v0.length());
        return Some(arc.v0);
    }
    let gap = (to - b.pos).as_vec3();
    let left = td.total - td.secs;
    if gap.length() < 0.35 || left <= dt * 0.5 {
        // The hit.
        td.pinned = Some(0.0);
        td.strike = true;
        m.takedown = Some(td);
        m.fall_speed = 0.0;
        m.air_secs = 0.0;
        out.trigger(b.entity, "takedown");
        out.impact(
            b.entity,
            ImpactKind::Takedown,
            td.to,
            up,
            v.length().max(8.0),
        );
        out.noise(b.entity, td.to, 10.0);
        return Some(Vec3::ZERO);
    }
    m.takedown = Some(td);
    m.set_mode(LocomotionMode::Air);
    // Steer onto the (moving) target over the time left, as a targeted leap does.
    let gh = flat(gap, up);
    let vh = flat(v, up);
    let vh = vh + (gh / left.max(dt) - vh).clamp_length_max(12.0 * dt);
    let vy = v.dot(up) - b.g * p.fall_gravity * dt;
    let need = gap.dot(up) / left.max(dt);
    let vy = if left < 0.12 { need } else { vy };
    Some(vh + up * vy)
}
