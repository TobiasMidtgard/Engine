//! Engine exhausts for the effects modules: [`Exhausts`] on every vehicle with an engine,
//! one nozzle per thruster, tailpipe or exhaust stub, in a fixed order, with how hard each
//! fires this frame.
//!
//! The drawn models put their nozzles where the model has them (a bell's exit, a tailpipe's
//! tip); a spacecraft built from data alone gets one nozzle at each thruster, sized for its
//! thrust. Levels: a thruster's own throttle; a combustion engine's throttle and load (more
//! at high rpm, a little at idle); a jet's spool, lit as an afterburner on boost.

use engine_core::math::Vec3;
use engine_core::prelude::*;
use engine_world::{EngineKind, EngineState, Exhaust, ExhaustKind, Exhausts};

use crate::chassis::Vehicle;
use crate::ship::{Spacecraft, Thruster, ThrusterRole};

/// What a thruster's exhaust looks like: main and hover engines are rockets, RCS jets puff.
pub fn thruster_kind(role: ThrusterRole) -> ExhaustKind {
    match role {
        ThrusterRole::Main | ThrusterRole::Hover => ExhaustKind::Rocket,
        ThrusterRole::Rcs => ExhaustKind::Thruster,
    }
}

/// Nozzles for thrusters with no drawn model: at the thrust point, blowing opposite the push,
/// with an exit radius that grows with thrust (0.77 m at 600 kN, 17 cm at 30 kN).
pub fn thruster_exhausts(thrusters: &[Thruster]) -> Vec<Exhaust> {
    thrusters
        .iter()
        .map(|t| {
            let radius = (0.001 * t.max_thrust_n.max(0.0).sqrt()).clamp(0.03, 5.0);
            Exhaust::new(thruster_kind(t.role), t.position, -t.direction, radius).at_level(t.level)
        })
        .collect()
}

/// How hard an engine's exhausts fire, 0..1.
pub fn engine_level(e: &EngineState) -> f32 {
    if !e.running {
        return 0.0;
    }
    match e.kind {
        EngineKind::Electric => 0.0,
        EngineKind::Piston { .. } => {
            let span = (e.max_rpm - e.idle_rpm).max(1.0);
            let rpm = ((e.rpm - e.idle_rpm) / span).clamp(0.0, 1.0);
            let work = 0.5 * e.throttle.clamp(0.0, 1.0) + 0.5 * e.load.clamp(0.0, 1.0);
            (0.06 + 0.94 * work * (0.4 + 0.6 * rpm)).min(1.0)
        }
        EngineKind::Turbine | EngineKind::Rocket | EngineKind::Thrusters => e.load.clamp(0.0, 1.0),
    }
}

/// A vehicle's thrusters (a spacecraft), engine and exhausts so far.
type ExhaustSources<'a> = (
    Entity,
    Option<&'a Spacecraft>,
    Option<&'a EngineState>,
    Option<&'a mut Exhausts>,
);

/// Writes every vehicle's [`Exhausts`] for this frame.
pub fn exhausts_system(mut commands: Commands, mut q: Query<ExhaustSources, With<Vehicle>>) {
    for (entity, ship, engine, exhausts) in &mut q {
        match (ship, exhausts) {
            (Some(ship), Some(mut ex)) if ex.0.len() == ship.thrusters.len() => {
                for (nozzle, t) in ex.0.iter_mut().zip(&ship.thrusters) {
                    nozzle.level = t.level.clamp(0.0, 1.0);
                }
            }
            // No nozzles yet, or the thrusters changed: one at each thruster.
            (Some(ship), _) => {
                commands
                    .entity(entity)
                    .insert(Exhausts(thruster_exhausts(&ship.thrusters)));
            }
            (None, Some(mut ex)) => {
                let level = engine.map_or(0.0, engine_level);
                let afterburner = engine.is_some_and(|e| e.boost > 0.5);
                for nozzle in &mut ex.0 {
                    nozzle.level = level;
                    if matches!(nozzle.kind, ExhaustKind::Jet | ExhaustKind::Afterburner) {
                        nozzle.kind = if afterburner {
                            ExhaustKind::Afterburner
                        } else {
                            ExhaustKind::Jet
                        };
                    }
                }
            }
            (None, None) => {}
        }
    }
}

/// Nozzles at the given exits, all of one kind, idle.
pub fn nozzles(kind: ExhaustKind, exits: &[(Vec3, Vec3, f32)]) -> Exhausts {
    Exhausts(
        exits
            .iter()
            .map(|&(at, dir, radius)| Exhaust::new(kind, at, dir, radius))
            .collect(),
    )
}
