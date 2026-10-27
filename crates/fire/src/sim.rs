//! Fire systems: fuel patches, ignition, the field step, burning objects, and everything fire
//! tells the rest of the world (heat, scorch, smoke, wind, damage, light, embers).

use engine_core::math::{DVec2, DVec3, IVec2, Rng, Vec2, Vec3};
use engine_core::prelude::*;
use engine_render::{Camera, PointLight};
use engine_world::physics::CharacterController;
use engine_world::{
    BurnState, Combustible, DamageEvent, DamageKind, FocusPoints, Gravity, Ground, HeatField,
    IgniteRequest, SimulationRange, SmokeEmission, SurfaceLayers, SurfaceMaterials, Water,
    WeatherState, WindField, WorldImpulse,
};

use crate::embers::{Ember, Embers};
use crate::field::{CellState, FireField, FuelSource, StepInput, TILE};
use crate::fuel::{CellFuel, FuelPatch, PlacedPatch};
use crate::objects::{
    Burning, Charring, ContainedFire, FlameShape, ObjectEnv, radiant_flux, step_object,
};
use crate::settings::FireSettings;

/// Takes fire damage (`DamageEvent`, kind `Fire`) when standing in flames. Characters with a
/// `CharacterController` take damage without it.
#[derive(Component, Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct FireVulnerable {
    /// Multiplier on `FireSettings::damage_per_second`.
    pub scale: f32,
}

impl Default for FireVulnerable {
    fn default() -> Self {
        Self { scale: 1.0 }
    }
}

/// Fuel patches in the world, highest priority first.
#[derive(Resource, Debug, Clone, Default)]
pub struct FirePatches {
    pub list: Vec<PlacedPatch>,
}

/// Something asked of the fire module through its API.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum FireCommand {
    Ignite {
        at: DVec3,
        radius: f32,
        temperature_c: f32,
    },
    Extinguish {
        at: DVec3,
        radius: f32,
        water_kg_m2: f32,
    },
    Clear,
}

/// Queue of [`FireCommand`]s, run at the next fixed step.
#[derive(Resource, Debug, Clone, Default)]
pub struct FireCommands(pub Vec<FireCommand>);

/// Step timing and per-step scratch.
#[derive(Resource, Debug, Clone)]
pub struct FireClock {
    acc: f32,
    /// Length of the step that just ran (0 when none ran this fixed tick).
    pub stepped: f32,
    /// Fast-forwarding (demo warm-up): every run of the step is one full step.
    pub(crate) fast_forward: bool,
    damage_acc: f32,
    gust_acc: f32,
    rng: Rng,
    /// While fast-forwarding, only objects within this distance of this point are stepped (a
    /// demo's jump leaves fires elsewhere where they are).
    pub(crate) scope: Option<(DVec3, f64)>,
    /// Where fire is awake this tick.
    pub(crate) awake: Awake,
}

/// Where fire is awake: within `radius` of these points (across the ground). Fires and embers
/// beyond it rest, as they were, until someone comes back.
#[derive(Debug, Clone)]
pub(crate) struct Awake {
    at: Vec<DVec3>,
    radius: f64,
}

impl FireClock {
    /// Start a demo's fresh fire from the same random sequence every time, so it burns the same
    /// way whatever ran before it.
    pub(crate) fn reset_for_demo(&mut self) {
        let fast_forward = self.fast_forward;
        *self = Self::default();
        self.fast_forward = fast_forward;
    }
}

impl Default for FireClock {
    fn default() -> Self {
        Self {
            acc: 0.0,
            stepped: 0.0,
            fast_forward: false,
            damage_acc: 0.0,
            gust_acc: 0.0,
            rng: Rng::new(0xF1BE),
            scope: None,
            awake: Awake {
                at: Vec::new(),
                radius: f64::INFINITY,
            },
        }
    }
}

impl Awake {
    /// Whether fire at `p` is awake (always, with no focus and no camera).
    pub(crate) fn contains(&self, p: DVec3) -> bool {
        self.within(p, self.radius)
    }

    /// Whether `p` is far enough out to rest: beyond the awake range and a margin, so things
    /// at the edge don't flicker in and out.
    fn resting(&self, p: DVec3, hysteresis: f64) -> bool {
        !self.within(p, self.radius * (1.0 + hysteresis.max(0.0)))
    }

    fn within(&self, p: DVec3, radius: f64) -> bool {
        self.at.is_empty()
            || self
                .at
                .iter()
                .any(|f| DVec2::new(f.x - p.x, f.z - p.z).length() < radius)
    }
}

/// A light the fire wants this frame (world position, lumens, range).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FireLightTarget {
    pub position: DVec3,
    pub lumens: f32,
    pub range: f32,
    pub seed: u32,
}

/// Lights fires add to the scene (managed point-light entities).
#[derive(Resource, Debug, Clone, Default)]
pub struct FireLights {
    pub targets: Vec<FireLightTarget>,
    pub entities: Vec<Entity>,
}

/// Marks a point light owned by the fire module.
#[derive(Component, Debug, Clone, Copy)]
pub struct FireLight {
    pub slot: usize,
}

/// Fuel for new cells: ground material, water, and fuel patches.
#[derive(Clone, Copy)]
pub struct WorldFuel<'a> {
    pub ground: &'a Ground,
    pub water: &'a Water,
    pub materials: &'a SurfaceMaterials,
    pub patches: &'a [PlacedPatch],
    pub ground_fuel: bool,
}

