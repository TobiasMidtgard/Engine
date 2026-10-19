//! The weather simulation: fronts that carry one kind of weather across the map, the cloud
//! map they paint, and the air at the player (temperature, dew point, pressure, wind, rain).
//!
//! Weather changes the way it does outside: when you ask for a storm, a front forms upwind
//! and travels toward the player. You see the towers on the horizon first, the wind picks up
//! as the gust front arrives, then the rain starts. Everything reads from one function,
//! [`WeatherSim::regime_at`], so the clouds you see, the rain that falls and the numbers in
//! [`WeatherState`] always agree.

use engine_core::math::noise::fbm2;
use engine_core::math::{DQuat, DVec2, DVec3, Rng, Vec2, Vec3, damp, smoothstep};
use engine_core::prelude::*;
use engine_world::{FocusPoints, PrecipitationKind, WeatherState};

use crate::climate::{self, Climate};
use crate::presets::{Regime, WeatherPreset, cloud_thickness_m};

/// What the weather should do. Edit it (or call `weather.set`) to change the weather;
/// [`WeatherState`] is the simulated result and is overwritten every step.
#[derive(Resource, Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Weather {
    /// The weather to move toward. Changing it sends a front across the sky.
    pub preset: WeatherPreset,
    /// Game seconds until a new preset has fully arrived at the player (0 = instantly).
    pub transition_s: f32,
    /// Pick new weather on its own when the current weather has run its course (and the
    /// forecast queue is empty).
    pub auto: bool,
    /// Average game minutes one kind of weather lasts in auto mode.
    pub mean_duration_min: f32,
    /// Upcoming changes, applied in order (`weather.forecast`).
    pub forecast: Vec<ForecastStep>,
    /// Weather runs this many times faster than game time (time-lapse; 1 = real).
    pub time_scale: f32,
    /// Direction the prevailing wind blows from, degrees: 0 north (+Z), 90 east (−X),
    /// 180 south, 270 west (+X). Fronts arrive from here.
    pub wind_from_deg: f32,
    /// Seed for clouds, gusts and auto weather.
    pub seed: u64,
}

impl Default for Weather {
    fn default() -> Self {
        Self {
            preset: WeatherPreset::PartlyCloudy,
            transition_s: 600.0,
            auto: false,
            mean_duration_min: 40.0,
            forecast: Vec::new(),
            time_scale: 1.0,
            wind_from_deg: 250.0,
            seed: 7,
        }
    }
}

/// One queued weather change.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ForecastStep {
    pub preset: WeatherPreset,
    /// Game seconds after the previous step started before this one begins.
    pub after_s: f32,
    /// How long this change takes to arrive, game seconds.
    pub transition_s: f32,
}

impl Default for ForecastStep {
    fn default() -> Self {
        Self {
            preset: WeatherPreset::Clear,
            after_s: 600.0,
            transition_s: 600.0,
        }
    }
}

/// A moving boundary between two kinds of weather.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Front {
    pub preset: WeatherPreset,
    pub to: Regime,
    /// A point on the front line when it formed (pattern X/Z, see [`WeatherFrame`]).
    pub origin: DVec2,
    /// Direction it moves (unit, pattern X/Z).
    pub dir: Vec2,
    /// Speed, m/s of weather time.
    pub speed: f32,
    /// Weather time when it formed, seconds.
    pub start_s: f64,
    /// Half the width of the zone where the two kinds of weather mix, meters.
    pub half_width_m: f32,
}

impl Front {
    /// How much the new weather has taken over at `p` (pattern X/Z) at weather time `t`:
    /// 0 = not yet, 1 = fully.
    pub fn weight(&self, p: DVec2, t: f64) -> f32 {
        let traveled = self.speed as f64 * (t - self.start_s).max(0.0);
        let s = (p - self.origin).dot(self.dir.as_dvec2());
        smoothstep(-self.half_width_m, self.half_width_m, (traveled - s) as f32)
    }

    fn traveled(&self, t: f64) -> f64 {
        self.speed as f64 * (t - self.start_s).max(0.0)
    }
}

/// How far upwind a new front forms, meters (far enough to see it coming).
pub const FRONT_DISTANCE_M: f32 = 30_000.0;
/// Fronts are merged into the background once they are this far past where they formed.
const FRONT_DONE_M: f64 = 220_000.0;
const MAX_FRONTS: usize = 4;

