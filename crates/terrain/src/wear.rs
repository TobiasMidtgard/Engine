//! Worn paths: ground that many feet, hooves and wheels cross wears into a path.
//!
//! Every [`GroundContact`](engine_world::GroundContact) adds a little wear to the half-meter cells it lands on; where the
//! same line is walked again and again the wear builds into a path, and where nobody walks it
//! heals. How fast both happen depends on the ground (see [`WornGround`]): grass is trodden
//! flat, thins and dies back to bare earth (which turns to mud in rain) and regrows over days;
//! snow packs hard within a few dozen passes and is buried again by the next snowfall; sand
//! takes a trough at once but the wind fills it within the hour; gravel is pushed aside into
//! a firm track that lasts for weeks; rock barely changes at all. Heavy feet wear more than
//! light ones, wheels more than feet, and soaked ground more than dry.
//!
//! The map is persistent and sparse: 64-cell tiles that exist only where someone has walked,
//! kept per terrain in that terrain's own frame (a path stays where it is when the world
//! re-anchors or the planet moves). Each region is a flat grid laid on the ground around the
//! place it was first walked, so a planet holds one region per few kilometers of walked land.
//! The renderer draws a window of it around the camera.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use engine_core::math::{DQuat, DVec3, IVec2};
use engine_core::prelude::*;
use engine_world::{ContactKind, Deformation as Deform, SurfaceMaterial, WearQuery, WorldShift};

/// Cells per tile side.
pub const TILE: i32 = 64;
const TILE_CELLS: usize = (TILE * TILE) as usize;

/// A region covers ground within this distance of its anchor (along its plane), meters.
/// Beyond it the planet's curve takes the grid off the ground, so another region starts.
const REGION_REACH_M: f64 = 4000.0;

/// How a kind of ground wears and heals.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[repr(u8)]
pub enum WornGround {
    /// Grass and leaf litter: trodden flat, then thinned to bare earth.
    #[default]
    Turf = 0,
    /// Dirt, mud, regolith, ash: packs down and sinks into a hollow.
    Earth = 1,
    /// Snow: packs to a hard, glossy track.
    Snow = 2,
    /// Sand: a soft trough that the wind fills in.
    Sand = 3,
    /// Gravel and scree: stones pushed aside into a firm track.
    Gravel = 4,
    /// Rock, ice, concrete, wood: polished only by long use.
    Hard = 5,
}

impl WornGround {
    /// The kind of wear a surface material takes.
    pub fn of(material: &SurfaceMaterial) -> Self {
        match material.name.as_str() {
            "grass" | "leaves" | "moss" | "turf" => Self::Turf,
            "snow" => Self::Snow,
            "sand" => Self::Sand,
            "gravel" | "scree" | "pebbles" => Self::Gravel,
            "dirt" | "mud" | "regolith" | "ash" | "soil" | "default" => Self::Earth,
            _ => match material.deformation {
                Deform::Flatten { .. } => Self::Turf,
                Deform::Snow => Self::Snow,
                Deform::Granular { .. } => Self::Sand,
                Deform::Plastic { .. } => Self::Earth,
                Deform::None => Self::Hard,
            },
        }
    }

    /// Share of the wear still to come that one 80 kg footfall adds. Dozens of passes make a
    /// path on soft ground; hundreds on turf, thousands on rock.
    pub fn gain(self) -> f32 {
        match self {
            Self::Turf => 0.012,
            Self::Earth => 0.02,
            Self::Snow => 0.05,
            Self::Sand => 0.06,
            Self::Gravel => 0.015,
            Self::Hard => 0.0008,
        }
    }

    /// Seconds for half the wear to heal where nobody walks (before
    /// [`WearSettings::heal_scale`]).
    pub fn half_life_s(self) -> f32 {
        const HOUR: f32 = 3600.0;
        match self {
            // Grass grows back over the bare earth within weeks.
            Self::Turf => 96.0 * HOUR,
            Self::Earth => 120.0 * HOUR,
            // Snow settles and sublimates; new snow fills it much faster (see `heal`).
            Self::Snow => 48.0 * HOUR,
            // Blowing sand fills a trough within the hour.
            Self::Sand => 0.5 * HOUR,
            Self::Gravel => 400.0 * HOUR,
            Self::Hard => 5000.0 * HOUR,
        }
    }
}

/// Settings for worn paths.
#[derive(Resource, Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct WearSettings {
    pub enabled: bool,
    /// Cell size, meters.
    pub cell_m: f32,
    /// Multiplies every gain: above 1 a path forms with fewer passes (a quick demo).
    pub gain_scale: f32,
    /// Multiplies every healing half-life: below 1 paths fade sooner.
    pub heal_scale: f32,
    /// Cells per side of the window the renderer draws around the camera.
    pub window: u32,
}

impl Default for WearSettings {
    fn default() -> Self {
        Self {
            enabled: true,
            cell_m: 0.5,
            gain_scale: 1.0,
            heal_scale: 1.0,
            window: 1024,
        }
    }
}

/// Wear and ground of one tile's cells.
#[derive(Debug, Clone)]
struct WearTile {
    wear: Vec<f32>,
    ground: Vec<WornGround>,
}

impl WearTile {
    fn new() -> Self {
        Self {
            wear: vec![0.0; TILE_CELLS],
            ground: vec![WornGround::Turf; TILE_CELLS],
        }
    }
}

