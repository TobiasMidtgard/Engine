//! Storm runoff: the rain the ground can't take in runs off the land, gathers in the hollows
//! and pours over the cliffs as threads of white water.
//!
//! On a worker, round the camera, the ground is sampled on a 1 m grid ([`WINDOW`] cells a
//! side), its pits filled (priority flood with a small rise per cell, Barnes, Lehman & Mulla
//! 2014) and every cell sent to its steepest downhill neighbour (D8, O'Callaghan & Mark
//! 1984), so each cell knows how much land drains through it. Where a hollow with enough land
//! behind it reaches a cliff's rim, the water pours over: the true edge and the face are found
//! with physics rays (rock props stand in front of the terrain's own step), and a thin fall is
//! traced down the face, sliding where the rock is steep and close, falling free where it
//! overhangs. The renderer draws them with the other falls ([`crate::flow::Falls`]).
//!
//! The flow over each rim is the land behind it times the rain the ground sheds:
//! Q = A · r · c, with the share c rising as the ground soaks up over the first minutes of
//! heavy rain (near 1 on bare rock and thin soil once wet, Horton's infiltration excess), and
//! falling away a few minutes after the rain stops. A flood held by `water.flood` holds it
//! wet.

use std::cmp::Reverse;
use std::collections::BinaryHeap;
use std::sync::Arc;

use bevy_tasks::Task;
use bevy_tasks::futures::check_ready;
use engine_core::math::{DVec2, DVec3, Vec2, Vec3};
use engine_core::prelude::*;
use engine_core::tasks::AsyncComputeTaskPool;
use engine_world::physics::{PhysicsQueries, PhysicsQuery, QueryFilter};
use engine_world::{FocusPoints, Ground, GroundQuery, WeatherState, WorldShift};

use crate::flow::{FALL_POINTS, FallPoint, Falls, Lip};

/// Cells a side of the window round the camera.
pub const WINDOW: usize = 256;
/// Cell size, meters.
pub const CELL_M: f64 = 1.0;
/// The camera moves this far before the land is routed again, meters.
const REBUILD_M: f64 = 48.0;
/// Seconds between routings while it rains even when the camera stays put (the rocks and
/// props the rays find stream in after the ground).
const REROUTE_S: f32 = 30.0;
/// Seconds between updates of the flows over the rims.
const PERIOD_S: f32 = 2.0;
/// A cell is a cliff's rim when the ground drops at least this much per meter over the next
/// [`RIM_CELLS`] cells down the water's way.
const RIM_SLOPE: f32 = 1.0;
/// Cells down the way over which a rim's drop is measured.
const RIM_CELLS: usize = 3;
/// Least land behind a rim cell for its water to count toward a thread, m² (less runs over
/// as a film).
const CELL_AREA_M2: f32 = 4.0;
/// Least land behind a thread (its rim cells gathered), m².
const LEAST_AREA_M2: f32 = 8.0;
/// Rims with more land behind them than this carry a stream all year; the stream's own flow
/// draws that one, m².
const MOST_AREA_M2: f32 = 20_000.0;
/// Rims closer than this along the edge pour as one thread, meters.
const MERGE_M: f64 = 3.0;
/// Rims this close to a simulated stream's lip (past its half width) are the stream's, meters.
const STREAM_CLEAR_M: f64 = 6.0;
/// Most threads drawn round the camera.
const MAX_THREADS: usize = 64;
/// Seconds for the ground to wet through in heavy rain (and to dry again).
const WETTING_S: f32 = 180.0;
/// Rain that sheds most of itself once the ground is wet, mm/h.
const HEAVY_RAIN_MM_H: f32 = 10.0;
/// Share of the rain that runs off wet rock and thin soil above a cliff.
const SHED: f32 = 0.8;
/// Least flow over a rim worth drawing, m³/s: a downpour (28 mm/h) gives it off 16 m² of
/// wet rock, so every gully down a cliff carries its thread.
const LEAST_FLOW: f32 = 0.0001;
/// Rain shed per m² the paths are traced for (25 mm/h on wet rock), m/s.
const NOMINAL_SHED: f32 = 25.0 / 3.6e6 * SHED;
/// Water sliding down rock rides this far off it, meters.
const SHEET: f32 = 0.05;

/// Where storm water pours over the cliffs round the camera, and how wet the ground is.
#[derive(Resource, Default)]
pub struct StormRunoff {
    /// 0 (dry) .. 1: how much of the rain the ground sheds.
    pub wet: f32,
    /// The rims found by the last routing, with their traced paths.
    rims: Arc<Vec<Thread>>,
    /// The threads as drawn: rims with flow, their lips carrying it.
    pub falls: Arc<Falls>,
    built_at: Option<DVec3>,
    /// Seconds since the land was last routed (rocks and props stream in after the ground).
    routed_s: f32,
    job: Option<Task<Vec<Thread>>>,
    since: f32,
    /// Flow per m² of land the drawn threads were sized for, m/s.
    shed_drawn: f32,
}

