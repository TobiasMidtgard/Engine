//! Environment fields shared by every simulation.
//!
//! | Resource          | Written by                    | Read by                                   |
//! |-------------------|-------------------------------|-------------------------------------------|
//! | [`WeatherState`]  | `engine_weather`              | render (sky, clouds, fog), audio, fire, foliage, water, NPC AI |
//! | [`WindField`]     | `engine_weather` (+ anyone via [`WindField::add_gust`]) | smoke, fire, particles, foliage sway, cloth, vehicles (aero) |
//! | [`SurfaceLayers`] | weather (rain/snow), water, fire (scorch) | render (wet/snow shading), footsteps, fire, foliage, vehicles (grip) |
//! | [`HeatField`]     | `engine_fire` (and explosions) | weather (convection), foliage, NPC AI, smoke |
//! | [`FarCloudLayer`] | `engine_space` (cloud tops seen from high up) | weather (fades its volumetric clouds where the far layer takes over) |
//! | [`CloudCoverMap`] | `engine_weather` (its cloud map's coverage) | space (the cloud shell's tops over the weather's window) |
//! | [`CloudCoverField`] | `engine_space` (the planet's cloud systems around the focus) | weather (its clouds' cover away from the player) |
//! | [`SunSource`]     | `engine_space` (the real star, when it follows the world clock) | weather (aims its sun and sky lights from it) |
//!
//! Fields cover a window around the [`crate::FocusPoints`] (the player/camera) and
//! re-center as it moves. Outside the window, sampling falls back to the global
//! [`WeatherState`] values.

use engine_core::math::{DVec2, DVec3, Vec3};
use engine_core::prelude::*;
use std::sync::Arc;

use crate::grid::{Grid2, Grid3};

/// What falls from the sky.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum PrecipitationKind {
    #[default]
    None,
    Rain,
    Snow,
    Sleet,
    Hail,
}

/// Global (regional) weather around the focus. One source of truth for sky, clouds, rain,
/// temperature and wind.
#[derive(Resource, Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct WeatherState {
    /// Air temperature at sea level near the focus, °C.
    pub temperature_c: f32,
    /// Temperature drop per meter of altitude (°C/m). Standard atmosphere ≈ 0.0065.
    pub lapse_rate: f32,
    /// Relative humidity 0..1.
    pub humidity: f32,
    /// Sea-level pressure, hPa.
    pub pressure_hpa: f32,
    /// Sky fraction covered by clouds 0..1.
    pub cloud_cover: f32,
    /// How thick/dark clouds are 0..1 (storm clouds ≈ 1).
    pub cloud_density: f32,
    /// Cloud layer bottom and top, meters above sea level.
    pub cloud_base_m: f32,
    pub cloud_top_m: f32,
    /// Precipitation rate in mm/hour (light rain ≈ 1, heavy ≈ 10, storm ≈ 50).
    pub precipitation_mm_h: f32,
    pub precipitation_kind: PrecipitationKind,
    /// Prevailing wind at 10 m height, m/s (world space).
    pub wind: Vec3,
    /// Gust strength as a fraction of the prevailing wind (0 calm, 1 very gusty).
    pub gustiness: f32,
    /// Ground fog density 0..1.
    pub fog: f32,
    /// Thunderstorm activity 0..1.
    pub storm: f32,
    /// Hours, 0..24 (written by the day/night cycle).
    pub time_of_day_h: f32,
    /// Day of year, 0..365 (seasons).
    pub day_of_year: f32,
    /// Unit direction toward the sun (world space).
    pub sun_direction: Vec3,
}

impl Default for WeatherState {
    fn default() -> Self {
        Self {
            temperature_c: 16.0,
            lapse_rate: 0.0065,
            humidity: 0.55,
            pressure_hpa: 1013.0,
            cloud_cover: 0.35,
            cloud_density: 0.45,
            cloud_base_m: 1500.0,
            cloud_top_m: 4000.0,
            precipitation_mm_h: 0.0,
            precipitation_kind: PrecipitationKind::None,
            wind: Vec3::new(3.0, 0.0, 1.0),
            gustiness: 0.3,
            fog: 0.05,
            storm: 0.0,
            time_of_day_h: 10.0,
            day_of_year: 172.0,
            sun_direction: Vec3::new(0.3, 0.8, 0.5).normalize(),
        }
    }
}

