//! Held until the ground loads: a body falling where its ground has no collider yet stops and
//! sleeps where it is, until the ground's collider is there or a focus comes near.
//!
//! The terrain keeps collider tiles around the foci and under the bodies nearest to them, and
//! builds them over several frames, so out past the simulation range a body woken (by a
//! collapse, a blast, a migration) may have nothing under it and fall through the hill for
//! good. Open-world engines freeze physics outside the streamed cells for the same reason
//! (Unreal's World Partition keeps unloaded cells' bodies from simulating); here the check is
//! per body: one falling past the range whose ray down meets no fixed or kinematic collider,
//! though the world's `Ground` has a surface just below it, is held asleep
//! ([`Simulation::ungrounded`]). Every [`crate::lod::LOD_EVERY`] steps the held ones look
//! again, and go once something is under them or a focus is within the range (the terrain
//! builds the tiles under bodies there on the spot).
//!
//! Near a focus a body can still fall out of the world: spawned inside the hill, it drops out
//! of the bottom of the terrain's one-sided collider and falls for good (the fortress's debris
//! fell at 100 m/s, 600 m under the hill). Unreal stops such actors at its kill height
//! (`KillZ`); here one falling [`UNDER_M`] or more under the ground's surface with nothing
//! below it is held too, wherever it is, until something is under it or it is back above.

use engine_core::math::{DVec3, Vec3};
use engine_core::prelude::*;
use rapier3d::prelude::{QueryFilter, Ray};

use crate::sim::{Region, Simulation};

/// Falling faster than this (m/s, along gravity) is looked at.
const FALLING: f32 = 1.0;
/// How far the ray down reaches, meters.
const PROBE_M: f32 = 60.0;
/// Held only within this height above the ground's surface (higher up it is in flight), m.
const ABOVE_M: f64 = 20.0;
/// Or this far below it (deeper it fell through long ago, and rests as lost, `crate::lod`), m.
const BELOW_M: f64 = 50.0;
/// Falling this far under the ground's surface, with nothing below, a body has left the
/// world (no cave in the showcase runs this deep under the terrain), m.
const UNDER_M: f64 = 20.0;
/// The ground is asked for from this high above a body (the first surface below is returned).
const GROUND_PROBE_M: f64 = 2000.0;

/// Is there a fixed or kinematic collider within [`PROBE_M`] below `local`?
fn floor_below(r: &Region, local: Vec3) -> bool {
    let down = r.world.gravity.normalize_or(Vec3::NEG_Y);
    let ray = Ray::new(local, down);
    r.world
        .cast_ray(&ray, PROBE_M, true, QueryFilter::exclude_dynamic())
        .is_some()
}

/// Hold the bodies falling with no ground loaded under them, and let go of the held ones whose
/// ground came ([module docs](self)).
pub(crate) fn hold_ungrounded(sim: &mut Simulation, foci: &[DVec3]) {
    let (Some(range), Some(ground)) = (sim.range, sim.ground.clone()) else {
        return;
    };
    if foci.is_empty() {
        return;
    }
    let near = |p: DVec3| foci.iter().any(|f| f.distance(p) <= range.active_m);
    let look = sim.lod_clock.is_multiple_of(crate::lod::LOD_EVERY);
    // Let go.
    if !sim.ungrounded.is_empty() && look {
        let held: Vec<Entity> = sim.ungrounded.iter().copied().collect();
        for e in held {
            let Some((rid, handle)) = sim.bodies.get(&e).map(|b| (b.region, b.handle)) else {
                sim.ungrounded.remove(&e);
                continue;
            };
            let r = sim.region(rid);
            let Some(rb) = r.world.bodies.get(handle) else {
                sim.ungrounded.remove(&e);
                continue;
            };
            let local = rb.translation();
            let at = r.global(local);
            let under = || {
                ground
                    .height(at + DVec3::Y * GROUND_PROBE_M)
                    .is_some_and(|h| at.y < h - UNDER_M)
            };
            if floor_below(r, local) || (near(at) && !under()) {
                sim.ungrounded.remove(&e);
                if let Some(rb) = sim.region_mut(rid).world.bodies.get_mut(handle) {
                    rb.wake_up(true);
                }
            }
        }
    }
    // Hold.
    let mut hold = Vec::new();
    for (_, r) in sim.live_regions() {
        if r.frozen || r.tier >= crate::lod::REST {
            continue;
        }
        let down = r.world.gravity.normalize_or(Vec3::NEG_Y);
        for rb in r
            .world
            .islands
            .active_bodies()
            .filter_map(|h| r.world.bodies.get(h))
            .filter(|rb| rb.is_dynamic() && rb.linvel().dot(down) > FALLING)
        {
            let local = rb.translation();
            let at = r.global(local);
            // Near a focus only out-of-world falls are looked for, every few steps.
            let close = near(at);
            if close && !look {
                continue;
            }
            let Some(h) = ground.height(at + DVec3::Y * GROUND_PROBE_M) else {
                continue;
            };
            let out_of_world = at.y < h - UNDER_M;
            let unloaded = !close && at.y < h + ABOVE_M && at.y > h - BELOW_M;
            if (out_of_world || unloaded) && !floor_below(r, local) {
                hold.push(crate::convert::entity_of(rb.user_data));
            }
        }
    }
    sim.ungrounded.extend(hold);
}
