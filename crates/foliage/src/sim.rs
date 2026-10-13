//! The ecology step as plain data in, plain data out, so the same code runs on live entities,
//! in a region's history (prewarm) and in tests.
//!
//! Each step, every living plant:
//! 1. reads the resources at its roots: soil moisture (base + springs + rain wetness), nutrients
//!    (+ ash and rotting wood), minerals (mineral water), light (sky minus the shade of taller
//!    crowns) and temperature (weather + fire heat), each reduced by neighbors drinking from
//!    the same soil;
//! 2. scores them against its species' needs (the scarcest resource decides) and moves its
//!    health toward that score: plants in poor spots weaken and die, crowded stands thin out;
//! 3. if mature, scatters seeds (downwind for wind-borne species); a seed takes root only where
//!    its own species would thrive and there is room.
//!
//! Dead plants stand as snags, stumps resprout or rot, and everything that rots or burns feeds
//! the soil it stood on.

use engine_core::math::{DVec2, DVec3, Rng, Vec2, Vec3};
use engine_core::prelude::Entity;
use engine_world::WorldShift;

use crate::plant::{EcologyRegion, Genes, Plant, PlantStage};
use crate::species::{Layer, Need, PlantLibrary, PlantSpecies, ResourceLevels, SpeciesId};

/// Environment the step reads. Implemented over the `engine_world` resources for live play and
/// by a plain struct in tests.
pub trait EnvSample {
    /// Air temperature at `p`, °C (weather, altitude and fire heat).
    fn temperature_c(&self, p: DVec3) -> f32;
    /// Surface wetness from rain and puddles, 0..1.
    fn wetness(&self, p: DVec3) -> f32;
    /// Wind at `p`, m/s.
    fn wind(&self, p: DVec3) -> Vec3;
    /// Ground height and how much water/nutrients the surface holds (0 rock .. 1 soil).
    fn ground(&self, p: DVec3) -> Option<(f64, f32)>;
    /// True when `p` is under water.
    fn submerged(&self, p: DVec3) -> bool;
}

/// Calm, mild, flat soil everywhere (tests and plants without a world).
pub struct FlatEnv {
    pub temperature_c: f32,
    pub wetness: f32,
    pub wind: Vec3,
}

impl Default for FlatEnv {
    fn default() -> Self {
        Self {
            temperature_c: 15.0,
            wetness: 0.0,
            wind: Vec3::new(3.0, 0.0, 1.0),
        }
    }
}

impl EnvSample for FlatEnv {
    fn temperature_c(&self, _p: DVec3) -> f32 {
        self.temperature_c
    }
    fn wetness(&self, _p: DVec3) -> f32 {
        self.wetness
    }
    fn wind(&self, _p: DVec3) -> Vec3 {
        self.wind
    }
    fn ground(&self, _p: DVec3) -> Option<(f64, f32)> {
        Some((0.0, 0.6))
    }
    fn submerged(&self, _p: DVec3) -> bool {
        false
    }
}

/// A plant as the step sees it.
#[derive(Debug, Clone)]
pub struct SimPlant {
    /// The plant's entity, if it has one yet.
    pub entity: Option<Entity>,
    pub pos: DVec3,
    pub plant: Plant,
    /// The step changed `plant` (write it back).
    pub changed: bool,
    /// Rotted away or decayed: despawn it.
    pub removed: bool,
}

impl SimPlant {
    pub fn new(pos: DVec3, plant: Plant) -> Self {
        Self {
            entity: None,
            pos,
            plant,
            changed: true,
            removed: false,
        }
    }
}

/// A felled trunk lying on the ground, in the step's terms.
#[derive(Debug, Clone)]
pub struct SimLog {
    pub entity: Option<Entity>,
    pub pos: DVec3,
    /// Direction it fell, radians around +Y (0 = +X).
    pub yaw: f32,
    pub species: SpeciesId,
    pub size: f32,
    pub years: f32,
    pub rot_years: f32,
    pub genes: Genes,
}

/// A resource source in the step's terms (world position + effect).
#[derive(Debug, Clone, Copy)]
pub struct SimSource {
    pub pos: DVec3,
    pub radius: f32,
    pub water: f32,
    pub nutrients: f32,
    pub minerals: f32,
}