/// One rim and the path its water takes down the face.
#[derive(Debug, Clone)]
pub struct Thread {
    /// Land draining over it, m².
    pub area: f32,
    pub lip: Lip,
    pub path: Vec<FallPoint>,
}

impl StormRunoff {
    /// Rain shed per m² of land now, m/s.
    pub fn shed(&self, rain_mm_h: f32) -> f32 {
        rain_mm_h / 3.6e6 * SHED * self.wet
    }

    /// The rims found round the camera (before any flow).
    pub fn rims(&self) -> &[Thread] {
        &self.rims
    }
}

/// How the land drains: per cell, the neighbour it sends its water to and the land draining
/// through it.
pub struct Drainage {
    pub n: usize,
    /// Index of the neighbour (0..8, see [`STEPS`]) each cell drains to, or 8 off the window.
    pub to: Vec<u8>,
    /// Land draining through each cell, its own included, in cells.
    pub cells: Vec<f32>,
}

/// The eight neighbours, x and z offsets.
pub const STEPS: [(i32, i32); 8] = [
    (1, 0),
    (1, 1),
    (0, 1),
    (-1, 1),
    (-1, 0),
    (-1, -1),
    (0, -1),
    (1, -1),
];

/// Fill the pits of `heights` (`n` × `n`, row by row in z) and route every cell to its
/// steepest downhill neighbour, counting the land draining through each.
pub fn drain(heights: &[f32], n: usize) -> Drainage {
    assert_eq!(heights.len(), n * n);
    // Priority flood from the edges: each cell is raised to at least a hair above the one it
    // was reached from, so every cell has a way down to the edge.
    const RISE: f32 = 1e-3;
    let mut filled = vec![f32::NAN; n * n];
    let mut heap: BinaryHeap<Reverse<(OrdF32, u32)>> = BinaryHeap::new();
    for i in 0..n {
        for j in [0, n - 1] {
            for k in [j * n + i, i * n + j] {
                if filled[k].is_nan() {
                    filled[k] = heights[k];
                    heap.push(Reverse((OrdF32(heights[k]), k as u32)));
                }
            }
        }
    }
    while let Some(Reverse((OrdF32(h), k))) = heap.pop() {
        let (x, z) = ((k as usize % n) as i32, (k as usize / n) as i32);
        for (dx, dz) in STEPS {
            let (a, b) = (x + dx, z + dz);
            if a < 0 || b < 0 || a >= n as i32 || b >= n as i32 {
                continue;
            }
            let m = b as usize * n + a as usize;
            if filled[m].is_nan() {
                filled[m] = heights[m].max(h + RISE);
                heap.push(Reverse((OrdF32(filled[m]), m as u32)));
            }
        }
    }
    // Steepest way down over the filled land; cells on the edge with none drain off it.
    let mut to = vec![8u8; n * n];
    for z in 0..n as i32 {
        for x in 0..n as i32 {
            let k = z as usize * n + x as usize;
            let mut best = 0.0f32;
            for (s, (dx, dz)) in STEPS.iter().enumerate() {
                let (a, b) = (x + dx, z + dz);
                if a < 0 || b < 0 || a >= n as i32 || b >= n as i32 {
                    continue;
                }
                let m = b as usize * n + a as usize;
                let run = if dx * dz != 0 {
                    std::f32::consts::SQRT_2
                } else {
                    1.0
                };
                let fall = (filled[k] - filled[m]) / run;
                if fall > best {
                    best = fall;
                    to[k] = s as u8;
                }
            }
        }
    }
    // Highest first, each cell hands its land on.
    let mut order: Vec<u32> = (0..(n * n) as u32).collect();
    order.sort_unstable_by(|a, b| filled[*b as usize].total_cmp(&filled[*a as usize]));
    let mut cells = vec![1.0f32; n * n];
    for k in order {
        let k = k as usize;
        if let Some(m) = downhill(k, to[k], n) {
            cells[m] += cells[k];
        }
    }
    Drainage { n, to, cells }
}

/// The cell `k` drains to, if on the grid.
pub fn downhill(k: usize, to: u8, n: usize) -> Option<usize> {
    let (dx, dz) = *STEPS.get(to as usize)?;
    let (a, b) = ((k % n) as i32 + dx, (k / n) as i32 + dz);
    (a >= 0 && b >= 0 && a < n as i32 && b < n as i32).then(|| b as usize * n + a as usize)
}

#[derive(Clone, Copy, PartialEq)]
struct OrdF32(f32);
impl Eq for OrdF32 {}
impl PartialOrd for OrdF32 {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}
impl Ord for OrdF32 {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.0.total_cmp(&other.0)
    }
}

