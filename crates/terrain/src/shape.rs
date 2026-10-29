//! Procedural planet shapes: continents, oceans, mountain ranges, hills, eroded detail,
//! craters and flat building pads, as a pure function of direction on the sphere.
//!
//! On planets with weather, an analytic erosion filter carves the mountains and hills into
//! branching valleys, spurs, gullies and couloirs ([`PlanetShape::erosion`]); the large
//! landforms carry their slopes along (as dual numbers) to steer it.
//!
//! The same function feeds the renderer (chunk meshes at every level of detail), the
//! [`Ground`](engine_world::Ground) query, footsteps and physics colliders, so what you see
//! is what you stand on. It is deterministic for a seed and evaluated in `f64`, so a
//! 6 371 km planet has centimeter detail and no precision loss anywhere on it.

use std::sync::Arc;

use engine_core::math::{DMat3, DQuat, DVec3};
use engine_core::prelude::*;

use crate::edits::OverlayPaint;
use crate::landforms::{Broken, Canyon, DarkRegion, IceCap, Landforms, ShieldVolcano, Spot};
use crate::noise;
use crate::rivers::{RiverCache, Rivers};

/// What a planet looks like at one point.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct ShapeSample {
    /// Height above the planet's base radius (sea level), meters. Negative under the sea.
    pub height: f64,
    /// 0 arid .. 1 lush.
    pub moisture: f32,
    /// Mean surface air temperature, °C (latitude and altitude).
    pub temperature_c: f32,
    /// 0 .. 1 dark lowland (lunar maria, basalt plains).
    pub mare: f32,
    /// 0 .. 1 river and lake water (a share of it where a channel is narrower than the detail
    /// asked for).
    pub water: f32,
    /// 0 .. 1 the part of `water` that is a lake.
    pub lake: f32,
    /// 0 .. 1 nearness to a river's banks (gallery woods line them).
    pub banks: f32,
}

/// A flat area for a level, a base or a spaceport: inside `radius_m` the ground is exactly at
/// `height_m`, then it blends into the natural terrain over `blend_m`.
///
/// A pad also lays out the landscape around it so the view from it is worth having: dry land
/// with rolling hills, a mountain range on one horizon (`mountains`, `mountain_azimuth_deg`,
/// `mountain_distance_m`) and the sea on another (`sea_azimuth_deg`, `sea_distance_m`). The
/// shaping fades out a few hundred kilometers away, where the planet's own continents take over.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct FlatPad {
    /// Planet-local latitude/longitude of the center, degrees (+Y is north).
    pub lat_deg: f64,
    pub lon_deg: f64,
    pub radius_m: f64,
    pub blend_m: f64,
    pub height_m: f64,
    /// 0 .. 1: height of the mountain range on the horizon (1 ≈ 4–5 km peaks, 0 = none).
    pub mountains: f64,
    /// Compass direction of the range from the pad, degrees (0 north, 90 east).
    pub mountain_azimuth_deg: f64,
    /// Distance from the pad to the crest line, meters.
    pub mountain_distance_m: f64,
    /// Compass direction of the nearest coast, degrees.
    pub sea_azimuth_deg: f64,
    /// Distance to the coast, meters (0 = no sea nearby, the pad is far inland).
    pub sea_distance_m: f64,
    /// Turn of the planet about the world's up axis at the home spot, degrees (spheres):
    /// north points toward −Z at 0 and toward +Z at 180 (east toward +X / −X), for a game
    /// whose compass is the other way round. Match `SpaceFrame::home_heading_deg`.
    pub heading_deg: f64,
}

impl Default for FlatPad {
    fn default() -> Self {
        Self {
            lat_deg: 40.0,
            lon_deg: 0.0,
            radius_m: 800.0,
            blend_m: 1500.0,
            // A low coastal plain: above the beach and the sea, level with the land around.
            height_m: 60.0,
            mountains: 0.8,
            mountain_azimuth_deg: 0.0,
            mountain_distance_m: 26_000.0,
            sea_azimuth_deg: 200.0,
            sea_distance_m: 16_000.0,
            heading_deg: 0.0,
        }
    }
}

impl FlatPad {
    /// Planet-local unit direction of the pad center.
    pub fn direction(&self) -> DVec3 {
        dir_from_lat_lon(self.lat_deg, self.lon_deg)
    }

    /// Planet-local to world rotation that puts the pad on top (+Y) with north toward −Z
    /// turned by [`heading_deg`](Self::heading_deg) about +Y.
    pub fn home_rotation(&self) -> DQuat {
        let q = crate::cube::home_rotation(self.lat_deg, self.lon_deg);
        (DQuat::from_rotation_y(self.heading_deg.to_radians()) * q).normalize()
    }
}

/// Planet-local unit direction for a latitude/longitude in degrees (+Y north, longitude 0
/// toward +Z, 90° toward +X).
pub fn dir_from_lat_lon(lat_deg: f64, lon_deg: f64) -> DVec3 {
    let (lat, lon) = (lat_deg.to_radians(), lon_deg.to_radians());
    DVec3::new(lat.cos() * lon.sin(), lat.sin(), lat.cos() * lon.cos())
}

/// Latitude/longitude in degrees of a planet-local direction.
pub fn lat_lon_from_dir(d: DVec3) -> (f64, f64) {
    let d = d.normalize();
    (
        d.y.clamp(-1.0, 1.0).asin().to_degrees(),
        d.x.atan2(d.z).to_degrees(),
    )
}

/// A hand-made ground (a level, a valley) that replaces the procedural shape around the home
/// spot, blending back into the planet at its edge. Build it with
/// [`PlanetTerrain::with_overlay`](crate::PlanetTerrain::with_overlay).
#[derive(Clone)]
pub struct HeightOverlay {
    /// Ground height (world y) at world `(x, z)`, meters, in the home frame (the home spot at
    /// the origin, +Y up, north toward −Z unless the pad's heading turns it). The ground passes exactly through these points:
    /// the terrain converts them to heights on the sphere itself, so don't subtract the
    /// planet's curvature unless the level is meant to follow it.
    pub height: Arc<dyn Fn(f64, f64) -> f64 + Send + Sync>,
    /// Full strength within this distance of the home spot, meters.
    pub radius_m: f64,
    /// Blend back to the planet over this distance, meters.
    pub blend_m: f64,
    /// Planet-local to world rotation of the planet entity (as stored in its `Transform`).
    pub rotation: DQuat,
    /// World position of the planet center.
    pub translation: DVec3,
    /// Flat terrain: world y is the height above the flat square, no curvature.
    pub flat: bool,
    /// Lowest and highest world y the function returns (sampled when built).
    pub y_range: (f64, f64),
    /// Ground layer and tint at world `(x, z)`, same frame as `height` (see
    /// [`PlanetTerrain::with_overlay_paint`](crate::PlanetTerrain::with_overlay_paint)).
    pub paint: Option<OverlayPaintFn>,
}

/// Paint of a hand-made ground at world `(x, z)`.
pub type OverlayPaintFn = Arc<dyn Fn(f64, f64) -> Option<OverlayPaint> + Send + Sync>;

impl std::fmt::Debug for HeightOverlay {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("HeightOverlay")
            .field("radius_m", &self.radius_m)
            .field("blend_m", &self.blend_m)
            .field("flat", &self.flat)
            .field("y_range", &self.y_range)
            .field("painted", &self.paint.is_some())
            .finish_non_exhaustive()
    }
}

impl PartialEq for HeightOverlay {
    fn eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.height, &other.height)
            && self.radius_m == other.radius_m
            && self.blend_m == other.blend_m
            && self.rotation == other.rotation
            && self.translation == other.translation
            && self.flat == other.flat
            && match (&self.paint, &other.paint) {
                (Some(a), Some(b)) => Arc::ptr_eq(a, b),
                (a, b) => a.is_none() && b.is_none(),
            }
    }
}

impl HeightOverlay {
    /// Weight (0..1) and height above sea level of the overlay ground in planet-local
    /// direction `dir` on a planet of radius `r`. `None` outside the overlay.
    fn sample(&self, dir: DVec3, r: f64) -> Option<(f64, f64)> {
        let w = self.rotation * dir;
        if self.flat {
            // Flat squares map (x, z) to the direction of (x, r, z) in the square's frame.
            if w.y <= 0.0 {
                return None;
            }
            let (x, z) = (w.x / w.y * r, w.z / w.y * r);
            let t = 1.0 - smooth(self.radius_m, self.radius_m + self.blend_m, x.hypot(z));
            return (t > 0.0).then(|| (t, (self.height)(x, z) - self.translation.y));
        }
        // The surface point at height `h` is `center + w (r + h)`; find the `h` whose world
        // point lies on the overlay (two fixed-point steps; the slope term is tiny).
        if w.y <= 0.0 {
            return None;
        }
        let c = self.translation;
        let along = |h: f64| c + w * (r + h);
        // Start where the ray meets world y = 0.
        let mut h = -c.y / w.y - r;
        let start = along(h);
        if start.x.hypot(start.z) > self.radius_m + self.blend_m + 1_000.0 {
            return None;
        }
        for _ in 0..2 {
            let p = along(h);
            let y = (self.height)(p.x, p.z);
            h = (y - c.y) / w.y - r;
        }
        let p = along(h);
        let t = 1.0 - smooth(self.radius_m, self.radius_m + self.blend_m, p.x.hypot(p.z));
        (t > 0.0).then_some((t, h))
    }

    /// The paint of the ground at planet-local direction `dir`, `h` above sea level on a
    /// planet of radius `r`, faded out with the overlay's blend.
    pub(crate) fn paint_at(&self, dir: DVec3, r: f64, h: f64) -> Option<OverlayPaint> {
        let paint = self.paint.as_ref()?;
        let w = self.rotation * dir;
        if w.y <= 0.0 {
            return None;
        }
        let (x, z) = if self.flat {
            (w.x / w.y * r, w.z / w.y * r)
        } else {
            let p = self.translation + w * (r + h);
            (p.x, p.z)
        };
        let t = 1.0 - smooth(self.radius_m, self.radius_m + self.blend_m, x.hypot(z)) as f32;
        if t <= 0.0 {
            return None;
        }
        let mut out = paint(x, z)?;
        out.weight = out.weight.clamp(0.0, 1.0) * t;
        out.tint_amount = out.tint_amount.clamp(0.0, 1.0) * t;
        Some(out)
    }
}

/// Climate: temperature by latitude and altitude, moisture pattern.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Climate {
    /// Mean temperature at the equator at sea level, °C.
    pub equator_c: f32,
    /// Mean temperature at the poles at sea level, °C.
    pub pole_c: f32,
    /// Cooling with altitude, °C per meter (Earth ≈ 0.0065).
    pub lapse_c_per_m: f32,
    /// Overall wetness 0 (desert world) .. 1 (jungle world).
    pub wetness: f32,
    /// 0..1 how strongly the general circulation shapes the wetness: rain along the equator
    /// and the mid-latitude storm tracks, deserts under the subtropical highs (about 25°),
    /// dry continental interiors and dry land in the lee of mountain belts (Earth 1).
    pub circulation: f32,
    /// Locked to its star, the star over planet-local +Z: warmth, ice and rain go by the
    /// angle from the point under the star instead of the latitude (see
    /// [`Climate::climate_latitude`]).
    pub substellar: bool,
}

impl Default for Climate {
    fn default() -> Self {
        Self {
            equator_c: 30.0,
            pole_c: -22.0,
            lapse_c_per_m: 0.0065,
            wetness: 0.58,
            circulation: 1.0,
            substellar: false,
        }
    }
}

