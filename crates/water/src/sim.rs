//! Fixed-step systems: keep bodies, waves, ripples and flowing regions up to date.

use std::hash::{Hash, Hasher};

use bevy_tasks::Task;
use bevy_tasks::futures::check_ready;
use engine_core::tasks::AsyncComputeTaskPool;

use engine_core::math::{DVec2, DVec3, EulerRot, Vec2, Vec3};
use engine_core::prelude::*;
use engine_render::Camera;
use engine_world::{FocusPoints, Ground, SimulationRange, WaterSplash, WindField};

use crate::body::{WaterBed, WaterBody, WaterSettings};
use crate::flow::{FlowBed, FlowRegion, FlowSim, grid_point};
use crate::state::{BodyState, SharedWater, WaterState};
use crate::waves::{WaveSet, build_waves};

/// Bookkeeping between steps.
#[derive(Resource, Default)]
pub struct SimBook {
    /// Hash of every body's placement and bed: when it changes, wave speeds and reservoirs are
    /// rebuilt.
    pub layout: u64,
    /// Seconds since the ripple speeds were last rebuilt.
    pub since_rebuild: f32,
    /// Ripple grid settings in use (cells, cell size).
    pub ripple_dims: (u32, f32),
}

fn typical_depth(body: &WaterBody, center: DVec3, ground: &Ground) -> f32 {
    match body.bed {
        WaterBed::Flat { depth_m } => depth_m,
        WaterBed::Bowl { depth_m, .. } => depth_m * 0.7,
        WaterBed::Ground => ground
            .height(center)
            .map(|h| (center.y - h) as f32)
            .filter(|d| *d > 0.1)
            .unwrap_or(30.0),
    }
}

/// Mirror `WaterBody` components into the simulation and advance their waves.
pub fn sync_bodies_system(
    time: Res<Time>,
    shared: Res<SharedWater>,
    wind: Res<WindField>,
    ground: Res<Ground>,
    bodies: Query<(
        Entity,
        &GlobalTransform,
        &Transform,
        Option<&ChildOf>,
        &WaterBody,
    )>,
    mut book: ResMut<SimBook>,
) {
    let dt = time.delta_secs();
    let mut state = shared.0.write();
    state.ground = ground.clone();
    state.time += dt as f64;
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    let mut seen = Vec::with_capacity(state.bodies.len());
    for (entity, gt, local, parent, body) in &bodies {
        // A root's own transform is its place even on its first step, before transforms have
        // propagated: flow regions made that step fill from the lakes they spill from.
        let (center, rotation) = match parent {
            None => (local.translation, local.rotation),
            Some(_) => (gt.translation(), gt.rotation()),
        };
        let yaw = rotation.to_euler(EulerRot::YXZ).0;
        entity.hash(&mut hasher);
        for v in [center.x, center.y, center.z, yaw as f64] {
            v.to_bits().hash(&mut hasher);
        }
        format!("{:?}{:?}{}", body.extent, body.bed, body.interactive).hash(&mut hasher);

        let depth = typical_depth(body, center, &state.ground);
        // The sea answers to the mean wind 10 m up, not to each passing gust (a storm's gusts
        // swing the wind by ±90%); local disturbances such as rotor wash still count.
        let w = wind.prevailing + wind.local.sample(center + DVec3::Y * 10.0);
        let target = build_waves(&body.waves, Vec2::new(w.x, w.z), depth);
        let idx = match state.bodies.iter().position(|b| b.entity == entity) {
            Some(i) => i,
            None => {
                state.bodies.push(BodyState {
                    entity,
                    center,
                    yaw,
                    body: body.clone(),
                    waves: WaveSet::default(),
                    mean_depth: depth,
                    wind: w,
                });
                let i = state.bodies.len() - 1;
                // Start with the full sea state (no slow build-up at load).
                state.bodies[i].waves.blend_toward(&target, 1.0);
                i
            }
        };
        let b = &mut state.bodies[idx];
        b.center = center;
        b.yaw = yaw;
        if b.body != *body {
            b.body = body.clone();
        }
        b.mean_depth = depth;
        // The sea builds and calms over `response_s`; a wind that jumps in one step (the
        // weather skipped ahead, a tour stop set a storm) brings its sea state with it.
        let jump = Vec2::new(w.x - b.wind.x, w.z - b.wind.z).length() > WIND_JUMP;
        b.wind = w;
        let t = if jump {
            1.0
        } else {
            1.0 - (-dt / body.waves.response_s.max(0.05)).exp()
        };
        b.waves.blend_toward(&target, t);
        b.waves.advance(dt);
        seen.push(entity);
    }
    state.bodies.retain(|b| seen.contains(&b.entity));
    let layout = hasher.finish();
    if layout != book.layout {
        book.layout = layout;
        book.since_rebuild = f32::INFINITY;
    }
}

/// The point detail follows: the primary `SimulationFocus`, or the active camera when nothing
/// is marked as the focus.
pub fn focus_point(focus: &FocusPoints, cameras: &Query<(&GlobalTransform, &Camera)>) -> DVec3 {
    if !focus.points.is_empty() {
        return focus.primary();
    }
    cameras
        .iter()
        .filter(|(_, c)| c.active)
        .max_by_key(|(_, c)| c.order)
        .map_or(DVec3::ZERO, |(g, _)| g.translation())
}

