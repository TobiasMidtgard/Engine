//! The meshes of [`Debris`] pieces: a few faceted variants of each kind, built once and kept
//! on the GPU. Each piece is centered on its particle and about one unit long, so the
//! particle's size scales it to meters. Every variant has the same number of vertices (a flat
//! triangle list, unused triangles collapsed to a point) so one instanced draw covers a layer
//! whatever variant each particle picked.

use bytemuck::{Pod, Zeroable};
use engine_core::math::Vec3;

use crate::effect::Debris;

/// Variants of each kind (particles pick one by their seed).
pub const VARIANTS: usize = 4;
/// Vertices per variant: up to 20 flat-shaded triangles.
pub const VERTICES: usize = 60;

/// One vertex of a piece (32 bytes, `DebrisVertex` in `debris_common.wgsl`).
#[repr(C)]
#[derive(Debug, Clone, Copy, Default, PartialEq, Pod, Zeroable)]
pub struct DebrisVertex {
    /// Position in the piece's frame (about -0.5..0.5), w unused.
    pub position: [f32; 4],
    /// Unit face normal, pointing out of the piece; w unused.
    pub normal: [f32; 4],
}

/// Every variant of every kind, in GPU order: kind, then variant, then vertex.
pub fn pieces() -> Vec<DebrisVertex> {
    let mut out = Vec::with_capacity(Debris::ALL.len() * VARIANTS * VERTICES);
    for kind in Debris::ALL {
        for variant in 0..VARIANTS {
            let mut rng = Rng(0x9E37_79B9 ^ ((kind as u32) << 8) ^ variant as u32);
            let tris = match kind {
                // Rounded lumps and angular broken chunks, alternating.
                Debris::Rock if variant % 2 == 1 => chunk(&mut rng),
                Debris::Rock => rock(&mut rng),
                Debris::Splinter => splinter(&mut rng),
                Debris::Shard => shard(&mut rng),
                Debris::Scrap => scrap(&mut rng),
            };
            out.extend(flat_shaded(&tris));
        }
    }
    out
}

/// Small deterministic random numbers (xorshift), so the pieces are the same every run.
struct Rng(u32);

impl Rng {
    fn next(&mut self) -> f32 {
        let mut x = self.0.max(1);
        x ^= x << 13;
        x ^= x >> 17;
        x ^= x << 5;
        self.0 = x;
        (x >> 8) as f32 / (1u32 << 24) as f32
    }

    fn range(&mut self, lo: f32, hi: f32) -> f32 {
        lo + (hi - lo) * self.next()
    }
}

type Tri = [Vec3; 3];

/// Triangles to vertices with their face normals, padded with collapsed triangles. Triangles
/// of closed pieces are wound and their normals turned to face away from the piece's center.
fn flat_shaded(tris: &[(Tri, Option<Vec3>)]) -> Vec<DebrisVertex> {
    assert!(
        tris.len() * 3 <= VERTICES,
        "too many triangles in a debris piece"
    );
    let mut out = Vec::with_capacity(VERTICES);
    for &([a, b, c], facing) in tris {
        let mut n = (b - a).cross(c - a).normalize_or_zero();
        let out_dir = facing.unwrap_or((a + b + c) / 3.0);
        let (b, c) = if n.dot(out_dir) < 0.0 {
            n = -n;
            (c, b)
        } else {
            (b, c)
        };
        for p in [a, b, c] {
            out.push(DebrisVertex {
                position: p.extend(0.0).to_array(),
                normal: n.extend(0.0).to_array(),
            });
        }
    }
    out.resize(VERTICES, DebrisVertex::default());
    out
}

/// Scale and center the points so the piece is one unit long along its longest axis.
fn fit(points: &mut [Vec3]) {
    let (mut lo, mut hi) = (Vec3::splat(f32::MAX), Vec3::splat(f32::MIN));
    for p in points.iter() {
        lo = lo.min(*p);
        hi = hi.max(*p);
    }
    let center = (lo + hi) * 0.5;
    let k = 1.0 / (hi - lo).max_element().max(1e-6);
    for p in points.iter_mut() {
        *p = (*p - center) * k;
    }
}

