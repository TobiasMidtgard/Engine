//! Mesh simplification by half-edge collapses ordered by quadric error (Garland and Heckbert
//! 1997), with locked vertices.
//!
//! Every collapse moves one vertex onto a neighbor, so the simplified mesh only uses original
//! vertices: their normals, texture coordinates and colors stay exact, and every level of the
//! cluster hierarchy shares one vertex buffer. Locked vertices never move and are never
//! removed, so a group of clusters simplified with its outer border locked still fits its
//! neighbors exactly (no cracks).
//!
//! Collapses run in passes: each pass sorts every possible collapse by cost and performs the
//! cheapest ones whose neighborhoods haven't changed yet in that pass. A collapse is skipped
//! when it would make the surface non-manifold (link condition) or turn a triangle over.

use engine_core::math::{DVec3, Vec3};

/// Symmetric 4×4 quadric (the upper triangle of `Σ w · p pᵀ` for planes `p = (n, d)`), with the
/// total weight so errors can be normalized to a mean squared distance.
#[derive(Debug, Clone, Copy, Default)]
struct Quadric {
    m: [f64; 10],
    weight: f64,
}

impl Quadric {
    /// Plane through `p0` with unit normal `n`, weighted by `w`.
    fn plane(n: DVec3, p0: DVec3, w: f64) -> Self {
        let d = -n.dot(p0);
        let (a, b, c) = (n.x, n.y, n.z);
        Self {
            m: [
                a * a * w,
                a * b * w,
                a * c * w,
                a * d * w,
                b * b * w,
                b * c * w,
                b * d * w,
                c * c * w,
                c * d * w,
                d * d * w,
            ],
            weight: w,
        }
    }

    fn add(&mut self, o: &Quadric) {
        for (a, b) in self.m.iter_mut().zip(o.m.iter()) {
            *a += b;
        }
        self.weight += o.weight;
    }

    /// Weighted mean squared distance from `p` to the planes.
    fn error(&self, p: DVec3) -> f64 {
        if self.weight <= 0.0 {
            return 0.0;
        }
        let m = &self.m;
        let (x, y, z) = (p.x, p.y, p.z);
        let e = m[0] * x * x
            + 2.0 * m[1] * x * y
            + 2.0 * m[2] * x * z
            + 2.0 * m[3] * x
            + m[4] * y * y
            + 2.0 * m[5] * y * z
            + 2.0 * m[6] * y
            + m[7] * z * z
            + 2.0 * m[8] * z
            + m[9];
        e.max(0.0) / self.weight
    }
}

/// Turning a triangle by more than about 78° rejects a collapse.
const MIN_NORMAL_COS: f64 = 0.2;

/// Result of [`simplify`].
#[derive(Debug, Clone, Default)]
pub struct Simplified {
    /// Remaining triangles (indices into the input positions), counter-clockwise as given.
    pub triangles: Vec<[u32; 3]>,
    /// Largest root mean squared distance from a moved vertex to the planes of the surface it
    /// replaced, in the positions' units.
    pub error: f32,
}

