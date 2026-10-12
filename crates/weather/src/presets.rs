//! Named kinds of weather and the numbers behind them.
//!
//! A [`WeatherPreset`] is what a designer, a script or an AI agent asks for ("storm"). Its
//! [`Regime`] is what the simulation moves toward: how much of the sky is covered, what kind
//! of cloud, how hard it rains, how windy and how cold it gets. Regimes blend smoothly, so a
//! front halfway across the sky is simply a mix of two of them.

use engine_core::prelude::*;

/// A kind of weather. Pass it to `weather.set` (by name, lowercase) or [`crate::Weather`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
pub enum WeatherPreset {
    /// Blue sky with a few small fair-weather clouds.
    #[default]
    Clear,
    /// Heaps of cumulus with blue sky between them.
    PartlyCloudy,
    /// A grey sheet of cloud, dry.
    Overcast,
    /// Steady rain from a thick, low cloud deck.
    Rain,
    /// Thunderstorm: towering clouds, downpours, gusts and lightning.
    Storm,
    /// Steady snowfall (forces sub-zero temperatures).
    Snow,
    /// Wind-driven heavy snow with very low visibility.
    Blizzard,
    /// Thick ground fog under a calm, humid sky.
    Fog,
}

impl WeatherPreset {
    /// Every preset, in a stable order (API listings, UI).
    pub const ALL: [WeatherPreset; 8] = [
        WeatherPreset::Clear,
        WeatherPreset::PartlyCloudy,
        WeatherPreset::Overcast,
        WeatherPreset::Rain,
        WeatherPreset::Storm,
        WeatherPreset::Snow,
        WeatherPreset::Blizzard,
        WeatherPreset::Fog,
    ];