/// A flat grid laid on a terrain around where it was first walked, in the terrain's frame.
#[derive(Debug, Clone)]
pub struct WearRegion {
    /// Terrain-local corner of cell (0, 0), meters.
    pub anchor: DVec3,
    /// Terrain-local unit axes of the grid's columns and rows, and its up.
    pub axis_u: DVec3,
    pub axis_v: DVec3,
    pub up: DVec3,
    /// (Shared with the snapshots `GroundWear` answers from; copied when written.)
    tiles: HashMap<IVec2, Arc<WearTile>>,
}

impl WearRegion {
    /// A region whose plane touches the ground at `at` (terrain-local) with `up` its normal.
    pub fn new(at: DVec3, up: DVec3) -> Self {
        let up = up.normalize();
        // Columns run as close to the terrain's +X as the plane allows.
        let mut axis_u = DVec3::X - up * up.x;
        if axis_u.length_squared() < 1e-6 {
            axis_u = DVec3::Z - up * up.z;
        }
        let axis_u = axis_u.normalize();
        let axis_v = up.cross(axis_u).normalize();
        Self {
            anchor: at,
            axis_u,
            axis_v,
            up,
            tiles: HashMap::new(),
        }
    }

    /// Grid coordinates (in cells) of a terrain-local point.
    pub fn cell_of(&self, local: DVec3, cell_m: f32) -> (f64, f64) {
        let d = local - self.anchor;
        (
            d.dot(self.axis_u) / cell_m as f64,
            d.dot(self.axis_v) / cell_m as f64,
        )
    }

    fn covers(&self, local: DVec3) -> bool {
        let d = local - self.anchor;
        let along = d - self.up * d.dot(self.up);
        along.length() < REGION_REACH_M && d.dot(self.up).abs() < 300.0
    }

    /// Wear of one cell.
    pub fn cell(&self, x: i32, y: i32) -> f32 {
        let t = IVec2::new(x.div_euclid(TILE), y.div_euclid(TILE));
        self.tiles.get(&t).map_or(0.0, |tile| {
            tile.wear[(y.rem_euclid(TILE) * TILE + x.rem_euclid(TILE)) as usize]
        })
    }

    /// Wear at a terrain-local point, blended between the four nearest cells.
    pub fn sample(&self, local: DVec3, cell_m: f32) -> f32 {
        let (u, v) = self.cell_of(local, cell_m);
        let (u, v) = (u - 0.5, v - 0.5);
        let (x, y) = (u.floor(), v.floor());
        let (fx, fy) = ((u - x) as f32, (v - y) as f32);
        let (x, y) = (x as i32, y as i32);
        let a = self.cell(x, y) * (1.0 - fx) + self.cell(x + 1, y) * fx;
        let b = self.cell(x, y + 1) * (1.0 - fx) + self.cell(x + 1, y + 1) * fx;
        a * (1.0 - fy) + b * fy
    }

    /// Tiles that hold wear.
    pub fn tile_keys(&self) -> impl Iterator<Item = IVec2> + '_ {
        self.tiles.keys().copied()
    }

    /// Wear of every cell of a tile, row by row (`None` where nobody has walked).
    pub fn tile(&self, key: IVec2) -> Option<&[f32]> {
        self.tiles.get(&key).map(|t| t.wear.as_slice())
    }
}

/// Worn ground of every terrain.
#[derive(Resource, Debug, Default)]
pub struct PathWear {
    pub cell_m: f32,
    pub terrains: HashMap<Entity, Vec<WearRegion>>,
    /// Tiles changed since the renderer last looked: (terrain, region, tile).
    pub dirty: HashSet<(Entity, usize, IVec2)>,
    /// Bumped whenever regions are added or cleared (the renderer redraws its window).
    pub generation: u64,
    /// Bumped on every change (the `GroundWear` snapshot is refreshed when it moves).
    pub revision: u64,
}

impl PathWear {
    pub fn new(cell_m: f32) -> Self {
        Self {
            cell_m,
            ..Default::default()
        }
    }

    /// The region of `terrain` that covers a terrain-local point, if any.
    pub fn region_at(&self, terrain: Entity, local: DVec3) -> Option<(usize, &WearRegion)> {
        self.terrains
            .get(&terrain)?
            .iter()
            .enumerate()
            .find(|(_, r)| r.covers(local))
    }

    /// The region of `terrain` to draw around a terrain-local point (the camera): the nearest
    /// one along the ground, if it is within sight of its paths.
    pub fn region_near(&self, terrain: Entity, local: DVec3) -> Option<(usize, &WearRegion)> {
        self.terrains
            .get(&terrain)?
            .iter()
            .enumerate()
            .map(|(i, r)| {
                let d = local - r.anchor;
                let height = d.dot(r.up);
                (i, r, (d - r.up * height).length(), height.abs())
            })
            .filter(|&(_, _, along, height)| along < REGION_REACH_M + 2000.0 && height < 3000.0)
            .min_by(|a, b| a.2.total_cmp(&b.2))
            .map(|(i, r, _, _)| (i, r))
    }

    /// Wear at a terrain-local point: 0 untouched .. 1 a well-worn path.
    pub fn wear_at(&self, terrain: Entity, local: DVec3) -> f32 {
        self.region_at(terrain, local)
            .map_or(0.0, |(_, r)| r.sample(local, self.cell_m))
    }