impl Climate {
    /// The latitude (radians, signed) whose climate planet-local direction `dir` has. On a
    /// world locked to its star that is ¾ of the angle from the point under the star, so the
    /// "equator" is a warm, wet eye under the star and the "poles" the whole night side from
    /// 120° on (an eyeball world: Pierrehumbert 2011; Yang, Cowan & Abbot 2013). The far view
    /// of `engine_space` paints locked worlds by the same angle.
    pub fn climate_latitude(&self, dir: DVec3) -> f64 {
        if self.substellar {
            let theta = dir.z.clamp(-1.0, 1.0).acos();
            (theta * 0.75).min(std::f64::consts::FRAC_PI_2)
        } else {
            dir.y.clamp(-1.0, 1.0).asin()
        }
    }
}

/// Parameters of a procedural planet (or a flat world when used with
/// [`TerrainDomain::Flat`](crate::TerrainDomain)). All lengths in meters.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct PlanetShape {
    pub seed: u64,
    /// Base radius (sea level), meters. Earth 6 371 km, the Moon 1 737 km, Mars 3 390 km.
    pub radius_m: f64,
    /// Whether below-zero heights are sea (shaded as ocean from afar; the water module draws
    /// the surface up close).
    pub ocean: bool,
    /// Rough share of the surface below sea level, 0..1.
    pub ocean_fraction: f64,
    /// Size of continents (wavelength), meters.
    pub continent_scale_m: f64,
    /// Depth of the ocean floor, meters.
    pub ocean_depth_m: f64,
    /// How high the interior of continents rises above the coast, meters.
    pub plateau_height_m: f64,
    /// Tallest mountains, meters.
    pub mountain_height_m: f64,
    /// Spacing of mountain ridges (wavelength), meters.
    pub mountain_scale_m: f64,
    /// 0..1 share of land with mountain ranges.
    pub mountain_coverage: f64,
    /// Rolling hills: height and wavelength, meters.
    pub hill_height_m: f64,
    pub hill_scale_m: f64,
    /// Eroded small-scale relief: height and largest wavelength, meters.
    pub detail_height_m: f64,
    pub detail_scale_m: f64,
    /// Finest wavelength ever generated, meters (smaller bumps come from the shader).
    pub min_detail_m: f64,
    /// 0..1 strength of water and ice erosion: valleys, spurs, gullies and couloirs cut down
    /// the slopes of mountains and hills, flat valley floors (0 = none, as on the Moon).
    pub erosion: f64,
    /// Height of the rock ledges that steep eroded mountainsides break into, meters (a
    /// second set a third as high sits on top; 0 = none).
    pub ledge_height_m: f64,
    /// 0..1 how far hills and mountains are worn round where no water carves them: on an
    /// airless world billions of years of small impacts round off every crest and fill in
    /// gullies, so its mountains are broad, smooth massifs (the Moon: 1).
    pub worn: f64,
    /// Impact craters: radius of the largest, meters (0 = none).
    pub crater_max_radius_m: f64,
    /// Smallest crater radius, meters.
    pub crater_min_radius_m: f64,
    /// 0..1 how crowded each size class of craters is.
    pub crater_density: f64,
    /// 0..1 share of dark, flooded lowland plains (lunar maria).
    pub maria: f64,
    /// 0..1 how readily the big impact basins of a world with maria filled with dark lava
    /// (the Moon: 1); at 0 they stay pale, dusty depressions like Mars's Hellas and Argyre.
    pub lava_basins: f64,
    /// Named landforms placed by latitude and longitude (Mars: Olympus Mons, Valles
    /// Marineris, the polar caps, ...).
    pub landforms: Landforms,
    pub climate: Climate,
    /// Rivers run from the hills to the sea (worlds with a sea and erosion; see
    /// [`crate::rivers`]). Their drainage is solved the first time the shape is sampled.
    pub rivers: bool,
    /// The solved rivers (not saved).
    #[serde(skip)]
    pub(crate) river_cache: RiverCache,
    /// Flat areas for levels and bases.
    pub pads: Vec<FlatPad>,
    /// Hand-made ground around the home spot (not saved; set it in code).
    #[serde(skip)]
    pub overlay: Option<HeightOverlay>,
}

impl Default for PlanetShape {
    fn default() -> Self {
        Self::earthlike(1)
    }
}

impl PlanetShape {
    /// An Earth-sized world with oceans, continents, snowy mountain ranges, hills and plains.
    pub fn earthlike(seed: u64) -> Self {
        Self {
            seed,
            radius_m: 6_371_000.0,
            ocean: true,
            ocean_fraction: 0.62,
            continent_scale_m: 3_200_000.0,
            ocean_depth_m: 4_200.0,
            plateau_height_m: 600.0,
            mountain_height_m: 3_000.0,
            mountain_scale_m: 60_000.0,
            mountain_coverage: 0.35,
            hill_height_m: 200.0,
            hill_scale_m: 8_000.0,
            detail_height_m: 10.0,
            detail_scale_m: 400.0,
            min_detail_m: 1.0,
            erosion: 1.0,
            ledge_height_m: 60.0,
            worn: 0.0,
            crater_max_radius_m: 0.0,
            crater_min_radius_m: 0.0,
            crater_density: 0.0,
            maria: 0.0,
            lava_basins: 0.0,
            landforms: Landforms::default(),
            climate: Climate::default(),
            rivers: true,
            river_cache: RiverCache::default(),
            pads: Vec::new(),
            overlay: None,
        }
    }

    /// A Moon-like airless body: grey regolith, dark maria, craters from 60 km to 4 m.
    pub fn moon(seed: u64) -> Self {
        Self {
            seed,
            radius_m: 1_737_400.0,
            ocean: false,
            ocean_fraction: 0.0,
            continent_scale_m: 1_200_000.0,
            ocean_depth_m: 0.0,
            plateau_height_m: 1_800.0,
            mountain_height_m: 1_500.0,
            mountain_scale_m: 90_000.0,
            mountain_coverage: 0.2,
            hill_height_m: 180.0,
            hill_scale_m: 9_000.0,
            detail_height_m: 18.0,
            detail_scale_m: 900.0,
            min_detail_m: 0.8,
            erosion: 0.0,
            ledge_height_m: 0.0,
            worn: 1.0,
            crater_max_radius_m: 60_000.0,
            crater_min_radius_m: 4.0,
            crater_density: 0.75,
            maria: 0.35,
            lava_basins: 1.0,
            landforms: Landforms::default(),
            climate: Climate {
                equator_c: 10.0,
                pole_c: -150.0,
                lapse_c_per_m: 0.0,
                wetness: 0.0,
                circulation: 0.0,
                substellar: false,
            },
            rivers: false,
            river_cache: RiverCache::default(),
            pads: Vec::new(),
            overlay: None,
        }
    }

    /// Mars: red dust over old cratered highlands and smooth northern lowlands, with the
    /// real planet's landmarks where they are ([`mars_landforms`]): the Tharsis volcanoes
    /// and Olympus Mons, Valles Marineris, Hellas, Argyre and Isidis, the polar caps, and the
    /// dark regions seen from Earth (Syrtis Major, Mare Acidalium, ...). The seed varies only
    /// the small things (craters, hills, ragged edges).
    pub fn mars(seed: u64) -> Self {
        Self {
            seed,
            radius_m: 3_389_500.0,
            ocean: false,
            ocean_fraction: 0.0,
            continent_scale_m: 2_000_000.0,
            ocean_depth_m: 0.0,
            plateau_height_m: 1_800.0,
            mountain_height_m: 3_000.0,
            mountain_scale_m: 120_000.0,
            mountain_coverage: 0.25,
            hill_height_m: 300.0,
            hill_scale_m: 16_000.0,
            detail_height_m: 30.0,
            detail_scale_m: 1_500.0,
            min_detail_m: 1.0,
            erosion: 0.0,
            ledge_height_m: 0.0,
            worn: 0.6,
            crater_max_radius_m: 40_000.0,
            crater_min_radius_m: 20.0,
            crater_density: 0.35,
            maria: 0.0,
            lava_basins: 0.0,
            landforms: mars_landforms(),
            // Cold, but only the caps hold ice all year.
            climate: Climate {
                equator_c: -20.0,
                pole_c: -85.0,
                lapse_c_per_m: 0.0025,
                wetness: 0.02,
                circulation: 0.0,
                substellar: false,
            },
            rivers: false,
            river_cache: RiverCache::default(),
            pads: Vec::new(),
            overlay: None,
        }
    }

    /// Add a flat pad (builder style).
    pub fn with_pad(mut self, pad: FlatPad) -> Self {
        self.pads.push(pad);
        self
    }

    /// Sample at a planet-local unit direction. `min_wavelength_m` limits the finest detail
    /// (the renderer passes twice the vertex spacing; physics and gameplay pass 0 for full
    /// detail).
    pub fn sample(&self, dir: DVec3, min_wavelength_m: f64) -> ShapeSample {
        let rivers = self.river_network();
        self.sample_with(dir, min_wavelength_m, rivers.as_deref())
    }

    /// The rivers of this shape, solved the first time they are asked for (`None` on worlds
    /// without them).
    pub fn river_network(&self) -> Option<Arc<Rivers>> {
        if !self.rivers || !self.ocean || self.erosion <= 0.0 {
            return None;
        }
        Some(
            self.river_cache
                .get(self.river_key(), || Rivers::build(self)),
        )
    }

    /// What the rivers depend on: the shape's large landforms, climate, pads and overlay.
    pub(crate) fn river_key(&self) -> u64 {
        use std::hash::{Hash, Hasher};
        let mut h = std::collections::hash_map::DefaultHasher::new();
        (self.seed, self.ocean, self.rivers, self.pads.len()).hash(&mut h);
        let mut values = vec![
            self.radius_m,
            self.ocean_fraction,
            self.continent_scale_m,
            self.ocean_depth_m,
            self.plateau_height_m,
            self.mountain_height_m,
            self.mountain_scale_m,
            self.mountain_coverage,
            self.hill_height_m,
            self.hill_scale_m,
            self.erosion,
            self.climate.equator_c as f64,
            self.climate.pole_c as f64,
            self.climate.wetness as f64,
            self.climate.circulation as f64,
            self.climate.substellar as u8 as f64,
        ];
        for p in &self.pads {
            values.extend([
                p.lat_deg,
                p.lon_deg,
                p.radius_m,
                p.blend_m,
                p.height_m,
                p.mountains,
                p.mountain_azimuth_deg,
                p.mountain_distance_m,
                p.sea_azimuth_deg,
                p.sea_distance_m,
            ]);
        }
        if let Some(o) = &self.overlay {
            let q = o.rotation;
            values.extend([o.radius_m, o.blend_m, q.x, q.y, q.z, q.w]);
        }
        for v in values {
            v.to_bits().hash(&mut h);
        }
        h.finish()
    }

