//! Rain fills puddles and they drain afterwards.
//!
//! Writes `SurfaceLayers::puddle_m` (standing water depth) on the shared ground-surface grid,
//! which footsteps, vehicles (grip) and the renderer read. Each cell collects the rain falling
//! on it plus runoff from around it in proportion to how much of a hollow it is (fine
//! micro-relief: ruts, dips, uneven paving), and loses water by soaking into the ground
//! (material absorbency, less once soaked) and by evaporation (faster when warm and windy).
//! Only gentle slopes hold water. Cells under a `WaterBody` are skipped.
//!
//! The ground around the focus is gone over one pass a second, a few rows of it each step
//! ([`PASS_S`]): every cell asks the ground where it is, and all of them in one step held a
//! frame for as long as 13,000 ground samples take, four times a second while it rained.

use engine_core::math::{DVec2, DVec3};
use engine_core::prelude::*;
use engine_render::Camera;
use engine_world::{FocusPoints, Ground, SurfaceLayers, SurfaceMaterials, WeatherState};

use crate::body::{RainShower, WaterSettings};
use crate::state::SharedWater;

/// How long one pass over the ground around the focus takes, seconds: each cell's puddle is
/// worked out again this often, for the time since the pass before reached it. (Puddles fill
/// and drain over minutes: at most 0.1 mm a second in a cloudburst.)
pub const PASS_S: f32 = 1.0;
/// With no rain and no puddle standing, how often the system looks for rain, seconds.
const LOOK_S: f32 = 0.25;
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

/// Evaporation, m/s: about 0.1–0.5 mm/h in mild weather, more when warm and windy.
fn evaporation(weather: &WeatherState) -> f32 {
    let (temp, wind) = (weather.temperature_c, weather.wind.length());
    ((0.15 + 0.02 * temp.max(0.0)) * (1.0 + 0.15 * wind)) / 3.6e6
}

/// What a cell's ground and the weather over it are to its puddle.
#[derive(Clone, Copy)]
struct Spot {
    /// Rain, mm/h.
    rate: f32,
    /// 0 (too steep to hold water) .. 1 (level).
    flatness: f32,
    /// [`hollowness`].
    hollow: f32,
    /// The ground material's absorbency.
    absorbency: f32,
    /// The cell's wetness, 0..1.
    wet: f32,
    /// [`evaporation`].
    evaporation: f32,
}

/// A puddle `current` m deep, `dt` s on.
fn deeper(current: f32, at: Spot, dt: f32) -> f32 {
    // Rain plus runoff from the surrounding bumps into the hollows.
    let rain = at.rate / 3.6e6 * (0.3 + 3.5 * at.hollow * at.hollow) * at.flatness;
    let soak = at.absorbency * 2.5e-6 * (1.0 - 0.85 * at.wet.max(current / MAX_DEPTH).min(1.0));
    let cap = MAX_DEPTH * at.hollow * at.hollow * at.flatness;
    (current + (rain - soak - at.evaporation) * dt).clamp(0.0, cap.max(0.0))
}

/// [`puddle_system`]'s pass over the ground around the focus, and what it keeps between two.
#[derive(Default)]
pub struct PuddlePass {
    /// Time since the pass under way began or, with none under way, since the last look for
    /// rain, seconds.
    since: f32,
    /// The pass under way.
    pass: Option<Pass>,
    /// It rained or a puddle stood somewhere when the last pass ended.
    active: bool,
}

