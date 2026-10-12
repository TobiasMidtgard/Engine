//! Where the world is on its planet, and the physics of air that every other part uses: the
//! sun's and moon's positions, seasonal and daily temperature, dew point and humidity.
//!
//! World axes: +Y up, **north = +Z, east = −X** (so the sun rises in −X and, in the northern
//! hemisphere, stands in −Z at noon).

use engine_core::math::{TAU, Vec3, smoothstep};
use engine_core::prelude::*;

/// The local climate: latitude, season and how warm it is on average. Change it to move the
/// world to the tropics, a desert or an ice planet.
#[derive(Resource, Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Climate {
    /// Latitude, degrees (+ north). Sets day length, the sun's height and the seasons.
    pub latitude_deg: f32,
    /// The planet's axial tilt, degrees (Earth 23.44; 0 = no seasons).
    pub axial_tilt_deg: f32,
    /// Year-round average sea-level temperature, °C.
    pub mean_temperature_c: f32,
    /// Half the difference between summer and winter, °C.
    pub seasonal_swing_c: f32,
    /// Half the difference between afternoon and pre-dawn under a clear sky, °C. Clouds and
    /// rain shrink it.
    pub daily_swing_c: f32,
    /// Length of the year in days.
    pub year_days: f32,
}

impl Default for Climate {
    fn default() -> Self {
        Self {
            latitude_deg: 35.0,
            axial_tilt_deg: 23.44,
            mean_temperature_c: 15.0,
            seasonal_swing_c: 9.0,
            daily_swing_c: 6.0,
            year_days: 365.25,
        }
    }
}

impl Climate {
    /// Normal sea-level temperature for a day of the year and hour, before weather, °C.
    /// `clear_sky` (0..1) scales the daily swing (cloudy days and nights stay mild).
    pub fn normal_temperature_c(&self, day_of_year: f32, hour: f32, clear_sky: f32) -> f32 {
        // Warmest about 40 days after the summer solstice (thermal lag), coldest in winter.
        let hemisphere = if self.latitude_deg >= 0.0 { 1.0 } else { -1.0 };
        let season = -(TAU * (day_of_year - narrow_lag(self.year_days)) / self.year_days).cos();
        let seasonal = self.seasonal_swing_c * season * hemisphere;
        // Warmest at 15:00, coldest just before dawn.
        let daily = (TAU * (hour - 15.0) / 24.0).cos();
        let swing = self.daily_swing_c * (0.25 + 0.75 * clear_sky.clamp(0.0, 1.0));
        self.mean_temperature_c + seasonal + swing * daily
    }

    /// Unit vector toward the sun (see the module docs for axes).
    pub fn sun_direction(&self, hour: f32, day_of_year: f32) -> Vec3 {
        let decl = self.solar_declination(day_of_year);
        celestial_direction(hour, decl, self.latitude_deg.to_radians())
    }

    /// The sun's declination for a day of the year, radians.
    pub fn solar_declination(&self, day_of_year: f32) -> f32 {
        -self.axial_tilt_deg.to_radians() * (TAU * (day_of_year + 10.0) / self.year_days).cos()
    }

    /// Hour of sunrise and sunset (local solar time), or `None` in polar day/night.
    pub fn sunrise_sunset(&self, day_of_year: f32) -> Option<(f32, f32)> {
        let decl = self.solar_declination(day_of_year);
        let lat = self.latitude_deg.to_radians();
        let c = -(lat.tan() * decl.tan());
        if !(-1.0..=1.0).contains(&c) {
            return None;
        }
        let h0 = c.acos() / TAU * 24.0;
        Some((12.0 - h0, 12.0 + h0))
    }
}

/// Thermal lag of the seasons: coldest ~3 weeks after the winter solstice.
fn narrow_lag(year_days: f32) -> f32 {
    20.0 * year_days / 365.25
}

/// Direction to a body with declination `decl` (radians) at local solar `hour` (12 = due
/// south for the northern hemisphere) from latitude `lat` (radians).
pub fn celestial_direction(hour: f32, decl: f32, lat: f32) -> Vec3 {
    let h = (hour - 12.0) / 24.0 * TAU;
    let up = lat.sin() * decl.sin() + lat.cos() * decl.cos() * h.cos();
    let east = -decl.cos() * h.sin();
    let north = lat.cos() * decl.sin() - lat.sin() * decl.cos() * h.cos();
    // east = −X, north = +Z.
    Vec3::new(-east, up, north).normalize_or(Vec3::Y)
}