    /// One footfall (or wheel, or hoof) on a terrain: wears the cells under the contact.
    /// `local` and `up` are terrain-local (the contact point and the ground's up there),
    /// `size` the contact's width and length in meters, `load_kg` its weight and `wetness`
    /// how soaked the ground is (0..1).
    #[allow(clippy::too_many_arguments)]
    pub fn tread(
        &mut self,
        terrain: Entity,
        local: DVec3,
        up: DVec3,
        size: [f32; 2],
        load_kg: f32,
        kind: ContactKind,
        ground: WornGround,
        wetness: f32,
        gain_scale: f32,
    ) {
        if kind == ContactKind::Body {
            return;
        }
        let cell_m = self.cell_m;
        let regions = self.terrains.entry(terrain).or_default();
        let index = match regions.iter().position(|r| r.covers(local)) {
            Some(i) => i,
            None => {
                regions.push(WearRegion::new(local, up));
                self.generation += 1;
                regions.len() - 1
            }
        };
        let region = &mut regions[index];
        // Heavier contacts wear more, less than in proportion (a horse is not six people).
        let weight = (load_kg.max(1.0) / 80.0).powf(0.6).clamp(0.2, 8.0);
        let kind_k = match kind {
            ContactKind::Wheel => 2.0,
            ContactKind::Track => 3.0,
            ContactKind::Hoof => 1.4,
            _ => 1.0,
        };
        let soft = match ground {
            WornGround::Turf | WornGround::Earth => 1.0 + 1.5 * wetness.clamp(0.0, 1.0),
            _ => 1.0,
        };
        let gain = (ground.gain() * weight * kind_k * soft * gain_scale).min(0.9);
        // Spread over the cells the contact covers; a footprint smaller than a cell shares
        // itself between the four nearest.
        let (u, v) = region.cell_of(local, cell_m);
        let reach = (size[0].max(size[1]) * 0.5 / cell_m).max(0.5) as f64;
        let (x0, x1) = ((u - reach).floor() as i32, (u + reach).ceil() as i32);
        let (y0, y1) = ((v - reach).floor() as i32, (v + reach).ceil() as i32);
        for y in y0..y1 {
            for x in x0..x1 {
                // Overlap of the contact's square with this cell.
                let ox = ((u + reach).min(x as f64 + 1.0) - (u - reach).max(x as f64)).max(0.0);
                let oy = ((v + reach).min(y as f64 + 1.0) - (v - reach).max(y as f64)).max(0.0);
                // (In cells: a cell the contact covers takes the full gain.)
                let share = (ox * oy) as f32;
                if share <= 1e-4 {
                    continue;
                }
                let key = IVec2::new(x.div_euclid(TILE), y.div_euclid(TILE));
                let tile = Arc::make_mut(
                    region
                        .tiles
                        .entry(key)
                        .or_insert_with(|| Arc::new(WearTile::new())),
                );
                let i = (y.rem_euclid(TILE) * TILE + x.rem_euclid(TILE)) as usize;
                let g = gain * share.min(1.0);
                let w = &mut tile.wear[i];
                *w += g * (1.0 - *w);
                tile.ground[i] = ground;
                self.dirty.insert((terrain, index, key));
                self.revision += 1;
            }
        }
    }

    /// Lets every worn cell heal for `dt` seconds. `snowfall_m_s`: fresh snow falling (meters
    /// per second), which buries packed snow.
    pub fn heal(&mut self, dt: f32, snowfall_m_s: f32, heal_scale: f32) {
        let terrains: Vec<(Entity, usize)> = self
            .terrains
            .iter()
            .flat_map(|(&e, r)| (0..r.len()).map(move |i| (e, i)))
            .collect();
        for (terrain, index) in terrains {
            self.heal_region(terrain, index, dt, snowfall_m_s, heal_scale);
        }
    }

    /// [`heal`](Self::heal) for one region only.
    fn heal_region(
        &mut self,
        terrain: Entity,
        index: usize,
        dt: f32,
        snowfall_m_s: f32,
        heal_scale: f32,
    ) {
        let scale = heal_scale.max(1e-6);
        let rates: [f32; 6] = std::array::from_fn(|k| {
            let ground = [
                WornGround::Turf,
                WornGround::Earth,
                WornGround::Snow,
                WornGround::Sand,
                WornGround::Gravel,
                WornGround::Hard,
            ][k];
            (-dt / (ground.half_life_s() * scale)).exp2()
        });
        // Ten centimeters of new snow buries a packed track.
        let buried = snowfall_m_s * dt * 10.0;
        let Some(region) = self
            .terrains
            .get_mut(&terrain)
            .and_then(|r| r.get_mut(index))
        else {
            return;
        };
        let dirty = &mut self.dirty;
        region.tiles.retain(|&key, tile| {
            let tile = Arc::make_mut(tile);
            let mut peak = 0.0f32;
            for (w, g) in tile.wear.iter_mut().zip(&tile.ground) {
                if *w > 0.0 {
                    *w *= rates[*g as usize];
                    if *g == WornGround::Snow {
                        *w = (*w - buried).max(0.0);
                    }
                    if *w < 1e-3 {
                        *w = 0.0;
                    }
                    peak = peak.max(*w);
                }
            }
            dirty.insert((terrain, index, key));
            peak > 0.0
        });
        self.revision += 1;
    }

    /// Forget every path of a terrain (it was removed).
    pub fn forget(&mut self, terrain: Entity) {
        if self.terrains.remove(&terrain).is_some() {
            self.generation += 1;
            self.dirty.retain(|d| d.0 != terrain);
        }
    }

