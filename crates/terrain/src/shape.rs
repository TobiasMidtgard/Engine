//! Procedural planet shapes: continents, oceans, mountain ranges, hills, eroded detail,
//! craters and flat building pads, as a pure function of direction on the sphere.
//!
//! The same function feeds the renderer (chunk meshes at every level of detail), the
//! [`Ground`](engine_world::Ground) query, footsteps and physics colliders, so what you see
//! is what you stand on. It is deterministic for a seed and evaluated in `f64`, so a
//! 6 371 km planet has centimeter detail and no precision loss anywhere on it.

use std::sync::Arc;

use engine_core::math::{DQuat, DVec3};
use engine_core::prelude::*;

use crate::edits::OverlayPaint;
use crate::noise;

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
        }
    }
}

impl FlatPad {
    /// Planet-local unit direction of the pad center.
    pub fn direction(&self) -> DVec3 {
        dir_from_lat_lon(self.lat_deg, self.lon_deg)
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
    /// the origin, +Y up, north toward −Z) as if the world were flat. The terrain adds the
    /// planet's curvature, so the ground passes exactly through these points.
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
}

impl Default for Climate {
    fn default() -> Self {
        Self {
            equator_c: 30.0,
            pole_c: -22.0,
            lapse_c_per_m: 0.0065,
            wetness: 0.58,
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
    /// Impact craters: radius of the largest, meters (0 = none).
    pub crater_max_radius_m: f64,
    /// Smallest crater radius, meters.
    pub crater_min_radius_m: f64,
    /// 0..1 how crowded each size class of craters is.
    pub crater_density: f64,
    /// 0..1 share of dark, flooded lowland plains (lunar maria).
    pub maria: f64,
    pub climate: Climate,
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
            detail_height_m: 60.0,
            detail_scale_m: 1_800.0,
            min_detail_m: 1.0,
            crater_max_radius_m: 0.0,
            crater_min_radius_m: 0.0,
            crater_density: 0.0,
            maria: 0.0,
            climate: Climate::default(),
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
            crater_max_radius_m: 60_000.0,
            crater_min_radius_m: 4.0,
            crater_density: 0.75,
            maria: 0.35,
            climate: Climate {
                equator_c: 10.0,
                pole_c: -150.0,
                lapse_c_per_m: 0.0,
                wetness: 0.0,
            },
            pads: Vec::new(),
            overlay: None,
        }
    }

