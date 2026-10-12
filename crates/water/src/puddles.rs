//! Rain fills puddles and they drain afterwards.
//!
//! Writes `SurfaceLayers::puddle_m` (standing water depth) on the shared ground-surface grid,
//! which footsteps, vehicles (grip) and the renderer read. Each cell collects the rain falling
//! on it plus runoff from around it in proportion to how much of a hollow it is (fine
//! micro-relief: ruts, dips, uneven paving), and loses water by soaking into the ground
//! (material absorbency, less once soaked) and by evaporation (faster when warm and windy).
//! Only gentle slopes hold water. Cells under a `WaterBody` are skipped.

use engine_core::math::{DVec2, DVec3};
use engine_core::prelude::*;
use engine_render::Camera;
use engine_world::{FocusPoints, Ground, SurfaceLayers, SurfaceMaterials, WeatherState};

use crate::body::{RainShower, WaterSettings};
use crate::state::SharedWater;

/// How often puddles update, seconds.
const PERIOD: f32 = 0.25;
/// Deepest puddle, meters.
const MAX_DEPTH: f32 = 0.06;

/// 0 (a bump) .. 1 (a hollow) for the ground cell at world x/z. Stable for each spot.
pub fn hollowness(x: f64, z: f64) -> f32 {
    use engine_core::math::noise::fbm2;
    let n = fbm2(x * 0.11, z * 0.11, 0x9D_D1E5, 3, 2.1, 0.5);
    (1.0 - (n * 0.5 + 0.5)).clamp(0.0, 1.0)
}

/// Rain falling at `p`, mm/h, from the weather and any showers.
pub fn rain_rate(weather: &WeatherState, showers: &[(DVec3, RainShower)], p: DVec3) -> f32 {
    let mut rate = if weather.is_raining() {
        weather.precipitation_mm_h
    } else {
        0.0
    };
    for (c, s) in showers {
        let d = DVec2::new(p.x - c.x, p.z - c.z).length() as f32;
        if d < s.radius_m {
            rate += s.mm_h;
        }
    }
    rate
}

#[allow(clippy::too_many_arguments)]
pub fn puddle_system(
    time: Res<Time>,
    settings: Res<WaterSettings>,
    weather: Res<WeatherState>,
    ground_query: Res<Ground>,
    materials: Res<SurfaceMaterials>,
    focus: Res<FocusPoints>,
    shared: Res<SharedWater>,
    showers_q: Query<(&GlobalTransform, &RainShower)>,
    cameras: Query<(&GlobalTransform, &Camera)>,
    mut surface: ResMut<SurfaceLayers>,
    mut accumulated: Local<f32>,
    mut active: Local<bool>,
) {
    if !settings.puddles {
        return;
    }
    *accumulated += time.delta_secs();
    if *accumulated < PERIOD {
        return;
    }
    let dt = std::mem::take(&mut *accumulated);
    let showers: Vec<(DVec3, RainShower)> = showers_q
        .iter()
        .map(|(g, s)| (g.translation(), *s))
        .collect();
    let raining = weather.is_raining() || showers.iter().any(|(_, s)| s.mm_h > 0.0);
    if !raining && !*active {
        return;
    }
    let focus = crate::sim::focus_point(&focus, &cameras);
    let radius = settings.puddle_radius_m as f64;
    let state = shared.0.read();
    let temp = weather.temperature_c;
    let wind = weather.wind.length();
    // Evaporation ≈ 0.1–0.5 mm/h in mild weather, more when warm and windy.
    let evaporation = ((0.15 + 0.02 * temp.max(0.0)) * (1.0 + 0.15 * wind)) / 3.6e6;
    let grid = &mut surface.grid;
    let cell = grid.cell as f64;
    let c0 = ((DVec2::new(focus.x, focus.z) - grid.origin) / cell).floor();
    let reach = (radius / cell).ceil() as i64;
    let mut any = false;
    for dj in -reach..=reach {
        for di in -reach..=reach {
            let (i, j) = (c0.x as i64 + di, c0.y as i64 + dj);
            if i < 0 || j < 0 || i >= grid.width as i64 || j >= grid.height as i64 {
                continue;
            }
            if (di * di + dj * dj) as f64 * cell * cell > radius * radius {
                continue;
            }
            let center = grid.origin + DVec2::new(i as f64 + 0.5, j as f64 + 0.5) * cell;
            let p = DVec3::new(center.x, focus.y + 50.0, center.y);
            let idx = j as usize * grid.width as usize + i as usize;
            let current = grid.data[idx].puddle_m;
            let rate = rain_rate(&weather, &showers, p);
            if rate <= 0.0 && current <= 0.0 {
                continue;
            }
            let Some(ground) = ground_query.sample(p) else {
                continue;
            };
            let p = DVec3::new(p.x, ground.height, p.z);
            // Open water isn't a puddle.
            if state
                .bodies
                .iter()
                .any(|b| b.depth_at(p, &state.ground).is_some_and(|d| d > 0.0))
            {
                continue;
            }
            let flatness = ((ground.normal.y - 0.94) / 0.05).clamp(0.0, 1.0);
            let hollow = hollowness(p.x, p.z);
            let material = materials.get(ground.surface);
            let wet = grid.data[idx].wetness;
            // Rain plus runoff from the surrounding bumps into the hollows.
            let rain = rate / 3.6e6 * (0.3 + 3.5 * hollow * hollow) * flatness;
            let soak =
                material.absorbency * 2.5e-6 * (1.0 - 0.85 * wet.max(current / MAX_DEPTH).min(1.0));
            let cap = MAX_DEPTH * hollow * hollow * flatness;
            let next = (current + (rain - soak - evaporation) * dt).clamp(0.0, cap.max(0.0));
            grid.data[idx].puddle_m = next;
            any |= next > 0.0;
        }
    }
    *active = any || raining;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hollowness_is_stable_and_spread() {
        let a = hollowness(12.3, -4.5);
        assert_eq!(a, hollowness(12.3, -4.5));
        let samples: Vec<f32> = (0..400)
            .map(|i| hollowness(i as f64 * 1.7, i as f64 * 0.9))
            .collect();
        let lo = samples.iter().filter(|h| **h < 0.4).count();
        let hi = samples.iter().filter(|h| **h > 0.6).count();
        assert!(lo > 20 && hi > 20, "{lo} {hi}");
    }
}