/// A chipped lump: an icosahedron with its corners pushed in and out, squashed unevenly.
fn rock(rng: &mut Rng) -> Vec<(Tri, Option<Vec3>)> {
    let t = (1.0 + 5f32.sqrt()) / 2.0;
    let mut v: Vec<Vec3> = [
        (-1.0, t, 0.0),
        (1.0, t, 0.0),
        (-1.0, -t, 0.0),
        (1.0, -t, 0.0),
        (0.0, -1.0, t),
        (0.0, 1.0, t),
        (0.0, -1.0, -t),
        (0.0, 1.0, -t),
        (t, 0.0, -1.0),
        (t, 0.0, 1.0),
        (-t, 0.0, -1.0),
        (-t, 0.0, 1.0),
    ]
    .iter()
    .map(|&(x, y, z)| Vec3::new(x, y, z).normalize())
    .collect();
    let squash = Vec3::new(1.0, rng.range(0.55, 0.8), rng.range(0.7, 0.95));
    for p in &mut v {
        *p *= rng.range(0.72, 1.0) * squash;
    }
    fit(&mut v);
    const FACES: [[usize; 3]; 20] = [
        [0, 11, 5],
        [0, 5, 1],
        [0, 1, 7],
        [0, 7, 10],
        [0, 10, 11],
        [1, 5, 9],
        [5, 11, 4],
        [11, 10, 2],
        [10, 7, 6],
        [7, 1, 8],
        [3, 9, 4],
        [3, 4, 2],
        [3, 2, 6],
        [3, 6, 8],
        [3, 8, 9],
        [4, 9, 5],
        [2, 4, 11],
        [6, 2, 10],
        [8, 6, 7],
        [9, 8, 1],
    ];
    FACES
        .iter()
        .map(|f| ([v[f[0]], v[f[1]], v[f[2]]], None))
        .collect()
}

/// A broken chunk: a block with uneven, tapering faces, as stone splits along fracture planes.
fn chunk(rng: &mut Rng) -> Vec<(Tri, Option<Vec3>)> {
    let (h, d) = (rng.range(0.45, 0.7), rng.range(0.6, 0.9));
    let taper = rng.range(0.45, 0.9);
    let v: Vec<Vec3> = (0..8)
        .map(|i| {
            let (x, y, z) = (
                if i & 1 == 0 { -1.0 } else { 1.0 },
                if i & 2 == 0 { -1.0 } else { 1.0 },
                if i & 4 == 0 { -1.0 } else { 1.0 },
            );
            // The top narrower than the base.
            let k = if y > 0.0 { taper } else { 1.0 };
            Vec3::new(
                x * k + rng.range(-0.2, 0.2),
                y * h + rng.range(-0.15, 0.15),
                z * d * k + rng.range(-0.2, 0.2),
            )
        })
        .collect();
    let mut v = v;
    fit(&mut v);
    // Each side of the block as two triangles (corner bits: 1 = +x, 2 = +y, 4 = +z).
    const QUADS: [[usize; 4]; 6] = [
        [0, 1, 3, 2],
        [4, 5, 7, 6],
        [0, 1, 5, 4],
        [2, 3, 7, 6],
        [0, 2, 6, 4],
        [1, 3, 7, 5],
    ];
    QUADS
        .iter()
        .flat_map(|q| {
            [
                ([v[q[0]], v[q[1]], v[q[2]]], None),
                ([v[q[0]], v[q[2]], v[q[3]]], None),
            ]
        })
        .collect()
}

/// A sliver of wood: a thin four-sided stick, uneven along its length, splintered to a point
/// at each end.
fn splinter(rng: &mut Rng) -> Vec<(Tri, Option<Vec3>)> {
    let tip = |rng: &mut Rng, x: f32| Vec3::new(x, rng.range(-0.03, 0.03), rng.range(-0.04, 0.04));
    let ring = |rng: &mut Rng, x: f32, r: f32| -> [Vec3; 4] {
        let turn = rng.range(-0.3, 0.3);
        std::array::from_fn(|i| {
            let a = turn + i as f32 * std::f32::consts::FRAC_PI_2;
            // Wider than thick, like wood split along the grain.
            Vec3::new(
                x + rng.range(-0.04, 0.04),
                a.sin() * r * rng.range(0.5, 0.75),
                a.cos() * r * rng.range(0.9, 1.3),
            )
        })
    };
    let mut points = vec![tip(rng, -0.5)];
    let (x, r) = (rng.range(-0.3, -0.15), rng.range(0.06, 0.09));
    points.extend(ring(rng, x, r));
    let (x, r) = (rng.range(0.1, 0.25), rng.range(0.05, 0.08));
    points.extend(ring(rng, x, r));
    points.push(tip(rng, 0.5));
    fit(&mut points);
    let (a, b) = (1, 5);
    let mut tris = Vec::new();
    for i in 0..4 {
        let j = (i + 1) % 4;
        // Facing out of the stick's axis.
        let side = |p: Vec3| Some(Vec3::new(0.0, p.y, p.z));
        tris.push((
            [points[0], points[a + i], points[a + j]],
            side(points[a + i] + points[a + j]),
        ));
        tris.push((
            [points[a + i], points[b + i], points[b + j]],
            side(points[a + i] + points[b + j]),
        ));
        tris.push((
            [points[a + i], points[b + j], points[a + j]],
            side(points[a + i] + points[b + j]),
        ));
        tris.push((
            [points[9], points[b + j], points[b + i]],
            side(points[b + i] + points[b + j]),
        ));
    }
    tris
}

