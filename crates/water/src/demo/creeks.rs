//! A meadow valley with a creek network, standing in for the terrain's own until it carves
//! one (Water H1, creeks-plan.md): a meandering main stream down the valley floor, side
//! creeks coming down the slopes into it, and two headwater brooks feeding each.
//!
//! It carves its own ground ([`Meadow::height`], [`Meadow::mesh`]) and answers the terrain's
//! creek query ([`CreekQuery`]) for it, so the creek renderer (`crate::creeks`) and its tests
//! have a network to draw before the terrain has one. Strahler orders, widths, depths and
//! discharges follow downstream hydraulic geometry (width ∝ Q^0.5, depth ∝ Q^0.4; Leopold and
//! Maddock 1953): about 1.1 m brooks of 120 l/s, 2.1 m side creeks of 0.4 m³/s, and a main
//! stream from 5 to 7 m wide carrying 1.5 to 3.5 m³/s.

use std::collections::HashMap;
use std::sync::Arc;

use engine_core::math::noise;
use engine_core::math::{DVec2, DVec3, Vec2, Vec3};
use engine_render::{Mesh, Vertex};
use engine_world::terrain::{CreekPoint, CreekQuery, CreekReach, CreekSample};

use crate::creeks::StepCourse;

/// Half the side of the valley, meters from its center.
pub const MEADOW_HALF: f32 = 400.0;
/// The water's edge as a share of a creek's half-width (as the ribbons draw it).
const EDGE: f32 = 1.15;
/// The banks rise this steeply from the water's edge (rise over run).
const BANK: f32 = 0.8;
/// Size of the cells the creeks' segments are filed under, meters.
const CELL: f32 = 16.0;
/// Points along each creek are this far apart, meters.
const POINT_M: f32 = 3.0;

/// The valley and its creeks, in its own space (the center at the origin, on the valley floor).
pub struct Meadow {
    pub reaches: Vec<CreekReach>,
    /// Each reach's box (x, z).
    bounds: Vec<(Vec2, Vec2)>,
    /// Segments by cell: (reach, first point).
    cells: HashMap<(i32, i32), Vec<(u32, u32)>>,
    /// Each reach's surface with its steps and pools, and meters from its source at each point.
    courses: Vec<(StepCourse, Vec<f32>)>,
}

/// The ground before the creeks cut it: falling gently down the valley (+x) and rising to
/// rounded hills either side.
pub fn meadow_base(x: f32, z: f32) -> f32 {
    let side = (z.abs() / MEADOW_HALF).min(1.2);
    let rolling = noise::fbm2(x as f64 * 0.006, z as f64 * 0.006, 0x5EED_C2EE, 4, 2.0, 0.5);
    let fine = noise::fbm2(x as f64 * 0.05, z as f64 * 0.05, 0x5EED_C2EF, 2, 2.0, 0.5);
    -0.022 * x + 42.0 * side.powf(1.5) + 6.0 * rolling * (0.3 + side) + 0.35 * fine
}

/// The main stream's line across the valley floor, z at `x`.
fn main_z(x: f32) -> f32 {
    22.0 * (x / 85.0).sin() + 9.0 * (x / 31.0 + 1.0).sin()
}

