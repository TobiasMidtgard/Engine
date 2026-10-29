//! Where a lake isn't held in: the stretches of its edge where the ground is lower than its
//! surface, so the water would end in mid-air at its extent instead of against a bank.
//!
//! A lake is a flat surface cut at its [`WaterExtent`]; it stays in only if the ground rises
//! above that surface all round the cut. [`rim_leaks`] walks the edge and returns the
//! stretches that don't, with how far the water stands above the ground there (the head that
//! would drive it out over the brim).

use engine_core::math::{DVec2, DVec3, Vec2};
use engine_core::prelude::*;

use crate::body::WaterExtent;

/// The edge is sampled this far apart, meters.
pub const RIM_STEP_M: f32 = 1.0;
/// Ground this little under the surface still holds it (a wet sandy lip), meters.
pub const LEAK_TOLERANCE_M: f64 = 0.03;

/// A stretch of a lake's edge where the water is higher than the ground under it.
#[derive(Debug, Clone, PartialEq)]
pub struct RimLeak {
    /// The lowest point of the stretch, world (y the ground there).
    pub lowest: DVec3,
    /// Outward direction there, world x/z (unit).
    pub out: Vec2,
    /// How far the surface stands above the ground at its lowest point, meters.
    pub head: f32,
    /// Length of the stretch along the edge, meters.
    pub length: f32,
    /// The edge points it covers, world (y the ground).
    pub points: Vec<DVec3>,
}

/// The edge of `extent` around `center` (turned by `yaw`, as [`crate::state::BodyState`] is),
/// as world x/z points about `step` apart with their outward directions. Endless water has none.
pub fn rim(center: DVec3, yaw: f32, extent: &WaterExtent, step: f32) -> Vec<(DVec2, Vec2)> {
    let (s, c) = yaw.sin_cos();
    // The inverse of `BodyState::local`.
    let world = |l: Vec2| Vec2::new(c * l.x + s * l.y, -s * l.x + c * l.y);
    let mut out = Vec::new();
    match *extent {
        WaterExtent::Infinite => {}
        WaterExtent::Circle { radius } => {
            let n = ((std::f32::consts::TAU * radius / step).ceil() as usize).max(8);
            for k in 0..n {
                let a = k as f32 / n as f32 * std::f32::consts::TAU;
                let dir = Vec2::new(a.cos(), a.sin());
                out.push((dir * radius, dir));
            }
        }
        WaterExtent::Rect {
            half_size: [hx, hz],
        } => {
            for (from, to, normal) in [
                (Vec2::new(-hx, -hz), Vec2::new(hx, -hz), Vec2::NEG_Y),
                (Vec2::new(hx, -hz), Vec2::new(hx, hz), Vec2::X),
                (Vec2::new(hx, hz), Vec2::new(-hx, hz), Vec2::Y),
                (Vec2::new(-hx, hz), Vec2::new(-hx, -hz), Vec2::NEG_X),
            ] {
                let n = (((to - from).length() / step).ceil() as usize).max(1);
                for k in 0..n {
                    out.push((from + (to - from) * (k as f32 / n as f32), normal));
                }
            }
        }
    }
    out.into_iter()
        .map(|(l, n)| {
            let w = world(l);
            (
                DVec2::new(center.x + w.x as f64, center.z + w.y as f64),
                world(n).normalize_or_zero(),
            )
        })
        .collect()
}

/// The stretches of the edge of a lake at `center` (y its surface) where `height` (ground at
/// world x/z, `None` where there is none) is under the surface. Longest first.
pub fn rim_leaks(
    center: DVec3,
    yaw: f32,
    extent: &WaterExtent,
    height: &dyn Fn(f64, f64) -> Option<f64>,
) -> Vec<RimLeak> {
    let points = rim(center, yaw, extent, RIM_STEP_M);
    let n = points.len();
    if n == 0 {
        return Vec::new();
    }
    let ground: Vec<Option<f64>> = points.iter().map(|(p, _)| height(p.x, p.y)).collect();
    let leaking = |k: usize| ground[k].is_none_or(|g| g < center.y - LEAK_TOLERANCE_M);
    // Start the walk on a held point, so a stretch never wraps around the start.
    let Some(start) = (0..n).find(|&k| !leaking(k)) else {
        return vec![leak(center, &points, &ground, (0..n).collect())];
    };
    let mut leaks = Vec::new();
    let mut run = Vec::new();
    for o in 1..=n {
        let k = (start + o) % n;
        if leaking(k) {
            run.push(k);
        } else if !run.is_empty() {
            leaks.push(leak(center, &points, &ground, std::mem::take(&mut run)));
        }
    }
    leaks.sort_by(|a, b| b.length.total_cmp(&a.length));
    leaks
}

fn leak(
    center: DVec3,
    points: &[(DVec2, Vec2)],
    ground: &[Option<f64>],
    run: Vec<usize>,
) -> RimLeak {
    // No ground at all (off the terrain): count it a long way down.
    let g = |k: usize| ground[k].unwrap_or(center.y - 1e3);
    let low = run
        .iter()
        .copied()
        .min_by(|&a, &b| g(a).total_cmp(&g(b)))
        .unwrap_or(run[0]);
    RimLeak {
        lowest: DVec3::new(points[low].0.x, g(low), points[low].0.y),
        out: points[low].1,
        head: (center.y - g(low)) as f32,
        length: run.len() as f32 * RIM_STEP_M,
        points: run
            .iter()
            .map(|&k| DVec3::new(points[k].0.x, g(k), points[k].0.y))
            .collect(),
    }
}

/// Fixed steps after which [`spill_audit_system`] looks (5 s and 30 s at 60 Hz).
const AUDIT_AFTER_STEPS: [u32; 2] = [300, 1800];

