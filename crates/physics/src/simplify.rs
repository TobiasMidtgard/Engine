//! Collision meshes from render meshes: fewer triangles, the same surface to within a
//! tolerance. A body touching a triangle mesh costs Rapier one contact manifold per triangle
//! near it, every step it is awake: the lake's rowboat, moored at a jetty fitted from its art
//! ([`crate::fit`], [`engine_world::physics::Fit::Mesh`], 66,000 triangles of planks, piles
//! and a ladder), had 740-800 of them, 1-3 ms of narrow phase a step on the VM (round 21).
//!
//! The same remedy as Unreal's simple collision and PhysX's and Unity's advice to collide
//! against simplified meshes, done the way mesh simplifiers do it: vertices welded, then edges
//! collapsed in order of their quadric error (Garland and Heckbert, "Surface Simplification
//! Using Quadric Error Metrics", SIGGRAPH 1997) onto one of their ends (a half-edge collapse,
//! as meshoptimizer's `simplify` does, so no new points appear, and in passes as it does), while
//! the error stays under
//! [`TOLERANCE_M`]. The quadrics are unweighted plane sums, so a collapse's error is at least
//! its largest distance from any plane it touched: bevels, grain and rounded corners go, flat
//! faces stay where they were. Open edges get a plane across them (so a deck's outline stays),
//! and a collapse that would turn a triangle over is skipped.

use std::collections::HashMap;

use engine_core::math::{DVec3, Vec3};

/// Meshes of more triangles than this are simplified ([`for_collision`]).
pub const SIMPLIFY_ABOVE: usize = 2048;
/// How far the simplified surface may stand from the drawn one, meters.
pub const TOLERANCE_M: f64 = 0.01;
/// Points closer than this are one point (render meshes split them per normal and UV), m.
const WELD_M: f64 = 1.0e-4;
/// A collapse may turn a triangle's normal at most this far (cosine).
const MIN_TURN_COS: f64 = 0.3;

/// A symmetric 4x4 matrix as its 10 distinct entries.
type Quadric = [f64; 10];

fn plane_quadric(n: DVec3, d: f64) -> Quadric {
    [
        n.x * n.x,
        n.x * n.y,
        n.x * n.z,
        n.x * d,
        n.y * n.y,
        n.y * n.z,
        n.y * d,
        n.z * n.z,
        n.z * d,
        d * d,
    ]
}

fn add(a: &mut Quadric, b: &Quadric) {
    for (x, y) in a.iter_mut().zip(b) {
        *x += y;
    }
}

/// Summed squared distance of `p` from the quadric's planes.
fn error(q: &Quadric, p: DVec3) -> f64 {
    let (x, y, z) = (p.x, p.y, p.z);
    (q[0] * x * x
        + 2.0 * q[1] * x * y
        + 2.0 * q[2] * x * z
        + 2.0 * q[3] * x
        + q[4] * y * y
        + 2.0 * q[5] * y * z
        + 2.0 * q[6] * y
        + q[7] * z * z
        + 2.0 * q[8] * z
        + q[9])
        .max(0.0)
}

/// `points` and `triangles` with duplicate points welded and degenerate triangles dropped.
fn weld(points: &[Vec3], triangles: &[[u32; 3]]) -> (Vec<DVec3>, Vec<[u32; 3]>) {
    let mut at: HashMap<[i64; 3], u32> = HashMap::new();
    let mut out: Vec<DVec3> = Vec::new();
    let remap: Vec<u32> = points
        .iter()
        .map(|p| {
            let p = p.as_dvec3();
            let key = (p / WELD_M).round().to_array().map(|c| c as i64);
            *at.entry(key).or_insert_with(|| {
                out.push(p);
                (out.len() - 1) as u32
            })
        })
        .collect();
    let tris = triangles
        .iter()
        .filter(|t| t.iter().all(|&i| (i as usize) < points.len()))
        .map(|t| t.map(|i| remap[i as usize]))
        .filter(|t| t[0] != t[1] && t[1] != t[2] && t[0] != t[2])
        .collect();
    (out, tris)
}

