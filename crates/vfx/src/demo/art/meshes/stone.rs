//! Cut stone: flagstones and paving layouts, arch voussoirs and rune glyphs.

use engine_core::math::{Quat, Rng, Vec2, Vec3};
use engine_render::Mesh;

use super::{Outline, Slab, moved, warp};

/// The centroid of a polygon's corners.
pub fn centroid(poly: &[Vec2]) -> Vec2 {
    poly.iter().copied().sum::<Vec2>() / poly.len().max(1) as f32
}

/// A polygon's area (absolute), m².
pub fn area(poly: &[Vec2]) -> f32 {
    let n = poly.len();
    (0..n)
        .map(|i| {
            let (a, b) = (poly[i], poly[(i + 1) % n]);
            a.x * b.y - b.x * a.y
        })
        .sum::<f32>()
        .abs()
        * 0.5
}

/// One paving stone or block: a convex polygon on the XZ plane, beveled, from `y0` to `y1`,
/// tilted by `tilt` (radians about X and Z) around its center. The polygon may sit anywhere;
/// the result is in the same space.
#[derive(Debug, Clone)]
pub struct Stone {
    pub poly: Vec<Vec2>,
    pub y0: f32,
    pub y1: f32,
    pub bevel: f32,
    pub tilt: Vec2,
    /// Corner rounding segments of the bevel.
    pub segs: u32,
    /// Extra rings across the top (for a domed or bent top).
    pub detail: u32,
    /// Raise the middle of the top by this much (worn stones dish down, new ones dome up).
    pub dome: f32,
}

impl Stone {
    pub fn new(poly: Vec<Vec2>, y0: f32, y1: f32, bevel: f32) -> Self {
        Self {
            poly,
            y0,
            y1,
            bevel,
            tilt: Vec2::ZERO,
            segs: 2,
            detail: 1,
            dome: 0.0,
        }
    }

    pub fn mesh(&self) -> Mesh {
        let c = centroid(&self.poly);
        let local: Vec<Vec2> = self.poly.iter().map(|p| *p - c).collect();
        let radius = local
            .iter()
            .map(|p| p.length())
            .fold(0.0, f32::max)
            .max(1e-3);
        let mut m = Slab::new(Outline::new(local), self.y0, self.y1, self.bevel)
            .segs(self.segs)
            .detail(self.detail)
            .open_bottom()
            .mesh();
        if self.dome != 0.0 {
            let (top, dome) = (self.y1, self.dome);
            let h = (self.y1 - self.y0).max(1e-3);
            warp(&mut m, |p| {
                let k = 1.0 - (Vec2::new(p.x, p.z).length() / radius).powi(2);
                let up = ((p.y - (top - h)) / h).clamp(0.0, 1.0);
                p + Vec3::Y * (dome * k.max(0.0) * up)
            });
        }
        let rot = Quat::from_rotation_x(self.tilt.x) * Quat::from_rotation_z(self.tilt.y);
        moved(m, Vec3::new(c.x, 0.0, c.y), rot)
    }
}

/// Paving courses filling the ring `r0..r1` around the origin: rings of stones `course`
/// (min, max) meters wide, each stone `length` (min, max) meters along the ring, with joints
/// `gap` wide. Returns each stone's polygon (counter-clockwise), with its course index.
pub fn ring_courses(
    rng: &mut Rng,
    r0: f32,
    r1: f32,
    course: (f32, f32),
    length: (f32, f32),
    gap: f32,
) -> Vec<(usize, Vec<Vec2>)> {
    let mut out = Vec::new();
    let mut r = r0;
    let mut index = 0;
    while r < r1 - 0.05 {
        let mut w = rng.range(course.0, course.1);
        if r1 - (r + w) < course.0 * 0.7 {
            w = r1 - r;
        }
        let (ri, ro) = (r + gap * 0.5, r + w - gap * 0.5);
        let mid = (ri + ro) * 0.5;
        let circumference = std::f32::consts::TAU * mid;
        // Split the circle into stones of random lengths that close exactly.
        let mut cuts = vec![0.0f32];
        let mut s = 0.0;
        loop {
            let l = rng.range(length.0, length.1);
            if s + l > circumference - length.0 * 0.6 {
                break;
            }
            s += l;
            cuts.push(s);
        }
        let scale = circumference / (s + (circumference - s));
        let phase = rng.range(0.0, std::f32::consts::TAU);
        let n = cuts.len();
        for i in 0..n {
            let a0 = phase + cuts[i] * scale / mid;
            let a1 = phase
                + if i + 1 < n {
                    cuts[i + 1] * scale / mid
                } else {
                    std::f32::consts::TAU
                };
            out.push((index, sector(ri, ro, a0, a1, gap)));
        }
        r += w;
        index += 1;
    }
    out
}