/// A cell at a cliff's rim with land enough behind it: where it is (world, the ground's
/// height), the way the water leaves (unit, world xz) and the land behind it (m²).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Rim {
    pub at: DVec3,
    pub dir: Vec2,
    pub area: f32,
}

/// The rims of `heights` (a [`WINDOW`]-style grid with its corner cell centred at `corner`,
/// heights relative to `base`): cells where the ground drops steeply to the next and the
/// water reaching them hasn't been falling already. Ones close together along an edge are
/// merged into the one with the most land behind it.
pub fn rims(heights: &[f32], n: usize, corner: DVec2, base: f64, drainage: &Drainage) -> Vec<Rim> {
    let cell = CELL_M as f32;
    // How steeply the water falls over the next few cells down its way (a terrain that
    // smooths its steps over a few meters still shows a cliff here), and its way at the start.
    let steep = |k: usize| -> Option<(f32, Vec2)> {
        let (dx, dz) = *STEPS.get(drainage.to[k] as usize)?;
        let mut at = k;
        let mut run = 0.0;
        for _ in 0..RIM_CELLS {
            let Some(m) = downhill(at, drainage.to[at], n) else {
                break;
            };
            let (sx, sz) = STEPS[drainage.to[at] as usize];
            run += Vec2::new(sx as f32, sz as f32).length() * cell;
            at = m;
        }
        (run > 0.0).then(|| {
            (
                (heights[k] - heights[at]) / run,
                Vec2::new(dx as f32, dz as f32).normalize(),
            )
        })
    };
    // Cells falling steeply whose biggest feeder isn't falling steeply too.
    let mut feeder = vec![(0.0f32, false); n * n];
    for k in 0..n * n {
        if let Some(m) = downhill(k, drainage.to[k], n)
            && drainage.cells[k] > feeder[m].0
        {
            let falls = steep(k).is_some_and(|(f, _)| f >= RIM_SLOPE);
            feeder[m] = (drainage.cells[k], falls);
        }
    }
    let area_of = |k: usize| drainage.cells[k] * cell * cell;
    let mut found: Vec<Rim> = (0..n * n)
        .filter(|k| (CELL_AREA_M2..MOST_AREA_M2).contains(&area_of(*k)))
        .filter_map(|k| {
            let (fall, dir) = steep(k)?;
            (fall >= RIM_SLOPE && !feeder[k].1).then(|| Rim {
                at: DVec3::new(
                    corner.x + (k % n) as f64 * CELL_M,
                    base + heights[k] as f64,
                    corner.y + (k / n) as f64 * CELL_M,
                ),
                dir,
                area: area_of(k),
            })
        })
        .collect();
    found.sort_by(|a, b| b.area.total_cmp(&a.area));
    let mut kept: Vec<Rim> = Vec::new();
    for r in found {
        match kept
            .iter_mut()
            .find(|k| DVec2::new(k.at.x - r.at.x, k.at.z - r.at.z).length() < MERGE_M)
        {
            Some(k) => k.area += r.area,
            None => kept.push(r),
        }
    }
    kept.retain(|r| r.area >= LEAST_AREA_M2);
    kept
}

/// Top of whatever stands at x/z (rock, a prop, the ground), searching down from `above`.
fn top_at(physics: &dyn PhysicsQuery, ground: &dyn GroundQuery, x: f64, z: f64, above: f64) -> f64 {
    let g = ground.height(DVec3::new(x, above, z)).unwrap_or(f64::MIN);
    let hit = physics
        .raycast(
            DVec3::new(x, above, z),
            Vec3::NEG_Y,
            (above - g).clamp(1.0, 200.0) as f32,
            &QueryFilter::default(),
        )
        .map_or(f64::MIN, |h| h.point.y);
    g.max(hit)
}

/// Ground under this steep (the up component of its normal, about 20 degrees) stops a thread:
/// on anything steeper, the scree and talus under a cliff, it tumbles on down as a cascade.
const LAND_NORMAL_Y: f32 = 0.94;
/// Ledges a thread steps down past its first drop.
const MAX_TIERS: usize = 4;
/// How far across a ledge a thread runs to find its edge, meters (a wider ledge holds it).
const LEDGE_M: f64 = 6.0;

/// The edge ahead of `from` along `dir` within `reach` meters: where the top of whatever
/// stands there (rock, a prop, the ground) drops by over 0.6 m within a step and stays down a
/// meter on. Its top, at the brink.
fn edge_ahead(
    physics: &dyn PhysicsQuery,
    ground: &dyn GroundQuery,
    from: DVec3,
    dir: Vec2,
    reach: f64,
) -> Option<DVec3> {
    const STEP: f64 = 0.25;
    let dir = DVec2::new(dir.x as f64, dir.y as f64);
    let above = from.y + 40.0;
    let mut prev = top_at(physics, ground, from.x, from.z, above);
    for s in 1..=(reach / STEP) as usize {
        let p = DVec2::new(from.x, from.z) + dir * (s as f64 * STEP);
        let top = top_at(physics, ground, p.x, p.y, above);
        if top < prev - 0.6 {
            let on = p + dir;
            if top_at(physics, ground, on.x, on.y, above) < prev - 0.6 {
                let back = p - dir * STEP;
                return Some(DVec3::new(back.x, prev, back.y));
            }
        }
        prev = top;
    }
    None
}