impl FuelSource for WorldFuel<'_> {
    fn fuel_at(&mut self, p: DVec3) -> (CellFuel, f64) {
        self.fuel_of(p)
    }

    fn fuel_many(&mut self, points: &[DVec3], out: &mut Vec<(CellFuel, f64)>) {
        let start = out.len();
        out.resize(start + points.len(), (CellFuel::NONE, 0.0));
        // Only the patches that reach these points (a tile's square) are tested per point.
        let (lo, hi) = points.iter().fold(
            (DVec2::splat(f64::INFINITY), DVec2::splat(f64::NEG_INFINITY)),
            |(lo, hi), p| (lo.min(DVec2::new(p.x, p.z)), hi.max(DVec2::new(p.x, p.z))),
        );
        let near: Vec<PlacedPatch> = self
            .patches
            .iter()
            .filter(|pp| {
                let c = DVec2::new(pp.center.x, pp.center.z);
                let r = pp.radius() as f64;
                c.x + r >= lo.x && c.x - r <= hi.x && c.y + r >= lo.y && c.y - r <= hi.y
            })
            .copied()
            .collect();
        let this = WorldFuel {
            patches: &near,
            ..*self
        };
        let this = &this;
        engine_core::tasks::par_chunks_mut(&mut out[start..], 64, |chunk, found| {
            for (k, f) in found.iter_mut().enumerate() {
                *f = this.fuel_of(points[chunk * 64 + k]);
            }
        });
    }
}

impl WorldFuel<'_> {
    fn fuel_of(&self, p: DVec3) -> (CellFuel, f64) {
        let Some(g) = self.ground.sample(p) else {
            return (CellFuel::NONE, p.y);
        };
        let at = DVec3::new(p.x, g.height, p.z);
        if let Some(w) = self.water.sample(at)
            && w.surface_height > g.height + 0.02
        {
            return (CellFuel::NONE, g.height);
        }
        if let Some(patch) = self.patches.iter().find(|pp| pp.contains(at)) {
            return (patch.patch.fuel(), g.height);
        }
        if self.ground_fuel {
            (
                CellFuel::from_material(self.materials.get(g.surface)),
                g.height,
            )
        } else {
            (CellFuel::NONE, g.height)
        }
    }
}

type PatchQuery<'w, 's> = Query<
    'w,
    's,
    (
        Entity,
        Ref<'static, FuelPatch>,
        Ref<'static, Transform>,
        Option<&'static GlobalTransform>,
        Option<&'static ChildOf>,
    ),
>;

/// Keep [`FirePatches`] in sync and refuel cells under patches that changed.
///
/// A patch counts as changed only if its fuel or where it lies did: one a world shift moved
/// (its `Transform` and its [`PlacedPatch`] both followed the shift) is not refueled.
#[allow(clippy::too_many_arguments)]
pub fn index_patches(
    q: PatchQuery,
    mut removed: RemovedComponents<FuelPatch>,
    mut patches: ResMut<FirePatches>,
    mut field: ResMut<FireField>,
    settings: Res<FireSettings>,
    ground: Res<Ground>,
    water: Res<Water>,
    materials: Res<SurfaceMaterials>,
) {
    let removed: Vec<Entity> = removed.read().collect();
    let stirred = q
        .iter()
        .any(|(_, p, t, _, _)| p.is_changed() || t.is_changed());
    if !stirred && removed.is_empty() {
        return;
    }
    let old = std::mem::take(&mut patches.list);
    let mut list: Vec<PlacedPatch> = q
        .iter()
        .map(|(entity, patch, t, gt, parent)| {
            let (center, rotation) = match (parent, gt) {
                (Some(_), Some(gt)) => (gt.translation(), gt.rotation()),
                _ => (t.translation, t.rotation),
            };
            let x = rotation * Vec3::X;
            let axis = Vec2::new(x.x, x.z).normalize_or(Vec2::X);
            PlacedPatch {
                entity,
                center,
                axis_x: axis,
                patch: *patch,
            }
        })
        .collect();
    let changed: Vec<Entity> = list
        .iter()
        .filter(|p| !old.iter().any(|o| o.entity == p.entity && o.same_as(p)))
        .map(|p| p.entity)
        .collect();
    list.sort_by_key(|p| std::cmp::Reverse(p.patch.priority));
    patches.list = list;
    if changed.is_empty() && removed.is_empty() {
        return;
    }

    // Refuel where changed or removed patches were or are.
    let touched: Vec<PlacedPatch> = old
        .iter()
        .chain(patches.list.iter())
        .filter(|p| changed.contains(&p.entity) || removed.contains(&p.entity))
        .copied()
        .collect();
    if touched.is_empty() || field.tiles.is_empty() {
        return;
    }
    let mut min = DVec2::splat(f64::INFINITY);
    let mut max = DVec2::splat(f64::NEG_INFINITY);
    for p in &touched {
        let r = p.radius() as f64;
        min = min.min(DVec2::new(p.center.x - r, p.center.z - r));
        max = max.max(DVec2::new(p.center.x + r, p.center.z + r));
    }
    let mut fuel = WorldFuel {
        ground: &ground,
        water: &water,
        materials: &materials,
        patches: &patches.list,
        ground_fuel: settings.ground_fuel,
    };
    let contains = |p: DVec3| touched.iter().any(|t| t.contains(p));
    field.refuel(min, max, &contains, &mut fuel);
}

