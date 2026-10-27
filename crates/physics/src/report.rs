//! Where the physics step's time goes, region by region: with `PHYSICS_REPORT=1` the step logs
//! every [`EVERY`] steps (or every N with `PHYSICS_REPORT=N`, for slow headless runs) the regions
//! that cost the most, with their bodies, level of detail, distance from the nearest focus and
//! what keeps them awake. For finding what keeps a scene slow; off by default.

use std::collections::HashMap;
use std::sync::OnceLock;
use std::time::Instant;

use engine_core::math::DVec3;
use engine_core::prelude::*;
use parking_lot::Mutex;

use crate::sim::{RegionId, Simulation};

/// Steps between reports, unless `PHYSICS_REPORT` names another number.
const EVERY: u32 = 120;
/// Regions listed per report.
const TOP: usize = 10;

#[derive(Default)]
struct Totals {
    steps: u32,
    step_ms: f64,
    /// A substep's parts ([`PARTS`]), ms.
    parts: [f64; PARTS.len()],
    /// Things counted per substep, by name: their sum and the number of substeps.
    counts: Vec<(&'static str, usize, usize)>,
    regions: HashMap<RegionId, (f64, u32)>,
}

fn totals() -> &'static Mutex<Totals> {
    static T: OnceLock<Mutex<Totals>> = OnceLock::new();
    T.get_or_init(Default::default)
}

/// `PHYSICS_REPORT` is set (to anything but `0`).
pub(crate) fn wanted() -> bool {
    static W: OnceLock<bool> = OnceLock::new();
    *W.get_or_init(|| std::env::var("PHYSICS_REPORT").is_ok_and(|v| !v.is_empty() && v != "0"))
}

/// Region `rid` stepped, taking `ms` (its jobs' time, whether or not they ran beside others).
pub(crate) fn region_ms(rid: RegionId, ms: f64) {
    let mut t = totals().lock();
    let e = t.regions.entry(rid).or_default();
    e.0 += ms;
    e.1 += 1;
}

/// What a substep's time goes to, in order.
const PARTS: [&str; 7] = [
    "forces and buoyancy",
    "kinematics and cones",
    "Rapier",
    "touches",
    "rolling and cones",
    "contact events",
    "breaking joints",
];

/// One substep's parts ([`PARTS`]).
pub(crate) fn substep_parts(parts: [std::time::Duration; PARTS.len()]) {
    let mut t = totals().lock();
    for (i, d) in parts.iter().enumerate() {
        t.parts[i] += d.as_secs_f64() * 1000.0;
    }
}

/// Count `n` of something this substep (reported as an average per substep).
pub(crate) fn count(name: &'static str, n: usize) {
    let mut t = totals().lock();
    match t.counts.iter_mut().find(|c| c.0 == name) {
        Some(c) => {
            c.1 += n;
            c.2 += 1;
        }
        None => t.counts.push((name, n, 1)),
    }
}

/// Steps between reports: `PHYSICS_REPORT=N` for N of 2 or more, else [`EVERY`].
fn every() -> u32 {
    static E: OnceLock<u32> = OnceLock::new();
    *E.get_or_init(|| {
        std::env::var("PHYSICS_REPORT")
            .ok()
            .and_then(|v| v.parse::<u32>().ok())
            .filter(|n| *n >= 2)
            .unwrap_or(EVERY)
    })
}

