//! Day–night cycle for the showcase: moves the sun, writes `WeatherState::time_of_day_h` and
//! `sun_direction`, and raises a cool ambient fill at night (moonlight stand-in).
//!
//! When `engine_weather` owns the day cycle, this file goes away and the showcase sets the
//! weather module's clock instead.

use engine::prelude::*;

/// The showcase clock. Registered, so `resource.set` / the AI bridge can change it.
#[derive(Resource, Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct DayCycle {
    /// Local solar time, hours 0..24.
    pub hour: f32,
    /// Game hours per real second (0 stops the clock).
    pub hours_per_second: f32,
    /// Sun illuminance at noon above the atmosphere, lux.
    pub sun_lux: f32,
}

impl Default for DayCycle {
    fn default() -> Self {
        Self {
            hour: 10.5,
            hours_per_second: 0.0,
            sun_lux: 120_000.0,
        }
    }
}

/// Marks the directional light the cycle drives.
#[derive(Component)]
pub struct Sun;

/// Unit vector toward the sun for an hour of the day. The sun rises in −X, sets in +X and
/// peaks about 66° up, slightly to the −Z side.
pub fn sun_direction(hour: f32) -> Vec3 {
    let theta = (hour - 12.0) / 24.0 * std::f32::consts::TAU;
    Vec3::new(theta.sin() * 0.9, theta.cos() * 0.8, -0.35).normalize()
}

pub fn spawn_sun(commands: &mut Commands) {
    commands.spawn((
        Sun,
        DirectionalLight {
            illuminance: DayCycle::default().sun_lux,
            ..Default::default()
        },
        Transform::default(),
    ));
}

pub fn advance(
    time: Res<Time>,
    mut cycle: ResMut<DayCycle>,
    mut weather: ResMut<WeatherState>,
    mut ambient: ResMut<AmbientLight>,
    mut sun: Query<(&mut Transform, &mut DirectionalLight), With<Sun>>,
) {
    if cycle.hours_per_second != 0.0 {
        cycle.hour = (cycle.hour + cycle.hours_per_second * time.real_delta_secs()).rem_euclid(24.0);
    }
    let dir = sun_direction(cycle.hour);
    weather.time_of_day_h = cycle.hour;
    weather.sun_direction = dir;

    for (mut t, mut light) in &mut sun {
        // The renderer attenuates the sun through the atmosphere and hides it below the
        // horizon, so the illuminance stays physical here.
        light.illuminance = cycle.sun_lux;
        let up = if dir.y.abs() > 0.99 { Vec3::Z } else { Vec3::Y };
        *t = Transform::default().looking_at(-dir.as_dvec3(), up);
    }

    // Night fill: a dim, blue-ish ambient so moonlit scenes read (cd/m²).
    let night = engine::core::math::smoothstep(0.05, -0.2, dir.y);
    let day_color = Vec3::new(0.6, 0.7, 1.0);
    let night_color = Vec3::new(0.35, 0.45, 0.8);
    ambient.color = day_color.lerp(night_color, night).to_array();
    ambient.brightness = 0.05 + night * 0.1;
}