/// Soil of one region on a grid: static base levels plus slow-changing extras.
#[derive(Debug, Clone)]
pub struct Soil {
    pub origin: DVec2,
    pub cell: f32,
    pub width: u32,
    pub height: u32,
    pub water: Vec<f32>,
    pub nutrients: Vec<f32>,
    pub minerals: Vec<f32>,
    /// Extra nutrients from ash and rotting wood; fades back toward zero over decades.
    pub bonus: Vec<f32>,
    /// Recent trampling (feet, hooves, wheels); fades within a year.
    pub trample: Vec<f32>,
    /// Under water or no ground: nothing grows.
    pub blocked: Vec<bool>,
}

impl Soil {
    /// Build the soil for a region centered at `center`.
    pub fn build(
        region: &EcologyRegion,
        center: DVec3,
        sources: &[SimSource],
        env: &dyn EnvSample,
    ) -> Soil {
        let cell = region.cell_m.max(0.25);
        let width = ((region.half_extents[0] * 2.0) / cell).ceil().max(1.0) as u32;
        let height = ((region.half_extents[1] * 2.0) / cell).ceil().max(1.0) as u32;
        let origin = DVec2::new(
            center.x - region.half_extents[0] as f64,
            center.z - region.half_extents[1] as f64,
        );
        let n = (width * height) as usize;
        let mut soil = Soil {
            origin,
            cell,
            width,
            height,
            water: vec![0.0; n],
            nutrients: vec![0.0; n],
            minerals: vec![0.0; n],
            bonus: vec![0.0; n],
            trample: vec![0.0; n],
            blocked: vec![false; n],
        };
        let seed = region.seed;
        for y in 0..height {
            for x in 0..width {
                let i = (y * width + x) as usize;
                let wx = origin.x + (x as f64 + 0.5) * cell as f64;
                let wz = origin.y + (y as f64 + 0.5) * cell as f64;
                let p = DVec3::new(wx, center.y + 2.0, wz);
                let patch = |salt: u64, scale: f64| {
                    engine_core::math::noise::fbm2(wx * scale, wz * scale, seed ^ salt, 3, 2.0, 0.5)
                };
                let k = region.patchiness * 0.3;
                let mut water = region.moisture + patch(0x57a7e2, 0.035) * k;
                let mut nutrients = region.nutrients + patch(0x9a17, 0.045) * k;
                let mut minerals = region.minerals;
                match env.ground(p) {
                    Some((_, holds)) => {
                        water *= 0.45 + 0.9 * holds;
                        nutrients *= 0.35 + 1.0 * holds;
                    }
                    None => soil.blocked[i] = true,
                }
                if env.submerged(p) {
                    soil.blocked[i] = true;
                }
                let (lx, lz) = (wx - center.x, wz - center.z);
                if region.round {
                    let edge = 1.0 + 0.12 * patch(0xed6e, 0.05) as f64;
                    let ex = lx / region.half_extents[0].max(0.1) as f64;
                    let ez = lz / region.half_extents[1].max(0.1) as f64;
                    if ex * ex + ez * ez > edge * edge {
                        soil.blocked[i] = true;
                    }
                }
                for c in &region.keep_clear {
                    if (lx - c[0] as f64).hypot(lz - c[1] as f64) < c[2] as f64 {
                        soil.blocked[i] = true;
                    }
                }
                for s in sources {
                    let d = DVec2::new(wx - s.pos.x, wz - s.pos.z).length() as f32;
                    if d < s.radius {
                        let f = 1.0 - (d / s.radius).powi(2);
                        let f = f * f;
                        water += s.water * f;
                        nutrients += s.nutrients * f;
                        minerals += s.minerals * f;
                    }
                }
                soil.water[i] = water.clamp(0.0, 1.0);
                soil.nutrients[i] = nutrients.clamp(0.0, 1.0);
                soil.minerals[i] = minerals.clamp(0.0, 1.0);
            }
        }
        soil
    }

    /// Cell index for a world position (clamped to the grid).
    pub fn index(&self, p: DVec3) -> usize {
        let x = (((p.x - self.origin.x) / self.cell as f64).floor() as i64)
            .clamp(0, self.width as i64 - 1) as u32;
        let y = (((p.z - self.origin.y) / self.cell as f64).floor() as i64)
            .clamp(0, self.height as i64 - 1) as u32;
        (y * self.width + x) as usize
    }

