//! Footwork between blows: a fighter keeps its distance from its foe (its measure), facing it,
//! and steps in guard to hold it: in when the foe is too far, back when too close, round it
//! when told to circle. In For Honor and the Souls games the space between blows is hovering
//! at the edge of reach and stepping in or out of it; in fencing it is the advance and the
//! retreat, and after an exchange the fighter withdraws to measure (the Abzug).
//!
//! [`Footwork`] rests while the fighter's own move plays (the move's warp does the closing
//! then), while it evades, is knocked back, down or onto a knee, and takes over again after:
//! a fighter shoved back by a bash, or carried in by its own warp, steps back to its measure.
//! While the foe's move plays it stands its ground: a blow is blocked or taken where it
//! stands (stepping off one is the evasion's job).
//! It moves its own `Transform` and writes the `speed` and `heading` the locomotion reads
//! (with a weapon stance up, the legs walk under the stance's upper body), or, with
//! [`Footwork::move_transform`] off, publishes [`Footwork::velocity`] for the module that owns
//! the movement. The `circle` parameter (-1 round to the right, 1 to the left) strafes it
//! round the foe.

use engine_core::prelude::*;
use engine_world::animation::AnimationParams;

use crate::combat::{Attacker, Evader};
use crate::hit::HitReaction;
use crate::hurt::Hurt;
use crate::knockdown::Knockdown;
use crate::paired::Riposted;

/// Keeps a fighter at its measure from its foe, facing it, stepping in guard.
#[derive(Component, Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Footwork {
    pub enabled: bool,
    /// Who it squares up to; its `Attacker`'s target when unset.
    pub foe: Option<Entity>,
    /// The distance it keeps, flat, from its feet to the foe's, m.
    pub measure: f32,
    /// How far off the measure it may stand before it steps, m.
    pub slack: f32,
    /// Stepping speeds: in, back and round, m/s.
    pub advance_speed: f32,
    pub retreat_speed: f32,
    pub circle_speed: f32,
    /// How fast it gets up to speed and stops, m/s².
    pub accel: f32,
    /// How fast it turns to face the foe, radians per second.
    pub turn_rate: f32,
    /// Move its own `Transform`. Off for characters whose movement module owns it: then
    /// [`Footwork::velocity`] says where it would step.
    pub move_transform: bool,
    /// How fast it is stepping (world, m/s; published).
    #[serde(skip)]
    pub velocity: Vec3,
    /// Whether it is holding the measure now (not resting for a move, a knock or a fall).
    #[serde(skip)]
    pub active: bool,
}

impl Default for Footwork {
    fn default() -> Self {
        Self {
            enabled: true,
            foe: None,
            measure: 1.4,
            slack: 0.08,
            // Guarded steps well under walking pace; fencing advances are about 0.46 m.
            advance_speed: 0.8,
            retreat_speed: 0.7,
            circle_speed: crate::build::humanoid::SIDESTEP_SPEED,
            accel: 3.0,
            turn_rate: 5.0,
            move_transform: true,
            velocity: Vec3::ZERO,
            active: false,
        }
    }
}

/// Distance over which a step to the measure eases into a stop, m.
const EASE: f32 = 0.4;
/// Close enough to the measure to stop stepping, m.
const SETTLED: f32 = 0.02;

/// Yaw (radians, about +Y) that faces the rig (looking down −Z) along `dir`.
fn yaw_of(dir: Vec3) -> f32 {
    (-dir.x).atan2(-dir.z)
}

/// Step fighters to their measure and round their foes (`AnimSet::Setup`, after the attacks).
#[allow(clippy::type_complexity)]
pub(crate) fn step_footwork(
    time: Res<Time>,
    mut q: Query<(
        Entity,
        &mut Footwork,
        &mut Transform,
        Option<&mut AnimationParams>,
        Option<&Attacker>,
        Option<&Evader>,
        Option<&HitReaction>,
        Option<&Hurt>,
        Option<&Knockdown>,
        Option<&Riposted>,
        Option<&LocalTimeScale>,
    )>,
    foes: Query<(&GlobalTransform, Option<&Attacker>)>,
) {
    for (entity, mut fw, mut t, params, attacker, evader, reaction, hurt, down, riposted, local) in
        &mut q
    {
        let dt = time.delta_for(local);
        let foe = fw.foe.or(attacker.and_then(|a| a.target));
        let busy = attacker.is_some_and(|a| a.current.is_some())
            || evader.is_some_and(|e| e.evading.is_some())
            || reaction.is_some_and(|r| r.stagger.0.length() > 0.02)
            || hurt.is_some_and(Hurt::is_down)
            || down.is_some()
            || riposted.is_some();
        let foe = foe.filter(|f| *f != entity).and_then(|f| foes.get(f).ok());
        // A blow coming in is met where it stands (blocked, deflected or taken), not walked
        // away from: stepping off it is the evasion's job.
        let foe_attacking = foe.is_some_and(|(_, a)| a.is_some_and(|a| a.current.is_some()));
        let foe_at = foe.map(|(g, _)| g.translation().as_vec3());
        let Some(foe_at) = foe_at.filter(|_| fw.enabled && !busy && dt > 0.0) else {
            if fw.active {
                // Hand back to whatever plays: a move's warp, the knock-back, the fall.
                fw.velocity = Vec3::ZERO;
                fw.active = false;
                if fw.move_transform
                    && let Some(mut p) = params
                {
                    p.set("speed", 0.0);
                }
            }
            continue;
        };
        fw.active = true;
        let here = t.translation.as_vec3();
        let to = (foe_at - here).with_y(0.0);
        let dist = to.length();
        if dist < 1e-3 {
            continue;
        }
        let forward = to / dist;
        let left = Vec3::new(forward.z, 0.0, -forward.x);
        // In or back to the measure once it is more than the slack off (easing in over the
        // last 0.4 m and stopping on it), round when told.
        let off = dist - fw.measure;
        let closing = fw.velocity.dot(forward).abs() > 0.05 && off.abs() > SETTLED;
        let radial = if foe_attacking || (off.abs() <= fw.slack && !closing) {
            0.0
        } else {
            (off / EASE).clamp(-fw.retreat_speed, fw.advance_speed)
        };
        let circle = if foe_attacking {
            0.0
        } else {
            params
                .as_ref()
                .map_or(0.0, |p| p.get("circle"))
                .clamp(-1.0, 1.0)
        };
        let want = forward * radial + left * circle * fw.circle_speed;
        let change = want - fw.velocity;
        let most = fw.accel * dt;
        fw.velocity += if change.length() > most {
            change.normalize() * most
        } else {
            change
        };
        if !fw.move_transform {
            continue;
        }
        let v = fw.velocity;
        t.translation += (v * dt).as_dvec3();
        // Face the foe, turning no faster than the turn rate.
        let face = yaw_of(forward);
        let now = yaw_of(t.rotation * Vec3::NEG_Z);
        let turn = (face - now + PI).rem_euclid(2.0 * PI) - PI;
        let step = turn.clamp(-fw.turn_rate * dt, fw.turn_rate * dt);
        t.rotation = Quat::from_rotation_y(now + step);
        if let Some(mut p) = params {
            let speed = v.length();
            let facing = t.rotation * Vec3::NEG_Z;
            let side = Vec3::new(facing.z, 0.0, -facing.x);
            p.set("speed", speed);
            if speed > 0.02 {
                p.set("heading", v.dot(side).atan2(v.dot(facing)));
            }
            p.set("turn", 0.0);
            p.set("accel", 0.0);
        }
    }
}