/// Trace the water over `rim`: find the true edge (walking on along its way until the top
/// drops away) and the path down the face. `None` where no edge is found or the drop is too
/// small to be a fall.
pub fn trace(physics: &dyn PhysicsQuery, ground: &dyn GroundQuery, rim: &Rim) -> Option<Thread> {
    const G: f32 = 9.81;
    const H: f32 = 1.0 / 90.0;
    let edge = edge_ahead(physics, ground, rim.at, rim.dir, 12.0)?;
    let mut tiers = 0;
    // The top of the drop the water is falling down now.
    let mut brink = edge.y;
    let way = Vec3::new(rim.dir.x, 0.0, rim.dir.y);
    // A rill's speed at the flow a downpour gives (Govers 1992: u = 3.52 Q^0.294). Slow,
    // thin water clings to the rock and slides; only a fast one leaps clear of the face.
    let speed = (3.52 * (rim.area * NOMINAL_SHED).powf(0.294)).clamp(0.25, 1.5);
    let filter = QueryFilter::default();
    let mut p = edge + DVec3::Y * SHEET as f64;
    let mut v = way * speed;
    let mut rock = Vec3::ZERO;
    let mut t = 0.0f32;
    let mut path = vec![(p, v, t, rock)];
    let mut landed = false;
    while t < 12.0 {
        t += H;
        if rock == Vec3::ZERO {
            v.y -= G * H;
        } else {
            let down = Vec3::new(0.0, -G, 0.0);
            v += (down - rock * down.dot(rock)) * H;
            v *= 1.0 - 1.2 * H;
        }
        let step = v * H;
        let len = step.length();
        let next = p + step.as_dvec3();
        // Rock (and props) by ray; the ground itself by its height, so the terrain's own
        // steps count where no collider stands.
        let hit = physics
            .raycast(p, step / len.max(1e-6), len + SHEET, &filter)
            .map(|h| (h.point, h.normal))
            .or_else(|| {
                let g = ground.sample(next + DVec3::Y * 2.0)?;
                (next.y < g.height + SHEET as f64 * 0.5)
                    .then(|| (DVec3::new(next.x, g.height, next.z), g.normal))
            });
        if let Some((point, normal)) = hit {
            if normal.y > LAND_NORMAL_Y && brink - point.y > 1.0 {
                p = point + normal.as_dvec3() * SHEET as f64;
                path.push((p, v, t, rock));
                // A ledge part way down: the water runs on across it and over its edge (a
                // storm's threads step down a banded cliff tier by tier to its foot).
                // (On along its way over the ledge; sliding down a face it has next to none
                // of its own, and runs away from the face the way the land falls.)
                let flat = Vec2::new(v.x, v.z);
                let heading = if flat.length() > 0.3 && flat.dot(rim.dir) > 0.0 {
                    flat.normalize()
                } else {
                    rim.dir
                };
                if tiers < MAX_TIERS
                    && let Some(next_edge) = edge_ahead(physics, ground, p, heading, LEDGE_M)
                {
                    tiers += 1;
                    let run = (next_edge - p).length() as f32;
                    t += run / speed;
                    p = next_edge + DVec3::Y * SHEET as f64;
                    brink = next_edge.y;
                    v = Vec3::new(heading.x, 0.0, heading.y) * speed;
                    rock = Vec3::ZERO;
                    path.push((p, v, t, normal));
                    continue;
                }
                landed = true;
                break;
            }
            if rock == Vec3::ZERO {
                // Hitting the face throws off some of its speed.
                v *= 0.7;
            }
            rock = normal;
            p = point + rock.as_dvec3() * SHEET as f64;
            let into = v.dot(rock);
            if into < 0.0 {
                v -= rock * into;
            }
        } else {
            p = next;
            if rock != Vec3::ZERO {
                // Still on the face, or does it fall away under the water?
                rock = physics
                    .raycast(p, -rock, SHEET + 0.35, &filter)
                    .map(|h| h.normal)
                    .or_else(|| {
                        let g = ground.sample(p + DVec3::Y * 2.0)?;
                        (p.y - g.height < (SHEET + 0.35) as f64).then_some(g.normal)
                    })
                    .unwrap_or(Vec3::ZERO);
            }
        }
        path.push((p, v, t, rock));
        if edge.y - p.y > 80.0 {
            break;
        }
    }
    let drop = (edge.y - p.y) as f32;
    if !landed || drop < 2.0 {
        return None;
    }
    let lip = Lip {
        top: edge + DVec3::Y * 0.02,
        dir: rim.dir,
        speed,
        q: 0.0,
        drop,
        width: 0.3,
        launch: 0.0,
        // Thin water over rough rock wanders and frays a little as it goes; a storm thread
        // stays a thread (Kauai's Weeping Wall, Yosemite's ephemeral falls), not a fan.
        turbulence: 0.1,
    };
    Some(Thread {
        area: rim.area,
        lip,
        path: even(&path, FALL_POINTS),
    })
}

