//! Rain swells the streams and turns them brown (fields-and-forests H2, freshets-plan.md).
//!
//! [`Freshet`] runs one small catchment model around the camera, fed by the rain the weather
//! (and any shower there) reports:
//!
//! - **The flood wave.** The rain fills a linear reservoir (storage S, outflow S / k, the
//!   simplest unit hydrograph, Nash 1957) whose outflow is the stream's quickflow: the water
//!   rises a few minutes after the rain starts and falls back more slowly once it stops.
//!   Real upland catchments take tens of minutes to hours; the game's weather is compressed,
//!   so the catchment is too ([`LAG_S`]).
//! - **Mud.** Suspended sediment climbs steeply with discharge (C = a Q^b, b about 2; sediment
//!   rating curves, Asselman 2000), from a store of loose fines on the bed and banks that the
//!   rising water washes out and dry weather slowly makes ready again. Small streams so clear
//!   before their flood has passed (the clockwise loop, Williams 1989).
//!
//! [`freshet_system`] steps it; [`freshet_regions_system`] swells every simulated stream's
//! springs and clouds its water ([`FlowRegion`]); the creek ribbons read [`Freshet::swell`]
//! and [`Freshet::mud`] when they are laid and drawn.

use std::collections::HashMap;

use engine_core::math::DVec3;
use engine_core::prelude::*;
use engine_render::Camera;
use engine_world::{FocusPoints, WeatherState};

use crate::body::{RainShower, WaterConstituents, WaterOptics};
use crate::flow::FlowRegion;
use crate::puddles::rain_rate;

/// The catchment's lag: the reservoir's time constant, seconds of game time.
pub const LAG_S: f32 = 240.0;
/// Quickflow (mm/h of rain over the catchment) that doubles a stream's base flow.
const DOUBLING_MM_H: f32 = 5.0;
/// Most a stream swells to, times its base flow: a flash flood. Small steep catchments peak
/// at tens of times their base flow in a cloudburst; past about three times the creeks and
/// the showcase stream leave their channels and run over the ground beside them.
pub const MOST_SWELL: f32 = 8.0;
/// Seconds of dry weather for the loose fines to build up again.
const RECHARGE_S: f32 = 1800.0;
/// How fast the risen water washes the loose fines out, per second per unit of swell above 1.
const WASHOUT_PER_S: f32 = 1.0 / 300.0;
/// How fast the brown water spreads into a lake per unit of mud and swell, m/s.
const PLUME_SPREAD_M_S: f32 = 0.3;
/// Widest a plume spreads, meters.
pub const MOST_PLUME_M: f32 = 90.0;
/// Seconds for a plume to clear once no more brown water comes in.
const PLUME_CLEAR_S: f32 = 900.0;
/// The swell or mud has to move this much before the streams are updated.
const STEP: f32 = 0.02;

/// The streams' response to the rain (see the module docs).
#[derive(Resource, Debug, Clone)]
pub struct Freshet {
    /// Discharge as a multiple of each stream's base flow: 1 in dry weather.
    pub swell: f32,
    /// How brown the water runs, 0 clear .. 1 thick with silt.
    pub mud: f32,
    /// Quickflow storage, mm of rain.
    store_mm: f32,
    /// Loose fines ready to wash out, 0..1.
    fines: f32,
    /// Rain forced by a gallery or test, mm/h, instead of the weather's.
    pub forced_mm_h: Option<f32>,
    /// Held where it is (a gallery's flood): [`freshet_system`] leaves it be.
    pub held: bool,
    /// How far the brown water has spread into a lake round where the falls come in,
    /// meters, and how brown it still is there (0..1): a plume builds while the flood runs
    /// in and lingers long after (fine clay settles out of still water over hours; compressed
    /// here like the rest).
    pub plume_m: f32,
    pub plume_mud: f32,
}

impl Default for Freshet {
    fn default() -> Self {
        Self {
            swell: 1.0,
            mud: 0.0,
            store_mm: 0.0,
            fines: 1.0,
            forced_mm_h: None,
            held: false,
            plume_m: 0.0,
            plume_mud: 0.0,
        }
    }
}

impl Freshet {
    /// A flood held at `swell` and `mud` (for galleries and tests).
    pub fn held(swell: f32, mud: f32) -> Self {
        Self {
            swell: swell.clamp(1.0, MOST_SWELL),
            mud: mud.clamp(0.0, 1.0),
            held: true,
            ..Default::default()
        }
    }

    /// How far into spate the streams run: 0 at their usual flow .. 1 at three times it (the
    /// shader roughens and whitens them by it).
    pub fn spate(&self) -> f32 {
        ((self.swell - 1.0) / 2.0).clamp(0.0, 1.0)
    }