    pub fn contains(&self, p: DVec3) -> bool {
        let x = (p.x - self.origin.x) / self.cell as f64;
        let y = (p.z - self.origin.y) / self.cell as f64;
        x >= 0.0 && y >= 0.0 && x < self.width as f64 && y < self.height as f64
    }

    /// Move the grid into a new reference frame ([`crate::shift`]). A translation moves the
    /// origin; a rotation resamples the grid around where its center went, each new cell taking
    /// the old cell under it (at height `center_y`, old frame). Cells the old grid didn't cover
    /// copy its nearest edge and are blocked, so nothing new seeds there.
    pub fn shift(&mut self, s: &WorldShift, center_y: f64) {
        if s.is_translation_only() {
            self.origin += DVec2::new(s.translation.x, s.translation.z);
            return;
        }
        let old = self.clone();
        let half = DVec2::new(self.width as f64, self.height as f64) * (self.cell as f64 * 0.5);
        let mid = old.origin + half;
        let c = s.point(DVec3::new(mid.x, center_y, mid.y));
        self.origin = DVec2::new(c.x, c.z) - half;
        for y in 0..self.height {
            for x in 0..self.width {
                let i = (y * self.width + x) as usize;
                let wx = self.origin.x + (x as f64 + 0.5) * self.cell as f64;
                let wz = self.origin.y + (y as f64 + 0.5) * self.cell as f64;
                let back = s.inverse_point(DVec3::new(wx, c.y, wz));
                let j = old.index(back);
                self.water[i] = old.water[j];
                self.nutrients[i] = old.nutrients[j];
                self.minerals[i] = old.minerals[j];
                self.bonus[i] = old.bonus[j];
                self.trample[i] = old.trample[j];
                self.blocked[i] = old.blocked[j] || !old.contains(back);
            }
        }
    }

    /// Add nutrients around `p` (ash, a rotting log), spread over `radius` meters.
    pub fn enrich(&mut self, p: DVec3, radius: f32, amount: f32) {
        let r = (radius / self.cell).ceil() as i64;
        let cx = ((p.x - self.origin.x) / self.cell as f64).floor() as i64;
        let cy = ((p.z - self.origin.y) / self.cell as f64).floor() as i64;
        for y in (cy - r)..=(cy + r) {
            for x in (cx - r)..=(cx + r) {
                if x < 0 || y < 0 || x >= self.width as i64 || y >= self.height as i64 {
                    continue;
                }
                let d = (((x - cx).pow(2) + (y - cy).pow(2)) as f32).sqrt() * self.cell;
                if d > radius.max(self.cell) {
                    continue;
                }
                let i = (y as u32 * self.width + x as u32) as usize;
                self.bonus[i] =
                    (self.bonus[i] + amount * (1.0 - d / radius.max(self.cell) * 0.5)).min(0.6);
            }
        }
    }

    /// Add a resource source to grown soil (a spring that appears during play).
    pub fn add_source(&mut self, s: &SimSource) {
        for y in 0..self.height {
            for x in 0..self.width {
                let wx = self.origin.x + (x as f64 + 0.5) * self.cell as f64;
                let wz = self.origin.y + (y as f64 + 0.5) * self.cell as f64;
                let d = DVec2::new(wx - s.pos.x, wz - s.pos.z).length() as f32;
                if d >= s.radius {
                    continue;
                }
                let f = (1.0 - (d / s.radius).powi(2)).powi(2);
                let i = (y * self.width + x) as usize;
                self.water[i] = (self.water[i] + s.water * f).clamp(0.0, 1.0);
                self.nutrients[i] = (self.nutrients[i] + s.nutrients * f).clamp(0.0, 1.0);
                self.minerals[i] = (self.minerals[i] + s.minerals * f).clamp(0.0, 1.0);
            }
        }
    }

    /// Soil water, nutrients (with ash and rot bonus) and minerals at `p`, 0..1.
    pub fn sample(&self, p: DVec3) -> [f32; 3] {
        let i = self.index(p);
        [
            self.water[i],
            (self.nutrients[i] + self.bonus[i]).min(1.0),
            self.minerals[i],
        ]
    }

    /// Record trampling at `p` (load in kg).
    pub fn trample(&mut self, p: DVec3, load_kg: f32) {
        if self.contains(p) {
            let i = self.index(p);
            self.trample[i] = (self.trample[i] + load_kg / 4000.0).min(1.0);
        }
    }
}