impl WeatherState {
    /// Air temperature at an altitude (meters), °C.
    pub fn temperature_at_altitude(&self, altitude_m: f64) -> f32 {
        self.temperature_c - self.lapse_rate * altitude_m.max(0.0) as f32
    }

    pub fn is_raining(&self) -> bool {
        self.precipitation_mm_h > 0.05
            && matches!(
                self.precipitation_kind,
                PrecipitationKind::Rain | PrecipitationKind::Sleet
            )
    }

    pub fn is_snowing(&self) -> bool {
        self.precipitation_mm_h > 0.05 && self.precipitation_kind == PrecipitationKind::Snow
    }

    /// 0 (night) .. 1 (sun high). Smooth around sunrise/sunset.
    pub fn daylight(&self) -> f32 {
        engine_core::math::smoothstep(-0.1, 0.25, self.sun_direction.y)
    }
}

/// A tester's wind: when `speed_m_s` is set, [`WindField::prevailing`] blows at that speed
/// (m/s at 10 m) in the weather's direction, whatever the weather says, so sway, smoke and
/// cloth can be tried in a gale. The weather itself (sky, fronts, wind sounds) is unchanged.
/// The F3 panel sets it with PageUp / PageDown and clears it with Home.
#[derive(Resource, Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct WindOverride {
    pub speed_m_s: Option<f32>,
}

impl WindOverride {
    /// Speeds the panel steps through, m/s: calm to violent storm, roughly one Beaufort step
    /// each.
    pub const STEPS: [f32; 12] = [
        0.0, 1.0, 2.5, 4.5, 7.0, 9.5, 12.5, 15.5, 19.0, 22.5, 26.5, 31.0,
    ];

    /// The next step above (`up`) or below the speed now (`from`, the wind blowing).
    pub fn step(from: f32, up: bool) -> f32 {
        let s = Self::STEPS;
        if up {
            s.into_iter()
                .find(|&v| v > from + 0.25)
                .unwrap_or(s[s.len() - 1])
        } else {
            s.into_iter()
                .rev()
                .find(|&v| v < from - 0.25)
                .unwrap_or(0.0)
        }
    }

    /// The prevailing wind for this weather wind.
    pub fn apply(&self, weather: Vec3) -> Vec3 {
        match self.speed_m_s {
            Some(speed) => weather.normalize_or(Vec3::X) * speed.max(0.0),
            None => weather,
        }
    }
}

/// Wind everywhere: prevailing wind with a height profile, gusts, and a local 3D grid of
/// disturbances (explosions, rotor wash, fire updrafts).
#[derive(Resource, Debug, Clone)]
pub struct WindField {
    /// Prevailing wind at 10 m, m/s. Mirrors [`WeatherState::wind`] (or a [`WindOverride`]).
    pub prevailing: Vec3,
    pub gustiness: f32,
    /// World time, for gust animation.
    pub time: f32,
    /// Where the gust pattern is anchored, world meters. World shifts move it, so gusts don't
    /// jump when the origin does.
    pub gust_origin: DVec3,
    /// Local disturbances added on top, m/s. Decays toward zero.
    pub local: Grid3<Vec3>,
    /// Wind fronts crossing the land, which every shader moving something in the wind
    /// evaluates the same way (see [`crate::gusts`]). The weather writes and advances them;
    /// with strength 0 they add nothing.
    pub waves: crate::gusts::GustWaves,
}

impl Default for WindField {
    fn default() -> Self {
        Self {
            prevailing: Vec3::new(3.0, 0.0, 1.0),
            gustiness: 0.3,
            time: 0.0,
            gust_origin: DVec3::ZERO,
            local: Grid3::new(
                DVec3::new(-64.0, -16.0, -64.0),
                4.0,
                [32, 12, 32],
                Vec3::ZERO,
            ),
            waves: Default::default(),
        }
    }
}

