//! The creek nearest the camera, simulated: the near tier of the creeks (creeks-research.md,
//! "T0"). Up close a creek has to part round its stones, pile up in front of them and trail a
//! wake and an eddy line behind, which a ribbon with an analytic current can't draw. So the
//! stretch of the nearest creek within [`SIM_REACH_M`] runs on a flow grid of its own
//! ([`FlowRegion`], the mountain streams' simulation), with the reach's stones standing in it
//! (laid and drawn by [`crate::creek_stones`]), and the ribbons ([`crate::creeks`]) leave that stretch out
//! once the simulation is running.
//!
//! The stretch is placed round the camera's nearest point on the creek, longer for a wider
//! creek and within a cell budget, built and settled on a worker, and swapped in when ready:
//! the ribbon draws the water until then, and the old stretch stays until the new one runs. It
//! is dropped when the camera is [`DROP_M`] from it.

use std::sync::Arc;

use bevy_tasks::Task;
use bevy_tasks::futures::check_ready;
use engine_core::math::{DVec2, DVec3};
use engine_core::prelude::*;
use engine_core::tasks::AsyncComputeTaskPool;
use engine_world::terrain::{CreekPoint, CreekQuery, CreekReach, Creeks};
use engine_world::{Ground, WorldShift};

use crate::creek_stones::{CreekStones, ReachStones, lay_reach_stones, reach_seed};
use crate::creeks::{
    CreekRibbons, Cut, LocalCreeks, Provider, gather_providers, main_camera, provider_key,
};
use crate::demo::valley::{StreamStone, ValleyStream, stream_region_cells};
use crate::flow::{FlowRegion, FlowSource};
use crate::state::SharedWater;

/// A creek this close to the camera is simulated, meters.
pub const SIM_REACH_M: f32 = 120.0;
/// A simulated stretch this far from the camera is dropped, meters.
pub const DROP_M: f32 = 180.0;
/// Most cells a stretch's grid may have (a far stream's step cost 0.3-0.4 ms at about this).
const MOST_CELLS: f64 = 30_000.0;
/// How often the camera is checked, seconds.
const PERIOD_S: f32 = 0.5;
/// The camera may wander this share of a stretch's length from its middle before a new one
/// is placed round it.
const RECENTER_SHARE: f32 = 0.3;

/// A stretch being simulated.
struct Stretch {
    /// The meters of each reach it takes in (the ribbons leave them out).
    cuts: Vec<Cut>,
    /// Its provider ([`provider_key`]).
    provider: usize,
    /// The reaches of the creek it runs down, each with the meters along that creek where it
    /// starts ([`Chain`]), and the stretch's from, to and the creek's length along it.
    chain: Vec<(u64, f32)>,
    span: (f32, f32, f32),
    /// Its course (world x, z), for how far the camera is from it.
    line: Vec<DVec2>,
    /// The flow region.
    entities: Vec<Entity>,
    region: Entity,
    /// Running: the ribbons leave it out.
    live: bool,
}

/// A stretch planned on a worker: its region's corner and settings (its stones' rocks in
/// it; [`crate::creek_stones`] draws them).
pub struct StretchPlan {
    corner: DVec3,
    region: FlowRegion,
}

/// A stretch being planned: what it will be once its region is in.
struct Job {
    cuts: Vec<Cut>,
    provider: usize,
    chain: Vec<(u64, f32)>,
    span: (f32, f32, f32),
    line: Vec<DVec2>,
}

/// The stretches of creek simulated near the camera (see the module docs).
#[derive(Resource, Default)]
pub struct CreekSims {
    /// Simulate no creek (draw them all as ribbons): for before pictures and comparisons.
    pub off: bool,
    /// Keep each stretch to its own reach, short of junctions, without side creeks (as before
    /// round 52): for comparisons.
    pub one_reach: bool,
    stretches: Vec<Stretch>,
    job: Option<(Job, Task<Option<StretchPlan>>)>,
    since: f32,
}

impl CreekSims {
    /// Stretches running.
    pub fn live(&self) -> usize {
        self.stretches.iter().filter(|s| s.live).count()
    }

    /// Stretches placed or being planned.
    pub fn pending(&self) -> bool {
        self.job.is_some() || self.stretches.iter().any(|s| !s.live)
    }
}

/// Meters along a polyline (x, z) to its nearest point to `p`, and how far `p` is from it.
fn along_line(points: &[CreekPoint], p: DVec2) -> (f32, f32) {
    let mut best = (0.0f32, f32::MAX);
    let mut s = 0.0f32;
    for w in points.windows(2) {
        let a = DVec2::new(w[0].at.x, w[0].at.z);
        let b = DVec2::new(w[1].at.x, w[1].at.z);
        let ab = b - a;
        let len = ab.length();
        let t = ((p - a).dot(ab) / (len * len).max(1e-9)).clamp(0.0, 1.0);
        let d = (a + ab * t).distance(p) as f32;
        if d < best.1 {
            best = (s + (len * t) as f32, d);
        }
        s += len as f32;
    }
    best
}

