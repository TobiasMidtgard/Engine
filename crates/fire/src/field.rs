//! The fire field: a sparse grid of ground cells that heat up, dry out, ignite, burn and
//! glow, spreading fire to their neighbors.
//!
//! The ground is split into square tiles of [`TILE`]² cells (0.5 m cells by default, so a tile
//! is 16 m across). Tiles only exist where fire has been: igniting a point creates its tile,
//! and burning tiles create their neighbors as the fire reaches them. A world can be any size;
//! the cost is proportional to the area that is burning or still hot.
//!
//! # Model
//!
//! Each cell holds a fuel bed (grass, litter, brush, wood) with a dry load (kg/m²) and a
//! moisture content (kg water per kg fuel). Burning cells release heat at
//! `load / burn_secs × 18 MJ/kg × 0.6` W/m² (cured grass ≈ 0.5 MW/m²). Part of it reaches the
//! cells around them through a kernel that reaches farther and harder downwind (flames lean
//! over the fuel ahead) and much less upwind. An intact cell ignites once the absorbed heat
//! covers drying its water and raising it to its ignition temperature, and loses heat to the
//! air meanwhile, so a weak fire stops at a gap instead of creeping across it. Wet fuel needs
//! more heat and burns weaker (Rothermel's moisture damping); above the fuel's moisture of
//! extinction it doesn't carry fire at all. Rain soaks fuel and puts fires out.
//!
//! Calibrated against the CSIRO grassland fire spread model: cured grass spreads about
//! 0.02–0.1 m/s in still air, about 0.5 m/s in a 3 m/s wind and 1.5–2.5 m/s in a 10 m/s
//! wind, and backs into the wind at a small fraction of that (see the tests).

use std::collections::HashMap;
use std::collections::hash_map::Entry;

use engine_core::math::{DVec2, DVec3, IVec2, Vec2, Vec3, smoothstep};
use engine_world::WorldShift;

use crate::fuel::{CellFuel, FuelKind};

/// Cells along one side of a tile.
pub const TILE: i32 = 32;
/// Cells in a tile.
pub const TILE_CELLS: usize = (TILE * TILE) as usize;
/// Kernel reach in cells (heat travels up to this many cells from a burning cell).
pub const REACH: i32 = 4;
const KERNEL_W: i32 = 2 * REACH + 1;
const KERNEL_LEN: usize = (KERNEL_W * KERNEL_W) as usize;
const PADDED: i32 = TILE + 2 * REACH;

/// Heat of combustion of dry vegetation, J/kg.
pub const HEAT_OF_COMBUSTION: f32 = 18.0e6;
/// Share of the heat of combustion released in flames.
pub const COMBUSTION_EFFICIENCY: f32 = 0.6;
/// Specific heat of dry fuel, J/(kg·K).
pub const FUEL_SPECIFIC_HEAT: f32 = 1500.0;
/// Latent heat of evaporating water, J/kg.
pub const LATENT_HEAT: f32 = 2.26e6;

/// Flame temperature above ambient at full intensity, °C.
pub const FLAME_TEMP_C: f32 = 900.0;

/// Tunable spread constants (exposed through `FireSettings`).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SpreadParams {
    /// Fraction of a burning cell's heat release that preheats the cells around it.
    pub transfer: f32,
    /// Decay length of heat transfer in still air, meters.
    pub reach_m: f32,
    /// How much farther heat reaches downwind per m/s of wind.
    pub wind_reach: f32,
    /// How much more heat goes downwind per m/s of wind.
    pub wind_boost: f32,
    /// How quickly upwind transfer falls off per m/s of wind.
    pub upwind_damping: f32,
    /// Fraction of absorbed heat a cell loses to the air per second.
    pub cooling: f32,
}

impl Default for SpreadParams {
    fn default() -> Self {
        Self {
            transfer: 0.2,
            reach_m: 0.6,
            wind_reach: 0.3,
            wind_boost: 1.0,
            upwind_damping: 0.9,
            cooling: 0.04,
        }
    }
}

/// One ground cell.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Cell {
    pub kind: FuelKind,
    /// Dry fuel load when intact, kg/m².
    pub load: f32,
    /// Fuel left, kg/m².
    pub fuel: f32,
    /// The fuel's own moisture, kg water per kg fuel.
    pub moisture: f32,
    /// Ignition temperature, °C.
    pub ignition_c: f32,
    /// Heat absorbed toward ignition, J/m².
    pub heat: f32,
    /// Flame intensity 0..1.
    pub burn: f32,
    /// Glowing coals 0..1.
    pub glow: f32,
    /// Temperature above ambient, °C.
    pub temp: f32,
    /// How charred the ground is, 0..1.
    pub scorch: f32,
    /// Ground height relative to the tile's `base_y`, meters.
    pub ground_y: f32,
    /// Surface wetness 0..1 (rain, puddles), sampled from `SurfaceLayers`.
    pub wet: f32,
    /// Seconds after burning out before the fuel regrows (0 = never).
    pub regrow_secs: f32,
    /// Seconds since this cell burnt out.
    pub burnt_for: f32,
    /// Heat release this step, W/m².
    pub hrr: f32,
    /// Heat arriving from outside the field (burning objects, explosions), W/m².
    pub external: f32,
}

impl Default for Cell {
    fn default() -> Self {
        Self {
            kind: FuelKind::None,
            load: 0.0,
            fuel: 0.0,
            moisture: 0.0,
            ignition_c: 900.0,
            heat: 0.0,
            burn: 0.0,
            glow: 0.0,
            temp: 0.0,
            scorch: 0.0,
            ground_y: 0.0,
            wet: 0.0,
            regrow_secs: 0.0,
            burnt_for: 0.0,
            hrr: 0.0,
            external: 0.0,
        }
    }
}

/// What a cell is doing, derived from its numbers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CellState {
    /// No fuel (rock, gravel, water).
    Bare,
    Intact,
    Burning,
    /// Flames out, coals glowing.
    Smoldering,
    Burnt,
}

impl Cell {
    pub fn from_fuel(fuel: CellFuel, ground_y: f32) -> Self {
        Self {
            kind: if fuel.burns() {
                fuel.kind
            } else {
                FuelKind::None
            },
            load: fuel.load_kg_m2,
            fuel: fuel.load_kg_m2,
            moisture: fuel.moisture,
            ignition_c: fuel.ignition_c,
            regrow_secs: fuel.regrow_secs,
            ground_y,
            ..Default::default()
        }
    }

    pub fn state(&self) -> CellState {
        if self.burn > 0.01 {
            CellState::Burning
        } else if self.glow > 0.01 {
            CellState::Smoldering
        } else if self.load <= 0.0 || self.kind == FuelKind::None {
            CellState::Bare
        } else if self.fuel < self.load * 0.5 {
            CellState::Burnt
        } else {
            CellState::Intact
        }
    }

    pub fn is_burning(&self) -> bool {
        self.burn > 0.01
    }

    /// Can still catch fire.
    pub fn is_intact(&self) -> bool {
        self.kind != FuelKind::None && self.burn <= 0.0 && self.fuel >= self.load * 0.5
    }

    /// Fuel moisture including surface water.
    pub fn effective_moisture(&self) -> f32 {
        self.moisture + self.wet * 1.2
    }

