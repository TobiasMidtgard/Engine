//! Running the ecology on live entities: regions build their soil and play out their history
//! when they first run, then step every few simulated weeks; plants grow smoothly in between.

use std::collections::HashMap;
use std::f32::consts::{FRAC_PI_2, PI};

use engine_core::prelude::*;
use engine_world::{
    BurnState, Combustible, FocusPoints, Ground, HeatField, SurfaceLayers, SurfaceMaterials, Water,
    WeatherState, WindField,
};

use crate::plant::{
    Disturbance, EcologyRegion, EcologySettings, FelledLog, Genes, Plant, PlantStage,
    ResourceSource,
};
use crate::sim::{
    EnvSample, FlatEnv, RegionSim, SimLog, SimPlant, SimSource, Soil, StepContext, StepOutput,
    grow_plant,
};
use crate::species::{Layer, PlantLibrary, SpeciesId};

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

/// Build regions that haven't run yet: soil, then their history (prewarm), then spawn the
/// plants and logs that history left behind.
pub fn init_regions(world: &mut World) {
    let mut q =
        world.query_filtered::<(Entity, &EcologyRegion, &Transform), Without<RegionState>>();
    let todo: Vec<(Entity, EcologyRegion, DVec3)> = q
        .iter(world)
        .map(|(e, r, t)| (e, r.clone(), t.translation))
        .collect();
    if todo.is_empty() {
        return;
    }
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
    for (entity, region, center) in todo {
        let start = std::time::Instant::now();
        let library = world
            .get_resource::<PlantLibrary>()
            .cloned()
            .unwrap_or_default();
        let (state, plants, logs) = {
            let env = EnvView::from_world(world);
            let sources: Vec<SimSource> = all_sources
                .iter()
                .copied()
                .filter(|s| {
                    (s.pos.x - center.x).abs() <= (region.half_extents[0] + s.radius) as f64
                        && (s.pos.z - center.z).abs() <= (region.half_extents[1] + s.radius) as f64
                })
                .collect();
            let soil = Soil::build(&region, center, &sources, &env);
            let mut sim = RegionSim::new(soil, region.seed);
            let rain = rain_species(&region, &library);
            let (plants, logs) = play_history(&mut sim, &region, center, &library, &rain, &env);
            (
                RegionState {
                    sim,
                    accum_years: 0.0,
                    pending_years: 0.0,
                    rain_species: rain,
                    center,
                    stats: RegionStats::default(),
                },
                plants,
                logs,
            )
        };
        let n = plants.len();
        for p in plants {
            spawn_sim_plant(world, p, Some(entity));
        }
        for log in &logs {
            spawn_log(world, log);
        }
        world.entity_mut(entity).insert(state);
        tracing::info!(
            "foliage: region {entity} grew {n} plants and {} logs in {:.0} ms",
            logs.len(),
            start.elapsed().as_secs_f32() * 1000.0
        );
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
            if sim.soil.blocked[sim.soil.index(p)] {
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
    // Paths, shores and plots stay passable: a trunk that came down across a keep-clear circle
    // was dragged around to lie beside it, or hauled away when it fits nowhere.
    logs.retain_mut(|l| {
        let yaw = l.yaw;
        [
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
            !blocks_keep_clear(region, center, l, library)
        })
    });
    (plants, logs)
}

/// Whether a log lies across one of the region's keep-clear circles.
fn blocks_keep_clear(
    region: &EcologyRegion,
    center: DVec3,
    log: &SimLog,
    library: &PlantLibrary,
) -> bool {
    let sp = library.get(log.species);
    let length = sp.max_height_m * log.size * 0.85;
    let a = Vec2::new((log.pos.x - center.x) as f32, (log.pos.z - center.z) as f32);
    let b = a + Vec2::new(log.yaw.cos(), log.yaw.sin()) * length;
    let reach = (sp.trunk_radius_m * log.size).max(0.1);
    region.keep_clear.iter().any(|c| {
        let p = Vec2::new(c[0], c[1]);
        let t = ((p - a).dot(b - a) / (b - a).length_squared().max(1e-6)).clamp(0.0, 1.0);
        p.distance(a + (b - a) * t) < c[2] + reach
    })
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
            // Loggers fell in rows (one direction) and haul most trunks away.
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
                    let yaw = felling_yaw + sim.rng.range(-0.35, 0.35);
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
    let sp_fuel;
    let ignition;
    let woody;
    {
        let library = world.get_resource::<PlantLibrary>();
        let sp = library
            .map(|l| l.get(p.plant.species).clone())
            .unwrap_or_default();
        sp_fuel = sp.fuel_kg;
        ignition = sp.ignition_c;
        woody = sp.is_woody();
    }
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
}

/// Grow every living plant a little (a few times per second), at its current vigor.
pub fn grow_plants(
    time: Res<Time>,
    settings: Res<EcologySettings>,
    library: Res<PlantLibrary>,
    mut clock: ResMut<GrowthClock>,
    regions: Query<&EcologyRegion>,
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
    plants.par_iter_mut().for_each(|(mut plant, region)| {
        if !plant.stage.is_alive() {
            return;
        }
        let scale = region
            .and_then(|r| r.0)
            .and_then(|r| regions.get(r).ok())
            .map(|r| r.time_scale.max(0.0))
            .unwrap_or(1.0);
        let sp = library.get(plant.species);
        let mut next = plant.clone();
        grow_plant(&mut next, sp, dt * scale);
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
    let years = dt * settings.years_per_second;

    // Which regions are due, and with how many years.
    let mut due: Vec<(Entity, f32)> = Vec::new();
    let mut q = world.query::<(Entity, &EcologyRegion, &mut RegionState)>();
    for (e, region, mut state) in q.iter_mut(world) {
        let near = focus.points.is_empty()
            || focus.distance(state.center)
                < (region.active_radius_m + region.half_extents[0].max(region.half_extents[1]))
                    as f64;
        if !near {
            state.pending_years = (state.pending_years + years * region.time_scale.max(0.0))
                .min(settings.max_catch_up_years);
            continue;
        }
        let years = years * region.time_scale.max(0.0);
        state.accum_years += years + std::mem::take(&mut state.pending_years);
        if state.accum_years >= region.step_years {
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
        {
            let env = EnvView::from_world(world);
            let ctx = StepContext {
                library: &library,
                region: &region,
                center: state.center,
                rain_species: &state.rain_species,
                env: &env,
            };
            // Long catch-ups run in coarse steps.
            let steps = (years / region.step_years).ceil().clamp(1.0, 8.0);
            let dt = years / steps;
            for _ in 0..steps as u32 {
                state
                    .sim
                    .step(&ctx, &mut plants, &mut logs, dt, steps > 1.0, &mut out);
            }
        }
        let mut stats = RegionStats {
            years,
            births: out.births.len() as u32,
            seeds_tried: out.seeds_tried,
            step_ms: start.elapsed().as_secs_f32() * 1000.0,
            ..Default::default()
        };
        for p in &plants {
            let Some(e) = p.entity else { continue };
            if p.removed {
                if let Ok(ent) = world.get_entity_mut(e) {
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