    /// [`Self::sample`] with the given rivers (none while solving them).
    pub(crate) fn sample_with(
        &self,
        dir: DVec3,
        min_wavelength_m: f64,
        rivers: Option<&Rivers>,
    ) -> ShapeSample {
        let dir = dir.normalize();
        let r = self.radius_m;
        let p = dir * r;
        let finest = min_wavelength_m.max(self.min_detail_m);
        let octaves_for = |scale: f64, max: f64| ((scale / finest).log2() + 1.0).clamp(0.0, max);
        let s = self.seed;

        // Continents with domain warping.
        let warp_p = p / (self.continent_scale_m * 0.55);
        let warp = DVec3::new(
            noise::fbm(warp_p, s ^ 0x11, 3, 0.5),
            noise::fbm(warp_p + 31.7, s ^ 0x12, 3, 0.5),
            noise::fbm(warp_p - 17.3, s ^ 0x13, 3, 0.5),
        ) * (self.continent_scale_m * 0.12);
        let cont = noise::fbm(
            (p + warp) / self.continent_scale_m,
            s ^ 0x21,
            octaves_for(self.continent_scale_m, 6.0).ceil() as u32,
            0.55,
        );
        // Bays, headlands and islands along the coasts.
        let coast_scale = self.continent_scale_m * 0.04;
        let bays = noise::fbm_smooth(
            p / coast_scale,
            s ^ 0x22,
            octaves_for(coast_scale, 6.0),
            0.6,
        );
        let threshold = (self.ocean_fraction - 0.5) * 0.55;
        let mut cl = D::c(cont + bays * 0.07 - threshold);
        // With erosion, the landforms below carry their slopes (see `D`) and stay smooth
        // below a few kilometers: the erosion filter carves everything finer. Without, they
        // are as rough as they need to be on their own.
        let eroding = self.erosion > 0.0;
        let along = |v: DVec3| v - dir * v.dot(dir);
        let fbm = |scale: f64, seed: u64, octaves: f64, grad_octaves: u32, gain: f64| {
            D::from(noise::fbm_d(
                p,
                scale,
                seed,
                octaves,
                grad_octaves,
                gain,
                &STEER_M,
            ))
        };

        // Pads lay out the land around them: a continent, a coast, uplands and a range.
        let mut calm = 1.0f64;
        let mut range_mask = 0.0f64;
        let mut range_h = D::ZERO;
        let mut pad_ridge = None;
        // How close to a sharp crest of the big landforms (1 on it): directions turn fast
        // there, so the widest gullies stop just below.
        let mut sharp_crest = 0.0f64;
        for pad in &self.pads {
            let c = pad.direction();
            let d = surface_distance(dir, c, r);
            calm = calm.min(smooth(pad.radius_m, pad.radius_m + pad.blend_m * 3.0, d.v));
            // Far away: the pad's continent, so it is not on an island.
            let continent = (1.0 - D::smooth(0.6e6, 2.2e6, d)) * 0.6;
            let region = 1.0 - D::smooth(150_000.0, 450_000.0, d);
            let east = DVec3::Y.cross(c).try_normalize().unwrap_or(DVec3::X);
            let north = c.cross(east);
            let axes = |az_deg: f64| {
                let (sa, ca) = az_deg.to_radians().sin_cos();
                let (toward, side) = (north * ca + east * sa, east * ca - north * sa);
                // (toward, along) in meters; r·sin(angle) ≈ distance this close to the pad.
                (
                    D::new(dir.dot(toward) * r, along(toward)),
                    D::new(dir.dot(side) * r, along(side)),
                )
            };
            // The sea on another horizon: a gulf with a ragged shore that widens and opens
            // into the planet's ocean far away.
            let mut sea = D::ZERO;
            if pad.sea_distance_m > 0.0 && self.ocean && dir.dot(c) > 0.0 {
                let (su, sv) = axes(pad.sea_azimuth_deg);
                let ragged = fbm(22_000.0, s ^ 0x7B, octaves_for(22_000.0, 5.0), 3, 0.55);
                let shore = (1.0 + 0.5 * ragged) * pad.sea_distance_m;
                // Hundreds of kilometers out, the gulf bends and its shores swell into capes
                // and bights, so from orbit it is not a straight-sided wedge. (Near the pad it
                // is as before.)
                let out = su.max(D::ZERO);
                let far = D::smooth(150_000.0, 700_000.0, su);
                let (bend, swell) = if far.v > 0.0 {
                    (
                        fbm(500_000.0, s ^ 0x7E, 4.0, 3, 0.5),
                        fbm(140_000.0, s ^ 0x7F, 5.0, 3, 0.6),
                    )
                } else {
                    (D::ZERO, D::ZERO)
                };
                let sv = sv - bend * out * far * 1.2;
                let half_width = 90_000.0
                    + 0.5 * out * (1.0 + swell * far * 1.6).max(D::c(0.15))
                    + ragged * 40_000.0;
                sea = D::smooth_t((su - shore + 3_000.0) / 23_000.0)
                    * (1.0 - D::smooth_t((sv.abs() - half_width) / (half_width * 0.5)))
                    * (1.0 - D::smooth(1.5e6, 2.5e6, su))
                    * smooth(0.0, 0.3, dir.dot(c));
            }
            let open_sea = D::c(-0.45 + bays * 0.3);
            if region.v <= 0.0 {
                cl = (cl + continent * (1.0 - sea)).lerp(open_sea, sea);
                continue;
            }
            // A low plain at the pad, uplands and basins around, rising toward the range.
            let (mu, mv) = axes(pad.mountain_azimuth_deg);
            let uplands = fbm(30_000.0, s ^ 0x7A, 4.0, 4, 0.5);
            let design = 0.09
                + 0.3 * pad.mountains.min(1.0) * D::smooth(4_000.0, pad.mountain_distance_m, mu)
                + uplands * 0.3 * D::smooth(2_000.0, 12_000.0, d);
            cl = (cl + continent * (1.0 - sea)).lerp(design, region);
            cl = cl.lerp(open_sea, sea);
            // The range: a bent crest line with passes; spurs and valleys run down from it.
            if pad.mountains > 0.0 {
                let bend = fbm(45_000.0, s ^ 0x7C, 3.0, 3, 0.5);
                let crest = (1.0 + 0.4 * bend) * pad.mountain_distance_m;
                let x = (mu - crest) / 11_000.0;
                let taper = 1.0 - D::smooth(45_000.0, 140_000.0, mv.abs());
                let w = taper * region * pad.mountains;
                let core = (-(x * x)).exp() * w;
                range_mask = range_mask.max((-(x.v * 0.5).powi(2)).exp() * w.v);
                if core.v > 1e-6 {
                    let passes = D::smooth(-0.35, 0.15, fbm(28_000.0, s ^ 0x7D, 3.0, 3, 0.5));
                    // Erosion carves everything below a few kilometers, so the range itself
                    // is only the big shapes then.
                    // Worn ranges keep only their big ridges.
                    let most = if eroding { 6.0 } else { 9.0 - 5.0 * self.worn };
                    let (ridge, ridge_crest) = *pad_ridge.get_or_insert_with(|| {
                        // Stretched across the crest: spurs and valleys, plus round peaks.
                        let q = DVec3::new(mu.v / 16_000.0, 0.37, mv.v / 6_500.0);
                        let (v, gq, spur_crest) = noise::ridged_d(
                            q,
                            1.0,
                            s ^ 0x33,
                            octaves_for(6_500.0, most),
                            if eroding { 3 } else { 0 },
                            &STEER_M.map(|m| m / 6_500.0),
                        );
                        let mut g = [DVec3::ZERO; LEVELS];
                        for (k, g) in g.iter_mut().enumerate() {
                            *g = mu.g[k] * (gq[k].x / 16_000.0) + mv.g[k] * (gq[k].z / 6_500.0);
                        }
                        let spurs = D { v, g };
                        let (pv, pg, peak_crest) = noise::ridged_d(
                            p,
                            12_000.0,
                            s ^ 0x34,
                            octaves_for(12_000.0, most),
                            if eroding { 3 } else { 0 },
                            &STEER_M,
                        );
                        let peaks = D::from((pv, pg));
                        (0.55 * spurs + 0.45 * peaks, spur_crest.max(peak_crest))
                    });
                    // Worn down: lower, rounder peaks.
                    let ridge = ridge.lerp(D::c(0.45), D::c(0.35 * self.worn));
                    let mut relief = 400.0 + 5_200.0 * ridge.powf(1.5);
                    if !eroding {
                        // Crags: sharp sub-ridges and gullies on the flanks.
                        let crags = noise::ridged_worn(
                            p / 3_000.0,
                            s ^ 0x35,
                            octaves_for(3_000.0, 7.0),
                            self.worn,
                        );
                        relief = relief + 700.0 * (1.0 - 0.6 * self.worn) * crags * crags * ridge;
                    }
                    let h = core * (0.35 + 0.65 * passes) * relief;
                    sharp_crest = sharp_crest.max(ridge_crest * smooth(300.0, 1_500.0, h.v));
                    range_h = range_h + h;
                }
            }
        }

        let (base, coast) = if self.ocean {
            let coast = D::smooth(-0.02, 0.08, cl);
            let land_h = 15.0 + self.plateau_height_m * D::smooth(0.02, 0.45, cl);
            let sea_h = -60.0 - self.ocean_depth_m * D::smooth(0.0, 0.22, -cl);
            (sea_h.lerp(land_h, coast), coast)
        } else {
            // Highlands and basins instead of continents and seas.
            (
                D::c(self.plateau_height_m * (cont * 1.6).clamp(-1.0, 1.0)),
                D::c(1.0),
            )
        };

        // Mountain ranges where the range mask is high.
        let range = noise::fbm(p / (self.mountain_scale_m * 14.0), s ^ 0x31, 4, 0.5);
        let cover = (self.mountain_coverage - 0.5) * 0.6;
        let mask = smooth(0.0, 0.35, range + cover) * coast.v;
        let mut mountains = range_h * coast;
        if mask > 0.0 && self.mountain_height_m > 0.0 {
            let scale = self.mountain_scale_m;
            let ridge = if eroding {
                let (v, g, c) =
                    noise::ridged_d(p, scale, s ^ 0x32, octaves_for(scale, 4.0), 4, &STEER_M);
                let h = v.max(0.0).powf(1.7) * mask * self.mountain_height_m * 1.6;
                sharp_crest = sharp_crest.max(c * smooth(300.0, 1_500.0, h));
                D::from((v, g))
            } else {
                D::c(noise::ridged_worn(
                    p / scale,
                    s ^ 0x32,
                    octaves_for(scale, 9.0),
                    self.worn,
                ))
            };
            mountains = mountains + ridge.powf(1.7) * (mask * self.mountain_height_m * 1.6);
        }
        let mask = mask.max(range_mask * coast.v);

        // Rolling hills: rounded tops and creased valleys, only ever raising the land, so
        // coastal plains stay dry.
        let hill_land = if self.ocean {
            D::smooth(-0.02, 0.12, cl)
        } else {
            D::c(1.0)
        };
        let billow = if eroding {
            let hn = fbm(
                self.hill_scale_m,
                s ^ 0x41,
                octaves_for(self.hill_scale_m, 3.0),
                3,
                0.58,
            );
            // Steered as if the creases were wide, so gullies turn gently into them.
            hn.soft_abs(0.055, [0.3; LEVELS])
        } else {
            let hn = noise::fbm_smooth(
                p / self.hill_scale_m,
                s ^ 0x41,
                octaves_for(self.hill_scale_m, 6.0),
                0.58,
            );
            // Worn hills are round in the creases too.
            let k2 = 0.003 + 0.03 * self.worn;
            D::c((hn * hn + k2).sqrt() - 0.055 - k2.sqrt() + 0.003f64.sqrt())
        };
        let hills = billow * hill_land * (2.0 * self.hill_height_m * (0.25 + 0.75 * calm));

        // Water and ice carve the big shapes into valleys, spurs and gullies.
        let landforms = base + mountains + hills;
        let mut h = landforms.v;
        let mut slope = 0.0;
        if eroding {
            let land = if self.ocean {
                smooth(-0.02, 0.12, cl.v)
            } else {
                1.0
            };
            let strength = self.erosion * land * (0.35 + 0.65 * calm);
            let steer = landforms.g.map(along);
            slope = steer[LEVELS - 1].length();
            if strength > 0.0 {
                let (dh, eroded_slope) = self.erode(
                    p,
                    dir,
                    &steer,
                    h,
                    sharp_crest,
                    mountains.v,
                    finest,
                    strength,
                );
                h += dh;
                slope = eroded_slope;
            }
        }

        // Rock ledges: steep mountainsides break into steps along the bedding planes, so
        // cliffs read as layered rock and the ledges hold snow, scree and grass. The layers
        // tilt and wander across the range; each set fades out where the vertices are too
        // far apart to draw its steps.
        if eroding && self.ledge_height_m > 0.0 {
            let steep = smooth(0.7, 1.4, slope);
            if steep > 0.0 {
                let tilt = noise::fbm(p / 3_000.0, s ^ 0x1ED6, 2, 0.5);
                let mut step = self.ledge_height_m;
                for k in 0..2 {
                    let lod = 1.0 - smooth(step * 0.15, step * 0.4, finest);
                    if lod <= 0.0 {
                        break;
                    }
                    let t = (h + tilt * step * 3.0) / step + k as f64 * 0.37;
                    let f = t - t.floor();
                    // Nearly flat for the lower part of each step, then a steep riser.
                    h += (smooth(0.45, 1.0, f) - f) * step * 0.85 * steep * lod;
                    step *= 0.3;
                }
            }
        }

        // Small eroded relief: rough on steep rock, smooth on meadows and valley floors.
        let roughness = if eroding {
            0.3 + 2.4 * smooth(0.35, 1.2, slope)
        } else {
            0.5 + 4.0 * mask * (1.0 - 0.75 * self.worn)
        };
        let detail = noise::eroded(
            p / self.detail_scale_m,
            s ^ 0x51,
            octaves_for(self.detail_scale_m, 12.0),
            0.48,
            Some(dir),
        ) * self.detail_height_m
            * roughness
            * (0.3 + 0.7 * calm);
        h += detail;

        // Landforms laid out by latitude and longitude.
        let marks = self.landforms.apply(dir, r, finest, s, &mut h);

        // Maria: dark flooded basins (flat, low).
        let mut mare = 0.0f32;
        if self.maria > 0.0 {
            // Big impact basins, most of those on the near side flooded to level plains,
            // and lava that spilled out of them across the lowlands between.
            let near = self.near_side();
            let basin = self.basins(dir, near, detail, finest, &mut h);
            let m = noise::fbm(p / (self.continent_scale_m * 0.7), s ^ 0x61, 6, 0.5);
            // Most lava came up on the near side, the most of it around its middle.
            let nearness = 0.22 * (dir.dot(near) - 0.4);
            let t = smooth(0.0, 0.08, m - (0.5 - self.maria) * 0.5 + nearness);
            h = lerp(h, -self.plateau_height_m * 0.8 + detail * 0.2, t * 0.9);
            // Flows of different ages and minerals: some darker, some paler.
            let tone = (0.75 + 0.5 * noise::fbm(p / 400_000.0, s ^ 0x62, 3, 0.5).abs()).min(1.0);
            mare = basin.max(t * tone) as f32;
        }

        mare = mare.max(marks.dark as f32);

        // Craters.
        if self.crater_max_radius_m > 0.0 {
            h += self.craters(p, finest, &mut mare);
        }

        // Rivers in their valleys.
        let (mut water, mut lake, mut banks) = (0.0, 0.0, 0.0);
        if let Some(rivers) = rivers {
            let here = rivers.at(p, dir, h, finest, r, s);
            h += here.fill - here.carve;
            (water, lake, banks) = (here.water, here.lake, here.banks);
        }

        // Pads: exactly flat inside, blended outside.
        for pad in &self.pads {
            let d = great_circle(dir, pad.direction(), r);
            let w = 1.0 - smooth(pad.radius_m, pad.radius_m + pad.blend_m, d);
            h = lerp(h, pad.height_m, w);
            water *= 1.0 - w;
            lake *= 1.0 - w;
            banks *= 1.0 - w;
        }
        // A hand-made ground replaces all of the above around the home spot.
        if let Some((t, ground)) = self.overlay.as_ref().and_then(|o| o.sample(dir, r)) {
            h = lerp(h, ground, t);
            water *= 1.0 - t;
            lake *= 1.0 - t;
            banks *= 1.0 - t;
        }

        // Climate.
        let c = &self.climate;
        let lat = c.climate_latitude(dir);
        let t_lat = c.pole_c + (c.equator_c - c.pole_c) * (lat.cos() as f32).powf(1.6);
        let mut temperature_c = t_lat - c.lapse_c_per_m * h.max(0.0) as f32;
        if self.ocean && c.circulation > 0.0 && h < 0.0 {
            // Currents carry warm water poleward in some seas and cold water toward the
            // equator in others, so the sea ice reaches far further south in some.
            let currents = noise::fbm(p / 2_500_000.0, s ^ 0x74, 3, 0.5) as f32;
            temperature_c += 9.0 * c.circulation * currents * smooth(0.0, 300.0, -h) as f32;
        }
        // Polar caps hold their ice whatever the latitude says.
        let temperature_c = temperature_c + (POLAR_ICE_C - temperature_c) * marks.ice as f32;
        // Wet and dry regions, and woods and fields between them.
        let wet_noise = (noise::fbm(p / 900_000.0, s ^ 0x71, 4, 0.5)
            + noise::fbm_smooth(p / 40_000.0, s ^ 0x72, octaves_for(40_000.0, 3.0), 0.5) * 0.7)
            as f32;
        let coastal = if self.ocean {
            (1.0 - smooth(0.05, 0.5, cl.v)) as f32 * 0.2
        } else {
            0.0
        };
        let belts = if c.circulation > 0.0 {
            c.circulation * self.circulation(p, dir, lat, cl.v) as f32
        } else {
            0.0
        };
        let moisture = (c.wetness + wet_noise * 0.55 + coastal - 0.08 + belts).clamp(0.0, 1.0);
        // River banks are green even in deserts.
        let moisture = moisture + (1.0 - moisture) * 0.4 * banks as f32;

        ShapeSample {
            height: h,
            moisture,
            temperature_c,
            mare,
            water: water as f32,
            lake: lake as f32,
            banks: banks as f32,
        }
    }

