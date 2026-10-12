//! The weather station: a showcase of everything the weather module does, around one point.
//!
//! About 26 × 16 m, built from generated meshes and textures (nothing is loaded from disk):
//!
//! * An asphalt bus yard (wheel ruts, sealed cracks, a patched trench, road markings, a drain
//!   gully and a manhole) where rain pools into puddles that mirror the sky and the lamps,
//!   framed by concrete kerbs; a sand pit edged with sleepers that soaks everything up, and a
//!   stone patio with a sett border in between.
//! * A glazed bus shelter: the ground under its roof stays dry, rain runs off into the gutter
//!   and its ceiling light, LED strip and advert light up at night.
//! * A lattice wind mast with guy wires, a cup anemometer that spins with the wind and a wind
//!   vane that swings into it; a windsock whose fabric bands lift one after another as the wind
//!   picks up and hang limp in calm air.
//! * A fenced instrument enclosure: a louvred Stevenson screen, a rain gauge that fills during
//!   showers, and a snow stake that measures the snow lying on the ground.
//! * A large thermometer whose red column follows the temperature, with a seven-segment
//!   display cycling through temperature, wind speed and rain rate.
//! * Street lamps that switch on at dusk and in dark storms, so night rain glitters in their
//!   light, and a chrome ball that reflects the clouds.
//!
//! Drive it from the console: `weather.set {"preset": "storm"}`, `weather.time {"hour": 21}`.

mod font;
mod meshgen;
mod noise;
mod props;
mod signs;
mod station;
mod texgen;

use engine_assets::{Assets, Handle};
use engine_core::math::{DVec3, Quat, Vec3, smoothstep};
use engine_core::prelude::*;
use engine_render::{MaterialRef, MeshRenderer, PointLight, StandardMaterial};
use engine_world::{PrecipitationKind, SurfaceLayers, WeatherState, WindField};

use crate::sky::SkyLights;

/// Root of a weather station built by [`spawn_demo`]; every part is a descendant.
#[derive(Component, Debug, Clone, Copy, Default)]
pub struct WeatherStation;

/// Windsock swivel: turns the sock downwind. Its fabric is a chain of [`SockSegment`]s.
#[derive(Component, Debug, Clone, Copy, Default)]
pub struct WindSock {
    /// Wind speed the fabric currently responds to (smoothed), m/s.
    pub speed: f32,
}

/// One fabric band of a windsock, `index` 0 at the frame. Each band hangs from the previous
/// one and lifts once the wind is strong enough (about 1.5 m/s per band, level at ~8 m/s).
#[derive(Component, Debug, Clone, Copy)]
pub struct SockSegment {
    pub index: u32,
    /// The [`WindSock`] swivel this band belongs to.
    pub sock: Entity,
}

/// Cup anemometer rotor: spins with the wind speed.
#[derive(Component, Debug, Clone, Copy, Default)]
pub struct Anemometer {
    /// Rotor angle, radians.
    pub angle: f32,
    /// Spin rate, rad/s (the rotor has some inertia).
    pub rate: f32,
}

/// Wind vane: its tail swings downwind (its nose points into the wind) on a damped pivot.
#[derive(Component, Debug, Clone, Copy, Default)]
pub struct WindVane {
    /// Heading of the tail, radians about +Y (0 = tail toward +Z).
    pub yaw: f32,
    /// Turn rate, rad/s.
    pub rate: f32,
}

/// Red column of a thermometer (a mesh one unit tall, scaled): `full_m` tall at `max_c`,
/// empty at `min_c`, standing on `base` (local position).
#[derive(Component, Debug, Clone, Copy)]
pub struct ThermometerColumn {
    pub base: DVec3,
    pub full_m: f32,
    pub min_c: f32,
    pub max_c: f32,
}

/// Water column of a rain gauge (a mesh one unit tall, scaled), filled by rain falling on it
/// and slowly evaporating.
#[derive(Component, Debug, Clone, Copy)]
pub struct RainGauge {
    /// Local position of the bottom of the column.
    pub base: DVec3,
    /// Column height when full, meters.
    pub full_m: f32,
    /// Rain that fills the gauge, mm.
    pub capacity_mm: f32,
    pub collected_mm: f32,
}

/// Snow heaped around a snow stake (a mesh one unit tall, scaled), as deep as the snow lying
/// on the ground there.
#[derive(Component, Debug, Clone, Copy)]
pub struct SnowStake {
    /// Local position of the foot of the stake.
    pub base: DVec3,
}

/// A light that switches on when the sun is low or a storm darkens the sky.
#[derive(Component, Debug, Clone, Copy)]
pub struct StreetLamp {
    /// Luminous flux when fully on, lumens.
    pub lumens: f32,
}