/// An annular sector `ri..ro`, angles `a0..a1`, shrunk by half a joint `gap` at the ends
/// (the radial edges stay parallel), with extra points on long arcs.
pub fn sector(ri: f32, ro: f32, a0: f32, a1: f32, gap: f32) -> Vec<Vec2> {
    let at = |r: f32, a: f32| Vec2::new(a.cos(), a.sin()) * r;
    let pieces = |r: f32| (((a1 - a0) * r / 0.45).ceil() as u32).clamp(1, 12);
    let mut pts = Vec::new();
    let (no, ni) = (pieces(ro), pieces(ri));
    let (go, gi) = (gap * 0.5 / ro, gap * 0.5 / ri);
    for k in 0..=no {
        let a = (a0 + go) + (a1 - a0 - 2.0 * go) * k as f32 / no as f32;
        pts.push(at(ro, a));
    }
    for k in 0..=ni {
        let a = (a1 - gi) - (a1 - a0 - 2.0 * gi) * k as f32 / ni as f32;
        pts.push(at(ri, a));
    }
    pts
}

/// Clip a convex polygon to the half-plane `dot(p, n) <= d`.
pub fn clip(poly: &[Vec2], n: Vec2, d: f32) -> Vec<Vec2> {
    let mut out = Vec::with_capacity(poly.len() + 1);
    let len = poly.len();
    for i in 0..len {
        let (a, b) = (poly[i], poly[(i + 1) % len]);
        let (da, db) = (a.dot(n) - d, b.dot(n) - d);
        if da <= 0.0 {
            out.push(a);
        }
        if (da <= 0.0) != (db <= 0.0) {
            let t = da / (da - db);
            out.push(a.lerp(b, t));
        }
    }
    out
}

/// Voronoi cells of jittered-grid seeds over the box `min..max`, `cell` meters apart,
/// `jitter` (0..1) of a cell off the grid, each shrunk by half of `gap`. Returns the cells'
/// convex polygons.
pub fn voronoi(
    rng: &mut Rng,
    min: Vec2,
    max: Vec2,
    cell: f32,
    jitter: f32,
    gap: f32,
) -> Vec<Vec<Vec2>> {
    let nx = ((max.x - min.x) / cell).ceil().max(1.0) as i32;
    let nz = ((max.y - min.y) / cell).ceil().max(1.0) as i32;
    let mut seeds = Vec::new();
    for j in 0..nz {
        for i in 0..nx {
            let base = min + Vec2::new(i as f32 + 0.5, j as f32 + 0.5) * cell;
            let off = Vec2::new(rng.range(-0.5, 0.5), rng.range(-0.5, 0.5)) * cell * jitter;
            seeds.push(base + off);
        }
    }
    let at = |i: i32, j: i32| seeds[(j * nx + i) as usize];
    let mut cells = Vec::new();
    for j in 0..nz {
        for i in 0..nx {
            let s = at(i, j);
            let mut poly = vec![
                Vec2::new(min.x, min.y),
                Vec2::new(max.x, min.y),
                Vec2::new(max.x, max.y),
                Vec2::new(min.x, max.y),
            ];
            for dj in -2..=2 {
                for di in -2..=2 {
                    let (ii, jj) = (i + di, j + dj);
                    if (di == 0 && dj == 0) || ii < 0 || jj < 0 || ii >= nx || jj >= nz {
                        continue;
                    }
                    let o = at(ii, jj);
                    let n = (o - s).normalize_or(Vec2::X);
                    let d = ((s + o) * 0.5).dot(n) - gap * 0.5;
                    poly = clip(&poly, n, d);
                    if poly.len() < 3 {
                        break;
                    }
                }
            }
            if poly.len() >= 3 {
                cells.push(poly);
            }
        }
    }
    cells
}

