//! Fixed-step systems: keep bodies, waves, ripples and flowing regions up to date.

use std::hash::{Hash, Hasher};

use engine_core::math::{DVec2, DVec3, EulerRot, Vec2, Vec3};
use engine_core::prelude::*;
use engine_render::Camera;
use engine_world::{FocusPoints, Ground, WindField};

use crate::body::{WaterBed, WaterBody, WaterSettings};
use crate::flow::{FlowBed, FlowRegion, FlowSim};
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
    bodies: Query<(Entity, &GlobalTransform, &WaterBody)>,
    mut book: ResMut<SimBook>,
) {
    let dt = time.delta_secs();
    let mut state = shared.0.write();
    state.ground = ground.clone();
    state.time += dt as f64;
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    let mut seen = Vec::with_capacity(state.bodies.len());
    for (entity, gt, body) in &bodies {
        let center = gt.translation();
        let yaw = gt.rotation().to_euler(EulerRot::YXZ).0;
        entity.hash(&mut hasher);
        for v in [center.x, center.y, center.z, yaw as f64] {
            v.to_bits().hash(&mut hasher);
        }
        format!("{:?}{:?}{}", body.extent, body.bed, body.interactive).hash(&mut hasher);

        let depth = typical_depth(body, center, &state.ground);
        let w = wind.sample(center + DVec3::Y * 10.0);
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
        b.wind = w;
        let t = 1.0 - (-dt / body.waves.response_s.max(0.05)).exp();
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

/// Where the ripple grid goes: on the focus when it is over interactive water, else over the
/// nearest such water within 150 m (so a camera on the shore sees wakes on the lake).
fn ripple_center(state: &WaterState, focus: DVec3, half: f64) -> DVec2 {
    let p = DVec2::new(focus.x, focus.z);
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
        let (bodies, ground) = (&state.bodies, &state.ground);
        let speed = settings.ripple_speed;
        let level_y = f.y;
        state.ripples.rebuild_speeds(speed, |p| {
            let p3 = DVec3::new(p.x, level_y, p.y);
            bodies
                .iter()
                .filter(|b| b.body.interactive)
                .filter_map(|b| b.depth_at(p3, ground))
                .fold(None, |acc: Option<f32>, d| {
                    Some(acc.map_or(d, |a| a.max(d)))
                })
        });
    }
    state
        .ripples
        .step(dt, settings.ripple_damping, settings.foam_life_s);
}

/// Bed heights for a new flow region (relative to its origin).
fn flow_bed(region: &FlowRegion, origin: DVec3, ground: &Ground) -> Vec<f32> {
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
                    let p = origin
                        + DVec3::new(
                            (i as f64 + 0.5) * region.cell_m as f64,
                            1000.0,
                            (j as f64 + 0.5) * region.cell_m as f64,
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

/// Create, update and advance flowing-water regions.
pub fn flow_system(
    time: Res<Time>,
    shared: Res<SharedWater>,
    regions: Query<(Entity, &GlobalTransform, &FlowRegion)>,
    book: Res<SimBook>,
    mut last_layout: Local<u64>,
) {
    let dt = time.delta_secs();
    let mut state = shared.0.write();
    let state = &mut *state;
    let relayout = *last_layout != book.layout;
    *last_layout = book.layout;
    let mut seen = Vec::new();
    for (entity, gt, region) in &regions {
        seen.push(entity);
        let origin = gt.translation();
        let idx = match state.flows.iter().position(|f| f.entity == entity) {
            Some(i) => i,
            None => {
                let bed = flow_bed(region, origin, &state.ground);
                let mut sim = FlowSim::new(entity, origin, region, bed);
                set_reservoirs(&mut sim, &state.bodies, &state.ground);
                // Let the stream establish itself before anyone looks.
                let steps = (region.prewarm_s.max(0.0) * 30.0) as u32;
                for _ in 0..steps {
                    sim.step(1.0 / 30.0);
                }
                sim.drained_m3 = 0.0;
                state.flows.push(sim);
                state.flows.len() - 1
            }
        };
        let sim = &mut state.flows[idx];
        sim.sync(region);
        if (sim.origin - origin).length_squared() > 1e-6 {
            sim.origin = origin;
        }
        if relayout {
            set_reservoirs(sim, &state.bodies, &state.ground);
        }
        sim.step(dt);
    }
    state.flows.retain(|f| seen.contains(&f.entity));
}

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