/// A flake of ice or glass: an uneven many-sided plate, thickest in the middle and sharp at
/// the rim.
fn shard(rng: &mut Rng) -> Vec<(Tri, Option<Vec3>)> {
    let corners = 6 + (rng.next() * 2.0) as usize;
    let start = rng.range(0.0, 1.0);
    let mut points: Vec<Vec3> = (0..corners)
        .map(|i| {
            let a = (i as f32 + start + rng.range(-0.25, 0.25)) / corners as f32
                * std::f32::consts::TAU;
            let r = rng.range(0.45, 1.0);
            // Stretched: shards are longer than wide.
            Vec3::new(a.cos() * r, rng.range(-0.02, 0.02), a.sin() * r * 0.6)
        })
        .collect();
    points.push(Vec3::new(
        rng.range(-0.15, 0.15),
        rng.range(0.05, 0.09),
        rng.range(-0.1, 0.1),
    ));
    points.push(Vec3::new(
        rng.range(-0.15, 0.15),
        -rng.range(0.04, 0.08),
        rng.range(-0.1, 0.1),
    ));
    fit(&mut points);
    let (top, bottom) = (points[corners], points[corners + 1]);
    let mut tris = Vec::new();
    for i in 0..corners {
        let (p, q) = (points[i], points[(i + 1) % corners]);
        let rim = (p + q) * 0.5;
        tris.push(([top, p, q], Some(rim + Vec3::Y * 0.5)));
        tris.push(([bottom, q, p], Some(rim - Vec3::Y * 0.5)));
    }
    tris
}

/// A torn scrap of sheet metal: a crumpled plate bent across the middle, with a top and a
/// bottom face a little apart.
fn scrap(rng: &mut Rng) -> Vec<(Tri, Option<Vec3>)> {
    let bend = rng.range(0.15, 0.35);
    let mut grid: Vec<Vec3> = Vec::with_capacity(9);
    for j in 0..3 {
        for i in 0..3 {
            let x = (i as f32 - 1.0) * 0.5 + rng.range(-0.08, 0.08);
            let z = (j as f32 - 1.0) * 0.3 + rng.range(-0.06, 0.06);
            let y = bend * x.abs() + rng.range(-0.04, 0.04);
            grid.push(Vec3::new(x, y, z));
        }
    }
    fit(&mut grid);
    let thick = Vec3::Y * 0.02;
    let mut tris = Vec::new();
    for j in 0..2 {
        for i in 0..2 {
            let k = j * 3 + i;
            for [a, b, c] in [[k, k + 1, k + 4], [k, k + 4, k + 3]] {
                let (pa, pb, pc) = (grid[a], grid[b], grid[c]);
                tris.push(([pa + thick, pb + thick, pc + thick], Some(Vec3::Y)));
                tris.push(([pa - thick, pb - thick, pc - thick], Some(Vec3::NEG_Y)));
            }
        }
    }
    tris
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_variant_fills_its_slot_and_fits_a_unit() {
        let all = pieces();
        assert_eq!(all.len(), Debris::ALL.len() * VARIANTS * VERTICES);
        for (k, piece) in all.chunks(VERTICES).enumerate() {
            let used: Vec<_> = piece
                .chunks(3)
                .filter(|t| t[0].normal != [0.0; 4])
                .collect();
            assert!(used.len() >= 12, "piece {k} has {} triangles", used.len());
            let mut lo = Vec3::splat(f32::MAX);
            let mut hi = Vec3::splat(f32::MIN);
            for t in &used {
                let n = Vec3::from_slice(&t[0].normal[..3]);
                assert!((n.length() - 1.0).abs() < 1e-3, "piece {k}: normal {n}");
                let p: Vec<Vec3> = t
                    .iter()
                    .map(|v| Vec3::from_slice(&v.position[..3]))
                    .collect();
                // Wound so the normal is the triangle's own (counter-clockwise from outside).
                let face = (p[1] - p[0]).cross(p[2] - p[0]);
                assert!(face.dot(n) > 0.0, "piece {k}: winding");
                for q in p {
                    lo = lo.min(q);
                    hi = hi.max(q);
                }
            }
            let extent = (hi - lo).max_element();
            assert!((0.9..=1.1).contains(&extent), "piece {k} is {extent} long");
            assert!(
                lo.max_element() < 0.0 && hi.min_element() > 0.0,
                "piece {k} off center"
            );
        }
    }

    #[test]
    fn closed_pieces_face_outward() {
        let all = pieces();
        let per_kind = VARIANTS * VERTICES;
        for kind in [Debris::Rock, Debris::Shard] {
            let start = kind as usize * per_kind;
            for t in all[start..start + per_kind].chunks(3) {
                if t[0].normal == [0.0; 4] {
                    continue;
                }
                let center: Vec3 = t
                    .iter()
                    .map(|v| Vec3::from_slice(&v.position[..3]))
                    .sum::<Vec3>()
                    / 3.0;
                let n = Vec3::from_slice(&t[0].normal[..3]);
                assert!(n.dot(center) > -1e-3, "{kind:?}: a face points inward");
            }
        }
    }

    #[test]
    fn variants_differ() {
        let all = pieces();
        assert_ne!(all[..VERTICES], all[VERTICES..2 * VERTICES]);
    }
}