    /// Walk a line many times at once: `passes` people of `load_kg` walking from `from` to
    /// `to` (terrain-local, on the ground; `up` the ground's up), a step every `step_m` and
    /// wandering up to `spread_m` to either side. For tests, demos and scripted scenes.
    #[allow(clippy::too_many_arguments)]
    pub fn walk_line(
        &mut self,
        terrain: Entity,
        from: DVec3,
        to: DVec3,
        up: DVec3,
        passes: u32,
        load_kg: f32,
        ground: WornGround,
        spread_m: f32,
        seed: u32,
    ) {
        let len = from.distance(to);
        if len < 1e-3 {
            return;
        }
        let dir = (to - from) / len;
        let side = up.cross(dir).normalize_or_zero();
        let step = 0.7;
        let steps = (len / step).ceil() as u32;
        for p in 0..passes {
            // Each walker keeps to their own line, a little to one side.
            // (Most near the line, a few well to the side.)
            let lane = (hash(seed, p, 0) + hash(seed, p, u32::MAX) - 1.0) * 1.6 * spread_m as f64;
            let phase = hash(seed, p, 1) * step;
            for s in 0..steps {
                let along = phase + s as f64 * step;
                if along > len {
                    break;
                }
                // Feet alternate a hand's width either side of the walker's line.
                let foot = if s % 2 == 0 { 0.12 } else { -0.12 };
                let sway = (hash(seed, p, s + 2) - 0.5) * 0.15;
                let at = from + dir * along + side * (lane + foot + sway);
                self.tread(
                    terrain,
                    at,
                    up,
                    [0.11, 0.27],
                    load_kg,
                    ContactKind::Foot,
                    ground,
                    0.0,
                    1.0,
                );
            }
        }
    }

    /// Walkers going back and forth between `points` (terrain-local, on the ground; `up` the
    /// ground's up), `trips` trips in all, each from one point to another picked at random,
    /// in proportion to how busy each is (the `f32` beside it).
    /// Like people, each takes the way that feels shortest: straight across the grass while
    /// nothing is worn, but along a worn path where it runs their way, since walking a path
    /// is easier than wading through grass (see [`TrailWalk::attraction`]). Paths nobody uses
    /// heal between trips. So the first walkers' lines are reinforced or forgotten, and a few
    /// shared trails emerge that join and part at forks, instead of a straight line between
    /// every pair. Only the region under the points heals.
    #[allow(clippy::too_many_arguments)]
    pub fn walk_between(
        &mut self,
        terrain: Entity,
        points: &[(DVec3, f32)],
        up: DVec3,
        trips: u32,
        load_kg: f32,
        ground: WornGround,
        walk: TrailWalk,
        seed: u32,
    ) {
        if points.len() < 2 || trips == 0 {
            return;
        }
        let up = up.normalize_or_zero();
        // Make sure the region exists (and find it) before the first trip.
        self.tread(
            terrain,
            points[0].0,
            up,
            [0.0, 0.0],
            1.0,
            ContactKind::Foot,
            ground,
            0.0,
            0.0,
        );
        let Some((index, region)) = self.region_at(terrain, points[0].0) else {
            return;
        };
        let places: Vec<DVec3> = points.iter().map(|p| p.0).collect();
        let mut map = RouteMap::new(region, &places, self.cell_m);
        let day_s = 86_400.0 / walk.trips_per_day.max(1e-3);
        let refresh = 10u32;
        for trip in 0..trips {
            if trip % refresh == 0 {
                if trip > 0 {
                    self.heal_region(terrain, index, day_s * refresh as f32, 0.0, walk.heal_scale);
                }
                map.read(&self.terrains[&terrain][index]);
            }
            // Two different places, the busier ones more often.
            let pick = |r: f64, skip: Option<usize>| {
                let total: f64 = (0..points.len())
                    .filter(|&i| Some(i) != skip)
                    .map(|i| points[i].1.max(0.0) as f64)
                    .sum();
                let mut left = r * total;
                let mut last = 0;
                for i in (0..points.len()).filter(|&i| Some(i) != skip) {
                    last = i;
                    left -= points[i].1.max(0.0) as f64;
                    if left < 0.0 {
                        break;
                    }
                }
                last
            };
            let a = pick(hash(seed, trip, 0), None);
            let b = pick(hash(seed, trip, 1), Some(a));
            if a == b {
                continue;
            }
            // Everyone starts and ends a step or two from the exact spot.
            let side = up.any_orthonormal_vector();
            let other = up.cross(side);
            let jitter = |k: u32| {
                side * (hash(seed, trip, k) - 0.5) * 2.0
                    + other * (hash(seed, trip, k + 1) - 0.5) * 2.0
            };
            let from = points[a].0 + jitter(2);
            let to = points[b].0 + jitter(4);
            let route = map.route(from, to, walk.attraction, seed ^ trip.wrapping_mul(0x9E37));
            // Each walker keeps a little to one side of the way (most near it, a few well off).
            let lane =
                (hash(seed, trip, 6) + hash(seed, trip, 7) - 1.0) * 1.6 * walk.spread_m as f64;
            // Steps along the way, eased round its corners as a walker cuts them (over a few
            // meters, so trails join and part in curves).
            let step = 0.7;
            let mut steps = Vec::new();
            let mut along = hash(seed, trip, 8) * step;
            for pair in route.windows(2) {
                let len = pair[0].distance(pair[1]);
                while along < len {
                    steps.push(pair[0].lerp(pair[1], along / len));
                    along += step;
                }
                along -= len;
            }
            steps.push(to);
            for _ in 0..2 {
                steps = (0..steps.len())
                    .map(|i| {
                        let k = 4.min(i).min(steps.len() - 1 - i);
                        let (mut sum, mut total) = (DVec3::ZERO, 0.0);
                        for (j, &p) in steps.iter().enumerate().take(i + k + 1).skip(i - k) {
                            let wgt = (k + 1 - j.abs_diff(i)) as f64;
                            sum += p * wgt;
                            total += wgt;
                        }
                        sum / total
                    })
                    .collect();
            }
            for (s, pair) in steps.windows(2).enumerate() {
                let dir = (pair[1] - pair[0]).normalize_or_zero();
                let right = up.cross(dir).normalize_or_zero();
                // On a path, feet find its trodden middle (which keeps a path narrow).
                let region = &self.terrains[&terrain][index];
                let (mut pull, mut total) = (0.0, 0.0);
                for k in -4..=4 {
                    let off = k as f64 * 0.25;
                    let w = region.sample(pair[0] + right * (lane + off), self.cell_m) as f64;
                    pull += w * w * off;
                    total += w * w;
                }
                let lane = if total > 0.05 {
                    lane + pull / total * 0.8
                } else {
                    lane
                };
                // Feet alternate a hand's width either side of the walker's line.
                let foot = if s % 2 == 0 { 0.12 } else { -0.12 };
                let sway = (hash(seed, trip, s as u32 + 9) - 0.5) * 0.15;
                self.tread(
                    terrain,
                    pair[0] + right * (lane + foot + sway),
                    up,
                    [0.11, 0.27],
                    load_kg,
                    ContactKind::Foot,
                    ground,
                    0.0,
                    1.0,
                );
            }
        }
    }
}