impl Meadow {
    /// The valley's network.
    pub fn new() -> Self {
        let mut reaches: Vec<CreekReach> = Vec::new();
        let mut id = 1u64;
        let mut push = |reaches: &mut Vec<CreekReach>, order: u8, points: Vec<CreekPoint>| {
            reaches.push(CreekReach { id, order, points });
            id += 1;
        };
        // The main stream, whole first, then cut at its junctions.
        let main_line: Vec<Vec2> = (0..=((2.0 * MEADOW_HALF) / POINT_M) as usize)
            .map(|k| {
                let x = -MEADOW_HALF + k as f32 * POINT_M;
                Vec2::new(x, main_z(x))
            })
            .collect();
        let main_y = profile(&main_line, 1.1, None);
        // Side creeks: six from each side, joining at these x.
        let mut joins: Vec<(f32, f32)> = Vec::new();
        for k in 0..12 {
            let side = if k % 2 == 0 { 1.0 } else { -1.0 };
            let x = -330.0 + 55.0 * k as f32 + 9.0 * ((k * 7) as f32).sin();
            joins.push((x, side));
        }
        let main_at = |x: f32| {
            let k = (((x + MEADOW_HALF) / POINT_M).round() as usize).min(main_line.len() - 1);
            (main_line[k], main_y[k], k)
        };
        // The main stream's flow grows with each creek it takes in.
        let mut cuts: Vec<usize> = joins.iter().map(|(x, _)| main_at(*x).2).collect();
        cuts.sort();
        let total = main_line.len();
        let mut start = 0;
        for (n, &cut) in cuts.iter().chain(std::iter::once(&(total - 1))).enumerate() {
            if cut <= start {
                continue;
            }
            let flow = 1.5 + 2.0 * n as f32 / 12.0;
            let points = (start..=cut)
                .map(|k| creek_point(main_line[k], main_y[k], flow, 3))
                .collect();
            push(&mut reaches, 3, points);
            start = cut;
        }
        for (i, &(x, side)) in joins.iter().enumerate() {
            let (foot, foot_y, _) = main_at(x);
            // Up the slope to the fork, wandering, then two brooks to their sources.
            let seed = i as f32 * 1.7;
            let fork_out = 150.0 + 40.0 * (seed * 2.3).sin();
            let line: Vec<Vec2> = (0..=(fork_out / POINT_M) as usize)
                .rev()
                .map(|k| {
                    let t = k as f32 * POINT_M;
                    Vec2::new(
                        foot.x - 0.3 * t + 8.0 * (t / 23.0 + seed).sin(),
                        foot.y + side * t,
                    )
                })
                .collect();
            let fork = line[0];
            let mut brooks = Vec::new();
            for (b, lean) in [(0usize, -1.0f32), (1, 1.0)] {
                let length = 130.0 + 30.0 * (seed + b as f32).cos();
                let brook: Vec<Vec2> = (0..=(length / POINT_M) as usize)
                    .rev()
                    .map(|k| {
                        let t = k as f32 * POINT_M;
                        let wiggle = 5.0 * (t / 17.0 + seed * 3.0 + b as f32).sin();
                        Vec2::new(fork.x + lean * 0.55 * t + wiggle, fork.y + side * 0.8 * t)
                    })
                    .collect();
                brooks.push(brook);
            }
            // Brooks first (their lowest point sets the creek's top), then the creek down to
            // the main stream's surface.
            let mut fork_y = f32::MAX;
            let mut brook_profiles = Vec::new();
            for brook in &brooks {
                let y = profile(brook, 0.4, None);
                fork_y = fork_y.min(*y.last().unwrap_or(&0.0));
                brook_profiles.push(y);
            }
            let creek_y = profile(&line, 0.6, Some((fork_y, foot_y)));
            for (brook, mut y) in brooks.into_iter().zip(brook_profiles) {
                // Meeting the creek at its surface.
                let n = y.len();
                let drop = y[n - 1] - creek_y[0];
                for (k, v) in y.iter_mut().enumerate() {
                    *v -= drop * k as f32 / (n - 1) as f32;
                }
                let points = brook
                    .iter()
                    .zip(&y)
                    .map(|(p, y)| creek_point(*p, *y, 0.12, 1))
                    .collect();
                push(&mut reaches, 1, points);
            }
            let points = line
                .iter()
                .zip(&creek_y)
                .map(|(p, y)| creek_point(*p, *y, 0.4, 2))
                .collect();
            push(&mut reaches, 2, points);
        }
        let courses = reaches
            .iter()
            .map(|r| {
                let mut along = vec![0.0f32];
                for w in r.points.windows(2) {
                    let d = DVec2::new(w[1].at.x - w[0].at.x, w[1].at.z - w[0].at.z).length();
                    along.push(along.last().copied().unwrap_or(0.0) + d as f32);
                }
                (StepCourse::new(r.points.clone()), along)
            })
            .collect();
        let mut meadow = Self {
            bounds: Vec::new(),
            cells: HashMap::new(),
            courses,
            reaches,
        };
        for (r, reach) in meadow.reaches.iter().enumerate() {
            let mut lo = Vec2::splat(f32::MAX);
            let mut hi = Vec2::splat(f32::MIN);
            for p in &reach.points {
                let q = Vec2::new(p.at.x as f32, p.at.z as f32);
                lo = lo.min(q);
                hi = hi.max(q);
            }
            meadow.bounds.push((lo, hi));
            for k in 0..reach.points.len().saturating_sub(1) {
                let (a, b) = (reach.points[k].at, reach.points[k + 1].at);
                let reach_m = carve_reach(&reach.points[k]);
                let lo = Vec2::new(a.x.min(b.x) as f32, a.z.min(b.z) as f32) - reach_m;
                let hi = Vec2::new(a.x.max(b.x) as f32, a.z.max(b.z) as f32) + reach_m;
                for i in (lo.x / CELL).floor() as i32..=(hi.x / CELL).floor() as i32 {
                    for j in (lo.y / CELL).floor() as i32..=(hi.y / CELL).floor() as i32 {
                        meadow
                            .cells
                            .entry((i, j))
                            .or_default()
                            .push((r as u32, k as u32));
                    }
                }
            }
        }
        meadow
    }