/// The weather at one spot of the cloud map.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct CloudCell {
    /// Sky covered here, 0..1.
    pub coverage: f32,
    /// Cloud type 0 stratus .. 1 cumulonimbus.
    pub cloud_type: f32,
    /// Rain or snow falling here, mm/h.
    pub precipitation_mm_h: f32,
    /// Extinction multiplier 0..1.
    pub density: f32,
    /// Thunderstorm activity 0..1.
    pub storm: f32,
}

/// The air at the player, smoothed over time.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Air {
    pub temperature_c: f32,
    pub dew_point_c: f32,
    pub pressure_hpa: f32,
    /// Wind direction it blows toward (unit X/Z) and speed (m/s).
    pub wind_dir: Vec2,
    pub wind_speed: f32,
    pub gustiness: f32,
    /// Cloud base above sea level, meters.
    pub cloud_base_m: f32,
    pub fog: f32,
}

impl Default for Air {
    fn default() -> Self {
        Self {
            temperature_c: 15.0,
            dew_point_c: 7.0,
            pressure_hpa: 1013.0,
            wind_dir: Vec2::new(0.94, -0.34),
            wind_speed: 4.0,
            gustiness: 0.3,
            cloud_base_m: 1200.0,
            fog: 0.0,
        }
    }
}

/// Where the weather pattern sits in the world. Fronts, the cloud pattern, the wind and the
/// sky's axes live in this frame, which matches the world until the world shifts (a floating
/// origin move or a turned reference frame): then the frame follows, so the clouds and fronts
/// stay over the same ground. `pattern = yaw-rotation · world + offset` (X/Z).
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct WeatherFrame {
    /// Rotation about +Y from world axes to pattern axes, radians.
    pub yaw: f64,
    /// Pattern X/Z of the world origin.
    pub offset: DVec2,
}

impl WeatherFrame {
    fn turn(yaw: f64, v: DVec2) -> DVec2 {
        let r = DQuat::from_rotation_y(yaw) * DVec3::new(v.x, 0.0, v.y);
        DVec2::new(r.x, r.z)
    }

    /// Pattern X/Z of world X/Z `p`.
    pub fn to_pattern(&self, p: DVec2) -> DVec2 {
        Self::turn(self.yaw, p) + self.offset
    }

    /// World X/Z of pattern X/Z `q`.
    pub fn to_world(&self, q: DVec2) -> DVec2 {
        Self::turn(-self.yaw, q - self.offset)
    }

    /// A pattern-axis direction or velocity in world axes.
    pub fn dir_to_world(&self, v: Vec2) -> Vec2 {
        Self::turn(-self.yaw, v.as_dvec2()).as_vec2()
    }

    /// A pattern-axis 3D direction (sun, moon) in world axes.
    pub fn dir3_to_world(&self, v: Vec3) -> Vec3 {
        (DQuat::from_rotation_y(-self.yaw) * v.as_dvec3()).as_vec3()
    }

    /// The world moved: a point at `p` is now at `s.point(p)`. Keep pattern points where they
    /// were on the ground (only the turn about the vertical counts).
    pub fn follow(&mut self, s: &engine_world::WorldShift) {
        // The turn about the vertical, in full precision (`WorldShift::yaw` is f32).
        let q = s.rotation;
        self.yaw -= 2.0 * q.y.atan2(q.w);
        let t = DVec2::new(s.translation.x, s.translation.z);
        self.offset -= Self::turn(self.yaw, t);
    }
}

/// Simulation state. Read it through `weather.get`; change the weather through [`Weather`].
#[derive(Resource, Debug, Clone)]
pub struct WeatherSim {
    /// The weather everywhere no front has reached.
    pub base: Regime,
    pub base_preset: WeatherPreset,
    /// Fronts in the order they formed (newest last).
    pub fronts: Vec<Front>,
    /// Weather time, seconds (game time × `Weather::time_scale`).
    pub time_s: f64,
    /// The preset the last front carries (what `Weather::preset` was when it formed).
    pub target: WeatherPreset,
    /// When auto mode or the forecast next changes the weather (weather time).
    pub next_change_s: f64,
    /// How far the cloud pattern has drifted with the wind, meters (pattern X/Z).
    pub cloud_drift: DVec2,
    /// Where the weather pattern sits in the world.
    pub frame: WeatherFrame,
    pub air: Air,
    /// The weather right at the player.
    pub local: CloudCell,
    /// The regime at the player.
    pub local_regime: Regime,
    /// Average coverage of the sky around the player (what [`WeatherState::cloud_cover`]
    /// reports).
    pub regional_cover: f32,
    /// 0..1 strongest at the edge of an arriving storm (gust front).
    pub gust_front: f32,
    /// Where the player was at the last step (world X/Z).
    pub focus: DVec2,
    pub rng: Rng,
    pub seed: u64,
    initialized: bool,
    /// The next step sets the air (temperature, humidity, pressure, cloud base) straight to
    /// the regime's instead of easing toward it: the weather was switched everywhere at once.
    snap_air: bool,
}