/// How far off walkers see a path and drift toward it (meters).
const SIGHT_M: f64 = 6.0;
/// How much a walker's whim weighs against the way's length, and over how many meters it
/// changes.
const WHIM: f64 = 0.25;
const WHIM_M: f64 = 15.0;

/// The ground between a set of points as walkers judge it: a coarse grid of how worn each
/// patch is, to find the way that feels shortest.
struct RouteMap {
    anchor: DVec3,
    axis_u: DVec3,
    axis_v: DVec3,
    /// Grid corner in region cells, and region cells per node.
    corner: IVec2,
    per_node: i32,
    node_m: f64,
    size: IVec2,
    wear: Vec<f32>,
}

impl RouteMap {
    fn new(region: &WearRegion, points: &[DVec3], cell_m: f32) -> Self {
        let cells: Vec<(f64, f64)> = points.iter().map(|&p| region.cell_of(p, cell_m)).collect();
        let (mut lo, mut hi) = ((f64::MAX, f64::MAX), (f64::MIN, f64::MIN));
        for &(u, v) in &cells {
            lo = (lo.0.min(u), lo.1.min(v));
            hi = (hi.0.max(u), hi.1.max(v));
        }
        let span = (hi.0 - lo.0).max(hi.1 - lo.1) * cell_m as f64;
        // Room to swing wide of the points, and nodes no finer than a stride but few enough
        // to search quickly.
        let margin = (span * 0.2).max(20.0) / cell_m as f64;
        let node_m = (span / 160.0).max(1.5);
        let per_node = ((node_m / cell_m as f64).round() as i32).max(1);
        let corner = IVec2::new(
            (lo.0 - margin).floor() as i32,
            (lo.1 - margin).floor() as i32,
        );
        let size = IVec2::new(
            ((hi.0 - lo.0 + 2.0 * margin) / per_node as f64).ceil() as i32 + 1,
            ((hi.1 - lo.1 + 2.0 * margin) / per_node as f64).ceil() as i32 + 1,
        );
        Self {
            anchor: region.anchor,
            axis_u: region.axis_u,
            axis_v: region.axis_v,
            corner,
            per_node,
            node_m: per_node as f64 * cell_m as f64,
            size,
            wear: vec![0.0; (size.x * size.y) as usize],
        }
    }

    /// Read the wear under every node: the most worn of its cells (a path narrower than a
    /// node still counts).
    fn read(&mut self, region: &WearRegion) {
        for y in 0..self.size.y {
            for x in 0..self.size.x {
                let (cx, cy) = (
                    self.corner.x + x * self.per_node,
                    self.corner.y + y * self.per_node,
                );
                let mut w = 0.0f32;
                for j in 0..self.per_node {
                    for i in 0..self.per_node {
                        w = w.max(region.cell(cx + i, cy + j));
                    }
                }
                // A path draws walkers once it shows: faint wear goes unnoticed.
                let t = ((w - 0.1) / 0.4).clamp(0.0, 1.0);
                self.wear[(y * self.size.x + x) as usize] = t * t * (3.0 - 2.0 * t);
            }
        }
        // Walkers see a path from a few meters off and drift toward it: near a path feels
        // nearly as easy as on it, fading out over `SIGHT_M` (a sweep each way spreads every
        // path's wear outward, less with distance).
        let (sx, sy) = (self.size.x, self.size.y);
        let fall = (self.node_m / SIGHT_M) as f32;
        let diag = fall * std::f32::consts::SQRT_2;
        let reach = [(-1, 0, fall), (-1, -1, diag), (0, -1, fall), (1, -1, diag)];
        for y in 0..sy {
            for x in 0..sx {
                let i = (y * sx + x) as usize;
                for &(dx, dy, f) in &reach {
                    let (nx, ny) = (x + dx, y + dy);
                    if nx >= 0 && ny >= 0 && nx < sx {
                        let n = self.wear[(ny * sx + nx) as usize] - f;
                        self.wear[i] = self.wear[i].max(n);
                    }
                }
            }
        }
        for y in (0..sy).rev() {
            for x in (0..sx).rev() {
                let i = (y * sx + x) as usize;
                for &(dx, dy, f) in &reach {
                    let (nx, ny) = (x - dx, y - dy);
                    if nx >= 0 && ny < sy && nx < sx {
                        let n = self.wear[(ny * sx + nx) as usize] - f;
                        self.wear[i] = self.wear[i].max(n);
                    }
                }
            }
        }
    }

