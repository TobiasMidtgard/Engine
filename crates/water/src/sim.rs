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

/// Follow the focus and advance the interactive ripples.
pub fn ripple_system(
    time: Res<Time>,
    settings: Res<WaterSettings>,
    shared: Res<SharedWater>,
    focus: Res<FocusPoints>,
    cameras: Query<(&GlobalTransform, &Camera)>,
    mut book: ResMut<SimBook>,
    mut spare: Local<Option<crate::ripples::RippleField>>,
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
        book.since_rebuild = f32::INFINITY;
    }
    let f = focus_point(&focus, &cameras);
    let center = ripple_center(&state, f, state.ripples.size() * 0.5);
    let moved = state.ripples.recenter(center);
    book.since_rebuild += dt;
    if moved || book.since_rebuild > 2.0 {
        book.since_rebuild = 0.0;
        let state = &mut *state;
        let (bodies, flows, ground) = (&state.bodies, &state.flows, &state.ground);
        let speed = settings.ripple_speed;
        let level_y = f.y;
        // Lakes and streams: footsteps, cloth and swimmers ring both.
        state.ripples.rebuild_speeds(speed, |p| {
            let p3 = DVec3::new(p.x, level_y, p.y);
            bodies
                .iter()
                .filter(|b| b.body.interactive)
                .filter_map(|b| b.depth_at(p3, ground))
                .chain(flows.iter().filter(|f| f.visible).filter_map(|f| {
                    f.sample(DVec3::new(p.x, f.origin.y, p.y))
                        .map(|(_, depth, _)| depth)
                }))
                .fold(None, |acc: Option<f32>, d| {
                    Some(acc.map_or(d, |a| a.max(d)))
                })
        });
        // Streams carry their rings and foam downstream.
        state.ripples.rebuild_drift(|p| {
            flows.iter().filter(|f| f.visible).find_map(|f| {
                f.sample(DVec3::new(p.x, f.origin.y, p.y))
                    .map(|(_, _, v)| v)
            })
        });
    }
    if !state.ripples.needs_step() {
        // Calm water: nothing to step, copy or upload.
        state.ripples.pending.clear();
        return;
    }
    // Step with the water unlocked: the step runs on the compute pool, and a thread waiting
    // for it there may pick up another system that reads the water, which would wait for
    // this lock forever. The field steps aside, and readers see a copy of what it held until
    // it is swapped back.
    let field = spare.get_or_insert_with(|| state.ripples.clone());
    std::mem::swap(&mut state.ripples, field);
    if state.ripples.n != field.n {
        state.ripples = field.clone();
    }
    state.ripples.copy_view_from(field);
    state.ripples.pending.clear();
    drop(state);
    field.current_damping = 1.1 / settings.current_life_s.max(0.1);
    field.swirl = settings.swirl.max(0.0);
    field.step(dt, settings.ripple_damping, settings.foam_life_s);
    let mut state = shared.0.write();
    // Disturbances that came in meanwhile wait for the next step.
    field.pending.append(&mut state.ripples.pending);
    std::mem::swap(&mut state.ripples, field);
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

/// Create, update and advance flowing-water regions. Those out of the `SimulationRange` of
/// every focus hold still, as they last ran, until one comes near.
#[allow(clippy::too_many_arguments)]
pub fn flow_system(
    time: Res<Time>,
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
    mut splashes: MessageWriter<WaterSplash>,
    config: Res<EngineConfig>,
    mut status: ResMut<WaterLoading>,
    mut loading: Local<Vec<(Entity, Task<FlowSim>)>>,
    mut last_layout: Local<u64>,
    mut since_spray: Local<f32>,
) {
    engine_core::tasks::init_task_pools();
    let dt = time.delta_secs();
    // Falls send their spray out as `WaterSplash` puffs (particles show it, audio plays it).
    *since_spray += dt;
    let spray = *since_spray >= SPRAY_PERIOD;
    if spray {
        *since_spray = 0.0;
    }
    let mut state = shared.0.write();
    let state = &mut *state;
    let relayout = *last_layout != book.layout;
    *last_layout = book.layout;
    let mut seen = Vec::new();
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
                        state.flows.push(job.build());
                    } else {
                        let task = AsyncComputeTaskPool::get().spawn(async move { job.build() });
                        loading.push((entity, task));
                    }
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
        if crate::rests(&range, &focus, center, radius, &mut sim.resting) {
            continue;
        }
        sim.step(dt);
        if spray {
            for (foot, v, power) in sim.plunges(0.5) {
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
    state.flows.retain(|f| seen.contains(&f.entity));
    // A region removed while it was settling: drop its work (that cancels it).
    loading.retain(|(e, _)| seen.contains(e));
    if status.settling != loading.len() {
        status.settling = loading.len();
    }
}

/// Flowing water still being made ready: a loading screen waits until [`is_ready`].
///
/// [`is_ready`]: WaterLoading::is_ready
#[derive(Resource, Debug, Clone, Default)]
pub struct WaterLoading {
    /// Flow regions whose bed and settling stream are still being worked out.
    pub settling: usize,
}

impl WaterLoading {
    pub fn is_ready(&self) -> bool {
        self.settling == 0
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