/// The points of `points` from `from` to `to` meters along it, ends interpolated.
fn sub_course(points: &[CreekPoint], from: f32, to: f32) -> Vec<CreekPoint> {
    let mut out = Vec::new();
    let mut s = 0.0f32;
    let lerp = |a: &CreekPoint, b: &CreekPoint, t: f32| CreekPoint {
        at: a.at.lerp(b.at, t as f64),
        width_m: a.width_m + (b.width_m - a.width_m) * t,
        depth_m: a.depth_m + (b.depth_m - a.depth_m) * t,
        flow_m3_s: a.flow_m3_s + (b.flow_m3_s - a.flow_m3_s) * t,
    };
    for w in points.windows(2) {
        let len = DVec2::new(w[1].at.x - w[0].at.x, w[1].at.z - w[0].at.z).length() as f32;
        let (s0, s1) = (s, s + len);
        if s1 >= from && s0 <= to && len > 0.0 {
            if out.is_empty() {
                out.push(lerp(&w[0], &w[1], ((from - s0) / len).clamp(0.0, 1.0)));
            }
            if s1 <= to {
                out.push(w[1]);
            } else {
                out.push(lerp(&w[0], &w[1], ((to - s0) / len).clamp(0.0, 1.0)));
                break;
            }
        }
        s = s1;
    }
    out
}

/// Cells a grid over `points`' box (with the stream's margin) would have at `cell` meters.
fn cells_over(points: &[CreekPoint], cell: f64) -> f64 {
    let margin = points.first().map_or(0.0, |p| p.width_m as f64 + 6.0);
    let (mut lo, mut hi) = (DVec2::splat(f64::MAX), DVec2::splat(f64::MIN));
    for p in points {
        let q = DVec2::new(p.at.x, p.at.z);
        lo = lo.min(q);
        hi = hi.max(q);
    }
    let size = hi - lo + DVec2::splat(2.0 * margin);
    size.x * size.y / (cell * cell)
}

/// The grid's cells for a creek `width` meters wide: four or more across it, 0.3 to 0.75 m.
pub fn cell_for(width: f32) -> f32 {
    (0.25 * width).clamp(0.3, 0.75)
}

/// A stretch stops this far short of its reach's mouth, meters: there the creek runs into
/// another, whose channel crosses the grid, and the water leaving the stretch would spill
/// over the ground beside it (the ribbons draw the last meters).
const MOUTH_M: f32 = 14.0;

/// A reach's length in plan, meters.
fn length_of(points: &[CreekPoint]) -> f32 {
    points.windows(2).fold(0.0f32, |s, w| {
        s + DVec2::new(w[1].at.x - w[0].at.x, w[1].at.z - w[0].at.z).length() as f32
    })
}

/// Where a stretch may end on a reach `total` meters long ([`MOUTH_M`] short of its mouth).
fn stretch_end(total: f32) -> f32 {
    (total - MOUTH_M).max(0.0)
}

/// The stretch of `reach` (world points) to simulate round `at` meters along it: (from, to)
/// meters along, about 40% upstream and 60% down, as long as its width calls for and its grid
/// allows, and short of its mouth.
pub fn stretch_of(points: &[CreekPoint], at: f32) -> Option<(f32, f32)> {
    let end = stretch_end(length_of(points));
    if end < 20.0 {
        return None;
    }
    let width = points.iter().map(|p| p.width_m).sum::<f32>() / points.len().max(1) as f32;
    let cell = cell_for(width) as f64;
    let mut length = (50.0 * width).clamp(60.0, 140.0).min(end);
    for _ in 0..8 {
        let from = (at - 0.4 * length).clamp(0.0, (end - length).max(0.0));
        let to = (from + length).min(end);
        let course = sub_course(points, from, to);
        if course.len() >= 2 && cells_over(&course, cell) <= MOST_CELLS {
            return Some((from, to));
        }
        length *= 0.8;
    }
    None
}

/// One reach's end meets the next one's start within this, meters: the same creek going on.
const JOINS_M: f64 = 1.0;
/// A side creek whose mouth is within this of a stretch's banks joins it there, meters.
const MOUTH_REACH_M: f64 = 12.0;
/// The last meters of a side creek simulated with the creek it runs into.
const TRIBUTARY_M: f32 = 30.0;
/// Most reaches a [`Chain`] takes in either way from the camera's.
const CHAIN_LINKS: usize = 3;
/// A [`Chain`] runs on at least this far either way from the camera's reach, where the creek
/// does, meters.
const CHAIN_M: f32 = 150.0;