    /// Heat needed to dry and ignite this cell, J/m².
    pub fn ignition_energy(&self, ambient_c: f32) -> f32 {
        let dt = (self.ignition_c - ambient_c).max(50.0);
        self.fuel * (FUEL_SPECIFIC_HEAT * dt + self.effective_moisture() * LATENT_HEAT)
    }

    /// Rothermel's moisture damping: 1 for bone-dry fuel, 0 at the moisture of extinction.
    pub fn moisture_damping(&self) -> f32 {
        let mx = self.kind.behavior().extinction_moisture;
        if mx <= 0.0 {
            return 0.0;
        }
        let r = (self.effective_moisture() / mx).clamp(0.0, 1.0);
        (1.0 - 2.59 * r + 5.11 * r * r - 3.52 * r * r * r).clamp(0.0, 1.0)
    }

    /// Flaming heat release at full intensity, W/m².
    pub fn peak_hrr(&self) -> f32 {
        let b = self.kind.behavior();
        self.load / b.burn_secs.max(0.1) * HEAT_OF_COMBUSTION * COMBUSTION_EFFICIENCY
    }

    /// Start flaming now if the fuel can carry fire. Returns whether it ignited.
    pub fn ignite(&mut self) -> bool {
        if !self.is_intact() || self.moisture_damping() <= 0.05 {
            return false;
        }
        self.burn = self.burn.max(0.25);
        self.heat = 0.0;
        true
    }

    /// Flame length, meters.
    pub fn flame_length(&self) -> f32 {
        let b = self.kind.behavior();
        // Tallest at the front where the fuel is fresh, dying down behind it.
        let left = (self.fuel / self.load.max(1e-3)).clamp(0.0, 1.0);
        b.flame_m
            * self.burn.sqrt()
            * (self.load / 0.35).clamp(0.3, 3.0).powf(0.46)
            * (0.2 + 0.8 * left.sqrt())
    }
}

/// A tile of cells.
#[derive(Debug, Clone)]
pub struct Tile {
    pub key: IVec2,
    /// World height that cell `ground_y` values are relative to.
    pub base_y: f64,
    pub cells: Vec<Cell>,
    /// Cells flaming after the last step.
    pub burning: u32,
    /// Anything to simulate (flames, coals, heat, regrowth).
    pub active: bool,
    /// Bumped whenever cells change in a way that shows (for GPU uploads).
    pub version: u64,
    /// Heat release of the tile, W.
    pub hrr_w: f64,
    /// Kernel used for this tile in the last step (depends on the local wind).
    kernel: Vec<f32>,
}

impl Tile {
    pub fn cell(&self, local: IVec2) -> &Cell {
        &self.cells[(local.y * TILE + local.x) as usize]
    }

    pub fn cell_mut(&mut self, local: IVec2) -> &mut Cell {
        &mut self.cells[(local.y * TILE + local.x) as usize]
    }
}

/// Environment for one simulation step.
pub struct StepInput<'a> {
    pub dt: f32,
    /// Air temperature, °C.
    pub ambient_c: f32,
    /// Precipitation, mm/hour (rain soaks fuel).
    pub rain_mm_h: f32,
    /// Relative humidity 0..1 (sets how far fuel dries).
    pub humidity: f32,
    /// Wind at a world point, m/s.
    pub wind: &'a (dyn Fn(DVec3) -> Vec3 + Sync),
    /// Surface wetness 0..1 at a world point.
    pub wetness: &'a (dyn Fn(DVec3) -> f32 + Sync),
    pub params: SpreadParams,
    /// Only tiles whose center is this close to a focus simulate (meters); `None` = all.
    pub near: Option<&'a (dyn Fn(DVec3) -> bool + Sync)>,
}

/// Totals after a step.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct FieldStats {
    pub tiles: u32,
    pub active_tiles: u32,
    pub burning_cells: u32,
    /// Area in flames, m².
    pub burning_m2: f32,
    /// Total heat release, MW.
    pub hrr_mw: f32,
}

/// The sparse fire field. See the [module docs](self).
#[derive(Debug, Clone, bevy_ecs::prelude::Resource)]
pub struct FireField {
    /// Cell size, meters.
    pub cell_m: f32,
    /// World X/Z of the minimum corner of cell (0, 0): cell and tile coordinates count from
    /// here. A floating-origin shift moves it (see [`FireField::shift`]), so every cell keeps
    /// its coordinates and its exact place on the ground.
    pub origin: DVec2,
    pub tiles: Vec<Tile>,
    map: HashMap<IVec2, usize>,
    /// Simulated seconds.
    pub time: f64,
    pub steps: u64,
    pub stats: FieldStats,
    /// Most tiles kept; when full, new tiles are not created (fire stops at the edge).
    pub max_tiles: usize,
    incoming: Vec<Vec<f32>>,
    /// Cells a rotating shift created where the old field had none: (tile key, cell indices).
    /// They get their fuel from the world at the next step, once the world answers in the new
    /// frame.
    unfueled: Vec<(IVec2, Vec<u16>)>,
}

/// Where the fuel for a new cell comes from: `(fuel, ground height)` at a world X/Z (the `y`
/// of the point is a height hint).
pub trait FuelSource {
    fn fuel_at(&mut self, p: DVec3) -> (CellFuel, f64);
}

impl<F: FnMut(DVec3) -> (CellFuel, f64)> FuelSource for F {
    fn fuel_at(&mut self, p: DVec3) -> (CellFuel, f64) {
        self(p)
    }
}

impl FireField {
    pub fn new(cell_m: f32) -> Self {
        Self {
            cell_m: cell_m.max(0.05),
            origin: DVec2::ZERO,
            tiles: Vec::new(),
            map: HashMap::new(),
            time: 0.0,
            steps: 0,
            stats: FieldStats::default(),
            max_tiles: 4096,
            incoming: Vec::new(),
            unfueled: Vec::new(),
        }
    }

    /// Global cell coordinates of a world point.
    pub fn cell_coord(&self, p: DVec3) -> IVec2 {
        let c = self.cell_m as f64;
        IVec2::new(
            ((p.x - self.origin.x) / c).floor() as i32,
            ((p.z - self.origin.y) / c).floor() as i32,
        )
    }

    /// Tile key and local cell of global cell coordinates.
    pub fn split(c: IVec2) -> (IVec2, IVec2) {
        let key = IVec2::new(c.x.div_euclid(TILE), c.y.div_euclid(TILE));
        (key, c - key * TILE)
    }

    /// World X/Z of a cell center.
    pub fn cell_center(&self, c: IVec2) -> DVec2 {
        self.origin + (c.as_dvec2() + DVec2::splat(0.5)) * self.cell_m as f64
    }

    /// World X/Z of a tile's minimum corner.
    pub fn tile_origin(&self, key: IVec2) -> DVec2 {
        tile_origin(self.origin, self.cell_m, key)
    }

    /// Tile edge length, meters.
    pub fn tile_size(&self) -> f64 {
        TILE as f64 * self.cell_m as f64
    }

    pub fn tile_index(&self, key: IVec2) -> Option<usize> {
        self.map.get(&key).copied()
    }

    pub fn tile(&self, key: IVec2) -> Option<&Tile> {
        self.map.get(&key).map(|&i| &self.tiles[i])
    }

