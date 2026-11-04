//! The creek nearest the camera, simulated: the near tier of the creeks (creeks-research.md,
//! "T0"). Up close a creek has to part round its stones, pile up in front of them and trail a
//! wake and an eddy line behind, which a ribbon with an analytic current can't draw. So the
//! stretch of the nearest creek within [`SIM_REACH_M`] runs on a flow grid of its own
//! ([`FlowRegion`], the mountain streams' simulation), with its stones laid by its reach
//! (`demo::valley::stream_stones`), and the ribbons ([`crate::creeks`]) leave that stretch out
//! once the simulation is running.
//!
//! The stretch is placed round the camera's nearest point on the creek, longer for a wider
//! creek and within a cell budget, built and settled on a worker, and swapped in when ready:
//! the ribbon draws the water until then, and the old stretch stays until the new one runs. It
//! is dropped when the camera is [`DROP_M`] from it.

use bevy_tasks::Task;
use bevy_tasks::futures::check_ready;
use engine_core::math::{DVec2, DVec3};
use engine_core::prelude::*;
use engine_core::tasks::AsyncComputeTaskPool;
use engine_world::terrain::{CreekPoint, CreekReach, Creeks};
use engine_world::{Ground, WorldShift};

use crate::creeks::{
    CreekRibbons, Cut, LocalCreeks, Provider, gather_providers, main_camera, provider_key,
};
use crate::demo::scenery::spawn_prop;
use crate::demo::valley::{
    StoneStretch, ValleyStream, plan_stones, stream_region_cells, stream_stones,
};
use crate::flow::FlowRegion;
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
    cut: Cut,
    /// Its course (world x, z), for how far the camera is from it.
    line: Vec<DVec2>,
    /// The flow region and the stones' props.
    entities: Vec<Entity>,
    region: Entity,
    /// Running: the ribbons leave it out.
    live: bool,
}

/// A stretch planned on a worker: its region's corner and settings, and its stones.
pub struct StretchPlan {
    corner: DVec3,
    region: FlowRegion,
    stones: Vec<StoneStretch>,
}

/// The stretches of creek simulated near the camera (see the module docs).
#[derive(Resource, Default)]
pub struct CreekSims {
    /// Simulate no creek (draw them all as ribbons): for before pictures and comparisons.
    pub off: bool,
    stretches: Vec<Stretch>,
    job: Option<(Cut, Vec<DVec2>, Task<Option<StretchPlan>>)>,
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

/// Plan a stretch's grid and stones (world-free: runs on a worker). `ground` is the height of
/// the ground at x, z.
pub fn plan_stretch(
    course: &[CreekPoint],
    ground: &dyn Fn(f64, f64) -> f64,
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
    let base = course.iter().map(|p| p.at.y).fold(f64::MAX, f64::min) - 6.0;
    let (corner, mut region) = stream_region_cells(&stream, base, ground, cell_for(width))?;
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
    region.fills.clear();
    let stones = stream_stones(&stream, ground);
    region.rocks = stones.iter().map(|s| s.rock(corner)).collect();
    Some(StretchPlan {
        corner,
        region,
        stones: plan_stones(&stones),
    })
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
        .map(|s| s.cut)
        .collect();
    if ribbons.cuts != cuts {
        ribbons.cuts = cuts;
    }
    // A planned stretch: put in.
    if let Some(done) = sims.job.as_mut().and_then(|(_, _, t)| check_ready(t)) {
        let (cut, line, _) = sims.job.take().expect("job");
        if let Some(plan) = done {
            let region = commands
                .spawn((
                    Name::new("Creek simulation"),
                    Transform::from_translation(plan.corner),
                    plan.region,
                ))
                .id();
            let mut entities = vec![region];
            for (k, (origin, parts, _)) in plan.stones.into_iter().enumerate() {
                entities.push(spawn_prop(
                    &mut commands,
                    &format!("Creek stones {k}"),
                    Transform::from_translation(origin),
                    parts,
                ));
            }
            sims.stretches.push(Stretch {
                cut,
                line,
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
    // Still round the middle of the stretch running (or about to) on this reach, or toward
    // the end of the reach it reaches: nothing to do.
    let total = length_of(&reach.points);
    let covered = sims.stretches.iter().any(|st| {
        let (from, to) = (st.cut.from_m, st.cut.to_m);
        let len = to - from;
        let mid = from + 0.4 * len;
        st.cut.provider == key
            && st.cut.reach == reach.id
            && ((at - mid).abs() < RECENTER_SHARE * len
                || (from <= 0.0 && at <= mid)
                || (to >= stretch_end(total) - 0.01 && at >= mid))
    });
    if covered {
        return;
    }
    let Some((from, to)) = stretch_of(&reach.points, at) else {
        return;
    };
    let course: Vec<CreekPoint> = sub_course(&reach.points, from, to)
        .into_iter()
        .map(|p| CreekPoint {
            at: p.at + *offset,
            ..p
        })
        .collect();
    let line: Vec<DVec2> = course.iter().map(|p| DVec2::new(p.at.x, p.at.z)).collect();
    let cut = Cut {
        provider: key,
        reach: reach.id,
        from_m: from,
        to_m: to,
    };
    let Some(ground) = ground.map(|g| (*g).clone()) else {
        return;
    };
    let probe = course.iter().map(|p| p.at.y).fold(f64::MIN, f64::max) + 50.0;
    let plan = move || {
        let height = |x: f64, z: f64| {
            ground
                .height(DVec3::new(x, probe, z))
                .unwrap_or(probe - 50.0)
        };
        plan_stretch(&course, &height)
    };
    engine_core::tasks::init_task_pools();
    let task = AsyncComputeTaskPool::get().spawn(async move { plan() });
    sims.job = Some((cut, line, task));
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
        let plan = plan_stretch(&course, &height).expect("plan");
        assert!(!plan.region.sources.is_empty());
        assert!(!plan.region.rocks.is_empty());
        let cells = plan.region.cells[0] as f64 * plan.region.cells[1] as f64;
        assert!(cells <= MOST_CELLS * 1.05, "{cells}");
    }
}