/// A creek's reaches end to end through one of them: where one stops at a junction the
/// creek runs on in the next, so a stretch can run across the junction rather than stop
/// short of it. Upstream it follows the biggest of the reaches that meet there.
pub struct Chain {
    pub points: Vec<CreekPoint>,
    /// Each reach in it, upstream first, and the meters along the chain where it starts.
    pub parts: Vec<(CreekReach, f32)>,
}

fn flat(p: &CreekPoint) -> DVec2 {
    DVec2::new(p.at.x, p.at.z)
}

/// The reaches of `query` that `pick` keeps near `p`, biggest by `flow` first.
fn joining(
    query: &dyn CreekQuery,
    p: DVec2,
    pick: impl Fn(&CreekReach) -> bool,
    flow: impl Fn(&CreekReach) -> f32,
) -> Option<CreekReach> {
    let mut found = Vec::new();
    query.reaches(p - DVec2::splat(2.0), p + DVec2::splat(2.0), &mut found);
    found
        .into_iter()
        .filter(|r| r.points.len() >= 2 && pick(r))
        .max_by(|a, b| flow(a).total_cmp(&flow(b)))
}

/// The [`Chain`] through `reach`.
pub fn chain_of(query: &dyn CreekQuery, reach: &CreekReach) -> Chain {
    let mut ups: Vec<CreekReach> = Vec::new();
    let mut downs: Vec<CreekReach> = Vec::new();
    let taken = |id: u64, ups: &[CreekReach], downs: &[CreekReach]| {
        id == reach.id || ups.iter().chain(downs).any(|r| r.id == id)
    };
    let mut length = 0.0;
    while ups.len() < CHAIN_LINKS && length < CHAIN_M {
        let head = ups.last().unwrap_or(reach);
        let start = flat(&head.points[0]);
        let Some(prev) = joining(
            query,
            start,
            |r| {
                !taken(r.id, &ups, &downs)
                    && flat(r.points.last().expect("points")).distance(start) < JOINS_M
            },
            |r| r.points.last().map_or(0.0, |p| p.flow_m3_s),
        ) else {
            break;
        };
        length += length_of(&prev.points);
        ups.push(prev);
    }
    let mut length = 0.0;
    while downs.len() < CHAIN_LINKS && length < CHAIN_M {
        let tail = downs.last().unwrap_or(reach);
        let end = flat(tail.points.last().expect("points"));
        let Some(next) = joining(
            query,
            end,
            |r| !taken(r.id, &ups, &downs) && flat(&r.points[0]).distance(end) < JOINS_M,
            |r| r.points[0].flow_m3_s,
        ) else {
            break;
        };
        length += length_of(&next.points);
        downs.push(next);
    }
    let mut points: Vec<CreekPoint> = Vec::new();
    let mut parts = Vec::new();
    for r in ups.into_iter().rev().chain([reach.clone()]).chain(downs) {
        let start = length_of(&points);
        let skip = usize::from(!points.is_empty());
        points.extend(r.points.iter().skip(skip).copied());
        parts.push((r, start));
    }
    Chain { points, parts }
}

/// The creek `reach` runs into at its mouth, and the meters down that one where it does:
/// where a side creek doesn't go on in a reach of its own but ends by (or in) another.
pub fn runs_into(query: &dyn CreekQuery, reach: &CreekReach) -> Option<(CreekReach, f32)> {
    let mouth = flat(reach.points.last()?);
    let mut found = Vec::new();
    let r = DVec2::splat(MOUTH_REACH_M + 10.0);
    query.reaches(mouth - r, mouth + r, &mut found);
    if found
        .iter()
        .any(|o| o.id != reach.id && flat(&o.points[0]).distance(mouth) < JOINS_M)
    {
        // It goes on.
        return None;
    }
    found
        .into_iter()
        .filter(|o| o.id != reach.id && o.points.len() >= 2)
        .filter_map(|o| {
            let (s, d) = along_line(&o.points, mouth);
            let half = 0.5 * o.points[0].width_m as f64;
            // Not one that ends here too (another side creek), nor beside its banks.
            (s < length_of(&o.points) - JOINS_M as f32 && (d as f64) < half + MOUTH_REACH_M)
                .then_some((o, s, d))
        })
        .min_by(|a, b| a.2.total_cmp(&b.2))
        .map(|(o, s, _)| (o, s))
}