    /// Wetter or drier than the planet's mean (about −0.9..+0.3) by latitude and place: the
    /// rains of the equator and of the mid-latitude storm tracks, the deserts under the
    /// subtropical highs and in the polar cold, continental interiors far from the sea, and
    /// the rain shadow of mountain belts upwind (westerlies poleward of 30°, trade winds from
    /// the east nearer the equator). `cont` is the continent field (coast at 0).
    fn circulation(&self, p: DVec3, dir: DVec3, lat: f64, cont: f64) -> f64 {
        let lat_deg = lat.to_degrees().abs();
        let bell = |c: f64, w: f64| (-((lat_deg - c) / w).powi(2)).exp();
        let inland = smooth(0.1, 0.5, cont);
        if self.climate.substellar {
            // Locked: rising air and heavy rain under the star, the air wrung dry in a ring
            // around it (where the far view paints its deserts), a damper rim along the
            // terminator and the night side's cold, dry air (Yang, Cowan & Abbot 2013).
            let belts = 0.35 * bell(0.0, 16.0) - 0.55 * bell(37.0, 9.0) + 0.1 * bell(62.0, 8.0)
                - 0.2 * smooth(70.0, 88.0, lat_deg);
            return if self.ocean {
                belts - 0.25 * inland
            } else {
                belts
            };
        }
        let belts = 0.3 * bell(0.0, 12.0) - 0.55 * bell(25.0, 8.0) + 0.12 * bell(55.0, 10.0)
            - 0.15 * smooth(65.0, 85.0, lat_deg);
        if !self.ocean {
            return belts;
        }
        // Mountain belts upwind, 700 km away along the parallel.
        let reach = 700_000.0 / (self.radius_m * lat.cos().max(0.2));
        let easterly = 1.0 - smooth(25.0, 35.0, lat_deg);
        let shift = reach * (2.0 * easterly - 1.0);
        let upwind = DQuat::from_rotation_y(shift) * dir * self.radius_m;
        let belt = |q: DVec3| {
            let range = noise::fbm(q / (self.mountain_scale_m * 14.0), self.seed ^ 0x31, 4, 0.5);
            smooth(0.0, 0.35, range + (self.mountain_coverage - 0.5) * 0.6)
        };
        let shadow = if self.mountain_height_m > 0.0 {
            belt(upwind) * (1.0 - belt(p)) * smooth(0.0, 0.3, cont)
        } else {
            0.0
        };
        belts - 0.25 * inland - 0.3 * shadow
    }

    /// Height only (full detail).
    pub fn height(&self, dir: DVec3) -> f64 {
        self.sample(dir, 0.0).height
    }

