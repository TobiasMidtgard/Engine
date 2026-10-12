//! Day and night: the clock, the calendar, and the sun and moon lights they drive.
//!
//! Who drives the lights:
//! * Entities tagged [`SunLight`] / [`MoonLight`] (with a `DirectionalLight`) follow the clock.
//! * A world with no directional light at all gets a sun and a moon spawned on the first frame.
//! * A game that lights its world with its own, untagged directional light keeps full control:
//!   the clock then leaves the lights and the time in [`WeatherState`] alone, and the weather
//!   reads the time from there. Tag your light with [`SunLight`] to hand it to the clock.

use engine_core::math::{Quat, Vec3, smoothstep};
use engine_core::prelude::*;
use engine_render::{AmbientLight, DirectionalLight};
use engine_world::WeatherState;

use crate::climate::{self, Climate};
use crate::sim::WeatherSim;

/// The sky clock. Set the hour and date, or let time run with `hours_per_second`.
#[derive(Resource, Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Sky {
    /// Local solar time, hours 0..24 (12 = the sun at its highest).
    pub hour: f32,
    /// Day of the year, 0..365 (80 ≈ spring equinox, 172 ≈ midsummer in the north).
    pub day_of_year: f32,
    /// Game hours that pass per game second (0 stops the clock; 0.1 = a day in 4 minutes).
    pub hours_per_second: f32,
    /// Sunlight above the atmosphere, lux (the renderer dims and reddens it through the air).
    pub sun_lux: f32,
    /// Full-moon light, lux (≈ 0.3 on Earth).
    pub moon_lux: f32,
    /// Moon phase: 0 new, 0.25 first quarter, 0.5 full, 0.75 last quarter. Advances with the
    /// days.
    pub moon_phase: f32,
    /// Keep a faint blue fill light at night so moonless scenes still read (cd/m²).
    pub night_fill: f32,
}

impl Default for Sky {
    fn default() -> Self {
        Self {
            hour: 10.5,
            day_of_year: 80.0,
            hours_per_second: 0.0,
            sun_lux: 120_000.0,
            moon_lux: 0.3,
            moon_phase: 0.42,
            night_fill: 0.1,
        }
    }
}

/// Marks the directional light that is the sun. The sky clock aims and dims it.
#[derive(Component, Debug, Clone, Copy, Default, Serialize, Deserialize)]
pub struct SunLight;

/// Marks the directional light that is the moon.
#[derive(Component, Debug, Clone, Copy, Default, Serialize, Deserialize)]
pub struct MoonLight;

/// What the clock decided this step, for the renderer features and the API.
#[derive(Resource, Debug, Clone, Copy, PartialEq)]
pub struct SkyLights {
    /// Unit vector toward the sun.
    pub sun_dir: Vec3,
    /// Sun elevation, degrees.
    pub sun_elevation_deg: f32,
    pub moon_dir: Vec3,
    /// Fraction of the moon's disc that is lit.
    pub moon_lit: f32,
    /// Sun illuminance before the overcast dimming, lux.
    pub sun_lux: f32,
    /// Factor the sun was dimmed by because thick cloud covers it (1 = not dimmed).
    pub cloud_dimming: f32,
    pub moon_lux: f32,
    /// True while the clock drives the lights and the time (see the module docs).
    pub in_charge: bool,
}

impl Default for SkyLights {
    fn default() -> Self {
        Self {
            sun_dir: Vec3::new(0.3, 0.8, -0.5).normalize(),
            sun_elevation_deg: 50.0,
            moon_dir: Vec3::NEG_Y,
            moon_lit: 0.0,
            sun_lux: 120_000.0,
            cloud_dimming: 1.0,
            moon_lux: 0.0,
            in_charge: false,
        }
    }
}

/// Days in a synodic month (new moon to new moon).
pub const LUNAR_MONTH_DAYS: f32 = 29.53;

/// Spawn a sun and a moon tagged for the clock.
pub fn spawn_sky_lights(commands: &mut Commands) {
    commands.spawn((
        SunLight,
        DirectionalLight {
            illuminance: Sky::default().sun_lux,
            ..Default::default()
        },
        Transform::default(),
    ));
    commands.spawn((
        MoonLight,
        DirectionalLight {
            color: [0.78, 0.84, 1.0],
            illuminance: 0.0,
            cast_shadows: true,
            angular_size_deg: 0.52,
        },
        Transform::default(),
    ));
}

/// First frames: give a world without any directional light a sun and a moon.
#[allow(clippy::type_complexity)]
pub(crate) fn ensure_lights(
    mut commands: Commands,
    mut done: Local<u8>,
    tagged: Query<(), Or<(With<SunLight>, With<MoonLight>)>>,
    any: Query<(), With<DirectionalLight>>,
) {
    // Wait one frame so lights spawned by the game's Startup systems exist.
    if *done >= 2 {
        return;
    }
    *done += 1;
    if *done == 2 && tagged.is_empty() && any.is_empty() {
        spawn_sky_lights(&mut commands);
    }
}