    /// The cell at a world point, if its tile exists.
    pub fn cell_at(&self, p: DVec3) -> Option<&Cell> {
        let (key, local) = Self::split(self.cell_coord(p));
        self.tile(key).map(|t| t.cell(local))
    }

    /// World position (with ground height) of a cell center.
    pub fn cell_world(&self, tile: &Tile, local: IVec2) -> DVec3 {
        let c = self.cell_center(tile.key * TILE + local);
        DVec3::new(c.x, tile.base_y + tile.cell(local).ground_y as f64, c.y)
    }

    /// Create the tile `key` if it doesn't exist, filling its cells from `fuel`.
    /// Returns its index, or `None` when the tile budget is used up.
    /// `hint_y` is a height near the ground (multi-level terrain returns the first surface at
    /// or below it).
    pub fn ensure_tile(
        &mut self,
        key: IVec2,
        hint_y: f64,
        fuel: &mut dyn FuelSource,
    ) -> Option<usize> {
        if let Some(&i) = self.map.get(&key) {
            return Some(i);
        }
        if self.tiles.len() >= self.max_tiles {
            return None;
        }
        let origin = self.tile_origin(key);
        let cell = self.cell_m as f64;
        let mut cells = Vec::with_capacity(TILE_CELLS);
        let mut heights = Vec::with_capacity(TILE_CELLS);
        let mut fuels = Vec::with_capacity(TILE_CELLS);
        for y in 0..TILE {
            for x in 0..TILE {
                let p = DVec3::new(
                    origin.x + (x as f64 + 0.5) * cell,
                    hint_y,
                    origin.y + (y as f64 + 0.5) * cell,
                );
                let (f, h) = fuel.fuel_at(p);
                fuels.push(f);
                heights.push(h);
            }
        }
        let base_y = heights.iter().copied().fold(f64::INFINITY, f64::min);
        let base_y = if base_y.is_finite() { base_y } else { 0.0 };
        for (f, h) in fuels.into_iter().zip(heights) {
            cells.push(Cell::from_fuel(f, (h - base_y) as f32));
        }
        let i = self.tiles.len();
        self.tiles.push(Tile {
            key,
            base_y,
            cells,
            burning: 0,
            active: false,
            version: 1,
            hrr_w: 0.0,
            kernel: vec![0.0; KERNEL_LEN],
        });
        self.map.insert(key, i);
        Some(i)
    }

    /// Refill intact (not burning) cells inside `contains` from `fuel`, e.g. after a fuel
    /// patch changed. Returns how many cells changed.
    pub fn refuel(
        &mut self,
        min: DVec2,
        max: DVec2,
        contains: &dyn Fn(DVec3) -> bool,
        fuel: &mut dyn FuelSource,
    ) -> usize {
        self.refill(min, max, contains, fuel, false)
    }

    /// Put out and reset every cell inside `contains` to fresh fuel with no scorch, as if it
    /// never burned. Returns how many cells changed.
    pub fn reset_area(
        &mut self,
        min: DVec2,
        max: DVec2,
        contains: &dyn Fn(DVec3) -> bool,
        fuel: &mut dyn FuelSource,
    ) -> usize {
        self.refill(min, max, contains, fuel, true)
    }

    fn refill(
        &mut self,
        min: DVec2,
        max: DVec2,
        contains: &dyn Fn(DVec3) -> bool,
        fuel: &mut dyn FuelSource,
        reset: bool,
    ) -> usize {
        let cell = self.cell_m as f64;
        let field_origin = self.origin;
        let mut changed = 0;
        for tile in &mut self.tiles {
            let origin = tile_origin(field_origin, self.cell_m, tile.key);
            let size = TILE as f64 * cell;
            if origin.x > max.x
                || origin.y > max.y
                || origin.x + size < min.x
                || origin.y + size < min.y
            {
                continue;
            }
            let mut touched = false;
            for y in 0..TILE {
                for x in 0..TILE {
                    let p = DVec3::new(
                        origin.x + (x as f64 + 0.5) * cell,
                        tile.base_y + 20.0,
                        origin.y + (y as f64 + 0.5) * cell,
                    );
                    if !contains(p) {
                        continue;
                    }
                    let c = &mut tile.cells[(y * TILE + x) as usize];
                    if !reset && (c.burn > 0.0 || c.glow > 0.0) {
                        continue;
                    }
                    let (f, h) = fuel.fuel_at(p);
                    let mut next = Cell::from_fuel(f, (h - tile.base_y) as f32);
                    next.wet = c.wet;
                    if !reset {
                        next.scorch = c.scorch;
                    }
                    *c = next;
                    changed += 1;
                    touched = true;
                }
            }
            if touched {
                tile.version += 1;
                tile.active = true;
            }
        }
        changed
    }

    /// Ignite every cell within `radius` of `p` that can burn. Creates tiles as needed.
    /// Returns how many cells caught.
    pub fn ignite(&mut self, p: DVec3, radius: f32, fuel: &mut dyn FuelSource) -> usize {
        let mut caught = 0;
        self.for_cells_in_disc(p, radius.max(self.cell_m * 0.5), fuel, |c, _| {
            if c.ignite() {
                caught += 1;
            }
        });
        caught
    }

    /// Add heat (J/m²) to cells within `radius` of `p`, fading to zero at the edge.
    pub fn add_heat(&mut self, p: DVec3, radius: f32, joules_m2: f32, fuel: &mut dyn FuelSource) {
        self.for_cells_in_disc(p, radius.max(self.cell_m * 0.5), fuel, |c, d| {
            let falloff = 1.0 - (d / radius.max(1e-3)).min(1.0);
            if c.is_intact() {
                c.heat += joules_m2 * falloff;
            }
            c.temp = c.temp.max(joules_m2 * falloff / 2000.0);
        });
    }

    /// Soak cells within `radius` with `water_kg_m2` of water and douse flames.
    pub fn extinguish(&mut self, p: DVec3, radius: f32, water_kg_m2: f32) -> usize {
        let mut doused = 0;
        let r = radius.max(self.cell_m * 0.5);
        let mut none = |_p: DVec3| (CellFuel::NONE, 0.0);
        self.for_cells_in_disc_existing(p, r, &mut none, |c, _| {
            let was = c.is_burning() || c.glow > 0.0;
            c.moisture += water_kg_m2 / c.load.max(0.1);
            c.burn = 0.0;
            c.glow *= 0.2;
            c.heat = 0.0;
            c.temp *= 0.3;
            if was {
                doused += 1;
            }
        });
        doused
    }

    fn for_cells_in_disc(
        &mut self,
        p: DVec3,
        radius: f32,
        fuel: &mut dyn FuelSource,
        mut f: impl FnMut(&mut Cell, f32),
    ) {
        self.visit_disc(p, radius, Some(fuel), &mut f);
    }

    fn for_cells_in_disc_existing(
        &mut self,
        p: DVec3,
        radius: f32,
        _fuel: &mut dyn FuelSource,
        mut f: impl FnMut(&mut Cell, f32),
    ) {
        self.visit_disc(p, radius, None, &mut f);
    }