impl Default for WeatherSim {
    fn default() -> Self {
        Self::new(&Weather::default())
    }
}

impl WeatherSim {
    pub fn new(weather: &Weather) -> Self {
        let regime = weather.preset.regime();
        Self {
            base: regime,
            base_preset: weather.preset,
            fronts: Vec::new(),
            time_s: 0.0,
            target: weather.preset,
            next_change_s: f64::INFINITY,
            cloud_drift: DVec2::ZERO,
            frame: WeatherFrame::default(),
            air: Air::default(),
            local: CloudCell::default(),
            local_regime: regime,
            regional_cover: regime.cloud_cover,
            gust_front: 0.0,
            focus: DVec2::ZERO,
            rng: Rng::new(weather.seed),
            seed: weather.seed,
            initialized: false,
            snap_air: false,
        }
    }

    /// The weather regime at world X/Z `p` now.
    pub fn regime_at(&self, p: DVec2) -> Regime {
        self.regime_at_pattern(self.frame.to_pattern(p))
    }

    fn regime_at_pattern(&self, q: DVec2) -> Regime {
        let mut r = self.base;
        for f in &self.fronts {
            r = r.lerp(&f.to, f.weight(q, self.time_s));
        }
        r
    }

    /// The preset that dominates at `p` (the newest front that has mostly arrived there).
    pub fn preset_at(&self, p: DVec2) -> WeatherPreset {
        let q = self.frame.to_pattern(p);
        self.fronts
            .iter()
            .rev()
            .find(|f| f.weight(q, self.time_s) > 0.5)
            .map(|f| f.preset)
            .unwrap_or(self.base_preset)
    }

    /// Cloud and rain at world X/Z `p` now (the same numbers the cloud map and the renderer
    /// use).
    pub fn cell_at(&self, p: DVec2) -> CloudCell {
        let q = self.frame.to_pattern(p);
        cloud_cell(&self.regime_at_pattern(q), q - self.cloud_drift, self.seed)
    }

    /// How much front `f` has taken over at world X/Z `p` (see [`Front::weight`]).
    pub fn front_weight(&self, f: &Front, p: DVec2) -> f32 {
        f.weight(self.frame.to_pattern(p), self.time_s)
    }

    /// Send a front carrying `preset` toward `focus`, arriving in `transition_s` weather
    /// seconds (0 = switch everywhere now).
    pub fn start_front(&mut self, preset: WeatherPreset, transition_s: f32, focus: DVec2) {
        self.target = preset;
        if transition_s <= 0.0 {
            self.base = preset.regime();
            self.base_preset = preset;
            self.fronts.clear();
            // Snow switched on at once falls as snow now, not as rain while the air cools.
            self.snap_air = true;
            return;
        }
        let dir = self.air.wind_dir.normalize_or(Vec2::X);
        let half_width = FRONT_DISTANCE_M * 0.2;
        // Arrive fully (weight ≈ 1) at the focus after `transition_s`.
        let speed = (FRONT_DISTANCE_M + half_width) / transition_s.max(1.0);
        let origin = self.frame.to_pattern(focus) - dir.as_dvec2() * FRONT_DISTANCE_M as f64;
        if self.fronts.len() >= MAX_FRONTS {
            let oldest = self.fronts.remove(0);
            self.base = oldest.to;
            self.base_preset = oldest.preset;
        }
        self.fronts.push(Front {
            preset,
            to: preset.regime(),
            origin,
            dir,
            speed,
            start_s: self.time_s,
            half_width_m: half_width,
        });
    }

    /// Merge fronts that have swept past the whole area into the background weather.
    fn retire_fronts(&mut self) {
        while let Some(f) = self.fronts.first() {
            if f.traveled(self.time_s) < FRONT_DONE_M {
                break;
            }
            let f = self.fronts.remove(0);
            self.base = f.to;
            self.base_preset = f.preset;
        }
    }
}