/// A change in the wind over one step bigger than this, m/s, is a jump (gusts change far
/// more slowly): the waves take on the new sea state at once.
const WIND_JUMP: f32 = 4.0;

/// Where the ripple grid goes: on the focus when it is over interactive water or near a
/// stream, else over the nearest interactive water within 150 m (so a camera on the shore
/// sees wakes on the lake).
fn ripple_center(state: &WaterState, focus: DVec3, half: f64) -> DVec2 {
    let p = DVec2::new(focus.x, focus.z);
    // Over or beside a stream: follow the focus (rings, wakes and foam show on the stream
    // round it).
    let margin = half * 0.5;
    if state.flows.iter().any(|f| {
        let l = f.local(focus);
        let (w, d) = ((f.nx as f32 * f.cell) as f64, (f.nz as f32 * f.cell) as f64);
        f.visible && l.x > -margin && l.y > -margin && l.x < w + margin && l.y < d + margin
    }) {
        return p;
    }
    let mut best: Option<(f64, DVec2)> = None;
    for b in state.bodies.iter().filter(|b| b.body.interactive) {
        let inset = b.body.extent.inset(b.local(focus)) as f64;
        if inset >= 0.0 {
            return p;
        }
        let gap = -inset;
        let to_center = DVec2::new(b.center.x, b.center.z) - p;
        if gap > 150.0 || best.is_some_and(|(g, _)| g <= gap) {
            continue;
        }
        let reach = (gap + half * 0.5).min(to_center.length());
        best = Some((gap, p + to_center.normalize_or_zero() * reach));
    }
    best.map_or(p, |(_, c)| c)
}

/// Still-water depth of the interactive bodies under each ripple cell, kept between
/// rebuilds: reading it means a terrain sample per cell under a lake on the ground, seconds
/// for the whole grid on a real landscape, and it only changes when the grid moves (then
/// only the cells it newly covers are read) or the bodies do. After a jump (a teleport, a
/// tour stop) the new cells are read [`STILL_READS`] a step, nearest the focus first, rather
/// than all at once (188 ms in one frame on arriving at the yard).
#[derive(Default)]
pub struct StillDepths {
    /// (grid corner, cells, cell size, `SimBook::layout`) the depths were read for.
    at: Option<(DVec2, usize, f32, u64)>,
    /// Per cell: the deepest interactive body there, NaN for none, infinite while unread.
    depth: Vec<f32>,
    /// Cells still unread.
    unread: usize,
    /// Never read more than [`STILL_READS`] a step, even the first time (the coarse level,
    /// whose water can come in over a few steps).
    patient: bool,
}

/// Cells of the ripple grid whose still depth is read per step after a jump (on four cores
/// about 2 ms of terrain samples).
const STILL_READS: usize = 2048;

impl StillDepths {
    /// Make `depth` the bodies' depths under a grid of `n`² cells of `cell` m from `origin`,
    /// reading at most `budget` new cells (all of them on the first read, or when the bodies
    /// change): call again while [`StillDepths::unread`] is above zero. Runs with the water
    /// unlocked: it goes wide when much is new.
    fn read(
        &mut self,
        at: (DVec2, usize, f32, u64),
        bodies: &[BodyState],
        ground: &Ground,
        level_y: f64,
        focus: DVec2,
    ) {
        let (origin, n, cell, layout) = at;
        let mut budget = STILL_READS;
        if self.at != Some(at) {
            let mut depth = vec![f32::INFINITY; n * n];
            match self.at {
                Some((old, old_n, old_cell, old_layout))
                    if (old_n, old_cell, old_layout) == (n, cell, layout)
                        && self.depth.len() == n * n =>
                {
                    let step = ((origin - old) / cell as f64).round();
                    if step.x.abs() < n as f64 && step.y.abs() < n as f64 {
                        let (di, dj) = (step.x as i64, step.y as i64);
                        for (idx, d) in depth.iter_mut().enumerate() {
                            let (i, j) = ((idx % n) as i64 + di, (idx / n) as i64 + dj);
                            if i >= 0 && j >= 0 && i < n as i64 && j < n as i64 {
                                *d = self.depth[j as usize * n + i as usize];
                            }
                        }
                    }
                }
                // The first read, or the lakes changed: all at once (the loading screen
                // covers it).
                _ if !self.patient => budget = usize::MAX,
                _ => {}
            }
            self.unread = depth.iter().filter(|d| **d == f32::INFINITY).count();
            self.depth = depth;
            self.at = Some(at);
        }
        if self.unread == 0 {
            return;
        }
        // The unread cells nearest the focus first.
        let mut todo: Vec<(f32, usize)> = self
            .depth
            .iter()
            .enumerate()
            .filter(|(_, d)| **d == f32::INFINITY)
            .map(|(idx, _)| {
                let p = origin
                    + DVec2::new((idx % n) as f64 + 0.5, (idx / n) as f64 + 0.5) * cell as f64;
                ((p - focus).length_squared() as f32, idx)
            })
            .collect();
        if todo.len() > budget {
            todo.select_nth_unstable_by(budget, |a, b| a.0.total_cmp(&b.0));
            todo.truncate(budget);
        }
        let read = |idx: usize| {
            let p =
                origin + DVec2::new((idx % n) as f64 + 0.5, (idx / n) as f64 + 0.5) * cell as f64;
            let p3 = DVec3::new(p.x, level_y, p.y);
            bodies
                .iter()
                .filter_map(|b| b.depth_at(p3, ground))
                .fold(f32::NAN, f32::max)
        };
        let mut got: Vec<(f32, usize)> = todo;
        if got.len() > 4 * n {
            engine_core::tasks::par_chunks_mut(&mut got, n * 8, |_, chunk| {
                for c in chunk {
                    c.0 = read(c.1);
                }
            });
        } else {
            for c in &mut got {
                c.0 = read(c.1);
            }
        }
        for (d, idx) in got {
            // A body that reads infinitely deep would never count as read.
            self.depth[idx] = if d == f32::INFINITY { f32::MAX } else { d };
            self.unread -= 1;
        }
    }