/// Handle `IgniteRequest`, hot `WorldImpulse`s and API commands.
#[allow(clippy::too_many_arguments)]
pub fn handle_ignition(
    mut ignites: MessageReader<IgniteRequest>,
    mut impulses: MessageReader<WorldImpulse>,
    mut commands_queue: ResMut<FireCommands>,
    mut field: ResMut<FireField>,
    mut objects: Query<(&GlobalTransform, &mut Combustible, Option<&FlameShape>)>,
    patches: Res<FirePatches>,
    settings: Res<FireSettings>,
    ground: Res<Ground>,
    water: Res<Water>,
    materials: Res<SurfaceMaterials>,
    mut heat: ResMut<HeatField>,
) {
    let mut events: Vec<FireCommand> = std::mem::take(&mut commands_queue.0);
    for r in ignites.read() {
        events.push(FireCommand::Ignite {
            at: r.position,
            radius: r.radius,
            temperature_c: r.temperature_c,
        });
    }
    for i in impulses.read() {
        if i.heat_c > 0.0 {
            events.push(FireCommand::Ignite {
                at: i.center,
                radius: i.radius * 0.4,
                temperature_c: i.heat_c,
            });
            heat.grid.modify_disc(i.center, i.radius * 0.5, i.heat_c);
        }
    }
    if events.is_empty() || !settings.enabled {
        return;
    }
    let mut fuel = WorldFuel {
        ground: &ground,
        water: &water,
        materials: &materials,
        patches: &patches.list,
        ground_fuel: settings.ground_fuel,
    };
    for e in events {
        match e {
            FireCommand::Ignite {
                at,
                radius,
                temperature_c,
            } => {
                if temperature_c >= 250.0 {
                    field.ignite(at, radius, &mut fuel);
                } else {
                    field.add_heat(at, radius, temperature_c * 600.0, &mut fuel);
                }
                for (gt, mut c, shape) in &mut objects {
                    let shape = shape
                        .copied()
                        .unwrap_or_else(|| FlameShape::from_fuel(c.fuel_kg));
                    let d = shape.center(gt.translation()).distance(at) as f32;
                    if d <= radius + shape.radius + shape.height * 0.5 {
                        c.temperature_c = c.temperature_c.max(temperature_c);
                    }
                }
            }
            FireCommand::Extinguish {
                at,
                radius,
                water_kg_m2,
            } => {
                field.extinguish(at, radius, water_kg_m2);
                for (gt, mut c, shape) in &mut objects {
                    let shape = shape
                        .copied()
                        .unwrap_or_else(|| FlameShape::from_fuel(c.fuel_kg));
                    let d = shape.center(gt.translation()).distance(at) as f32;
                    if d <= radius + shape.radius {
                        let area = std::f32::consts::PI * shape.radius * shape.radius;
                        c.moisture += water_kg_m2 * area / c.fuel_kg.max(0.5);
                        c.temperature_c = c.temperature_c.min(100.0);
                    }
                }
            }
            FireCommand::Clear => {
                let cell = field.cell_m;
                let max_tiles = field.max_tiles;
                let origin = field.origin;
                *field = FireField::new(cell);
                field.max_tiles = max_tiles;
                field.origin = origin;
            }
        }
    }
}

trait HeatGridExt {
    fn modify_disc(&mut self, center: DVec3, radius: f32, heat_c: f32);
}

impl HeatGridExt for engine_world::Grid2<f32> {
    fn modify_disc(&mut self, center: DVec3, radius: f32, heat_c: f32) {
        let r = radius.max(self.cell);
        let lo = self.cell_at(center - DVec3::splat(r as f64));
        let hi = self.cell_at(center + DVec3::splat(r as f64));
        for y in lo.y..=hi.y {
            for x in lo.x..=hi.x {
                let c = IVec2::new(x, y);
                let cc = self.cell_center(c);
                let d = DVec2::new(cc.x - center.x, cc.y - center.z).length() as f32;
                if d > r {
                    continue;
                }
                if let Some(v) = self.get_mut(c) {
                    *v = v.max(heat_c * (1.0 - d / r));
                }
            }
        }
    }
}

/// Where fire is awake ([`FireClock`]): near the foci (or the cameras) only, within the
/// shared simulation range, or the fire's own radius if that is smaller. Its own small system,
/// so [`step_field`] reads no transforms and can run beside the physics step.
pub fn wake_fire(
    settings: Res<FireSettings>,
    mut clock: ResMut<FireClock>,
    focus: Res<FocusPoints>,
    cameras: Query<(&Camera, &GlobalTransform)>,
    range: Option<Res<SimulationRange>>,
) {
    let awake = &mut clock.awake;
    awake.at.clear();
    awake.at.extend(focus.points.iter().copied());
    if awake.at.is_empty() {
        let cams = cameras
            .iter()
            .filter(|(c, _)| c.active)
            .map(|(_, t)| t.translation());
        awake.at.extend(cams);
    }
    let range_m = range.map_or(f64::INFINITY, |r| r.active_m);
    awake.radius = range_m.min(settings.sim_radius_m as f64);
}