    /// The segments filed at `x`, `z`, and each one's nearest point: (reach, segment, share
    /// along it, distance; negative to the right of the creek looking downstream).
    fn near(&self, x: f32, z: f32) -> impl Iterator<Item = (usize, usize, f32, f32)> + '_ {
        let key = ((x / CELL).floor() as i32, (z / CELL).floor() as i32);
        let p = Vec2::new(x, z);
        self.cells
            .get(&key)
            .into_iter()
            .flatten()
            .map(move |&(r, k)| {
                let pts = &self.reaches[r as usize].points;
                let a = Vec2::new(pts[k as usize].at.x as f32, pts[k as usize].at.z as f32);
                let b = Vec2::new(
                    pts[k as usize + 1].at.x as f32,
                    pts[k as usize + 1].at.z as f32,
                );
                let ab = b - a;
                let t = ((p - a).dot(ab) / ab.length_squared().max(1e-6)).clamp(0.0, 1.0);
                let side = if ab.perp_dot(p - a) < 0.0 { -1.0 } else { 1.0 };
                (r as usize, k as usize, t, side * (a + ab * t).distance(p))
            })
    }

    /// Ground height at `x`, `z`, the creeks' channels carved in.
    pub fn height(&self, x: f32, z: f32) -> f32 {
        let base = meadow_base(x, z);
        let mut cut = f32::MAX;
        let mut width = 1.0f32;
        let wobble = noise::fbm2(x as f64 * 0.08, z as f64 * 0.08, 0xBA4C, 2, 2.0, 0.5) * 1.6;
        for (r, k, t, signed) in self.near(x, z) {
            let mut p = lerp_point(&self.reaches[r].points, k, t);
            let d = signed.abs();
            // The water's surface as the creek's ribbon draws it, steps and pools and all, so
            // its bed steps with it.
            let (course, along) = &self.courses[r];
            let s = along[k] + t * (along[k + 1] - along[k]);
            let x = (signed / (0.5 * p.width_m * EDGE)).clamp(-1.0, 1.0);
            let (y, depth) = course.surface(s, x);
            p.at.y = y;
            p.depth_m = depth;
            let c = channel(&p, d, wobble.clamp(-1.0, 1.0));
            if c < cut {
                cut = c;
                width = p.width_m;
            }
        }
        if cut == f32::MAX {
            return base;
        }
        // The bank's top rounded off into the meadow, over about a creek's width.
        smooth_min(base, cut, 0.4 + 0.35 * width)
    }

    /// What the ground is at `x`, `z`: 0 grass .. 1 damp bank; 2 the creek's bed.
    fn wetness(&self, x: f32, z: f32) -> f32 {
        let mut wet = 0.0f32;
        for (r, k, t, d) in self.near(x, z) {
            let d = d.abs();
            let p = lerp_point(&self.reaches[r].points, k, t);
            let edge = 0.5 * p.width_m * EDGE;
            let w = if d < edge {
                2.0
            } else {
                1.0 - ((d - edge) / (1.0 + 0.4 * p.width_m)).clamp(0.0, 1.0)
            };
            wet = wet.max(w);
        }
        wet
    }

    /// The valley's ground mesh, `spacing` meters apart, coloured grass, damp banks and stony
    /// beds.
    pub fn mesh(&self, spacing: f32) -> Mesh {
        let n = (2.0 * MEADOW_HALF / spacing).round() as u32 + 1;
        let pos =
            |i: u32, j: u32| Vec2::splat(-MEADOW_HALF) + Vec2::new(i as f32, j as f32) * spacing;
        let heights: Vec<f32> = (0..n * n)
            .map(|v| {
                let p = pos(v % n, v / n);
                self.height(p.x, p.y)
            })
            .collect();
        let h = |i: i64, j: i64| {
            let (i, j) = (i.clamp(0, n as i64 - 1), j.clamp(0, n as i64 - 1));
            heights[(j * n as i64 + i) as usize]
        };
        let mut vertices = Vec::with_capacity(heights.len());
        for j in 0..n {
            for i in 0..n {
                let (ii, jj) = (i as i64, j as i64);
                let normal = Vec3::new(
                    h(ii - 1, jj) - h(ii + 1, jj),
                    2.0 * spacing,
                    h(ii, jj - 1) - h(ii, jj + 1),
                )
                .normalize();
                let p = pos(i, j);
                let y = heights[(j * n + i) as usize];
                let color = ground_color(p, normal, self.wetness(p.x, p.y));
                vertices.push(
                    Vertex::new(Vec3::new(p.x, y, p.y), normal, p).with_color(color.extend(1.0)),
                );
            }
        }
        let mut indices = Vec::with_capacity(((n - 1) * (n - 1) * 6) as usize);
        for j in 0..n - 1 {
            for i in 0..n - 1 {
                let a = j * n + i;
                let (b, c, d) = (a + 1, a + n, a + n + 1);
                let hh = |v: u32| heights[v as usize];
                if (hh(a) - hh(d)).abs() < (hh(b) - hh(c)).abs() {
                    indices.extend_from_slice(&[a, c, d, a, d, b]);
                } else {
                    indices.extend_from_slice(&[a, c, b, b, c, d]);
                }
            }
        }
        Mesh::new(vertices, indices)
    }
}