/// Drives a material's emissive luminance between day and night (lamp lenses, the LED strip,
/// the beacon), following the same dusk switch as [`StreetLamp`].
#[derive(Component, Debug, Clone, Copy)]
pub struct Glow {
    pub material: Handle<StandardMaterial>,
    /// Emissive luminance in daylight, cd/m².
    pub day_cd_m2: f32,
    /// Emissive luminance when dark, cd/m².
    pub night_cd_m2: f32,
}

/// One segment of the thermometer's seven-segment display. Positions 0–3 are digits (bits
/// 0–6 = segments a–g), position 4 bit 0 is the decimal point and position 5 bits 0–2 light
/// the unit (°C, m/s, mm/h).
#[derive(Component, Debug, Clone, Copy)]
pub struct LedSegment {
    pub position: u8,
    pub bit: u8,
    pub on: Handle<StandardMaterial>,
    pub off: Handle<StandardMaterial>,
}

/// Spawn the weather station centered on `origin` (its ground surfaces sit at `origin.y`).
/// Textures, meshes and materials are generated once and registered under `weather/station/…`
/// asset keys, so further stations reuse them.
pub fn spawn_demo(commands: &mut Commands, origin: DVec3) {
    commands.queue(move |world: &mut World| station::build(world, origin));
}

/// How dark it is for the lamps: 0 in daylight, 1 after dusk or under a black storm sky.
fn darkness(lights: &SkyLights, state: &WeatherState) -> f32 {
    smoothstep(4.0, -2.0, lights.sun_elevation_deg)
        .max(smoothstep(0.85, 0.98, state.cloud_cover) * smoothstep(0.5, 1.0, state.storm))
}

/// Seven-segment patterns for 0–9 (bit 0 = a … bit 6 = g).
const DIGITS: [u8; 10] = [
    0b011_1111, 0b000_0110, 0b101_1011, 0b100_1111, 0b110_0110, 0b110_1101, 0b111_1101, 0b000_0111,
    0b111_1111, 0b110_1111,
];
const MINUS: u8 = 0b100_0000;

/// Segment masks for the display showing `value` with one decimal in unit slot `unit`:
/// `[digit0, digit1, digit2, digit3, decimal point, units]`.
fn display_masks(value: f32, unit: usize) -> [u8; 6] {
    let tenths = (value.abs() * 10.0).round().min(9999.0) as u32;
    let negative = value < 0.0 && tenths > 0;
    let d = [
        tenths / 1000 % 10,
        tenths / 100 % 10,
        tenths / 10 % 10,
        tenths % 10,
    ];
    let mut m = [0u8; 6];
    m[3] = DIGITS[d[3] as usize];
    m[2] = DIGITS[d[2] as usize];
    let tens = tenths >= 100;
    let hundreds = tenths >= 1000;
    if tens {
        m[1] = DIGITS[d[1] as usize];
    }
    if hundreds {
        m[0] = DIGITS[d[0] as usize];
    }
    if negative {
        // The sign sits right before the first digit (no room past -99.9).
        if tens {
            m[0] = MINUS;
        } else {
            m[1] = MINUS;
        }
    }
    m[4] = 1;
    m[5] = 1 << unit.min(2);
    m
}

/// Change-tracking state of [`instruments_system`].
#[derive(Default)]
pub(crate) struct InstrumentState {
    /// Wind speed at each windsock this frame, for its fabric bands.
    socks: Vec<(Entity, f32)>,
    /// Display masks last written, and how many LED segments they were written to.
    shown: Option<([u8; 6], usize)>,
    /// Weather time at the last update, seconds (the rain gauge integrates over weather
    /// time, so fast-forwarding the weather fills it too).
    weather_s: Option<f64>,
}