/// Split a convex polygon by a line through `p` along `dir`, leaving a crack `gap` wide.
pub fn split(poly: &[Vec2], p: Vec2, dir: Vec2, gap: f32) -> (Vec<Vec2>, Vec<Vec2>) {
    let n = Vec2::new(-dir.y, dir.x).normalize_or(Vec2::Y);
    let d = p.dot(n);
    (clip(poly, n, d - gap * 0.5), clip(poly, -n, -d - gap * 0.5))
}

/// Voussoirs of a round arch in the XY plane (springing at y = 0, center at the origin):
/// `count` wedge stones from radius `r_in` to `r_out`, `depth` thick along Z, joints `gap`
/// wide. The middle stone (the keystone, when `count` is odd) rises `key_rise` higher and
/// stands `key_out` proud of the faces. Returns each stone with its angle (radians).
pub fn voussoirs(
    r_in: f32,
    r_out: f32,
    depth: f32,
    count: u32,
    gap: f32,
    key: (f32, f32),
    bevel: f32,
) -> Vec<(Mesh, f32)> {
    let (key_rise, key_out) = key;
    let step = std::f32::consts::PI / count as f32;
    (0..count)
        .map(|i| {
            let a0 = i as f32 * step;
            let a1 = a0 + step;
            let keystone = count % 2 == 1 && i == count / 2;
            let ro = if keystone { r_out + key_rise } else { r_out };
            let gi = gap * 0.5 / r_in;
            let go = gap * 0.5 / ro;
            let at = |r: f32, a: f32| Vec2::new(a.cos() * r, a.sin() * r);
            let mut poly = vec![at(r_in, a0 + gi), at(ro, a0 + go)];
            if keystone {
                // A keystone widens upward with a flat top.
                let mid = (a0 + a1) * 0.5;
                let spread = step * 0.62;
                poly = vec![
                    at(r_in, a0 + gi),
                    at(ro, mid - spread),
                    at(ro, mid + spread),
                    at(r_in, a1 - gi),
                ];
            } else {
                poly.push(at(ro, a1 - go));
                poly.push(at(r_in, a1 - gi));
            }
            let c = centroid(&poly);
            let local: Vec<Vec2> = poly.iter().map(|p| *p - c).collect();
            let d = depth * 0.5 + if keystone { key_out } else { 0.0 };
            let m = Slab::new(Outline::new(local), -d, d, bevel).mesh();
            // Slab space (x, z) is the arch plane (x, y); its extrusion (y) runs along Z.
            let m = moved(
                m,
                Vec3::new(c.x, c.y, 0.0),
                Quat::from_rotation_x(-std::f32::consts::FRAC_PI_2),
            );
            (m, (a0 + a1) * 0.5)
        })
        .collect()
}

/// A rune: two to five straight strokes on a 3 × 4 lattice in a 1 × 1.4 cell (a stave and
/// branches, like old carved alphabets). Returns stroke endpoints.
pub fn rune(rng: &mut Rng) -> Vec<(Vec2, Vec2)> {
    let lattice = |i: i32, j: i32| Vec2::new(i as f32 * 0.5, j as f32 * (1.4 / 3.0));
    let mut strokes = Vec::new();
    // Most runes stand on a vertical stave.
    let stave = rng.range_i32(0, 3);
    if rng.chance(0.85) {
        strokes.push((lattice(stave, 0), lattice(stave, 3)));
    }
    let branches = rng.range_i32(1, 4);
    for _ in 0..branches {
        let j0 = rng.range_i32(0, 4);
        let side = if stave == 0 {
            1
        } else if stave == 2 {
            -1
        } else if rng.chance(0.5) {
            1
        } else {
            -1
        };
        let dj = rng.range_i32(-1, 2);
        let i1 = (stave + side * rng.range_i32(1, 3)).clamp(0, 2);
        let j1 = (j0 + dj).clamp(0, 3);
        if i1 == stave && j1 == j0 {
            continue;
        }
        strokes.push((lattice(stave, j0), lattice(i1, j1)));
    }
    if strokes.is_empty() {
        strokes.push((lattice(1, 0), lattice(1, 3)));
    }
    strokes
}