/// Advance the fire field at `FireSettings::sim_hz` of world time, where [`wake_fire`] last
/// found it awake. It needs nothing from this physics step (only last step's world: wind,
/// weather, ground, wetness), so it runs in `SimSet::DuringPhysics`, beside physics.
#[allow(clippy::too_many_arguments)]
pub fn step_field(
    time: Res<Time>,
    settings: Res<FireSettings>,
    mut clock: ResMut<FireClock>,
    mut field: ResMut<FireField>,
    weather: Res<WeatherState>,
    wind: Res<WindField>,
    surface: Res<SurfaceLayers>,
    patches: Res<FirePatches>,
    ground: Res<Ground>,
    water: Res<Water>,
    materials: Res<SurfaceMaterials>,
) {
    clock.stepped = 0.0;
    if !settings.enabled {
        return;
    }
    let step = 1.0 / settings.sim_hz.clamp(1.0, 240.0);
    if clock.fast_forward {
        clock.acc = step;
    } else {
        clock.acc += time.delta_secs();
    }
    if clock.acc < step {
        return;
    }
    // One step of the accumulated time (bounded so a long hitch doesn't explode the step).
    let dt = clock.acc.min(step * 4.0);
    clock.acc = 0.0;
    clock.stepped = dt;
    field.max_tiles = settings.max_tiles as usize;
    field.new_tiles_per_step = settings.new_tiles_per_step.max(1) as usize;

    let awake = &clock.awake;
    let near = move |p: DVec3| awake.contains(p);
    let wind_fn = |p: DVec3| wind.sample(p);
    let wet_fn = |p: DVec3| {
        let s = surface.sample(p);
        (s.wetness + s.puddle_m * 20.0 + s.snow_m * 10.0).min(1.0)
    };
    let input = StepInput {
        dt,
        ambient_c: weather.temperature_c,
        rain_mm_h: if weather.is_raining() {
            weather.precipitation_mm_h
        } else {
            0.0
        },
        humidity: weather.humidity,
        wind: &wind_fn,
        wetness: &wet_fn,
        params: settings.spread(),
        near: Some(&near),
    };
    let mut fuel = WorldFuel {
        ground: &ground,
        water: &water,
        materials: &materials,
        patches: &patches.list,
        ground_fuel: settings.ground_fuel,
    };
    field.step(&input, &mut fuel);
    if field.steps.is_multiple_of(200) {
        field.prune();
    }
}

type ObjectQuery<'w, 's> = Query<
    'w,
    's,
    (
        Entity,
        &'static GlobalTransform,
        &'static mut Combustible,
        Option<&'static mut Burning>,
        Option<&'static FlameShape>,
        Option<&'static ContainedFire>,
    ),
>;

/// Give each new [`Combustible`] fire's bookkeeping. On its own, unordered, so its
/// `Commands` apply at the end of the schedule instead of adding a sync point inside the fire
/// chain (`step_objects` then needs no `Commands`).
pub fn add_burning(
    mut commands: Commands,
    q: Query<Entity, (With<Combustible>, Without<Burning>)>,
) {
    for e in &q {
        commands.entity(e).insert(Burning::default());
    }
}

/// Heat, ignite and burn [`Combustible`] objects; let burning objects heat the ground.
#[allow(clippy::too_many_arguments)]
pub fn step_objects(
    clock: Res<FireClock>,
    settings: Res<FireSettings>,
    weather: Res<WeatherState>,
    mut field: ResMut<FireField>,
    mut q: ObjectQuery,
    patches: Res<FirePatches>,
    ground: Res<Ground>,
    water: Res<Water>,
    materials: Res<SurfaceMaterials>,
) {
    let dt = clock.stepped;
    if dt <= 0.0 {
        return;
    }
    // Burning objects radiate to everything around them and heat what their flames touch.
    let sources: Vec<(Entity, DVec3, FlameShape, f32)> = q
        .iter()
        .filter_map(|(e, gt, c, b, shape, _)| {
            let b = b?;
            (b.hrr_w > 0.0 && clock.awake.contains(gt.translation())).then(|| {
                let shape = shape
                    .copied()
                    .unwrap_or_else(|| FlameShape::from_fuel(c.fuel_kg));
                (e, gt.translation(), shape, b.hrr_w)
            })
        })
        .collect();
    let rain = if weather.is_raining() {
        weather.precipitation_mm_h
    } else {
        0.0
    };
    let mut deposits: Vec<(DVec3, f32, f32)> = Vec::new();
    let scope = clock.scope.filter(|_| clock.fast_forward);
    for (entity, gt, mut c, b, shape, contained) in &mut q {
        if let Some((origin, radius)) = scope
            && gt.translation().distance(origin) > radius
        {
            continue;
        }
        // Far from everyone it rests, burning or not, until someone comes back.
        if !clock.awake.contains(gt.translation()) {
            continue;
        }
        // `add_burning` gives it one at the end of the step it appears in.
        let Some(mut b) = b else {
            continue;
        };
        let shape = shape
            .copied()
            .unwrap_or_else(|| FlameShape::from_fuel(b.fuel0_kg.max(c.fuel_kg)));
        let pos = gt.translation();
        let center = shape.center(pos);
        let base = shape.base(pos);
        let mut flux = 0.0;
        if c.state != BurnState::Burning {
            // From the ground fire: strongest when the object stands in it.
            let ground_y = field
                .cell_at(base)
                .and_then(|_| ground.height(base))
                .unwrap_or(base.y);
            // Ground flames are a meter or two tall: their heat falls off fast above that, so
            // a grass fire chars a trunk but needs the trunk's own flames to reach a crown.
            let h = (base.y - ground_y).max(0.0) as f32;
            let q_ground = field.heat_release_around(base, shape.radius + 0.75);
            flux += q_ground * settings.transfer / (1.0 + (h / 2.0).powi(4));
            for (e, p, s, hrr) in &sources {
                if *e == entity {
                    continue;
                }
                let d = s.center(*p).distance(center) as f32;
                flux += radiant_flux(*hrr, (d - shape.radius * 0.5).max(0.5));
                flux += s.flame_contact(*p, *hrr, &shape, pos);
            }
        }
        let env = ObjectEnv {
            dt,
            ambient_c: weather.temperature_at_altitude(pos.y),
            rain_mm_h: rain,
            flux_w_m2: flux,
        };
        let c = &mut *c;
        if let Some(contained) = contained {
            contained.tend(c, &b);
        }
        step_object(c, &mut b, &shape, &env);
        if b.hrr_w > 0.0 && contained.is_none() {
            let r = shape.radius + 0.75;
            let w_m2 = b.hrr_w * 0.12 / (std::f32::consts::PI * r * r);
            deposits.push((base, r, w_m2));
        }
    }
    if !deposits.is_empty() {
        let mut fuel = WorldFuel {
            ground: &ground,
            water: &water,
            materials: &materials,
            patches: &patches.list,
            ground_fuel: settings.ground_fuel,
        };
        for (p, r, w) in deposits {
            field.add_external(p, r, w, &mut fuel);
        }
    }
}

