//! Running the ecology on live entities: regions build their soil and play out their history
//! when they first run, then step every few simulated weeks; plants grow smoothly in between.

use std::collections::HashMap;
use std::f32::consts::{FRAC_PI_2, PI, TAU};
use std::sync::{Arc, Mutex};

use engine_core::prelude::*;
use engine_core::tasks::AsyncComputeTaskPool;
use engine_world::{
    BurnState, Combustible, FocusPoints, Ground, HeatField, SimulationRange, SurfaceLayers,
    SurfaceMaterials, Water, WeatherState, WindField, WorldShift,
};

use crate::loading::FoliageLoading;
use crate::plant::{
    Disturbance, EcologyRegion, EcologySettings, FelledLog, Genes, Plant, PlantStage,
    ResourceSource,
};
use crate::sim::{
    EnvSample, FlatEnv, RegionSim, SimLog, SimPlant, SimSource, Soil, StepContext, StepOutput,
    grow_plant,
};
use crate::species::{Layer, PlantLibrary, PlantSpecies, SpeciesId};

/// Which region a plant belongs to (`None` = it grows on its own, without spreading).
#[derive(Component, Debug, Clone, Copy, PartialEq)]
pub struct InRegion(pub Option<Entity>);

/// Marks a resource source already counted in the soil of every grown region.
#[derive(Component, Debug, Clone, Copy)]
pub struct SourceApplied;

/// Live simulation state of a region.
#[derive(Component)]
pub struct RegionState {
    pub sim: RegionSim,
    /// Simulated years waiting for the next step.
    pub accum_years: f32,
    /// Years missed while the player was far away.
    pub pending_years: f32,
    pub rain_species: Vec<SpeciesId>,
    pub center: DVec3,
    pub stats: RegionStats,
    /// A catch-up under way, one coarse step a frame: steps left and years each.
    pub catch_up: Option<(u32, f32)>,
}

/// Counters from the last step, for `foliage.stats` and diagnostics.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct RegionStats {
    pub alive: [u32; 3],
    pub births: u32,
    pub deaths: u32,
    pub seeds_tried: u32,
    pub years: f32,
    pub step_ms: f32,
}

/// The `engine_world` environment, read through the [`EnvSample`] trait.
pub struct EnvView<'a> {
    pub weather: Option<&'a WeatherState>,
    pub surface: Option<&'a SurfaceLayers>,
    pub heat: Option<&'a HeatField>,
    pub wind: Option<&'a WindField>,
    pub ground: Option<&'a Ground>,
    pub water: Option<&'a Water>,
    pub materials: Option<&'a SurfaceMaterials>,
}

impl<'a> EnvView<'a> {
    pub fn from_world(world: &'a World) -> Self {
        Self {
            weather: world.get_resource(),
            surface: world.get_resource(),
            heat: world.get_resource(),
            wind: world.get_resource(),
            ground: world.get_resource(),
            water: world.get_resource(),
            materials: world.get_resource(),
        }
    }
}

impl EnvSample for EnvView<'_> {
    fn temperature_c(&self, p: DVec3) -> f32 {
        let air = self
            .weather
            .map(|w| w.temperature_at_altitude(p.y))
            .unwrap_or(15.0);
        air + self.heat.map(|h| h.sample(p)).unwrap_or(0.0)
    }
    fn wetness(&self, p: DVec3) -> f32 {
        self.surface
            .filter(|s| s.grid.contains(p))
            .map(|s| s.sample(p).wetness)
            .unwrap_or(0.0)
    }
    fn wind(&self, p: DVec3) -> Vec3 {
        self.wind
            .map(|w| w.sample(p))
            .or_else(|| self.weather.map(|w| w.wind))
            .unwrap_or(Vec3::ZERO)
    }
    fn ground(&self, p: DVec3) -> Option<(f64, f32)> {
        let Some(ground) = self.ground else {
            return FlatEnv::default().ground(p);
        };
        let g = ground.sample(p)?;
        let holds = self
            .materials
            .map(|m| m.get(g.surface).absorbency)
            .unwrap_or(0.6);
        Some((g.height, holds))
    }
    fn submerged(&self, p: DVec3) -> bool {
        let Some(water) = self.water else {
            return false;
        };
        let ground = self.ground.and_then(|g| g.height(p)).unwrap_or(p.y);
        water
            .sample(DVec3::new(p.x, ground, p.z))
            .is_some_and(|w| w.surface_height > ground + 0.05)
    }
}

fn region_contains(region: &EcologyRegion, center: DVec3, p: DVec3) -> bool {
    (p.x - center.x).abs() <= region.half_extents[0] as f64
        && (p.z - center.z).abs() <= region.half_extents[1] as f64
}

fn rain_species(region: &EcologyRegion, library: &PlantLibrary) -> Vec<SpeciesId> {
    let mut ids = Vec::new();
    for name in &region.species {
        match library.id(name) {
            Some(id) => ids.push(id),
            None => tracing::warn!("foliage: region lists unknown species \"{name}\""),
        }
    }
    ids
}

/// The environment as owned copies, for building regions on worker threads.
#[derive(Default)]
struct EnvSnapshot {
    weather: Option<WeatherState>,
    surface: Option<SurfaceLayers>,
    heat: Option<HeatField>,
    wind: Option<WindField>,
    ground: Option<Ground>,
    water: Option<Water>,
    materials: Option<SurfaceMaterials>,
}

impl EnvSnapshot {
    fn of(world: &World) -> Self {
        Self {
            weather: world.get_resource().cloned(),
            surface: world.get_resource().cloned(),
            heat: world.get_resource().cloned(),
            wind: world.get_resource().cloned(),
            ground: world.get_resource().cloned(),
            water: world.get_resource().cloned(),
            materials: world.get_resource().cloned(),
        }
    }

    fn view(&self) -> EnvView<'_> {
        EnvView {
            weather: self.weather.as_ref(),
            surface: self.surface.as_ref(),
            heat: self.heat.as_ref(),
            wind: self.wind.as_ref(),
            ground: self.ground.as_ref(),
            water: self.water.as_ref(),
            materials: self.materials.as_ref(),
        }
    }
}

/// A region's soil and history, worked out.
struct BuiltRegion {
    state: RegionState,
    plants: Vec<SimPlant>,
    logs: Vec<SimLog>,
    /// How many plants and logs it grew.
    grew: (usize, usize),
    /// How long working it out took.
    ms: f32,
}

/// On a region whose soil and history are being worked out off the frame, then planted a
/// slice a frame.
#[derive(Component)]
pub struct RegionBuild {
    slot: Arc<Mutex<Option<BuiltRegion>>>,
    planting: Option<BuiltRegion>,
    /// World shifts since the build began, applied when it is done.
    shift: Option<WorldShift>,
}

impl RegionBuild {
    /// Follow a world shift: what is being planted moves now, a build still running when it
    /// is done.
    pub(crate) fn follow(&mut self, s: &WorldShift) {
        match &mut self.planting {
            Some(built) => built.follow(s),
            None => self.shift = Some(self.shift.map_or(*s, |a| a.then(s))),
        }
    }
}

impl BuiltRegion {
    fn follow(&mut self, s: &WorldShift) {
        let center = self.state.center;
        self.state.sim.shift(s, center);
        self.state.center = s.point(center);
        for p in &mut self.plants {
            p.pos = s.point(p.pos);
        }
        // Yaw is measured from +X toward +Z, the opposite sense to a turn about +Y.
        let yaw = s.yaw();
        for l in &mut self.logs {
            l.pos = s.point(l.pos);
            l.yaw = (l.yaw - yaw).rem_euclid(TAU);
        }
    }
}