/// Coverage, type and rain for one spot, given its regime and its position in the drifting
/// cloud pattern. Shared by the CPU (rain at the player) and the cloud map (what you see).
pub fn cloud_cell(r: &Regime, drifted: DVec2, seed: u64) -> CloudCell {
    let c = r.cloud_cover.clamp(0.0, 1.0);
    // Large-scale structure: clusters of cloud and clear gaps, 15–30 km across.
    let n = fbm2(
        drifted.x / 22_000.0,
        drifted.y / 22_000.0,
        seed,
        3,
        2.1,
        0.5,
    );
    let amp = 0.08 + 2.0 * c * (1.0 - c);
    let coverage = (c + n * amp).clamp(0.0, 1.0);
    // Storm cells: towers where the second pattern peaks, lower cloud between them.
    let cells = fbm2(
        drifted.x / 9_000.0 + 17.3,
        drifted.y / 9_000.0 - 4.1,
        seed ^ 0x5717,
        2,
        2.0,
        0.5,
    );
    let towering = smoothstep(0.6, 1.0, r.cloud_type);
    let cellness = smoothstep(-0.05, 0.3, cells);
    let cloud_type = if towering > 0.0 {
        let low = 0.35;
        let tower = low + (r.cloud_type - low) * cellness;
        r.cloud_type + (tower - r.cloud_type) * towering
    } else {
        (r.cloud_type + cells * 0.06).clamp(0.0, 1.0)
    };
    // Layer clouds rain where they're thick; towering clouds rain from their cells even when
    // there's blue sky between them (showers).
    let shower = towering * smoothstep(0.45, 0.9, cellness) * smoothstep(0.3, 0.55, coverage);
    let rainy = smoothstep(0.55, 0.95, coverage).max(shower);
    let intensity = 1.0 + towering * (cellness * 1.3 - 0.55);
    CloudCell {
        coverage,
        cloud_type,
        precipitation_mm_h: (r.precipitation_mm_h * rainy * intensity).max(0.0),
        density: r.cloud_density,
        storm: r.storm * smoothstep(0.3, 0.8, cellness.max(1.0 - towering)) * rainy,
    }
}