/// Simplify `triangles` toward `target` triangles by collapsing edges. `locked[v]` vertices
/// keep their place; the caller locks borders that must not change (and seams). Vertices whose
/// edges aren't each shared by exactly two triangles are locked automatically.
pub fn simplify(
    positions: &[Vec3],
    triangles: &[[u32; 3]],
    locked: &[bool],
    target: usize,
) -> Simplified {
    let n = positions.len();
    let pos: Vec<DVec3> = positions.iter().map(|p| p.as_dvec3()).collect();
    let mut tris: Vec<[u32; 3]> = triangles.to_vec();
    let mut alive = vec![true; tris.len()];
    let mut live = tris.len();

    // Incident triangles per vertex (dead entries are skipped lazily).
    let mut incident: Vec<Vec<u32>> = vec![Vec::new(); n];
    for (t, tri) in tris.iter().enumerate() {
        for &v in tri {
            incident[v as usize].push(t as u32);
        }
    }

    // Area-weighted plane quadrics.
    let mut quadrics = vec![Quadric::default(); n];
    for tri in &tris {
        let [a, b, c] = tri.map(|i| pos[i as usize]);
        let cross = (b - a).cross(c - a);
        let area2 = cross.length();
        if area2 <= 0.0 {
            continue;
        }
        let q = Quadric::plane(cross / area2, a, area2 * 0.5);
        for &v in tri {
            quadrics[v as usize].add(&q);
        }
    }

    // Movable vertices: not locked and every incident edge shared by exactly two triangles.
    let mut movable: Vec<bool> = (0..n).map(|v| !locked[v]).collect();
    {
        let mut edges: Vec<(u32, u32)> = Vec::with_capacity(tris.len() * 3);
        for tri in &tris {
            for k in 0..3 {
                let (a, b) = (tri[k], tri[(k + 1) % 3]);
                edges.push((a.min(b), a.max(b)));
            }
        }
        edges.sort_unstable();
        let mut i = 0;
        while i < edges.len() {
            let mut j = i;
            while j < edges.len() && edges[j] == edges[i] {
                j += 1;
            }
            if j - i != 2 {
                movable[edges[i].0 as usize] = false;
                movable[edges[i].1 as usize] = false;
            }
            i = j;
        }
    }

    let mut dead = vec![false; n];
    let mut dirty = vec![false; n];
    let mut worst = 0.0f64;
    let mut candidates: Vec<(f64, u32, u32)> = Vec::new();
    let mut ring_u: Vec<u32> = Vec::new();
    let mut ring_v: Vec<u32> = Vec::new();

    while live > target {
        candidates.clear();
        for u in 0..n {
            if dead[u] || !movable[u] {
                continue;
            }
            neighbors(u as u32, &incident, &tris, &alive, &mut ring_u);
            for &v in &ring_u {
                let cost = quadrics[u].error(pos[v as usize]);
                candidates.push((cost, u as u32, v));
            }
        }
        if candidates.is_empty() {
            break;
        }
        candidates.sort_unstable_by(|a, b| a.0.total_cmp(&b.0));
        dirty.fill(false);
        let mut collapsed = 0;
        for &(cost, u, v) in &candidates {
            if live <= target {
                break;
            }
            let (ui, vi) = (u as usize, v as usize);
            if dead[ui] || dead[vi] || dirty[ui] || dirty[vi] {
                continue;
            }
            neighbors(u, &incident, &tris, &alive, &mut ring_u);
            if !ring_u.contains(&v) {
                continue;
            }
            neighbors(v, &incident, &tris, &alive, &mut ring_v);
            // Link condition: u and v may only share the opposite corners of the two
            // triangles on their edge, or the collapse pinches the surface.
            let common = ring_u.iter().filter(|w| ring_v.contains(w)).count();
            let shared = incident[ui]
                .iter()
                .filter(|&&t| alive[t as usize] && tris[t as usize].contains(&v))
                .count();
            if shared != 2 || common != 2 {
                continue;
            }
            if flips(u, v, &incident[ui], &tris, &alive, &pos) {
                continue;
            }
            // Collapse u into v.
            for k in 0..incident[ui].len() {
                let t = incident[ui][k] as usize;
                if !alive[t] {
                    continue;
                }
                if tris[t].contains(&v) {
                    alive[t] = false;
                    live -= 1;
                } else {
                    for c in &mut tris[t] {
                        if *c == u {
                            *c = v;
                        }
                    }
                    incident[vi].push(t as u32);
                }
            }
            incident[ui].clear();
            let qu = quadrics[ui];
            quadrics[vi].add(&qu);
            dead[ui] = true;
            dirty[ui] = true;
            dirty[vi] = true;
            for &w in &ring_u {
                dirty[w as usize] = true;
            }
            worst = worst.max(cost);
            collapsed += 1;
        }
        if collapsed == 0 {
            break;
        }
        // Drop dead triangles from the incident lists now and then.
        for list in &mut incident {
            list.retain(|&t| alive[t as usize]);
        }
    }

    Simplified {
        triangles: tris
            .iter()
            .zip(&alive)
            .filter(|(_, a)| **a)
            .map(|(t, _)| *t)
            .collect(),
        error: worst.sqrt() as f32,
    }
}