/// Everything fire tells the world after a step: heat, scorch, smoke, updrafts, embers, lights.
#[allow(clippy::too_many_arguments, clippy::type_complexity)]
pub fn emit_outputs(
    mut clock: ResMut<FireClock>,
    settings: Res<FireSettings>,
    field: Res<FireField>,
    mut heat: ResMut<HeatField>,
    mut surface: ResMut<SurfaceLayers>,
    mut wind: ResMut<WindField>,
    mut smoke: MessageWriter<SmokeEmission>,
    mut embers: ResMut<Embers>,
    mut lights: ResMut<FireLights>,
    objects: Query<(
        &GlobalTransform,
        &Combustible,
        &Burning,
        Option<&FlameShape>,
        Option<&ContainedFire>,
    )>,
    mut diagnostics: ResMut<Diagnostics>,
) {
    let dt = clock.stepped;
    if dt <= 0.0 {
        return;
    }
    let cell = field.cell_m;
    let area = cell * cell;

    // Heat fades where nothing keeps it up.
    let k = (-dt / 2.0).exp();
    for v in &mut heat.grid.data {
        if *v > 0.01 {
            *v *= k;
        } else {
            *v = 0.0;
        }
    }

    let mut light_candidates: Vec<FireLightTarget> = Vec::new();
    let mut gusts: Vec<(DVec3, f32)> = Vec::new();
    let mut new_embers: Vec<Ember> = Vec::new();
    let ember_budget = settings.max_embers.saturating_sub(embers.list.len() as u32) as usize;
    let clock = &mut *clock;
    let rng = &mut clock.rng;
    let mut emissions = 0u32;

    for tile in &field.tiles {
        if !tile.active && tile.burning == 0 {
            continue;
        }
        let origin = field.tile_origin(tile.key);
        let half = TILE as f64 * cell as f64 * 0.5;
        if !clock
            .awake
            .contains(DVec3::new(origin.x + half, 0.0, origin.y + half))
        {
            continue;
        }
        // 4×4-cell blocks: smoke, light and updraft per block.
        const B: i32 = 4;
        let mut tile_hrr = 0.0f64;
        let mut tile_centroid = DVec3::ZERO;
        for by in 0..TILE / B {
            for bx in 0..TILE / B {
                let mut hrr = 0.0f32;
                let mut consumed = 0.0f32;
                let mut glow = 0.0f32;
                let mut darkness = 0.0f32;
                let mut moisture = 0.0f32;
                let mut flame = 0.0f32;
                let mut weight_pos = DVec3::ZERO;
                let mut weight_sum = 0.0f64;
                let mut n = 0.0f32;
                for y in by * B..(by + 1) * B {
                    for x in bx * B..(bx + 1) * B {
                        let c = tile.cell(IVec2::new(x, y));
                        let p = DVec3::new(
                            origin.x + (x as f64 + 0.5) * cell as f64,
                            tile.base_y + c.ground_y as f64,
                            origin.y + (y as f64 + 0.5) * cell as f64,
                        );
                        // Heat and scorch to the shared fields.
                        if c.temp > 1.0 {
                            heat.grid.modify_disc(p, cell * 0.5, c.temp);
                        }
                        if c.scorch > 0.0 {
                            surface.modify(p, |s| s.scorch = s.scorch.max(c.scorch));
                        } else if c.load > 0.0 {
                            surface.modify(p, |s| {
                                if s.scorch > 0.0 && c.state() == CellState::Intact {
                                    s.scorch = 0.0;
                                }
                            });
                        }
                        if c.hrr <= 0.0 && c.glow <= 0.0 {
                            continue;
                        }
                        let b = c.kind.behavior();
                        hrr += c.hrr * area;
                        consumed += c.hrr * area
                            / (crate::field::HEAT_OF_COMBUSTION
                                * crate::field::COMBUSTION_EFFICIENCY)
                            * dt;
                        glow += c.glow;
                        darkness += b.smoke_darkness;
                        moisture += c.effective_moisture();
                        flame = flame.max(c.flame_length());
                        weight_pos += p * c.hrr.max(1.0) as f64;
                        weight_sum += c.hrr.max(1.0) as f64;
                        n += 1.0;
                        // Embers from flaming cells.
                        if settings.embers && c.is_burning() && new_embers.len() < ember_budget {
                            let expected = b.embers * settings.ember_rate * c.burn * area * dt;
                            if rng.f32() < expected {
                                new_embers.push(Ember {
                                    position: p + DVec3::Y * (c.flame_length() as f64 * 0.6),
                                    velocity: Vec3::new(
                                        rng.range(-0.5, 0.5),
                                        1.0,
                                        rng.range(-0.5, 0.5),
                                    ),
                                    heat: 1.0,
                                    age: 0.0,
                                    lift: rng.range(0.3, 2.5) * (0.5 + c.burn),
                                    size: rng.range(0.004, 0.02),
                                    life: rng.range(2.0, 7.0),
                                    ignites: true,
                                });
                            }
                        }
                    }
                }
                if n == 0.0 {
                    continue;
                }
                let pos = weight_pos / weight_sum;
                tile_hrr += hrr as f64;
                tile_centroid += pos * hrr as f64;
                // Smoke: flaming smoke rises hot; coals and wet fuel give whiter, cooler smoke.
                // Grass coals only wisp: they glow for a minute after the flames, and more smoke
                // than this lies over the burnt ground behind the front as a low brown bank.
                let moisture = moisture / n;
                let smolder = (glow / n) * 0.00012 * B as f32 * B as f32 * area * dt;
                let mass = consumed * settings.smoke_yield + smolder;
                if mass > 0.0 {
                    let steam = (moisture / 0.6).clamp(0.0, 1.0);
                    let darkness = (darkness / n) * (1.0 - 0.6 * steam);
                    let hot = (hrr / (B * B) as f32 / area / 300_000.0).clamp(0.0, 1.0);
                    smoke.write(SmokeEmission {
                        position: pos + DVec3::Y * (flame.max(0.3) as f64 * 0.8),
                        velocity: Vec3::Y * (1.0 + 4.0 * hot),
                        radius: B as f32 * cell * 0.6,
                        mass_kg: mass,
                        temperature_c: 120.0 + 450.0 * hot,
                        darkness,
                    });
                    emissions += 1;
                }
            }
        }
        if tile_hrr > 0.0 {
            let c = tile_centroid / tile_hrr;
            light_candidates.push(FireLightTarget {
                position: c + DVec3::Y * 1.2,
                lumens: (tile_hrr as f32 / 1000.0) * settings.light_lm_per_kw,
                range: 0.0,
                seed: (tile.key.x as u32).wrapping_mul(73856093)
                    ^ (tile.key.y as u32).wrapping_mul(19349663),
            });
            gusts.push((c, tile_hrr as f32));
        }
    }

    // Burning objects: smoke from the top of the flames, embers, light, updraft.
    for (gt, c, b, shape, contained) in &objects {
        if b.hrr_w <= 0.0 && b.glow <= 0.0 || !clock.awake.contains(gt.translation()) {
            continue;
        }
        let shape = shape
            .copied()
            .unwrap_or_else(|| FlameShape::from_fuel(b.fuel0_kg));
        let center = shape.center(gt.translation());
        let top = shape.base(gt.translation()) + DVec3::Y * shape.height as f64;
        // Small fires burn less completely (cool edges, logs smoldering where they leave the
        // flames), so a campfire or torch makes up to half as much again smoke per kg as a
        // bonfire. (More, and a campfire's smoke filled the air around it as a thick veil.)
        let small = 1.5 - 0.5 * ((b.fuel0_kg - 0.5) / 4.5).clamp(0.0, 1.0);
        let mass = b.consumed_kg * settings.smoke_yield * (1.0 + c.moisture) * small
            + b.glow * 0.0002 * shape.radius * dt;
        if mass > 0.0 {
            let hot = (b.intensity).clamp(0.0, 1.0);
            smoke.write(SmokeEmission {
                position: if b.intensity > 0.0 { top } else { center },
                velocity: Vec3::Y * (1.0 + 4.0 * hot),
                radius: shape.radius.max(0.3),
                mass_kg: mass,
                temperature_c: 150.0 + 450.0 * hot,
                darkness: 0.6 * (1.0 - 0.5 * (c.moisture / 0.6).clamp(0.0, 1.0)),
            });
            emissions += 1;
        }
        if b.intensity > 0.0 {
            light_candidates.push(FireLightTarget {
                position: center,
                lumens: b.hrr_w / 1000.0 * settings.light_lm_per_kw,
                range: 0.0,
                seed: gt.translation().x.to_bits() as u32,
            });
            gusts.push((center, b.hrr_w));
            if settings.embers && new_embers.len() < ember_budget {
                let expected = b.hrr_w / 1.0e6 * 6.0 * settings.ember_rate * dt;
                let mut n = expected.floor() as u32;
                if rng.f32() < expected.fract() {
                    n += 1;
                }
                for _ in 0..n.min(8) {
                    let o = rng.in_unit_disk() * shape.radius;
                    new_embers.push(Ember {
                        position: top
                            + DVec3::new(
                                o.x as f64,
                                -(rng.f32() * shape.height * 0.5) as f64,
                                o.y as f64,
                            ),
                        velocity: Vec3::new(rng.range(-1.0, 1.0), 2.0, rng.range(-1.0, 1.0)),
                        heat: 1.0,
                        age: 0.0,
                        lift: rng.range(1.0, 4.0),
                        size: rng.range(0.005, 0.03),
                        life: rng.range(3.0, 10.0),
                        ignites: contained.is_none(),
                    });
                }
            }
        }
    }
    embers.list.extend(new_embers);

    // Fire plumes pull air up (sparingly: the wind grid is coarse).
    clock.gust_acc += dt;
    if clock.gust_acc >= 0.25 {
        clock.gust_acc = 0.0;
        gusts.sort_by(|a, b| b.1.total_cmp(&a.1));
        for (p, hrr) in gusts.into_iter().take(12) {
            let strength = (hrr / 1.0e6).sqrt().clamp(0.0, 4.0) * 1.5;
            if strength > 0.05 {
                wind.add_gust(p + DVec3::Y * 4.0, 6.0, strength, Some(Vec3::Y));
            }
        }
    }

    // Lights: the strongest fires, spaced apart.
    light_candidates.sort_by(|a, b| b.lumens.total_cmp(&a.lumens));
    let mut chosen: Vec<FireLightTarget> = Vec::new();
    for c in light_candidates {
        if c.lumens < 50.0 {
            break;
        }
        if let Some(near) = chosen
            .iter_mut()
            .find(|o| o.position.distance(c.position) < 10.0)
        {
            near.lumens += c.lumens;
            continue;
        }
        if chosen.len() < settings.max_lights as usize {
            chosen.push(c);
        }
    }
    for c in &mut chosen {
        c.lumens = c.lumens.min(1.5e6);
        c.range = (c.lumens.sqrt() * 0.12).clamp(8.0, 120.0);
    }
    lights.targets = chosen;

    diagnostics.set("fire.burning_m2", field.stats.burning_m2 as f64);
    diagnostics.set("fire.hrr_mw", field.stats.hrr_mw as f64);
    diagnostics.set("fire.tiles", field.stats.tiles as f64);
    diagnostics.set("fire.embers", embers.list.len() as f64);
    diagnostics.set("fire.smoke_emissions", emissions as f64);
}

