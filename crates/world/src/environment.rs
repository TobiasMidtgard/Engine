//! Environment fields shared by every simulation.
//!
//! | Resource          | Written by                    | Read by                                   |
//! |-------------------|-------------------------------|-------------------------------------------|
//! | [`WeatherState`]  | `engine_weather`              | render (sky, clouds, fog), audio, fire, foliage, water, NPC AI |
//! | [`WindField`]     | `engine_weather` (+ anyone via [`WindField::add_gust`]) | smoke, fire, particles, foliage sway, cloth, vehicles (aero) |
//! | [`SurfaceLayers`] | weather (rain/snow), water, fire (scorch) | render (wet/snow shading), footsteps, fire, foliage, vehicles (grip) |
//! | [`HeatField`]     | `engine_fire` (and explosions) | weather (convection), foliage, NPC AI, smoke |
//!
//! Fields cover a window around the [`crate::FocusPoints`] (the player/camera) and
//! re-center as it moves. Outside the window, sampling falls back to the global
//! [`WeatherState`] values.

use engine_core::math::{DVec2, DVec3, Vec3};
use engine_core::prelude::*;

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

/// Wind everywhere: prevailing wind with a height profile, gusts, and a local 3D grid of
/// disturbances (explosions, rotor wash, fire updrafts).
#[derive(Resource, Debug, Clone)]
pub struct WindField {
    /// Prevailing wind at 10 m, m/s. Mirrors [`WeatherState::wind`].
    pub prevailing: Vec3,
    pub gustiness: f32,
    /// World time, for gust animation.
    pub time: f32,
    /// Local disturbances added on top, m/s. Decays toward zero.
    pub local: Grid3<Vec3>,
}

impl Default for WindField {
    fn default() -> Self {
        Self {
            prevailing: Vec3::new(3.0, 0.0, 1.0),
            gustiness: 0.3,
            time: 0.0,
            local: Grid3::new(DVec3::new(-64.0, -16.0, -64.0), 4.0, [32, 12, 32], Vec3::ZERO),
        }
    }
}

impl WindField {
    /// Wind velocity at a world position, m/s.
    pub fn sample(&self, p: DVec3) -> Vec3 {
        // Log-ish profile: calmer near the ground, stronger aloft.
        let h = (p.y - self.local.origin.y - 16.0).max(0.5) as f32;
        let profile = (h / 10.0).powf(0.14).clamp(0.3, 2.5);
        let g = engine_core::math::noise::perlin3(
            p.x * 0.02 + self.time as f64 * 0.3,
            p.y * 0.02,
            p.z * 0.02 - self.time as f64 * 0.2,
            0xA11CE,
        );
        let gust = 1.0 + g * self.gustiness;
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