    /// The erosion filter: valleys, spurs, gullies and couloirs cut into landforms of
    /// `height` at `p` (planet-local, meters; `up` is the surface normal), down to
    /// wavelength `finest`. `steer` is the landforms' slope along the surface (rise per
    /// meter) at each steering level ([`STEER_M`]), `crest` how close a sharp crest is
    /// (0..1), `relief` the height of the mountains and `strength` how hard to erode.
    /// Returns the change in height (never up) and the slope after erosion (rise over run).
    ///
    /// Each octave lays stripes that run downhill (see [`noise::stripes`]) and carves them
    /// into gullies as deep as the ground is steep, leaving the crests between them where
    /// they were. An octave follows the slope of landforms wider than its noise cells, so
    /// its gullies run straight instead of swirling, plus part of the gullies cut so far,
    /// so tributaries run into them at an angle and the network branches like drainage. The
    /// widest octaves also cut in proportion to the mountains' height (valleys) and leave
    /// sediment on their floors (flat valley bottoms); high up, crests are sharp (arêtes).
    /// It is a filter in the spirit of Rune Skovbo Johansen's erosion filter and Clay
    /// John's eroded terrain noise: analytic, so every patch can be built on its own at any
    /// level of detail.
    #[allow(clippy::too_many_arguments)]
    fn erode(
        &self,
        p: DVec3,
        up: DVec3,
        steer: &[DVec3; LEVELS],
        height: f64,
        crest: f64,
        relief: f64,
        finest: f64,
        strength: f64,
    ) -> (f64, f64) {
        use std::f64::consts::{PI, TAU};
        // Slopes of the gullies cut so far.
        let mut cuts = DVec3::ZERO;
        let mut dh = 0.0;
        // Sediment on the floors of the widest valleys, and how far from the crests and
        // floors of the previous octave (where flow turns fast).
        let (mut fill, mut flank) = (1.0, 1.0);
        let mut period = EROSION_WIDEST_M;
        let mut turn = DMat3::IDENTITY;
        // Water leaves rounded spurs in the lowlands; ice and frost high up leave sharp
        // arêtes between the gullies.
        let alpine = smooth(700.0, 2_400.0, height);
        for (i, (depth_k, incise_k)) in EROSION_DEPTH.iter().zip(EROSION_INCISE).enumerate() {
            // Fade the octaves in with the level of detail, until their gullies are many
            // vertices wide (see `EROSION_RESOLVED`).
            let lod = smooth(1.0, EROSION_RESOLVED, period / finest);
            if lod <= 0.0 {
                break;
            }
            // Downhill on the landforms as wide as this octave's cells, plus part of the
            // gullies cut so far: enough to branch, not so much that they swirl.
            let flow = steer[i.min(LEVELS - 1)] + cuts * EROSION_BRANCHING;
            let slope = flow.length();
            if slope < 1e-9 {
                break;
            }
            let across = up.cross(flow) / slope;
            let cell = period * EROSION_CELL;
            let (c, sn) = noise::stripes(
                turn * (p / cell),
                turn * across,
                self.seed ^ (0xE705 + i as u64 * 0x9E37),
                EROSION_CELL,
            );
            let rho = c.hypot(sn);
            // 0 between gullies (the ground as it was), 1 on a gully floor. Gullies only
            // ever cut: the crests between them stay where they were, and where a gully
            // ends it simply fades out.
            let u = (0.5 - 0.5 * c / rho.max(1e-9)).clamp(0.0, 1.0);
            // Narrow crests between wide gullies high up (still without a crease, which
            // would draw hairlines); rounded spurs below. An arête is rounded off over the
            // finest detail asked for, so it is never sharper than the vertices drawing it.
            let cut = if alpine > 0.0 {
                let round = (PI * EROSION_CREST_ROUNDING * finest / period).powi(2);
                let r = round.powf(0.6);
                let arete = ((u + round).powf(0.6) - r) / ((1.0 + round).powf(0.6) - r);
                rho * lerp(u, arete, alpine)
            } else {
                rho * u
            };
            // As deep as the ground is steep (up to about 45°), in proportion to the width;
            // the widest valleys also in proportion to the height of the mountains. Gullies
            // wider than a crest's rounding stop just below it.
            let real = steer[LEVELS - 1] + cuts;
            let steep = real.length() / (1.0 + real.length_squared()).sqrt();
            // Where the ground here slopes another way than the wide landforms, gullies
            // would run along the contours: fade them out.
            let aligned = smooth(
                0.35,
                0.85,
                (real.dot(flow) / (real.length() * slope + 1e-12)).abs(),
            );
            let incise = incise_k * relief.max(0.0) * slope / (slope + 0.05);
            // Near a peak, saddle or pit of the wide landforms their downhill direction spins
            // around, and so would the gullies (stars and spirals): fade them out where the
            // wide landforms are much flatter than the ground here.
            let calm = 0.02 + 0.2 * real.length();
            let settled = slope * slope / (slope * slope + calm * calm);
            let below_crest = 1.0 - crest * smooth(150.0, 600.0, period);
            // Frost and ice cut deeper high up.
            let depth = (depth_k * period * steep * (1.0 + EROSION_ALPINE * alpine) + incise)
                * strength
                * fill
                * flank
                * below_crest
                * aligned
                * settled;
            dh -= depth * cut * lod;
            // Slopes of the cut, from the rounded profile so directions turn smoothly over
            // crests and floors (a jump in direction would tear the next octave apart).
            cuts += across * (depth * 0.5 * sn * TAU / period * lod);
            // The next octave fades toward this one's crests and floors, where flow turns
            // sharply (it would swirl); sediment fills the floors of the widest valleys. Only
            // as far as this octave was cut: where it faded out, its stripes may spin.
            let shown = aligned * settled;
            flank = 1.0 - 0.6 * (1.0 - sn * sn) * shown;
            if i < 2 {
                // (Continuous across c = 0: a step here would draw a hairline.)
                let floor = c.min(0.0);
                fill *= 1.0 - 0.7 * floor * floor * shown;
            }
            period *= 0.5;
            turn *= OCTAVE_TURN;
        }
        (dh, (steer[LEVELS - 1] + cuts).length())
    }

    /// Seed of the crater scatter (16 bits, so the shader gets it exactly as an `f32`).
    pub fn crater_seed(&self) -> u32 {
        (self.seed & 0xFFFF) as u32
    }

    /// The middle of the hemisphere where most maria lie: planet-local +Z (latitude 0,
    /// longitude 0), the face a tidally locked moon turns to its planet (engine_space), so the
    /// Moon shows its maria to the Earth.
    fn near_side(&self) -> DVec3 {
        DVec3::Z
    }

    /// Big impact basins (the lunar maria): a sunken floor ringed by worn mountains and an
    /// outer scarp, the rings ragged and broken into massifs. Most basins on the near side are
    /// flooded with lava up to a level plain (`h` is lowered onto it); returns how much of the
    /// ground at `dir` is that lava, 0..1. Sizes scale with the planet (150 to 600 km across on
    /// the Moon).
    fn basins(&self, dir: DVec3, near: DVec3, detail: f64, finest: f64, h: &mut f64) -> f64 {
        let r = self.radius_m;
        let scale = r / 1_737_400.0;
        let mut lava = 0.0f64;
        for i in 0..BASINS {
            let rnd = |salt| noise::cell_random(i as i64, 0, 0, self.seed ^ 0xBA51, salt);
            // Most basins on the near side are within 60° of its middle.
            let mut center = unit_from(rnd(1), rnd(2));
            if rnd(3) < 0.65 {
                center = (center + near * 1.3).normalize();
            }
            let radius = (100_000.0 + 450_000.0 * rnd(4).powi(2)) * scale;
            let x = great_circle(dir, center, r) / radius;
            if x > 1.75 {
                continue;
            }
            // Old basins are worn down and their rings broken.
            let age = rnd(5);
            let depth = (2_000.0 + 2_500.0 * rnd(6)) * scale.sqrt() * (1.0 - 0.5 * age);
            let p = dir * r;
            let wobble = noise::fbm(p / (radius * 0.35), self.seed ^ (0xB0 + i as u64), 5, 0.55);
            let x = x + 0.1 * wobble;
            let floor = -depth * (1.0 - smooth(0.7, 1.0, x));
            let ring = |at: f64, width: f64| (-((x - at) / width).powi(2)).exp();
            let rings = ring(1.0, 0.06) + 0.45 * ring(1.4, 0.05);
            let massifs = if rings > 0.01 {
                let wavelength = 30_000.0 * scale;
                let octaves = ((wavelength / finest).log2() + 1.0).clamp(0.0, 6.0);
                noise::ridged_worn(p / wavelength, self.seed ^ 0xB5, octaves, self.worn)
            } else {
                0.0
            };
            *h += floor + rings * depth * 0.7 * (1.0 - 0.5 * age) * (0.2 + 1.2 * massifs);
            let side = if center.dot(near) > 0.0 { 2.6 } else { 0.3 };
            let flooded = rnd(7) < self.maria * self.lava_basins * side;
            if flooded {
                let shore = 0.78 + 0.15 * rnd(8);
                let t = 1.0 - smooth(shore - 0.06, shore + 0.02, x);
                *h = lerp(*h, -depth * 0.55 + detail * 0.15, t);
                // Each basin's lava has its own shade.
                lava = lava.max(t * (0.75 + 0.25 * rnd(9)));
            }
        }
        lava
    }

    /// The youngest big craters, whose rays of bright ejecta reach across the globe (like
    /// Tycho, Copernicus and Kepler on the Moon): a couple from the largest size class and a
    /// few from each of the next two ([`RAYED_PER_CLASS`]), as the planet-local unit
    /// direction of the center and the crater's radius on the ground, meters. They are the
    /// freshest of the same scatter as [`craters`](Self::craters), so the shape draws them
    /// crisp, and the shader draws their rays.
    pub fn rayed_craters(&self) -> Vec<(DVec3, f64)> {
        let mut all = Vec::new();
        if self.crater_max_radius_m <= 0.0 || self.crater_density <= 0.0 {
            return all;
        }
        let mut radius = self.crater_max_radius_m;
        for (class, &quota) in RAYED_PER_CLASS.iter().enumerate() {
            if radius < self.crater_min_radius_m {
                break;
            }
            let cell = radius * 3.2;
            let seed = self.crater_seed() ^ (class as u32).wrapping_mul(0x9E37);
            let n = (self.radius_m / cell).ceil() as i32 + 1;
            let mut found: Vec<(DVec3, f64)> = Vec::new();
            for cz in -n..=n {
                for cy in -n..=n {
                    for cx in -n..=n {
                        let rnd = |salt| noise::cell_random32(cx, cy, cz, seed, salt);
                        if rnd(6) > RAYED_AGE || rnd(1) > self.crater_density {
                            continue;
                        }
                        let center =
                            DVec3::new(cx as f64 + rnd(2), cy as f64 + rnd(3), cz as f64 + rnd(4))
                                * cell;
                        if self.young_crater_gone(center, rnd(1)) {
                            continue;
                        }
                        let rc = radius * (0.55 + 0.45 * rnd(5));
                        // Centers off the surface make smaller craters (see `craters`).
                        let off = center.length() - self.radius_m;
                        let on_ground = (rc * rc - off * off).max(0.0).sqrt();
                        if on_ground > 0.6 * rc {
                            found.push((center.normalize(), on_ground));
                        }
                    }
                }
            }
            found.sort_by(|a, b| b.1.total_cmp(&a.1));
            all.extend(found.into_iter().take(quota));
            radius *= 0.5;
        }
        all
    }

    /// Whether young ground ([`Landforms::youth`]) at a crater's center has buried the crater
    /// drawn with `pick` (its density draw): young lava keeps a tenth of the craters.
    fn young_crater_gone(&self, center: DVec3, pick: f64) -> bool {
        if self.landforms.volcanoes.is_empty()
            && self.landforms.rises.is_empty()
            && self.landforms.lowlands.is_empty()
        {
            return false;
        }
        let youth = self
            .landforms
            .youth(center.normalize_or_zero(), self.radius_m);
        pick > self.crater_density * (1.0 - 0.9 * youth)
    }

    /// Sum of crater profiles at `p` (planet-local, meters), from the largest size class down
    /// to `finest`. Each class scatters at most one crater per cell of a 3D grid; the profile
    /// uses the 3D distance to the crater's center, so centers below or above the surface make
    /// smaller and shallower craters (natural size variety). A crater reaches at most half a
    /// cell, so the 2×2×2 cells around `p` find every crater that touches it.
    ///
    /// The terrain shader scatters the same craters (`crater_detail` in terrain.wgsl) to shade
    /// the ones too small for the vertices; keep the two in step.
    fn craters(&self, p: DVec3, finest: f64, mare: &mut f32) -> f64 {
        let mut total = 0.0;
        let mut radius = self.crater_max_radius_m;
        let min_r = self.crater_min_radius_m.max(finest * 0.75);
        let mut class = 0u32;
        while radius >= min_r {
            let cell = radius * 3.2;
            let q = p / cell - 0.5;
            let base = q.floor();
            for dz in 0..2 {
                for dy in 0..2 {
                    for dx in 0..2 {
                        let (cx, cy, cz) =
                            (base.x as i32 + dx, base.y as i32 + dy, base.z as i32 + dz);
                        let seed = self.crater_seed() ^ class.wrapping_mul(0x9E37);
                        let rnd = |salt| noise::cell_random32(cx, cy, cz, seed, salt);
                        if rnd(1) > self.crater_density {
                            continue;
                        }
                        let center =
                            DVec3::new(cx as f64 + rnd(2), cy as f64 + rnd(3), cz as f64 + rnd(4))
                                * cell;
                        let rc = radius * (0.55 + 0.45 * rnd(5));
                        let x = (p - center).length() / rc;
                        if x >= 1.5 || self.young_crater_gone(center, rnd(1)) {
                            continue;
                        }
                        // Bigger craters are shallower relative to size, with flat floors.
                        // Old craters are worn down: shallower, with low rims.
                        let worn = smooth(0.1, 0.9, rnd(6));
                        let wear = 1.0 - 0.65 * worn;
                        let depth = rc * if rc > 8_000.0 { 0.06 } else { 0.18 } * wear;
                        let rim = rc * 0.045 * wear * wear;
                        let floor = if rc > 8_000.0 { 0.55 } else { 0.0 };
                        total += crater_profile(x, depth, rim, floor, worn);
                        // Large fresh craters expose bright material (less mare).
                        if x < 1.2 && rc > 3_000.0 {
                            *mare *= (0.4 + 0.6 * x / 1.2) as f32;
                        }
                    }
                }
            }
            radius *= 0.5;
            class += 1;
        }
        total
    }
}