/// Run one weather step of `dt` game seconds. Pure function of the inputs, so tools and tests
/// can fast-forward the weather (`weather.advance`).
pub fn step(
    sim: &mut WeatherSim,
    weather: &mut Weather,
    climate: &Climate,
    focus: DVec3,
    hour: f32,
    day_of_year: f32,
    dt: f32,
) {
    let dt_w = dt * weather.time_scale.max(0.0);
    let focus_xz = DVec2::new(focus.x, focus.z);
    sim.focus = focus_xz;
    // Fronts and the cloud pattern live in the weather's own frame.
    let focus_q = sim.frame.to_pattern(focus_xz);
    if sim.seed != weather.seed {
        sim.seed = weather.seed;
        sim.rng = Rng::new(weather.seed);
    }

    // The wind direction comes first: fronts travel with it.
    let wander = engine_core::math::noise::perlin2(sim.time_s / 1800.0, 3.7, sim.seed) * 18.0;
    let from = (weather.wind_from_deg + wander).to_radians();
    // "From" north (+Z) blows toward −Z; from east (−X) blows toward +X.
    let toward = Vec2::new(from.sin(), -from.cos());

    if !sim.initialized {
        sim.initialized = true;
        sim.air.wind_dir = toward;
        sim.base = weather.preset.regime();
        sim.base_preset = weather.preset;
        sim.target = weather.preset;
        sim.fronts.clear();
        sim.next_change_s = schedule_next(sim, weather);
    }

    // Requested changes: the preset field, then the forecast queue, then auto mode.
    if weather.preset != sim.target {
        sim.start_front(weather.preset, weather.transition_s, focus_xz);
        sim.next_change_s = schedule_next(sim, weather);
    } else if sim.time_s >= sim.next_change_s {
        if !weather.forecast.is_empty() {
            let next = weather.forecast.remove(0);
            weather.preset = next.preset;
            sim.start_front(next.preset, next.transition_s, focus_xz);
        } else if weather.auto {
            let next = pick_auto(sim, climate, day_of_year);
            weather.preset = next;
            sim.start_front(next, weather.transition_s, focus_xz);
        }
        sim.next_change_s = schedule_next(sim, weather);
    }

    sim.time_s += dt_w as f64;
    sim.retire_fronts();

    // The weather at the player.
    let regime = sim.regime_at_pattern(focus_q);
    let cell = sim.cell_at(focus_xz);
    sim.local_regime = regime;
    sim.local = cell;
    sim.regional_cover = regional_cover(sim, focus_xz);

    // Gust front: strongest where an arriving storm's front edge is overhead.
    sim.gust_front = sim
        .fronts
        .iter()
        .filter(|f| f.to.storm > 0.3)
        .map(|f| {
            let w = f.weight(focus_q, sim.time_s);
            4.0 * w * (1.0 - w) * f.to.storm
        })
        .fold(0.0, f32::max);

    // Temperature and moisture relax toward the regime with thermal inertia (~25 min).
    let clear = 1.0 - sim.regional_cover * regime.cloud_density.max(0.3);
    let rain_cooling = cell.precipitation_mm_h.min(25.0) * 0.12;
    let target_t = (climate.normal_temperature_c(day_of_year, hour, clear)
        + regime.temperature_offset_c
        - rain_cooling)
        .min(regime.temperature_cap_c);
    let rh_target = if cell.precipitation_mm_h > 0.3 {
        regime.humidity.max(0.96)
    } else {
        regime.humidity
    };
    // Dew point follows the day's mean temperature (moisture doesn't swing with the sun).
    let daily_mean = (climate.normal_temperature_c(day_of_year, 9.5, clear)
        + regime.temperature_offset_c)
        .min(regime.temperature_cap_c);
    let target_dew = climate::dew_point_c(daily_mean, rh_target);
    let rate = 1.0 / 1500.0;
    let snap = std::mem::take(&mut sim.snap_air);
    let ease = sim.time_s > dt_w as f64 * 1.5 && !snap;
    let air = &mut sim.air;
    if dt_w > 0.0 && air.temperature_c.is_finite() && ease {
        air.temperature_c = damp(air.temperature_c, target_t, rate, dt_w);
        air.dew_point_c = damp(air.dew_point_c, target_dew, rate, dt_w);
        air.pressure_hpa = damp(
            air.pressure_hpa,
            1013.0 + regime.pressure_offset_hpa,
            rate,
            dt_w,
        );
    } else {
        air.temperature_c = target_t;
        air.dew_point_c = target_dew;
        air.pressure_hpa = 1013.0 + regime.pressure_offset_hpa;
    }
    air.dew_point_c = air.dew_point_c.min(air.temperature_c);

    // Wind: the regime's speed, a burst in the gust front, veering as fronts pass.
    let veer = sim
        .fronts
        .iter()
        .map(|f| f.weight(focus_q, sim.time_s) * 35f32.to_radians())
        .sum::<f32>();
    let dir = Vec2::from_angle(veer).rotate(toward);
    let speed = regime.wind_speed + sim.gust_front * 9.0;
    let wind_rate = 1.0 / 120.0;
    if air.wind_dir.length_squared() < 0.5 {
        air.wind_dir = dir;
    }
    air.wind_dir = air
        .wind_dir
        .lerp(dir, 1.0 - (-wind_rate * dt_w).exp())
        .normalize_or(dir);
    air.wind_speed = damp(air.wind_speed, speed, 1.0 / 30.0, dt_w);
    air.gustiness = (regime.gustiness + sim.gust_front * 0.5).min(1.0);

    // Clouds drift with the wind aloft (stronger than at the ground).
    let aloft = air.wind_dir * (air.wind_speed * 1.8 + 4.0);
    sim.cloud_drift += aloft.as_dvec2() * dt_w as f64;

    // Cloud base: the lifting condensation level, lower in rain.
    let rh = climate::relative_humidity(air.temperature_c, air.dew_point_c);
    let lcl = climate::cloud_base_m(air.temperature_c, air.dew_point_c);
    let base_target =
        lcl.clamp(300.0, 2800.0) * (1.0 - 0.35 * smoothstep(0.5, 8.0, cell.precipitation_mm_h));
    air.cloud_base_m = if !ease {
        base_target
    } else {
        damp(air.cloud_base_m, base_target, 1.0 / 600.0, dt_w)
    };
    let radiation = climate::radiation_fog(rh, air.wind_speed) * 0.8;
    // Rain and snow haze the air a little too. The veil the clouds draw over the distance does
    // most of it (heavy rain ≈ 1 km, snow ≈ 500 m), so snow adds only a light haze here:
    // the renderer's fog on top of the veil hid the near ground twice over.
    let precip_haze = match kind_for(air.temperature_c) {
        PrecipitationKind::Snow => (cell.precipitation_mm_h * 0.02).min(0.15),
        _ => (cell.precipitation_mm_h * 0.006).min(0.15),
    };
    air.fog = regime.fog.max(radiation) + precip_haze;
}