/// Plants bucketed on a dense grid over the region for neighbor queries.
struct Buckets {
    origin: DVec2,
    cell: f32,
    width: i64,
    height: i64,
    starts: Vec<u32>,
    items: Vec<u32>,
}

impl Buckets {
    fn new(origin: DVec2, extent: Vec2, cell: f32) -> Self {
        let width = (extent.x / cell).ceil().max(1.0) as i64 + 1;
        let height = (extent.y / cell).ceil().max(1.0) as i64 + 1;
        Self {
            origin,
            cell,
            width,
            height,
            starts: vec![0; (width * height + 1) as usize],
            items: Vec::new(),
        }
    }

    fn cell_of(&self, p: DVec3) -> (i64, i64) {
        let x =
            (((p.x - self.origin.x) / self.cell as f64).floor() as i64).clamp(0, self.width - 1);
        let y =
            (((p.z - self.origin.y) / self.cell as f64).floor() as i64).clamp(0, self.height - 1);
        (x, y)
    }

    /// Counting sort of `ids` (indices into `pos`) by cell.
    fn fill(&mut self, pos: &[DVec3], ids: impl Iterator<Item = u32> + Clone) {
        self.starts.iter_mut().for_each(|s| *s = 0);
        for i in ids.clone() {
            let (x, y) = self.cell_of(pos[i as usize]);
            self.starts[(y * self.width + x) as usize + 1] += 1;
        }
        for k in 1..self.starts.len() {
            self.starts[k] += self.starts[k - 1];
        }
        self.items.clear();
        self.items.resize(*self.starts.last().unwrap() as usize, 0);
        let mut cursor = self.starts.clone();
        for i in ids {
            let (x, y) = self.cell_of(pos[i as usize]);
            let c = (y * self.width + x) as usize;
            self.items[cursor[c] as usize] = i;
            cursor[c] += 1;
        }
    }

    /// Call `f` with every item whose cell overlaps the circle.
    fn for_each_near(&self, p: DVec3, radius: f32, mut f: impl FnMut(u32)) {
        let r = (radius / self.cell).ceil() as i64;
        let (cx, cy) = self.cell_of(p);
        for y in (cy - r).max(0)..=(cy + r).min(self.height - 1) {
            let row = y * self.width;
            let a = self.starts[(row + (cx - r).max(0)) as usize] as usize;
            let b = self.starts[(row + (cx + r).min(self.width - 1)) as usize + 1] as usize;
            for &i in &self.items[a..b] {
                f(i);
            }
        }
    }
}

/// Per-plant numbers cached for one step.
#[derive(Clone, Copy, Default)]
struct Cached {
    height: f32,
    crown: f32,
    root: f32,
    layer: Layer,
    alive: bool,
    /// Occupies space for spacing checks (alive, stumps, standing snags).
    solid: bool,
}

/// What a step produced besides changing plants in place.
#[derive(Debug, Default)]
pub struct StepOutput {
    /// New seedlings to spawn.
    pub births: Vec<SimPlant>,
    /// Seeds that were tried (for stats).
    pub seeds_tried: u32,
}

/// Inputs shared by every plant in a step.
pub struct StepContext<'a> {
    pub library: &'a PlantLibrary,
    pub region: &'a EcologyRegion,
    pub center: DVec3,
    /// Species ids that seed rain brings in.
    pub rain_species: &'a [SpeciesId],
    pub env: &'a dyn EnvSample,
}

/// Simulation state of a region that persists between steps.
pub struct RegionSim {
    pub soil: Soil,
    pub rng: Rng,
    by_layer: [Buckets; 3],
    cached: Vec<Cached>,
    positions: Vec<DVec3>,
    species_of: Vec<SpeciesId>,
    sizes: Vec<f32>,
    /// Widest living crown per layer this step, meters (how far shade can reach).
    crown_reach: [f32; 3],
}

/// Neighbor search radius per layer, meters (bucket size).
const BUCKET_CELL: [f32; 3] = [6.0, 3.0, 1.5];

impl RegionSim {
    pub fn new(soil: Soil, seed: u64) -> Self {
        let extent = Vec2::new(
            soil.width as f32 * soil.cell,
            soil.height as f32 * soil.cell,
        );
        let by_layer = BUCKET_CELL.map(|c| Buckets::new(soil.origin, extent, c));
        Self {
            soil,
            rng: Rng::new(seed ^ 0xec0_1061),
            by_layer,
            cached: Vec::new(),
            positions: Vec::new(),
            species_of: Vec::new(),
            sizes: Vec::new(),
            crown_reach: [0.0; 3],
        }
    }