    /// Lowercase name used by the API (`"partly_cloudy"`).
    pub fn name(self) -> &'static str {
        match self {
            WeatherPreset::Clear => "clear",
            WeatherPreset::PartlyCloudy => "partly_cloudy",
            WeatherPreset::Overcast => "overcast",
            WeatherPreset::Rain => "rain",
            WeatherPreset::Storm => "storm",
            WeatherPreset::Snow => "snow",
            WeatherPreset::Blizzard => "blizzard",
            WeatherPreset::Fog => "fog",
        }
    }

    /// Parse a name as written by [`WeatherPreset::name`] (also accepts `PartlyCloudy`,
    /// `partly cloudy`, `thunderstorm`, …).
    pub fn from_name(name: &str) -> Option<WeatherPreset> {
        let key: String = name
            .chars()
            .filter(|c| c.is_ascii_alphanumeric())
            .map(|c| c.to_ascii_lowercase())
            .collect();
        Some(match key.as_str() {
            "clear" | "sunny" | "fair" => WeatherPreset::Clear,
            "partlycloudy" | "cloudy" | "cumulus" => WeatherPreset::PartlyCloudy,
            "overcast" | "grey" | "gray" => WeatherPreset::Overcast,
            "rain" | "rainy" | "drizzle" | "showers" => WeatherPreset::Rain,
            "storm" | "thunderstorm" | "thunder" => WeatherPreset::Storm,
            "snow" | "snowy" => WeatherPreset::Snow,
            "blizzard" => WeatherPreset::Blizzard,
            "fog" | "foggy" | "mist" => WeatherPreset::Fog,
            _ => return None,
        })
    }

    /// The numbers the simulation moves toward for this preset.
    pub fn regime(self) -> Regime {
        let base = Regime::default();
        match self {
            WeatherPreset::Clear => Regime {
                cloud_cover: 0.08,
                cloud_density: 0.6,
                cloud_type: 0.45,
                precipitation_mm_h: 0.0,
                wind_speed: 3.0,
                gustiness: 0.3,
                temperature_offset_c: 1.5,
                humidity: 0.45,
                pressure_offset_hpa: 8.0,
                fog: 0.0,
                storm: 0.0,
                ..base
            },
            WeatherPreset::PartlyCloudy => Regime {
                cloud_cover: 0.45,
                cloud_density: 0.8,
                cloud_type: 0.55,
                precipitation_mm_h: 0.0,
                wind_speed: 5.0,
                gustiness: 0.4,
                temperature_offset_c: 0.0,
                humidity: 0.55,
                pressure_offset_hpa: 2.0,
                fog: 0.0,
                storm: 0.0,
                ..base
            },
            WeatherPreset::Overcast => Regime {
                cloud_cover: 0.93,
                cloud_density: 0.9,
                cloud_type: 0.12,
                precipitation_mm_h: 0.0,
                wind_speed: 6.0,
                gustiness: 0.35,
                temperature_offset_c: -2.0,
                humidity: 0.75,
                pressure_offset_hpa: -4.0,
                fog: 0.04,
                storm: 0.0,
                ..base
            },
            WeatherPreset::Rain => Regime {
                cloud_cover: 0.98,
                cloud_density: 1.0,
                cloud_type: 0.3,
                precipitation_mm_h: 5.0,
                wind_speed: 7.0,
                gustiness: 0.5,
                temperature_offset_c: -4.0,
                humidity: 0.93,
                pressure_offset_hpa: -10.0,
                fog: 0.08,
                storm: 0.0,
                ..base
            },
            WeatherPreset::Storm => Regime {
                cloud_cover: 0.9,
                cloud_density: 1.0,
                cloud_type: 1.0,
                precipitation_mm_h: 28.0,
                wind_speed: 13.0,
                gustiness: 0.9,
                temperature_offset_c: -6.0,
                humidity: 0.95,
                pressure_offset_hpa: -16.0,
                fog: 0.1,
                storm: 1.0,
                ..base
            },
            WeatherPreset::Snow => Regime {
                cloud_cover: 0.98,
                cloud_density: 0.9,
                cloud_type: 0.22,
                precipitation_mm_h: 2.5,
                wind_speed: 4.0,
                gustiness: 0.35,
                temperature_offset_c: -6.0,
                temperature_cap_c: -2.0,
                humidity: 0.9,
                pressure_offset_hpa: -8.0,
                fog: 0.2,
                storm: 0.0,
            },
            WeatherPreset::Blizzard => Regime {
                cloud_cover: 1.0,
                cloud_density: 1.0,
                cloud_type: 0.28,
                precipitation_mm_h: 6.0,
                wind_speed: 17.0,
                gustiness: 0.8,
                temperature_offset_c: -12.0,
                temperature_cap_c: -8.0,
                humidity: 0.9,
                pressure_offset_hpa: -14.0,
                fog: 0.55,
                storm: 0.0,
            },
            WeatherPreset::Fog => Regime {
                cloud_cover: 0.35,
                cloud_density: 0.5,
                cloud_type: 0.0,
                precipitation_mm_h: 0.0,
                wind_speed: 1.0,
                gustiness: 0.1,
                temperature_offset_c: -1.0,
                humidity: 0.99,
                pressure_offset_hpa: 4.0,
                fog: 0.85,
                storm: 0.0,
                ..base
            },
        }
    }
}

/// The targets one kind of weather sets for the simulation. Every field blends linearly.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Regime {
    /// Fraction of the sky covered by cloud, 0..1.
    pub cloud_cover: f32,
    /// How much light the clouds swallow, 0..1 (multiplies their extinction).
    pub cloud_density: f32,
    /// Cloud shape: 0 flat stratus sheet, 0.5 cumulus heaps, 1 towering cumulonimbus.
    pub cloud_type: f32,
    /// Rain or snow under a fully covered sky, mm of water per hour.
    pub precipitation_mm_h: f32,
    /// Wind 10 m above the ground, m/s.
    pub wind_speed: f32,
    /// Gusts as a fraction of the wind (0 steady .. 1 very gusty).
    pub gustiness: f32,
    /// Added to the climate's normal temperature, °C.
    pub temperature_offset_c: f32,
    /// The temperature never rises above this, °C (snow presets keep it below freezing).
    pub temperature_cap_c: f32,
    /// Relative humidity the air moves toward, 0..1.
    pub humidity: f32,
    /// Added to 1013 hPa sea-level pressure.
    pub pressure_offset_hpa: f32,
    /// Ground fog, 0..1 (1 ≈ 150 m visibility).
    pub fog: f32,
    /// Thunderstorm activity, 0..1 (lightning, gust fronts).
    pub storm: f32,
}