/// Rain, sleet or snow at a ground temperature.
pub fn kind_for(t_c: f32) -> PrecipitationKind {
    if t_c <= 0.5 {
        PrecipitationKind::Snow
    } else if t_c < 2.0 {
        PrecipitationKind::Sleet
    } else {
        PrecipitationKind::Rain
    }
}

fn regional_cover(sim: &WeatherSim, focus: DVec2) -> f32 {
    // The sky you see: mostly the 15 km around you.
    let mut sum = 0.0;
    let mut n = 0.0;
    for ring in [0.0, 6_000.0, 14_000.0] {
        let taps = if ring == 0.0 { 1 } else { 8 };
        for k in 0..taps {
            let a = k as f64 / taps as f64 * std::f64::consts::TAU;
            let p = focus + DVec2::new(a.cos(), a.sin()) * ring;
            sum += sim.cell_at(p).coverage;
            n += 1.0;
        }
    }
    sum / n
}

fn schedule_next(sim: &mut WeatherSim, weather: &Weather) -> f64 {
    if let Some(next) = weather.forecast.first() {
        return sim.time_s + next.after_s.max(0.0) as f64;
    }
    if weather.auto {
        // Exponentially distributed durations, at least a quarter of the mean.
        let mean = weather.mean_duration_min.max(1.0) as f64 * 60.0;
        let u = sim.rng.f32().clamp(1e-4, 0.9999) as f64;
        return sim.time_s + mean * (0.25 - 0.75 * u.ln());
    }
    f64::INFINITY
}

/// Choose the next weather for auto mode: a Markov chain that prefers gradual changes and
/// turns rain into snow in the cold.
fn pick_auto(sim: &mut WeatherSim, climate: &Climate, day_of_year: f32) -> WeatherPreset {
    use WeatherPreset::*;
    let cold = climate.normal_temperature_c(day_of_year, 12.0, 0.5) < 2.0;
    let weights: &[(WeatherPreset, f32)] = match sim.target {
        Clear => &[
            (Clear, 3.0),
            (PartlyCloudy, 5.0),
            (Fog, 1.0),
            (Overcast, 1.0),
        ],
        PartlyCloudy => &[
            (Clear, 3.0),
            (PartlyCloudy, 2.0),
            (Overcast, 3.0),
            (Showers, 1.5),
            (Storm, 1.5),
            (Rain, 1.0),
        ],
        Showers => &[
            (PartlyCloudy, 3.0),
            (Showers, 1.0),
            (Storm, 1.0),
            (Rain, 1.0),
        ],
        Overcast => &[
            (PartlyCloudy, 3.0),
            (Rain, 4.0),
            (Overcast, 1.0),
            (Fog, 0.5),
        ],
        Rain => &[
            (Overcast, 3.0),
            (PartlyCloudy, 2.0),
            (Showers, 1.5),
            (Rain, 1.0),
            (Storm, 1.0),
        ],
        Storm => &[(Rain, 3.0), (PartlyCloudy, 3.0), (Clear, 1.0)],
        Snow => &[(Overcast, 3.0), (Snow, 1.0), (Blizzard, 1.0), (Clear, 1.0)],
        Blizzard => &[(Snow, 3.0), (Overcast, 2.0)],
        Fog => &[(Clear, 3.0), (PartlyCloudy, 3.0), (Overcast, 1.0)],
    };
    let total: f32 = weights.iter().map(|w| w.1).sum();
    let mut pick = sim.rng.f32() * total;
    let mut choice = weights[0].0;
    for (p, w) in weights {
        if pick < *w {
            choice = *p;
            break;
        }
        pick -= w;
    }
    match (choice, cold) {
        (Rain | Showers, true) => Snow,
        (Storm, true) => Blizzard,
        (Snow, false) => Rain,
        (Blizzard, false) => Storm,
        (p, _) => p,
    }
}