    fn visit_disc(
        &mut self,
        p: DVec3,
        radius: f32,
        mut fuel: Option<&mut dyn FuelSource>,
        f: &mut dyn FnMut(&mut Cell, f32),
    ) {
        let lo = self.cell_coord(p - DVec3::splat(radius as f64));
        let hi = self.cell_coord(p + DVec3::splat(radius as f64));
        for cy in lo.y..=hi.y {
            for cx in lo.x..=hi.x {
                let c = IVec2::new(cx, cy);
                let center = self.cell_center(c);
                let d = DVec2::new(center.x - p.x, center.y - p.z).length() as f32;
                if d > radius {
                    continue;
                }
                let (key, local) = Self::split(c);
                let index = match fuel.as_deref_mut() {
                    Some(src) => self.ensure_tile(key, p.y + 2.0, src),
                    None => self.tile_index(key),
                };
                let Some(i) = index else { continue };
                let tile = &mut self.tiles[i];
                f(tile.cell_mut(local), d);
                tile.active = true;
                tile.version += 1;
            }
        }
    }

    /// Heat (W/m²) cells around `p` within `radius` are releasing, averaged over the disc.
    pub fn heat_release_around(&self, p: DVec3, radius: f32) -> f32 {
        let r = radius.max(self.cell_m);
        let lo = self.cell_coord(p - DVec3::splat(r as f64));
        let hi = self.cell_coord(p + DVec3::splat(r as f64));
        let mut sum = 0.0;
        let mut n = 0;
        for cy in lo.y..=hi.y {
            for cx in lo.x..=hi.x {
                let c = IVec2::new(cx, cy);
                let center = self.cell_center(c);
                if DVec2::new(center.x - p.x, center.y - p.z).length() as f32 > r {
                    continue;
                }
                n += 1;
                let (key, local) = Self::split(c);
                if let Some(t) = self.tile(key) {
                    sum += t.cell(local).hrr;
                }
            }
        }
        if n == 0 { 0.0 } else { sum / n as f32 }
    }

    /// Add heat from outside (W/m², this step) to cells within `radius` of `p`.
    pub fn add_external(
        &mut self,
        p: DVec3,
        radius: f32,
        watts_m2: f32,
        fuel: &mut dyn FuelSource,
    ) {
        self.for_cells_in_disc(p, radius.max(self.cell_m * 0.5), fuel, |c, _| {
            c.external += watts_m2;
        });
    }

    /// Advance the field by `input.dt` seconds.
    pub fn step(&mut self, input: &StepInput, fuel: &mut dyn FuelSource) {
        let dt = input.dt;
        if dt <= 0.0 {
            return;
        }
        self.time += dt as f64;
        self.steps += 1;
        self.fuel_unfueled(fuel);

        // Burning tiles bring their neighbors into existence so the fire can walk into them.
        let mut grow = Vec::new();
        for t in &self.tiles {
            if t.burning == 0 && !t.cells.iter().any(|c| c.external > 0.0) {
                continue;
            }
            for dy in -1..=1 {
                for dx in -1..=1 {
                    let k = t.key + IVec2::new(dx, dy);
                    if !self.map.contains_key(&k) {
                        grow.push((k, t.base_y + 20.0));
                    }
                }
            }
        }
        grow.sort_by_key(|(k, _)| (k.x, k.y));
        grow.dedup_by_key(|(k, _)| *k);
        for (k, hint_y) in grow {
            let center = self.tile_origin(k) + DVec2::splat(self.tile_size() * 0.5);
            if let Some(near) = input.near
                && !near(DVec3::new(center.x, 0.0, center.y))
            {
                continue;
            }
            if let Some(i) = self.ensure_tile(k, hint_y, fuel) {
                self.tiles[i].active = true;
            }
        }

        // Which tiles run: active ones, and every neighbor of a burning tile.
        let n = self.tiles.len();
        let mut process = vec![false; n];
        for (i, t) in self.tiles.iter().enumerate() {
            if !t.active {
                continue;
            }
            let center = self.tile_origin(t.key) + DVec2::splat(self.tile_size() * 0.5);
            if let Some(near) = input.near
                && !near(DVec3::new(center.x, t.base_y, center.y))
            {
                continue;
            }
            process[i] = true;
            if t.burning > 0 {
                for dy in -1..=1 {
                    for dx in -1..=1 {
                        if let Some(&j) = self.map.get(&(t.key + IVec2::new(dx, dy))) {
                            process[j] = true;
                        }
                    }
                }
            }
        }

        // Kernels from the local wind (2 m above the ground at the tile center).
        let cell = self.cell_m;
        let field_origin = self.origin;
        for (i, t) in self.tiles.iter_mut().enumerate() {
            if !process[i] {
                continue;
            }
            let half = TILE as f64 * cell as f64 * 0.5;
            let c = tile_origin(field_origin, cell, t.key) + DVec2::splat(half);
            let w = (input.wind)(DVec3::new(c.x, t.base_y + 2.0, c.y));
            build_kernel(&mut t.kernel, Vec2::new(w.x, w.z), cell, &input.params);
        }

        // Gather incoming heat for every processed tile (reads neighbors, writes scratch).
        self.incoming.resize_with(n, Vec::new);
        {
            let tiles = &self.tiles;
            let map = &self.map;
            let process = &process;
            engine_core::tasks::par_chunks_mut(&mut self.incoming, 4, |chunk_index, chunk| {
                let mut padded = vec![0.0f32; (PADDED * PADDED) as usize];
                for (k, incoming) in chunk.iter_mut().enumerate() {
                    let i = chunk_index * 4 + k;
                    if !process[i] {
                        continue;
                    }
                    incoming.resize(TILE_CELLS, 0.0);
                    gather_tile(tiles, map, i, &mut padded, incoming, input.params.transfer);
                }
            });
        }

        // Update cells.
        {
            let incoming = &self.incoming;
            let process = &process;
            let map = &self.map;
            let _ = map;
            engine_core::tasks::par_chunks_mut(&mut self.tiles, 4, |chunk_index, chunk| {
                for (k, tile) in chunk.iter_mut().enumerate() {
                    let i = chunk_index * 4 + k;
                    if !process[i] {
                        continue;
                    }
                    update_tile(tile, &incoming[i], input, cell, field_origin);
                }
            });
        }

        // Stats.
        let mut stats = FieldStats {
            tiles: self.tiles.len() as u32,
            ..Default::default()
        };
        let area = cell * cell;
        for t in &self.tiles {
            if t.active {
                stats.active_tiles += 1;
            }
            stats.burning_cells += t.burning;
            stats.hrr_mw += (t.hrr_w / 1e6) as f32;
        }
        stats.burning_m2 = stats.burning_cells as f32 * area;
        self.stats = stats;
    }

    /// Iterate cells in tiles that intersect the disc around `p`, with their world position.
    pub fn cells_near(&self, p: DVec3, radius: f64) -> impl Iterator<Item = (DVec3, &Cell)> {
        let size = self.tile_size();
        let cell = self.cell_m as f64;
        let (field_origin, cell_m) = (self.origin, self.cell_m);
        self.tiles
            .iter()
            .filter(move |t| {
                let o = tile_origin(field_origin, cell_m, t.key);
                let c = o + DVec2::splat(size * 0.5);
                (DVec2::new(p.x, p.z) - c).length() < radius + size
            })
            .flat_map(move |t| {
                let o = tile_origin(field_origin, cell_m, t.key);
                t.cells.iter().enumerate().map(move |(i, c)| {
                    let x = (i as i32 % TILE) as f64;
                    let y = (i as i32 / TILE) as f64;
                    (
                        DVec3::new(
                            o.x + (x + 0.5) * cell,
                            t.base_y + c.ground_y as f64,
                            o.y + (y + 0.5) * cell,
                        ),
                        c,
                    )
                })
            })
    }