/// Work out a region's soil and history.
fn build_region(
    region: &EcologyRegion,
    center: DVec3,
    sources: &[SimSource],
    library: &PlantLibrary,
    env: &EnvView,
) -> BuiltRegion {
    let start = std::time::Instant::now();
    let soil = Soil::build(region, center, sources, env);
    let mut sim = RegionSim::new(soil, region.seed);
    let rain = rain_species(region, library);
    let (plants, logs) = play_history(&mut sim, region, center, library, &rain, env);
    BuiltRegion {
        state: RegionState {
            sim,
            accum_years: 0.0,
            pending_years: 0.0,
            rain_species: rain,
            center,
            stats: RegionStats::default(),
            catch_up: None,
        },
        grew: (plants.len(), logs.len()),
        plants,
        logs,
        ms: start.elapsed().as_secs_f32() * 1000.0,
    }
}

/// Build regions that haven't run yet: soil, then their history (prewarm), then spawn the
/// plants and logs that history left behind. With [`FoliageLoading::background`] the soil and
/// history are worked out on the async task pool and the plants spawned within
/// [`FoliageLoading::budget_ms`] a frame; the region gets its `RegionState` once all are in.
pub fn init_regions(world: &mut World) {
    let loading = world
        .get_resource::<FoliageLoading>()
        .cloned()
        .unwrap_or_else(FoliageLoading::in_frame);
    let mut q = world.query_filtered::<(Entity, &EcologyRegion, &Transform), (
        Without<RegionState>,
        Without<RegionBuild>,
    )>();
    let todo: Vec<(Entity, EcologyRegion, DVec3)> = q
        .iter(world)
        .map(|(e, r, t)| (e, r.clone(), t.translation))
        .collect();
    if !todo.is_empty() {
        start_regions(world, todo, &loading);
    }
    plant_regions(world, &loading);
}

fn start_regions(
    world: &mut World,
    todo: Vec<(Entity, EcologyRegion, DVec3)>,
    loading: &FoliageLoading,
) {
    let mut sq = world.query::<(Entity, &ResourceSource, &Transform)>();
    let source_ids: Vec<Entity> = sq.iter(world).map(|(e, _, _)| e).collect();
    for e in source_ids {
        world.entity_mut(e).insert(SourceApplied);
    }
    let all_sources: Vec<SimSource> = sq
        .iter(world)
        .map(|(_, s, t)| SimSource {
            pos: t.translation,
            radius: s.radius_m,
            water: s.water,
            nutrients: s.nutrients,
            minerals: s.minerals,
        })
        .collect();
    let library = Arc::new(
        world
            .get_resource::<PlantLibrary>()
            .cloned()
            .unwrap_or_default(),
    );
    let pool = AsyncComputeTaskPool::try_get().filter(|_| loading.background);
    let env = Arc::new(match pool {
        Some(_) => EnvSnapshot::of(world),
        None => EnvSnapshot::default(),
    });
    for (entity, region, center) in todo {
        let sources: Vec<SimSource> = all_sources
            .iter()
            .copied()
            .filter(|s| {
                (s.pos.x - center.x).abs() <= (region.half_extents[0] + s.radius) as f64
                    && (s.pos.z - center.z).abs() <= (region.half_extents[1] + s.radius) as f64
            })
            .collect();
        let slot = Arc::new(Mutex::new(None));
        match pool {
            Some(pool) => {
                let (out, library, env) = (slot.clone(), library.clone(), env.clone());
                pool.spawn(async move {
                    let built = build_region(&region, center, &sources, &library, &env.view());
                    if let Ok(mut s) = out.lock() {
                        *s = Some(built);
                    }
                })
                .detach();
            }
            None => {
                let built = build_region(
                    &region,
                    center,
                    &sources,
                    &library,
                    &EnvView::from_world(world),
                );
                if let Ok(mut s) = slot.lock() {
                    *s = Some(built);
                }
            }
        }
        world.entity_mut(entity).insert(RegionBuild {
            slot,
            planting: None,
            shift: None,
        });
    }
}

/// Spawn the plants and logs of the regions worked out, within the frame's budget.
fn plant_regions(world: &mut World, loading: &FoliageLoading) {
    let mut q = world.query::<(Entity, &mut RegionBuild)>();
    let building: Vec<Entity> = q.iter(world).map(|(e, _)| e).collect();
    let started = std::time::Instant::now();
    let budget = loading.frame_budget_ms(world.get_resource::<engine_world::LoadingBudget>());
    let over = || started.elapsed().as_secs_f32() * 1000.0 >= budget;
    for entity in building {
        let Ok((_, mut build)) = q.get_mut(world, entity) else {
            continue;
        };
        if build.planting.is_none() {
            let shift = build.shift.take();
            // Reversed, so planting pops them in their order.
            build.planting = build
                .slot
                .lock()
                .ok()
                .and_then(|mut s| s.take())
                .map(|mut b| {
                    if let Some(s) = &shift {
                        b.follow(s);
                    }
                    b.plants.reverse();
                    b.logs.reverse();
                    b
                });
            if build.planting.is_none() {
                build.shift = shift;
            }
        }
        let Some(mut built) = build.planting.take() else {
            continue;
        };
        loop {
            if !built.plants.is_empty() {
                for _ in 0..64 {
                    let Some(p) = built.plants.pop() else { break };
                    spawn_sim_plant(world, p, Some(entity));
                }
            } else if let Some(log) = built.logs.pop() {
                let l = spawn_log(world, &log);
                world.entity_mut(l).insert(InRegion(Some(entity)));
            } else {
                break;
            }
            if over() {
                break;
            }
        }
        if !built.plants.is_empty() || !built.logs.is_empty() {
            if let Ok((_, mut build)) = q.get_mut(world, entity) {
                build.planting = Some(built);
            }
            continue;
        }
        tracing::info!(
            "foliage: region {entity} grew {} plants and {} logs in {:.0} ms",
            built.grew.0,
            built.grew.1,
            built.ms
        );
        world
            .entity_mut(entity)
            .remove::<RegionBuild>()
            .insert(built.state);
    }
    let left = q.iter(world).count();
    if let Some(mut l) = world.get_resource_mut::<FoliageLoading>() {
        l.bypass_change_detection().regions_building = left;
    }
}