/// Where to simulate round a camera `at` meters down `reach` (provider-local): the stretch
/// down its [`Chain`], and the side creeks running into it there.
pub struct Placed {
    /// The stretch's course, down the chain.
    pub course: Vec<CreekPoint>,
    /// The last meters of each side creek joining it.
    pub tributaries: Vec<Vec<CreekPoint>>,
    /// Every reach it takes in, with the meters of it (from, to).
    pub cuts: Vec<(CreekReach, f32, f32)>,
    /// The chain's reaches and where along it each starts.
    pub chain: Vec<(u64, f32)>,
    /// From, to and the chain's length, meters along the chain.
    pub span: (f32, f32, f32),
}

impl Placed {
    /// How many of [`Placed::cuts`] are down the chain (the side creeks' come after).
    pub fn chain_cuts(&self) -> usize {
        self.cuts.len() - self.tributaries.len()
    }
}

/// Place a stretch round `at` meters down `reach` (see [`Placed`]).
///
/// `joined` false keeps it to `reach`, without side creeks.
pub fn place_stretch(
    query: &dyn CreekQuery,
    reach: &CreekReach,
    at: f32,
    joined: bool,
) -> Option<Placed> {
    let chain = if joined {
        chain_of(query, reach)
    } else {
        Chain {
            points: reach.points.clone(),
            parts: vec![(reach.clone(), 0.0)],
        }
    };
    let start = chain.parts.iter().find(|p| p.0.id == reach.id)?.1;
    let total = length_of(&chain.points);
    let (from, to) = stretch_of(&chain.points, start + at)?;
    let course = sub_course(&chain.points, from, to);
    let mut cuts: Vec<(CreekReach, f32, f32)> = chain
        .parts
        .iter()
        .filter_map(|(r, begin)| {
            let len = length_of(&r.points);
            let (a, b) = ((from - begin).max(0.0), (to - begin).min(len));
            (b > a + 0.01).then(|| (r.clone(), a, b))
        })
        .collect();
    // Side creeks running into it inside the stretch: their last meters run on its grid.
    let width = course.iter().map(|p| p.width_m).sum::<f32>() / course.len().max(1) as f32;
    let (mut lo, mut hi) = (DVec2::splat(f64::MAX), DVec2::splat(f64::MIN));
    for p in &course {
        lo = lo.min(flat(p));
        hi = hi.max(flat(p));
    }
    let mut found = Vec::new();
    let reach_m = 0.5 * width as f64 + MOUTH_REACH_M;
    query.reaches(
        lo - DVec2::splat(reach_m),
        hi + DVec2::splat(reach_m),
        &mut found,
    );
    let mut tributaries = Vec::new();
    let length = to - from;
    for r in found {
        if !joined || r.points.len() < 2 || chain.parts.iter().any(|p| p.0.id == r.id) {
            continue;
        }
        let mouth = flat(r.points.last().expect("points"));
        let (s, d) = along_line(&course, mouth);
        if d as f64 > reach_m || s < 5.0 || s > length - 5.0 {
            continue;
        }
        let len = length_of(&r.points);
        let a = (len - TRIBUTARY_M).max(0.0);
        let line = sub_course(&r.points, a, len);
        if line.len() < 2 {
            continue;
        }
        tributaries.push(line);
        cuts.push((r, a, len));
    }
    // Within the grid's budget, or without them.
    let all: Vec<CreekPoint> = course
        .iter()
        .chain(tributaries.iter().flatten())
        .copied()
        .collect();
    if !tributaries.is_empty() && cells_over(&all, cell_for(width) as f64) > MOST_CELLS * 1.15 {
        cuts.truncate(cuts.len() - tributaries.len());
        tributaries.clear();
    }
    Some(Placed {
        course,
        tributaries,
        cuts,
        chain: chain.parts.iter().map(|(r, s)| (r.id, *s)).collect(),
        span: (from, to, total),
    })
}