/// Vertices sharing a live triangle with `v` (sorted, without duplicates).
fn neighbors(v: u32, incident: &[Vec<u32>], tris: &[[u32; 3]], alive: &[bool], out: &mut Vec<u32>) {
    out.clear();
    for &t in &incident[v as usize] {
        if !alive[t as usize] {
            continue;
        }
        for &w in &tris[t as usize] {
            if w != v {
                out.push(w);
            }
        }
    }
    out.sort_unstable();
    out.dedup();
}

/// Whether moving `u` onto `v` turns any of `u`'s other triangles over (or collapses one to
/// nothing).
fn flips(
    u: u32,
    v: u32,
    incident_u: &[u32],
    tris: &[[u32; 3]],
    alive: &[bool],
    pos: &[DVec3],
) -> bool {
    let pv = pos[v as usize];
    for &t in incident_u {
        let t = t as usize;
        if !alive[t] || tris[t].contains(&v) {
            continue;
        }
        let [a, b, c] = tris[t];
        let p = |i: u32| pos[i as usize];
        let before = (p(b) - p(a)).cross(p(c) - p(a));
        let q = |i: u32| if i == u { pv } else { pos[i as usize] };
        let after = (q(b) - q(a)).cross(q(c) - q(a));
        let (lb, la) = (before.length(), after.length());
        if la <= lb * 1e-6 || before.dot(after) < MIN_NORMAL_COS * lb * la {
            return true;
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A bumpy `n`×`n` grid on XZ (counter-clockwise from above).
    fn grid(n: u32) -> (Vec<Vec3>, Vec<[u32; 3]>) {
        let mut p = Vec::new();
        for z in 0..=n {
            for x in 0..=n {
                let (fx, fz) = (x as f32, z as f32);
                p.push(Vec3::new(
                    fx,
                    0.05 * (fx * 0.7).sin() * (fz * 0.9).cos(),
                    fz,
                ));
            }
        }
        let row = n + 1;
        let mut t = Vec::new();
        for z in 0..n {
            for x in 0..n {
                let i = z * row + x;
                t.push([i, i + row, i + 1]);
                t.push([i + 1, i + row, i + row + 1]);
            }
        }
        (p, t)
    }

    #[test]
    fn simplifies_inside_a_locked_border() {
        let n = 16;
        let (p, t) = grid(n);
        let on_border = |i: usize| {
            let (x, z) = (i as u32 % (n + 1), i as u32 / (n + 1));
            x == 0 || z == 0 || x == n || z == n
        };
        let locked: Vec<bool> = (0..p.len()).map(on_border).collect();
        let out = simplify(&p, &t, &locked, t.len() / 4);
        assert!(
            out.triangles.len() <= t.len() / 3,
            "only reduced {} to {}",
            t.len(),
            out.triangles.len()
        );
        assert!(out.error > 0.0 && out.error < 0.1, "error {}", out.error);
        // The border is unchanged: every boundary edge of the input is still a boundary edge.
        let boundary = |tris: &[[u32; 3]]| {
            let mut count = std::collections::HashMap::new();
            for tri in tris {
                for k in 0..3 {
                    let (a, b) = (tri[k], tri[(k + 1) % 3]);
                    *count.entry((a.min(b), a.max(b))).or_insert(0) += 1;
                }
            }
            let mut b: Vec<_> = count
                .into_iter()
                .filter(|(_, c)| *c == 1)
                .map(|(e, _)| e)
                .collect();
            b.sort_unstable();
            b
        };
        assert_eq!(boundary(&t), boundary(&out.triangles));
        // Nothing turned over: every triangle still faces up.
        for tri in &out.triangles {
            let [a, b, c] = tri.map(|i| p[i as usize]);
            assert!((b - a).cross(c - a).y > 0.0);
        }
    }

    #[test]
    fn locked_everywhere_changes_nothing() {
        let (p, t) = grid(4);
        let out = simplify(&p, &t, &vec![true; p.len()], 1);
        assert_eq!(out.triangles.len(), t.len());
        assert_eq!(out.error, 0.0);
    }
}
