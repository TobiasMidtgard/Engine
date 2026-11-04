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
//! That is checked soon after it drops through: the fortress's debris fell through the hill at
//! 19-24 m/s, and until 20 m down every piece was swept against the terrain it was inside of,
//! every step (round 22 on the PC, up to 4 ms a step in the loops' second round). A crater or
//! a cellar has a floor under it, so nothing standing in one is held.

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
/// world (fell through the terrain's one-sided collider), m.
const UNDER_M: f64 = 3.0;
/// The ground is asked for from this high above a body (the first surface below is returned).
const GROUND_PROBE_M: f64 = 2000.0;
/// Steps between two looks at a body falling past the simulation range (each on steps of its
/// own): falling 25 m/s, it drops 1.7 m in between, well inside [`ABOVE_M`].
const FAR_EVERY: u32 = 4;

/// Is there a fixed or kinematic collider within [`PROBE_M`] below `local`?
/// The world `Ground`'s height under `p` (the first surface below it). A slow answer is
/// logged when a report is wanted: most take microseconds, but one now and then took 5-9 ms
/// of a step (round 13 on the VM, the CPU busy throughout).
pub(crate) fn height(ground: &engine_world::Ground, p: DVec3) -> Option<f64> {
    if !crate::report::wanted() {
        return ground.height(p);
    }
    let started = std::time::Instant::now();
    let h = ground.height(p);
    let ms = started.elapsed().as_secs_f64() * 1000.0;
    if ms > 1.0 {
        engine_core::log::info!(
            "physics: the ground took {ms:.1} ms for its height at ({:.0} {:.0} {:.0})",
            p.x,
            p.y,
            p.z
        );
    }
    h
}

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
    // Each body is looked at on steps of its own, not all on the same step: a collapse's
    // hundreds of falling pieces asked the ground for its height all at once, 9 ms of one step
    // (round 13 on the VM; the ground's height walks roads, streams and cliffs).
    let clock = sim.lod_clock;
    let turn = |e: Entity, every: u32| (e.index_u32().wrapping_add(clock)).is_multiple_of(every);
    let mut asked = 0usize;
    // Let go.
    if !sim.ungrounded.is_empty() {
        let held: Vec<Entity> = sim
            .ungrounded
            .iter()
            .copied()
            .filter(|e| turn(*e, crate::lod::LOD_EVERY))
            .collect();
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
            let mut under = || {
                asked += 1;
                height(&ground, at + DVec3::Y * GROUND_PROBE_M).is_some_and(|h| at.y < h - UNDER_M)
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
            // Near a focus only out-of-world falls are looked for, every few steps; past it,
            // falls onto ground not loaded yet too, every [`FAR_EVERY`] steps.
            let close = near(at);
            let e = crate::convert::entity_of(rb.user_data);
            if !turn(
                e,
                if close {
                    crate::lod::LOD_EVERY
                } else {
                    FAR_EVERY
                },
            ) {
                continue;
            }
            asked += 1;
            let Some(h) = height(&ground, at + DVec3::Y * GROUND_PROBE_M) else {
                continue;
            };
            let out_of_world = at.y < h - UNDER_M;
            let unloaded = !close && at.y < h + ABOVE_M && at.y > h - BELOW_M;
            if (out_of_world || unloaded) && !floor_below(r, local) {
                hold.push(e);
            }
        }
    }
    sim.ungrounded.extend(hold);
    if asked > 0 && crate::report::wanted() {
        crate::report::count("ground heights asked for holds", asked);
    }
}