/// Plan a stretch's grid (world-free: runs on a worker), with `stones` (the reach's, laid by
/// [`crate::creek_stones`]) standing in it. `ground` is the height of the ground at x, z.
///
/// `tributaries` are side creeks' last meters running into it (see [`place_stretch`]): the
/// grid takes them in, with a spring at the head of each bringing its flow.
pub fn plan_stretch(
    course: &[CreekPoint],
    tributaries: &[Vec<CreekPoint>],
    ground: &dyn Fn(f64, f64) -> f64,
    stones: &[StreamStone],
) -> Option<StretchPlan> {
    if course.len() < 2 {
        return None;
    }
    let path: Vec<[f64; 2]> = course.iter().map(|p| [p.at.x, p.at.z]).collect();
    let width = course.iter().map(|p| p.width_m).sum::<f32>() / course.len() as f32;
    let stream = ValleyStream {
        path: &path,
        width_m: width,
        flow_m3_s: course[0].flow_m3_s,
    };
    let base = course
        .iter()
        .chain(tributaries.iter().flatten())
        .map(|p| p.at.y)
        .fold(f64::MAX, f64::min)
        - 6.0;
    let cell = cell_for(width);
    let (mut corner, mut region) = stream_region_cells(&stream, base, ground, cell)?;
    // Grown to take in the side creeks, their banks too.
    if !tributaries.is_empty() {
        let margin = width as f64 + 6.0;
        let size = DVec2::new(
            region.cells[0] as f64 * cell as f64,
            region.cells[1] as f64 * cell as f64,
        );
        let (mut lo, mut hi) = (
            DVec2::new(corner.x, corner.z),
            DVec2::new(corner.x, corner.z) + size,
        );
        for p in tributaries.iter().flatten() {
            let q = DVec2::new(p.at.x, p.at.z);
            lo = lo.min(q - DVec2::splat(margin));
            hi = hi.max(q + DVec2::splat(margin));
        }
        let shift = (DVec2::new(corner.x, corner.z) - lo).as_vec2().to_array();
        for s in region.sources.iter_mut().chain(&mut region.priming) {
            s.at = [s.at[0] + shift[0], s.at[1] + shift[1]];
        }
        let cells = ((hi - lo) / cell as f64).ceil();
        region.cells = [cells.x as u32, cells.y as u32];
        corner = DVec3::new(lo.x, corner.y, lo.y);
    }
    // The springs on the creek's own line: the stream's search for the lowest ground near its
    // course can land beside a creek running along a slope, below its bed, and flood the
    // grass there. And no hollows filled to the brim (the brim of a pool can be that open
    // ground too): the priming springs fill the pools as the creek would.
    let on_line = |at: [f32; 2]| -> [f32; 2] {
        let p = DVec2::new(corner.x + at[0] as f64, corner.z + at[1] as f64);
        let mut best = (f64::MAX, p);
        for w in course.windows(2) {
            let a = DVec2::new(w[0].at.x, w[0].at.z);
            let b = DVec2::new(w[1].at.x, w[1].at.z);
            let ab = b - a;
            let t = ((p - a).dot(ab) / ab.length_squared().max(1e-9)).clamp(0.0, 1.0);
            let q = a + ab * t;
            if q.distance(p) < best.0 {
                best = (q.distance(p), q);
            }
        }
        [(best.1.x - corner.x) as f32, (best.1.y - corner.z) as f32]
    };
    if let Some(first) = region.sources.first_mut() {
        first.at = [
            (course[0].at.x - corner.x) as f32,
            (course[0].at.z - corner.z) as f32,
        ];
    }
    for p in &mut region.priming {
        p.at = on_line(p.at);
    }
    // Each side creek's spring at the head of its last meters, and one priming it halfway.
    for t in tributaries {
        let local = |p: &CreekPoint| [(p.at.x - corner.x) as f32, (p.at.z - corner.z) as f32];
        let radius = (t[0].width_m * 0.4).max(cell);
        region.sources.push(FlowSource {
            at: local(&t[0]),
            radius_m: radius,
            rate_m3_s: t[0].flow_m3_s,
        });
        region.priming.push(FlowSource {
            at: local(&t[t.len() / 2]),
            radius_m: radius,
            rate_m3_s: t[0].flow_m3_s,
        });
    }
    region.fills.clear();
    let size = DVec2::new(
        region.cells[0] as f64 * region.cell_m as f64,
        region.cells[1] as f64 * region.cell_m as f64,
    );
    // The stones in the channel. Not the boulders along the banks: half up the bank, mostly
    // out of the water, a film the grid spread over their tops poured off their edges as
    // falls; the water runs round them under their mesh just as well.
    // Off the line of the creek nearest, as a share of that creek's width.
    let off_line = |p: DVec2| {
        std::iter::once(course)
            .chain(tributaries.iter().map(|t| &t[..]))
            .flat_map(|line| line.windows(2))
            .map(|w| {
                let a = DVec2::new(w[0].at.x, w[0].at.z);
                let ab = DVec2::new(w[1].at.x, w[1].at.z) - a;
                let t = ((p - a).dot(ab) / ab.length_squared().max(1e-9)).clamp(0.0, 1.0);
                (a + ab * t).distance(p) / w[0].width_m.max(0.1) as f64
            })
            .fold(f64::MAX, f64::min)
    };
    region.rocks = stones
        .iter()
        .filter(|s| {
            let d = DVec2::new(s.at.x - corner.x, s.at.z - corner.z);
            d.x >= 0.0 && d.y >= 0.0 && d.x <= size.x && d.y <= size.y
        })
        .filter(|s| off_line(DVec2::new(s.at.x, s.at.z)) < 0.42)
        .map(|s| s.rock(corner))
        .collect();
    Some(StretchPlan { corner, region })
}

