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
    /// Which input triangle each remaining one was (corner for corner).
    pub kept: Vec<u32>,
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
    run(positions, triangles, locked, target, None)
}

/// [`Welded::moved`]: the corners (triangle, corner) about to move, and the vertex they move
/// onto.
pub type Moved<'a> = dyn FnMut(&[(u32, u8)], u32) -> bool + 'a;

/// What [`simplify_welded`] asks of its caller.
pub struct Welded<'a> {
    /// The connected part each vertex belongs to, and each part's surface area in the mesh
    /// the chain of levels started from: no part may end up with much less or more surface
    /// than it had ([`MIN_PART_KEPT`]), however many levels down.
    pub part: &'a [u32],
    pub part_area: &'a [f64],
    /// Asked before a collapse: the corners (triangle, corner 0..3, indices into the input
    /// triangles) about to move onto vertex `v`. `false` forbids the collapse; `true` lets it
    /// happen, and the callee notes what it needs to (which of a place's vertices each corner
    /// is drawn with from now on).
    pub moved: &'a mut Moved<'a>,
}

/// [`simplify`] for triangles over welded positions (one vertex per place, so an edge used by
/// one triangle really is the surface's border): vertices on a border may move along it (a
/// plank loses the vertices along its sides, a tube's rim some of its corners), no collapse
/// may change the surface around it by much ([`MIN_AREA_KEPT`], [`MIN_PART_KEPT`]: a leaf card
/// keeps its four corners, a rail its thickness, a bent blade of grass doesn't become the
/// triangle between its ends), and the caller decides which collapses its other vertex data
/// allows ([`Welded`]).
pub fn simplify_welded(
    positions: &[Vec3],
    triangles: &[[u32; 3]],
    locked: &[bool],
    target: usize,
    welded: Welded,
) -> Simplified {
    run(positions, triangles, locked, target, Some(welded))
}

/// Weight of the planes that hold an open border in place, against the triangles' own
/// (per border edge: this times its length squared).
const BORDER_WEIGHT: f64 = 8.0;

fn run(
    positions: &[Vec3],
    triangles: &[[u32; 3]],
    locked: &[bool],
    target: usize,
    mut welded: Option<Welded>,
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
    // Over welded positions a vertex with two border edges is movable too, along the border:
    // each border edge adds a plane through itself, square to its triangle, so leaving the
    // border's line costs as much as leaving the surface does.
    let mut movable: Vec<bool> = (0..n).map(|v| !locked[v]).collect();
    let mut border = vec![0u8; n];
    {
        let mut edges: Vec<(u32, u32, u32)> = Vec::with_capacity(tris.len() * 3);
        for (t, tri) in tris.iter().enumerate() {
            for k in 0..3 {
                let (a, b) = (tri[k], tri[(k + 1) % 3]);
                edges.push((a.min(b), a.max(b), t as u32));
            }
        }
        edges.sort_unstable();
        for run in edges.chunk_by(|a, b| (a.0, a.1) == (b.0, b.1)) {
            let (a, b, t) = run[0];
            if run.len() == 2 {
                continue;
            }
            if run.len() == 1 && welded.is_some() {
                let [p, q, r] = tris[t as usize].map(|i| pos[i as usize]);
                let normal = (q - p).cross(r - p);
                let along = pos[b as usize] - pos[a as usize];
                let out = along.cross(normal);
                if out.length_squared() > 0.0 {
                    let plane = Quadric::plane(
                        out.normalize(),
                        pos[a as usize],
                        BORDER_WEIGHT * along.length_squared(),
                    );
                    for v in [a, b] {
                        quadrics[v as usize].add(&plane);
                        border[v as usize] = border[v as usize].saturating_add(1);
                    }
                    continue;
                }
            }
            movable[a as usize] = false;
            movable[b as usize] = false;
        }
        // Where borders meet (or one ends), nothing moves.
        for v in 0..n {
            movable[v] &= border[v] == 0 || border[v] == 2;
        }
    }
    let mut moves: Vec<(u32, u8)> = Vec::new();
    // The surface each part has now.
    let mut part_area: Vec<f64> = Vec::new();
    if let Some(w) = &welded {
        part_area.resize(w.part_area.len(), 0.0);
        for tri in &tris {
            let [a, b, c] = tri.map(|i| pos[i as usize]);
            part_area[w.part[tri[0] as usize] as usize] += (b - a).cross(c - a).length() * 0.5;
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
                // A border vertex only moves to the next one along its border.
                if border[u] != 0 && border[v as usize] == 0 {
                    continue;
                }
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
            // (Along a border the edge has one triangle and one opposite corner.)
            let sides = if border[ui] != 0 { 1 } else { 2 };
            if shared != sides || common != sides {
                continue;
            }
            if flips(u, v, &incident[ui], &tris, &alive, &pos) {
                continue;
            }
            // Thin parts (a rail, a rope, a post) must not flatten away, nor a bent blade
            // become the triangle between its ends: their surface strays little as they do,
            // but the picture changes.
            let mut area_after = 0.0;
            if let Some(w) = &welded {
                let (before, after) = area_around(u, v, &incident[ui], &tris, &alive, &pos);
                let part = w.part[ui] as usize;
                area_after = part_area[part] + after - before;
                let had = w.part_area[part];
                if after < before * MIN_AREA_KEPT
                    || after * MIN_AREA_KEPT > before
                    || area_after < had * MIN_PART_KEPT
                    || area_after * MIN_PART_KEPT > had
                {
                    continue;
                }
            }
            if let Some(w) = &mut welded {
                moves.clear();
                for &t in &incident[ui] {
                    let tri = &tris[t as usize];
                    if alive[t as usize] && !tri.contains(&v) {
                        let corner = tri.iter().position(|c| *c == u).unwrap_or(0);
                        moves.push((t, corner as u8));
                    }
                }
                if !(w.moved)(&moves, v) {
                    continue;
                }
                part_area[w.part[ui] as usize] = area_after;
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

    let kept: Vec<u32> = (0..tris.len() as u32)
        .filter(|&t| alive[t as usize])
        .collect();
    Simplified {
        triangles: kept.iter().map(|&t| tris[t as usize]).collect(),
        kept,
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

/// A collapse must leave this share of the area of the triangles around the moved vertex, and
/// not more than one over it.
const MIN_AREA_KEPT: f64 = 0.8;
/// A connected part must keep this share of the area it had in the full mesh, and not more
/// than one over it.
const MIN_PART_KEPT: f64 = 0.9;

/// The area of the triangles around `u`, and of those left once `u` has moved onto `v`. On a
/// surface that goes on, the triangles left stretch over the two that go; where a thin part
/// flattens they don't, and where a bent strip is cut short they span the bend.
fn area_around(
    u: u32,
    v: u32,
    incident_u: &[u32],
    tris: &[[u32; 3]],
    alive: &[bool],
    pos: &[DVec3],
) -> (f64, f64) {
    let pv = pos[v as usize];
    let (mut before, mut after) = (0.0, 0.0);
    for &t in incident_u {
        let t = t as usize;
        if !alive[t] {
            continue;
        }
        let [a, b, c] = tris[t].map(|i| pos[i as usize]);
        before += (b - a).cross(c - a).length() * 0.5;
        if !tris[t].contains(&v) {
            let [a, b, c] = tris[t].map(|i| if i == u { pv } else { pos[i as usize] });
            after += (b - a).cross(c - a).length() * 0.5;
        }
    }
    (before, after)
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