/// Copy the simulation into the shared [`WeatherState`] every other module reads.
pub fn write_state(sim: &WeatherSim, state: &mut WeatherState) {
    let air = &sim.air;
    let cell = &sim.local;
    state.temperature_c = air.temperature_c;
    state.humidity = climate::relative_humidity(air.temperature_c, air.dew_point_c);
    state.pressure_hpa = air.pressure_hpa;
    state.cloud_cover = sim.regional_cover;
    let thickness = cloud_thickness_m(cell.cloud_type);
    state.cloud_density =
        sim.local_regime.cloud_density * (0.5 + 0.5 * smoothstep(400.0, 4000.0, thickness));
    state.cloud_base_m = air.cloud_base_m;
    state.cloud_top_m = air.cloud_base_m + thickness;
    state.precipitation_mm_h = cell.precipitation_mm_h;
    state.precipitation_kind = if cell.precipitation_mm_h > 0.02 {
        kind_for(air.temperature_c)
    } else {
        PrecipitationKind::None
    };
    let wind = sim.frame.dir_to_world(air.wind_dir);
    state.wind = Vec3::new(wind.x, 0.0, wind.y) * air.wind_speed;
    state.gustiness = air.gustiness;
    state.fog = air.fog.clamp(0.0, 1.0);
    state.storm = cell.storm.max(sim.gust_front * 0.5);
    state.lapse_rate = if sim.local_regime.humidity > 0.9 {
        0.005 // saturated air cools more slowly with height
    } else {
        0.0065
    };
}