struct Pass {
    /// The time the pass accounts for (since the pass before it began), seconds.
    dt: f32,
    /// Where it is centered (the focus when it began), world X/Z.
    center: DVec2,
    /// Rows of cells either side of the center's row, and the next row to do
    /// (`-reach..=reach`).
    reach: i64,
    next: i64,
    /// A puddle stands in a row done so far.
    any: bool,
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
    mut book: Local<PuddlePass>,
) {
    if !settings.puddles {
        return;
    }
    let step = time.delta_secs();
    let PuddlePass {
        since,
        pass,
        active,
    } = &mut *book;
    *since += step;
    if pass.is_none() && *since < LOOK_S {
        return;
    }
    let showers: Vec<(DVec3, RainShower)> = showers_q
        .iter()
        .map(|(g, s)| (g.translation(), *s))
        .collect();
    let raining = weather.is_raining() || showers.iter().any(|(_, s)| s.mm_h > 0.0);
    let focus = crate::sim::focus_point(&focus, &cameras);
    if pass.is_none() {
        let dt = std::mem::take(since);
        if !raining && !*active {
            return;
        }
        let reach = (settings.puddle_radius_m as f64 / surface.grid.cell as f64).ceil() as i64;
        *pass = Some(Pass {
            dt,
            center: DVec2::new(focus.x, focus.z),
            reach,
            next: -reach,
            any: false,
        });
    }
    let Some(run) = pass.as_mut() else {
        return;
    };
    // The rows due by the end of this step: all of them over PASS_S.
    let rows = 2 * run.reach + 1;
    let due = (((*since + step) / PASS_S).min(1.0) * rows as f32).ceil() as i64;
    let end = (due - run.reach).min(run.reach + 1);
    if end > run.next {
        let radius = settings.puddle_radius_m as f64;
        let state = shared.0.read();
        let evaporation = evaporation(&weather);
        let grid = &mut surface.grid;
        let cell = grid.cell as f64;
        // The center's cell on the grid as it lies now (the grid follows the focus too).
        let c0 = ((run.center - grid.origin) / cell).floor();
        for dj in run.next..end {
            for di in -run.reach..=run.reach {
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
                let at = Spot {
                    rate,
                    flatness: ((ground.normal.y - 0.94) / 0.05).clamp(0.0, 1.0),
                    hollow: hollowness(p.x, p.z),
                    absorbency: materials.get(ground.surface).absorbency,
                    wet: grid.data[idx].wetness,
                    evaporation,
                };
                let next = deeper(current, at, run.dt);
                grid.data[idx].puddle_m = next;
                run.any |= next > 0.0;
            }
        }
        run.next = end;
    }
    if run.next > run.reach {
        *active = run.any || raining;
        *pass = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use engine_core::time::TimeSource;
    use engine_world::{GroundQuery, GroundSample, SurfaceId, WorldPlugin};
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};

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

    /// Level ground that counts how often it is asked.
    struct Counted(Arc<AtomicUsize>);
    impl GroundQuery for Counted {
        fn sample(&self, _p: DVec3) -> Option<GroundSample> {
            self.0.fetch_add(1, Ordering::Relaxed);
            Some(GroundSample {
                height: 0.0,
                normal: Vec3::Y,
                surface: SurfaceId(3),
            })
        }
    }

    /// A shower over level ground, stepped `step_secs` at a time, and the ground's count.
    fn rainy(step_secs: f64, mm_h: f32) -> (App, Arc<AtomicUsize>) {
        let mut app = App::new();
        app.world.resource_mut::<EngineConfig>().headless = true;
        app.world
            .resource_mut::<Time>()
            .set_source(TimeSource::Manual { step_secs });
        app.add_plugins((WorldPlugin, crate::WaterPlugin));
        let asked = Arc::new(AtomicUsize::new(0));
        app.world
            .insert_resource(Ground(Arc::new(Counted(asked.clone()))));
        app.world.spawn((
            Transform::from_translation(DVec3::ZERO),
            RainShower {
                radius_m: 500.0,
                mm_h,
            },
        ));
        (app, asked)
    }

    /// The cells the pass goes over around the origin.
    fn disc(app: &App) -> Vec<(usize, DVec2)> {
        let radius = app.world.resource::<WaterSettings>().puddle_radius_m as f64;
        let grid = &app.world.resource::<SurfaceLayers>().grid;
        let cell = grid.cell as f64;
        let c0 = ((DVec2::ZERO - grid.origin) / cell).floor();
        let reach = (radius / cell).ceil() as i64;
        let mut out = Vec::new();
        for dj in -reach..=reach {
            for di in -reach..=reach {
                if (di * di + dj * dj) as f64 * cell * cell > radius * radius {
                    continue;
                }
                let (i, j) = ((c0.x as i64 + di) as usize, (c0.y as i64 + dj) as usize);
                let center = grid.origin + DVec2::new(i as f64 + 0.5, j as f64 + 0.5) * cell;
                out.push((j * grid.width as usize + i, center));
            }
        }
        out
    }

    /// While it rains the ground around the focus was all asked in one step, four times a
    /// second (65-84 ms each on the session VM's lap). Now a pass takes a second of steps, a
    /// few rows each, every cell once a pass, and the puddles come out as deep.
    #[test]
    fn a_pass_goes_over_the_ground_a_few_rows_a_step() {
        let (mut app, asked) = rainy(1.0 / 60.0, 50.0);
        let cells = disc(&app);
        let width = {
            let radius = app.world.resource::<WaterSettings>().puddle_radius_m;
            let cell = app.world.resource::<SurfaceLayers>().grid.cell;
            2 * (radius / cell).ceil() as usize + 1
        };
        assert!(cells.len() > 10_000, "{} cells", cells.len());
        // Ten seconds of rain.
        let mut most = 0;
        let mut total = 0;
        for _ in 0..600 {
            let before = asked.load(Ordering::Relaxed);
            app.update();
            let n = asked.load(Ordering::Relaxed) - before;
            most = most.max(n);
            total += n;
        }
        // A step does the rows of its share of the second (and a step or two may run in an
        // update); the rest of the water asks the ground a few times a step at most.
        let a_step = width * (width as f32 / (PASS_S * 60.0)).ceil() as usize;
        assert!(
            most <= 2 * a_step + 64,
            "{most} ground samples in an update, {a_step} a step expected"
        );
        // Nine passes done in ten seconds (the first begins a quarter of a second in), a
        // tenth under way: every cell asked once a pass.
        let passes = total as f32 / cells.len() as f32;
        assert!((9.0..10.2).contains(&passes), "{passes} passes");
        // Each puddle is as deep as the rain since made it: the same sum, a step at a time,
        // over the time the passes done so far account for.
        let grid = &app.world.resource::<SurfaceLayers>().grid;
        let weather = app.world.resource::<WeatherState>();
        let materials = app.world.resource::<SurfaceMaterials>();
        let (mut wet_cells, mut deepest) = (0, 0.0f32);
        for &(idx, center) in &cells {
            let got = grid.data[idx].puddle_m;
            let at = Spot {
                rate: 50.0,
                flatness: 1.0,
                hollow: hollowness(center.x, center.y),
                absorbency: materials.get(SurfaceId(3)).absorbency,
                wet: grid.data[idx].wetness,
                evaporation: evaporation(weather),
            };
            let after =
                |secs: f32| (0..(secs * 60.0) as usize).fold(0.0, |d, _| deeper(d, at, 1.0 / 60.0));
            // Between the ninth pass's account and the tenth's.
            let (least, most) = (after(8.2), after(10.3));
            assert!(
                got >= least * 0.98 - 1e-7 && got <= most * 1.02 + 1e-7,
                "a puddle {got} m deep at {center:?}, {least}..{most} expected"
            );
            wet_cells += (got > 0.0) as usize;
            deepest = deepest.max(got);
        }
        assert!(wet_cells > cells.len() / 4, "{wet_cells} cells hold water");
        assert!(deepest > 1e-4, "the deepest puddle is {deepest} m");
    }

    /// Dry ground with no rain is left alone.
    #[test]
    fn dry_ground_with_no_rain_is_not_gone_over() {
        let (mut app, asked) = rainy(1.0 / 60.0, 0.0);
        app.update_n(120);
        let n = asked.load(Ordering::Relaxed);
        assert!(n < 1000, "{n} ground samples with no rain and no puddles");
    }
}