impl Default for Meadow {
    fn default() -> Self {
        Self::new()
    }
}

/// How far from a creek's line its channel reaches, meters.
fn carve_reach(p: &CreekPoint) -> f32 {
    0.5 * p.width_m * EDGE + 3.0 / BANK + 2.0
}

/// The channel's ground at `d` meters from a creek's line: a bed sunk `depth` under the water
/// in the middle, meeting the surface at the water's edge, then a bank rising from it,
/// steeper or gentler from place to place (`wobble`, -1..1).
fn channel(p: &CreekPoint, d: f32, wobble: f32) -> f32 {
    let surface = p.at.y as f32;
    let edge = 0.5 * p.width_m * EDGE;
    if d < edge {
        let x = d / edge;
        surface - p.depth_m * (1.0 - x * x) - 0.02
    } else {
        surface - 0.02 + (d - edge) * BANK * (1.0 + 0.45 * wobble)
    }
}

/// `a.min(b)`, rounded over a band `k` wide where the two are close (the bank's top edge).
fn smooth_min(a: f32, b: f32, k: f32) -> f32 {
    let h = (k - (a - b).abs()).max(0.0) / k;
    a.min(b) - h * h * k * 0.25
}

fn lerp_point(points: &[CreekPoint], k: usize, t: f32) -> CreekPoint {
    let (a, b) = (&points[k], &points[k + 1]);
    CreekPoint {
        at: a.at.lerp(b.at, t as f64),
        width_m: a.width_m + (b.width_m - a.width_m) * t,
        depth_m: a.depth_m + (b.depth_m - a.depth_m) * t,
        flow_m3_s: a.flow_m3_s + (b.flow_m3_s - a.flow_m3_s) * t,
    }
}