/// Simulate a region's history in coarse steps. Returns the surviving plants and logs.
pub fn play_history(
    sim: &mut RegionSim,
    region: &EcologyRegion,
    center: DVec3,
    library: &PlantLibrary,
    rain: &[SpeciesId],
    env: &dyn EnvSample,
) -> (Vec<SimPlant>, Vec<SimLog>) {
    let h = &region.history;
    let mut plants: Vec<SimPlant> = Vec::new();
    let mut logs: Vec<SimLog> = Vec::new();
    // Head start: random seedlings of each layer's species.
    for layer in Layer::ALL {
        let ids: Vec<SpeciesId> = rain
            .iter()
            .copied()
            .filter(|&id| library.get(id).layer == layer)
            .collect();
        if ids.is_empty() {
            continue;
        }
        for _ in 0..h.initial_plants[layer.index()] {
            let id = *sim.rng.pick(&ids).unwrap();
            let p = center
                + DVec3::new(
                    sim.rng.range(-1.0, 1.0) as f64 * region.half_extents[0] as f64,
                    0.0,
                    sim.rng.range(-1.0, 1.0) as f64 * region.half_extents[1] as f64,
                );
            if sim.soil.blocked[sim.soil.index(p)]
                || !region.may_root(layer, DVec2::new(p.x - center.x, p.z - center.z))
            {
                continue;
            }
            let y = env
                .ground(p + DVec3::Y * 2.0)
                .map(|g| g.0)
                .unwrap_or(center.y);
            let genes = Genes::random(&mut sim.rng);
            let mut plant = Plant::new(id, sim.rng.range(0.03, 0.25), genes);
            plant.age_years = plant.size * library.get(id).years_to_mature;
            plants.push(SimPlant::new(DVec3::new(p.x, y, p.z), plant));
        }
    }
    let ctx = StepContext {
        library,
        region,
        center,
        rain_species: rain,
        env,
    };
    // Coarse steps: a year at a time for the long growth, half years after the disturbance.
    let run = |sim: &mut RegionSim,
               plants: &mut Vec<SimPlant>,
               logs: &mut Vec<SimLog>,
               years: f32,
               step: f32| {
        let mut t = 0.0;
        while t < years {
            let dt = step.min(years - t);
            let mut out = StepOutput::default();
            sim.step(&ctx, plants, logs, dt, true, &mut out);
            plants.retain(|p| !p.removed);
            plants.extend(out.births);
            t += dt;
        }
    };
    run(
        sim,
        &mut plants,
        &mut logs,
        h.grow_years,
        region.step_years.max(1.0),
    );
    disturb(sim, &h.disturbance, center, library, &mut plants, &mut logs);
    run(
        sim,
        &mut plants,
        &mut logs,
        h.years_since,
        region.step_years.max(0.5),
    );
    fit_logs(region, center, library, &plants, &[], &mut logs);
    (plants, logs)
}

/// Paths, shores and plots stay passable, and no log runs through a standing trunk or another
/// log: one that came down across any of them was dragged around to lie beside it, or hauled
/// away (or long since rotted) when it fits nowhere. Keeps the `logs` that fit, turned to
/// where they fit, clear of `lying` logs already on the ground.
pub fn fit_logs(
    region: &EcologyRegion,
    center: DVec3,
    library: &PlantLibrary,
    plants: &[SimPlant],
    lying: &[SimLog],
    logs: &mut Vec<SimLog>,
) {
    let stems: Vec<(Vec2, f32)> = plants
        .iter()
        .filter(|p| !p.removed && !matches!(p.plant.stage, PlantStage::Stump { .. }))
        .filter_map(|p| {
            let sp = library.get(p.plant.species);
            let local = Vec2::new((p.pos.x - center.x) as f32, (p.pos.z - center.z) as f32);
            (sp.is_woody() && p.plant.size > 0.1)
                .then(|| (local, (sp.trunk_radius_m * p.plant.size).max(0.05)))
        })
        .collect();
    let mut lying: Vec<LogSpan> = lying
        .iter()
        .map(|l| LogSpan::of(center, l, library))
        .collect();
    logs.retain_mut(|l| {
        let yaw = l.yaw;
        let fits = [
            0.0,
            PI,
            0.5,
            -0.5,
            PI + 0.5,
            PI - 0.5,
            FRAC_PI_2,
            -FRAC_PI_2,
        ]
        .into_iter()
        .any(|turn| {
            l.yaw = (yaw + turn).rem_euclid(TAU);
            let span = LogSpan::of(center, l, library);
            span.within(region)
                && !blocks_keep_clear(region, center, l, library)
                && !stems
                    .iter()
                    .any(|&(p, r)| span.distance_to(p) < r + span.radius)
                && !lying
                    .iter()
                    .any(|o| span.distance_to_span(o) < span.radius + o.radius)
        });
        if fits {
            lying.push(LogSpan::of(center, l, library));
        }
        fits
    });
}

/// Whether a plant the simulation is done with comes down whole in live play (a big snag),
/// rather than crumbling away.
pub fn topples(plant: &Plant, sp: &PlantSpecies) -> bool {
    matches!(plant.stage, PlantStage::Dead { .. }) && sp.is_tree() && plant.size > 0.5
}

/// Which way a snag comes down: any way, but away from anyone standing within its reach.
fn topple_direction(focus: &FocusPoints, pos: DVec3, height: f32, e: Entity) -> Vec3 {
    let r = engine_core::math::hash01(e.to_bits() as i64, 7, 0, 0x5a6);
    let near = focus
        .points
        .iter()
        .map(|f| (pos - *f).with_y(0.0))
        .filter(|away| away.length() > 0.1 && away.length() < height as f64 * 1.5)
        .min_by(|a, b| a.length().total_cmp(&b.length()));
    let yaw = match near {
        Some(away) => away.z.atan2(away.x) as f32 + (r - 0.5) * 1.2,
        None => r * TAU,
    };
    Vec3::new(yaw.cos(), 0.0, yaw.sin())
}

/// Whether a log lies across one of the region's keep-clear circles.
fn blocks_keep_clear(
    region: &EcologyRegion,
    center: DVec3,
    log: &SimLog,
    library: &PlantLibrary,
) -> bool {
    let span = LogSpan::of(center, log, library);
    region
        .keep_clear
        .iter()
        .any(|c| span.distance_to(Vec2::new(c[0], c[1])) < c[2] + span.radius)
}

/// A log's footprint on the ground: butt to tip in region-local X/Z, and its thickness.
struct LogSpan {
    a: Vec2,
    b: Vec2,
    radius: f32,
}

impl LogSpan {
    fn of(center: DVec3, log: &SimLog, library: &PlantLibrary) -> Self {
        let sp = library.get(log.species);
        // Bare old logs have lost their tops (`meshgen::tree`).
        let reach = if log.years >= crate::felling::LOG_BARE_YEARS {
            0.4
        } else {
            0.85
        };
        let length = sp.max_height_m * log.size * reach;
        let a = Vec2::new((log.pos.x - center.x) as f32, (log.pos.z - center.z) as f32);
        Self {
            a,
            b: a + Vec2::new(log.yaw.cos(), log.yaw.sin()) * length,
            radius: (sp.trunk_radius_m * log.size).max(0.1),
        }
    }

    /// Whether the tip stays inside the region (the butt always is, where the tree stood).
    fn within(&self, region: &EcologyRegion) -> bool {
        let [hx, hz] = region.half_extents.map(|h| h.max(0.1));
        if region.round {
            (self.b.x / hx).powi(2) + (self.b.y / hz).powi(2) < 0.9
        } else {
            self.b.x.abs() < hx - 1.0 && self.b.y.abs() < hz - 1.0
        }
    }

    fn distance_to(&self, p: Vec2) -> f32 {
        let ab = self.b - self.a;
        let t = ((p - self.a).dot(ab) / ab.length_squared().max(1e-6)).clamp(0.0, 1.0);
        p.distance(self.a + ab * t)
    }

    fn distance_to_span(&self, o: &LogSpan) -> f32 {
        let cross = |u: Vec2, v: Vec2| u.x * v.y - u.y * v.x;
        let (d1, d2) = (self.b - self.a, o.b - o.a);
        let denom = cross(d1, d2);
        if denom.abs() > 1e-6 {
            let t = cross(o.a - self.a, d2) / denom;
            let u = cross(o.a - self.a, d1) / denom;
            if (0.0..=1.0).contains(&t) && (0.0..=1.0).contains(&u) {
                return 0.0;
            }
        }
        self.distance_to(o.a)
            .min(self.distance_to(o.b))
            .min(o.distance_to(self.a))
            .min(o.distance_to(self.b))
    }
}