impl WindField {
    /// Wind velocity at a world position, m/s.
    pub fn sample(&self, p: DVec3) -> Vec3 {
        // Log-ish profile: calmer near the ground, stronger aloft.
        let h = (p.y - self.local.origin.y - 16.0).max(0.5) as f32;
        let profile = (h / 10.0).powf(0.14).clamp(0.3, 2.5);
        let q = p - self.gust_origin;
        let g = engine_core::math::noise::perlin3(
            q.x * 0.02 + self.time as f64 * 0.3,
            q.y * 0.02,
            q.z * 0.02 - self.time as f64 * 0.2,
            0xA11CE,
        );
        let waves = self
            .waves
            .value(engine_core::math::Vec2::new(q.x as f32, q.z as f32));
        let gust = 1.0 + g * self.gustiness + waves;
        self.prevailing * profile * gust + self.local.sample(p)
    }

    /// Add a local wind disturbance of `strength` m/s at `center`, fading to zero at `radius`.
    /// `direction: None` pushes radially outward (explosions); `Some(dir)` pushes along `dir`
    /// (rotor wash, a dragon's wingbeat, a fire's updraft with `Vec3::Y`).
    pub fn add_gust(&mut self, center: DVec3, radius: f32, strength: f32, direction: Option<Vec3>) {
        let r_cells = (radius / self.local.cell).ceil() as i32;
        let c = self.local.cell_at(center);
        for z in -r_cells..=r_cells {
            for y in -r_cells..=r_cells {
                for x in -r_cells..=r_cells {
                    let cell = c + engine_core::math::IVec3::new(x, y, z);
                    let cc = self.local.cell_center(cell);
                    let d = (cc - center).length() as f32;
                    if d > radius {
                        continue;
                    }
                    let falloff = 1.0 - d / radius.max(1e-3);
                    let dir = direction
                        .map(|d| d.normalize_or_zero())
                        .unwrap_or_else(|| (cc - center).as_vec3().normalize_or_zero());
                    if let Some(v) = self.local.get_mut(cell) {
                        *v += dir * strength * falloff;
                    }
                }
            }
        }
    }

    /// Decay local disturbances (call once per step from the wind owner).
    pub fn decay(&mut self, dt: f32, rate: f32) {
        let k = (-rate * dt).exp();
        for v in &mut self.local.data {
            *v *= k;
        }
    }
}

/// Per-cell ground surface state around the focus.
#[derive(Debug, Clone, Copy, PartialEq, Default, Serialize, Deserialize)]
pub struct SurfaceCell {
    /// 0 dry .. 1 soaked (darkens and adds gloss to surfaces, slows fire, makes mud).
    pub wetness: f32,
    /// Standing water depth, meters (puddles).
    pub puddle_m: f32,
    /// Snow depth, meters.
    pub snow_m: f32,
    /// 0 untouched .. 1 fully burnt/charred.
    pub scorch: f32,
}

impl crate::grid::Blend for SurfaceCell {
    fn blend(a: Self, b: Self, t: f32) -> Self {
        Self {
            wetness: a.wetness + (b.wetness - a.wetness) * t,
            puddle_m: a.puddle_m + (b.puddle_m - a.puddle_m) * t,
            snow_m: a.snow_m + (b.snow_m - a.snow_m) * t,
            scorch: a.scorch + (b.scorch - a.scorch) * t,
        }
    }
}

/// Ground surface state (wetness, puddles, snow, scorch) on a grid around the focus.
#[derive(Resource, Debug, Clone)]
pub struct SurfaceLayers {
    pub grid: Grid2<SurfaceCell>,
}

impl Default for SurfaceLayers {
    fn default() -> Self {
        Self {
            grid: Grid2::centered(DVec2::ZERO, 512.0, 1.0, SurfaceCell::default()),
        }
    }
}

impl SurfaceLayers {
    pub fn sample(&self, p: DVec3) -> SurfaceCell {
        self.grid.sample(p)
    }