    fn node_of(&self, p: DVec3) -> IVec2 {
        let d = p - self.anchor;
        let cell = self.per_node as f64 / self.node_m;
        let u = d.dot(self.axis_u) * cell - self.corner.x as f64;
        let v = d.dot(self.axis_v) * cell - self.corner.y as f64;
        IVec2::new(
            ((u / self.per_node as f64).floor() as i32).clamp(0, self.size.x - 1),
            ((v / self.per_node as f64).floor() as i32).clamp(0, self.size.y - 1),
        )
    }

    fn point_of(&self, n: IVec2, height_from: DVec3) -> DVec3 {
        let cell_m = self.node_m / self.per_node as f64;
        let u = (self.corner.x as f64 + (n.x as f64 + 0.5) * self.per_node as f64) * cell_m;
        let v = (self.corner.y as f64 + (n.y as f64 + 0.5) * self.per_node as f64) * cell_m;
        let d = height_from - self.anchor;
        let flat = d - self.axis_u * d.dot(self.axis_u) - self.axis_v * d.dot(self.axis_v);
        self.anchor + flat + self.axis_u * u + self.axis_v * v
    }

    /// The way from `from` to `to` that feels shortest when a well-worn path feels
    /// `attraction` shorter than open ground (0..0.9), as a walker's line.
    fn route(&self, from: DVec3, to: DVec3, attraction: f32, seed: u32) -> Vec<DVec3> {
        use std::cmp::Reverse;
        use std::collections::BinaryHeap;
        let c = attraction.clamp(0.0, 0.9) as f64;
        let (start, goal) = (self.node_of(from), self.node_of(to));
        let (sx, sy) = (self.size.x, self.size.y);
        let idx = |n: IVec2| (n.y * sx + n.x) as usize;
        // What a meter costs at each node. Each walker's whim adds to it: off the paths
        // nobody keeps to one line, so their ways wander over a few meters (smooth over
        // `WHIM_M`), while a path that shows holds them.
        let lattice = (WHIM_M / self.node_m).max(1.0);
        let knots_x = (sx as f64 / lattice).ceil() as usize + 2;
        let knots: Vec<f64> = (0..knots_x * ((sy as f64 / lattice).ceil() as usize + 2))
            .map(|k| hash(seed, (k % knots_x) as u32, (k / knots_x) as u32))
            .collect();
        let mut cost = vec![0.0f64; self.wear.len()];
        for y in 0..sy {
            let v = y as f64 / lattice;
            let (ky, fy) = (v.floor() as usize, v.fract());
            let fy = fy * fy * (3.0 - 2.0 * fy);
            for x in 0..sx {
                let u = x as f64 / lattice;
                let (kx, fx) = (u.floor() as usize, u.fract());
                let fx = fx * fx * (3.0 - 2.0 * fx);
                let k = ky * knots_x + kx;
                let a = knots[k] * (1.0 - fx) + knots[k + 1] * fx;
                let b = knots[k + knots_x] * (1.0 - fx) + knots[k + knots_x + 1] * fx;
                let i = (y * sx + x) as usize;
                cost[i] = 1.0 - c * self.wear[i] as f64 + (a * (1.0 - fy) + b * fy) * WHIM;
            }
        }
        let floor = 1.0 - c;
        let h = |n: IVec2| (n - goal).as_dvec2().length() * floor;
        const MOVES: [(i32, i32); 16] = [
            (1, 0),
            (-1, 0),
            (0, 1),
            (0, -1),
            (1, 1),
            (1, -1),
            (-1, 1),
            (-1, -1),
            (2, 1),
            (2, -1),
            (-2, 1),
            (-2, -1),
            (1, 2),
            (1, -2),
            (-1, 2),
            (-1, -2),
        ];
        let mut best = vec![f64::INFINITY; cost.len()];
        let mut came = vec![u32::MAX; cost.len()];
        let mut open = BinaryHeap::new();
        best[idx(start)] = 0.0;
        open.push((Reverse(OrdF64(h(start))), start.x, start.y));
        while let Some((Reverse(OrdF64(f)), x, y)) = open.pop() {
            let n = IVec2::new(x, y);
            if n == goal {
                break;
            }
            let g = best[idx(n)];
            if f > g + h(n) + 1e-9 {
                continue;
            }
            for (dx, dy) in MOVES {
                let m = IVec2::new(x + dx, y + dy);
                if m.x < 0 || m.y < 0 || m.x >= sx || m.y >= sy {
                    continue;
                }
                let len = ((dx * dx + dy * dy) as f64).sqrt();
                // (A knight's move crosses the two nodes between too.)
                let per_m = if dx.abs() + dy.abs() == 3 {
                    let a = IVec2::new(x + dx / 2, y + dy / 2);
                    let b = IVec2::new(x + dx - dx / 2, y + dy - dy / 2);
                    (cost[idx(n)] + cost[idx(a)] + cost[idx(b)] + cost[idx(m)]) * 0.25
                } else {
                    (cost[idx(n)] + cost[idx(m)]) * 0.5
                };
                let ng = g + len * per_m;
                if ng < best[idx(m)] {
                    best[idx(m)] = ng;
                    came[idx(m)] = idx(n) as u32;
                    open.push((Reverse(OrdF64(ng + h(m))), m.x, m.y));
                }
            }
        }
        let mut nodes = vec![goal];
        let mut at = idx(goal);
        while came[at] != u32::MAX {
            at = came[at] as usize;
            nodes.push(IVec2::new(at as i32 % sx, at as i32 / sx));
        }
        nodes.reverse();
        // Pull the grid's way taut: from each turn, straight on to the farthest node the
        // straight line reaches for no more than the grid's way costs. (The grid alone
        // prefers its own directions, and walkers would lay paths along them.)
        let line_cost = |a: IVec2, b: IVec2| {
            let d = (b - a).as_dvec2();
            let len = d.length();
            let n = (len * 2.0).ceil().max(1.0) as i32;
            let mut sum = 0.0;
            for k in 0..=n {
                let p = a.as_dvec2() + d * (k as f64 / n as f64);
                let q = IVec2::new(p.x.round() as i32, p.y.round() as i32);
                let wgt = if k == 0 || k == n { 0.5 } else { 1.0 };
                sum += cost[idx(q)] * wgt;
            }
            sum / n as f64 * len
        };
        let mut taut = vec![nodes[0]];
        let mut i = 0;
        while i + 1 < nodes.len() {
            let mut j = i + 1;
            while j + 1 < nodes.len() {
                let grid = best[idx(nodes[j + 1])] - best[idx(nodes[i])];
                if line_cost(nodes[i], nodes[j + 1]) > grid * 1.002 + 1e-6 {
                    break;
                }
                j += 1;
            }
            taut.push(nodes[j]);
            i = j;
        }
        let mut line: Vec<DVec3> = Vec::with_capacity(taut.len() + 2);
        line.push(from);
        for &n in taut.iter().skip(1).take(taut.len().saturating_sub(2)) {
            line.push(self.point_of(n, from));
        }
        line.push(to);
        line
    }
}