fn disturb(
    sim: &mut RegionSim,
    d: &Disturbance,
    center: DVec3,
    library: &PlantLibrary,
    plants: &mut [SimPlant],
    logs: &mut Vec<SimLog>,
) {
    let within = |p: DVec3, c: [f32; 2], r: f32| {
        DVec2::new(p.x - center.x - c[0] as f64, p.z - center.z - c[1] as f64).length() < r as f64
    };
    match d {
        Disturbance::None => {}
        Disturbance::Many(list) => {
            for d in list {
                disturb(sim, d, center, library, plants, logs);
            }
        }
        Disturbance::ClearCut { center: c, radius } => {
            // Loggers fell mostly one way and haul most trunks away; trees leaning or hung
            // up on their neighbors go down wherever they can, so the slash lies crossed.
            let felling_yaw = sim.rng.range(0.0, TAU);
            for p in plants.iter_mut() {
                if !within(p.pos, *c, *radius) {
                    continue;
                }
                let sp = library.get(p.plant.species);
                if sp.is_tree() && p.plant.stage.is_alive() && p.plant.size > 0.25 {
                    p.plant.stage = PlantStage::Stump { years: 0.0 };
                    if sim.rng.chance(0.55) {
                        continue;
                    }
                    let yaw = if sim.rng.chance(0.35) {
                        sim.rng.range(0.0, TAU)
                    } else {
                        felling_yaw + sim.rng.range(-0.6, 0.6)
                    };
                    logs.push(SimLog {
                        entity: None,
                        pos: p.pos,
                        yaw,
                        species: p.plant.species,
                        size: p.plant.size,
                        years: 0.0,
                        rot_years: 8.0 + 10.0 * p.plant.size,
                        genes: p.plant.genes,
                    });
                } else if sim.rng.chance(0.7) {
                    // Crushed by the logging.
                    p.removed = true;
                }
            }
        }
        Disturbance::Burn { center: c, radius } => {
            for p in plants.iter_mut() {
                if !within(p.pos, *c, *radius) || p.removed {
                    continue;
                }
                let sp = library.get(p.plant.species);
                if sp.is_woody() && p.plant.size > 0.2 {
                    p.plant.stage = PlantStage::Burnt { years: 0.0 };
                } else {
                    p.removed = true;
                }
                sim.soil.enrich(p.pos, 1.5, 0.12);
            }
        }
    }
}

/// Spawn a plant from simulation data.
pub fn spawn_sim_plant(world: &mut World, p: SimPlant, region: Option<Entity>) -> Entity {
    let of = |sp: &PlantSpecies| (sp.fuel_kg, sp.ignition_c, sp.is_woody());
    let (sp_fuel, ignition, woody) = match world.get_resource::<PlantLibrary>() {
        Some(l) => of(l.get(p.plant.species)),
        None => of(&PlantSpecies::default()),
    };
    let combustible = Combustible {
        fuel_kg: sp_fuel * p.plant.size * p.plant.size,
        moisture: 0.3,
        ignition_c: ignition,
        temperature_c: 15.0,
        state: match p.plant.stage {
            PlantStage::Burnt { .. } => BurnState::Burnt,
            _ => BurnState::Intact,
        },
    };
    let mut e = world.spawn((
        Transform::from_translation(p.pos),
        p.plant,
        InRegion(region),
    ));
    // Trees and bushes burn as objects; grass, flowers and ferns burn with the ground fire
    // under them (see `scorch_small_plants`).
    if woody {
        e.insert(combustible);
    }
    e.id()
}

/// Ground fires (`HeatField`) burn the small plants they sweep over.
pub fn scorch_small_plants(
    time: Res<Time>,
    mut timer: Local<f32>,
    heat: Option<Res<HeatField>>,
    library: Res<PlantLibrary>,
    mut plants: Query<(&mut Plant, &Transform), Without<Combustible>>,
) {
    let Some(heat) = heat else { return };
    *timer -= time.delta_secs();
    if *timer > 0.0 {
        return;
    }
    *timer = 0.25;
    plants.par_iter_mut().for_each(|(mut plant, t)| {
        if !plant.stage.is_alive() || library.get(plant.species).is_woody() {
            return;
        }
        if heat.sample(t.translation + DVec3::Y * 0.2) > 250.0 {
            plant.stage = PlantStage::Burnt { years: 0.0 };
        }
    });
}

/// When a region goes (its zone unloaded), its plants and logs go with it, so a zone loaded
/// again doesn't find the last visit's forest still standing beside the new one.
pub fn drop_unloaded_regions(
    mut commands: Commands,
    mut removed: RemovedComponents<EcologyRegion>,
    regions: Query<(), With<EcologyRegion>>,
    owned: Query<(Entity, &InRegion)>,
) {
    let gone: Vec<Entity> = removed.read().filter(|e| !regions.contains(*e)).collect();
    if gone.is_empty() {
        return;
    }
    for (e, r) in &owned {
        if r.0.is_some_and(|r| gone.contains(&r)) {
            commands.entity(e).despawn();
        }
    }
}

/// Spawn a felled log lying on the ground (from a region's history).
pub fn spawn_log(world: &mut World, log: &SimLog) -> Entity {
    world
        .spawn((
            Transform::from_translation(log.pos),
            FelledLog {
                species: log.species,
                years: log.years,
                rot_years: log.rot_years,
                size: log.size,
            },
            crate::felling::LogPose {
                yaw: log.yaw,
                genes: log.genes,
                charred: false,
            },
        ))
        .id()
}

/// Put plants that don't know their region into the one containing them.
#[allow(clippy::type_complexity)]
pub fn assign_regions(
    mut commands: Commands,
    plants: Query<(Entity, &Transform), (With<Plant>, Without<InRegion>)>,
    regions: Query<(Entity, &EcologyRegion, &Transform)>,
) {
    for (e, t) in &plants {
        let region = regions
            .iter()
            .find(|(_, r, rt)| region_contains(r, rt.translation, t.translation))
            .map(|(re, _, _)| re);
        commands.entity(e).insert(InRegion(region));
    }
}

/// Simulated years accumulated for smooth growth.
#[derive(Resource, Default)]
pub struct GrowthClock {
    years: f32,
    secs: f32,
    /// Years gathered for regions out of the simulation range, which grow only every
    /// [`RESTING_TICKS`]th tick.
    resting_years: f32,
    ticks: u32,
}

/// Regions beyond the simulation range grow once in this many growth ticks (every 4 s).
const RESTING_TICKS: u32 = 16;

/// Whether a region lies wholly beyond the simulation range of every focus.
fn region_resting(
    range: Option<&SimulationRange>,
    focus: &FocusPoints,
    region: &EcologyRegion,
    center: DVec3,
) -> bool {
    let Some(range) = range else { return false };
    let radius = region.half_extents[0].max(region.half_extents[1]) as f64;
    !focus.points.is_empty()
        && focus.distance(center) - radius > range.active_m * (1.0 + range.hysteresis)
}