    /// Move the soil and the neighbor grids into a new reference frame ([`crate::shift`]);
    /// `center` is the region's center in the old frame.
    pub fn shift(&mut self, s: &WorldShift, center: DVec3) {
        self.soil.shift(s, center.y);
        for b in &mut self.by_layer {
            b.origin = self.soil.origin;
        }
        for p in &mut self.positions {
            *p = s.point(*p);
        }
    }

    fn index(&mut self, ctx: &StepContext, plants: &[SimPlant]) {
        self.positions.clear();
        self.positions.extend(plants.iter().map(|p| p.pos));
        self.species_of.clear();
        self.species_of
            .extend(plants.iter().map(|p| p.plant.species));
        self.sizes.clear();
        self.sizes.extend(plants.iter().map(|p| p.plant.size));
        self.cached.clear();
        for p in plants {
            let sp = ctx.library.get(p.plant.species);
            let alive = p.plant.stage.is_alive() && !p.removed;
            let solid = !p.removed
                && match p.plant.stage {
                    PlantStage::Alive | PlantStage::Stump { .. } => true,
                    PlantStage::Dead { .. } | PlantStage::Burnt { .. } => sp.is_woody(),
                };
            let crown = p.plant.crown_radius(sp);
            self.cached.push(Cached {
                height: p.plant.height(sp),
                crown,
                root: (crown * 0.8).max(0.35),
                layer: sp.layer,
                alive,
                solid,
            });
        }
        self.crown_reach = [0.0; 3];
        for c in &self.cached {
            if c.alive {
                let r = &mut self.crown_reach[c.layer.index()];
                *r = r.max(c.crown);
            }
        }
        for layer in Layer::ALL {
            let cached = &self.cached;
            let ids = (0..plants.len() as u32)
                .filter(move |&i| cached[i as usize].solid && cached[i as usize].layer == layer);
            self.by_layer[layer.index()].fill(&self.positions, ids);
        }
    }

    /// Light (0..1) reaching height `h` at `p`, skipping plant `skip`.
    fn light_at(&self, ctx: &StepContext, p: DVec3, h: f32, skip: Option<u32>) -> f32 {
        let mut shade = 0.0f32;
        for layer in Layer::ALL {
            let reach = self.crown_reach[layer.index()];
            if reach <= 0.0 {
                continue;
            }
            self.by_layer[layer.index()].for_each_near(p, reach, |j| {
                if Some(j) == skip {
                    return;
                }
                let c = &self.cached[j as usize];
                if !c.alive || c.height < h * 1.15 + 0.1 {
                    return;
                }
                let d = horizontal(self.positions[j as usize], p);
                let x = d / c.crown.max(0.05);
                if x < 1.0 {
                    shade += density_of(ctx, j, self) * (1.0 - x * x).sqrt();
                }
            });
        }
        ctx.region.sky_light * (-1.6 * shade).exp()
    }

    /// Root competition (0 = alone) for a plant of `layer` with root radius `root` at `p`.
    fn crowding_at(
        &self,
        ctx: &StepContext,
        p: DVec3,
        root: f32,
        layer: Layer,
        skip: Option<u32>,
    ) -> f32 {
        let mut c = 0.0f32;
        for other in Layer::ALL {
            let weight = if other == layer { 1.0 } else { 0.3 };
            let reach = root + max_root(other);
            self.by_layer[other.index()].for_each_near(p, reach, |j| {
                if Some(j) == skip {
                    return;
                }
                let n = &self.cached[j as usize];
                if !n.alive {
                    return;
                }
                let d = horizontal(self.positions[j as usize], p);
                let sum = root + n.root;
                if d < sum {
                    c += weight * uptake_of(ctx, j, self) * (1.0 - d / sum);
                }
            });
        }
        c
    }