    /// Drop tiles with nothing left to show or simulate (no fire, coals, scorch or regrowth),
    /// keeping memory proportional to what has burned. Cheap; call now and then.
    pub fn prune(&mut self) {
        let keep: Vec<bool> = self
            .tiles
            .iter()
            .map(|t| {
                t.active
                    || t.burning > 0
                    || t.cells
                        .iter()
                        .any(|c| c.scorch > 0.0 || c.glow > 0.0 || c.temp > 1.0 || c.heat > 0.0)
            })
            .collect();
        if keep.iter().all(|k| *k) {
            return;
        }
        let mut i = 0;
        self.tiles.retain(|_| {
            let k = keep[i];
            i += 1;
            k
        });
        self.map.clear();
        for (i, t) in self.tiles.iter().enumerate() {
            self.map.insert(t.key, i);
        }
        self.incoming.clear();
    }

    /// Move the field into a new reference frame (see [`WorldShift`]).
    ///
    /// A translation moves [`FireField::origin`] and every tile's `base_y`: cells keep their
    /// coordinates and land exactly where the ground they burn on went, whatever the distance.
    /// A rotation resamples every tile into the new frame (each new cell takes the nearest old
    /// one; a quarter turn about +Y maps cells onto cells), keeping burning tiles and those
    /// nearest `focus` (old frame) first should the turned field need more than `max_tiles`.
    /// New cells the old field didn't cover get their fuel from the world at the next
    /// [`FireField::step`], when the world answers in the new frame.
    pub fn shift(&mut self, s: &WorldShift, focus: Option<DVec3>) {
        if s.is_translation_only() {
            self.origin += DVec2::new(s.translation.x, s.translation.z);
            for t in &mut self.tiles {
                t.base_y += s.translation.y;
                t.version += 1;
            }
        } else {
            self.rotate(s, focus);
        }
    }

    fn rotate(&mut self, s: &WorldShift, focus: Option<DVec3>) {
        let budget = self.max_tiles.max(self.tiles.len());
        let mut old = std::mem::replace(self, FireField::new(self.cell_m));
        self.max_tiles = old.max_tiles;
        self.time = old.time;
        self.steps = old.steps;
        self.stats = old.stats;
        // Where the old origin went (at height 0), so a quarter turn about +Y maps the old
        // cells onto new ones.
        let o = s.point(DVec3::new(old.origin.x, 0.0, old.origin.y));
        self.origin = DVec2::new(o.x, o.z);
        // Burning tiles first, then the ones nearest the focus.
        let size = old.tile_size();
        let center_of = |t: &Tile| {
            let c = old.tile_origin(t.key) + DVec2::splat(size * 0.5);
            DVec3::new(c.x, t.base_y, c.y)
        };
        let mut order: Vec<(bool, f64, usize)> = old
            .tiles
            .iter()
            .enumerate()
            .map(|(i, t)| {
                let d = focus.map_or(0.0, |f| center_of(t).distance_squared(f));
                (t.burning == 0, d, i)
            })
            .collect();
        order.sort_by(|a, b| a.0.cmp(&b.0).then(a.1.total_cmp(&b.1)).then(a.2.cmp(&b.2)));
        let version = old.tiles.iter().map(|t| t.version).sum::<u64>() + 1;

        // New tiles under construction: each cell's state and world height, once filled.
        let mut keys: Vec<IVec2> = Vec::new();
        let mut building: HashMap<IVec2, Vec<Option<(Cell, f64)>>> = HashMap::new();
        for &(_, _, i) in &order {
            let t = &old.tiles[i];
            let o = old.tile_origin(t.key);
            // The tile's footprint in the new frame.
            let mut lo = DVec2::splat(f64::INFINITY);
            let mut hi = DVec2::splat(f64::NEG_INFINITY);
            for (a, b) in [(0.0, 0.0), (1.0, 0.0), (0.0, 1.0), (1.0, 1.0)] {
                let p = s.point(DVec3::new(o.x + a * size, t.base_y, o.y + b * size));
                lo = lo.min(DVec2::new(p.x, p.z));
                hi = hi.max(DVec2::new(p.x, p.z));
            }
            let y = s.point(center_of(t)).y;
            let c0 = self.cell_coord(DVec3::new(lo.x, 0.0, lo.y));
            let c1 = self.cell_coord(DVec3::new(hi.x, 0.0, hi.y));
            for cy in c0.y..=c1.y {
                for cx in c0.x..=c1.x {
                    let c = IVec2::new(cx, cy);
                    let center = self.cell_center(c);
                    let back = s.inverse_point(DVec3::new(center.x, y, center.y));
                    let (key, local) = Self::split(old.cell_coord(back));
                    if key != t.key {
                        continue;
                    }
                    let (new_key, new_local) = Self::split(c);
                    let cells = match building.entry(new_key) {
                        Entry::Occupied(e) => e.into_mut(),
                        Entry::Vacant(e) if keys.len() < budget => {
                            keys.push(new_key);
                            e.insert(vec![None; TILE_CELLS])
                        }
                        Entry::Vacant(_) => continue,
                    };
                    let height = s.point(old.cell_world(t, local)).y;
                    cells[(new_local.y * TILE + new_local.x) as usize] =
                        Some((*t.cell(local), height));
                }
            }
        }

        let area = (self.cell_m * self.cell_m) as f64;
        for key in keys {
            let Some(built) = building.remove(&key) else {
                continue;
            };
            let base_y = built
                .iter()
                .flatten()
                .map(|(_, h)| *h)
                .fold(f64::INFINITY, f64::min);
            if !base_y.is_finite() {
                continue;
            }
            let mut missing = Vec::new();
            let cells: Vec<Cell> = built
                .into_iter()
                .enumerate()
                .map(|(i, c)| match c {
                    Some((cell, h)) => Cell {
                        ground_y: (h - base_y) as f32,
                        ..cell
                    },
                    None => {
                        missing.push(i as u16);
                        Cell::default()
                    }
                })
                .collect();
            if !missing.is_empty() {
                self.unfueled.push((key, missing));
            }
            self.map.insert(key, self.tiles.len());
            self.tiles.push(Tile {
                key,
                base_y,
                burning: cells.iter().filter(|c| c.is_burning()).count() as u32,
                hrr_w: cells.iter().map(|c| c.hrr as f64 * area).sum(),
                cells,
                active: true,
                version,
                kernel: vec![0.0; KERNEL_LEN],
            });
        }
        // Cells still owed fuel from an earlier turn are owed it where they went.
        for (key, list) in std::mem::take(&mut old.unfueled) {
            let Some(t) = old.tile(key) else { continue };
            for i in list {
                let local = IVec2::new(i as i32 % TILE, i as i32 / TILE);
                let p = s.point(old.cell_world(t, local));
                let (new_key, new_local) = Self::split(self.cell_coord(p));
                if self.tile_index(new_key).is_none() {
                    continue;
                }
                let idx = (new_local.y * TILE + new_local.x) as u16;
                match self.unfueled.iter_mut().find(|(k, _)| *k == new_key) {
                    Some((_, cells)) if !cells.contains(&idx) => cells.push(idx),
                    Some(_) => {}
                    None => self.unfueled.push((new_key, vec![idx])),
                }
            }
        }
    }