    /// Depth under cell `idx`: NaN where there is no still water or it isn't read yet.
    fn at_cell(&self, idx: usize) -> f32 {
        let d = self.depth[idx];
        if d == f32::INFINITY { f32::NAN } else { d }
    }
}

/// The water under the dispersive surface's coarse level ([`crate::surface::CoarseBed`]),
/// read like the ripple grid's still depths, but a few thousand cells a step from the
/// start.
pub struct CoarseStill {
    depths: StillDepths,
}

impl Default for CoarseStill {
    fn default() -> Self {
        Self {
            depths: StillDepths {
                patient: true,
                ..Default::default()
            },
        }
    }
}

impl CoarseStill {
    /// Read the coarse level's water round the ripple grid `field` (its old bed, moved in
    /// whole coarse cells), and hand `field` the bed when it changed.
    fn read(
        &mut self,
        field: &mut crate::ripples::RippleField,
        layout: u64,
        bodies: &[BodyState],
        ground: &Ground,
        level_y: f64,
        focus: DVec2,
    ) {
        use crate::surface::{COARSE, CoarseBed, N};
        if field.n != N {
            field.coarse = None;
            return;
        }
        let cell = field.cell * COARSE as f32;
        let c = cell as f64;
        let want = field.origin + DVec2::splat((field.size() - c * N as f64) * 0.5);
        let origin = match &field.coarse {
            Some(b) if b.cell == cell => b.origin + ((want - b.origin) / c).round() * c,
            _ => (want / c).round() * c,
        };
        self.depths
            .read((origin, N, cell, layout), bodies, ground, level_y, focus);
        let (mut sum, mut count) = (0.0f64, 0usize);
        let mut absorb = vec![0.0; N * N];
        let wet: Vec<f32> = (0..N * N)
            .map(|k| {
                let d = self.depths.at_cell(k);
                if d > 0.0 && d < 1e4 {
                    sum += d as f64;
                    count += 1;
                    absorb[k] = crate::surface::shore_absorb(d);
                    1.0
                } else {
                    0.0
                }
            })
            .collect();
        let depth = if count > 0 {
            (sum / count as f64) as f32
        } else {
            crate::surface::DEPTH_M
        }
        .clamp(0.5, 30.0);
        let same = field.coarse.as_ref().is_some_and(|b| {
            b.origin == origin
                && b.cell == cell
                && b.wet == wet
                && b.absorb == absorb
                && (b.depth - depth).abs() < 0.25
        });
        if !same {
            let version = field.coarse.as_ref().map_or(0, |b| b.version + 1);
            field.coarse = Some(std::sync::Arc::new(CoarseBed {
                origin,
                cell,
                wet,
                depth,
                absorb,
                version,
            }));
        }
    }
}

/// Fields stepped beside the physics step ([`SimSet::DuringPhysics`]), waiting to be put back
/// in [`SimSet::Environment`]. Meanwhile readers (the physics step's buoyancy among them) see
/// a copy of the state as it was, so what they read doesn't depend on which finishes first.
#[derive(Resource, Default)]
pub struct RipplesInFlight {
    /// The ripple field stepped this step, and the spare that holds readers' copy.
    ripples: Option<crate::ripples::RippleField>,
    ripple_spare: Option<crate::ripples::RippleField>,
}