    pub fn modify(&mut self, p: DVec3, f: impl FnOnce(&mut SurfaceCell)) {
        let c = self.grid.cell_at(p);
        if let Some(cell) = self.grid.get_mut(c) {
            f(cell);
        }
    }
}

/// Ground surface state far away, on a coarse grid to the horizon: how much of the ground
/// is wet and how much under snow, so a snowfall whitens the whole valley and the hills and
/// not only the [`SurfaceLayers`] around the player.
#[derive(Debug, Clone, Copy, PartialEq, Default, Serialize, Deserialize)]
pub struct FarSurfaceCell {
    /// Share of the ground in the cell that is wet, 0..1.
    pub wet_share: f32,
    /// Share of the ground in the cell under snow, 0..1 (before the snow line, which
    /// [`FarSurface::sample`] adds).
    pub snow_cover: f32,
}

impl crate::grid::Blend for FarSurfaceCell {
    fn blend(a: Self, b: Self, t: f32) -> Self {
        Self {
            wet_share: a.wet_share + (b.wet_share - a.wet_share) * t,
            snow_cover: a.snow_cover + (b.snow_cover - a.snow_cover) * t,
        }
    }
}

/// [`FarSurfaceCell`]s to the horizon, and the snow line. Written by the weather (wet share
/// and snow cover from where it has rained and snowed, the line from temperature and lapse
/// rate); read by terrain, foliage and water for what is past the near layers. Readers blend
/// it under [`SurfaceLayers`] where those end.
#[derive(Resource, Debug, Clone)]
pub struct FarSurface {
    /// 16 km at 64 m by default, centered on the focus by its writer.
    pub grid: Grid2<FarSurfaceCell>,
    /// World height above which the ground lies under snow, meters (infinite: no snow line).
    pub snow_line_m: f64,
    /// How far below and above the line the snow thins out and thickens, meters.
    pub snow_line_fade_m: f32,
}

impl Default for FarSurface {
    fn default() -> Self {
        Self {
            grid: Grid2::centered(DVec2::ZERO, 16_384.0, 64.0, FarSurfaceCell::default()),
            snow_line_m: f64::INFINITY,
            snow_line_fade_m: 150.0,
        }
    }
}

impl FarSurface {
    /// The cell at world `p` (bilinear), with snow above the snow line at `p`'s height.
    pub fn sample(&self, p: DVec3) -> FarSurfaceCell {
        let mut c = self.grid.sample(p);
        if self.snow_line_m.is_finite() {
            let fade = self.snow_line_fade_m.max(1.0) as f64;
            let t = ((p.y - (self.snow_line_m - fade)) / (2.0 * fade)).clamp(0.0, 1.0) as f32;
            c.snow_cover = c.snow_cover.max(t * t * (3.0 - 2.0 * t));
        }
        c
    }
}

/// Extra air/ground temperature from fires and other heat sources, °C above ambient.
#[derive(Resource, Debug, Clone)]
pub struct HeatField {
    pub grid: Grid2<f32>,
}

impl Default for HeatField {
    fn default() -> Self {
        Self {
            grid: Grid2::centered(DVec2::ZERO, 512.0, 1.0, 0.0),
        }
    }
}

impl HeatField {
    pub fn sample(&self, p: DVec3) -> f32 {
        self.grid.sample(p)
    }
}

/// A far cloud layer drawn by another module, such as space's cloud tops on a planet seen from
/// high up. The weather's volumetric clouds hand over to it between `inner_m` and `outer_m` of
/// ground distance from the camera, so the two meet without a seam or a doubled layer.
///
/// `weight` (0..1) is how much of the far layer is drawn now (space fades its shell in with
/// altitude). The writer removes the resource when it draws no far layer; without it the
/// weather's clouds draw as they always did. Readers use [`FarCloudLayer::near_share`].
#[derive(Resource, Debug, Clone, Copy, PartialEq, Default)]
pub struct FarCloudLayer {
    /// Ground distance from the camera (m) where the near clouds start to fade.
    pub inner_m: f32,
    /// Ground distance (m) where the far layer has fully taken over.
    pub outer_m: f32,
    /// How much of the far layer is drawn, 0..1.
    pub weight: f32,
}