/// A creek's point with its width and depth from its discharge.
fn creek_point(p: Vec2, y: f32, flow: f32, _order: u8) -> CreekPoint {
    CreekPoint {
        at: DVec3::new(p.x as f64, y as f64, p.y as f64),
        width_m: 3.3 * flow.powf(0.5),
        depth_m: 0.38 * flow.powf(0.4),
        flow_m3_s: flow,
    }
}

/// The water's surface along a line (upstream first): the ground sunk `incision` meters,
/// never climbing downstream. With `ends`, from the first height to the last.
fn profile(line: &[Vec2], incision: f32, ends: Option<(f32, f32)>) -> Vec<f32> {
    let mut out: Vec<f32> = Vec::with_capacity(line.len());
    let mut lowest = f32::MAX;
    for (k, p) in line.iter().enumerate() {
        let y = meadow_base(p.x, p.y) - incision;
        let step = if k > 0 { line[k - 1].distance(*p) } else { 0.0 };
        // At least a gentle fall (1 in 500), so the water always runs on.
        lowest = (lowest - 0.002 * step).min(y);
        out.push(lowest);
    }
    if let Some((top, foot)) = ends {
        // Meet the stream it runs into at its surface, and start no higher than what feeds it.
        let n = out.len();
        let first = out[0];
        let last = out[n - 1];
        for (k, v) in out.iter_mut().enumerate() {
            let t = k as f32 / (n - 1).max(1) as f32;
            *v += (top.min(first) - first) * (1.0 - t) + (foot - last) * t;
        }
        // Keep it falling.
        for k in 1..n {
            out[k] = out[k].min(out[k - 1] - 0.001);
        }
    }
    out
}

impl CreekQuery for Meadow {
    fn nearest(&self, p: DVec3, within_m: f32) -> Option<CreekSample> {
        let mut best: Option<(usize, usize, f32, f32)> = None;
        // Cells round the point out to `within_m`.
        let r = (within_m / CELL).ceil() as i32;
        let (ci, cj) = (
            (p.x as f32 / CELL).floor() as i32,
            (p.z as f32 / CELL).floor() as i32,
        );
        let q = Vec2::new(p.x as f32, p.z as f32);
        for i in ci - r..=ci + r {
            for j in cj - r..=cj + r {
                let Some(segs) = self.cells.get(&(i, j)) else {
                    continue;
                };
                for &(rr, k) in segs {
                    let pts = &self.reaches[rr as usize].points;
                    let a = Vec2::new(pts[k as usize].at.x as f32, pts[k as usize].at.z as f32);
                    let b = Vec2::new(
                        pts[k as usize + 1].at.x as f32,
                        pts[k as usize + 1].at.z as f32,
                    );
                    let ab = b - a;
                    let t = ((q - a).dot(ab) / ab.length_squared().max(1e-6)).clamp(0.0, 1.0);
                    let d = (a + ab * t).distance(q);
                    if d <= within_m && best.is_none_or(|b| d < b.3) {
                        best = Some((rr as usize, k as usize, t, d));
                    }
                }
            }
        }
        let (r, k, t, d) = best?;
        let reach = &self.reaches[r];
        let c = lerp_point(&reach.points, k, t);
        let (a, b) = (reach.points[k].at, reach.points[k + 1].at);
        Some(CreekSample {
            distance_m: d,
            center: c.at,
            downstream: Vec2::new((b.x - a.x) as f32, (b.z - a.z) as f32).normalize_or(Vec2::X),
            width_m: c.width_m,
            depth_m: c.depth_m,
            bed_m: c.at.y - c.depth_m as f64,
            flow_m3_s: c.flow_m3_s,
            order: reach.order,
        })
    }