/// The stones of `reach` from `from` to `to` meters down it (and a couple of meters beyond),
/// as laid for the whole reach.
fn stones_between(stones: &ReachStones, from: f32, to: f32) -> Vec<StreamStone> {
    stones.stones[stones.between(from - 2.0, to + 2.0)].to_vec()
}

/// Keeps the creek nearest the camera simulated (see the module docs).
#[allow(clippy::too_many_arguments, clippy::type_complexity)]
pub fn creek_sim_system(
    mut commands: Commands,
    time: Res<Time>,
    creeks: Option<Res<Creeks>>,
    locals: Query<(&LocalCreeks, &GlobalTransform)>,
    cameras: Query<(&engine_render::Camera, &GlobalTransform), Without<engine_render::ExtraView>>,
    focus: Res<engine_world::FocusPoints>,
    ground: Option<Res<Ground>>,
    shared: Res<SharedWater>,
    mut shifts: MessageReader<WorldShift>,
    mut sims: ResMut<CreekSims>,
    mut ribbons: ResMut<CreekRibbons>,
    stones: Res<CreekStones>,
) {
    if let Some(s) = shifts.read().copied().reduce(|a, next| a.then(&next)) {
        for st in &mut sims.stretches {
            for p in &mut st.line {
                let q = s.point(DVec3::new(p.x, 0.0, p.y));
                *p = DVec2::new(q.x, q.z);
            }
        }
        // Planned in the old frame: asked again.
        sims.job = None;
    }
    // Running yet? When a new stretch runs, the ones before it go.
    if sims.stretches.iter().any(|s| !s.live) {
        let state = shared.0.read();
        for st in &mut sims.stretches {
            if !st.live && state.flows.iter().any(|f| f.entity == st.region) {
                st.live = true;
            }
        }
    }
    if let Some(newest) = sims.stretches.iter().rposition(|s| s.live) {
        let mut k = 0;
        sims.stretches.retain(|st| {
            let keep = k >= newest;
            if !keep {
                for e in &st.entities {
                    commands.entity(*e).despawn();
                }
            }
            k += 1;
            keep
        });
    }
    let cuts: Vec<Cut> = sims
        .stretches
        .iter()
        .filter(|s| s.live)
        .flat_map(|s| s.cuts.iter().copied())
        .collect();
    if ribbons.cuts != cuts {
        ribbons.cuts = cuts;
    }
    // A planned stretch: put in.
    if let Some(done) = sims.job.as_mut().and_then(|(_, t)| check_ready(t)) {
        let (job, _) = sims.job.take().expect("job");
        if let Some(plan) = done {
            let region = commands
                .spawn((
                    Name::new("Creek simulation"),
                    Transform::from_translation(plan.corner),
                    plan.region,
                ))
                .id();
            let entities = vec![region];
            sims.stretches.push(Stretch {
                cuts: job.cuts,
                provider: job.provider,
                chain: job.chain,
                span: job.span,
                line: job.line,
                entities,
                region,
                live: false,
            });
        }
    }
    sims.since += time.delta_secs();
    if sims.since < PERIOD_S {
        return;
    }
    sims.since = 0.0;
    let Some(camera) = main_camera(&cameras, &focus) else {
        return;
    };
    let eye = DVec2::new(camera.x, camera.z);
    // Too far from the camera: dropped.
    let far = |st: &Stretch| {
        st.line
            .iter()
            .map(|p| p.distance(eye))
            .fold(f64::MAX, f64::min)
            > DROP_M as f64
    };
    sims.stretches.retain(|st| {
        if far(st) {
            for e in &st.entities {
                commands.entity(*e).despawn();
            }
            false
        } else {
            true
        }
    });
    if sims.job.is_some() || sims.off {
        return;
    }
    let providers = gather_providers(creeks.as_deref(), &locals);
    // The nearest creek, and the reach it is on.
    let mut best: Option<(f32, usize)> = None;
    for (k, (provider, offset)) in providers.iter().enumerate() {
        if let Some(sample) = provider.nearest(camera - *offset, SIM_REACH_M)
            && best.is_none_or(|b| sample.distance_m < b.0)
        {
            best = Some((sample.distance_m, k));
        }
    }
    let Some((_, k)) = best else {
        return;
    };
    let (provider, offset): &Provider = &providers[k];
    let key = provider_key(provider);
    let local = camera - *offset;
    let Some(sample) = provider.nearest(local, SIM_REACH_M) else {
        return;
    };
    let mut reaches: Vec<CreekReach> = Vec::new();
    let c = DVec2::new(sample.center.x, sample.center.z);
    provider.reaches(c - DVec2::splat(2.0), c + DVec2::splat(2.0), &mut reaches);
    let Some((reach, at)) = reaches
        .into_iter()
        .map(|r| {
            let (s, d) = along_line(&r.points, c);
            (r, s, d)
        })
        .min_by(|a, b| a.2.total_cmp(&b.2))
        .map(|(r, s, _)| (r, s))
    else {
        return;
    };
    // By a side creek's mouth: simulated with the creek it runs into, from there.
    let joined = !sims.one_reach;
    let (reach, at) = if joined && length_of(&reach.points) - at <= TRIBUTARY_M {
        runs_into(provider.as_ref(), &reach).unwrap_or((reach, at))
    } else {
        (reach, at)
    };
    // Still round the middle of a stretch running (or about to) down this creek, or toward
    // an end of the creek it reaches, or by the mouth of a side creek it takes in: nothing to
    // do.
    let covered = sims.stretches.iter().any(|st| {
        if st.provider != key {
            return false;
        }
        if let Some(&(_, start)) = st.chain.iter().find(|c| c.0 == reach.id) {
            let at = start + at;
            let (from, to, total) = st.span;
            let len = to - from;
            let mid = from + 0.4 * len;
            return (at - mid).abs() < RECENTER_SHARE * len
                || (from <= 0.0 && at <= mid)
                || (to >= stretch_end(total) - 0.01 && at >= mid);
        }
        st.cuts
            .iter()
            .any(|c| c.reach == reach.id && at >= c.from_m - 5.0 && at <= c.to_m + 0.01)
    });
    if covered {
        return;
    }
    let Some(placed) = place_stretch(provider.as_ref(), &reach, at, joined) else {
        return;
    };
    let world = |line: &[CreekPoint]| -> Vec<CreekPoint> {
        line.iter()
            .map(|p| CreekPoint {
                at: p.at + *offset,
                ..*p
            })
            .collect()
    };
    let course = world(&placed.course);
    let tributaries: Vec<Vec<CreekPoint>> = placed.tributaries.iter().map(|t| world(t)).collect();
    let line: Vec<DVec2> = course
        .iter()
        .chain(tributaries.iter().flatten())
        .map(|p| DVec2::new(p.at.x, p.at.z))
        .collect();
    let job = Job {
        cuts: placed
            .cuts
            .iter()
            .map(|(r, from, to)| Cut {
                provider: key,
                reach: r.id,
                from_m: *from,
                to_m: *to,
            })
            .collect(),
        provider: key,
        chain: placed.chain.clone(),
        span: placed.span,
        line,
    };
    let Some(ground) = ground.map(|g| (*g).clone()) else {
        return;
    };
    let probe = course
        .iter()
        .chain(tributaries.iter().flatten())
        .map(|p| p.at.y)
        .fold(f64::MIN, f64::max)
        + 50.0;
    // Each reach's stones: as laid for the rocks drawn (else laid here the same way); none
    // when the stones are off.
    let off = stones.off;
    let parts: Vec<_> = placed
        .cuts
        .iter()
        .map(|(r, from, to)| {
            (
                stones.reach((key, r.id)),
                world(&r.points),
                reach_seed(r),
                *from,
                *to,
            )
        })
        .collect();
    let plan = move || {
        let height = |x: f64, z: f64| {
            ground
                .height(DVec3::new(x, probe, z))
                .unwrap_or(probe - 50.0)
        };
        let mut in_it = Vec::new();
        if !off {
            for (laid, whole, seed, from, to) in parts {
                let all = laid.unwrap_or_else(|| Arc::new(lay_reach_stones(&whole, &height, seed)));
                in_it.extend(stones_between(&all, from, to));
            }
        }
        plan_stretch(&course, &tributaries, &height, &in_it)
    };
    engine_core::tasks::init_task_pools();
    let task = AsyncComputeTaskPool::get().spawn(async move { plan() });
    sims.job = Some((job, task));
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::demo::creeks::meadow;

    #[test]
    fn a_stretch_fits_its_grid_and_follows_its_creek() {
        let m = meadow();
        for order in [1u8, 2, 3] {
            let reach = m.reaches.iter().find(|r| r.order == order).expect("reach");
            let total = length_of(&reach.points);
            let (from, to) = stretch_of(&reach.points, 0.5 * total).expect("stretch");
            assert!(to > from + 30.0, "order {order}: {from}..{to}");
            let course = sub_course(&reach.points, from, to);
            let width = course[0].width_m;
            assert!(cells_over(&course, cell_for(width) as f64) <= MOST_CELLS);
            // Its ends on the creek, where asked.
            let first = DVec2::new(course[0].at.x, course[0].at.z);
            let (s, d) = along_line(&reach.points, first);
            assert!(d < 0.01 && (s - from).abs() < 0.01, "{s} {d}");
            // Short of the mouth, where the creek runs into another.
            let (_, to_end) = stretch_of(&reach.points, total).expect("stretch at the mouth");
            assert!(to_end <= total - MOUTH_M + 0.01, "{to_end} {total}");
        }
    }

    #[test]
    fn a_planned_stretch_runs_down_its_creek_with_stones() {
        let m = meadow();
        let reach = m.reaches.iter().find(|r| r.order == 2).expect("reach");
        let total = length_of(&reach.points);
        let (from, to) = stretch_of(&reach.points, 0.5 * total).expect("stretch");
        let course = sub_course(&reach.points, from, to);
        let height = |x: f64, z: f64| m.height(x as f32, z as f32) as f64;
        let stones = lay_reach_stones(&reach.points, &height, reach_seed(reach));
        let plan =
            plan_stretch(&course, &[], &height, &stones_between(&stones, from, to)).expect("plan");
        assert!(!plan.region.sources.is_empty());
        assert!(!plan.region.rocks.is_empty());
        let cells = plan.region.cells[0] as f64 * plan.region.cells[1] as f64;
        assert!(cells <= MOST_CELLS * 1.05, "{cells}");
    }

    #[test]
    fn a_stretch_runs_on_across_a_junction_and_takes_in_the_side_creek() {
        let m = meadow();
        let id = |id: u64| m.reaches.iter().find(|r| r.id == id).expect("reach");
        // The main stream's third reach, a few meters below where its second ends and a side
        // creek (19) comes in.
        let reach = id(3);
        let placed = place_stretch(&*m, reach, 4.0, true).expect("placed");
        // It runs down the main stream across the junction rather than starting at it.
        let ids: Vec<u64> = placed.cuts.iter().map(|c| c.0.id).collect();
        assert!(ids.contains(&2) && ids.contains(&3), "{ids:?}");
        let (from, to, _) = placed.span;
        let start = placed.chain.iter().find(|c| c.0 == 3).expect("in chain").1;
        assert!(
            from < start - 20.0 && to > start + 20.0,
            "{from}..{to} ({start})"
        );
        // And the side creek's last meters run on its grid, cut from the ribbons.
        assert!(ids.contains(&19), "{ids:?}");
        let side = placed
            .cuts
            .iter()
            .find(|c| c.0.id == 19)
            .expect("side creek");
        assert!((side.2 - length_of(&side.0.points)).abs() < 0.01);
        assert!(side.2 - side.1 > 20.0, "{side:?}");
        // Planned with both springs, inside its budget.
        let height = |x: f64, z: f64| m.height(x as f32, z as f32) as f64;
        let plan = plan_stretch(&placed.course, &placed.tributaries, &height, &[]).expect("plan");
        assert_eq!(plan.region.sources.len(), 1 + placed.tributaries.len());
        let main_q = placed.course[0].flow_m3_s;
        let side_q = placed.tributaries[0][0].flow_m3_s;
        assert!((plan.region.sources[1].rate_m3_s - side_q).abs() < 1e-6);
        assert!(side_q < main_q);
        let cells = plan.region.cells[0] as f64 * plan.region.cells[1] as f64;
        assert!(cells <= MOST_CELLS * 1.3, "{cells}");
        // The side creek's head is inside the grid.
        let head = placed.tributaries[0][0].at;
        let size = DVec2::new(
            plan.region.cells[0] as f64 * plan.region.cell_m as f64,
            plan.region.cells[1] as f64 * plan.region.cell_m as f64,
        );
        let d = DVec2::new(head.x - plan.corner.x, head.z - plan.corner.z);
        assert!(
            d.x > 0.0 && d.y > 0.0 && d.x < size.x && d.y < size.y,
            "{d} {size}"
        );
    }

    #[test]
    fn a_side_creek_runs_into_the_main_stream() {
        let m = meadow();
        let id = |id: u64| m.reaches.iter().find(|r| r.id == id).expect("reach");
        // Side creek 19 ends a few meters from the main stream, by its reaches 2 and 3.
        let (into, at) = runs_into(&*m, id(19)).expect("runs into");
        assert_eq!(into.order, 3);
        assert!(at >= 0.0 && at <= length_of(&into.points), "{at}");
        // A headwater going on in the creek below it doesn't.
        assert!(runs_into(&*m, id(14)).is_none());
    }
}