impl FarCloudLayer {
    /// How much of the near (volumetric) clouds to keep at `ground_distance_m` from the camera:
    /// 1 inside `inner_m`, falling smoothly to `1 - weight` at `outer_m` and beyond.
    pub fn near_share(&self, ground_distance_m: f32) -> f32 {
        let span = (self.outer_m - self.inner_m).max(1e-3);
        let t = ((ground_distance_m - self.inner_m) / span).clamp(0.0, 1.0);
        let handed_over = t * t * (3.0 - 2.0 * t);
        1.0 - self.weight.clamp(0.0, 1.0) * handed_over
    }
}

/// Where the clouds are around the focus: the cloud cover the near (volumetric) clouds are carved
/// from, so a far layer drawn by another module (space's cloud shell) puts its clouds in the
/// same places instead of a pattern of its own. One map drives every cloud drawn, as in
/// Horizon Zero Dawn's Nubis or Flight Simulator's weather grid.
///
/// A square of `size` × `size` cells of `cell_m` on world X/Z, laid out from `origin` (the
/// minimum corner), rows along +Z. The pattern drifts with the wind: what the map holds at
/// point `p` is now at `p + drift_m`, so read the cover at `q` from `q - drift_m`
/// ([`CloudCoverMap::cover_at`] does). The writer inserts the resource while it draws clouds,
/// rebuilds it after world shifts (in `PostUpdate`, after `WorldShiftSet::Apply`) and bumps
/// `revision` whenever `coverage` changes; `drift_m` moves every frame without a new
/// revision. Without the resource, readers draw as they would with no weather.
#[derive(Resource, Debug, Clone, PartialEq)]
pub struct CloudCoverMap {
    /// World X/Z of the map's minimum corner (texel 0's outer corner), meters.
    pub origin: DVec2,
    /// Cell size, meters.
    pub cell_m: f32,
    /// Cells per side.
    pub size: u32,
    /// Cover per cell, 0..255 for 0..1, `size * size` of them, rows along +Z.
    pub coverage: Arc<[u8]>,
    /// How far the pattern has moved since the map was built (world X/Z, meters).
    pub drift_m: DVec2,
    /// Cover beyond the map, 0..1.
    pub background: f32,
    /// Increases whenever `coverage`, `origin` or the layout changes (re-upload then).
    pub revision: u64,
}

impl Default for CloudCoverMap {
    fn default() -> Self {
        Self {
            origin: DVec2::ZERO,
            cell_m: 1000.0,
            size: 0,
            coverage: Arc::from([]),
            drift_m: DVec2::ZERO,
            background: 0.0,
            revision: 0,
        }
    }
}

impl CloudCoverMap {
    /// Cover (0..1) at world X/Z `p` now, bilinear between cell centers, `background` beyond
    /// the map.
    pub fn cover_at(&self, p: DVec2) -> f32 {
        let n = self.size as usize;
        if n == 0 || self.coverage.len() < n * n {
            return self.background;
        }
        let f = (p - self.drift_m - self.origin) / self.cell_m as f64 - DVec2::splat(0.5);
        let span = n as f64;
        if f.x < -0.5 || f.y < -0.5 || f.x > span - 0.5 || f.y > span - 0.5 {
            return self.background;
        }
        let (x0, z0) = (f.x.floor(), f.y.floor());
        let (tx, tz) = ((f.x - x0) as f32, (f.y - z0) as f32);
        let at = |x: f64, z: f64| {
            let (x, z) = (
                x.clamp(0.0, span - 1.0) as usize,
                z.clamp(0.0, span - 1.0) as usize,
            );
            self.coverage[z * n + x] as f32 / 255.0
        };
        let a = at(x0, z0) + (at(x0 + 1.0, z0) - at(x0, z0)) * tx;
        let b = at(x0, z0 + 1.0) + (at(x0 + 1.0, z0 + 1.0) - at(x0, z0 + 1.0)) * tx;
        a + (b - a) * tz
    }
}