/// Follow the focus and advance the interactive ripples, on a field taken out of the water
/// (readers see a copy of it until [`publish_ripples_system`] puts it back).
#[allow(clippy::too_many_arguments)]
pub fn ripple_system(
    time: Res<Time>,
    settings: Res<WaterSettings>,
    shared: Res<SharedWater>,
    focus: Res<FocusPoints>,
    cameras: Query<(&GlobalTransform, &Camera)>,
    mut book: ResMut<SimBook>,
    mut flight: ResMut<RipplesInFlight>,
    mut still: Local<StillDepths>,
    mut coarse: Local<CoarseStill>,
) {
    let dt = time.delta_secs();
    let mut state = shared.0.write();
    let dims = (
        settings.ripple_cells.clamp(16, 1024),
        settings.ripple_cell_m.max(0.02),
    );
    if book.ripple_dims != dims {
        book.ripple_dims = dims;
        state.ripples = crate::ripples::RippleField::new(dims.0 as usize, dims.1);
        flight.ripple_spare = None;
        book.since_rebuild = f32::INFINITY;
    }
    let f = focus_point(&focus, &cameras);
    let center = ripple_center(&state, f, state.ripples.size() * 0.5);
    book.since_rebuild += dt;
    let rebuild = state.ripples.would_recenter(center)
        || book.since_rebuild > 2.0
        || still.unread > 0
        || coarse.depths.unread > 0;
    if !rebuild && !state.ripples.needs_step() {
        // Calm water: nothing to step, copy or upload.
        state.ripples.pending.clear();
        return;
    }
    // Step with the water unlocked: the step runs on the compute pool, and a thread waiting
    // for it there may pick up another system that reads the water, which would wait for
    // this lock forever. The field steps aside, and readers see a copy of what it held until
    // it is put back.
    let mut field = flight
        .ripple_spare
        .take()
        .unwrap_or_else(|| state.ripples.clone());
    std::mem::swap(&mut state.ripples, &mut field);
    if state.ripples.n != field.n {
        state.ripples = field.clone();
    }
    state.ripples.copy_view_from(&field);
    state.ripples.pending.clear();
    let span = tracing::info_span!("water", name = "ripples: rebuild").entered();
    if rebuild {
        field.recenter(center);
        book.since_rebuild = 0.0;
        // The lakes under the grid, read with the water unlocked (AGENTS.md, "Performance
        // rules"): only cells the grid newly covers, or all of them when the lakes change.
        let at = (field.origin, field.n, field.cell, book.layout);
        let bodies: Vec<BodyState> = state
            .bodies
            .iter()
            .filter(|b| b.body.interactive)
            .cloned()
            .collect();
        let ground = state.ground.clone();
        drop(state);
        still.read(at, &bodies, &ground, f.y, DVec2::new(f.x, f.z));
        coarse.read(
            &mut field,
            book.layout,
            &bodies,
            &ground,
            f.y,
            DVec2::new(f.x, f.z),
        );
        state = shared.0.write();
        let (origin, n, cell) = (field.origin, field.n, field.cell);
        // Streams, cell by cell only where each one lies (cheap: a grid lookup).
        let mut stream = vec![(f32::NAN, None::<Vec2>); n * n];
        for sim in state.flows.iter().filter(|f| f.visible) {
            let (c, radius) = sim.bounds();
            let lo = ((DVec2::new(c.x, c.z) - DVec2::splat(radius) - origin) / cell as f64)
                .floor()
                .max(DVec2::ZERO);
            let hi = ((DVec2::new(c.x, c.z) + DVec2::splat(radius) - origin) / cell as f64)
                .ceil()
                .min(DVec2::splat(n as f64));
            for j in lo.y as usize..hi.y as usize {
                for i in lo.x as usize..hi.x as usize {
                    let p = origin + DVec2::new(i as f64 + 0.5, j as f64 + 0.5) * cell as f64;
                    if let Some((_, depth, v)) = sim.sample(DVec3::new(p.x, sim.origin.y, p.y)) {
                        let s = &mut stream[j * n + i];
                        s.0 = s.0.max(depth);
                        s.1 = s.1.or(Some(v));
                    }
                }
            }
        }
        drop(state);
        let index = |p: DVec2| {
            let g = ((p - origin) / cell as f64 - DVec2::splat(0.5)).round();
            (g.y as usize * n + g.x as usize).min(n * n - 1)
        };
        // Lakes and streams: footsteps, cloth and swimmers ring both.
        let still = &*still;
        field.rebuild_speeds(settings.ripple_speed, |p| {
            let k = index(p);
            let d = still.at_cell(k).max(stream[k].0);
            (!d.is_nan()).then_some(d)
        });
        // Streams carry their rings and foam downstream.
        field.rebuild_drift(|p| stream[index(p)].1);
    } else {
        drop(state);
    }
    drop(span);
    let _span = tracing::info_span!("water", name = "ripples: step").entered();
    if field.needs_step() {
        field.current_damping = 1.1 / settings.current_life_s.max(0.1);
        field.swirl = settings.swirl.max(0.0);
        field.step(dt, settings.ripple_damping, settings.foam_life_s);
    }
    flight.ripples = Some(field);
}

/// Put the ripple field stepped beside the physics step back where readers see it.
pub fn publish_ripples_system(shared: Res<SharedWater>, mut flight: ResMut<RipplesInFlight>) {
    let Some(mut field) = flight.ripples.take() else {
        return;
    };
    let mut state = shared.0.write();
    // Disturbances that came in meanwhile wait for the next step.
    field.pending.append(&mut state.ripples.pending);
    std::mem::swap(&mut state.ripples, &mut field);
    flight.ripple_spare = Some(field);
}

/// Bed heights for a new flow region (relative to its origin; the grid turned by `yaw`).
fn flow_bed(region: &FlowRegion, origin: DVec3, yaw: f32, ground: &Ground) -> Vec<f32> {
    let (nx, nz) = (
        region.cells[0].max(2) as usize,
        region.cells[1].max(2) as usize,
    );
    match &region.bed {
        FlowBed::Heights(h) => h.clone(),
        FlowBed::Ground => {
            let mut out = Vec::with_capacity(nx * nz);
            for j in 0..nz {
                for i in 0..nx {
                    let c = region.cell_m as f64;
                    let p = grid_point(
                        origin,
                        yaw,
                        DVec2::new((i as f64 + 0.5) * c, (j as f64 + 0.5) * c),
                        1000.0,
                    );
                    out.push(
                        ground
                            .height(p)
                            .map(|h| (h - origin.y) as f32)
                            .unwrap_or(0.0),
                    );
                }
            }
            out
        }
    }
}