/// Height of a crater at `x` (distance from its center in crater radii; nothing at 1.5 and
/// beyond), meters: `depth` below the land around it at the center and a `rim` raised around
/// the edge. A fresh crater (`worn` 0) is a bowl (flat-floored inside `floor`) with a
/// sharp-crested rim; a worn one (`worn` 1) has been sandblasted by smaller impacts into a
/// shallow dish with a rounded rim that blends into the land. `crater_detail` and
/// `micro_craters` in terrain.wgsl shade the same profile (keep them in step).
fn crater_profile(x: f64, depth: f64, rim: f64, floor: f64, worn: f64) -> f64 {
    if x >= 1.5 {
        return 0.0;
    }
    let bowl = if x < 1.0 {
        let xf = ((x - floor) / (1.0 - floor)).max(0.0);
        depth * (xf * xf - 1.0) + rim * smooth(0.6, 1.0, x)
    } else {
        rim * (1.0 - smooth(1.0, 1.5, x)) * (1.5 - x) * 2.0
    };
    let dish = depth * (smooth(floor * 0.8, 1.25, x) - 1.0)
        + rim * (1.0 - smooth(0.0, 0.4, (x - 1.05).abs()));
    lerp(bowl, dish, worn)
}

/// The landmarks of the real Mars, at their latitudes and longitudes (degrees east).
pub fn mars_landforms() -> Landforms {
    const KM: f64 = 1_000.0;
    let volcano = |lat, lon, radius_km: f64, height_m, caldera_km: f64, scarp_m| ShieldVolcano {
        at: Spot::new(lat, lon, radius_km * KM),
        height_m,
        caldera_m: caldera_km * KM,
        scarp_m,
    };
    let path = |points: &[[f64; 3]]| -> Vec<[f64; 3]> {
        points
            .iter()
            .map(|&[lat, lon, w]| [lat, lon, w * KM])
            .collect()
    };
    let broken = |lat, lon, radius_km: f64, depth_m, cell_km: f64, chaos| Broken {
        at: Spot::new(lat, lon, radius_km * KM),
        depth_m,
        cell_m: cell_km * KM,
        chaos,
    };
    let dark = |lat, lon, radius_km: f64, darkness| DarkRegion {
        at: Spot::new(lat, lon, radius_km * KM),
        darkness,
    };
    Landforms {
        // The northern lowlands (the Borealis basin), about 4 km below the highlands.
        lowlands: vec![(Spot::new(67.0, 208.0, 4_140.0 * KM), 4_000.0)],
        // The Tharsis and Elysium rises.
        rises: vec![
            (Spot::new(2.0, 252.0, 2_800.0 * KM), 6_500.0),
            (Spot::new(24.0, 146.0, 900.0 * KM), 3_000.0),
        ],
        basins: vec![
            (Spot::new(-42.4, 70.5, 1_150.0 * KM), 7_000.0), // Hellas
            (Spot::new(-49.7, 316.0, 900.0 * KM), 4_500.0),  // Argyre
            (Spot::new(12.9, 87.0, 750.0 * KM), 3_000.0),    // Isidis
        ],
        volcanoes: vec![
            volcano(18.65, 226.2, 300.0, 21_000.0, 40.0, 6_000.0), // Olympus Mons
            volcano(11.8, 255.5, 230.0, 14_000.0, 28.0, 0.0),      // Ascraeus Mons
            volcano(1.5, 247.0, 190.0, 11_000.0, 25.0, 0.0),       // Pavonis Mons
            volcano(-8.3, 239.9, 220.0, 12_000.0, 55.0, 0.0),      // Arsia Mons
            volcano(40.5, 250.4, 650.0, 5_000.0, 30.0, 0.0),       // Alba Mons
            volcano(24.8, 146.9, 120.0, 9_000.0, 7.0, 0.0),        // Elysium Mons
            volcano(32.1, 150.2, 90.0, 4_800.0, 6.0, 0.0),         // Hecates Tholus
        ],
        canyons: vec![
            // Valles Marineris: Ius, Melas, Coprates, Eos and Capri Chasmata, widest in Melas
            // (~200 km) and opening east into the chaos.
            Canyon {
                path: path(&[
                    [-6.6, 268.0, 15.0],
                    [-7.1, 274.0, 30.0],
                    [-7.8, 279.5, 40.0],
                    [-9.2, 284.5, 75.0],
                    [-10.2, 288.5, 100.0],
                    [-11.3, 292.5, 75.0],
                    [-12.8, 297.0, 40.0],
                    [-13.5, 303.0, 45.0],
                    [-13.0, 308.0, 55.0],
                    [-10.5, 312.5, 85.0],
                    [-6.5, 317.5, 90.0],
                    [-3.5, 322.0, 65.0],
                ]),
                depth_m: 7_000.0,
            },
            // Ius Chasma's southern trough, parallel to the main one.
            Canyon {
                path: path(&[[-8.9, 271.0, 8.0], [-9.4, 277.0, 13.0], [-9.3, 281.5, 10.0]]),
                depth_m: 4_500.0,
            },
            // Candor Chasma, north of Melas, and Ophir Chasma north of it.
            Canyon {
                path: path(&[
                    [-5.6, 281.5, 30.0],
                    [-6.2, 285.0, 70.0],
                    [-6.8, 289.0, 75.0],
                    [-7.6, 293.0, 35.0],
                ]),
                depth_m: 6_500.0,
            },
            Canyon {
                path: path(&[
                    [-3.6, 283.5, 25.0],
                    [-3.9, 287.0, 45.0],
                    [-4.4, 290.5, 25.0],
                ]),
                depth_m: 6_000.0,
            },
            // Tithonium Chasma.
            Canyon {
                path: path(&[
                    [-4.2, 268.0, 10.0],
                    [-4.6, 276.0, 15.0],
                    [-4.2, 281.0, 12.0],
                ]),
                depth_m: 5_000.0,
            },
            // Coprates Catena: a chain of pits and troughs south of Coprates.
            Canyon {
                path: path(&[
                    [-14.6, 292.0, 6.0],
                    [-15.1, 297.5, 9.0],
                    [-15.0, 302.5, 6.0],
                ]),
                depth_m: 2_500.0,
            },
            // Hebes Chasma, closed all round.
            Canyon {
                path: path(&[[-1.0, 282.0, 18.0], [-0.8, 286.5, 20.0]]),
                depth_m: 5_000.0,
            },
            // Ganges Chasma.
            Canyon {
                path: path(&[
                    [-8.5, 309.0, 25.0],
                    [-6.5, 312.0, 35.0],
                    [-3.0, 314.0, 25.0],
                ]),
                depth_m: 4_000.0,
            },
            // Chasma Boreale, cut into the north polar cap.
            Canyon {
                path: path(&[
                    [80.8, 346.0, 45.0],
                    [82.3, 353.0, 60.0],
                    [84.0, 2.0, 50.0],
                    [85.6, 16.0, 32.0],
                    [86.5, 30.0, 15.0],
                ]),
                depth_m: 2_000.0,
            },
        ],
        broken: vec![
            broken(-6.5, 258.0, 350.0, 4_500.0, 160.0, false), // Noctis Labyrinthus
            broken(-8.0, 326.0, 280.0, 3_500.0, 40.0, true),   // Aurorae Chaos
            broken(0.8, 325.0, 220.0, 4_000.0, 35.0, true),    // Hydraotes Chaos
            broken(-2.0, 342.0, 250.0, 3_000.0, 35.0, true),   // Iani Chaos
        ],
        ice_caps: vec![
            IceCap {
                at: Spot::new(90.0, 0.0, 550.0 * KM),
                dome_m: 2_800.0,
                troughs: 12.0,
            },
            IceCap {
                at: Spot::new(-87.0, 315.0, 220.0 * KM),
                dome_m: 3_000.0,
                troughs: -7.0,
            },
        ],
        dark_regions: vec![
            dark(16.0, 68.0, 300.0, 0.9), // Syrtis Major
            dark(5.0, 70.0, 500.0, 0.95),
            dark(-8.0, 15.0, 500.0, 0.8), // Sinus Sabaeus
            dark(-9.0, 35.0, 550.0, 0.8),
            dark(-7.0, 52.0, 450.0, 0.75),
            dark(-2.0, 357.0, 400.0, 0.85),  // Sinus Meridiani
            dark(-10.0, 335.0, 500.0, 0.75), // Margaritifer Sinus
            dark(-25.0, 325.0, 900.0, 0.8),  // Mare Erythraeum
            dark(-22.0, 300.0, 600.0, 0.7),
            dark(-15.0, 310.0, 400.0, 0.6),  // Aurorae Sinus
            dark(-26.0, 270.0, 300.0, 0.75), // Solis Lacus
            dark(-32.0, 205.0, 700.0, 0.75), // Mare Sirenum
            dark(-25.0, 180.0, 500.0, 0.6),
            dark(-22.0, 145.0, 800.0, 0.8), // Mare Cimmerium
            dark(-18.0, 105.0, 700.0, 0.8), // Mare Tyrrhenum
            dark(-10.0, 90.0, 400.0, 0.6),
            dark(-30.0, 40.0, 400.0, 0.6), // Mare Serpentis
            dark(-35.0, 85.0, 350.0, 0.5), // Mare Hadriacum
            dark(45.0, 330.0, 800.0, 0.9), // Mare Acidalium
            dark(55.0, 345.0, 500.0, 0.7),
            dark(42.0, 100.0, 500.0, 0.6), // Utopia
            dark(30.0, 75.0, 350.0, 0.5),  // Nilosyrtis
            dark(-60.0, 0.0, 700.0, 0.6),  // Mare Australe
            dark(-60.0, 120.0, 600.0, 0.5),
            dark(81.0, 180.0, 450.0, 0.85), // Olympia Undae, the dunes around the north cap
            dark(79.0, 240.0, 350.0, 0.7),
            dark(80.0, 120.0, 300.0, 0.6),
        ],
        mottling: 0.6,
    }
}

/// Impact basins on a world with maria.
const BASINS: u32 = 14;

/// Temperature of a polar cap's ice, °C: far below any snow line.
const POLAR_ICE_C: f32 = -125.0;

/// A unit vector from two numbers in `[0, 1)`, evenly spread over the sphere.
fn unit_from(u: f64, v: f64) -> DVec3 {
    let z = 2.0 * u - 1.0;
    let (sn, cs) = (std::f64::consts::TAU * v).sin_cos();
    let ring = (1.0 - z * z).max(0.0).sqrt();
    DVec3::new(ring * cs, ring * sn, z)
}

/// Craters whose age (the scatter's age hash, 0 = new) is below this have rays.
const RAYED_AGE: f64 = 0.04;
/// How many rayed craters [`PlanetShape::rayed_craters`] picks from each of the largest size
/// classes (the shader draws up to 8).
pub const RAYED_PER_CLASS: [usize; 3] = [2, 3, 3];

