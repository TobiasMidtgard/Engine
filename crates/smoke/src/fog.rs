//! Fog banks: ground fog that fills low ground, drifts with the wind and burns off.
//!
//! A [`FogBank`] sits on an entity with a [`SmokeVolume`] and fills that volume's low ground
//! with fog up to a level: hilltops stick out of it, valleys and hollows fill. The fog is
//! simulated like the smoke (it is white, chilled air in the same volume), so the wind carries
//! it, bullets punch holes through it, fans and vehicles stir it and NPCs can't see through it
//! (`Obscurance`), and it forms again behind them. It follows the weather (`WeatherState::fog`)
//! unless given a fixed amount: it thickens on calm, humid nights and burns off by day.

use engine_core::math::Vec3;
use engine_core::prelude::*;
use engine_world::WeatherState;

use crate::domain::FogRates;
use crate::settings::SmokeSettings;
use crate::sim::{SmokeDomains, SmokeVolume};

/// Ground fog over the floor of the [`SmokeVolume`] on the same entity.
#[derive(Component, Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct FogBank {
    /// How much fog, 0..1. Negative follows the weather (`WeatherState::fog`).
    pub amount: f32,
    /// How far you can see inside full fog, meters (Koschmieder: 3.9 / extinction).
    pub visibility_m: f32,
    /// Height of the fog's top above the lowest ground in the volume at full amount, meters
    /// (less fog is shallower).
    pub depth_m: f32,
    /// How softly the top fades out, meters.
    pub top_fade_m: f32,
    /// How much colder than the air the fog forms, °C: cold air sinks, so it drains off slopes
    /// and pools in hollows.
    pub chill_c: f32,
    /// Seconds for fog to form back where it was cleared (1/e).
    pub form_secs: f32,
    /// Seconds for fog to burn off when there should be less of it (1/e).
    pub burn_secs: f32,
    /// Fog color (linear albedo).
    pub color: [f32; 3],
}

impl Default for FogBank {
    fn default() -> Self {
        Self {
            amount: -1.0,
            visibility_m: 60.0,
            depth_m: 12.0,
            top_fade_m: 4.0,
            chill_c: 2.0,
            form_secs: 40.0,
            burn_secs: 60.0,
            color: [0.95, 0.96, 0.97],
        }
    }
}

impl FogBank {
    /// A bank with a fixed amount of fog (0..1), whatever the weather.
    pub fn fixed(amount: f32) -> Self {
        Self {
            amount,
            ..Default::default()
        }
    }

    /// The amount of fog for `weather_fog` (`WeatherState::fog`).
    pub fn amount_for(&self, weather_fog: f32) -> f32 {
        if self.amount < 0.0 {
            weather_fog
        } else {
            self.amount
        }
        .clamp(0.0, 1.0)
    }

    /// Fog density for a smoke extinction of `extinction_m2_kg` (kg/m³).
    pub fn density(&self, extinction_m2_kg: f32) -> f32 {
        3.912 / self.visibility_m.max(1.0) / extinction_m2_kg.max(1.0)
    }

    /// Share of full fog (0..1) at world height `y`, with `amount` of fog over ground whose
    /// lowest point is `low_y`. Thicker low down, fading out over the top.
    pub fn level(&self, amount: f32, low_y: f64, y: f64) -> f32 {
        if amount <= 0.0 {
            return 0.0;
        }
        let depth = (self.depth_m * amount.sqrt()).max(0.5) as f64;
        let fade = (self.top_fade_m.max(0.5) as f64).min(depth);
        let top = low_y + depth;
        let h = ((top - y) / fade).clamp(0.0, 1.0) as f32;
        let edge = h * h * (3.0 - 2.0 * h);
        let below = ((y - low_y) / depth).clamp(0.0, 1.0) as f32;
        amount.sqrt() * edge * (1.0 - 0.35 * below)
    }
}

/// Seconds for foggy air to chill back down after mixing (1/e): ground fog keeps losing heat
/// to the sky, so it stays colder than the air above it.
const CHILL_SECS: f32 = 2.0;