    /// Give cells a rotating shift created their fuel and ground height from the world.
    fn fuel_unfueled(&mut self, fuel: &mut dyn FuelSource) {
        if self.unfueled.is_empty() {
            return;
        }
        let cell = self.cell_m as f64;
        for (key, list) in std::mem::take(&mut self.unfueled) {
            let Some(i) = self.tile_index(key) else {
                continue;
            };
            let o = self.tile_origin(key);
            let tile = &mut self.tiles[i];
            for idx in list {
                let (x, y) = ((idx as i32 % TILE) as f64, (idx as i32 / TILE) as f64);
                let p = DVec3::new(
                    o.x + (x + 0.5) * cell,
                    tile.base_y + 20.0,
                    o.y + (y + 0.5) * cell,
                );
                let (f, h) = fuel.fuel_at(p);
                tile.cells[idx as usize] = Cell::from_fuel(f, (h - tile.base_y) as f32);
            }
            tile.active = true;
            tile.version += 1;
        }
    }
}

/// World X/Z of the minimum corner of tile `key` in a field with origin `field_origin`.
fn tile_origin(field_origin: DVec2, cell_m: f32, key: IVec2) -> DVec2 {
    field_origin + (key * TILE).as_dvec2() * cell_m as f64
}

/// Fill `kernel` with the share of a burning cell's heat that reaches each offset, for wind
/// `wind` (m/s, world X/Z). Offsets are indexed `(dy + REACH) * KERNEL_W + (dx + REACH)` and
/// point from the receiving cell to the burning one.
pub fn build_kernel(kernel: &mut [f32], wind: Vec2, cell: f32, p: &SpreadParams) {
    let speed = wind.length();
    let dir = if speed > 1e-3 {
        wind / speed
    } else {
        Vec2::ZERO
    };
    // Normalization: the still-air kernel sums to 1.
    let mut still = 0.0;
    for dy in -REACH..=REACH {
        for dx in -REACH..=REACH {
            if dx == 0 && dy == 0 {
                continue;
            }
            let d = Vec2::new(dx as f32, dy as f32).length() * cell;
            still += (-d / p.reach_m).exp() / d;
        }
    }
    for dy in -REACH..=REACH {
        for dx in -REACH..=REACH {
            let idx = ((dy + REACH) * KERNEL_W + (dx + REACH)) as usize;
            if dx == 0 && dy == 0 {
                kernel[idx] = 0.0;
                continue;
            }
            // From the burning cell (at -offset) toward the receiver: direction = offset.
            let o = Vec2::new(-dx as f32, -dy as f32);
            let d = o.length() * cell;
            let cos = if speed > 1e-3 {
                o.normalize().dot(dir)
            } else {
                0.0
            };
            let down = cos.max(0.0);
            let up = (-cos).max(0.0);
            let reach = p.reach_m * (1.0 + p.wind_reach * speed * down);
            let boost = 1.0 + p.wind_boost * speed * down * down;
            let damp = (-p.upwind_damping * speed * up).exp();
            kernel[idx] = (-d / reach).exp() / d * boost * damp / still;
        }
    }
}

fn gather_tile(
    tiles: &[Tile],
    map: &HashMap<IVec2, usize>,
    i: usize,
    padded: &mut [f32],
    incoming: &mut [f32],
    transfer: f32,
) {
    let tile = &tiles[i];
    padded.fill(0.0);
    let mut any = false;
    for dy in -1..=1 {
        for dx in -1..=1 {
            let Some(&j) = map.get(&(tile.key + IVec2::new(dx, dy))) else {
                continue;
            };
            let t = &tiles[j];
            if t.burning == 0 && t.hrr_w <= 0.0 {
                continue;
            }
            // Copy the part of tile j that falls inside the padded window.
            for ly in 0..TILE {
                let py = dy * TILE + ly + REACH;
                if !(0..PADDED).contains(&py) {
                    continue;
                }
                for lx in 0..TILE {
                    let px = dx * TILE + lx + REACH;
                    if !(0..PADDED).contains(&px) {
                        continue;
                    }
                    let h = t.cells[(ly * TILE + lx) as usize].hrr;
                    if h > 0.0 {
                        padded[(py * PADDED + px) as usize] = h;
                        any = true;
                    }
                }
            }
        }
    }
    for (idx, out) in incoming.iter_mut().enumerate() {
        *out = tile.cells[idx].external;
    }
    if !any {
        return;
    }
    let k = &tile.kernel;
    for ly in 0..TILE {
        for lx in 0..TILE {
            let idx = (ly * TILE + lx) as usize;
            if !tile.cells[idx].is_intact() {
                continue;
            }
            let mut q = 0.0;
            for ky in 0..KERNEL_W {
                let row = ((ly + ky) * PADDED + lx) as usize;
                let krow = (ky * KERNEL_W) as usize;
                for kx in 0..KERNEL_W as usize {
                    q += padded[row + kx] * k[krow + kx];
                }
            }
            incoming[idx] += q * transfer;
        }
    }
}