    /// A Mars-like desert world: red dust, canyons and old craters, polar ice.
    pub fn mars(seed: u64) -> Self {
        Self {
            seed,
            radius_m: 3_389_500.0,
            ocean: false,
            ocean_fraction: 0.0,
            continent_scale_m: 2_000_000.0,
            ocean_depth_m: 0.0,
            plateau_height_m: 3_000.0,
            mountain_height_m: 6_000.0,
            mountain_scale_m: 120_000.0,
            mountain_coverage: 0.25,
            hill_height_m: 300.0,
            hill_scale_m: 16_000.0,
            detail_height_m: 30.0,
            detail_scale_m: 1_500.0,
            min_detail_m: 1.0,
            crater_max_radius_m: 40_000.0,
            crater_min_radius_m: 20.0,
            crater_density: 0.35,
            maria: 0.15,
            climate: Climate {
                equator_c: -20.0,
                pole_c: -110.0,
                lapse_c_per_m: 0.0025,
                wetness: 0.02,
            },
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
        let mut cl = cont + bays * 0.07 - threshold;

        // Pads lay out the land around them: a continent, a coast, uplands and a range.
        let mut calm = 1.0f64;
        let mut range_mask = 0.0f64;
        let mut range_h = 0.0;
        let mut pad_ridge = None;
        for pad in &self.pads {
            let c = pad.direction();
            let d = great_circle(dir, c, r);
            calm = calm.min(smooth(pad.radius_m, pad.radius_m + pad.blend_m * 3.0, d));
            // Far away: the pad's continent, so it is not on an island.
            let continent = 0.6 * (1.0 - smooth(0.6e6, 2.2e6, d));
            let region = 1.0 - smooth(150_000.0, 450_000.0, d);
            let axes = |az_deg: f64| {
                let east = DVec3::Y.cross(c).try_normalize().unwrap_or(DVec3::X);
                let north = c.cross(east);
                let (sa, ca) = az_deg.to_radians().sin_cos();
                // (toward, along) in meters; r·sin(angle) ≈ distance this close to the pad.
                (
                    dir.dot(north * ca + east * sa) * r,
                    dir.dot(east * ca - north * sa) * r,
                )
            };
            // The sea on another horizon: a gulf with a ragged shore that widens and opens
            // into the planet's ocean far away.
            let mut sea = 0.0;
            if pad.sea_distance_m > 0.0 && self.ocean && dir.dot(c) > 0.0 {
                let (su, sv) = axes(pad.sea_azimuth_deg);
                let ragged =
                    noise::fbm_smooth(p / 22_000.0, s ^ 0x7B, octaves_for(22_000.0, 5.0), 0.55);
                let shore = pad.sea_distance_m * (1.0 + 0.5 * ragged);
                let half_width = 90_000.0 + 0.5 * su.max(0.0) + ragged * 40_000.0;
                sea = smooth(shore - 3_000.0, shore + 20_000.0, su)
                    * (1.0 - smooth(half_width, half_width * 1.5, sv.abs()))
                    * (1.0 - smooth(1.5e6, 2.5e6, su))
                    * smooth(0.0, 0.3, dir.dot(c));
            }
            let open_sea = -0.45 + bays * 0.3;
            if region <= 0.0 {
                cl = lerp(cl + continent * (1.0 - sea), open_sea, sea);
                continue;
            }
            // A low plain at the pad, uplands and basins around, rising toward the range.
            let (mu, mv) = axes(pad.mountain_azimuth_deg);
            let uplands = noise::fbm(p / 30_000.0, s ^ 0x7A, 4, 0.5);
            let design = 0.09
                + 0.3 * smooth(4_000.0, pad.mountain_distance_m, mu) * pad.mountains.min(1.0)
                + uplands * 0.3 * smooth(2_000.0, 12_000.0, d);
            cl = lerp(cl + continent * (1.0 - sea), design, region);
            cl = lerp(cl, open_sea, sea);
            // The range: a bent crest line with passes; spurs and valleys run down from it.
            if pad.mountains > 0.0 {
                let crest = pad.mountain_distance_m
                    * (1.0 + 0.4 * noise::fbm(p / 45_000.0, s ^ 0x7C, 3, 0.5));
                let x = mu - crest;
                let taper = 1.0 - smooth(45_000.0, 140_000.0, mv.abs());
                let w = pad.mountains * taper * region;
                let core = (-(x / 11_000.0).powi(2)).exp() * w;
                range_mask = range_mask.max((-(x / 22_000.0).powi(2)).exp() * w);
                if core > 1e-3 {
                    let passes = smooth(-0.35, 0.15, noise::fbm(p / 28_000.0, s ^ 0x7D, 3, 0.5));
                    let ridge = *pad_ridge.get_or_insert_with(|| {
                        // Stretched across the crest: spurs and valleys, plus round peaks.
                        let q = DVec3::new(mu / 16_000.0, 0.37, mv / 6_500.0);
                        let spurs = noise::ridged(q, s ^ 0x33, octaves_for(6_500.0, 9.0));
                        let peaks =
                            noise::ridged(p / 12_000.0, s ^ 0x34, octaves_for(12_000.0, 9.0));
                        0.55 * spurs + 0.45 * peaks
                    });
                    // Crags: sharp sub-ridges and gullies on the flanks.
                    let crags = noise::ridged(p / 3_000.0, s ^ 0x35, octaves_for(3_000.0, 7.0));
                    range_h += core
                        * (0.35 + 0.65 * passes)
                        * (400.0 + 5_200.0 * ridge.powf(1.5) + 700.0 * crags * crags * ridge);
                }
            }
        }

        let (base, coast) = if self.ocean {
            let coast = smooth(-0.02, 0.08, cl);
            let land_h = 15.0 + self.plateau_height_m * smooth(0.02, 0.45, cl);
            let sea_h = -60.0 - self.ocean_depth_m * smooth(0.0, 0.22, -cl);
            (lerp(sea_h, land_h, coast), coast)
        } else {
            // Highlands and basins instead of continents and seas.
            (self.plateau_height_m * (cont * 1.6).clamp(-1.0, 1.0), 1.0)
        };

        // Mountain ranges where the range mask is high.
        let range = noise::fbm(p / (self.mountain_scale_m * 14.0), s ^ 0x31, 4, 0.5);
        let cover = (self.mountain_coverage - 0.5) * 0.6;
        let mask = smooth(0.0, 0.35, range + cover) * coast;
        let mut mountains = range_h * coast;
        if mask > 0.0 && self.mountain_height_m > 0.0 {
            let o = octaves_for(self.mountain_scale_m, 9.0);
            let ridge = noise::ridged(p / self.mountain_scale_m, s ^ 0x32, o);
            mountains += mask * ridge.powf(1.7) * self.mountain_height_m * 1.6;
        }
        let mask = mask.max(range_mask * coast);

        // Hills and eroded detail.
        // Rolling hills: rounded tops and creased valleys, only ever raising the land, so
        // coastal plains stay dry.
        let hn = noise::fbm_smooth(
            p / self.hill_scale_m,
            s ^ 0x41,
            octaves_for(self.hill_scale_m, 6.0),
            0.58,
        );
        let hills = ((hn * hn + 0.003).sqrt() - 0.055)
            * 2.0
            * self.hill_height_m
            * if self.ocean {
                smooth(-0.02, 0.12, cl)
            } else {
                1.0
            }
            * (0.25 + 0.75 * calm);
        let detail = noise::eroded(
            p / self.detail_scale_m,
            s ^ 0x51,
            octaves_for(self.detail_scale_m, 12.0),
            0.48,
            Some(dir),
        ) * self.detail_height_m
            * (0.5 + 4.0 * mask)
            * (0.3 + 0.7 * calm);

        // Maria: dark flooded basins (flat, low).
        let mut mare = 0.0f32;
        let mut h = base + mountains + hills + detail;
        if self.maria > 0.0 {
            let m = noise::fbm(p / (self.continent_scale_m * 0.7), s ^ 0x61, 4, 0.5);
            let t = smooth(0.0, 0.12, m - (0.5 - self.maria) * 0.5);
            mare = t as f32;
            h = lerp(h, -self.plateau_height_m * 0.8 + detail * 0.2, t * 0.9);
        }

        // Craters.
        if self.crater_max_radius_m > 0.0 {
            h += self.craters(p, finest, &mut mare);
        }

        // Pads: exactly flat inside, blended outside.
        for pad in &self.pads {
            let d = great_circle(dir, pad.direction(), r);
            let w = 1.0 - smooth(pad.radius_m, pad.radius_m + pad.blend_m, d);
            h = lerp(h, pad.height_m, w);
        }
        // A hand-made ground replaces all of the above around the home spot.
        if let Some((t, ground)) = self.overlay.as_ref().and_then(|o| o.sample(dir, r)) {
            h = lerp(h, ground, t);
        }

        // Climate.
        let lat = dir.y.clamp(-1.0, 1.0).asin();
        let c = &self.climate;
        let t_lat = c.pole_c + (c.equator_c - c.pole_c) * (lat.cos() as f32).powf(1.6);
        let temperature_c = t_lat - c.lapse_c_per_m * h.max(0.0) as f32;
        // Wet and dry regions, and woods and fields between them.
        let wet_noise = (noise::fbm(p / 900_000.0, s ^ 0x71, 4, 0.5)
            + noise::fbm(p / 40_000.0, s ^ 0x72, 3, 0.5) * 0.7) as f32;
        let coastal = if self.ocean {
            (1.0 - smooth(0.05, 0.5, cl)) as f32 * 0.2
        } else {
            0.0
        };
        let moisture = (c.wetness + wet_noise * 0.55 + coastal - 0.08).clamp(0.0, 1.0);

        ShapeSample {
            height: h,
            moisture,
            temperature_c,
            mare,
        }
    }

    /// Height only (full detail).
    pub fn height(&self, dir: DVec3) -> f64 {
        self.sample(dir, 0.0).height
    }

    /// Sum of crater profiles at `p` (planet-local, meters), from the largest size class down
    /// to `finest`. Each class scatters at most one crater per cell of a 3D grid; the profile
    /// uses the 3D distance to the crater's center, so centers below or above the surface make
    /// smaller and shallower craters (natural size variety). A crater reaches at most half a
    /// cell, so the 2×2×2 cells around `p` find every crater that touches it.
    fn craters(&self, p: DVec3, finest: f64, mare: &mut f32) -> f64 {
        let mut total = 0.0;
        let mut radius = self.crater_max_radius_m;
        let min_r = self.crater_min_radius_m.max(finest * 0.75);
        let mut class = 0u64;
        while radius >= min_r {
            let cell = radius * 3.2;
            let q = p / cell - 0.5;
            let base = q.floor();
            for dz in 0..2 {
                for dy in 0..2 {
                    for dx in 0..2 {
                        let (cx, cy, cz) =
                            (base.x as i64 + dx, base.y as i64 + dy, base.z as i64 + dz);
                        let seed = self.seed ^ (class * 0x9E37);
                        if noise::cell_random(cx, cy, cz, seed, 1) > self.crater_density {
                            continue;
                        }
                        let center = DVec3::new(
                            cx as f64 + noise::cell_random(cx, cy, cz, seed, 2),
                            cy as f64 + noise::cell_random(cx, cy, cz, seed, 3),
                            cz as f64 + noise::cell_random(cx, cy, cz, seed, 4),
                        ) * cell;
                        let rc = radius * (0.55 + 0.45 * noise::cell_random(cx, cy, cz, seed, 5));
                        let x = (p - center).length() / rc;
                        if x >= 1.5 {
                            continue;
                        }
                        // Bigger craters are shallower relative to size, with flat floors.
                        let depth = rc * if rc > 8_000.0 { 0.06 } else { 0.18 };
                        let rim = rc * 0.045;
                        let bowl = if x < 1.0 {
                            let floor = if rc > 8_000.0 { 0.55 } else { 0.0 };
                            let xf = ((x - floor) / (1.0 - floor)).max(0.0);
                            depth * (xf * xf - 1.0) + rim * smooth(0.6, 1.0, x)
                        } else {
                            rim * (1.0 - smooth(1.0, 1.5, x)) * (1.5 - x) * 2.0
                        };
                        total += bowl;
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
    fn lower_detail_stays_close_to_full_detail() {
        let shape = PlanetShape::earthlike(11);
        for i in 0..50 {
            let d = dir_from_lat_lon(i as f64 * 1.3 - 30.0, i as f64 * 6.1);
            let full = shape.sample(d, 0.0).height;
            let coarse = shape.sample(d, 200.0).height;
            assert!((full - coarse).abs() < 150.0, "{full} vs {coarse}");
        }
    }

    #[test]
    fn lat_lon_round_trip() {
        let d = dir_from_lat_lon(33.0, -120.0);
        let (lat, lon) = lat_lon_from_dir(d);
        assert!((lat - 33.0).abs() < 1e-9 && (lon + 120.0).abs() < 1e-9);
    }
}