    /// Resource levels a plant at `p` of height `h` would get.
    #[allow(clippy::too_many_arguments)]
    fn levels_at(
        &self,
        ctx: &StepContext,
        p: DVec3,
        h: f32,
        root: f32,
        layer: Layer,
        skip: Option<u32>,
    ) -> (ResourceLevels, f32) {
        let i = self.soil.index(p);
        let crowd = self.crowding_at(ctx, p, root, layer, skip);
        let share = 1.0 / (1.0 + crowd * 0.6);
        let wet = ctx.env.wetness(p);
        let water = (self.soil.water[i] + 0.35 * wet).min(1.0);
        let nutrients = (self.soil.nutrients[i] + self.soil.bonus[i]).min(1.0);
        let minerals = self.soil.minerals[i];
        let raw = ResourceLevels {
            water,
            light: self.light_at(ctx, p, h, skip),
            nutrients,
            minerals,
            temperature_c: ctx
                .env
                .temperature_c(p + DVec3::Y * h.max(0.2) as f64 * 0.5),
        };
        let shared = ResourceLevels {
            water: water * share,
            nutrients: nutrients * share,
            minerals: minerals * share,
            ..raw
        };
        (shared, share)
    }

    /// Advance the region by `dt` years. Grows plants too when `grow` (prewarm; live play grows
    /// them every fixed step instead, so growth is smooth).
    pub fn step(
        &mut self,
        ctx: &StepContext,
        plants: &mut [SimPlant],
        logs: &mut Vec<SimLog>,
        dt: f32,
        grow: bool,
        out: &mut StepOutput,
    ) {
        self.index(ctx, plants);
        let mut layer_counts = [0u32; 3];
        for c in &self.cached {
            if c.solid {
                layer_counts[c.layer.index()] += 1;
            }
        }
        // Soil extras fade: nutrients over ~30 years, trampling within a few months.
        let fade = (-dt / 30.0).exp();
        self.soil.bonus.iter_mut().for_each(|b| *b *= fade);
        let tfade = (-dt * 4.0).exp();
        self.soil.trample.iter_mut().for_each(|t| *t *= tfade);

        let mut seeds: Vec<(DVec3, SpeciesId, Genes)> = Vec::new();
        #[allow(clippy::needless_range_loop)]
        for i in 0..plants.len() {
            let (pos, species) = (plants[i].pos, plants[i].plant.species);
            let sp = ctx.library.get(species);
            match plants[i].plant.stage {
                PlantStage::Alive => {
                    let cached = self.cached[i];
                    let (levels, share) = self.levels_at(
                        ctx,
                        pos,
                        cached.height,
                        cached.root,
                        cached.layer,
                        Some(i as u32),
                    );
                    let p = &mut plants[i].plant;
                    let (suit, mut need) = sp.suitability(&levels, &p.genes);
                    if share < 0.8 && matches!(need, Need::Water | Need::Nutrients | Need::Minerals)
                    {
                        need = Need::Crowding;
                    }
                    p.vigor = suit;
                    p.limiting = need;
                    // Seedlings respond fast; old trees ride out a bad year or two.
                    let rate = 1.2 + 4.0 * (1.0 - p.size);
                    p.health += (suit - p.health) * (1.0 - (-rate * dt).exp());
                    if cached.layer == Layer::Ground {
                        p.health -= self.soil.trample[self.soil.index(pos)] * dt * 2.0;
                    }
                    if grow {
                        grow_plant(p, sp, dt);
                    }
                    let old = p.age_years > sp.lifespan_years * (0.7 + 0.3 * (p.genes.vigor + 0.3));
                    if p.health < 0.08 || (old && self.rng.chance(dt * 0.15)) {
                        p.stage = PlantStage::Dead { years: 0.0 };
                        p.health = 0.0;
                    } else if p.is_mature() && sp.seeds_per_year > 0.0 {
                        let expected = sp.seeds_per_year * dt * suit * p.size;
                        let n = poisson(&mut self.rng, expected);
                        for _ in 0..n {
                            let q = disperse(
                                &mut self.rng,
                                pos,
                                sp,
                                ctx.env.wind(pos + DVec3::Y * cached.height as f64),
                            );
                            let genes = p.genes.inherit(&mut self.rng);
                            seeds.push((q, species, genes));
                        }
                    }
                    plants[i].changed = true;
                }
                PlantStage::Dead { years } => {
                    let years = years + dt;
                    plants[i].plant.stage = PlantStage::Dead { years };
                    let last = if sp.is_woody() {
                        2.0 + 5.0 * plants[i].plant.size
                    } else {
                        0.5
                    };
                    if years > last {
                        plants[i].removed = true;
                        self.soil.enrich(
                            pos,
                            plants[i].plant.crown_radius(sp).max(1.0),
                            0.15 * plants[i].plant.size,
                        );
                        // Over a region's history most big snags came down whole and lie
                        // rotting on the forest floor (in live play they just crumble); thinner
                        // ones broke up as they fell.
                        let size = plants[i].plant.size;
                        if grow && sp.is_tree() && size > 0.5 && self.rng.chance(0.75) {
                            logs.push(SimLog {
                                entity: None,
                                pos,
                                yaw: self.rng.range(0.0, std::f32::consts::TAU),
                                species,
                                size,
                                years: 0.0,
                                rot_years: 8.0 + 10.0 * size,
                                genes: plants[i].plant.genes,
                            });
                        }
                    }
                    plants[i].changed = true;
                }
                PlantStage::Burnt { years } => {
                    let years = years + dt;
                    plants[i].plant.stage = PlantStage::Burnt { years };
                    let last = if sp.is_woody() { 7.0 } else { 0.3 };
                    if years > last {
                        plants[i].removed = true;
                        // The collapsed snag leaves ash and charcoal behind.
                        self.soil
                            .enrich(pos, plants[i].plant.crown_radius(sp).max(1.0), 0.1);
                    }
                    plants[i].changed = true;
                }
                PlantStage::Stump { years } => {
                    let years = years + dt;
                    let p = &mut plants[i].plant;
                    p.stage = PlantStage::Stump { years };
                    if sp.resprouts && years > 0.5 && self.rng.chance(dt * 0.8) {
                        // Coppice: new shoots from the old roots grow fast.
                        p.stage = PlantStage::Alive;
                        p.size = 0.06;
                        p.health = 0.8;
                        p.chop_damage = 0.0;
                    } else if years > 10.0 {
                        plants[i].removed = true;
                        self.soil.enrich(pos, 1.5, 0.1);
                    }
                    plants[i].changed = true;
                }
            }
        }

        for log in logs.iter_mut() {
            log.years += dt;
        }
        logs.retain(|log| {
            if log.years > log.rot_years {
                self.soil.enrich(log.pos, 4.0 * log.size, 0.25 * log.size);
                false
            } else {
                true
            }
        });
        for log in logs.iter() {
            // A rotting log feeds a strip of soil along it (a nurse log).
            if self.rng.chance(dt) {
                self.soil.enrich(log.pos, 2.0 * log.size, 0.02);
            }
        }

        // Seed rain from outside the region.
        if !ctx.rain_species.is_empty() && ctx.region.seed_rain > 0.0 {
            let area_ha = ctx.region.half_extents[0] * ctx.region.half_extents[1] * 4.0 / 10_000.0;
            let n = poisson(&mut self.rng, ctx.region.seed_rain * area_ha * dt);
            for _ in 0..n {
                let id = *self.rng.pick(ctx.rain_species).unwrap();
                let q = ctx.center
                    + DVec3::new(
                        self.rng.range(-1.0, 1.0) as f64 * ctx.region.half_extents[0] as f64,
                        0.0,
                        self.rng.range(-1.0, 1.0) as f64 * ctx.region.half_extents[1] as f64,
                    );
                let genes = Genes::random(&mut self.rng);
                seeds.push((q, id, genes));
            }
        }

        out.seeds_tried += seeds.len() as u32;
        let start = out.births.len();
        for (q, species, genes) in seeds {
            let sp = ctx.library.get(species);
            let layer = sp.layer;
            if layer_counts[layer.index()] >= ctx.region.max_plants[layer.index()] {
                continue;
            }
            if !self.soil.contains(q) || self.soil.blocked[self.soil.index(q)] {
                continue;
            }
            if layer == Layer::Canopy
                && !ctx
                    .region
                    .tree_allowed(DVec2::new(q.x - ctx.center.x, q.z - ctx.center.z))
            {
                continue;
            }
            if !self.has_room(ctx, q, sp, &out.births[start..]) {
                continue;
            }
            let root = (sp.crown_radius_m * 0.04).max(0.35);
            let (levels, _) = self.levels_at(ctx, q, 0.15, root, layer, None);
            let (suit, _) = sp.suitability(&levels, &genes);
            if !self.rng.chance(sp.establishment * suit * suit) {
                continue;
            }
            let y = ctx
                .env
                .ground(q + DVec3::Y * 2.0)
                .map(|g| g.0)
                .unwrap_or(q.y);
            let mut plant = Plant::new(species, 0.03, genes);
            plant.health = 0.7;
            plant.vigor = suit;
            out.births
                .push(SimPlant::new(DVec3::new(q.x, y, q.z), plant));
            layer_counts[layer.index()] += 1;
        }
    }