/// Grow every living plant a little (a few times per second), at its current vigor.
#[allow(clippy::too_many_arguments)]
pub fn grow_plants(
    time: Res<Time>,
    settings: Res<EcologySettings>,
    library: Res<PlantLibrary>,
    range: Option<Res<SimulationRange>>,
    focus: Option<Res<FocusPoints>>,
    mut clock: ResMut<GrowthClock>,
    regions: Query<(&EcologyRegion, &RegionState)>,
    mut plants: Query<(&mut Plant, Option<&InRegion>)>,
) {
    if settings.paused {
        return;
    }
    clock.years += time.delta_secs() * settings.years_per_second;
    clock.secs += time.delta_secs();
    if clock.secs < 0.25 {
        return;
    }
    let dt = std::mem::take(&mut clock.years);
    clock.secs = 0.0;
    if dt <= 0.0 {
        return;
    }
    // Regions out of range rest: their plants grow rarely, by all the years gathered meanwhile.
    clock.resting_years += dt;
    clock.ticks = clock.ticks.wrapping_add(1);
    let resting_turn = clock.ticks.is_multiple_of(RESTING_TICKS);
    let resting_dt = if resting_turn {
        std::mem::take(&mut clock.resting_years)
    } else {
        0.0
    };
    let focus = focus.as_deref().cloned().unwrap_or_default();
    let range = range.as_deref();
    plants.par_iter_mut().for_each(|(mut plant, region)| {
        if !plant.stage.is_alive() {
            return;
        }
        let region = region.and_then(|r| r.0).and_then(|r| regions.get(r).ok());
        let resting = region.is_some_and(|(r, s)| region_resting(range, &focus, r, s.center));
        if resting && !resting_turn {
            return;
        }
        let years = if resting { resting_dt } else { dt };
        let scale = region.map(|(r, _)| r.time_scale.max(0.0)).unwrap_or(1.0);
        let sp = library.get(plant.species);
        let mut next = plant.clone();
        grow_plant(&mut next, sp, years * scale);
        // Only mark visible changes (the visuals re-scale changed plants).
        if (next.size - plant.size).abs() > 0.002 {
            *plant = next;
        } else {
            *plant.bypass_change_detection() = next;
        }
    });
}

/// Step every region that has simulated enough time since its last step.
pub fn ecology_step(world: &mut World) {
    let dt = world.resource::<Time>().delta_secs();
    let settings = world
        .get_resource::<EcologySettings>()
        .cloned()
        .unwrap_or_default();
    if settings.paused || dt <= 0.0 {
        return;
    }
    let focus = world
        .get_resource::<FocusPoints>()
        .cloned()
        .unwrap_or_default();
    let range = world.get_resource::<SimulationRange>().cloned();
    let years = dt * settings.years_per_second;

    // Which regions are due, and with how many years.
    let mut due: Vec<(Entity, f32)> = Vec::new();
    let mut q = world.query::<(Entity, &EcologyRegion, &mut RegionState)>();
    for (e, region, mut state) in q.iter_mut(world) {
        let near = (focus.points.is_empty()
            || focus.distance(state.center)
                < (region.active_radius_m + region.half_extents[0].max(region.half_extents[1]))
                    as f64)
            && !region_resting(range.as_ref(), &focus, region, state.center);
        if !near {
            state.pending_years = (state.pending_years + years * region.time_scale.max(0.0))
                .min(settings.max_catch_up_years);
            continue;
        }
        let years = years * region.time_scale.max(0.0);
        state.accum_years += years + std::mem::take(&mut state.pending_years);
        if state.catch_up.is_some() {
            // Its next coarse step (the years since keep gathering for after).
            due.push((e, 0.0));
        } else if state.accum_years >= region.step_years {
            let take = state.accum_years;
            state.accum_years = 0.0;
            due.push((e, take));
        }
    }
    if due.is_empty() {
        return;
    }

    let library = world
        .get_resource::<PlantLibrary>()
        .cloned()
        .unwrap_or_default();
    // Gather plants and logs per due region.
    let mut groups: HashMap<Entity, (Vec<SimPlant>, Vec<SimLog>)> = due
        .iter()
        .map(|(e, _)| (*e, (Vec::new(), Vec::new())))
        .collect();
    let mut pq = world.query::<(Entity, &Plant, &Transform, &InRegion)>();
    for (e, plant, t, region) in pq.iter(world) {
        if let Some(g) = region.0.and_then(|r| groups.get_mut(&r)) {
            g.0.push(SimPlant {
                entity: Some(e),
                pos: t.translation,
                plant: plant.clone(),
                changed: false,
                removed: false,
            });
        }
    }
    let mut lq = world.query::<(Entity, &FelledLog, &Transform, &crate::felling::LogPose)>();
    let region_bounds: Vec<(Entity, EcologyRegion, DVec3)> = due
        .iter()
        .filter_map(|(e, _)| {
            let r = world.get::<EcologyRegion>(*e)?.clone();
            let c = world.get::<RegionState>(*e)?.center;
            Some((*e, r, c))
        })
        .collect();
    for (e, log, t, pose) in lq.iter(world) {
        if let Some((re, _, _)) = region_bounds
            .iter()
            .find(|(_, r, c)| region_contains(r, *c, t.translation))
            && let Some(g) = groups.get_mut(re)
        {
            g.1.push(SimLog {
                entity: Some(e),
                pos: t.translation,
                yaw: pose.yaw,
                species: log.species,
                size: log.size,
                years: log.years,
                rot_years: log.rot_years,
                genes: pose.genes,
            });
        }
    }

    for (region_entity, years) in due {
        let Some(mut state) = world.entity_mut(region_entity).take::<RegionState>() else {
            continue;
        };
        let region = world
            .get::<EcologyRegion>(region_entity)
            .cloned()
            .unwrap_or_default();
        let (mut plants, mut logs) = groups.remove(&region_entity).unwrap_or_default();
        let log_ids: Vec<Entity> = logs.iter().filter_map(|l| l.entity).collect();
        let start = std::time::Instant::now();
        let mut out = StepOutput::default();
        // Long catch-ups run in coarse steps, one a frame (all at once they took over 100 ms on
        // coming back to the forest); a step's worth (and the frame that tipped it over) is
        // one step.
        let (dt, catch_up) = match state.catch_up.take() {
            Some((left, dt)) => {
                if left > 1 {
                    state.catch_up = Some((left - 1, dt));
                }
                (dt, true)
            }
            None => {
                let steps = (years / region.step_years - 0.25).ceil().clamp(1.0, 8.0);
                let dt = years / steps;
                if steps > 1.0 {
                    state.catch_up = Some((steps as u32 - 1, dt));
                }
                (dt, steps > 1.0)
            }
        };
        let years = dt;
        {
            let env = EnvView::from_world(world);
            let ctx = StepContext {
                library: &library,
                region: &region,
                center: state.center,
                rain_species: &state.rain_species,
                env: &env,
            };
            state
                .sim
                .step(&ctx, &mut plants, &mut logs, dt, catch_up, &mut out);
        }
        let mut stats = RegionStats {
            years,
            births: out.births.len() as u32,
            seeds_tried: out.seeds_tried,
            step_ms: start.elapsed().as_secs_f32() * 1000.0,
            ..Default::default()
        };
        let mut toppling = Vec::new();
        for p in &plants {
            let Some(e) = p.entity else { continue };
            if p.removed {
                // In live play a big snag comes down whole and its stump stays a while; in a
                // catch-up it came down while nobody was near (see the new logs below).
                let sp = library.get(p.plant.species);
                if !catch_up && topples(&p.plant, sp) {
                    toppling.push((e, p.pos, p.plant.height(sp)));
                } else if let Ok(ent) = world.get_entity_mut(e) {
                    ent.despawn();
                }
                stats.deaths += 1;
                continue;
            }
            if p.plant.stage.is_alive() {
                stats.alive[library.get(p.plant.species).layer.index()] += 1;
            }
            if p.changed
                && let Ok(mut ent) = world.get_entity_mut(e)
            {
                if let Some(mut plant) = ent.get_mut::<Plant>()
                    && *plant != p.plant
                {
                    // Keep size/age written by the growth system since the gather.
                    let (size, age) = (plant.size, plant.age_years);
                    *plant = p.plant.clone();
                    if p.plant.stage.is_alive() && p.plant.size >= size - 1e-3 {
                        plant.size = plant.size.max(size);
                        plant.age_years = plant.age_years.max(age);
                    }
                }
                let sp = library.get(p.plant.species);
                if let Some(mut c) = ent.get_mut::<Combustible>()
                    && c.state == BurnState::Intact
                {
                    c.fuel_kg = sp.fuel_kg * p.plant.size * p.plant.size;
                    c.moisture = (p.plant.health * 0.4 + 0.1).min(0.8);
                }
            }
        }
        // Logs that rotted away.
        for e in log_ids {
            if !logs.iter().any(|l| l.entity == Some(e))
                && let Ok(ent) = world.get_entity_mut(e)
            {
                ent.despawn();
            }
        }
        for l in &logs {
            if let Some(e) = l.entity
                && let Some(mut log) = world.get_mut::<FelledLog>(e)
            {
                log.years = l.years;
            }
        }
        for (e, pos, height) in toppling {
            let dir = topple_direction(&focus, pos, height, e);
            if crate::felling::fell_tree(world, e, dir).is_none()
                && let Ok(ent) = world.get_entity_mut(e)
            {
                ent.despawn();
            }
        }
        if catch_up {
            let (lying, mut fresh): (Vec<SimLog>, Vec<SimLog>) =
                logs.into_iter().partition(|l| l.entity.is_some());
            fit_logs(&region, state.center, &library, &plants, &lying, &mut fresh);
            for l in &fresh {
                let l = spawn_log(world, l);
                world.entity_mut(l).insert(InRegion(Some(region_entity)));
            }
        }
        for b in out.births {
            spawn_sim_plant(world, b, Some(region_entity));
        }
        state.stats = stats;
        let total: u32 = state.stats.alive.iter().sum();
        world.entity_mut(region_entity).insert(state);
        if let Some(mut d) = world.get_resource_mut::<Diagnostics>() {
            d.set("foliage.plants", total as f64);
        }
    }
}