/// Mark cells of a flow region that lie in still water (lakes act as reservoirs).
fn set_reservoirs(sim: &mut FlowSim, state_bodies: &[BodyState], ground: &Ground) {
    for j in 0..sim.nz {
        for i in 0..sim.nx {
            let p = sim.cell_center(i, j);
            let idx = j * sim.nx + i;
            let mut level = f32::NAN;
            for b in state_bodies {
                if let Some(d) = b.depth_at(DVec3::new(p.x, b.center.y, p.z), ground) {
                    let rel = (b.center.y - sim.origin.y) as f32;
                    if d > 0.0 && rel > sim.bed[idx] {
                        level = rel;
                    }
                }
            }
            sim.reservoir[idx] = level;
        }
    }
}

/// Create and update flowing-water regions (stepped by [`flow_step_system`]). Those out of
/// the `SimulationRange` of every focus hold still, as they last ran, until one comes near.
#[allow(clippy::too_many_arguments)]
pub fn flow_system(
    shared: Res<SharedWater>,
    focus: Res<FocusPoints>,
    range: Res<SimulationRange>,
    regions: Query<(
        Entity,
        &GlobalTransform,
        &Transform,
        Option<&ChildOf>,
        &FlowRegion,
    )>,
    book: Res<SimBook>,
    config: Res<EngineConfig>,
    mut status: ResMut<WaterLoading>,
    mut loading: Local<Vec<(Entity, Task<FlowSim>)>>,
    mut last_layout: Local<u64>,
) {
    engine_core::tasks::init_task_pools();
    let mut guard = shared.0.write();
    let state = &mut *guard;
    let relayout = *last_layout != book.layout;
    *last_layout = book.layout;
    let mut seen = Vec::new();
    // Regions headless runs make in place, after the lock is let go.
    let mut builds = Vec::new();
    for (entity, gt, local, parent, region) in &regions {
        seen.push(entity);
        // A root's own transform is its place even on the first step, before transforms have
        // propagated (a region sampling its bed from the ground at the world origin would
        // keep the wrong riverbed).
        let (origin, rotation) = match parent {
            None => (local.translation, local.rotation),
            Some(_) => (gt.translation(), gt.rotation()),
        };
        let yaw = rotation.to_euler(EulerRot::YXZ).0;
        let idx = match state.flows.iter().position(|f| f.entity == entity) {
            Some(i) => i,
            None => {
                // A new region's bed, its lakes and the stream settling along it take seconds
                // for a long course: that runs on a worker, as Unreal's async loading thread
                // and Unity's async loads do, and the frame thread only picks the region up
                // once it is ready (`WaterLoading` says how many are still settling). Headless
                // runs (tests, tours) wait for it there and then, so they see the same water.
                if !loading.iter().any(|(e, _)| *e == entity) {
                    let job = FlowJob {
                        entity,
                        region: region.clone(),
                        origin,
                        yaw,
                        ground: state.ground.clone(),
                        bodies: state.bodies.clone(),
                    };
                    if config.headless {
                        builds.push(job);
                        continue;
                    }
                    let task = AsyncComputeTaskPool::get().spawn(async move { job.build() });
                    loading.push((entity, task));
                }
                // Picked up where it is now (a world shift meanwhile moves it below).
                if let Some(k) = loading.iter().position(|(e, _)| *e == entity) {
                    let Some(sim) = check_ready(&mut loading[k].1) else {
                        continue;
                    };
                    // Finished: dropping its handle cancels nothing.
                    drop(loading.swap_remove(k));
                    state.flows.push(sim);
                }
                state.flows.len() - 1
            }
        };
        let sim = &mut state.flows[idx];
        sim.sync(region);
        if (sim.origin - origin).length_squared() > 1e-6 {
            sim.origin = origin;
        }
        if (sim.yaw - yaw).abs() > 1e-6 {
            sim.yaw = yaw;
        }
        if relayout {
            set_reservoirs(sim, &state.bodies, &state.ground);
        }
        let (center, radius) = sim.bounds();
        crate::rests(&range, &focus, center, radius, &mut sim.resting);
    }
    state.flows.retain(|f| seen.contains(&f.entity));
    // A region removed while it was settling: drop its work (that cancels it).
    loading.retain(|(e, _)| seen.contains(e));
    if status.settling != loading.len() {
        status.settling = loading.len();
    }
    if builds.is_empty() {
        return;
    }
    // Made with the water unlocked (AGENTS.md, "Performance rules").
    drop(guard);
    let built: Vec<FlowSim> = builds.into_iter().map(FlowJob::build).collect();
    shared.0.write().flows.extend(built);
}