/// Moon: phase 0 = new, 0.5 = full. Returns (direction, fraction of the disc lit 0..1).
/// Simplified orbit in the plane of the sun's path: good enough for lighting and the sky.
pub fn moon(climate: &Climate, hour: f32, day_of_year: f32, phase: f32) -> (Vec3, f32) {
    let sun_decl = climate.solar_declination(day_of_year);
    let decl = sun_decl * (TAU * phase).cos();
    // The moon trails the sun by `phase` of a day.
    let moon_hour = (hour - phase * 24.0).rem_euclid(24.0);
    let dir = celestial_direction(moon_hour, decl, climate.latitude_deg.to_radians());
    let lit = 0.5 * (1.0 - (TAU * phase).cos());
    (dir, lit)
}

/// Saturation vapor pressure over water, hPa (Magnus formula).
pub fn saturation_vapor_pressure_hpa(t_c: f32) -> f32 {
    6.112 * (17.62 * t_c / (243.12 + t_c)).exp()
}

/// Relative humidity (0..1) of air at `t_c` with dew point `dew_c`.
pub fn relative_humidity(t_c: f32, dew_c: f32) -> f32 {
    (saturation_vapor_pressure_hpa(dew_c.min(t_c)) / saturation_vapor_pressure_hpa(t_c))
        .clamp(0.0, 1.0)
}

/// Dew point of air at `t_c` with relative humidity `rh` (0..1), °C.
pub fn dew_point_c(t_c: f32, rh: f32) -> f32 {
    let g = rh.clamp(0.01, 1.0).ln() + 17.62 * t_c / (243.12 + t_c);
    243.12 * g / (17.62 - g)
}

/// Height of the cloud base above the ground (lifting condensation level), meters:
/// about 125 m per °C between air temperature and dew point.
pub fn cloud_base_m(t_c: f32, dew_c: f32) -> f32 {
    125.0 * (t_c - dew_c).max(0.0)
}

/// How likely radiation fog is, 0..1: near-saturated, calm air.
pub fn radiation_fog(rh: f32, wind_speed: f32) -> f32 {
    smoothstep(0.93, 0.995, rh) * (1.0 - smoothstep(1.5, 5.0, wind_speed))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sun_rises_east_and_stands_south_at_noon() {
        let c = Climate::default();
        let morning = c.sun_direction(7.0, 80.0);
        assert!(
            morning.x < -0.5,
            "morning sun should be east (−X): {morning:?}"
        );
        let noon = c.sun_direction(12.0, 80.0);
        assert!(noon.y > 0.75 && noon.z < -0.4, "noon {noon:?}");
        let evening = c.sun_direction(17.0, 80.0);
        assert!(
            evening.x > 0.5,
            "evening sun should be west (+X): {evening:?}"
        );
        let midnight = c.sun_direction(0.0, 80.0);
        assert!(midnight.y < -0.5);
    }

    #[test]
    fn equinox_day_is_twelve_hours_and_summer_is_longer() {
        let c = Climate::default();
        let (rise, set) = c.sunrise_sunset(80.0).unwrap();
        assert!(
            (rise - 6.0).abs() < 0.15 && (set - 18.0).abs() < 0.15,
            "{rise} {set}"
        );
        let (rise, set) = c.sunrise_sunset(172.0).unwrap();
        assert!(set - rise > 13.5, "{rise} {set}");
        let arctic = Climate {
            latitude_deg: 80.0,
            ..c
        };
        assert!(arctic.sunrise_sunset(172.0).is_none(), "midnight sun");
    }

    #[test]
    fn temperature_peaks_in_the_afternoon_and_summer() {
        let c = Climate::default();
        let dawn = c.normal_temperature_c(200.0, 5.0, 1.0);
        let afternoon = c.normal_temperature_c(200.0, 15.0, 1.0);
        assert!(afternoon - dawn > 10.0);
        let cloudy =
            c.normal_temperature_c(200.0, 15.0, 0.0) - c.normal_temperature_c(200.0, 5.0, 0.0);
        assert!(cloudy < afternoon - dawn);
        assert!(
            c.normal_temperature_c(200.0, 12.0, 1.0)
                > c.normal_temperature_c(20.0, 12.0, 1.0) + 12.0
        );
    }

    #[test]
    fn humidity_round_trips() {
        for t in [-10.0, 0.0, 15.0, 30.0] {
            for rh in [0.2, 0.5, 0.9, 1.0] {
                let d = dew_point_c(t, rh);
                assert!(d <= t + 1e-3);
                assert!((relative_humidity(t, d) - rh).abs() < 0.01, "t {t} rh {rh}");
            }
        }
        assert!((cloud_base_m(20.0, 10.0) - 1250.0).abs() < 1.0);
    }

    #[test]
    fn full_moon_rises_at_sunset() {
        let c = Climate::default();
        let (dir, lit) = moon(&c, 0.0, 80.0, 0.5);
        assert!(lit > 0.99);
        assert!(dir.y > 0.6, "full moon high at midnight: {dir:?}");
        let (dir, _) = moon(&c, 12.0, 80.0, 0.5);
        assert!(dir.y < -0.6, "full moon below at noon");
    }
}