/// Fly embers every fixed step and let hot landings try to start spot fires.
#[allow(clippy::too_many_arguments)]
pub fn step_embers(
    time: Res<Time>,
    settings: Res<FireSettings>,
    mut embers: ResMut<Embers>,
    mut clock: ResMut<FireClock>,
    wind: Res<WindField>,
    gravity: Res<Gravity>,
    ground: Res<Ground>,
    mut field: ResMut<FireField>,
    range: Option<Res<SimulationRange>>,
) {
    let dt = time.delta_secs();
    if dt <= 0.0 || embers.list.is_empty() && embers.landings.is_empty() {
        return;
    }
    // Embers that drifted out to where fire rests go out.
    let hysteresis = range.map_or(0.25, |r| r.hysteresis);
    embers
        .list
        .retain(|e| !clock.awake.resting(e.position, hysteresis));
    let g = gravity.at(embers.list.first().map(|e| e.position).unwrap_or_default());
    embers.step(dt, g, |p| wind.sample(p), |p| ground.height(p));
    let landings = std::mem::take(&mut embers.landings);
    for (p, heat) in landings {
        let Some(c) = field.cell_at(p).copied() else {
            continue;
        };
        if c.is_intact() && clock.rng.f32() < settings.spotting * heat * c.moisture_damping() {
            let mut none = |q: DVec3| (CellFuel::NONE, q.y);
            let r = field.cell_m * 0.6;
            field.ignite(p, r, &mut none);
        }
    }
}