/// Flow regions stepped beside the physics step, waiting for [`publish_flows_system`], and the
/// copies readers see meanwhile.
#[derive(Resource, Default)]
pub struct FlowsInFlight {
    flows: Option<Vec<FlowSim>>,
    views: Vec<FlowSim>,
}

/// Advance the flowing-water regions that aren't resting, on the regions taken out of the
/// water: readers see copies of them as they were until [`publish_flows_system`] puts them
/// back. Falls send their spray out as `WaterSplash` puffs (particles show it, audio plays
/// it).
pub fn flow_step_system(
    time: Res<Time>,
    shared: Res<SharedWater>,
    mut splashes: MessageWriter<WaterSplash>,
    mut flight: ResMut<FlowsInFlight>,
    mut since_spray: Local<f32>,
) {
    let dt = time.delta_secs();
    *since_spray += dt;
    let spray = *since_spray >= SPRAY_PERIOD;
    if spray {
        *since_spray = 0.0;
    }
    let mut state = shared.0.write();
    if state.flows.iter().all(|f| f.resting) {
        return;
    }
    // Step with the water unlocked (AGENTS.md, "Performance rules"): the step goes wide on
    // the compute pool, and a thread waiting there runs other modules' tasks, such as fire
    // sampling `Water`, which would wait for this lock forever.
    let mut flows = std::mem::take(&mut state.flows);
    let mut view = std::mem::take(&mut flight.views);
    view.truncate(flows.len());
    for (i, sim) in flows.iter().enumerate() {
        match view.get_mut(i) {
            Some(v) if v.entity == sim.entity && v.version == sim.version && sim.resting => {}
            Some(v) => v.copy_from(sim),
            None => view.push(sim.clone()),
        }
    }
    state.flows = view;
    drop(state);

    for sim in flows.iter_mut().filter(|sim| !sim.resting) {
        sim.step(dt);
        if spray {
            for &(foot, v, power) in &sim.falls().plunges {
                // About 1 kg of spray per second per 2 kW of falling water.
                let mass = power * 5e-4 * SPRAY_PERIOD;
                if mass < 0.02 {
                    continue;
                }
                // Flow times drop (m⁴/s): bigger falls throw spray higher and wider. The
                // cloud spans a few meters round the plunge (a 9 m fall of 1 m³/s: about 2 m
                // across), the mist drifting off it with the wind.
                let size = (power / 9810.0).max(0.0).sqrt();
                splashes.write(WaterSplash {
                    position: foot,
                    velocity: Vec3::new(v.x * 0.4, 1.5 + size.min(4.0), v.y * 0.4),
                    radius: (0.3 + size * 0.25).min(2.5),
                    mass_kg: mass,
                    source: Some(sim.entity),
                });
            }
        }
    }
    flight.flows = Some(flows);
}

/// Put the flow regions stepped beside the physics step back where readers see them.
pub fn publish_flows_system(shared: Res<SharedWater>, mut flight: ResMut<FlowsInFlight>) {
    let Some(flows) = flight.flows.take() else {
        return;
    };
    let mut state = shared.0.write();
    flight.views = std::mem::replace(&mut state.flows, flows);
}

/// Flowing water still being made ready: a loading screen waits until [`is_ready`].
///
/// [`is_ready`]: WaterLoading::is_ready
#[derive(Resource, Debug, Clone, Default)]
pub struct WaterLoading {
    /// Flow regions whose bed and settling stream are still being worked out.
    pub settling: usize,
    /// Pieces of the demos' art (textures, ground, rocks, people) still being made on
    /// workers ([`crate::demo::art`]).
    pub art: usize,
}

impl WaterLoading {
    /// Nothing left to make: the demo art is in and every stream has settled.
    pub fn is_ready(&self) -> bool {
        self.settling == 0 && self.art == 0
    }
}

/// Everything a new flow region needs to be made ready off the frame thread.
struct FlowJob {
    entity: Entity,
    region: FlowRegion,
    origin: DVec3,
    yaw: f32,
    ground: Ground,
    bodies: Vec<BodyState>,
}

impl FlowJob {
    /// The region's bed from the ground, its lakes, and its stream run until it has settled.
    fn build(self) -> FlowSim {
        let region = &self.region;
        let bed = flow_bed(region, self.origin, self.yaw, &self.ground);
        let mut sim = FlowSim::new(self.entity, self.origin, region, bed);
        sim.yaw = self.yaw;
        set_reservoirs(&mut sim, &self.bodies, &self.ground);
        // Let the stream establish itself before anyone looks: its priming springs run
        // alongside its own for the first half, so a long course fills along its whole
        // length, then its own alone so the stream settles to them.
        let steps = (region.prewarm_s.max(0.0) * 30.0) as u32;
        sim.sources = region
            .sources
            .iter()
            .chain(&region.priming)
            .copied()
            .collect();
        for k in 0..steps {
            if k == steps / 2 {
                sim.sources = region.sources.clone();
            }
            sim.step(1.0 / 30.0);
        }
        sim.sources = region.sources.clone();
        sim.drained_m3 = 0.0;
        sim.refresh_falls();
        sim
    }
}

/// Seconds between spray puffs from each waterfall.
const SPRAY_PERIOD: f32 = 0.25;