/// The other direction from [`CloudCoverMap`]: the large-scale cloud systems around the focus
/// (fronts, clear zones hundreds of kilometers across) as a module drawing the whole planet
/// sees them, on the same kind of grid (coarser and wider, such as 512 km of 2 km cells).
/// The weather keeps its own cover at the player (its preset and fronts) and lets the field
/// take over with distance, so what you see from the ground out to the horizon is the system
/// you saw from orbit.
///
/// The writer inserts it while a planet's cloud systems are drawn, re-centers it as the focus
/// moves, and republishes it after world shifts; it removes it when there is no such planet.
#[derive(Resource, Debug, Clone, PartialEq, Default)]
pub struct CloudCoverField(pub CloudCoverMap);

impl std::ops::Deref for CloudCoverField {
    type Target = CloudCoverMap;
    fn deref(&self) -> &CloudCoverMap {
        &self.0
    }
}

impl std::ops::DerefMut for CloudCoverField {
    fn deref_mut(&mut self) -> &mut CloudCoverMap {
        &mut self.0
    }
}

#[cfg(test)]
mod cloud_cover_tests {
    use super::*;

    #[test]
    fn cover_is_read_where_the_pattern_drifted() {
        // 2 x 2 cells of 100 m from (-100, -100): clear on the left (x < 0), cloudy on the right.
        let mut map = CloudCoverMap {
            origin: DVec2::splat(-100.0),
            cell_m: 100.0,
            size: 2,
            coverage: Arc::from([0u8, 255, 0, 255]),
            background: 0.25,
            ..Default::default()
        };
        assert_eq!(map.cover_at(DVec2::new(-50.0, -50.0)), 0.0);
        assert_eq!(map.cover_at(DVec2::new(50.0, 50.0)), 1.0);
        assert!((map.cover_at(DVec2::new(0.0, 0.0)) - 0.5).abs() < 1e-6);
        assert_eq!(map.cover_at(DVec2::new(500.0, 0.0)), 0.25);
        // The wind carried the pattern 100 m toward -X: the cloudy half is now left of 0.
        map.drift_m = DVec2::new(-100.0, 0.0);
        assert_eq!(map.cover_at(DVec2::new(-50.0, 0.0)), 1.0);
        assert_eq!(CloudCoverMap::default().cover_at(DVec2::ZERO), 0.0);
    }
}

#[cfg(test)]
mod far_cloud_tests {
    use super::FarCloudLayer;

    #[test]
    fn near_clouds_hand_over_to_the_far_layer_by_its_weight() {
        let layer = FarCloudLayer {
            inner_m: 42_000.0,
            outer_m: 60_000.0,
            weight: 1.0,
        };
        assert_eq!(layer.near_share(10_000.0), 1.0);
        assert_eq!(layer.near_share(42_000.0), 1.0);
        assert!((layer.near_share(51_000.0) - 0.5).abs() < 1e-4);
        assert_eq!(layer.near_share(60_000.0), 0.0);
        assert_eq!(layer.near_share(120_000.0), 0.0);

        let half = FarCloudLayer {
            weight: 0.5,
            ..layer
        };
        assert!((half.near_share(90_000.0) - 0.5).abs() < 1e-6);
        let none = FarCloudLayer {
            weight: 0.0,
            ..layer
        };
        assert_eq!(none.near_share(90_000.0), 1.0);
    }
}

#[cfg(test)]
mod wind_override_tests {
    use super::*;

    #[test]
    fn override_sets_the_speed_in_the_weathers_direction() {
        let weather = Vec3::new(3.0, 0.0, 4.0);
        assert_eq!(WindOverride::default().apply(weather), weather);
        let gale = WindOverride {
            speed_m_s: Some(19.0),
        }
        .apply(weather);
        assert!((gale.length() - 19.0).abs() < 1e-4);
        assert!(gale.normalize().dot(weather.normalize()) > 0.9999);
        assert_eq!(WindOverride::step(5.0, true), 7.0);
        assert_eq!(WindOverride::step(7.0, false), 4.5);
        assert_eq!(WindOverride::step(0.0, false), 0.0);
        assert_eq!(WindOverride::step(31.0, true), 31.0);
    }
}