/// Update: animate the station's instruments, lamps and display from the weather.
#[allow(clippy::too_many_arguments, clippy::type_complexity)]
pub(crate) fn instruments_system(
    time: Res<Time>,
    wind: Option<Res<WindField>>,
    state: Res<WeatherState>,
    surface: Res<SurfaceLayers>,
    lights: Res<SkyLights>,
    sim: Option<Res<crate::sim::WeatherSim>>,
    materials: Option<ResMut<Assets<StandardMaterial>>>,
    mut parts: ParamSet<(
        Query<(Entity, &mut Transform, &GlobalTransform, &mut WindSock)>,
        Query<(&mut Transform, &SockSegment)>,
        Query<(&mut Transform, &GlobalTransform, &mut Anemometer)>,
        Query<(&mut Transform, &GlobalTransform, &mut WindVane)>,
        Query<(&mut Transform, &ThermometerColumn)>,
        Query<(&mut Transform, &mut RainGauge)>,
        Query<(
            &mut Transform,
            &mut MeshRenderer,
            &GlobalTransform,
            &SnowStake,
        )>,
    )>,
    mut lamps: Query<(&mut PointLight, &StreetLamp)>,
    glows: Query<&Glow>,
    mut leds: Query<(&mut MeshRenderer, &LedSegment), Without<SnowStake>>,
    mut local: Local<InstrumentState>,
) {
    let dt = time.delta_secs();
    let t = time.elapsed_secs();
    let wind_at = |p: DVec3| wind.as_ref().map(|w| w.sample(p)).unwrap_or(state.wind);
    let downwind = |w: Vec3| {
        let flat = Vec3::new(w.x, 0.0, w.z);
        let speed = flat.length();
        (speed, (speed > 0.05).then(|| flat.x.atan2(flat.z)))
    };

    // Windsock: the swivel turns downwind; the fabric responds to a smoothed wind speed.
    local.socks.clear();
    for (e, mut tr, gt, mut sock) in &mut parts.p0() {
        let (speed, yaw) = downwind(wind_at(gt.translation()));
        sock.speed += (speed - sock.speed) * (dt * 2.5).min(1.0);
        if let Some(yaw) = yaw {
            let target = Quat::from_rotation_y(yaw);
            tr.rotation = tr.rotation.slerp(target, (dt * 2.0).min(1.0));
        }
        local.socks.push((e, sock.speed));
    }
    for (mut tr, seg) in &mut parts.p1() {
        let speed = local
            .socks
            .iter()
            .find(|(e, _)| *e == seg.sock)
            .map_or(0.0, |s| s.1);
        let k = seg.index as f32;
        // Each band lifts once the wind can hold it (about 1.5 m/s per band); limp bands
        // fold down at their joint, more toward the tail.
        let lift = smoothstep(1.5 * k + 0.3, 1.5 * k + 2.6, speed);
        let hang = [0.55, 0.35, 0.25, 0.18, 0.12][seg.index.min(4) as usize];
        let droop = hang * (1.0 - lift);
        // Fabric ripple: faster and wider as the wind blows, growing toward the tail.
        let gust = smoothstep(1.0, 14.0, speed);
        let phase = t * (3.0 + 0.9 * speed) - k * 1.3;
        let amp = (0.015 + 0.01 * k) * (0.3 + gust);
        let flap_yaw = amp * phase.sin();
        let flap_pitch = amp * 0.6 * (phase * 1.3 + 0.7).sin();
        tr.rotation = Quat::from_rotation_y(flap_yaw) * Quat::from_rotation_x(droop + flap_pitch);
        // Limp fabric folds flat: narrower across (scale about the hinge axis is safe for the
        // chain), relative to the band before.
        let width = |lift: f32| 0.72 + 0.28 * lift;
        let prev = if seg.index == 0 {
            1.0
        } else {
            width(smoothstep(
                1.5 * (k - 1.0) + 0.3,
                1.5 * (k - 1.0) + 2.6,
                speed,
            ))
        };
        tr.scale = Vec3::new(width(lift) / prev, 1.0, 1.0);
    }

    // Anemometer: the cups run at about a third of the wind speed; the rotor has inertia.
    for (mut tr, gt, mut a) in &mut parts.p2() {
        let (speed, _) = downwind(wind_at(gt.translation()));
        let target = speed * 0.33 / props::CUP_RADIUS;
        a.rate += (target - a.rate) * (dt * 1.5).min(1.0);
        a.angle = (a.angle + a.rate * dt).rem_euclid(std::f32::consts::TAU);
        tr.rotation = Quat::from_rotation_y(a.angle);
    }

    // Wind vane: a damped pivot; the stronger the wind, the stiffer it holds.
    for (mut tr, gt, mut v) in &mut parts.p3() {
        let (speed, yaw) = downwind(wind_at(gt.translation()));
        let steps = (dt / (1.0 / 60.0)).ceil().clamp(1.0, 30.0);
        let h = dt / steps;
        for _ in 0..steps as u32 {
            let err = yaw.map_or(0.0, |y| {
                (y - v.yaw + std::f32::consts::PI).rem_euclid(std::f32::consts::TAU)
                    - std::f32::consts::PI
            });
            let stiffness = 2.0 * speed.min(20.0);
            let damping = 1.2 + 0.25 * speed.min(20.0);
            v.rate += (err * stiffness - v.rate * damping) * h;
            v.yaw += v.rate * h;
        }
        v.yaw = v.yaw.rem_euclid(std::f32::consts::TAU);
        // A little hunting about the mean in gusty wind.
        let hunt = 0.04 * smoothstep(2.0, 12.0, speed) * (t * 1.7).sin() * (t * 0.43).cos();
        tr.rotation = Quat::from_rotation_y(v.yaw + hunt);
    }

    for (mut tr, c) in &mut parts.p4() {
        let f = ((state.temperature_c - c.min_c) / (c.max_c - c.min_c)).clamp(0.02, 1.0);
        tr.scale = Vec3::new(1.0, c.full_m * f, 1.0);
        tr.translation = c.base;
    }

    // Weather time since the last update (a fast-forward integrates at the current rate).
    let weather_dt_s = match (&sim, local.weather_s) {
        (Some(sim), Some(last)) => (sim.time_s - last).clamp(0.0, 86_400.0) as f32,
        _ => dt,
    };
    local.weather_s = sim.as_ref().map(|s| s.time_s);
    let raining = matches!(
        state.precipitation_kind,
        PrecipitationKind::Rain | PrecipitationKind::Sleet | PrecipitationKind::Hail
    );
    for (mut tr, mut g) in &mut parts.p5() {
        let rate = if raining {
            state.precipitation_mm_h
        } else {
            0.0
        };
        // Fills with the rain, evaporates ~1 mm/h, a little faster in the sun.
        let evap = 1.0 + 2.0 * state.daylight();
        g.collected_mm =
            (g.collected_mm + (rate - evap) * weather_dt_s / 3600.0).clamp(0.0, g.capacity_mm);
        let h = (g.full_m * g.collected_mm / g.capacity_mm).max(0.002);
        tr.scale = Vec3::new(1.0, h, 1.0);
        tr.translation = g.base;
    }

    for (mut tr, mut r, gt, s) in &mut parts.p6() {
        let depth = surface.sample(gt.translation()).snow_m.clamp(0.0, 1.5);
        let show = depth > 0.004;
        if r.visible != show {
            r.visible = show;
        }
        tr.translation = s.base;
        tr.scale = Vec3::new(1.0, depth.max(0.001), 1.0);
    }

    // Lamps and glowing materials come on in the dusk and in dark storms.
    let dark = darkness(&lights, &state);
    for (mut l, lamp) in &mut lamps {
        let lm = lamp.lumens * dark;
        if l.intensity_lm != lm {
            l.intensity_lm = lm;
        }
    }
    if let Some(mut materials) = materials {
        for g in &glows {
            let target = g.day_cd_m2 + (g.night_cd_m2 - g.day_cd_m2) * dark;
            let current = materials.get(g.material).map(|m| m.emissive_strength);
            if current.is_some_and(|c| (c - target).abs() > 0.002 * target.max(1.0))
                && let Some(m) = materials.get_mut(g.material)
            {
                m.emissive_strength = target;
            }
        }
    }

    // The display cycles: temperature 6 s, wind speed 3 s, rain rate 3 s.
    let count = leds.iter().len();
    if count > 0 {
        let wind_speed = downwind(state.wind).0;
        let masks = match (t % 12.0) as u32 {
            0..=5 => display_masks(state.temperature_c, 0),
            6..=8 => display_masks(wind_speed, 1),
            _ => display_masks(
                if raining {
                    state.precipitation_mm_h
                } else {
                    0.0
                },
                2,
            ),
        };
        if local.shown != Some((masks, count)) {
            local.shown = Some((masks, count));
            for (mut r, seg) in &mut leds {
                let lit = masks
                    .get(seg.position as usize)
                    .is_some_and(|m| m & (1 << seg.bit) != 0);
                let want = MaterialRef::Handle(if lit { seg.on } else { seg.off });
                if r.material != want {
                    r.material = want;
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn display_formats_values() {
        let m = display_masks(-12.34, 0);
        assert_eq!(m[..4], [MINUS, DIGITS[1], DIGITS[2], DIGITS[3]]);
        assert_eq!(m[5], 1);
        let m = display_masks(5.06, 1);
        assert_eq!(m[..4], [0, 0, DIGITS[5], DIGITS[1]]);
        assert_eq!(m[5], 2);
        let m = display_masks(-0.4, 0);
        assert_eq!(m[..4], [0, MINUS, DIGITS[0], DIGITS[4]]);
        // Rounds to zero: no sign.
        let m = display_masks(-0.01, 2);
        assert_eq!(m[..4], [0, 0, DIGITS[0], DIGITS[0]]);
        assert_eq!(m[5], 4);
    }
}