/// An `f64` that sorts (no NaNs here).
#[derive(PartialEq)]
struct OrdF64(f64);
impl Eq for OrdF64 {}
impl PartialOrd for OrdF64 {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}
impl Ord for OrdF64 {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.0.total_cmp(&other.0)
    }
}

/// How the walkers of [`PathWear::walk_between`] choose their way.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct TrailWalk {
    /// How much shorter a well-worn path feels than open ground (0..0.9): 0 nobody keeps to
    /// a path (and none forms); 0.5 a walker keeps to a path that runs up to 60° off their
    /// way rather than cut across.
    pub attraction: f32,
    /// How far walkers keep to either side of their way (meters).
    pub spread_m: f32,
    /// Trips a day: how much time passes between trips, for healing.
    pub trips_per_day: f32,
    /// Multiplies the healing half-lives.
    pub heal_scale: f32,
}

impl Default for TrailWalk {
    fn default() -> Self {
        Self {
            attraction: 0.5,
            spread_m: 0.25,
            trips_per_day: 200.0,
            heal_scale: 1.0,
        }
    }
}

/// What [`GroundWear`](engine_world::GroundWear) answers from: the wear map as it was at one
/// moment, with each terrain's placement then. Tiles are shared with the live map until it
/// writes them, so taking one costs little.
#[derive(Debug, Clone, Default)]
pub struct WearSnapshot {
    cell_m: f32,
    terrains: Vec<(DVec3, DQuat, Vec<WearRegion>)>,
}

impl WearSnapshot {
    /// The map now, with each terrain at the world position and rotation `placement` gives
    /// (terrains it has none for are left out).
    pub fn of(wear: &PathWear, placement: impl Fn(Entity) -> Option<(DVec3, DQuat)>) -> Self {
        Self {
            cell_m: wear.cell_m,
            terrains: wear
                .terrains
                .iter()
                .filter_map(|(e, regions)| {
                    let (p, r) = placement(*e)?;
                    Some((p, r, regions.clone()))
                })
                .collect(),
        }
    }

    /// The same map after the world re-anchored.
    pub fn shifted(&self, s: &WorldShift) -> Self {
        Self {
            cell_m: self.cell_m,
            terrains: self
                .terrains
                .iter()
                .map(|(p, r, regions)| (s.point(*p), s.rotation * *r, regions.clone()))
                .collect(),
        }
    }
}

impl WearQuery for WearSnapshot {
    fn wear(&self, p: DVec3) -> f32 {
        for (position, rotation, regions) in &self.terrains {
            let local = rotation.inverse() * (p - *position);
            if let Some(r) = regions.iter().find(|r| r.covers(local)) {
                return r.sample(local, self.cell_m);
            }
        }
        0.0
    }
}

/// A repeatable random number in 0..1.
fn hash(seed: u32, a: u32, b: u32) -> f64 {
    let mut h = seed
        .wrapping_mul(0x9E37_79B9)
        .wrapping_add(a.wrapping_mul(0x85EB_CA6B))
        .wrapping_add(b.wrapping_mul(0xC2B2_AE35));
    h ^= h >> 15;
    h = h.wrapping_mul(0x2C1B_3C6D);
    h ^= h >> 12;
    h = h.wrapping_mul(0x297A_2D39);
    h ^= h >> 15;
    h as f64 / u32::MAX as f64
}

#[cfg(test)]
mod tests {
    use super::*;

    fn walked(ground: WornGround, passes: u32) -> f32 {
        let mut wear = PathWear::new(0.5);
        let e = Entity::PLACEHOLDER;
        wear.walk_line(
            e,
            DVec3::new(-20.0, 0.0, 0.0),
            DVec3::new(20.0, 0.0, 0.0),
            DVec3::Y,
            passes,
            80.0,
            ground,
            0.25,
            7,
        );
        wear.wear_at(e, DVec3::ZERO)
    }

    #[test]
    fn one_walker_leaves_no_path_but_a_crowd_does() {
        let once = walked(WornGround::Turf, 1);
        let crowd = walked(WornGround::Turf, 400);
        assert!(once < 0.03, "one pass wore {once}");
        assert!(crowd > 0.6, "400 passes wore only {crowd}");
    }