/// Publish counters for `engine.describe` / `render.stats`.
pub fn diagnostics_system(shared: Res<SharedWater>, mut diagnostics: ResMut<Diagnostics>) {
    let state = shared.0.read();
    diagnostics.set("water.bodies", state.bodies.len() as f64);
    diagnostics.set("water.flow_regions", state.flows.len() as f64);
    let energy = if state.ripples.has_water {
        state.ripples.energy() as f64
    } else {
        0.0
    };
    diagnostics.set("water.ripple_energy", energy);
    let volume: f64 = state.flows.iter().map(|f| f.volume()).sum();
    diagnostics.set("water.flow_volume_m3", volume);
    if let Some(b) = state.bodies.first() {
        diagnostics.set("water.wave_height_m", b.waves.significant_height() as f64);
        diagnostics.set(
            "water.wind_m_s",
            Vec3::new(b.wind.x, 0.0, b.wind.z).length() as f64,
        );
    }
}

/// Make sure the grid has a sensible size before the first step.
pub fn init_state(settings: &WaterSettings) -> WaterState {
    WaterState::new(
        settings.ripple_cells.clamp(16, 1024) as usize,
        settings.ripple_cell_m.max(0.02),
    )
}

/// Cells per side of the patches [`flow_bed_system`] looks over at once.
const BED_PATCH: usize = 8;
/// Patches looked over per step, across all streams: a stream's whole course in a few tenths
/// of a second, at a few dozen physics queries a step where nothing stands in it.
const BED_PATCHES_PER_STEP: usize = 24;
/// Things standing less than this above the ground don't part the water, meters.
const STANDING_MIN: f32 = 0.15;

/// Where [`flow_bed_system`] has got to in one stream.
pub struct BedScan {
    flow: Entity,
    next: usize,
    /// Every patch looked over once.
    swept: bool,
    /// Fixed scenery standing in the stream when it was made (a cliff it falls down, stones
    /// it was laid round): its bed was made round them already, so they are left out.
    scenery: Vec<Entity>,
}

/// One patch of a stream to look over: its box (center, half extents, turn) and its cells
/// (index, middle, ground and water surface, world y).
struct BedPatch {
    flow: Entity,
    first_sweep: bool,
    center: DVec3,
    half: Vec3,
    yaw: f32,
    /// Something stood in it last time (it must be looked at even if nothing is now).
    had: bool,
    cells: Vec<(usize, DVec3, f64, f64)>,
}