    /// `GALLERY_FLOOD=swell,mud[,plume m[,plume mud]]` (say `2.5,0.8,40`), for the galleries'
    /// rain shots.
    pub fn from_env() -> Option<Self> {
        let v = std::env::var("GALLERY_FLOOD").ok()?;
        let mut it = v.split(',').map(|x| x.trim().parse::<f32>().ok());
        let mut f = Self::held(it.next()??, it.next().flatten().unwrap_or(0.0));
        f.plume_m = it.next().flatten().unwrap_or(0.0).clamp(0.0, MOST_PLUME_M);
        f.plume_mud = it.next().flatten().unwrap_or(f.mud).clamp(0.0, 1.0);
        Some(f)
    }

    /// Step `dt` seconds with `rain_mm_h` falling.
    pub fn step(&mut self, rain_mm_h: f32, dt: f32) {
        let k = LAG_S / 3600.0; // hours
        let dt_h = dt / 3600.0;
        // dS/dt = i − S/k, exact for a constant rain over the step.
        let decay = (-dt_h / k).exp();
        self.store_mm = rain_mm_h * k + (self.store_mm - rain_mm_h * k) * decay;
        let quick = self.store_mm / k;
        self.swell = (1.0 + quick / DOUBLING_MM_H).min(MOST_SWELL);
        let rise = self.swell - 1.0;
        // The risen water carries off loose fines; dry weather makes more ready.
        let washout = WASHOUT_PER_S * rise * self.fines;
        let recharge = (1.0 - self.fines) / RECHARGE_S * if rise < 0.05 { 1.0 } else { 0.2 };
        self.fines = (self.fines + (recharge - washout) * dt).clamp(0.0, 1.0);
        // C ∝ Q^2 from what is ready: brown within a doubling of the flow.
        self.mud = (self.fines * (rise * 0.8).powi(2)).min(1.0);
        // The plume: pushed out by the brown water coming in (faster the more of it), browner
        // toward what comes in, and fading slowly once it stops; spent, it shrinks away.
        self.plume_m =
            (self.plume_m + PLUME_SPREAD_M_S * self.mud * self.swell * dt).min(MOST_PLUME_M);
        if self.mud > self.plume_mud {
            self.plume_mud += (self.mud - self.plume_mud) * (dt / 60.0).min(1.0);
        } else {
            self.plume_mud *= (-dt / PLUME_CLEAR_S).exp();
        }
        if self.plume_mud < 0.02 {
            self.plume_m *= (-dt / 120.0).exp();
        }
    }
}

/// Steps [`Freshet`] with the rain at the camera.
pub fn freshet_system(
    time: Res<Time>,
    weather: Res<WeatherState>,
    focus: Res<FocusPoints>,
    showers: Query<(&GlobalTransform, &RainShower)>,
    cameras: Query<(&GlobalTransform, &Camera)>,
    mut freshet: ResMut<Freshet>,
) {
    let dt = time.delta_secs();
    if dt <= 0.0 || freshet.held {
        return;
    }
    let rain = match freshet.forced_mm_h {
        Some(r) => r,
        None => {
            let at: DVec3 = crate::sim::focus_point(&focus, &cameras);
            let showers: Vec<(DVec3, RainShower)> =
                showers.iter().map(|(g, s)| (g.translation(), *s)).collect();
            rain_rate(&weather, &showers, at)
        }
    };
    // Unchanged (dry and drained): leave it be, so nothing downstream sees a change.
    if rain <= 0.0 && freshet.store_mm < 1e-4 && freshet.fines >= 1.0 && freshet.plume_m < 0.01 {
        return;
    }
    freshet.step(rain, dt);
}

/// What a simulated stream was laid with, before the rain: its springs' rates and its look.
#[derive(Debug, Clone)]
pub struct FreshetBase {
    rates: Vec<f32>,
    optics: WaterOptics,
    /// The swell and mud last applied.
    applied: (f32, f32),
}