    #[test]
    fn soft_ground_wears_faster_than_hard() {
        let snow = walked(WornGround::Snow, 40);
        let turf = walked(WornGround::Turf, 40);
        let rock = walked(WornGround::Hard, 40);
        assert!(
            snow > turf && turf > rock,
            "snow {snow} turf {turf} rock {rock}"
        );
        assert!(rock < 0.02);
    }

    #[test]
    fn the_path_is_narrow() {
        let mut wear = PathWear::new(0.5);
        let e = Entity::PLACEHOLDER;
        wear.walk_line(
            e,
            DVec3::new(-20.0, 0.0, 0.0),
            DVec3::new(20.0, 0.0, 0.0),
            DVec3::Y,
            300,
            80.0,
            WornGround::Turf,
            0.25,
            3,
        );
        let middle = wear.wear_at(e, DVec3::ZERO);
        let aside = wear.wear_at(e, DVec3::new(0.0, 0.0, 2.0));
        assert!(
            middle > 0.5 && aside < 0.02,
            "middle {middle}, 2 m aside {aside}"
        );
    }

    #[test]
    fn sand_heals_within_hours_and_turf_takes_days() {
        let mut wear = PathWear::new(0.5);
        let e = Entity::PLACEHOLDER;
        for (z, ground) in [(0.0, WornGround::Sand), (10.0, WornGround::Turf)] {
            wear.walk_line(
                e,
                DVec3::new(-5.0, 0.0, z),
                DVec3::new(5.0, 0.0, z),
                DVec3::Y,
                200,
                80.0,
                ground,
                0.1,
                1,
            );
        }
        for _ in 0..(3 * 60) {
            wear.heal(60.0, 0.0, 1.0);
        }
        let sand = wear.wear_at(e, DVec3::ZERO);
        let turf = wear.wear_at(e, DVec3::new(0.0, 0.0, 10.0));
        assert!(sand < 0.05, "sand after 3 h: {sand}");
        assert!(turf > 0.5, "turf after 3 h: {turf}");
    }

    #[test]
    fn snowfall_buries_a_packed_track() {
        let mut wear = PathWear::new(0.5);
        let e = Entity::PLACEHOLDER;
        wear.walk_line(
            e,
            DVec3::new(-5.0, 0.0, 0.0),
            DVec3::new(5.0, 0.0, 0.0),
            DVec3::Y,
            60,
            80.0,
            WornGround::Snow,
            0.1,
            1,
        );
        assert!(wear.wear_at(e, DVec3::ZERO) > 0.5);
        // 15 cm of snow over an hour.
        for _ in 0..60 {
            wear.heal(60.0, 0.15 / 3600.0, 1.0);
        }
        assert!(wear.wear_at(e, DVec3::ZERO) < 0.05);
        assert!(
            wear.terrains[&e][0].tiles.is_empty(),
            "empty tiles are dropped"
        );
    }

    #[test]
    fn far_apart_paths_get_their_own_regions() {
        let mut wear = PathWear::new(0.5);
        let e = Entity::PLACEHOLDER;
        let r = 6_000_000.0;
        for (i, dir) in [DVec3::Y, DVec3::new(0.0, 0.99, 0.14).normalize()]
            .into_iter()
            .enumerate()
        {
            let at = dir * r;
            wear.walk_line(
                e,
                at,
                at + dir.any_orthonormal_vector() * 4.0,
                dir,
                50,
                80.0,
                WornGround::Earth,
                0.1,
                i as u32,
            );
        }
        assert_eq!(wear.terrains[&e].len(), 2);
        let mid = DVec3::Y * r + DVec3::Y.any_orthonormal_vector() * 2.0;
        assert!(wear.wear_at(e, mid) > 0.15, "{}", wear.wear_at(e, mid));
    }

    /// Bands of worn ground (wear above 0.3) crossing a line across the way.
    fn bands(wear: &PathWear, e: Entity, along: f64) -> usize {
        let mut bands = 0;
        let mut inside = false;
        for k in -60..=60 {
            let w = wear.wear_at(e, DVec3::new(k as f64 * 0.25, 0.0, along));
            if w > 0.3 && !inside {
                bands += 1;
            }
            inside = w > 0.3;
        }
        bands
    }

    #[test]
    fn walkers_share_a_trail_toward_places_in_the_same_direction() {
        // From a farm to two places 14° apart, and back: straight lines would be two paths
        // 8 m apart halfway; walkers who keep to paths wear one shared trail that forks.
        let e = Entity::PLACEHOLDER;
        let places = [
            (DVec3::ZERO, 2.0),
            (DVec3::new(-8.0, 0.0, 65.0), 1.0),
            (DVec3::new(8.0, 0.0, 65.0), 1.0),
        ];
        let walk = |attraction| {
            let mut wear = PathWear::new(0.5);
            let trail = TrailWalk {
                attraction,
                trips_per_day: 800.0,
                ..Default::default()
            };
            wear.walk_between(e, &places, DVec3::Y, 500, 80.0, WornGround::Turf, trail, 3);
            wear
        };
        let paths = walk(0.5);
        assert_eq!(bands(&paths, e, 20.0), 1, "no shared trail near the farm");
        assert_eq!(bands(&paths, e, 64.0), 2, "the trail never forks");
        // Walkers who don't care for paths go straight, each to their own place.
        let straight = walk(0.0);
        assert_eq!(
            bands(&straight, e, 20.0),
            2,
            "straight walkers shared a path"
        );
    }
}