/// FixedUpdate: advance the weather and publish [`WeatherState`].
pub(crate) fn weather_system(
    time: Res<Time>,
    mut weather: ResMut<Weather>,
    climate: Res<Climate>,
    focus: Res<FocusPoints>,
    mut sim: ResMut<WeatherSim>,
    mut state: ResMut<WeatherState>,
) {
    let (hour, day) = (state.time_of_day_h, state.day_of_year);
    // Only touch `Weather` when the step changed it (keeps change detection meaningful).
    let mut w = weather.clone();
    step(
        &mut sim,
        &mut w,
        &climate,
        focus.primary(),
        hour,
        day,
        time.delta_secs(),
    );
    if w != *weather {
        *weather = w;
    }
    write_state(&sim, &mut state);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(sim: &mut WeatherSim, weather: &mut Weather, seconds: f32) {
        let climate = Climate::default();
        let steps = (seconds / 1.0).ceil() as usize;
        for _ in 0..steps {
            step(sim, weather, &climate, DVec3::ZERO, 14.0, 172.0, 1.0);
        }
    }

    #[test]
    fn a_storm_rolls_in_then_clears() {
        let mut weather = Weather {
            preset: WeatherPreset::Clear,
            transition_s: 600.0,
            ..Default::default()
        };
        let mut sim = WeatherSim::new(&weather);
        let mut state = WeatherState::default();
        run(&mut sim, &mut weather, 5.0);
        write_state(&sim, &mut state);
        let calm_wind = state.wind.length();
        assert!(state.cloud_cover < 0.3, "{}", state.cloud_cover);
        assert_eq!(state.precipitation_mm_h, 0.0);

        weather.preset = WeatherPreset::Storm;
        // Halfway: the storm is visible (sky filling up) before it rains.
        run(&mut sim, &mut weather, 200.0);
        write_state(&sim, &mut state);
        let front = sim.fronts[0];
        let far_upwind = front.origin + front.dir.as_dvec2() * 2_000.0;
        assert!(sim.regime_at(far_upwind).storm > 0.9, "storm upwind first");
        assert!(sim.local_regime.storm < 0.5);

        run(&mut sim, &mut weather, 500.0);
        write_state(&sim, &mut state);
        assert!(state.cloud_cover > 0.75, "cover {}", state.cloud_cover);
        assert!(
            state.wind.length() > calm_wind * 2.0,
            "wind {}",
            state.wind.length()
        );
        assert!(sim.local_regime.storm > 0.95);

        // Clear again: rain stops, sky opens.
        weather.preset = WeatherPreset::Clear;
        run(&mut sim, &mut weather, 900.0);
        write_state(&sim, &mut state);
        assert!(state.cloud_cover < 0.35, "cover {}", state.cloud_cover);
        assert_eq!(state.precipitation_mm_h, 0.0);
        assert!(sim.fronts.len() <= MAX_FRONTS);
    }

    #[test]
    fn rain_falls_under_rain_clouds() {
        let mut weather = Weather {
            preset: WeatherPreset::Rain,
            ..Default::default()
        };
        let mut sim = WeatherSim::new(&weather);
        let mut state = WeatherState::default();
        run(&mut sim, &mut weather, 60.0);
        write_state(&sim, &mut state);
        assert!(state.is_raining(), "{state:?}");
        assert!(state.precipitation_mm_h > 2.0);
        assert!(state.humidity > 0.85);
        assert!(state.cloud_base_m < 1200.0);
    }

    #[test]
    fn snow_preset_is_below_freezing() {
        let mut weather = Weather {
            preset: WeatherPreset::Snow,
            ..Default::default()
        };
        let mut sim = WeatherSim::new(&weather);
        let mut state = WeatherState::default();
        run(&mut sim, &mut weather, 10.0);
        write_state(&sim, &mut state);
        assert!(state.temperature_c < 0.0);
        assert!(state.is_snowing(), "{state:?}");
        assert!(state.fog > 0.1, "snow cuts visibility");
    }

    #[test]
    fn snow_switched_on_at_once_on_a_summer_afternoon_falls_as_snow_now() {
        let mut weather = Weather {
            preset: WeatherPreset::PartlyCloudy,
            transition_s: 0.0,
            ..Default::default()
        };
        let mut sim = WeatherSim::new(&weather);
        let mut state = WeatherState::default();
        run(&mut sim, &mut weather, 600.0);
        write_state(&sim, &mut state);
        assert!(state.temperature_c > 10.0, "{state:?}");
        let cloud_base = state.cloud_base_m;

        weather.preset = WeatherPreset::Snow;
        run(&mut sim, &mut weather, 2.0);
        write_state(&sim, &mut state);
        assert!(state.is_snowing(), "{state:?}");
        assert!(state.cloud_base_m < cloud_base, "{state:?}");

        // A front carrying the weather in still lets the air cool slowly.
        weather.transition_s = 600.0;
        weather.preset = WeatherPreset::Clear;
        run(&mut sim, &mut weather, 2.0);
        write_state(&sim, &mut state);
        assert!(state.temperature_c < 2.0, "{state:?}");
    }

    #[test]
    fn forecast_runs_in_order_and_auto_keeps_changing() {
        let mut weather = Weather {
            preset: WeatherPreset::Clear,
            forecast: vec![
                ForecastStep {
                    preset: WeatherPreset::Overcast,
                    after_s: 10.0,
                    transition_s: 0.0,
                },
                ForecastStep {
                    preset: WeatherPreset::Rain,
                    after_s: 10.0,
                    transition_s: 0.0,
                },
            ],
            ..Default::default()
        };
        let mut sim = WeatherSim::new(&weather);
        run(&mut sim, &mut weather, 12.0);
        assert_eq!(weather.preset, WeatherPreset::Overcast);
        run(&mut sim, &mut weather, 12.0);
        assert_eq!(weather.preset, WeatherPreset::Rain);
        assert!(weather.forecast.is_empty());

        let mut weather = Weather {
            auto: true,
            mean_duration_min: 1.0,
            transition_s: 0.0,
            ..Default::default()
        };
        let mut sim = WeatherSim::new(&weather);
        let mut seen = std::collections::HashSet::new();
        for _ in 0..200 {
            run(&mut sim, &mut weather, 30.0);
            seen.insert(weather.preset);
        }
        assert!(seen.len() >= 4, "auto weather visited {seen:?}");
    }

    #[test]
    fn nights_are_colder_and_clouds_keep_them_mild() {
        let climate = Climate::default();
        let mut temps = Vec::new();
        for preset in [WeatherPreset::Clear, WeatherPreset::Overcast] {
            let mut weather = Weather {
                preset,
                ..Default::default()
            };
            let mut sim = WeatherSim::new(&weather);
            let mut lo = f32::MAX;
            let mut hi = f32::MIN;
            for i in 0..(48 * 60) {
                let hour = (i as f32 / 60.0) % 24.0;
                step(
                    &mut sim,
                    &mut weather,
                    &climate,
                    DVec3::ZERO,
                    hour,
                    172.0,
                    60.0,
                );
                if i > 24 * 60 {
                    lo = lo.min(sim.air.temperature_c);
                    hi = hi.max(sim.air.temperature_c);
                }
            }
            temps.push(hi - lo);
        }
        assert!(temps[0] > 6.0, "clear daily range {}", temps[0]);
        assert!(
            temps[1] < temps[0],
            "overcast range {} vs clear {}",
            temps[1],
            temps[0]
        );
    }
}