/// `count` + 1 points evenly spaced along `path` (position, velocity, seconds, rock normal).
fn even(path: &[(DVec3, Vec3, f32, Vec3)], count: usize) -> Vec<FallPoint> {
    let mut along = vec![0.0f64; path.len()];
    for k in 1..path.len() {
        along[k] = along[k - 1] + path[k].0.distance(path[k - 1].0);
    }
    let total = along.last().copied().unwrap_or(0.0).max(1e-4);
    let last = path.len() - 1;
    let mut k = 0;
    (0..=count)
        .map(|s| {
            let want = total * s as f64 / count.max(1) as f64;
            while k + 1 < last && along[k + 1] < want {
                k += 1;
            }
            let (a, b) = (path[k], path[(k + 1).min(last)]);
            let span = (along[(k + 1).min(last)] - along[k]).max(1e-6);
            let f = ((want - along[k]) / span).clamp(0.0, 1.0);
            FallPoint {
                pos: a.0.lerp(b.0, f),
                vel: a.1.lerp(b.1, f as f32),
                t: a.2 + (b.2 - a.2) * f as f32,
                rock: if f < 0.5 { a.3 } else { b.3 },
            }
        })
        .collect()
}

/// Width of the band a thread wanders over at the rim for a flow `q` (m³/s): its rivulets
/// spread with the square root of the flow (rill width ~ Q^0.3-0.5); a few litres a second
/// wander over a third of a meter.
pub fn thread_width(q: f32) -> f32 {
    (4.0 * q.max(0.0).sqrt()).clamp(0.3, 2.0)
}

/// Flow per meter of lip each thread is drawn with, m²/s. A thread tumbling down rough rock
/// is white water through the band it wanders over, not a thin glassy film across it, so it
/// is drawn with the coverage of a sheet this thick.
const THREAD_COVER_Q: f32 = 0.08;

/// Route the land round `camera` and trace the threads over its rims (on a worker).
/// `lips` are where the simulated streams already fall: rims beside them are theirs.
pub fn find_threads(
    physics: &dyn PhysicsQuery,
    ground: &dyn GroundQuery,
    camera: DVec3,
    lips: &[(DVec3, f32)],
) -> Vec<Thread> {
    let n = WINDOW;
    let half = (n as f64 * CELL_M) * 0.5;
    let corner = DVec2::new(
        (camera.x - half).floor() + 0.5,
        (camera.z - half).floor() + 0.5,
    );
    let base = camera.y;
    let mut heights = vec![0.0f32; n * n];
    for (k, h) in heights.iter_mut().enumerate() {
        let x = corner.x + (k % n) as f64 * CELL_M;
        let z = corner.y + (k / n) as f64 * CELL_M;
        *h = ground
            .height(DVec3::new(x, base + 500.0, z))
            .map_or(-1000.0, |g| (g - base) as f32);
    }
    let drainage = drain(&heights, n);
    let in_flow = |p: DVec3| {
        lips.iter().any(|(at, width)| {
            DVec2::new(at.x - p.x, at.z - p.z).length() < (*width as f64 * 0.5 + STREAM_CLEAR_M)
        })
    };
    let mut found: Vec<Rim> = rims(&heights, n, corner, base, &drainage)
        .into_iter()
        .filter(|r| !in_flow(r.at))
        .collect();
    // The ones nearest the camera first.
    found.sort_by(|a, b| a.at.distance(camera).total_cmp(&b.at.distance(camera)));
    let threads: Vec<Thread> = found
        .iter()
        .filter_map(|r| trace(physics, ground, r))
        .take(MAX_THREADS)
        .collect();
    tracing::info!(
        rims = found.len(),
        threads = threads.len(),
        "storm runoff routed"
    );
    threads
}

/// Lips and paths for the threads with flow, for the renderer.
pub fn falls_for(threads: &[Thread], shed: f32) -> Falls {
    let mut falls = Falls::default();
    for t in threads {
        let q = t.area * shed;
        if q < LEAST_FLOW {
            continue;
        }
        let width = thread_width(q);
        let mut lip = t.lip;
        lip.width = width;
        lip.q = (q / width).max(THREAD_COVER_Q * (q / 0.0003).min(1.0));
        falls.lips.push(lip);
        falls.paths.push(t.path.clone());
        falls.sides.push([false, false]);
        // Where it lands it splashes (`Falls::plunges`: the renderer throws a crown of drops
        // there, no boil: it lands on ground).
        if let Some(foot) = t.path.last() {
            let away = Vec2::new(foot.vel.x, foot.vel.z);
            falls
                .plunges
                .push((foot.pos, away, 9810.0 * q * t.lip.drop));
        }
    }
    falls
}