/// Distance along the surface between two unit directions on a sphere of radius `r`.
pub fn great_circle(a: DVec3, b: DVec3, r: f64) -> f64 {
    // 2 r asin(|a - b| / 2): accurate for tiny and large angles alike.
    2.0 * r * ((a - b).length() * 0.5).clamp(0.0, 1.0).asin()
}

#[inline]
fn lerp(a: f64, b: f64, t: f64) -> f64 {
    a + (b - a) * t
}

#[inline]
fn smooth(e0: f64, e1: f64, x: f64) -> f64 {
    let t = ((x - e0) / (e1 - e0)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// [`great_circle`] from `center` to `dir`, with its gradient (the surface direction away
/// from `center`, one meter per meter).
fn surface_distance(dir: DVec3, center: DVec3, r: f64) -> D {
    let away = (dir * dir.dot(center) - center)
        .try_normalize()
        .unwrap_or(DVec3::ZERO);
    D::new(great_circle(dir, center, r), away)
}

/// Widest gullies the erosion filter cuts (spacing between neighbouring spurs), meters.
const EROSION_WIDEST_M: f64 = 2_400.0;
/// Octaves of erosion, each half as wide: 2.4 km valleys down to 38 m gullies.
const EROSION_OCTAVES: usize = 7;
/// An erosion octave fades in between wavelengths of 1 and this many times the finest detail
/// asked for (2 to 16 vertex spacings on a terrain patch). Gullies only a few vertices wide
/// would alias: their crests zig-zag between vertices, and the shading flips from one face
/// of a crest to the other within a vertex, so ridge silhouettes get dark, serrated rims.
const EROSION_RESOLVED: f64 = 8.0;
/// Radius of the rounding on top of arêtes, in units of the finest detail asked for.
const EROSION_CREST_ROUNDING: f64 = 1.5;
/// Gully stripes per noise cell: how far a gully runs before it forks or merges.
const EROSION_CELL: f64 = 2.0;
/// How much each octave's gullies steer the next (0 = all run straight down the big
/// landforms, 1 = tributaries run down the walls of the gully they join).
const EROSION_BRANCHING: f64 = 0.3;
/// Depth of each octave's gullies relative to their width, on a 45° slope.
const EROSION_DEPTH: [f64; EROSION_OCTAVES] = [0.30, 0.26, 0.23, 0.21, 0.19, 0.17, 0.16];
/// Depth of each octave's valleys relative to the height of the mountains they cut.
const EROSION_INCISE: [f64; EROSION_OCTAVES] = [0.18, 0.08, 0.03, 0.0, 0.0, 0.0, 0.0];
/// How much deeper gullies are in the alpine zone (frost and ice).
const EROSION_ALPINE: f64 = 1.2;
/// A fixed rotation per erosion octave, so the noise cells of different octaves never line up.
const OCTAVE_TURN: DMat3 = DMat3::from_cols(
    DVec3::new(0.80, 0.60, 0.00),
    DVec3::new(-0.36, 0.48, 0.80),
    DVec3::new(0.48, -0.64, 0.60),
);

/// Steering levels: the slopes that steer erosion, low-passed to features at least this long
/// (meters). Erosion octave `i` follows level `i` (the last level for finer octaves), whose
/// direction holds steady across that octave's noise cells; the last level has everything.
const STEER_M: [f64; LEVELS] = [6_000.0, 3_000.0, 1_500.0, 0.0];
/// Number of steering levels.
const LEVELS: usize = 4;

/// A value with its gradient (per meter, planet-local) at each steering level, so the shapes
/// of the large landforms carry their slopes along to the erosion filter.
#[derive(Debug, Clone, Copy)]
struct D {
    v: f64,
    g: [DVec3; LEVELS],
}

impl D {
    const ZERO: D = D {
        v: 0.0,
        g: [DVec3::ZERO; LEVELS],
    };

    /// A value with the same (smooth, large-scale) gradient at every level.
    #[inline]
    fn new(v: f64, g: DVec3) -> D {
        D { v, g: [g; LEVELS] }
    }

    /// A constant.
    #[inline]
    fn c(v: f64) -> D {
        D {
            v,
            g: [DVec3::ZERO; LEVELS],
        }
    }

    /// `f(self)` from `f` and its derivative at `self.v`.
    #[inline]
    fn chain(self, f: f64, df: f64) -> D {
        D {
            v: f,
            g: self.g.map(|g| g * df),
        }
    }

    fn exp(self) -> D {
        let e = self.v.exp();
        self.chain(e, e)
    }

    fn powf(self, k: f64) -> D {
        if self.v <= 0.0 {
            return D::ZERO;
        }
        let pv = self.v.powf(k);
        self.chain(pv, k * pv / self.v)
    }

    /// `sqrt(x² + δ²) − δ`: |x| with the crease rounded over ±δ. The steering gradients are
    /// rounded more, over `±steer[k]`, so directions taken from them turn over a crest
    /// gradually, at a pace each level can follow.
    fn soft_abs(self, delta: f64, steer: [f64; LEVELS]) -> D {
        let x = self.v;
        let mut g = self.g;
        for (g, d) in g.iter_mut().zip(steer) {
            *g *= x / (x * x + d * d).sqrt();
        }
        D {
            v: (x * x + delta * delta).sqrt() - delta,
            g,
        }
    }

    fn abs(self) -> D {
        if self.v < 0.0 { -self } else { self }
    }

    fn max(self, o: D) -> D {
        if self.v >= o.v { self } else { o }
    }

    fn lerp(self, b: D, t: D) -> D {
        self + (b - self) * t
    }

    /// Smoothstep of `x` between `e0` and `e1`.
    fn smooth(e0: f64, e1: f64, x: D) -> D {
        D::smooth_t((x - e0) / (e1 - e0))
    }

    /// Smoothstep of an already normalized `t` (0..1 ramp).
    fn smooth_t(t: D) -> D {
        if t.v <= 0.0 {
            D::ZERO
        } else if t.v >= 1.0 {
            D::c(1.0)
        } else {
            t.chain(t.v * t.v * (3.0 - 2.0 * t.v), 6.0 * t.v * (1.0 - t.v))
        }
    }
}

impl From<(f64, [DVec3; LEVELS])> for D {
    #[inline]
    fn from((v, g): (f64, [DVec3; LEVELS])) -> D {
        D { v, g }
    }
}

impl std::ops::Add for D {
    type Output = D;
    #[inline]
    fn add(self, o: D) -> D {
        let mut g = self.g;
        for (a, b) in g.iter_mut().zip(o.g) {
            *a += b;
        }
        D { v: self.v + o.v, g }
    }
}

impl std::ops::Sub for D {
    type Output = D;
    #[inline]
    fn sub(self, o: D) -> D {
        self + -o
    }
}

impl std::ops::Mul for D {
    type Output = D;
    #[inline]
    fn mul(self, o: D) -> D {
        let mut g = self.g;
        for (a, b) in g.iter_mut().zip(o.g) {
            *a = *a * o.v + b * self.v;
        }
        D { v: self.v * o.v, g }
    }
}

impl std::ops::Div for D {
    type Output = D;
    #[inline]
    fn div(self, o: D) -> D {
        let q = self.v / o.v;
        let mut g = self.g;
        for (a, b) in g.iter_mut().zip(o.g) {
            *a = (*a - b * q) / o.v;
        }
        D { v: q, g }
    }
}

impl std::ops::Neg for D {
    type Output = D;
    #[inline]
    fn neg(self) -> D {
        D {
            v: -self.v,
            g: self.g.map(|g| -g),
        }
    }
}

impl std::ops::Add<f64> for D {
    type Output = D;
    #[inline]
    fn add(self, o: f64) -> D {
        D {
            v: self.v + o,
            g: self.g,
        }
    }
}

impl std::ops::Sub<f64> for D {
    type Output = D;
    #[inline]
    fn sub(self, o: f64) -> D {
        self + -o
    }
}

impl std::ops::Mul<f64> for D {
    type Output = D;
    #[inline]
    fn mul(self, o: f64) -> D {
        D {
            v: self.v * o,
            g: self.g.map(|g| g * o),
        }
    }
}

impl std::ops::Div<f64> for D {
    type Output = D;
    #[inline]
    fn div(self, o: f64) -> D {
        self * (1.0 / o)
    }
}

impl std::ops::Add<D> for f64 {
    type Output = D;
    #[inline]
    fn add(self, o: D) -> D {
        o + self
    }
}

impl std::ops::Sub<D> for f64 {
    type Output = D;
    #[inline]
    fn sub(self, o: D) -> D {
        -o + self
    }
}

impl std::ops::Mul<D> for f64 {
    type Output = D;
    #[inline]
    fn mul(self, o: D) -> D {
        o * self
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pads_are_flat_and_dry() {
        let pad = FlatPad {
            lat_deg: 40.0,
            lon_deg: 10.0,
            ..Default::default()
        };
        let shape = PlanetShape::earthlike(7).with_pad(pad);
        let c = pad.direction();
        // Points within the pad radius are exactly at the pad height.
        for i in 0..20 {
            let a = i as f64 * 0.3;
            let off = DVec3::new(a.cos(), 0.0, a.sin()) * (700.0 / shape.radius_m);
            let d = (c + off - c * c.dot(off)).normalize();
            assert!((shape.height(d) - pad.height_m).abs() < 1e-6);
        }
        // Somewhere in the 5–40 km ring there are proper hills or mountains.
        let mut max_h = f64::MIN;
        for i in 0..400 {
            let a = i as f64 * 0.157;
            let dist = 5_000.0 + (i % 20) as f64 * 1_800.0;
            let east = DVec3::Y.cross(c).normalize();
            let north = c.cross(east);
            let off = (east * a.cos() + north * a.sin()) * (dist / shape.radius_m);
            max_h = max_h.max(shape.height((c + off).normalize()));
        }
        assert!(max_h > 300.0, "highest point near the pad: {max_h} m");
    }

    #[test]
    fn home_has_a_snowy_range_and_a_coast() {
        let pad = FlatPad::default();
        let shape = PlanetShape::earthlike(7).with_pad(pad);
        let c = pad.direction();
        let east = DVec3::Y.cross(c).normalize();
        let north = c.cross(east);
        let at = |toward: DVec3, dist: f64, side: f64| {
            let off = toward * (dist / shape.radius_m) + toward.cross(c) * (side / shape.radius_m);
            shape.sample((c + off).normalize(), 0.0)
        };
        // Snow-capped peaks somewhere along the range to the north.
        let mut peak = ShapeSample::default();
        for i in 0..60 {
            for k in 0..20 {
                let s = at(
                    north,
                    15_000.0 + k as f64 * 1_500.0,
                    (i as f64 - 30.0) * 1_500.0,
                );
                if s.height > peak.height {
                    peak = s;
                }
            }
        }
        assert!(peak.height > 3_000.0, "highest peak {} m", peak.height);
        assert!(
            peak.temperature_c < -5.0,
            "peak at {} °C",
            peak.temperature_c
        );
        // The sea to the south-southwest, dry land between it and the pad.
        let toward_sea =
            (north * 200f64.to_radians().cos() + east * 200f64.to_radians().sin()).normalize();
        let coast = (1..60)
            .map(|k| k as f64 * 500.0)
            .find(|&d| at(toward_sea, d, 0.0).height < 0.0)
            .expect("no sea within 30 km");
        assert!(coast > 5_000.0, "sea only {coast} m away");
        assert!(at(toward_sea, 60_000.0, 0.0).height < -500.0);
    }

    #[test]
    fn earthlike_has_land_and_sea() {
        let shape = PlanetShape::earthlike(3);
        let (mut land, mut sea) = (0, 0);
        for i in 0..400 {
            let d = dir_from_lat_lon(-70.0 + (i % 20) as f64 * 7.0, (i / 20) as f64 * 18.0);
            if shape.sample(d, 5_000.0).height > 0.0 {
                land += 1;
            } else {
                sea += 1;
            }
        }
        assert!(land > 60 && sea > 120, "land {land}, sea {sea}");
    }

    #[test]
    fn moon_has_craters() {
        let shape = PlanetShape::moon(5);
        let mut min = f64::MAX;
        let mut max = f64::MIN;
        for i in 0..2000 {
            let d = dir_from_lat_lon(10.0 + (i % 50) as f64 * 0.01, (i / 50) as f64 * 0.01);
            let h = shape.sample(d, 50.0).height;
            min = min.min(h);
            max = max.max(h);
        }
        assert!(max - min > 100.0, "relief {min}..{max}");
    }

    #[test]
    fn earth_has_wet_and_dry_belts() {
        let shape = PlanetShape::earthlike(7);
        // Mean wetness of the land within 5° of a latitude (both hemispheres), and the
        // spread of sea temperatures there.
        let band = |lat0: f64| {
            let (mut wet, mut land) = (0.0, 0);
            let (mut cold, mut warm) = (f32::MAX, f32::MIN);
            for i in 0..1_200 {
                let lat = (lat0 - 5.0 + (i % 11) as f64) * if i % 2 == 0 { 1.0 } else { -1.0 };
                let s = shape.sample(dir_from_lat_lon(lat, (i / 2) as f64 * 0.6), 20_000.0);
                if s.height > 0.0 {
                    wet += s.moisture as f64;
                    land += 1;
                } else {
                    cold = cold.min(s.temperature_c);
                    warm = warm.max(s.temperature_c);
                }
            }
            (wet / land.max(1) as f64, warm - cold)
        };
        let (equator, _) = band(5.0);
        let (subtropics, _) = band(25.0);
        let (storm_tracks, spread) = band(55.0);
        assert!(equator > subtropics + 0.3, "{equator} {subtropics}");
        assert!(
            storm_tracks > subtropics + 0.2,
            "{storm_tracks} {subtropics}"
        );
        // Warm and cold currents: some seas at 55° freeze, others stay open.
        assert!(spread > 8.0, "{spread}");
    }

    #[test]
    fn mars_landmarks_stand_where_they_are() {
        let shape = PlanetShape::mars(5);
        let at = |lat: f64, lon: f64| shape.sample(dir_from_lat_lon(lat, lon), 2_000.0);
        // Olympus Mons towers over the plains around it.
        let summit = at(18.65, 226.2).height;
        let plain = at(18.65, 240.0).height;
        assert!(summit - plain > 15_000.0, "Olympus {summit} over {plain}");
        // Valles Marineris cuts kilometers below its rims.
        let floor = at(-9.5, 286.0).height;
        let rim = at(-14.0, 286.0).height;
        assert!(rim - floor > 4_000.0, "Melas floor {floor} under rim {rim}");
        // Hellas lies deep; the northern lowlands lie below the southern highlands.
        assert!(at(-42.4, 70.5).height < -4_000.0);
        assert!(at(60.0, 200.0).height < at(-30.0, 200.0).height);
        // The north pole is icy, the equator is not; Syrtis Major is dark.
        let pole = at(89.0, 0.0);
        assert!(
            pole.temperature_c < -100.0,
            "pole {} °C",
            pole.temperature_c
        );
        assert!(at(0.0, 200.0).temperature_c > -60.0);
        assert!(at(10.0, 69.0).mare > 0.5);
    }

    #[test]
    fn maria_lie_on_the_near_side() {
        let shape = PlanetShape::moon(7);
        let (mut near, mut far) = (0.0, 0.0);
        for i in 0..400 {
            let lat = (i / 20) as f64 * 9.0 - 85.5;
            let lon = (i % 20) as f64 * 9.0 - 85.5;
            near += shape.sample(dir_from_lat_lon(lat, lon), 5_000.0).mare as f64;
            far += shape
                .sample(dir_from_lat_lon(lat, lon + 180.0), 5_000.0)
                .mare as f64;
        }
        let (near, far) = (near / 400.0, far / 400.0);
        assert!(
            near > 0.15 && near > 3.0 * far,
            "near side {near}, far side {far}"
        );
    }

    #[test]
    fn the_moon_has_a_few_rayed_craters() {
        let rayed = PlanetShape::moon(7).rayed_craters();
        assert_eq!(rayed.len(), RAYED_PER_CLASS.iter().sum::<usize>());
        for (dir, r) in &rayed {
            assert!((dir.length() - 1.0).abs() < 1e-9);
            assert!((4_000.0..60_000.0).contains(r), "radius {r}");
        }
        assert!(PlanetShape::earthlike(7).rayed_craters().is_empty());
    }

    #[test]
    fn crater_profiles_are_continuous() {
        for worn in [0.0, 0.5, 1.0] {
            for floor in [0.0, 0.55] {
                let at = |x: f64| crater_profile(x, 100.0, 20.0, floor, worn);
                assert!(at(0.0) < -50.0, "worn {worn}: center {}", at(0.0));
                assert_eq!(at(1.5), 0.0);
                assert!(at(1.4999).abs() < 0.01, "worn {worn}: edge {}", at(1.4999));
                for i in 0..1500 {
                    let x = i as f64 * 0.001;
                    let step = (at(x + 0.001) - at(x)).abs();
                    assert!(step < 1.0, "worn {worn}, floor {floor}: step {step} at {x}");
                }
            }
        }
        // Worn down, the rim's crest is round instead of sharp.
        let kink = |worn: f64| {
            let at = |x: f64| crater_profile(x, 100.0, 20.0, 0.0, worn);
            (at(1.0) - at(0.99)) / 0.01 - (at(1.01) - at(1.0)) / 0.01
        };
        assert!(
            kink(1.0) < 0.3 * kink(0.0),
            "{} vs {}",
            kink(1.0),
            kink(0.0)
        );
    }

    #[test]
    fn worn_ridges_are_ridged_noise_rounded() {
        for i in 0..50 {
            let p = DVec3::new(i as f64 * 0.37, 1.3, -(i as f64) * 0.21);
            assert_eq!(noise::ridged(p, 9, 6.0), noise::ridged_worn(p, 9, 6.0, 0.0));
        }
    }

    #[test]
    fn lower_detail_stays_close_to_full_detail() {
        let shape = PlanetShape::earthlike(11);
        for i in 0..50 {
            let d = dir_from_lat_lon(i as f64 * 1.3 - 30.0, i as f64 * 6.1);
            let full = shape.sample(d, 0.0).height;
            let coarse = shape.sample(d, 200.0).height;
            assert!((full - coarse).abs() < 150.0, "{full} vs {coarse}");
        }
    }

    /// Erosion never tears the ground: walking across the range and down to the coast, the
    /// height has no steps or near-vertical walls (a narrowing step at a millimeter).
    #[test]
    fn eroded_ground_is_continuous() {
        let pad = FlatPad::default();
        let shape = PlanetShape::earthlike(7).with_pad(pad);
        let c = pad.direction();
        let east = DVec3::Y.cross(c).normalize();
        let north = c.cross(east);
        let at = |x: f64, y: f64| {
            let d = (c + east * (x / shape.radius_m) + north * (y / shape.radius_m)).normalize();
            shape.sample(d, 10.0).height
        };
        // Across the range (through a saddle of its big shapes, where gullies once spun
        // around) and through the hills down to the coast.
        for (x, y0) in [(-2_700.0, 23_500.0), (-9_000.0, -15_500.0)] {
            let mut prev = at(x, y0);
            for k in 1..1_500 {
                let y = y0 + k as f64;
                let h = at(x, y);
                if (h - prev).abs() > 1.0 {
                    // Narrow the steepest part of the step down to a millimeter.
                    let (mut a, mut b) = (y - 1.0, y);
                    for _ in 0..10 {
                        let m = 0.5 * (a + b);
                        if (at(x, m) - at(x, a)).abs() > (at(x, b) - at(x, m)).abs() {
                            b = m;
                        } else {
                            a = m;
                        }
                    }
                    let step = at(x, b) - at(x, a);
                    assert!(step.abs() < 0.02, "step of {step} m at ({x}, {a})");
                }
                prev = h;
            }
        }
    }

    /// The eroded home landscape stays inside the terrain's conservative height range (the
    /// renderer culls patches with it).
    #[test]
    fn eroded_home_stays_in_height_range() {
        let pad = FlatPad::default();
        let terrain = crate::PlanetTerrain {
            shape: PlanetShape::earthlike(7).with_pad(pad),
            ..crate::PlanetTerrain::earthlike(7)
        };
        let (low, high) = terrain.height_range();
        let shape = &terrain.shape;
        let c = pad.direction();
        let east = DVec3::Y.cross(c).normalize();
        let north = c.cross(east);
        let (mut lowest, mut highest) = (f64::MAX, f64::MIN);
        for j in 0..100 {
            for i in 0..100 {
                let (x, y) = (i as f64 * 600.0 - 30_000.0, j as f64 * 600.0 - 20_000.0);
                let d =
                    (c + east * (x / shape.radius_m) + north * (y / shape.radius_m)).normalize();
                let h = shape.sample(d, 20.0).height;
                lowest = lowest.min(h);
                highest = highest.max(h);
            }
        }
        assert!(
            low < lowest && highest < high,
            "{lowest}..{highest} outside {low}..{high}"
        );
    }

    /// Cost of one shape sample around the home range, at patch-like levels of detail.
    /// `cargo test --release -p engine_terrain sample_cost -- --ignored --nocapture`
    #[test]
    #[ignore = "timing; run in release"]
    fn sample_cost() {
        let pad = FlatPad::default();
        let shape = PlanetShape::earthlike(7).with_pad(pad);
        let c = pad.direction();
        let east = DVec3::Y.cross(c).normalize();
        let north = c.cross(east);
        let mut total = 0.0;
        let mut n_total = 0usize;
        for wavelength in [1.0, 4.0, 16.0, 64.0, 256.0] {
            let n = 40_000usize;
            let started = std::time::Instant::now();
            let mut sink = 0.0;
            for i in 0..n {
                // A 60 × 60 km square over the pad, the range and the coast.
                let x = ((i * 7919) % 1000) as f64 * 60.0 - 30_000.0;
                let y = ((i * 104_729) % 1000) as f64 * 60.0 - 20_000.0;
                let d =
                    (c + east * (x / shape.radius_m) + north * (y / shape.radius_m)).normalize();
                sink += shape.sample(d, wavelength).height;
            }
            let us = started.elapsed().as_secs_f64() * 1e6;
            println!(
                "min wavelength {wavelength:>5} m: {:.2} µs/sample ({sink:.0})",
                us / n as f64
            );
            total += us;
            n_total += n;
        }
        println!("mean: {:.2} µs/sample", total / n_total as f64);
    }

    #[test]
    fn lat_lon_round_trip() {
        let d = dir_from_lat_lon(33.0, -120.0);
        let (lat, lon) = lat_lon_from_dir(d);
        assert!((lat - 33.0).abs() < 1e-9 && (lon + 120.0).abs() < 1e-9);
    }
}