/// The real star's direction this frame: unit vector in world space from the ground toward the
/// star, so the sun the weather lights the world with is the star the sky shows, also after
/// the world re-anchors on another body or re-homes far from the valley
/// (engine-design/one-sun.md, playtest 3 item 36).
///
/// The writer (the space module, which already finds the star for its own light) inserts it
/// every frame it knows the star, and removes it when it doesn't. The weather keeps its clock
/// (hours, days, the moon) either way; with the resource it aims the sun and its sky lights
/// from `direction`, without it from its own formula for the home spot.
#[derive(Resource, Debug, Clone, Copy, PartialEq)]
pub struct SunSource {
    /// Unit vector toward the star, world space.
    pub direction: Vec3,
}

/// The real moon this frame: the body the night sky shows, so the moon the weather lights the
/// night with is that body, where it stands and as full as it is (round 23 on Tobias's PC found
/// two moons: the weather's own and the space module's, 18-19 degrees apart).
///
/// The writer (the space module, next to [`SunSource`]) inserts it every frame it knows a moon
/// in the sky (the non-star body that looks largest from the ground, other than the one under
/// it) and removes it when there is none. Readers (the weather's moon light) aim at
/// `direction`, scale their light by `lit_fraction`, and draw no disc of their own when
/// `drawn_by_space` (the body is drawn already); without the resource they keep their own
/// formula.
#[derive(Resource, Debug, Clone, Copy, PartialEq)]
pub struct MoonSource {
    /// Unit vector toward the moon's center, world space.
    pub direction: Vec3,
    /// Fraction of its disc the star lights as seen from here: 0 new, 1 full.
    pub lit_fraction: f32,
    /// Its apparent diameter from here, degrees (the Moon from Earth: about 0.52).
    pub angular_diameter_deg: f32,
    /// The space module draws the body (lit, with its phase): draw no disc for it.
    pub drawn_by_space: bool,
}

impl MoonSource {
    /// The lit fraction of a sphere at `moon` seen from `eye` with its star at `star` (any one
    /// frame, any units): `(1 + cos(phase angle)) / 2`, the phase angle being the angle at the
    /// moon between the star and the eye.
    pub fn lit_fraction_of(eye: DVec3, moon: DVec3, star: DVec3) -> f32 {
        let (to_star, to_eye) = (
            (star - moon).normalize_or_zero(),
            (eye - moon).normalize_or_zero(),
        );
        (0.5 * (1.0 + to_star.dot(to_eye))).clamp(0.0, 1.0) as f32
    }
}

#[cfg(test)]
mod moon_source_tests {
    use super::*;

    #[test]
    fn a_moon_opposite_the_star_is_full_and_one_beside_it_new() {
        let eye = DVec3::ZERO;
        let star = DVec3::new(1.5e11, 0.0, 0.0);
        let full = MoonSource::lit_fraction_of(eye, DVec3::new(-3.8e8, 0.0, 0.0), star);
        assert!(full > 0.999, "{full}");
        let new = MoonSource::lit_fraction_of(eye, DVec3::new(3.8e8, 0.0, 0.0), star);
        assert!(new < 0.001, "{new}");
        // 170 degrees from the star (round 23's night): nearly full, not a crescent.
        let a = 170f64.to_radians();
        let m = DVec3::new(a.cos(), a.sin(), 0.0) * 3.8e8;
        let k = MoonSource::lit_fraction_of(eye, m, star);
        assert!(k > 0.99, "{k}");
        let quarter = MoonSource::lit_fraction_of(eye, DVec3::new(0.0, 3.8e8, 0.0), star);
        assert!((quarter - 0.5).abs() < 0.01, "{quarter}");
    }
}