    fn reaches(&self, lo: DVec2, hi: DVec2, out: &mut Vec<CreekReach>) {
        for (reach, (a, b)) in self.reaches.iter().zip(&self.bounds) {
            if (b.x as f64) >= lo.x
                && (a.x as f64) <= hi.x
                && (b.y as f64) >= lo.y
                && (a.y as f64) <= hi.y
            {
                out.push(reach.clone());
            }
        }
    }
}

/// The meadow, shared (for [`crate::creeks::LocalCreeks`] and a ground query at once).
pub fn meadow() -> Arc<Meadow> {
    static MEADOW: std::sync::OnceLock<Arc<Meadow>> = std::sync::OnceLock::new();
    MEADOW.get_or_init(|| Arc::new(Meadow::new())).clone()
}

fn ground_color(p: Vec2, normal: Vec3, wet: f32) -> Vec3 {
    let srgb = |r: u8, g: u8, b: u8| {
        let c = |v: u8| (v as f32 / 255.0).powf(2.2);
        Vec3::new(c(r), c(g), c(b))
    };
    let patch = noise::fbm2(p.x as f64 * 0.03, p.y as f64 * 0.03, 0xC0105, 3, 2.0, 0.5);
    let grass = srgb(0x5a, 0x74, 0x34).lerp(srgb(0x7a, 0x82, 0x3c), (0.5 + patch).clamp(0.0, 1.0));
    let soil = srgb(0x4a, 0x3c, 0x2a);
    let stones = srgb(0x6e, 0x66, 0x58);
    let rock = srgb(0x77, 0x72, 0x6a);
    let mut c = grass;
    if wet > 1.5 {
        c = stones;
    } else {
        c = c.lerp(soil * 0.8, (wet * 1.2).clamp(0.0, 1.0) * 0.8);
    }
    let steep = 1.0 - normal.y;
    c.lerp(
        rock,
        ((steep - 0.25) * 4.0).clamp(0.0, 1.0) * (wet < 1.5) as u8 as f32,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_network_runs_downhill_and_its_query_finds_it() {
        let m = meadow();
        // Dozens of reaches: the main stream between junctions, a creek and two brooks per
        // side creek.
        assert!(m.reaches.len() >= 40, "{}", m.reaches.len());
        for reach in &m.reaches {
            assert!(
                reach.points.windows(2).all(|w| w[1].at.y <= w[0].at.y),
                "reach {} climbs",
                reach.id
            );
            // Its water sits in its channel, steps and pools and all: the bed under the
            // surface its ribbon draws there by that surface's depth, the ground beside it
            // over it.
            let k = reach.points.len() / 2;
            let mid = &reach.points[k];
            let (x, z) = (mid.at.x as f32, mid.at.z as f32);
            let (course, along) = &m.courses[(reach.id - 1) as usize];
            let (y, depth) = course.surface(along[k], 0.0);
            let bed = m.height(x, z);
            assert!((bed - (y as f32 - depth)).abs() < 0.1, "{bed} {y} {depth}");
            let side = Vec2::new(
                -(reach.points[k + 1].at.z - mid.at.z) as f32,
                (reach.points[k + 1].at.x - mid.at.x) as f32,
            )
            .normalize();
            let bank = Vec2::new(x, z) + side * mid.width_m;
            assert!(m.height(bank.x, bank.y) > y as f32);
        }
        // The query finds the main stream from its bank, and nothing far from any creek.
        let x = 37.0;
        let near = m
            .nearest(DVec3::new(x as f64, 0.0, main_z(x) as f64 + 3.0), 10.0)
            .expect("main stream");
        assert_eq!(near.order, 3);
        assert!((near.distance_m - 3.0).abs() < 0.6, "{near:?}");
        assert!(near.downstream.x > 0.5);
        let mut all = Vec::new();
        m.reaches(DVec2::splat(-1e4), DVec2::splat(1e4), &mut all);
        assert_eq!(all.len(), m.reaches.len());
        let mut none = Vec::new();
        m.reaches(DVec2::splat(5e3), DVec2::splat(6e3), &mut none);
        assert!(none.is_empty());
    }
}