/// Keep the streams' beds up to date with what stands in them and the ground under them, so
/// the water parts round a rock dropped in, piles up behind a crate and finds the trench dug
/// for it. What stands on the bed comes from the physics scene, patch by patch (one overlap
/// test; rays only where something is); terrain edits re-read the ground.
#[allow(clippy::too_many_arguments)]
pub fn flow_bed_system(
    shared: Res<SharedWater>,
    physics: Option<Res<engine_world::physics::PhysicsQueries>>,
    regions: Query<&FlowRegion>,
    bodies: Query<&engine_world::physics::RigidBody>,
    parents: Query<&ChildOf>,
    mut changes: MessageReader<engine_world::TerrainChanged>,
    mut scans: Local<Vec<BedScan>>,
) {
    use engine_world::physics::{BodyKind, ColliderShape, QueryFilter};
    let changes: Vec<_> = changes.read().copied().collect();
    let mut patches: Vec<BedPatch> = Vec::new();
    let mut regrade: Vec<(Entity, usize, f32)> = Vec::new();
    {
        let state = shared.0.read();
        // Terrain edits: the ground under ground-bedded streams, read again where it changed.
        for c in &changes {
            for sim in &state.flows {
                let ground_bed = regions
                    .get(sim.entity)
                    .is_ok_and(|r| matches!(r.bed, FlowBed::Ground));
                let (center, radius) = sim.bounds();
                if !ground_bed || center.distance(c.center) > radius + c.radius as f64 {
                    continue;
                }
                for j in 0..sim.nz {
                    for i in 0..sim.nx {
                        let p = sim.cell_center(i, j);
                        let flat = DVec2::new(p.x - c.center.x, p.z - c.center.z).length();
                        if flat > c.radius as f64 + sim.cell as f64 {
                            continue;
                        }
                        let top = DVec3::new(p.x, sim.origin.y + 1000.0, p.z);
                        if let Some(h) = state.ground.height(top) {
                            regrade.push((sim.entity, j * sim.nx + i, (h - sim.origin.y) as f32));
                        }
                    }
                }
            }
        }
        // What stands in the water: a few patches of each running stream a step.
        if physics.is_some() {
            scans.retain(|s| state.flows.iter().any(|f| f.entity == s.flow));
            let running: Vec<&FlowSim> = state
                .flows
                .iter()
                .filter(|f| f.visible && !f.resting)
                .collect();
            let per = (BED_PATCHES_PER_STEP / running.len().max(1)).max(1);
            for sim in running {
                let (px, pz) = (sim.nx.div_ceil(BED_PATCH), sim.nz.div_ceil(BED_PATCH));
                let slot = match scans.iter().position(|s| s.flow == sim.entity) {
                    Some(k) => k,
                    None => {
                        scans.push(BedScan {
                            flow: sim.entity,
                            next: 0,
                            swept: false,
                            scenery: Vec::new(),
                        });
                        scans.len() - 1
                    }
                };
                for _ in 0..per.min(px * pz) {
                    let scan = &mut scans[slot];
                    let patch = scan.next;
                    scan.next = (patch + 1) % (px * pz);
                    let first_sweep = !scan.swept;
                    scan.swept |= scan.next == 0;
                    let (i0, j0) = ((patch % px) * BED_PATCH, (patch / px) * BED_PATCH);
                    let (i1, j1) = ((i0 + BED_PATCH).min(sim.nx), (j0 + BED_PATCH).min(sim.nz));
                    // Only patches with water in them, or something standing there last time.
                    let (mut lo, mut hi) = (f32::INFINITY, f32::NEG_INFINITY);
                    let (mut wet, mut had) = (false, false);
                    let mut cells = Vec::with_capacity(BED_PATCH * BED_PATCH);
                    for j in j0..j1 {
                        for i in i0..i1 {
                            let k = j * sim.nx + i;
                            wet |= sim.depth[k] > 0.01;
                            had |= sim.standing[k] > f32::NEG_INFINITY;
                            lo = lo.min(sim.ground[k]);
                            hi = hi.max(sim.bed[k] + sim.depth[k]);
                            cells.push((
                                k,
                                sim.cell_center(i, j),
                                sim.origin.y + sim.ground[k] as f64,
                                sim.origin.y + (sim.bed[k] + sim.depth[k]) as f64,
                            ));
                        }
                    }
                    if !wet && !had {
                        continue;
                    }
                    let c = sim.cell as f64;
                    let mid = DVec2::new((i0 + i1) as f64 * 0.5 * c, (j0 + j1) as f64 * 0.5 * c);
                    let (bottom, top) = (lo + STANDING_MIN, hi.max(lo + STANDING_MIN) + 0.5);
                    patches.push(BedPatch {
                        flow: sim.entity,
                        first_sweep,
                        center: sim.world(mid, (bottom + top) as f64 * 0.5),
                        half: Vec3::new(
                            (i1 - i0) as f32 * sim.cell * 0.5,
                            (top - bottom) * 0.5,
                            (j1 - j0) as f32 * sim.cell * 0.5,
                        ),
                        yaw: sim.yaw,
                        had,
                        cells,
                    });
                }
            }
        }
    }
    // The physics queries with the water unlocked (AGENTS.md, "Performance rules").
    let moves = |e: Entity| {
        let kind = |e: Entity| bodies.get(e).ok().map(|b| b.kind);
        let k = kind(e).or_else(|| parents.get(e).ok().and_then(|p| kind(p.parent())));
        matches!(k, Some(BodyKind::Dynamic | BodyKind::Kinematic))
    };
    let mut standing: Vec<(Entity, usize, f32)> = Vec::new();
    if let Some(physics) = &physics {
        for patch in &patches {
            let Some(scan) = scans.iter_mut().find(|s| s.flow == patch.flow) else {
                continue;
            };
            let mut found = Vec::new();
            physics.overlap(
                &ColliderShape::Cuboid {
                    half_extents: patch.half,
                },
                patch.center,
                engine_core::math::Quat::from_rotation_y(patch.yaw),
                &QueryFilter::default(),
                &mut found,
            );
            // Fixed things there on the first sweep are the stream's scenery.
            for e in &found {
                if patch.first_sweep && !moves(*e) && !scan.scenery.contains(e) {
                    scan.scenery.push(*e);
                }
            }
            found.retain(|e| !scan.scenery.contains(e));
            if found.is_empty() && !patch.had {
                continue;
            }
            let filter = QueryFilter {
                exclude: scan.scenery.clone(),
                ..Default::default()
            };
            for &(k, at, ground, surface) in &patch.cells {
                let from = surface.max(ground) + 3.0;
                let reach = (from - ground) as f32 + 0.3;
                let top = (!found.is_empty())
                    .then(|| {
                        physics.raycast(DVec3::new(at.x, from, at.z), Vec3::NEG_Y, reach, &filter)
                    })
                    .flatten()
                    .map(|h| h.point.y)
                    .filter(|y| *y > ground + STANDING_MIN as f64)
                    // Standing on the ground, not floating over it: right above the ground
                    // there is something (a ray starting inside it hits at once).
                    .filter(|_| {
                        physics
                            .raycast(
                                DVec3::new(at.x, ground + 0.05, at.z),
                                Vec3::Y,
                                0.25,
                                &filter,
                            )
                            .is_some_and(|h| h.distance <= 0.2)
                    });
                standing.push((patch.flow, k, top.map_or(f32::NAN, |y| y as f32)));
            }
        }
    }
    if regrade.is_empty() && standing.is_empty() {
        return;
    }
    let mut state = shared.0.write();
    for sim in state.flows.iter_mut() {
        for &(e, k, h) in &regrade {
            if e == sim.entity && k < sim.ground.len() {
                sim.set_ground(k, h);
            }
        }
        let base = sim.origin.y;
        for &(e, k, top) in &standing {
            if e == sim.entity && k < sim.standing.len() {
                let rel = if top.is_nan() {
                    f32::NEG_INFINITY
                } else {
                    (top as f64 - base) as f32
                };
                sim.set_standing(k, rel);
            }
        }
    }
}
