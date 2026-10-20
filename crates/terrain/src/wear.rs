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

use engine_core::math::{DVec3, IVec2};
use engine_core::prelude::*;
use engine_world::{ContactKind, Deformation as Deform, SurfaceMaterial};

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
    tiles: HashMap<IVec2, WearTile>,
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
                let tile = region.tiles.entry(key).or_insert_with(WearTile::new);
                let i = (y.rem_euclid(TILE) * TILE + x.rem_euclid(TILE)) as usize;
                let g = gain * share.min(1.0);
                let w = &mut tile.wear[i];
                *w += g * (1.0 - *w);
                tile.ground[i] = ground;
                self.dirty.insert((terrain, index, key));
            }
        }
    }

    /// Lets every worn cell heal for `dt` seconds. `snowfall_m_s`: fresh snow falling (meters
    /// per second), which buries packed snow.
    pub fn heal(&mut self, dt: f32, snowfall_m_s: f32, heal_scale: f32) {
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
        for (&terrain, regions) in &mut self.terrains {
            for (index, region) in regions.iter_mut().enumerate() {
                region.tiles.retain(|&key, tile| {
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
                    self.dirty.insert((terrain, index, key));
                    peak > 0.0
                });
            }
        }
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
}