    /// No plant of the same layer too close (including seedlings placed this step).
    fn has_room(&self, ctx: &StepContext, q: DVec3, sp: &PlantSpecies, fresh: &[SimPlant]) -> bool {
        let layer = sp.layer;
        let mut room = true;
        self.by_layer[layer.index()].for_each_near(q, sp.min_spacing_m.max(2.5), |j| {
            if !room {
                return;
            }
            let other = ctx.library.get(self.cached_species(j));
            let spacing = 0.5 * (sp.min_spacing_m + other.min_spacing_m);
            if horizontal(self.positions[j as usize], q) < spacing {
                room = false;
            }
        });
        if !room {
            return false;
        }
        // Nothing sprouts inside a trunk.
        if layer != Layer::Canopy {
            self.by_layer[Layer::Canopy.index()].for_each_near(q, 1.0, |j| {
                if horizontal(self.positions[j as usize], q) < 0.6 {
                    room = false;
                }
            });
        }
        room && fresh.iter().all(|b| {
            let other = ctx.library.get(b.plant.species);
            other.layer != layer
                || horizontal(b.pos, q) >= 0.5 * (sp.min_spacing_m + other.min_spacing_m)
        })
    }

    fn cached_species(&self, j: u32) -> SpeciesId {
        self.species_of[j as usize]
    }
}