/// Form and burn off the fog of every [`FogBank`] (fixed step, before the smoke steps).
pub fn settle_fog(
    time: Res<Time>,
    settings: Res<SmokeSettings>,
    weather: Res<WeatherState>,
    banks: Query<(Entity, &FogBank), With<SmokeVolume>>,
    mut domains: ResMut<SmokeDomains>,
) {
    let dt = time.delta_secs();
    if !settings.enabled || dt <= 0.0 {
        return;
    }
    for (e, bank) in &banks {
        let Some(d) = domains.list.iter_mut().find(|d| d.owner == Some(e)) else {
            continue;
        };
        // A bank far from everyone rests as it is, with the volume it lies in.
        if d.resting {
            continue;
        }
        let domain = &mut d.domain;
        domain.fog = true;
        let amount = bank.amount_for(weather.fog);
        if amount <= 0.0 && domain.mass_kg <= 0.0 {
            continue;
        }
        let low = domain.floor.iter().copied().min().unwrap_or(0) as f64;
        let low_y = domain.origin.y + low * domain.cell as f64;
        // A new bank starts with its fog already there.
        let mut rates = FogRates::new(dt, bank.form_secs, bank.burn_secs, CHILL_SECS);
        if domain.mass_kg <= 0.0 {
            rates = FogRates {
                form: 1.0,
                burn: 1.0,
                chill: 1.0,
            };
        }
        domain.settle_fog(
            |y| bank.level(amount, low_y, y),
            bank.density(settings.extinction_m2_kg),
            bank.chill_c,
            Vec3::from(bank.color),
            rates,
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::{Domain, StepParams};
    use engine_core::math::{DVec3, UVec3};

    #[test]
    fn fog_fills_the_low_ground_up_to_its_top() {
        let bank = FogBank::fixed(1.0);
        assert_eq!(bank.level(1.0, 10.0, 30.0), 0.0, "above the top");
        assert!(bank.level(1.0, 10.0, 11.0) > 0.9, "near the low ground");
        assert!(
            bank.level(0.25, 10.0, 18.0) < bank.level(1.0, 10.0, 18.0),
            "less is shallower"
        );
        assert_eq!(bank.level(0.0, 10.0, 11.0), 0.0, "none");
        assert_eq!(
            FogBank::default().amount_for(0.4),
            0.4,
            "follows the weather"
        );
        assert_eq!(FogBank::fixed(0.7).amount_for(0.0), 0.7);
    }

    fn rates_of(k: f32) -> FogRates {
        FogRates {
            form: k,
            burn: k,
            chill: k,
        }
    }

    /// A valley: ground at 0 in the middle 8 columns, rising to 16 m at the sides.
    fn valley() -> Domain {
        let mut d = Domain::new(DVec3::new(-32.0, 0.0, -16.0), 2.0, UVec3::new(32, 16, 16));
        d.set_floor(|x, _| Some((x.abs() - 8.0).max(0.0) * 0.7));
        d
    }

    #[test]
    fn fog_forms_in_the_valley_and_stays_out_of_the_air_above() {
        engine_core::tasks::init_task_pools();
        let bank = FogBank::fixed(1.0);
        let mut d = valley();
        let rho = bank.density(3000.0);
        for _ in 0..200 {
            d.settle_fog(
                |y| bank.level(1.0, 0.0, y),
                rho,
                2.0,
                Vec3::ONE,
                rates_of(0.05),
            );
        }
        let low = d.sample(DVec3::new(0.0, 3.0, 0.0));
        let high = d.sample(DVec3::new(0.0, 25.0, 0.0));
        let slope = d.sample(DVec3::new(28.0, 20.0, 0.0));
        assert!(
            low.density > rho * 0.8,
            "valley floor: {} of {rho}",
            low.density
        );
        assert!(low.heat < -1.5, "fog is chilled: {}", low.heat);
        assert_eq!(high.density, 0.0, "clear above the top");
        assert_eq!(slope.density, 0.0, "the hillside above it is clear");
    }

    #[test]
    fn chilled_fog_sinks_and_burns_off() {
        engine_core::tasks::init_task_pools();
        let rho = FogBank::fixed(1.0).density(3000.0);
        let mut d = valley();
        // A ball of chilled fog in the air over the valley...
        for z in 0..d.dims.z {
            for y in 0..d.dims.y {
                for x in 0..d.dims.x {
                    if d.voxel_center(x, y, z).distance(DVec3::new(0.0, 22.0, 0.0)) < 5.0 {
                        let i = d.index(x, y, z);
                        d.voxels[i].density = rho;
                        d.voxels[i].tint = Vec3::splat(rho);
                        d.voxels[i].heat = -2.0;
                    }
                }
            }
        }
        d.mass_kg = d.voxels.iter().map(|v| v.density).sum::<f32>() * 8.0;
        let params = StepParams {
            dt: 0.05,
            ambient_c: 10.0,
            dissipation_secs: 1.0e4,
            ..Default::default()
        };
        let height = |d: &Domain| {
            let (mut m, mut my) = (0.0, 0.0);
            for z in 0..d.dims.z {
                for y in 0..d.dims.y {
                    for x in 0..d.dims.x {
                        let v = d.voxels[d.index(x, y, z)];
                        m += v.density as f64;
                        my += v.density as f64 * d.voxel_center(x, y, z).y;
                    }
                }
            }
            my / m.max(1e-12)
        };
        // ... sinks while it's colder than the air around it (kept chilled, like ground fog).
        let before = height(&d);
        let rates = FogRates::new(0.05, 1.0e6, 1.0e6, 2.0);
        for _ in 0..300 {
            d.settle_fog(|_| 0.0, rho, 2.0, Vec3::ONE, rates);
            for v in &mut d.voxels {
                if v.density > rho * 0.05 {
                    v.heat = v.heat.min(-1.5);
                }
            }
            d.step(&params, &|_| Vec3::ZERO);
        }
        assert!(
            height(&d) < before - 2.0,
            "fog sinks: {before} -> {}",
            height(&d)
        );
        // No more fog: it burns off.
        let mass = d.mass_kg;
        for _ in 0..60 {
            d.settle_fog(|_| 0.0, rho, 2.0, Vec3::ONE, rates_of(0.1));
        }
        assert!(
            d.mass_kg < mass * 0.05,
            "burnt off: {} of {mass} kg",
            d.mass_kg
        );
    }
}