/// A sigil: a rune inside a ring with four ticks around it, `radius` meters to the ring,
/// centered on the origin (strokes for [`strokes_mesh`]).
pub fn sigil(rng: &mut Rng, radius: f32) -> Vec<(Vec2, Vec2)> {
    use std::f32::consts::{FRAC_PI_2, FRAC_PI_4, TAU};
    let n = 32;
    let mut strokes: Vec<(Vec2, Vec2)> = (0..n)
        .map(|i| {
            let a0 = i as f32 / n as f32 * TAU;
            let a1 = (i + 1) as f32 / n as f32 * TAU;
            (Vec2::from_angle(a0) * radius, Vec2::from_angle(a1) * radius)
        })
        .collect();
    for k in 0..4 {
        let d = Vec2::from_angle(k as f32 * FRAC_PI_2 + FRAC_PI_4);
        strokes.push((d * radius * 1.14, d * radius * 1.34));
    }
    let k = radius * 0.85;
    let mid = Vec2::new(0.5, 0.7);
    strokes.extend(
        rune(rng)
            .into_iter()
            .map(|(a, b)| ((a - mid) * k, (b - mid) * k)),
    );
    strokes
}

/// Flat stroke quads on the XY plane facing +Z, `width` wide (meters, in the units of the
/// strokes), ends extended so strokes join cleanly.
pub fn strokes_mesh(strokes: &[(Vec2, Vec2)], width: f32) -> Mesh {
    use engine_render::Vertex;
    let mut m = Mesh::default();
    for (a, b) in strokes {
        let d = (*b - *a).normalize_or(Vec2::Y);
        let n = Vec2::new(-d.y, d.x) * width * 0.5;
        let (a, b) = (*a - d * width * 0.5, *b + d * width * 0.5);
        let base = m.vertices.len() as u32;
        for p in [a - n, b - n, b + n, a + n] {
            m.vertices
                .push(Vertex::new(Vec3::new(p.x, p.y, 0.0), Vec3::Z, p));
        }
        // Counter-clockwise seen from +Z whichever way the stroke runs.
        let (p0, p1, p2) = (a - n, b - n, b + n);
        let ccw = (p1 - p0).perp_dot(p2 - p0) > 0.0;
        if ccw {
            m.indices
                .extend_from_slice(&[base, base + 1, base + 2, base, base + 2, base + 3]);
        } else {
            m.indices
                .extend_from_slice(&[base, base + 2, base + 1, base, base + 3, base + 2]);
        }
    }
    m
}

/// A line of `count` runes, each `size` meters tall, centered on the origin, on the XY plane
/// facing +Z (laid out along X, or along -Y when `vertical`).
pub fn rune_line(rng: &mut Rng, count: u32, size: f32, vertical: bool) -> Mesh {
    let cell = size / 1.4;
    let pitch = if vertical { size * 1.35 } else { cell * 1.6 };
    let total = pitch * (count.max(1) - 1) as f32;
    let mut out = Mesh::default();
    for i in 0..count {
        let strokes: Vec<(Vec2, Vec2)> = rune(rng)
            .into_iter()
            .map(|(a, b)| {
                let f = |p: Vec2| (p - Vec2::new(0.5, 0.7)) * cell;
                (f(a), f(b))
            })
            .collect();
        let mut g = strokes_mesh(&strokes, cell * 0.16);
        let off = i as f32 * pitch - total * 0.5;
        let t = if vertical {
            Vec3::new(0.0, -off, 0.0)
        } else {
            Vec3::new(off, 0.0, 0.0)
        };
        g = moved(g, t, Quat::IDENTITY);
        out.merge(&g);
    }
    out
}

/// Bend a flat strip laid along X (facing +Z) into a ring of `radius` on the ground at height
/// `y`, facing up: X becomes distance around the ring, Y the radial offset.
pub fn bend_to_ring(mesh: &mut Mesh, radius: f32, y: f32) {
    warp(mesh, |p| {
        let a = p.x / radius;
        let r = radius + p.y;
        Vec3::new(r * a.cos(), y + p.z, r * a.sin())
    });
}