fn update_tile(
    tile: &mut Tile,
    incoming: &[f32],
    input: &StepInput,
    cell_m: f32,
    field_origin: DVec2,
) {
    let dt = input.dt;
    let ambient = input.ambient_c;
    // Equilibrium moisture of dead fuel: drier in dry air.
    let emc = 0.03 + 0.17 * input.humidity.clamp(0.0, 1.0);
    let rain = input.rain_mm_h.max(0.0);
    let mut burning = 0;
    let mut hrr_sum = 0.0f64;
    let mut changed = false;
    let mut any_active = false;
    let origin = tile_origin(field_origin, cell_m, tile.key);
    let area = cell_m * cell_m;
    let cooling = input.params.cooling;
    for (idx, c) in tile.cells.iter_mut().enumerate() {
        let q = incoming.get(idx).copied().unwrap_or(0.0);
        c.external = 0.0;
        if c.kind == FuelKind::None {
            if c.temp > 0.5 || c.glow > 0.0 {
                c.temp *= (-dt / 6.0).exp();
                c.glow = (c.glow - dt / 10.0).max(0.0);
                any_active = true;
                changed = true;
            } else if c.temp != 0.0 {
                c.temp = 0.0;
            }
            c.hrr = 0.0;
            continue;
        }
        let b = c.kind.behavior();
        // Surface wetness from rain and puddles, sampled where something happens.
        if c.burn > 0.0 || q > 0.0 || c.heat > 0.0 || rain > 0.0 || c.glow > 0.0 {
            let x = (idx as i32 % TILE) as f64;
            let y = (idx as i32 / TILE) as f64;
            let p = DVec3::new(
                origin.x + (x + 0.5) * cell_m as f64,
                tile.base_y + c.ground_y as f64,
                origin.y + (y + 0.5) * cell_m as f64,
            );
            c.wet = (input.wetness)(p).clamp(0.0, 1.0);
        }
        // Rain soaks fuel; dry weather slowly dries it toward equilibrium.
        if rain > 0.0 {
            c.moisture += rain / 3600.0 * 0.15 / c.load.max(0.05) * dt;
            c.moisture = c.moisture.min(3.0);
        } else if c.moisture > emc {
            c.moisture += (emc - c.moisture) * (dt / 1800.0);
        }

        if c.burn > 0.0 {
            any_active = true;
            changed = true;
            let damping = c.moisture_damping();
            let flaming = c.fuel > c.load * 0.06;
            let target = if flaming { damping } else { 0.0 };
            let rate = if target > c.burn { 2.5 } else { 1.5 };
            c.burn += (target - c.burn) * (rate * dt).min(1.0);
            if c.burn < 0.01 {
                c.burn = 0.0;
            }
            let consumed = (c.load / b.burn_secs * c.burn * dt).min(c.fuel);
            c.fuel -= consumed;
            c.hrr = consumed / dt * HEAT_OF_COMBUSTION * COMBUSTION_EFFICIENCY;
            // Coals build up while it flames.
            c.glow = (c.glow + dt / (b.burn_secs * 0.5))
                .min(1.0)
                .max(c.burn * 0.5);
            c.temp = c.temp.max(FLAME_TEMP_C * c.burn);
            c.temp += (FLAME_TEMP_C * c.burn - c.temp) * (dt * 2.0).min(1.0);
            c.scorch = c
                .scorch
                .max(1.0 - c.fuel / c.load.max(1e-3))
                .max(c.burn * 0.3);
            if c.burn > 0.01 {
                burning += 1;
            }
            hrr_sum += c.hrr as f64 * area as f64;
            continue;
        }

        if c.glow > 0.0 {
            any_active = true;
            changed = true;
            // Coals: little heat, no flames; rain and water put them out faster.
            let wet_kill = 1.0 + c.effective_moisture() * 4.0;
            c.glow = (c.glow - dt / b.glow_secs.max(0.1) * wet_kill).max(0.0);
            c.hrr = c.glow * c.peak_hrr() * 0.02;
            c.temp += (300.0 * c.glow - c.temp) * (dt * 0.5).min(1.0);
            c.scorch = c.scorch.max(0.6);
            hrr_sum += c.hrr as f64 * area as f64;
            continue;
        }
        c.hrr = 0.0;

        if !c.is_intact() {
            // Burnt: cool down, then maybe regrow.
            if c.temp > 0.5 {
                c.temp *= (-dt / 8.0).exp();
                any_active = true;
                changed = true;
            } else {
                c.temp = 0.0;
            }
            if c.regrow_secs > 0.0 {
                any_active = true;
                c.burnt_for += dt;
                // Green shoots: the char fades over the second half (in steps, so the
                // renderer's map only refreshes a few times).
                let fade = ((1.0 - c.burnt_for / c.regrow_secs) * 2.0).clamp(0.0, 1.0);
                let fade = (fade * 12.0).ceil() / 12.0;
                if fade < c.scorch {
                    c.scorch = fade;
                    changed = true;
                }
                if c.burnt_for >= c.regrow_secs {
                    c.fuel = c.load;
                    c.scorch = 0.0;
                    c.burnt_for = 0.0;
                    c.heat = 0.0;
                    changed = true;
                }
            }
            continue;
        }

        // Intact fuel: absorb heat, lose some to the air, ignite when there's enough.
        if q > 0.0 || c.heat > 0.0 {
            any_active = true;
            c.heat += q * dt;
            c.heat -= c.heat * (cooling * dt).min(1.0);
            let need = c.ignition_energy(ambient);
            let frac = (c.heat / need.max(1.0)).min(1.0);
            let t = frac * (c.ignition_c - ambient).max(0.0);
            if (t - c.temp).abs() > 5.0 {
                changed = true;
            }
            c.temp = t;
            if c.heat >= need && c.moisture_damping() > 0.05 && c.ignite() {
                burning += 1;
                changed = true;
            }
            if c.heat < 1.0 {
                c.heat = 0.0;
            }
        }
    }
    tile.burning = burning;
    tile.hrr_w = hrr_sum;
    tile.active = any_active || rain > 0.0 && tile.burning > 0;
    if changed {
        tile.version += 1;
    }
}