/// Damage things standing in flames.
#[allow(clippy::too_many_arguments, clippy::type_complexity)]
pub fn apply_damage(
    mut clock: ResMut<FireClock>,
    settings: Res<FireSettings>,
    field: Res<FireField>,
    victims: Query<
        (Entity, &GlobalTransform, Option<&FireVulnerable>),
        Or<(With<FireVulnerable>, With<CharacterController>)>,
    >,
    objects: Query<(
        &GlobalTransform,
        &Burning,
        &Combustible,
        Option<&FlameShape>,
    )>,
    mut damage: MessageWriter<DamageEvent>,
) {
    clock.damage_acc += clock.stepped;
    if clock.damage_acc < 0.25 {
        return;
    }
    let dt = clock.damage_acc;
    clock.damage_acc = 0.0;
    let flames: Vec<(DVec3, FlameShape, f32)> = objects
        .iter()
        .filter(|(_, b, ..)| b.intensity > 0.05)
        .map(|(gt, b, c, s)| {
            (
                gt.translation(),
                s.copied()
                    .unwrap_or_else(|| FlameShape::from_fuel(c.fuel_kg)),
                b.intensity,
            )
        })
        .collect();
    for (entity, gt, vulnerable) in &victims {
        let p = gt.translation();
        let mut exposure = 0.0f32;
        if let Some(c) = field.cell_at(p)
            && c.is_burning()
        {
            exposure = c.burn;
        }
        for (pos, shape, intensity) in &flames {
            let base = shape.base(*pos);
            let d = DVec2::new(p.x - base.x, p.z - base.z).length() as f32;
            let h = (p.y - base.y) as f32;
            if d < shape.radius + 0.3 && h > -1.0 && h < shape.height {
                exposure = exposure.max(*intensity);
            }
        }
        if exposure <= 0.0 {
            continue;
        }
        let scale = vulnerable.map(|v| v.scale).unwrap_or(1.0);
        damage.write(DamageEvent {
            target: entity,
            amount: settings.damage_per_second * exposure * scale * dt,
            kind: DamageKind::Fire,
            point: p,
            impulse: Vec3::ZERO,
            source: None,
        });
    }
}