/// The whole step is done: count it, and log the report every [`EVERY`] steps.
pub(crate) fn step_done(world: &World, sim: &Simulation, foci: &[DVec3], started: Instant) {
    let mut t = totals().lock();
    t.steps += 1;
    t.step_ms += started.elapsed().as_secs_f64() * 1000.0;
    if t.steps < every() {
        return;
    }
    let steps = t.steps as f64;
    let mut rows: Vec<(RegionId, f64, u32)> = t
        .regions
        .iter()
        .map(|(id, (ms, n))| (*id, *ms, *n))
        .collect();
    rows.sort_by(|a, b| b.1.total_cmp(&a.1));
    let mut out = format!(
        "physics report: {:.2} ms per step over {} steps, {} regions, foci{}",
        t.step_ms / steps,
        t.steps,
        sim.live_regions().count(),
        foci.iter()
            .map(|f| format!(" ({:.0} {:.0} {:.0})", f.x, f.y, f.z))
            .collect::<String>()
    );
    let in_regions: f64 = rows.iter().map(|r| r.1).sum();
    let parts: Vec<String> = PARTS
        .iter()
        .zip(t.parts)
        .map(|(name, ms)| format!("{name} {:.2}", ms / steps))
        .collect();
    out += &format!(
        "; regions {:.2} ms per step ({}), the rest (sync, upkeep, write back) {:.2}",
        in_regions / steps,
        parts.join(", "),
        (t.step_ms - in_regions) / steps
    );
    for (name, sum, n) in &t.counts {
        out += &format!(
            ", {:.0} {name} per substep",
            *sum as f64 / (*n).max(1) as f64
        );
    }
    for (id, ms, n) in rows.into_iter().take(TOP) {
        let Some(Some(r)) = sim.regions.get(id as usize) else {
            continue;
        };
        let w = &r.world;
        let bodies = w.bodies.iter().filter(|(_, b)| !b.is_fixed()).count();
        let awake = w.islands.active_bodies().count();
        let pairs = w.narrow_phase.contact_pairs().count();
        let touching = w
            .narrow_phase
            .contact_pairs()
            .filter(|p| p.has_any_active_contact())
            .count();
        let near = foci
            .iter()
            .map(|f| f.distance(r.anchor))
            .fold(f64::INFINITY, f64::min);
        out += &format!(
            "\n  region {id}: {:.2} ms per step ({:.2} per run, {n} runs), tier {}{}, \
             {bodies} bodies ({awake} awake), {} colliders, {pairs} pairs ({touching} touching), \
             {} joints, anchor {near:.0} m from focus",
            ms / steps,
            ms / n.max(1) as f64,
            r.tier,
            if r.fine.is_some() { " fine" } else { "" },
            w.colliders.len(),
            w.impulse_joints.iter().count() + w.multibody_joints.iter().count(),
        );
        // What decides how it splits and merges: bodies asking for fine steps, characters,
        // and the farthest-reaching movers.
        let here: Vec<_> = sim.bodies.iter().filter(|(_, b)| b.region == id).collect();
        let fine = here.iter().filter(|(_, b)| b.fine.is_some()).count();
        let characters = sim.characters.values().filter(|c| c.region == id).count();
        let mut wide: Vec<_> = here.iter().map(|(e, b)| (**e, b.reach, b.kind)).collect();
        wide.sort_by(|a, b| b.1.total_cmp(&a.1));
        out += &format!(
            "\n    {fine} fine, mixed {}, {characters} characters; widest:",
            r.mixed
        );
        for (e, reach, kind) in wide.into_iter().take(4) {
            let name = world.get::<Name>(e).map_or("?", |n| n.as_str());
            out += &format!(" {name} ({kind:?}, {reach:.1} m)");
        }
        // What keeps it stepping: its fastest awake bodies.
        let mut moving: Vec<(f32, Entity, bool, DVec3)> = w
            .islands
            .active_bodies()
            .filter_map(|h| w.bodies.get(h))
            .map(|rb| {
                let e = crate::convert::entity_of(rb.user_data);
                let at = r.global(rb.translation());
                (rb.linvel().length(), e, rb.is_kinematic(), at)
            })
            .collect();
        if !moving.is_empty() {
            moving.sort_by(|a, b| b.0.total_cmp(&a.0));
            out += "; awake:";
            for (speed, e, kinematic, at) in moving.into_iter().take(4) {
                let name = world
                    .get::<Name>(e)
                    .or_else(|| {
                        let parent = world.get::<ChildOf>(e)?.parent();
                        world.get::<Name>(parent)
                    })
                    .map_or("?", |n| n.as_str());
                let kind = if kinematic { ", kinematic" } else { "" };
                out += &format!(
                    " {name} ({speed:.1} m/s{kind}, at {:.0} {:.0} {:.0})",
                    at.x, at.y, at.z
                );
            }
        }
    }
    engine_core::log::info!("{out}");
    *t = Totals::default();
}