fn normal(p: &[DVec3], t: [u32; 3]) -> DVec3 {
    let [a, b, c] = t.map(|i| p[i as usize]);
    (b - a).cross(c - a)
}

/// `points` and `triangles` for colliding against: as drawn up to [`SIMPLIFY_ABOVE`]
/// triangles (welded), else simplified to within [`TOLERANCE_M`].
pub fn for_collision(points: &[Vec3], triangles: &[[u32; 3]]) -> (Vec<Vec3>, Vec<[u32; 3]>) {
    if triangles.len() <= SIMPLIFY_ABOVE {
        return (points.to_vec(), triangles.to_vec());
    }
    let started = std::time::Instant::now();
    let (mut p, mut tris) = weld(points, triangles);
    let n = p.len();
    // Each point's triangles, and the planes about it.
    let mut around: Vec<Vec<u32>> = vec![Vec::new(); n];
    let mut quadrics: Vec<Quadric> = vec![[0.0; 10]; n];
    let mut edges: HashMap<(u32, u32), u32> = HashMap::new();
    for (ti, t) in tris.iter().enumerate() {
        let nrm = normal(&p, *t);
        let len = nrm.length();
        for &i in t {
            around[i as usize].push(ti as u32);
        }
        for k in 0..3 {
            let (a, b) = (t[k], t[(k + 1) % 3]);
            *edges.entry((a.min(b), a.max(b))).or_default() += 1;
        }
        if len <= 1.0e-12 {
            continue;
        }
        let nrm = nrm / len;
        let q = plane_quadric(nrm, -nrm.dot(p[t[0] as usize]));
        for &i in t {
            add(&mut quadrics[i as usize], &q);
        }
    }
    // Open edges keep their place: a plane through them, square to the surface. Points on an
    // edge of more than two triangles (a plank's face meeting another plank) stay put.
    let mut locked = vec![false; n];
    for (t, nrm) in tris.iter().map(|t| (t, normal(&p, *t))) {
        let face = nrm.normalize_or_zero();
        for k in 0..3 {
            let (a, b) = (t[k], t[(k + 1) % 3]);
            match edges[&(a.min(b), a.max(b))] {
                1 => {
                    let side = (p[b as usize] - p[a as usize])
                        .cross(face)
                        .normalize_or_zero();
                    if side != DVec3::ZERO {
                        let q = plane_quadric(side, -side.dot(p[a as usize]));
                        add(&mut quadrics[a as usize], &q);
                        add(&mut quadrics[b as usize], &q);
                    }
                }
                2 => {}
                _ => {
                    locked[a as usize] = true;
                    locked[b as usize] = true;
                }
            }
        }
    }
    drop(edges);
    let mut alive = vec![true; tris.len()];
    let limit = TOLERANCE_M * TOLERANCE_M;
    // In passes, as meshoptimizer does: each point's cheapest collapse under the limit, the
    // cheapest first, none two beside each other in one pass (so each one's check for turned
    // triangles holds), until a pass finds none. (One queue of every edge, both ways, popped
    // 60 stale or refused entries a collapse: 1.5 s for 230,000 triangles.)
    let mut best: Vec<(f64, u32)> = vec![(f64::INFINITY, u32::MAX); n];
    let mut touched = vec![false; n];
    loop {
        best.fill((f64::INFINITY, u32::MAX));
        for (t, _) in tris.iter().zip(&alive).filter(|(_, a)| **a) {
            for k in 0..3 {
                let (a, b) = (t[k], t[(k + 1) % 3]);
                for (from, to) in [(a, b), (b, a)] {
                    if locked[from as usize] {
                        continue;
                    }
                    let mut q = quadrics[from as usize];
                    add(&mut q, &quadrics[to as usize]);
                    let cost = error(&q, p[to as usize]);
                    if cost <= limit && cost < best[from as usize].0 {
                        best[from as usize] = (cost, to);
                    }
                }
            }
        }
        let mut order: Vec<(f64, u32, u32)> = best
            .iter()
            .enumerate()
            .filter(|(_, b)| b.1 != u32::MAX)
            .map(|(i, b)| (b.0, i as u32, b.1))
            .collect();
        if order.is_empty() {
            break;
        }
        order.sort_unstable_by(|a, b| a.0.total_cmp(&b.0));
        touched.fill(false);
        let mut collapsed = 0usize;
        for (_, from_u, to_u) in order {
            let (from, to) = (from_u as usize, to_u as usize);
            if touched[from] || touched[to] {
                continue;
            }
            // No triangle about `from` turns over.
            let mut flips = false;
            for &ti in &around[from] {
                let t = tris[ti as usize];
                if !alive[ti as usize] || t.contains(&to_u) {
                    continue;
                }
                let before = normal(&p, t);
                let moved = t.map(|i| if i == from_u { to_u } else { i });
                let after = normal(&p, moved);
                let (lb, la) = (before.length(), after.length());
                if la <= 1.0e-12 || (lb > 1.0e-12 && before.dot(after) < MIN_TURN_COS * lb * la) {
                    flips = true;
                    break;
                }
            }
            if flips {
                continue;
            }
            // Collapse: `from`'s triangles go to `to`, those on the edge go.
            let mine = std::mem::take(&mut around[from]);
            for ti in mine {
                if !alive[ti as usize] {
                    continue;
                }
                let t = &mut tris[ti as usize];
                if t.contains(&to_u) {
                    alive[ti as usize] = false;
                    continue;
                }
                for i in t.iter_mut() {
                    if *i == from_u {
                        *i = to_u;
                    }
                }
                around[to].push(ti);
            }
            let qf = quadrics[from];
            add(&mut quadrics[to], &qf);
            around[to].retain(|&ti| alive[ti as usize]);
            touched[from] = true;
            for &ti in &around[to] {
                for i in tris[ti as usize] {
                    touched[i as usize] = true;
                }
            }
            collapsed += 1;
        }
        if collapsed == 0 {
            break;
        }
    }
    // Only the points still used, in order.
    let mut index = vec![u32::MAX; n];
    let mut out_points: Vec<Vec3> = Vec::new();
    let mut out_tris: Vec<[u32; 3]> = Vec::new();
    for (t, _) in tris.iter_mut().zip(&alive).filter(|(_, a)| **a) {
        let mapped = t.map(|i| {
            let slot = &mut index[i as usize];
            if *slot == u32::MAX {
                out_points.push(p[i as usize].as_vec3());
                *slot = (out_points.len() - 1) as u32;
            }
            *slot
        });
        out_tris.push(mapped);
    }
    p.clear();
    if crate::report::wanted() {
        engine_core::log::info!(
            "physics: a fitted mesh of {} triangles collides as {} ({:.1} ms)",
            triangles.len(),
            out_tris.len(),
            started.elapsed().as_secs_f64() * 1000.0
        );
    }
    (out_points, out_tris)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A plank `w` x `h` x `d` m with its long top face split into a fine grid (as a render
    /// mesh's grain and bevels are), as one closed box.
    fn plank(cells: u32, w: f32, h: f32, d: f32) -> (Vec<Vec3>, Vec<[u32; 3]>) {
        let mut points = Vec::new();
        let mut tris = Vec::new();
        // A grid face from `o` along `u` and `v`.
        let mut face = |o: Vec3, u: Vec3, v: Vec3, cells: u32| {
            let base = points.len() as u32;
            for j in 0..=cells {
                for i in 0..=cells {
                    points.push(o + u * (i as f32 / cells as f32) + v * (j as f32 / cells as f32));
                }
            }
            let row = cells + 1;
            for j in 0..cells {
                for i in 0..cells {
                    let a = base + j * row + i;
                    tris.push([a, a + 1, a + row + 1]);
                    tris.push([a, a + row + 1, a + row]);
                }
            }
        };
        let (x, y, z) = (Vec3::X * w, Vec3::Y * h, Vec3::Z * d);
        face(Vec3::ZERO + y, z, x, cells); // top (+y)
        face(Vec3::ZERO, x, z, cells); // bottom
        face(Vec3::ZERO, y, x, cells); // -z
        face(z, x, y, cells); // +z
        face(Vec3::ZERO, z, y, cells); // -x
        face(x, y, z, cells); // +x
        (points, tris)
    }

    #[test]
    fn small_meshes_stay_as_drawn() {
        let (p, t) = plank(4, 2.0, 0.05, 0.2);
        let (sp, st) = for_collision(&p, &t);
        assert_eq!((sp.len(), st.len()), (p.len(), t.len()));
    }

    #[test]
    fn a_finely_cut_plank_collides_as_a_few_triangles_on_the_same_faces() {
        let (p, t) = plank(24, 2.0, 0.05, 0.2);
        assert!(t.len() > SIMPLIFY_ABOVE);
        let (sp, st) = for_collision(&p, &t);
        assert!(
            st.len() * 20 < t.len(),
            "{} of {} triangles",
            st.len(),
            t.len()
        );
        // Every point kept is on the box's surface, and the box keeps its corners.
        for q in &sp {
            let on = [
                q.x.abs(),
                (q.x - 2.0).abs(),
                q.y.abs(),
                (q.y - 0.05).abs(),
                q.z.abs(),
                (q.z - 0.2).abs(),
            ]
            .iter()
            .any(|d| *d < 1.0e-5);
            assert!(on, "{q} left the surface");
        }
        for corner in [
            Vec3::ZERO,
            Vec3::new(2.0, 0.05, 0.2),
            Vec3::new(2.0, 0.0, 0.0),
        ] {
            let near = sp
                .iter()
                .map(|q| q.distance(corner))
                .fold(f32::MAX, f32::min);
            assert!(
                near <= TOLERANCE_M as f32,
                "lost {corner} (nearest {near} m)"
            );
        }
        // About the same volume (still closed, nothing caved in).
        let volume: f64 = st
            .iter()
            .map(|t| {
                let [a, b, c] = t.map(|i| sp[i as usize].as_dvec3());
                a.dot(b.cross(c)) / 6.0
            })
            .sum();
        let full = 2.0 * 0.05 * 0.2;
        assert!(
            (volume.abs() - full).abs() < 0.1 * full,
            "volume {volume} of {full}"
        );
    }

    #[test]
    fn bumps_bigger_than_the_tolerance_stay() {
        // A finely cut sheet with a 5 cm ridge across it: the ridge stays.
        let cells = 40u32;
        let mut points = Vec::new();
        for j in 0..=cells {
            for i in 0..=cells {
                let x = i as f32 / cells as f32 * 4.0;
                let ridge = if (x - 2.0).abs() < 0.11 { 0.05 } else { 0.0 };
                points.push(Vec3::new(x, ridge, j as f32 / cells as f32 * 4.0));
            }
        }
        let row = cells + 1;
        let mut tris = Vec::new();
        for j in 0..cells {
            for i in 0..cells {
                let a = j * row + i;
                tris.push([a, a + row + 1, a + 1]);
                tris.push([a, a + row, a + row + 1]);
            }
        }
        let (sp, st) = for_collision(&points, &tris);
        assert!(st.len() < tris.len() / 4, "{} of {}", st.len(), tris.len());
        assert!(sp.iter().any(|q| q.y > 0.049), "the ridge went");
        assert!(sp.iter().any(|q| q.y < 0.001), "the floor went");
    }
}
