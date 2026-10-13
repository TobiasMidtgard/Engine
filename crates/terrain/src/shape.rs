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
    /// 0..1 strength of water and ice erosion: valleys, spurs, gullies and couloirs cut down
    /// the slopes of mountains and hills, flat valley floors (0 = none, as on the Moon).
    pub erosion: f64,
    /// Height of the rock ledges that steep eroded mountainsides break into, meters (a
    /// second set a third as high sits on top; 0 = none).
    pub ledge_height_m: f64,
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
            detail_height_m: 10.0,
            detail_scale_m: 400.0,
            min_detail_m: 1.0,
            erosion: 1.0,
            ledge_height_m: 60.0,
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
            erosion: 0.0,
            ledge_height_m: 0.0,
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
            erosion: 0.0,
            ledge_height_m: 0.0,
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
                let half_width = 90_000.0 + 0.5 * su.max(D::ZERO) + ragged * 40_000.0;
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
                    let most = if eroding { 6.0 } else { 9.0 };
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
                    let mut relief = 400.0 + 5_200.0 * ridge.powf(1.5);
                    if !eroding {
                        // Crags: sharp sub-ridges and gullies on the flanks.
                        let crags = noise::ridged(p / 3_000.0, s ^ 0x35, octaves_for(3_000.0, 7.0));
                        relief = relief + 700.0 * crags * crags * ridge;
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
                D::c(noise::ridged(p / scale, s ^ 0x32, octaves_for(scale, 9.0)))
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
            D::c((hn * hn + 0.003).sqrt() - 0.055)
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
            0.5 + 4.0 * mask
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

        // Maria: dark flooded basins (flat, low).
        let mut mare = 0.0f32;
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
            + noise::fbm_smooth(p / 40_000.0, s ^ 0x72, octaves_for(40_000.0, 3.0), 0.5) * 0.7)
            as f32;
        let coastal = if self.ocean {
            (1.0 - smooth(0.05, 0.5, cl.v)) as f32 * 0.2
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
                        if x >= 1.5 {
                            continue;
                        }
                        // Bigger craters are shallower relative to size, with flat floors.
                        // Old craters are worn down: shallower, with low rims.
                        let wear = 1.0 - 0.65 * smooth(0.1, 0.9, rnd(6));
                        let depth = rc * if rc > 8_000.0 { 0.06 } else { 0.18 } * wear;
                        let rim = rc * 0.045 * wear * wear;
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