/// Plants react to fire: burnt plants become charred snags (or vanish).
pub fn react_to_fire(mut plants: Query<(&mut Plant, &Combustible), Changed<Combustible>>) {
    for (mut plant, c) in &mut plants {
        if c.state == BurnState::Burnt && !matches!(plant.stage, PlantStage::Burnt { .. }) {
            plant.stage = PlantStage::Burnt { years: 0.0 };
        }
    }
}

/// Feet, hooves and wheels wear down small plants (paths form where creatures walk).
pub fn trample(
    mut contacts: MessageReader<engine_world::GroundContact>,
    mut regions: Query<(&EcologyRegion, &mut RegionState)>,
) {
    for c in contacts.read() {
        for (region, mut state) in &mut regions {
            if region_contains(region, state.center, c.position) {
                state.sim.soil.trample(c.position, c.load_kg);
            }
        }
    }
}

/// Terrain was dug or raised: plants move with the ground, or are uprooted.
pub fn follow_terrain(
    mut commands: Commands,
    mut changes: MessageReader<engine_world::TerrainChanged>,
    ground: Option<Res<Ground>>,
    mut plants: Query<(Entity, &mut Transform, &mut Plant)>,
) {
    let Some(ground) = ground else { return };
    for c in changes.read() {
        for (e, mut t, mut plant) in &mut plants {
            if t.translation.distance(c.center) > c.radius as f64 + 1.0 {
                continue;
            }
            // Small plants lean with the new slope (worked out again when next drawn).
            commands.entity(e).remove::<crate::visuals::PlantBase>();
            plant.set_changed();
            let Some(h) = ground.height(t.translation + DVec3::Y * 2.0) else {
                continue;
            };
            let dy = h - t.translation.y;
            if dy.abs() > 0.6 && plant.stage.is_alive() {
                plant.stage = PlantStage::Dead { years: 0.0 };
            }
            t.translation.y = h;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::plant::{Genes, RegionHistory};
    use std::f32::consts::FRAC_PI_2;

    fn forest() -> EcologyRegion {
        EcologyRegion {
            half_extents: [30.0, 30.0],
            species: ["oak", "birch", "pine", "fern", "grass"]
                .map(String::from)
                .to_vec(),
            history: RegionHistory {
                grow_years: 40.0,
                initial_plants: [30, 0, 60],
                ..Default::default()
            },
            ..Default::default()
        }
    }

    fn grow(
        region: &EcologyRegion,
        sources: &[SimSource],
    ) -> (Vec<SimPlant>, Vec<SimLog>, RegionSim) {
        let library = PlantLibrary::default();
        let env = FlatEnv::default();
        let soil = Soil::build(region, DVec3::ZERO, sources, &env);
        let mut sim = RegionSim::new(soil, region.seed);
        let rain = rain_species(region, &library);
        let (p, l) = play_history(&mut sim, region, DVec3::ZERO, &library, &rain, &env);
        (p, l, sim)
    }

    /// How long a step of the showcase forest takes (`cargo test --release -p engine_foliage
    /// forest_step_timing -- --ignored --nocapture`).
    #[test]
    #[ignore]
    fn forest_step_timing() {
        if std::env::var_os("SEQUENTIAL").is_none() {
            engine_core::tasks::init_task_pools();
        }
        let region = crate::demo::forest_region();
        let t = std::time::Instant::now();
        let (mut plants, mut logs, mut sim) = grow(&region, &[]);
        eprintln!(
            "history: {} plants, {} logs in {:.0} ms",
            plants.len(),
            logs.len(),
            t.elapsed().as_secs_f64() * 1000.0
        );
        let library = PlantLibrary::default();
        let env = FlatEnv::default();
        let rain = rain_species(&region, &library);
        let ctx = StepContext {
            library: &library,
            region: &region,
            center: DVec3::ZERO,
            rain_species: &rain,
            env: &env,
        };
        for _ in 0..5 {
            let mut out = StepOutput::default();
            let t = std::time::Instant::now();
            sim.step(
                &ctx,
                &mut plants,
                &mut logs,
                region.step_years,
                false,
                &mut out,
            );
            eprintln!(
                "step: {:.1} ms, {} plants, {} births",
                t.elapsed().as_secs_f64() * 1000.0,
                plants.len(),
                out.births.len()
            );
            plants.retain(|p| !p.removed);
            plants.extend(out.births);
        }
    }

    fn count(plants: &[SimPlant], lib: &PlantLibrary, name: &str, min_size: f32) -> usize {
        let id = lib.id(name).unwrap();
        plants
            .iter()
            .filter(|p| {
                p.plant.species == id && p.plant.stage.is_alive() && p.plant.size >= min_size
            })
            .count()
    }

    #[test]
    fn logs_across_a_path_are_hauled_away() {
        let lib = PlantLibrary::default();
        let region = EcologyRegion {
            keep_clear: vec![[0.0, 5.0, 1.5]],
            ..forest()
        };
        let center = DVec3::new(100.0, 0.0, 0.0);
        let log = |x: f64, yaw: f32| SimLog {
            entity: None,
            pos: center + DVec3::new(x, 0.0, 0.0),
            yaw,
            species: lib.id("oak").unwrap(),
            size: 0.8,
            years: 0.0,
            rot_years: 10.0,
            genes: Genes::default(),
        };
        // Falling toward +Z, the trunk reaches across the clearing at (0, 5).
        assert!(blocks_keep_clear(
            &region,
            center,
            &log(0.0, FRAC_PI_2),
            &lib
        ));
        // Falling away from it, or beside it, it stays.
        assert!(!blocks_keep_clear(
            &region,
            center,
            &log(0.0, -FRAC_PI_2),
            &lib
        ));
        assert!(!blocks_keep_clear(
            &region,
            center,
            &log(6.0, FRAC_PI_2),
            &lib
        ));
    }

    #[test]
    fn a_log_from_a_catch_up_lies_clear_of_the_logs_already_there() {
        let lib = PlantLibrary::default();
        let region = forest();
        let log = |x: f64, z: f64, yaw: f32| SimLog {
            entity: None,
            pos: DVec3::new(x, 0.0, z),
            yaw,
            species: lib.id("oak").unwrap(),
            size: 0.8,
            years: 0.0,
            rot_years: 10.0,
            genes: Genes::default(),
        };
        // One lies along +X from the origin; the new one came down across it.
        let lying = [log(0.0, 0.0, 0.0)];
        let mut fresh = vec![log(3.0, -4.0, FRAC_PI_2)];
        fit_logs(&region, DVec3::ZERO, &lib, &[], &lying, &mut fresh);
        assert_eq!(fresh.len(), 1, "it fits, turned away from the other");
        let (a, b) = (
            LogSpan::of(DVec3::ZERO, &fresh[0], &lib),
            LogSpan::of(DVec3::ZERO, &lying[0], &lib),
        );
        assert!(a.distance_to_span(&b) >= a.radius + b.radius);
    }

    #[test]
    fn a_region_unloaded_takes_its_plants_and_logs_with_it() {
        let mut app = App::new();
        app.add_plugins(engine_world::WorldPlugin);
        app.add_plugins(crate::FoliagePlugin);
        app.world.insert_resource(crate::FoliageLoading::in_frame());
        app.world.resource_mut::<EcologySettings>().paused = true;
        let region = app.world.spawn((Transform::default(), forest())).id();
        // A plant of its own, not the region's, stays.
        let lone = crate::api::spawn_plant(
            &mut app.world,
            "oak",
            DVec3::new(900.0, 0.0, 0.0),
            0.5,
            None,
        )
        .unwrap();
        app.update();
        app.update();
        let owned = |app: &mut App| {
            let mut q = app.world.query::<&InRegion>();
            q.iter(&app.world).filter(|r| r.0 == Some(region)).count()
        };
        assert!(owned(&mut app) > 100);
        app.world.despawn(region);
        app.update();
        assert_eq!(owned(&mut app), 0, "the region's plants stayed behind");
        assert!(app.world.get_entity(lone).is_ok());
        let mut logs = app.world.query::<&FelledLog>();
        assert_eq!(logs.iter(&app.world).count(), 0, "its logs stayed behind");
    }

    #[test]
    fn a_long_catch_up_runs_one_coarse_step_a_tick() {
        let mut app = App::new();
        app.world
            .resource_mut::<Time>()
            .set_source(engine_core::time::TimeSource::Manual { step_secs: 0.1 });
        app.add_plugins(engine_world::WorldPlugin);
        app.add_plugins(crate::FoliagePlugin);
        app.world.insert_resource(crate::FoliageLoading::in_frame());
        let region = app.world.spawn((Transform::default(), forest())).id();
        let focus = app
            .world
            .spawn((
                Transform::default(),
                engine_world::SimulationFocus::default(),
            ))
            .id();
        app.world.resource_mut::<EcologySettings>().years_per_second = 0.5;
        app.update_n(2);
        // Away for a while: years pile up.
        app.world.get_mut::<Transform>(focus).unwrap().translation = DVec3::new(50_000.0, 0.0, 0.0);
        app.update_n(200);
        let missed = app.world.get::<RegionState>(region).unwrap().pending_years;
        let step = app.world.get::<EcologyRegion>(region).unwrap().step_years;
        assert!(missed > 8.0 * step, "{missed} years missed");
        // Back: the catch-up runs in coarse steps (up to eight), one a fixed tick, so it spreads
        // over frames (two ticks a frame here).
        app.world.get_mut::<Transform>(focus).unwrap().translation = DVec3::ZERO;
        let mut steps = Vec::new();
        for _ in 0..12 {
            app.update();
            let state = app.world.get::<RegionState>(region).unwrap();
            steps.push(state.stats.years);
        }
        let coarse: Vec<_> = steps.iter().filter(|&&y| y > step * 1.5).collect();
        assert!((3..=8).contains(&coarse.len()), "{steps:?}");
        // Each tick ran one of them (the same size), not all of them.
        assert!(coarse.iter().all(|&&y| y == *coarse[0]), "{steps:?}");
        assert!(
            app.world
                .get::<RegionState>(region)
                .unwrap()
                .catch_up
                .is_none()
        );
    }

    #[test]
    fn a_region_built_off_the_frame_grows_what_one_built_in_the_frame_does() {
        let grow = |loading: crate::FoliageLoading| {
            let mut app = App::new();
            app.add_plugins(engine_world::WorldPlugin);
            app.add_plugins(crate::FoliagePlugin);
            app.world.insert_resource(loading);
            app.world.resource_mut::<EcologySettings>().paused = true;
            let region = app.world.spawn((Transform::default(), forest())).id();
            let start = std::time::Instant::now();
            app.update();
            while app.world.get::<RegionState>(region).is_none() && start.elapsed().as_secs() < 60 {
                std::thread::sleep(std::time::Duration::from_millis(5));
                app.update();
            }
            assert_eq!(
                app.world
                    .resource::<crate::FoliageLoading>()
                    .regions_building(),
                0
            );
            let mut q = app.world.query::<(&Plant, &Transform, &InRegion)>();
            let mut plants: Vec<(u16, [i64; 3])> = q
                .iter(&app.world)
                .filter(|(_, _, r)| r.0 == Some(region))
                .map(|(p, t, _)| {
                    (
                        p.species.0,
                        t.translation.to_array().map(|v| (v * 1e3) as i64),
                    )
                })
                .collect();
            plants.sort();
            plants
        };
        let in_frame = grow(crate::FoliageLoading::in_frame());
        // A tiny budget: the plants go in a slice a frame.
        let background = grow(crate::FoliageLoading {
            budget_ms: 0.0,
            ..Default::default()
        });
        assert!(in_frame.len() > 100, "{} plants", in_frame.len());
        assert_eq!(in_frame, background);
    }

    #[test]
    fn a_big_snag_comes_down_whole_in_live_play() {
        let mut app = App::new();
        app.world
            .resource_mut::<Time>()
            .set_source(engine_core::time::TimeSource::Manual {
                step_secs: 1.0 / 60.0,
            });
        app.add_plugins(engine_world::WorldPlugin);
        app.add_plugins(crate::FoliagePlugin);
        app.world.resource_mut::<EcologySettings>().years_per_second = 1.0;
        app.world.spawn((
            Transform::default(),
            EcologyRegion {
                half_extents: [20.0, 20.0],
                species: vec!["oak".into()],
                seed_rain: 0.0,
                ..Default::default()
            },
        ));
        let lib = PlantLibrary::default();
        let mut snag = Plant::new(lib.id("oak").unwrap(), 0.8, Genes::default());
        // A woody snag stands 2 + 5 * size years.
        snag.stage = PlantStage::Dead { years: 5.9 };
        let snag = app
            .world
            .spawn((Transform::from_xyz(3.0, 0.0, 4.0), snag))
            .id();
        for _ in 0..40 {
            app.update();
        }
        let stage = app.world.get::<Plant>(snag).map(|p| p.stage);
        assert!(
            matches!(stage, Some(PlantStage::Stump { .. })),
            "the snag left {stage:?}"
        );
        let mut down = app
            .world
            .query_filtered::<(), Or<(With<crate::felling::FallingTree>, With<FelledLog>)>>();
        assert_eq!(down.iter(&app.world).count(), 1, "it came down whole");
    }

    #[test]
    fn old_snags_lie_as_logs_clear_of_trunks_and_each_other() {
        let lib = PlantLibrary::default();
        let region = EcologyRegion {
            history: RegionHistory {
                grow_years: 120.0,
                initial_plants: [60, 0, 60],
                ..Default::default()
            },
            ..forest()
        };
        let (plants, logs, _) = grow(&region, &[]);
        assert!(logs.len() >= 3, "{} logs", logs.len());
        let spans: Vec<LogSpan> = logs
            .iter()
            .map(|l| LogSpan::of(DVec3::ZERO, l, &lib))
            .collect();
        for (i, a) in spans.iter().enumerate() {
            assert!(a.within(&region));
            for b in &spans[i + 1..] {
                assert!(a.distance_to_span(b) >= a.radius + b.radius);
            }
            for p in plants.iter().filter(|p| {
                lib.get(p.plant.species).is_woody()
                    && p.plant.size > 0.1
                    && !matches!(p.plant.stage, PlantStage::Stump { .. })
            }) {
                let r = lib.get(p.plant.species).trunk_radius_m * p.plant.size;
                assert!(a.distance_to(Vec2::new(p.pos.x as f32, p.pos.z as f32)) >= r + a.radius);
            }
        }
    }

    #[test]
    fn only_ground_plants_grow_in_a_sight_line() {
        let lib = PlantLibrary::default();
        let region = EcologyRegion {
            sight_lines: vec![[0.0, 0.0, 8.0]],
            species: ["oak", "birch", "pine", "hazel", "fern", "grass"]
                .map(String::from)
                .to_vec(),
            history: RegionHistory {
                grow_years: 60.0,
                initial_plants: [80, 60, 200],
                ..Default::default()
            },
            ..forest()
        };
        let (plants, _, _) = grow(&region, &[]);
        let inside = |p: &&SimPlant| p.pos.x.hypot(p.pos.z) < 8.0;
        let layer = |p: &SimPlant| lib.get(p.plant.species).layer;
        assert!(
            !plants
                .iter()
                .filter(inside)
                .any(|p| layer(p) != Layer::Ground)
        );
        // Ground plants still grow there; trees and shrubs grow outside it.
        assert!(
            plants
                .iter()
                .filter(inside)
                .any(|p| layer(p) == Layer::Ground)
        );
        assert!(plants.iter().any(|p| layer(p) == Layer::Canopy));
        assert!(plants.iter().any(|p| layer(p) == Layer::Understory));
    }

    #[test]
    fn a_forest_grows_and_differs_tree_to_tree() {
        let lib = PlantLibrary::default();
        let (plants, _, _) = grow(&forest(), &[]);
        let trees: Vec<&SimPlant> = plants
            .iter()
            .filter(|p| lib.get(p.plant.species).is_tree() && p.plant.stage.is_alive())
            .collect();
        assert!(trees.len() > 20, "only {} trees", trees.len());
        let big = trees.iter().filter(|p| p.plant.size > 0.6).count();
        assert!(big > 8, "only {big} big trees");
        // Variety: sizes, genes and mesh variants differ.
        let mut heights: Vec<f32> = trees
            .iter()
            .map(|p| p.plant.height(lib.get(p.plant.species)))
            .collect();
        heights.sort_by(f32::total_cmp);
        assert!(heights[heights.len() - 1] - heights[0] > 5.0, "{heights:?}");
        let variants: std::collections::HashSet<u8> =
            trees.iter().map(|p| p.plant.genes.variant % 4).collect();
        assert_eq!(variants.len(), 4);
        // Plants keep their distance.
        for (i, a) in trees.iter().enumerate() {
            for b in trees.iter().skip(i + 1) {
                let d = DVec2::new(a.pos.x - b.pos.x, a.pos.z - b.pos.z).length();
                assert!(d > 1.0, "trees {d:.2} m apart");
            }
        }
        // Grass grows in the open, not under the trees: on average it sees more light.
        assert!(count(&plants, &lib, "grass", 0.3) > 30);
    }

    #[test]
    fn water_decides_who_grows() {
        let lib = PlantLibrary::default();
        let dry = EcologyRegion {
            moisture: 0.07,
            ..forest()
        };
        let wet = EcologyRegion {
            moisture: 0.75,
            ..forest()
        };
        let (dry_plants, _, _) = grow(&dry, &[]);
        let (wet_plants, _, _) = grow(&wet, &[]);
        // Pines tolerate drought; birch and ferns need water.
        assert!(count(&dry_plants, &lib, "pine", 0.3) > count(&dry_plants, &lib, "birch", 0.3));
        assert!(count(&wet_plants, &lib, "fern", 0.2) > count(&dry_plants, &lib, "fern", 0.2) + 5);
        // Far more trees close their canopy on wet ground.
        let trees = |p: &[SimPlant]| {
            p.iter()
                .filter(|p| {
                    p.plant.stage.is_alive()
                        && lib.get(p.plant.species).is_tree()
                        && p.plant.size > 0.3
                })
                .count()
        };
        assert!(trees(&wet_plants) > trees(&dry_plants) * 3);
    }

    #[test]
    fn clear_cut_leaves_stumps_and_logs_then_regrows() {
        let lib = PlantLibrary::default();
        let mut region = forest();
        region.history.disturbance = Disturbance::ClearCut {
            center: [0.0, 0.0],
            radius: 15.0,
        };
        region.history.years_since = 0.1;
        let (fresh, logs, _) = grow(&region, &[]);
        let stumps = fresh
            .iter()
            .filter(|p| matches!(p.plant.stage, PlantStage::Stump { .. }))
            .count();
        assert!(stumps > 3, "{stumps} stumps");
        assert!(
            !logs.is_empty() && logs.len() < stumps,
            "{} logs, {stumps} stumps",
            logs.len()
        );

        region.history.years_since = 12.0;
        let (later, _, _) = grow(&region, &[]);
        let young_trees_in_clearing = later
            .iter()
            .filter(|p| {
                lib.get(p.plant.species).is_tree()
                    && p.plant.stage.is_alive()
                    && p.plant.age_years < 12.5
                    && DVec2::new(p.pos.x, p.pos.z).length() < 15.0
            })
            .count();
        assert!(
            young_trees_in_clearing > 3,
            "{young_trees_in_clearing} young trees"
        );
    }

    #[test]
    fn mushrooms_grow_only_where_mineral_water_reaches() {
        let lib = PlantLibrary::default();
        let cave = EcologyRegion {
            half_extents: [14.0, 14.0],
            cell_m: 1.0,
            sky_light: 0.0,
            moisture: 0.55,
            species: vec!["glowcap".into(), "lantern cap".into(), "grass".into()],
            seed_rain: 30.0,
            history: RegionHistory {
                grow_years: 8.0,
                initial_plants: [0, 4, 40],
                ..Default::default()
            },
            ..Default::default()
        };
        let pool = SimSource {
            pos: DVec3::new(-6.0, 0.0, 0.0),
            radius: 8.0,
            water: 0.3,
            nutrients: 0.0,
            minerals: 0.9,
        };
        let (plants, _, _) = grow(&cave, &[pool]);
        let id = lib.id("glowcap").unwrap();
        let caps: Vec<&SimPlant> = plants
            .iter()
            .filter(|p| p.plant.species == id && p.plant.stage.is_alive())
            .collect();
        assert!(caps.len() > 10, "{} glowcaps", caps.len());
        for c in &caps {
            assert!(
                c.pos.distance(pool.pos) < pool.radius as f64 + 1.0,
                "glowcap at {:?}",
                c.pos
            );
        }
        // No light, so no grass.
        assert_eq!(count(&plants, &lib, "grass", 0.0), 0);
    }
}