/// Smoothed 0..1 "how much is going on" for visuals.
pub fn visual_glow(c: &Cell) -> f32 {
    smoothstep(0.0, 1.0, c.glow)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fuel::FuelKind;

    fn pools() {
        engine_core::tasks::init_task_pools();
    }

    fn grass(_p: DVec3) -> (CellFuel, f64) {
        (
            CellFuel {
                kind: FuelKind::Grass,
                load_kg_m2: 0.35,
                moisture: 0.06,
                ignition_c: 300.0,
                regrow_secs: 0.0,
            },
            0.0,
        )
    }

    fn input(wind: Vec3, dt: f32) -> (Box<dyn Fn(DVec3) -> Vec3 + Sync>, f32) {
        (Box::new(move |_| wind), dt)
    }

    /// Farthest burning-or-burnt distance from `origin` along `dir`.
    fn front(field: &FireField, origin: DVec3, dir: DVec3) -> f64 {
        field
            .cells_near(origin, 200.0)
            .filter(|(_, c)| {
                matches!(
                    c.state(),
                    CellState::Burning | CellState::Smoldering | CellState::Burnt
                )
            })
            .map(|(p, _)| (p - origin).dot(dir))
            .fold(0.0, f64::max)
    }

    /// Ignite a line across the wind and measure the head and back spread rates (m/s).
    fn spread_rates(wind_speed: f32, secs: f32) -> (f64, f64) {
        spread_rates_with(wind_speed, secs, SpreadParams::default())
    }

    fn spread_rates_with(wind_speed: f32, secs: f32, params: SpreadParams) -> (f64, f64) {
        pools();
        let mut field = FireField::new(0.5);
        let mut fuel = grass;
        let (wind, dt) = input(Vec3::new(wind_speed, 0.0, 0.0), 0.05);
        let wet = |_p: DVec3| 0.0;
        let step = StepInput {
            dt,
            ambient_c: 20.0,
            rain_mm_h: 0.0,
            humidity: 0.3,
            wind: wind.as_ref(),
            wetness: &wet,
            params,
            near: None,
        };
        for z in -10..=10 {
            field.ignite(DVec3::new(0.0, 0.0, z as f64), 0.8, &mut fuel);
        }
        // Let the fire establish, then measure.
        let warm = (4.0 / dt) as usize;
        for _ in 0..warm {
            field.step(&step, &mut fuel);
        }
        let h0 = front(&field, DVec3::ZERO, DVec3::X);
        let b0 = front(&field, DVec3::ZERO, -DVec3::X);
        let n = (secs / dt) as usize;
        for _ in 0..n {
            field.step(&step, &mut fuel);
        }
        let h1 = front(&field, DVec3::ZERO, DVec3::X);
        let b1 = front(&field, DVec3::ZERO, -DVec3::X);
        ((h1 - h0) / secs as f64, (b1 - b0) / secs as f64)
    }

    #[test]
    #[ignore]
    fn calibrate() {
        for transfer in [0.2, 0.3, 0.45] {
            for cooling in [0.03, 0.06] {
                for (boost, reach_m) in [(1.0, 0.35), (2.5, 0.35), (1.0, 0.6), (2.5, 0.6)] {
                    let p = SpreadParams {
                        transfer,
                        cooling,
                        wind_boost: boost,
                        reach_m,
                        ..Default::default()
                    };
                    let a = spread_rates_with(0.0, 20.0, p);
                    let b = spread_rates_with(3.0, 10.0, p);
                    let c = spread_rates_with(10.0, 5.0, p);
                    eprintln!(
                        "t {transfer} c {cooling} b {boost} r {reach_m}: {:.3}/{:.3} {:.3}/{:.3} {:.3}/{:.3}",
                        a.0, a.1, b.0, b.1, c.0, c.1
                    );
                }
            }
        }
    }

    #[test]
    fn spread_matches_grassland_rates() {
        let (still, still_back) = spread_rates(0.0, 30.0);
        let (light, light_back) = spread_rates(3.0, 12.0);
        let (strong, strong_back) = spread_rates(10.0, 6.0);
        eprintln!(
            "still {still:.3}/{still_back:.3}  3 m/s {light:.3}/{light_back:.3}  10 m/s {strong:.3}/{strong_back:.3}"
        );
        assert!((0.01..0.15).contains(&still), "still air: {still} m/s");
        assert!((still_back - still).abs() < 0.05, "still air is symmetric");
        assert!((0.25..1.0).contains(&light), "3 m/s wind: {light} m/s");
        assert!((1.0..3.0).contains(&strong), "10 m/s wind: {strong} m/s");
        assert!(
            light_back < light * 0.25,
            "backing fire: {light_back} vs {light}"
        );
        assert!(
            strong_back < strong * 0.15,
            "backing fire: {strong_back} vs {strong}"
        );
    }

    #[test]
    fn firebreak_and_wet_fuel_stop_the_fire() {
        pools();
        let mut field = FireField::new(0.5);
        // Dry grass, a 4 m gravel break at x in [10, 14), then grass again.
        let mut fuel = |p: DVec3| {
            if (10.0..14.0).contains(&p.x) || p.x < -1.0 {
                (CellFuel::NONE, 0.0)
            } else if p.z > 6.0 {
                // A soaked strip on the other side.
                let mut g = grass(p).0;
                g.moisture = 0.6;
                (g, 0.0)
            } else {
                grass(p)
            }
        };
        let wind = |_p: DVec3| Vec3::new(4.0, 0.0, 0.0);
        let wet = |_p: DVec3| 0.0;
        let step = StepInput {
            dt: 0.05,
            ambient_c: 20.0,
            rain_mm_h: 0.0,
            humidity: 0.3,
            wind: &wind,
            wetness: &wet,
            params: SpreadParams::default(),
            near: None,
        };
        for z in -4..=4 {
            field.ignite(DVec3::new(0.0, 0.0, z as f64), 0.8, &mut fuel);
        }
        // The soaked strip doesn't even light.
        assert_eq!(field.ignite(DVec3::new(2.0, 0.0, 8.0), 0.5, &mut fuel), 0);
        for _ in 0..(60.0 / 0.05) as usize {
            field.step(&step, &mut fuel);
        }
        let reached = front(&field, DVec3::ZERO, DVec3::X);
        assert!(reached > 8.0, "fire should reach the break, got {reached}");
        assert!(reached < 14.0, "fire jumped the 4 m break: {reached}");
        let wet_burnt = field
            .cells_near(DVec3::ZERO, 50.0)
            .filter(|(p, c)| p.z > 7.0 && c.state() == CellState::Burnt)
            .count();
        assert_eq!(wet_burnt, 0, "soaked grass burned");
    }

    #[test]
    fn rain_puts_fire_out() {
        pools();
        let mut field = FireField::new(0.5);
        let mut fuel = grass;
        let wind = |_p: DVec3| Vec3::new(2.0, 0.0, 0.0);
        let wet = |_p: DVec3| 0.0;
        let mut step = StepInput {
            dt: 0.05,
            ambient_c: 20.0,
            rain_mm_h: 0.0,
            humidity: 0.3,
            wind: &wind,
            wetness: &wet,
            params: SpreadParams::default(),
            near: None,
        };
        field.ignite(DVec3::ZERO, 2.0, &mut fuel);
        for _ in 0..100 {
            field.step(&step, &mut fuel);
        }
        assert!(field.stats.burning_cells > 10);
        step.rain_mm_h = 40.0;
        for _ in 0..(120.0 / 0.05) as usize {
            field.step(&step, &mut fuel);
        }
        assert_eq!(field.stats.burning_cells, 0, "{:?}", field.stats);
    }

    #[test]
    fn fire_burns_out_and_regrows() {
        pools();
        let mut field = FireField::new(0.5);
        let mut fuel = |p: DVec3| {
            let (mut f, h) = grass(p);
            f.regrow_secs = 20.0;
            if p.x.abs() > 3.0 || p.z.abs() > 3.0 {
                (CellFuel::NONE, h)
            } else {
                (f, h)
            }
        };
        let wind = |_p: DVec3| Vec3::ZERO;
        let wet = |_p: DVec3| 0.0;
        let step = StepInput {
            dt: 0.1,
            ambient_c: 20.0,
            rain_mm_h: 0.0,
            humidity: 0.3,
            wind: &wind,
            wetness: &wet,
            params: SpreadParams::default(),
            near: None,
        };
        field.ignite(DVec3::ZERO, 3.0, &mut fuel);
        for _ in 0..200 {
            field.step(&step, &mut fuel);
        }
        let c = *field.cell_at(DVec3::new(0.2, 0.0, 0.2)).unwrap();
        assert_eq!(c.state(), CellState::Burnt, "{c:?}");
        assert!(c.scorch > 0.9);
        for _ in 0..250 {
            field.step(&step, &mut fuel);
        }
        let c = *field.cell_at(DVec3::new(0.2, 0.0, 0.2)).unwrap();
        assert_eq!(c.state(), CellState::Intact, "{c:?}");
        assert_eq!(c.scorch, 0.0);
    }

    #[test]
    fn heat_alone_ignites_dry_fuel() {
        pools();
        let mut field = FireField::new(0.5);
        let mut fuel = grass;
        let need = field
            .ensure_tile(IVec2::ZERO, 0.0, &mut fuel)
            .map(|i| field.tiles[i].cells[0].ignition_energy(20.0));
        field.add_heat(
            DVec3::new(4.0, 0.0, 4.0),
            1.0,
            need.unwrap() * 3.0,
            &mut fuel,
        );
        let wind = |_p: DVec3| Vec3::ZERO;
        let wet = |_p: DVec3| 0.0;
        let step = StepInput {
            dt: 0.05,
            ambient_c: 20.0,
            rain_mm_h: 0.0,
            humidity: 0.3,
            wind: &wind,
            wetness: &wet,
            params: SpreadParams::default(),
            near: None,
        };
        field.step(&step, &mut fuel);
        assert!(field.stats.burning_cells > 0);
    }
}