/// Spawn, move and flicker the fire's point lights (every frame, for smooth flicker).
pub fn sync_lights(
    mut commands: Commands,
    time: Res<Time>,
    mut lights: ResMut<FireLights>,
    mut q: Query<(&FireLight, &mut Transform, &mut PointLight)>,
) {
    let want = lights.targets.len();
    while lights.entities.len() < want {
        let slot = lights.entities.len();
        let e = commands
            .spawn((
                Name::new(format!("Fire light {slot}")),
                FireLight { slot },
                Transform::default(),
                PointLight {
                    color: [1.0, 0.55, 0.22],
                    intensity_lm: 0.0,
                    range: 10.0,
                    radius: 0.5,
                },
            ))
            .id();
        lights.entities.push(e);
    }
    let t = time.elapsed_secs_f64();
    for (light, mut transform, mut point) in &mut q {
        match lights.targets.get(light.slot) {
            Some(target) => {
                let s = target.seed as f64 * 0.001;
                let n = engine_core::math::noise::perlin2(t * 6.0 + s, s * 3.0, 7) * 0.5
                    + engine_core::math::noise::perlin2(t * 17.0 + s, s, 11) * 0.25;
                let flicker = (0.82 + 0.3 * n).clamp(0.4, 1.3);
                transform.translation = target.position
                    + DVec3::new(n as f64 * 0.3, (n * 0.2) as f64, (n * -0.2) as f64);
                point.intensity_lm = target.lumens * flicker;
                point.range = target.range;
                point.radius = (target.range * 0.05).clamp(0.3, 3.0);
            }
            None => {
                point.intensity_lm = 0.0;
            }
        }
    }
}

/// Darken and shrink [`Charring`] props as they burn.
#[allow(clippy::type_complexity)]
pub fn apply_charring(
    mut commands: Commands,
    fresh: Query<
        (Entity, &engine_render::MeshRenderer, &Transform),
        (With<Charring>, Without<CharBase>),
    >,
    mut q: Query<
        (
            &Burning,
            &Charring,
            &CharBase,
            &mut engine_render::MeshRenderer,
            &mut Transform,
        ),
        Changed<Burning>,
    >,
) {
    for (e, r, t) in &fresh {
        if let engine_render::MaterialRef::Inline(m) = &r.material {
            commands.entity(e).insert(CharBase {
                material: *m,
                scale: t.scale,
            });
        }
    }
    for (b, charring, base, mut renderer, mut t) in &mut q {
        // Quantized so a burning forest shares a handful of materials.
        let level = (charring.surface(b) * 8.0).round() / 8.0;
        let glow = (b.glow.max(b.intensity) * 4.0).round() / 4.0;
        let mut m = base.material;
        for i in 0..3 {
            m.base_color[i] += (charring.char_color[i] - m.base_color[i]) * level;
        }
        m.perceptual_roughness = m.perceptual_roughness.max(0.85 * level);
        if glow > 0.0 && level > 0.2 {
            m.emissive = [1.0, 0.28, 0.05];
            m.emissive_strength = 400.0 * glow * level;
        } else {
            m.emissive_strength = 0.0;
        }
        let next = engine_render::MaterialRef::Inline(m);
        if renderer.material != next {
            renderer.material = next;
        }
        let s = 1.0 - charring.shrink * b.char;
        let scale = base.scale * s.max(0.05);
        if t.scale != scale {
            t.scale = scale;
        }
    }
}

/// Original look of a [`Charring`] prop.
#[derive(Component, Debug, Clone, Copy)]
pub struct CharBase {
    pub material: engine_render::StandardMaterial,
    pub scale: Vec3,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn patches_override_ground_fuel() {
        let ground = Ground::default();
        let water = Water::default();
        let materials = SurfaceMaterials::default();
        let patch = PlacedPatch {
            entity: Entity::PLACEHOLDER,
            center: DVec3::new(10.0, 0.0, 0.0),
            axis_x: Vec2::X,
            patch: FuelPatch::firebreak(crate::fuel::PatchShape::Disc { radius: 2.0 }),
        };
        let patches = [patch];
        let mut fuel = WorldFuel {
            ground: &ground,
            water: &water,
            materials: &materials,
            patches: &patches,
            ground_fuel: true,
        };
        // The default ground is grass everywhere…
        assert!(fuel.fuel_at(DVec3::new(0.0, 5.0, 0.0)).0.burns());
        // …except in the firebreak.
        assert!(!fuel.fuel_at(DVec3::new(10.5, 5.0, 0.0)).0.burns());
    }
}