/// FixedUpdate: advance the clock, aim the lights, publish the time.
#[allow(clippy::too_many_arguments, clippy::type_complexity)]
pub(crate) fn sky_system(
    time: Res<Time>,
    mut sky: ResMut<Sky>,
    climate: Res<Climate>,
    sim: Res<WeatherSim>,
    mut state: ResMut<WeatherState>,
    mut lights_out: ResMut<SkyLights>,
    mut ambient: ResMut<AmbientLight>,
    mut suns: Query<(&mut Transform, &mut DirectionalLight), (With<SunLight>, Without<MoonLight>)>,
    mut moons: Query<(&mut Transform, &mut DirectionalLight), (With<MoonLight>, Without<SunLight>)>,
) {
    let in_charge = !suns.is_empty();
    if !in_charge {
        // The game drives its own light: follow its clock.
        *lights_out = SkyLights {
            sun_dir: state.sun_direction,
            sun_elevation_deg: state.sun_direction.y.clamp(-1.0, 1.0).asin().to_degrees(),
            in_charge: false,
            ..*lights_out
        };
        return;
    }

    if sky.hours_per_second != 0.0 {
        let hours = sky.hours_per_second * time.delta_secs();
        let mut s = *sky;
        s.hour += hours;
        let days = s.hour.div_euclid(24.0);
        s.hour = s.hour.rem_euclid(24.0);
        s.day_of_year = (s.day_of_year + days).rem_euclid(climate.year_days);
        s.moon_phase = (s.moon_phase + hours / (LUNAR_MONTH_DAYS * 24.0)).rem_euclid(1.0);
        *sky = s;
    }

    let out = compute_lights(&sky, &climate, &sim);
    *lights_out = out;
    state.time_of_day_h = sky.hour;
    state.day_of_year = sky.day_of_year;
    state.sun_direction = out.sun_dir;

    for (mut t, mut light) in &mut suns {
        aim(&mut t, out.sun_dir);
        light.illuminance = out.sun_lux * out.cloud_dimming;
    }
    for (mut t, mut light) in &mut moons {
        aim(&mut t, out.moon_dir);
        light.illuminance = out.moon_lux * out.cloud_dimming.max(0.05);
    }

    // Night fill: a dim blue constant light so moonless nights still read.
    let night = smoothstep(0.05, -0.2, out.sun_dir.y);
    let day_color = Vec3::new(0.6, 0.7, 1.0);
    let night_color = Vec3::new(0.35, 0.45, 0.8);
    ambient.color = day_color.lerp(night_color, night).to_array();
    ambient.brightness = 0.05 + night * sky.night_fill;
}

/// Where the sun and moon are and how bright, for a clock and climate. `sim` dims the sun
/// under thick cloud.
pub fn compute_lights(sky: &Sky, climate: &Climate, sim: &WeatherSim) -> SkyLights {
    let sun_dir = climate.sun_direction(sky.hour, sky.day_of_year);
    let elevation = sun_dir.y.clamp(-1.0, 1.0).asin().to_degrees();
    let (moon_dir, moon_lit) = climate::moon(climate, sky.hour, sky.day_of_year, sky.moon_phase);
    let moon_elevation = moon_dir.y.clamp(-1.0, 1.0).asin().to_degrees();
    // The renderer shades with the brightest light only, so the sun fades out during
    // nautical twilight and the moon takes over at night.
    let sun_lux = sky.sun_lux * smoothstep(-14.0, -5.0, elevation);
    let moon_lux = sky.moon_lux * moon_lit.powf(1.5) * smoothstep(-1.0, 4.0, moon_elevation);
    SkyLights {
        sun_dir,
        sun_elevation_deg: elevation,
        moon_dir,
        moon_lit,
        sun_lux,
        cloud_dimming: cloud_dimming(sim, sun_dir),
        moon_lux,
        in_charge: true,
    }
}

/// How much thick cloud dims the sun at the player (1 = not at all). Only kicks in when most
/// of the sky is covered, so passing cumulus don't flicker the whole scene; the renderer's
/// cloud shadows (when it has them) handle the patchy case.
pub fn cloud_dimming(sim: &WeatherSim, sun_dir: Vec3) -> f32 {
    let overcast = smoothstep(0.6, 0.95, sim.regional_cover);
    if overcast <= 0.0 {
        return 1.0;
    }
    // Where the sun's ray crosses the middle of the cloud layer.
    let mid = sim.air.cloud_base_m + 0.5 * crate::presets::cloud_thickness_m(sim.local.cloud_type);
    let horizontal = Vec3::new(sun_dir.x, 0.0, sun_dir.z) / sun_dir.y.max(0.15);
    let offset = (horizontal * mid).clamp_length_max(30_000.0);
    let cell =
        sim.cell_at(sim.focus + engine_core::math::DVec2::new(offset.x as f64, offset.z as f64));
    let thickness = crate::presets::cloud_thickness_m(cell.cloud_type);
    let tau = smoothstep(0.3, 0.9, cell.coverage) * cell.density * thickness / 1000.0 * 5.0;
    let transmittance = (-tau).exp();
    1.0 - (1.0 - transmittance) * overcast
}

fn aim(t: &mut Transform, toward: Vec3) {
    let up = if toward.y.abs() > 0.99 {
        Vec3::Z
    } else {
        Vec3::Y
    };
    let look = Transform::default().looking_at(-toward.as_dvec3(), up);
    t.rotation = look.rotation;
    if !t.rotation.is_finite() {
        t.rotation = Quat::IDENTITY;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sun_sets_and_moon_takes_over() {
        let climate = Climate::default();
        let sim = WeatherSim::default();
        let noon = compute_lights(
            &Sky {
                hour: 12.0,
                ..Default::default()
            },
            &climate,
            &sim,
        );
        assert!(noon.sun_lux > 100_000.0);
        let night = compute_lights(
            &Sky {
                hour: 0.0,
                moon_phase: 0.5,
                ..Default::default()
            },
            &climate,
            &sim,
        );
        assert_eq!(night.sun_lux, 0.0);
        assert!(night.moon_lux > 0.2, "full moon at midnight: {night:?}");
        assert!(night.moon_lux > night.sun_lux);
    }
}
