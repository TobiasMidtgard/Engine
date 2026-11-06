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
use rapier3d::prelude::ColliderHandle;

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
    /// Rapier's own stages ([`STAGES`]), ms, all regions together.
    stages: [f64; STAGES.len()],
    regions: HashMap<RegionId, (f64, u32)>,
    /// Rapier's narrow phase by region, ms.
    narrow: HashMap<RegionId, f64>,
    /// Rapier's narrow phase in this step so far, ms (for [`step_log`]).
    step_narrow: f64,
    /// The step's own parts outside the regions' jobs, by name, ms ([`part_ms`]).
    outside: Vec<(&'static str, f64)>,
    /// Pieces of those parts, by name: their sum and their worst step, ms ([`piece_ms`]).
    pieces: Vec<PieceTotal>,
    /// Bodies that made a region's substep sweep for fast motion (CCD), and why: their count.
    sweepers: HashMap<Sweeper, usize>,
}

/// A body that set off a sweep, why, and what was in its path.
type Sweeper = (Entity, &'static str, Option<Entity>);

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

/// Rapier's stages, in order, as its counters time them.
/// "final broad phase" is the end-of-step tree update (and the broad phase's deferred tree
/// optimization, joined there), "positions" moving colliders to their bodies' new poses, and
/// "whole step" Rapier's own clock: past it, our "Rapier" holds only the hand-off to the
/// region's thread pool.
const STAGES: [&str; 15] = [
    "broad phase",
    "narrow phase",
    "islands",
    "solver set-up",
    "solving",
    "CCD",
    "user changes",
    "final broad phase",
    "positions",
    "whole step",
    "all collision detection",
    "all solver",
    "constraint collection",
    "CCD sweeps",
    "kinematic velocities",
];

/// One Rapier step's stage times, read from its counters.
pub(crate) fn rapier_stages(rid: RegionId, c: &rapier3d::counters::Counters) {
    let ms = [
        c.cd.broad_phase_time.time_ms(),
        c.cd.narrow_phase_time.time_ms(),
        c.stages.island_construction_time.time_ms(),
        c.solver.velocity_assembly_time.time_ms(),
        c.solver.velocity_resolution_time.time_ms(),
        c.stages.ccd_time.time_ms(),
        c.stages.user_changes.time_ms(),
        c.cd.final_broad_phase_time.time_ms(),
        c.stages.update_time.time_ms(),
        c.step_time.time_ms(),
        c.stages.collision_detection_time.time_ms(),
        c.stages.solver_time.time_ms(),
        c.stages.island_constraints_collection_time.time_ms(),
        c.ccd.toi_computation_time.time_ms(),
        c.custom.time_ms(),
    ];
    let mut t = totals().lock();
    for (i, v) in ms.iter().enumerate() {
        t.stages[i] += v;
    }
    *t.narrow.entry(rid).or_default() += ms[1];
    t.step_narrow += ms[1];
}

/// `body` made a region's substep run Rapier's sweeps (CCD), for `why` (and `what` it could
/// pass through).
pub(crate) fn sweeper(body: Entity, why: &'static str, what: Option<Entity>) {
    *totals()
        .lock()
        .sweepers
        .entry((body, why, what))
        .or_default() += 1;
}

/// `ms` of the step's own work went to `name` (sync, upkeep, write back...).
pub(crate) fn part_ms(name: &'static str, ms: f64) {
    let mut t = totals().lock();
    match t.outside.iter_mut().find(|c| c.0 == name) {
        Some(c) => c.1 += ms,
        None => t.outside.push((name, ms)),
    }
}

/// A piece's totals: its time in all, its worst run, and how much of that run its thread
/// spent on the CPU (a worst run far longer than its CPU time waited: for a lock, for other
/// threads, or for a core, as the VM's four cores often make it).
struct PieceTotal {
    name: &'static str,
    ms: f64,
    worst: f64,
    worst_cpu: Option<f64>,
}

/// A piece of one of the step's own parts took `ms` this step, `cpu_ms` of it on its thread's
/// CPU (reported per step, with the worst step: a part's spikes are usually one piece).
pub(crate) fn piece_ms(name: &'static str, ms: f64, cpu_ms: Option<f64>) {
    let mut t = totals().lock();
    match t.pieces.iter_mut().find(|c| c.name == name) {
        Some(c) => {
            c.ms += ms;
            if ms > c.worst {
                c.worst = ms;
                c.worst_cpu = cpu_ms;
            }
        }
        None => t.pieces.push(PieceTotal {
            name,
            ms,
            worst: ms,
            worst_cpu: cpu_ms,
        }),
    }
}

/// Times a piece of a part while it lives ([`piece_ms`]), when a report is wanted.
pub(crate) struct Piece(
    &'static str,
    Option<(Instant, Option<cpu_time::ThreadTime>)>,
);

impl Piece {
    pub(crate) fn start(name: &'static str) -> Self {
        Self(
            name,
            wanted().then(|| (Instant::now(), cpu_time::ThreadTime::try_now().ok())),
        )
    }
}

impl Drop for Piece {
    fn drop(&mut self) {
        if let Some((t, cpu)) = self.1 {
            let cpu = cpu.and_then(|c| c.try_elapsed().ok());
            piece_ms(
                self.0,
                t.elapsed().as_secs_f64() * 1000.0,
                cpu.map(|c| c.as_secs_f64() * 1000.0),
            );
        }
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
    let ms = started.elapsed().as_secs_f64() * 1000.0;
    t.steps += 1;
    t.step_ms += ms;
    let narrow = std::mem::take(&mut t.step_narrow);
    step_log(ms, narrow, sim, foci);
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
        "physics report: {:.2} ms per step over {} steps, {} regions, {} bodies waiting for \
         their ground, foci{}",
        t.step_ms / steps,
        t.steps,
        sim.live_regions().count(),
        sim.ungrounded.len(),
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
    // The step's span holds the time `physics_simulate` waited for its lock and for the
    // systems beside it, and the gap before `physics_write_back`: not physics's work.
    let own: f64 = t.outside.iter().map(|o| o.1).sum();
    let outside: Vec<String> = t
        .outside
        .iter()
        .map(|(name, ms)| format!("{name} {:.2}", ms / steps))
        .collect();
    out += &format!(
        "; the step's own parts ({}; waiting between them {:.2})",
        outside.join(", "),
        (t.step_ms - own).max(0.0) / steps
    );
    if !t.pieces.is_empty() {
        let pieces: Vec<String> = t
            .pieces
            .iter()
            .map(|p| {
                let cpu = p
                    .worst_cpu
                    .map(|c| format!(", {c:.2} on the CPU"))
                    .unwrap_or_default();
                format!("{} {:.2} (worst {:.2}{cpu})", p.name, p.ms / steps, p.worst)
            })
            .collect();
        out += &format!("; their pieces ({})", pieces.join(", "));
    }
    let stages: Vec<String> = STAGES
        .iter()
        .zip(t.stages)
        .map(|(name, ms)| format!("{name} {:.2}", ms / steps))
        .collect();
    out += &format!("; Rapier's stages ({})", stages.join(", "));
    for (name, sum, n) in &t.counts {
        out += &format!(
            ", {:.0} {name} per substep",
            *sum as f64 / (*n).max(1) as f64
        );
    }
    if !t.sweepers.is_empty() {
        let mut by: Vec<(&Sweeper, &usize)> = t.sweepers.iter().collect();
        by.sort_by(|a, b| b.1.cmp(a.1));
        let total: usize = by.iter().map(|(_, n)| **n).sum();
        let shown: Vec<String> = by
            .iter()
            .take(TOP)
            .map(|((e, why, what), n)| match what {
                Some(w) => format!("{} ({why}: {}) {n}", label(world, *e), label(world, *w)),
                None => format!("{} ({why}) {n}", label(world, *e)),
            })
            .collect();
        out += &format!(
            "; {total} region substeps swept for fast bodies, set off by {}",
            shown.join(", ")
        );
    }
    for (id, ms, n) in rows.into_iter().take(TOP) {
        let Some(Some(r)) = sim.regions.get(id as usize) else {
            continue;
        };
        let w = &r.world;
        let bodies = w.bodies.iter().filter(|(_, b)| !b.is_fixed()).count();
        let awake = w.islands.active_bodies().count();
        // Held bodies (resting stations', or waiting for their ground) something woke.
        let held = w
            .islands
            .active_bodies()
            .filter_map(|h| w.bodies.get(h))
            .map(|rb| crate::convert::entity_of(rb.user_data))
            .filter(|e| {
                sim.resting.contains(e)
                    || sim.ungrounded.contains(e)
                    || sim.held_for_shapes.contains_key(e)
            })
            .count();
        let pairs = w.narrow_phase.contact_pairs().count();
        let touching = w
            .narrow_phase
            .contact_pairs()
            .filter(|p| p.has_any_active_contact())
            .count();
        // (One per triangle of a mesh a body is near: a fitted jetty's 66,000 drawn triangles
        // made 800 for one moored rowboat, round 21.)
        let manifolds: usize = w
            .narrow_phase
            .contact_pairs()
            .map(|p| p.manifolds().len())
            .sum();
        let near = foci
            .iter()
            .map(|f| f.distance(r.anchor))
            .fold(f64::INFINITY, f64::min);
        out += &format!(
            "\n  region {id}: {:.2} ms per step ({:.2} per run, {n} runs; narrow phase {:.2}), tier \
             {}{}{}, {bodies} bodies ({awake} awake, {held} of them held), {} colliders, {pairs} \
             pairs ({touching} touching, {manifolds} manifolds), {} joints, anchor {near:.0} m \
             from focus",
            ms / steps,
            ms / n.max(1) as f64,
            t.narrow.get(&id).copied().unwrap_or(0.0) / steps,
            r.tier,
            if r.fine.is_some() { " fine" } else { "" },
            if r.dormant { ", dormant" } else { "" },
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
            out += &format!(" {} ({kind:?}, {reach:.1} m)", label(world, e));
        }
        // What its pairs are: the colliders in the most of them (a big floor or a mesh under
        // a heap is in one with everything on it).
        let mut per: HashMap<ColliderHandle, usize> = HashMap::new();
        for p in w.narrow_phase.contact_pairs() {
            *per.entry(p.collider1).or_default() += 1;
            *per.entry(p.collider2).or_default() += 1;
        }
        let mut most: Vec<(ColliderHandle, usize)> = per.into_iter().collect();
        most.sort_by_key(|m| std::cmp::Reverse(m.1));
        if !most.is_empty() {
            out += "; most pairs:";
            for (h, n) in most.into_iter().take(5) {
                let name = w.colliders.get(h).map_or_else(
                    || "?".into(),
                    |c| label(world, crate::convert::entity_of(c.user_data)),
                );
                out += &format!(" {name} ({n})");
            }
        }
        // What its narrow phase goes on: the pairs of the most manifolds, with their shapes
        // (a hull's points or a mesh's triangles set what each manifold costs).
        let mut heavy: Vec<_> = w
            .narrow_phase
            .contact_pairs()
            .map(|p| (p.manifolds().len(), p.collider1, p.collider2))
            .filter(|m| m.0 > 8)
            .collect();
        heavy.sort_by_key(|m| std::cmp::Reverse(m.0));
        if !heavy.is_empty() {
            let what = |h: ColliderHandle| {
                w.colliders.get(h).map_or_else(
                    || "?".into(),
                    |c| {
                        format!(
                            "{} [{}]",
                            label(world, crate::convert::entity_of(c.user_data)),
                            shape_size(c.shape())
                        )
                    },
                )
            };
            out += "; most manifolds:";
            for (n, a, b) in heavy.into_iter().take(4) {
                out += &format!(" {} on {} ({n})", what(a), what(b));
            }
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
                let name = label(world, e);
                let kind = if kinematic { ", kinematic" } else { "" };
                out += &format!(
                    " {name} ({speed:.1} m/s{kind}, at {:.0} {:.0} {:.0})",
                    at.x, at.y, at.z
                );
            }
            out += &why_awake(world, w);
        }
    }
    engine_core::log::info!("{out}");
    *t = Totals::default();
}

/// Why a region's awake bodies are awake: moving, resting but not for long enough yet, resting
/// long enough but kept up by their island (Rapier sleeps whole islands), never allowed to sleep,
/// or kinematic; with the names of the last three, which are the ones to look at.
fn why_awake(world: &World, w: &rapier3d::prelude::PhysicsWorld) -> String {
    let (mut moving, mut settling, mut kept, mut never, mut kin_moving, mut kin_still) =
        (0, 0, 0, 0, 0, 0);
    let mut names: Vec<(u8, Entity)> = Vec::new();
    for rb in w.islands.active_bodies().filter_map(|h| w.bodies.get(h)) {
        let e = crate::convert::entity_of(rb.user_data);
        let a = rb.activation();
        if rb.is_kinematic() {
            if rb.linvel().length() > 0.01 || rb.angvel().length() > 0.01 {
                kin_moving += 1;
            } else {
                kin_still += 1;
                names.push((2, e));
            }
        } else if a.normalized_linear_threshold < 0.0 || a.angular_threshold < 0.0 {
            never += 1;
            names.push((0, e));
        } else if rb.linvel().length() > a.normalized_linear_threshold
            || rb.angvel().length() > a.angular_threshold
        {
            moving += 1;
        } else if a.time_since_can_sleep < a.time_until_sleep {
            settling += 1;
        } else {
            kept += 1;
            names.push((1, e));
        }
    }
    let mut out = format!(
        "\n    why awake: {moving} moving, {settling} settling, {kept} at rest but kept up by their \
         island, {never} never sleep, kinematic {kin_moving} moving and {kin_still} still"
    );
    names.sort_by_key(|n| n.0);
    for (what, list) in [(0, "never sleep"), (1, "kept up"), (2, "still kinematic")] {
        let mine: Vec<String> = names
            .iter()
            .filter(|n| n.0 == what)
            .take(6)
            .map(|n| label(world, n.1))
            .collect();
        if !mine.is_empty() {
            out += &format!("; {list}: {}", mine.join(", "));
        }
    }
    out
}

/// What to call a body in the report: its name or its nearest named ancestor's, or else its
/// id and the components that say who made it (those outside the core, the world contracts
/// and physics), as `crate::Type`.
pub(crate) fn label(world: &World, e: Entity) -> String {
    let mut cur = e;
    loop {
        if let Some(name) = world.get::<Name>(cur) {
            return name.as_str().to_string();
        }
        match world.get::<ChildOf>(cur) {
            Some(parent) => cur = parent.parent(),
            None => break,
        }
    }
    let Ok(components) = world.inspect_entity(e) else {
        return format!("{e}");
    };
    let mine: Vec<String> = components
        .map(|(_, info)| info.name().to_string())
        .filter(|n| {
            ![
                "bevy",
                "engine_core",
                "engine_world",
                "engine_physics",
                "engine_render",
            ]
            .iter()
            .any(|core| n.starts_with(core))
        })
        .map(|n| {
            let path = n.split('<').next().unwrap_or(&n);
            let krate = path.split("::").next().unwrap_or(path);
            let ty = path.rsplit("::").next().unwrap_or(path);
            format!("{krate}::{ty}")
        })
        .take(2)
        .collect();
    if mine.is_empty() {
        format!("{e}")
    } else {
        format!("{e} {}", mine.join(" "))
    }
}

/// A shape's kind and size for the report: a hull's points, a mesh's triangles, a
/// heightfield's cells or a compound's parts.
pub(crate) fn shape_size(s: &dyn rapier3d::parry::shape::Shape) -> String {
    if let Some(h) = s.as_convex_polyhedron() {
        format!("hull of {} points", h.points().len())
    } else if let Some(m) = s.as_trimesh() {
        format!("mesh of {} triangles", m.num_triangles())
    } else if let Some(h) = s.as_heightfield() {
        format!("heightfield {}x{}", h.nrows(), h.ncols())
    } else if let Some(c) = s.as_compound() {
        format!("{} parts", c.shapes().len())
    } else {
        format!("{:?}", s.shape_type())
    }
}

/// The `n` contact pairs of `world` with the most manifolds, by entity and shape, with their
/// contacts and deepest penetration (for a slow step's log line, off the frame: no names).
pub(crate) fn heavy_pairs(
    world: &rapier3d::prelude::PhysicsWorld,
    anchor: DVec3,
    n: usize,
) -> String {
    let mut heavy: Vec<_> = world
        .narrow_phase
        .contact_pairs()
        .map(|p| {
            let contacts: usize = p.manifolds().iter().map(|m| m.points.len()).sum();
            let deepest = p
                .manifolds()
                .iter()
                .flat_map(|m| m.points.iter())
                .map(|c| -c.dist)
                .fold(0.0f32, f32::max);
            (
                p.manifolds().len(),
                contacts,
                deepest,
                p.collider1,
                p.collider2,
            )
        })
        .collect();
    heavy.sort_by_key(|m| std::cmp::Reverse(m.0));
    let what = |h: ColliderHandle| {
        world.colliders.get(h).map_or_else(
            || "?".into(),
            |c| {
                // Body type and world position, so a pair with no name can be found in a zone.
                let kind =
                    c.parent()
                        .and_then(|b| world.bodies.get(b))
                        .map_or("static", |b| match b.body_type() {
                            rapier3d::prelude::RigidBodyType::Dynamic => "dynamic",
                            rapier3d::prelude::RigidBodyType::Fixed => "fixed",
                            _ => "kinematic",
                        });
                let t = c.position().translation;
                let at = anchor + DVec3::new(f64::from(t.x), f64::from(t.y), f64::from(t.z));
                format!(
                    "{} [{}, {kind}, at {:.0} {:.0} {:.0}]",
                    crate::convert::entity_of(c.user_data),
                    shape_size(c.shape()),
                    at.x,
                    at.y,
                    at.z
                )
            },
        )
    };
    heavy
        .into_iter()
        .take(n)
        .map(|(m, k, d, a, b)| {
            format!(
                "{} on {} ({m} manifolds, {k} contacts, {d:.2} m deep)",
                what(a),
                what(b)
            )
        })
        .collect::<Vec<_>>()
        .join("; ")
}

/// With `PHYSICS_STEP_LOG=<file>` (and `PHYSICS_REPORT`), one line a step: seconds since the
/// first step, the step's ms, its Rapier narrow phase ms, awake bodies and the first focus, for
/// frame-time strips over a stop (round 23).
fn step_log(ms: f64, narrow: f64, sim: &Simulation, foci: &[DVec3]) {
    use std::io::Write;
    static LOG: OnceLock<Option<(Mutex<std::io::BufWriter<std::fs::File>>, Instant)>> =
        OnceLock::new();
    let log = LOG.get_or_init(|| {
        let path = std::env::var("PHYSICS_STEP_LOG")
            .ok()
            .filter(|p| !p.is_empty())?;
        let file = std::fs::File::create(path).ok()?;
        Some((Mutex::new(std::io::BufWriter::new(file)), Instant::now()))
    });
    let Some((file, t0)) = log else {
        return;
    };
    let awake: usize = sim
        .regions
        .iter()
        .flatten()
        .map(|r| r.world.islands.active_bodies().count())
        .sum();
    let f = foci.first().copied().unwrap_or_default();
    let mut file = file.lock();
    let _ = writeln!(
        file,
        "{:.3} {ms:.3} {narrow:.3} {awake} {:.0} {:.0} {:.0}",
        t0.elapsed().as_secs_f64(),
        f.x,
        f.y,
        f.z
    );
    let _ = file.flush();
}