/// With `ENGINE_WATER_SPILL_AUDIT` set: log, once, each lake's edge where it isn't held in, and
/// where water reaches each stream region's edges (running out and vanishing there, or standing
/// against an unseen wall).
pub fn spill_audit_system(shared: Res<crate::state::SharedWater>, mut steps: Local<u32>) {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    if !*ON.get_or_init(|| std::env::var_os("ENGINE_WATER_SPILL_AUDIT").is_some()) {
        return;
    }
    // Once the world's ground is in (the first steps answer with a flat default) and the
    // streams have run a while.
    *steps += 1;
    if !AUDIT_AFTER_STEPS.contains(&*steps) {
        return;
    }
    let state = shared.0.read();
    for b in &state.bodies {
        let probe = b.center.y + 50.0;
        let height = |x: f64, z: f64| state.ground.height(DVec3::new(x, probe, z));
        if height(b.center.x, b.center.z).is_none() {
            continue;
        }
        let leaks = rim_leaks(b.center, b.yaw, &b.body.extent, &height);
        eprintln!(
            "SPILL step {} body {:?} at {:.1?} extent {:?}: {} leaking stretch(es)",
            *steps,
            b.entity,
            b.center,
            b.body.extent,
            leaks.len()
        );
        for l in leaks.iter().take(8) {
            eprintln!(
                "SPILL   {:.0} m of edge, lowest ground {:.2} m under the surface at {:.1?}, out {:.2?}",
                l.length, l.head, l.lowest, l.out
            );
        }
    }
    for f in &state.flows {
        if f.resting {
            continue;
        }
        let (nx, nz) = (f.nx, f.nz);
        let mut wet = Vec::new();
        for j in 0..nz {
            for i in 0..nx {
                if i != 0 && j != 0 && i + 1 != nx && j + 1 != nz {
                    continue;
                }
                let k = j * nx + i;
                if f.depth[k] > 0.02 {
                    wet.push((f.cell_center(i, j), f.depth[k]));
                }
            }
        }
        eprintln!(
            "SPILL flow {:?} at {:.1?} {}x{} cells of {} m ({}): {} wet edge cells, drained {:.1} m3",
            f.entity,
            f.origin,
            nx,
            nz,
            f.cell,
            if f.open_edges { "open edges" } else { "walled" },
            wet.len(),
            f.drained_m3
        );
        for (p, d) in wet.iter().step_by((wet.len() / 8).max(1)) {
            eprintln!("SPILL   edge cell {p:.1?} depth {d:.2}");
        }
        // Cross-sections: the water against the world's ground beside it.
        for j in [nz / 5, nz * 2 / 5, nz * 3 / 5, nz * 4 / 5] {
            let row = j * nx;
            let wet: Vec<usize> = (0..nx).filter(|&i| f.depth[row + i] > 0.03).collect();
            let (Some(&a), Some(&b)) = (wet.first(), wet.last()) else {
                continue;
            };
            let mut line = format!("SPILL   row {j}:");
            for i in a.saturating_sub(6)..(b + 7).min(nx) {
                let c = f.cell_center(i, j);
                let g = state
                    .ground
                    .height(DVec3::new(c.x, c.y + 200.0, c.z))
                    .unwrap_or(f64::NAN);
                let top = f.origin.y + (f.bed[row + i] + f.depth[row + i]) as f64;
                line += &format!(
                    " [g {:.2} w {:.2}]",
                    g,
                    if f.depth[row + i] > 0.03 {
                        top
                    } else {
                        f64::NAN
                    }
                );
            }
            eprintln!("{line}");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_lake_in_a_bowl_is_held_all_round() {
        let bowl = |x: f64, z: f64| Some((x * x + z * z).sqrt() * 0.2 - 3.0);
        let extent = WaterExtent::Circle { radius: 20.0 };
        assert!(rim_leaks(DVec3::ZERO, 0.0, &extent, &bowl).is_empty());
    }

    #[test]
    fn a_low_notch_in_the_bank_is_found_with_its_head() {
        // A bowl whose bank dips to 0.4 m under the surface over 5 m toward +x.
        let notched = |x: f64, z: f64| {
            let bank = (x * x + z * z).sqrt() * 0.2 - 3.0;
            if x > 0.0 && z.abs() < 2.5 {
                Some(bank.min(-0.4))
            } else {
                Some(bank)
            }
        };
        let extent = WaterExtent::Circle { radius: 20.0 };
        let leaks = rim_leaks(DVec3::ZERO, 0.0, &extent, &notched);
        assert_eq!(leaks.len(), 1, "{leaks:?}");
        let l = &leaks[0];
        assert!((l.head - 0.4).abs() < 1e-3, "{}", l.head);
        assert!(l.out.x > 0.95, "{:?}", l.out);
        assert!((3.0..=7.0).contains(&l.length), "{}", l.length);
    }

    #[test]
    fn a_turned_rect_reports_the_side_that_is_low() {
        // Ground falls away toward world +z only.
        let slope = |_x: f64, z: f64| Some(if z > 9.0 { -1.0 } else { 1.0 });
        let extent = WaterExtent::Rect {
            half_size: [10.0, 10.0],
        };
        // A quarter turn: the extent's local x is world... whichever; the low side is +z.
        let leaks = rim_leaks(DVec3::ZERO, std::f32::consts::FRAC_PI_2, &extent, &slope);
        assert_eq!(leaks.len(), 1, "{leaks:?}");
        assert!(leaks[0].out.y > 0.95, "{:?}", leaks[0].out);
        assert!(leaks[0].points.iter().all(|p| p.z > 9.0));
    }
}