/// Keeps [`StormRunoff`] up to date round the camera (see the module docs).
#[allow(clippy::too_many_arguments)]
pub fn storm_runoff_system(
    time: Res<Time>,
    weather: Res<WeatherState>,
    ground: Res<Ground>,
    physics: Option<Res<PhysicsQueries>>,
    cameras: Query<(&engine_render::Camera, &GlobalTransform), Without<engine_render::ExtraView>>,
    focus: Res<FocusPoints>,
    shared: Res<crate::state::SharedWater>,
    freshet: Option<Res<crate::freshet::Freshet>>,
    mut shifts: MessageReader<WorldShift>,
    mut runoff: ResMut<StormRunoff>,
) {
    let dt = time.delta_secs();
    let rain = if weather.is_raining() {
        weather.precipitation_mm_h
    } else {
        0.0
    };
    // The ground wets through in heavy rain and dries after; a held flood keeps it wet.
    let target = (rain / HEAVY_RAIN_MM_H).min(1.0);
    let held = freshet.is_some_and(|f| f.held && f.swell > 1.5);
    let wet = if held {
        1.0
    } else {
        runoff.wet + (target - runoff.wet) * (1.0 - (-dt / WETTING_S).exp())
    };
    if (wet - runoff.wet).abs() > 1e-6 {
        runoff.wet = wet;
    }
    if shifts.read().next().is_some() {
        runoff.built_at = None;
        runoff.job = None;
    }
    if let Some(found) = runoff.job.as_mut().and_then(check_ready) {
        runoff.job = None;
        tracing::info!(
            "storm runoff: {} threads over the rims round the camera",
            found.len()
        );
        runoff.rims = Arc::new(found);
        runoff.shed_drawn = -1.0;
    }
    runoff.since += dt;
    runoff.routed_s += dt;
    if runoff.since < PERIOD_S {
        return;
    }
    runoff.since = 0.0;
    // Sized again when the flow over the rims moves by a tenth.
    let shed = runoff.shed(rain);
    let drawn = runoff.shed_drawn;
    if (shed - drawn).abs() > 0.1 * drawn.max(1e-9) || (drawn < 0.0) {
        runoff.shed_drawn = shed;
        runoff.falls = Arc::new(falls_for(&runoff.rims, shed));
    }
    if shed <= 0.0 && runoff.rims.is_empty() {
        return;
    }
    let Some(camera) = crate::creeks::main_camera(&cameras, &focus) else {
        return;
    };
    if runoff.job.is_some()
        || (runoff
            .built_at
            .is_some_and(|b| b.distance(camera) < REBUILD_M)
            && runoff.routed_s < REROUTE_S)
    {
        return;
    }
    // Only route the land once it rains (and keep the last until the camera moves on).
    if shed <= 0.0 {
        return;
    }
    let Some(physics) = physics else {
        return;
    };
    runoff.built_at = Some(camera);
    runoff.routed_s = 0.0;
    let lips: Vec<(DVec3, f32)> = shared
        .0
        .read()
        .flows
        .iter()
        .flat_map(|f| f.falls().lips.iter().map(|l| (l.top, l.width)))
        .collect();
    let physics = physics.0.clone();
    let ground = ground.0.clone();
    match AsyncComputeTaskPool::try_get() {
        Some(pool) => {
            runoff.job = Some(pool.spawn(async move {
                find_threads(physics.as_ref(), ground.as_ref(), camera, &lips)
            }));
        }
        None => {
            runoff.rims = Arc::new(find_threads(
                physics.as_ref(),
                ground.as_ref(),
                camera,
                &lips,
            ));
            runoff.shed_drawn = -1.0;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_valley_drains_down_its_middle() {
        // A V valley sloping down in x: everything ends up in the middle row and off the
        // low edge.
        let n = 32;
        let h: Vec<f32> = (0..n * n)
            .map(|k| {
                let (x, z) = ((k % n) as f32, (k / n) as f32);
                (z - 15.5).abs() * 0.5 + (n as f32 - x) * 0.1
            })
            .collect();
        let low_end = |d: &Drainage| d.cells[15 * n + n - 2] + d.cells[16 * n + n - 2];
        let d = drain(&h, n);
        assert!(low_end(&d) > (n * n) as f32 * 0.5, "{}", low_end(&d));
        // A pit doesn't trap the water.
        let mut pit = h.clone();
        pit[15 * n + 20] -= 3.0;
        let d2 = drain(&pit, n);
        assert!(low_end(&d2) > (n * n) as f32 * 0.5, "{}", low_end(&d2));
    }

    #[test]
    fn rims_are_found_where_a_hollow_meets_a_cliff() {
        // A plateau sloping gently toward a cliff at x = 40 (20 m high), with a hollow
        // gathering the water at z = 32.
        let n = 64;
        let h: Vec<f32> = (0..n * n)
            .map(|k| {
                let (x, z) = ((k % n) as f32, (k / n) as f32);
                let top = 20.0 + (40.0 - x) * 0.05 + (z - 32.0).abs() * 0.08;
                if x < 40.0 { top } else { (63.0 - x) * 0.02 }
            })
            .collect();
        let d = drain(&h, n);
        let found = rims(&h, n, DVec2::ZERO, 0.0, &d);
        assert!(!found.is_empty());
        let big = found[0];
        assert!((big.at.z - 32.0).abs() < 2.0, "{big:?}");
        // Within a few cells of the edge (the trace walks on to it).
        assert!((36.0..40.0).contains(&big.at.x), "{big:?}");
        assert!(big.dir.x > 0.6, "{big:?}");
        assert!(big.area > 300.0, "{big:?}");
        // Nothing along the face itself counts as a rim again.
        assert!(found.iter().all(|r| r.at.x < 40.0), "{found:?}");
    }

    use engine_world::physics::Hit;

    /// A 20 m cliff face at x = 40 over flat ground at 0, rays marched in 1 cm steps.
    struct Cliff;
    impl Cliff {
        fn solid(p: DVec3) -> bool {
            p.y < 0.0 || (p.x < 40.0 && p.y < 20.0)
        }
    }
    impl PhysicsQuery for Cliff {
        fn raycast(&self, o: DVec3, d: Vec3, max: f32, _: &QueryFilter) -> Option<Hit> {
            let d = d.as_dvec3();
            let mut prev = o;
            for k in 1..=(max * 100.0) as usize {
                let p = o + d * (k as f64 * 0.01);
                if Self::solid(p) {
                    let normal = if prev.x >= 40.0 && p.x < 40.0 && p.y > 0.0 {
                        Vec3::X
                    } else {
                        Vec3::Y
                    };
                    return Some(Hit {
                        entity: Entity::PLACEHOLDER,
                        point: prev,
                        normal,
                        distance: k as f32 * 0.01,
                        surface: Default::default(),
                    });
                }
                prev = p;
            }
            None
        }
        fn shapecast(
            &self,
            _: &engine_world::physics::ColliderShape,
            _: DVec3,
            _: engine_core::math::Quat,
            _: Vec3,
            _: f32,
            _: &QueryFilter,
        ) -> Option<Hit> {
            None
        }
        fn overlap(
            &self,
            _: &engine_world::physics::ColliderShape,
            _: DVec3,
            _: engine_core::math::Quat,
            _: &QueryFilter,
            _: &mut Vec<Entity>,
        ) {
        }
        fn closest_point(&self, _: DVec3, _: f32, _: &QueryFilter) -> Option<Hit> {
            None
        }
    }
    struct Flat;
    impl GroundQuery for Flat {
        fn sample(&self, _: DVec3) -> Option<engine_world::GroundSample> {
            Some(engine_world::GroundSample {
                height: 0.0,
                normal: Vec3::Y,
                surface: Default::default(),
            })
        }
    }

    #[test]
    fn a_thread_runs_from_the_true_edge_to_the_foot() {
        // The terrain's rim is 3 m back from the rock's edge: the thread starts at the rock.
        let rim = Rim {
            at: DVec3::new(37.0, 20.0, 5.0),
            dir: Vec2::X,
            area: 400.0,
        };
        let t = trace(&Cliff, &Flat, &rim).expect("a thread");
        assert!((t.lip.top.x - 40.0).abs() < 0.3, "{:?}", t.lip.top);
        assert!((t.lip.drop - 20.0).abs() < 0.5, "{}", t.lip.drop);
        let foot = t.path.last().unwrap().pos;
        assert!(foot.y < 0.5, "{foot:?}");
        // Slow water falls close to the face, not out across the ground.
        assert!(foot.x > 40.0 && foot.x < 42.5, "{foot:?}");
        assert_eq!(t.path.len(), FALL_POINTS + 1);
        // No edge, no thread.
        let flat = Rim {
            at: DVec3::new(10.0, 20.0, 5.0),
            dir: Vec2::Y,
            area: 400.0,
        };
        assert!(trace(&Cliff, &Flat, &flat).is_none());
    }

    /// [`Cliff`] with a 3 m ledge half way down.
    struct Banded;
    impl PhysicsQuery for Banded {
        fn raycast(&self, o: DVec3, d: Vec3, max: f32, _: &QueryFilter) -> Option<Hit> {
            let solid =
                |p: DVec3| p.y < 0.0 || (p.x < 40.0 && p.y < 20.0) || (p.x < 43.0 && p.y < 10.0);
            let d = d.as_dvec3();
            let mut prev = o;
            for k in 1..=(max * 100.0) as usize {
                let p = o + d * (k as f64 * 0.01);
                if solid(p) {
                    let face = (prev.x >= 40.0 && p.x < 40.0 && p.y > 10.0)
                        || (prev.x >= 43.0 && p.x < 43.0 && p.y > 0.0);
                    return Some(Hit {
                        entity: Entity::PLACEHOLDER,
                        point: prev,
                        normal: if face { Vec3::X } else { Vec3::Y },
                        distance: k as f32 * 0.01,
                        surface: Default::default(),
                    });
                }
                prev = p;
            }
            None
        }
        fn shapecast(
            &self,
            _: &engine_world::physics::ColliderShape,
            _: DVec3,
            _: engine_core::math::Quat,
            _: Vec3,
            _: f32,
            _: &QueryFilter,
        ) -> Option<Hit> {
            None
        }
        fn overlap(
            &self,
            _: &engine_world::physics::ColliderShape,
            _: DVec3,
            _: engine_core::math::Quat,
            _: &QueryFilter,
            _: &mut Vec<Entity>,
        ) {
        }
        fn closest_point(&self, _: DVec3, _: f32, _: &QueryFilter) -> Option<Hit> {
            None
        }
    }

    #[test]
    fn a_thread_steps_down_a_ledge_to_the_foot() {
        let rim = Rim {
            at: DVec3::new(37.0, 20.0, 5.0),
            dir: Vec2::X,
            area: 400.0,
        };
        let t = trace(&Banded, &Flat, &rim).expect("a thread");
        // It lands on the ledge, runs across it and falls on to the ground at the cliff's foot.
        assert!((t.lip.drop - 20.0).abs() < 0.5, "{}", t.lip.drop);
        let foot = t.path.last().unwrap().pos;
        assert!(foot.y < 0.5 && foot.x > 43.0, "{foot:?}");
        assert!(
            t.path
                .iter()
                .any(|p| (p.pos.y - 10.0).abs() < 0.5 && p.pos.x < 43.2),
            "{:?}",
            t.path
        );
    }

    /// Scree at 45 degrees from half way down [`Cliff`]'s face out to 10 m from it.
    struct Talus;
    impl GroundQuery for Talus {
        fn sample(&self, p: DVec3) -> Option<engine_world::GroundSample> {
            let on = (40.0..50.0).contains(&p.x);
            Some(engine_world::GroundSample {
                height: if p.x < 40.0 {
                    20.0
                } else {
                    (50.0 - p.x).clamp(0.0, 10.0)
                },
                normal: if on {
                    Vec3::new(1.0, 1.0, 0.0).normalize()
                } else {
                    Vec3::Y
                },
                surface: Default::default(),
            })
        }
    }

    #[test]
    fn a_thread_tumbles_down_the_scree_to_the_flat() {
        let rim = Rim {
            at: DVec3::new(37.0, 20.0, 5.0),
            dir: Vec2::X,
            area: 400.0,
        };
        let t = trace(&Cliff, &Talus, &rim).expect("a thread");
        let foot = t.path.last().unwrap().pos;
        assert!(foot.y < 0.5 && foot.x > 49.0, "{foot:?}");
        assert!((t.lip.drop - 20.0).abs() < 0.5, "{}", t.lip.drop);
        // And splashes where it lands.
        let falls = falls_for(&[t], 0.0001);
        assert_eq!(falls.plunges.len(), 1);
        assert!(falls.plunges[0].0.distance(foot) < 1e-6 && falls.plunges[0].2 > 0.0);
    }

    #[test]
    fn threads_take_the_rain_the_land_sheds() {
        let lip = Lip {
            top: DVec3::ZERO,
            dir: Vec2::X,
            speed: 0.8,
            q: 0.0,
            drop: 20.0,
            width: 0.3,
            launch: 0.0,
            turbulence: 0.3,
        };
        let t = Thread {
            area: 1000.0,
            lip,
            path: vec![],
        };
        let mut r = StormRunoff {
            wet: 1.0,
            ..Default::default()
        };
        // 28 mm/h on 1000 m² of wet rock: about 6 litres a second.
        let shed = r.shed(28.0);
        let falls = falls_for(std::slice::from_ref(&t), shed);
        let lip = falls.lips[0];
        assert!((t.area * shed - 0.0062).abs() < 0.001, "{shed}");
        assert!(
            lip.width >= 0.3 && lip.q >= THREAD_COVER_Q * 0.99,
            "{lip:?}"
        );
        r.wet = 0.0;
        assert!(falls_for(&[t], r.shed(28.0)).lips.is_empty());
    }
}