impl Default for Regime {
    fn default() -> Self {
        Self {
            cloud_cover: 0.3,
            cloud_density: 0.7,
            cloud_type: 0.5,
            precipitation_mm_h: 0.0,
            wind_speed: 4.0,
            gustiness: 0.3,
            temperature_offset_c: 0.0,
            temperature_cap_c: 60.0,
            humidity: 0.55,
            pressure_offset_hpa: 0.0,
            fog: 0.0,
            storm: 0.0,
        }
    }
}

impl Regime {
    /// Blend toward `other` by `t` (0 = self, 1 = other).
    pub fn lerp(&self, other: &Regime, t: f32) -> Regime {
        let t = t.clamp(0.0, 1.0);
        let l = |a: f32, b: f32| a + (b - a) * t;
        Regime {
            cloud_cover: l(self.cloud_cover, other.cloud_cover),
            cloud_density: l(self.cloud_density, other.cloud_density),
            cloud_type: l(self.cloud_type, other.cloud_type),
            precipitation_mm_h: l(self.precipitation_mm_h, other.precipitation_mm_h),
            wind_speed: l(self.wind_speed, other.wind_speed),
            gustiness: l(self.gustiness, other.gustiness),
            temperature_offset_c: l(self.temperature_offset_c, other.temperature_offset_c),
            temperature_cap_c: l(self.temperature_cap_c, other.temperature_cap_c),
            humidity: l(self.humidity, other.humidity),
            pressure_offset_hpa: l(self.pressure_offset_hpa, other.pressure_offset_hpa),
            fog: l(self.fog, other.fog),
            storm: l(self.storm, other.storm),
        }
    }
}

/// Cloud layer thickness for a cloud type (0 stratus .. 1 cumulonimbus), meters.
pub fn cloud_thickness_m(cloud_type: f32) -> f32 {
    let t = cloud_type.clamp(0.0, 1.0);
    if t < 0.5 {
        // Stratus sheet (450 m) to nimbostratus/cumulus (1800 m).
        450.0 + (1800.0 - 450.0) * (t / 0.5)
    } else {
        // Cumulus (1800 m) to cumulonimbus towers reaching the tropopause (10 km).
        let u = (t - 0.5) / 0.5;
        1800.0 + (10_000.0 - 1800.0) * u * u
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_round_trip() {
        for p in WeatherPreset::ALL {
            assert_eq!(WeatherPreset::from_name(p.name()), Some(p));
        }
        assert_eq!(
            WeatherPreset::from_name("Partly Cloudy"),
            Some(WeatherPreset::PartlyCloudy)
        );
        assert_eq!(
            WeatherPreset::from_name("thunderstorm"),
            Some(WeatherPreset::Storm)
        );
        assert_eq!(WeatherPreset::from_name("lava"), None);
    }

    #[test]
    fn regimes_make_sense() {
        let clear = WeatherPreset::Clear.regime();
        let storm = WeatherPreset::Storm.regime();
        assert!(storm.cloud_cover > clear.cloud_cover);
        assert!(storm.precipitation_mm_h > 10.0);
        assert!(storm.wind_speed > clear.wind_speed);
        assert!(WeatherPreset::Snow.regime().temperature_cap_c < 0.0);
        let mid = clear.lerp(&storm, 0.5);
        assert!((mid.storm - 0.5).abs() < 1e-6);
    }

    #[test]
    fn thickness_grows_with_type() {
        assert!(cloud_thickness_m(0.0) < 600.0);
        assert!(cloud_thickness_m(1.0) > 8000.0);
        let mut prev = 0.0;
        for i in 0..=20 {
            let t = cloud_thickness_m(i as f32 / 20.0);
            assert!(t >= prev);
            prev = t;
        }
    }
}