/// Swells every simulated stream's springs by [`Freshet::swell`] and clouds the water of every
/// flow region by [`Freshet::mud`] (a stream fed from a lake has no springs, but its water
/// runs brown with the rest). Each region's own rates and look are kept here (no component,
/// so no command sync point).
pub fn freshet_regions_system(
    freshet: Res<Freshet>,
    mut regions: Query<(Entity, &mut FlowRegion)>,
    mut bases: Local<HashMap<Entity, FreshetBase>>,
) {
    let silty = flood_optics();
    let (swell, mud) = (freshet.swell, freshet.mud);
    for (entity, region) in &mut regions {
        let base = bases.entry(entity).or_insert_with(|| FreshetBase {
            rates: region.sources.iter().map(|s| s.rate_m3_s).collect(),
            optics: region.optics,
            applied: (1.0, 0.0),
        });
        if (swell - base.applied.0).abs() < STEP && (mud - base.applied.1).abs() < STEP {
            continue;
        }
        let mut region = region;
        if base.rates.len() != region.sources.len() {
            // Its springs were changed by their owner: start again from those.
            base.rates = region.sources.iter().map(|s| s.rate_m3_s).collect();
            base.optics = region.optics;
        }
        for (s, r) in region.sources.iter_mut().zip(&base.rates) {
            if *r > 0.0 {
                s.rate_m3_s = r * swell;
            }
        }
        region.optics = muddy(&base.optics, &silty, mud);
        base.applied = (swell, mud);
    }
    if bases.len() > regions.iter().len() {
        bases.retain(|e, _| regions.contains(*e));
    }
}

/// Flood water: brown with fine silt and clay washed off the land, and the tea-coloured
/// humic matter rain flushes out of the soil (both well up in a storm runoff), so it
/// reads as tan to chocolate rather than the grey of glacial silt.
pub fn flood_optics() -> WaterOptics {
    WaterOptics {
        foam: 1.0,
        ..WaterOptics::from_constituents(WaterConstituents {
            chlorophyll: 0.5,
            cdom_440: 1.5,
            sediment: 30.0,
            sediment_color: [0.66, 0.45, 0.25],
        })
    }
}

/// `clear` clouded toward `silty` by `mud` (0..1): absorption and scattering blended in log
/// space (turbidity spans orders of magnitude), the rest linearly.
pub fn muddy(clear: &WaterOptics, silty: &WaterOptics, mud: f32) -> WaterOptics {
    if mud <= 0.0 {
        return *clear;
    }
    let t = mud.clamp(0.0, 1.0);
    let log = |a: f32, b: f32| (a.max(1e-4).ln() * (1.0 - t) + b.max(1e-4).ln() * t).exp();
    let lin = |a: f32, b: f32| a + (b - a) * t;
    let mut out = *clear;
    for k in 0..3 {
        out.absorption[k] = log(clear.absorption[k], silty.absorption[k]);
        out.scatter_color[k] = lin(clear.scatter_color[k], silty.scatter_color[k]);
    }
    out.scattering = log(clear.scattering, silty.scattering);
    out.backscatter = lin(clear.backscatter, silty.backscatter);
    out.caustics = lin(clear.caustics, silty.caustics);
    out.refraction = lin(clear.refraction, silty.refraction);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_storm_swells_the_stream_and_muddies_it_first() {
        let mut f = Freshet::default();
        let dt = 1.0;
        // Twenty minutes of heavy rain, then dry.
        let (mut peak_swell, mut peak_mud) = ((0.0f32, 0usize), (0.0f32, 0usize));
        let mut swells = Vec::new();
        let mut plume_after = (0.0, 0.0);
        for t in 0..3600 {
            let rain = if t < 1200 { 10.0 } else { 0.0 };
            f.step(rain, dt);
            swells.push(f.swell);
            if t == 1500 {
                plume_after = (f.plume_m, f.plume_mud);
            }
            if f.swell > peak_swell.0 {
                peak_swell = (f.swell, t);
            }
            if f.mud > peak_mud.0 {
                peak_mud = (f.mud, t);
            }
        }
        // It rises over minutes, not at once.
        assert!(swells[30] < 1.3, "{}", swells[30]);
        assert!(peak_swell.0 > 2.5, "{peak_swell:?}");
        // The mud peaks before the flood does, and has mostly cleared while the water is
        // still up.
        assert!(peak_mud.0 > 0.5, "{peak_mud:?}");
        assert!(peak_mud.1 < peak_swell.1, "{peak_mud:?} {peak_swell:?}");
        // And it all drains away again.
        assert!(f.swell < 1.05, "{}", f.swell);
        // The lake's plume grew while the brown water came in, and lingers after the rain.
        assert!(
            plume_after.0 > 40.0 && plume_after.1 > 0.1,
            "{plume_after:?}"
        );
        // Then clears away.
        assert!(f.plume_m < plume_after.0 * 0.5, "{}", f.plume_m);
    }

    #[test]
    fn mud_clouds_the_water() {
        let clear = WaterOptics::stream();
        let silty = flood_optics();
        assert_eq!(muddy(&clear, &silty, 0.0).scattering, clear.scattering);
        let half = muddy(&clear, &silty, 0.5);
        assert!(half.scattering > clear.scattering * 2.0);
        assert!((muddy(&clear, &silty, 1.0).scattering - silty.scattering).abs() < 1e-3);
    }
}