fn horizontal(a: DVec3, b: DVec3) -> f32 {
    DVec2::new(a.x - b.x, a.z - b.z).length() as f32
}

fn max_root(layer: Layer) -> f32 {
    match layer {
        Layer::Canopy => 6.0,
        Layer::Understory => 2.0,
        Layer::Ground => 0.6,
    }
}

fn density_of(ctx: &StepContext, j: u32, sim: &RegionSim) -> f32 {
    let sp = ctx.library.get(sim.species_of[j as usize]);
    // Young crowns are thin.
    sp.canopy_density * (0.4 + 0.6 * sim.sizes[j as usize].min(1.0))
}

fn uptake_of(ctx: &StepContext, j: u32, sim: &RegionSim) -> f32 {
    let sp = ctx.library.get(sim.species_of[j as usize]);
    let s = sim.sizes[j as usize];
    sp.uptake * s * s
}

/// Grow one living plant by `dt` years at its current vigor.
pub fn grow_plant(p: &mut Plant, sp: &PlantSpecies, dt: f32) {
    if !p.stage.is_alive() {
        return;
    }
    p.age_years += dt;
    let rate = 1.6 / sp.years_to_mature.max(0.05) * p.vigor * p.genes.vigor;
    p.size = (p.size + (1.0 - p.size) * (1.0 - (-rate * dt).exp())).min(1.0);
}

/// Where a seed from a plant at `pos` lands.
fn disperse(rng: &mut Rng, pos: DVec3, sp: &PlantSpecies, wind: Vec3) -> DVec3 {
    let a = rng.range(0.0, std::f32::consts::TAU);
    let d = -(1.0 - rng.f32()).max(1e-4).ln() * sp.seed_range_m;
    let mut off = Vec2::new(a.cos(), a.sin()) * d;
    let w = Vec2::new(wind.x, wind.z);
    let speed = w.length();
    if speed > 0.1 && sp.wind_dispersal > 0.0 {
        off += w / speed
            * sp.seed_range_m
            * sp.wind_dispersal
            * (speed / 5.0).min(2.0)
            * rng.range(0.3, 1.7);
    }
    pos + DVec3::new(off.x as f64, 0.0, off.y as f64)
}

/// Poisson-distributed count with mean `mean` (small means; capped for safety).
fn poisson(rng: &mut Rng, mean: f32) -> u32 {
    if mean <= 0.0 {
        return 0;
    }
    if mean > 30.0 {
        return (mean + rng.range(-1.0, 1.0) * mean.sqrt()).round().max(0.0) as u32;
    }
    let l = (-mean).exp();
    let mut k = 0;
    let mut p = 1.0f32;
    loop {
        p *= rng.f32();
        if p <= l || k > 60 {
            return k;
        }
        k += 1;
    }
}
