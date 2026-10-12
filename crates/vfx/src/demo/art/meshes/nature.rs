//! Natural shapes: boulders and pebbles, leaves and bushes, moss and grass.

use engine_core::math::{Quat, Rng, Vec2, Vec3, smoothstep};
use engine_render::{Mesh, Vertex};

use super::{
    auto_smooth, cavity, double_faced, icosphere, noise1, noise3, paint, placed, tube, two_sided,
    uv_box,
};

/// How a boulder is shaped.
#[derive(Debug, Clone, Copy)]
pub struct RockShape {
    /// Overall size (x, y, z), meters.
    pub size: Vec3,
    pub seed: u64,
    /// Icosphere subdivisions (2: 320 triangles, 3: 1280, 4: 5120).
    pub detail: u32,
    /// Lumpiness, a fraction of the radius.
    pub lumps: f32,
    /// Flat fracture faces cut into it.
    pub cuts: u32,
    /// Where the bottom is cut flat, -1 (none) .. 1 (everything); sunk rocks hide it anyway.
    pub floor: f32,
}

impl RockShape {
    pub fn new(size: Vec3, seed: u64) -> Self {
        Self {
            size,
            seed,
            detail: 3,
            lumps: 0.12,
            cuts: 7,
            floor: -0.55,
        }
    }

    pub fn detail(mut self, detail: u32) -> Self {
        self.detail = detail;
        self
    }

    pub fn lumps(mut self, lumps: f32) -> Self {
        self.lumps = lumps;
        self
    }

    pub fn cuts(mut self, cuts: u32) -> Self {
        self.cuts = cuts;
        self
    }

    pub fn floor(mut self, floor: f32) -> Self {
        self.floor = floor;
        self
    }
}

/// A weathered boulder centered on the origin: a lumpy sphere cut by flat fracture faces,
/// darker in its cracks and undersides, lighter on worn edges and weathered tops, with faint
/// bedding layers. UVs in meters.
pub fn rock(shape: RockShape) -> Mesh {
    let mut rng = Rng::new(shape.seed);
    let mut m = icosphere(shape.detail);
    let planes: Vec<(Vec3, f32)> = (0..shape.cuts)
        .map(|_| {
            let mut n = rng.unit_vector();
            n.y *= 0.7;
            (n.normalize(), rng.range(0.5, 0.78))
        })
        .collect();
    let seed = shape.seed;
    for v in &mut m.vertices {
        let d = Vec3::from(v.position);
        let lump = noise1(d, 1.4, seed, 4) * shape.lumps + noise1(d, 4.0, seed ^ 7, 2) * 0.03;
        let mut p = d * (1.0 + lump);
        for (n, off) in &planes {
            let over = p.dot(*n) - off;
            if over > 0.0 {
                p -= *n * over;
            }
        }
        // Fine chipping so fracture faces aren't perfectly flat.
        p += noise3(p, 7.0, seed ^ 3, 2) * 0.02;
        p.y = p.y.max(shape.floor);
        v.position = (p * shape.size * 0.5).to_array();
    }
    // Crisp edges where fracture faces meet, smooth elsewhere.
    let mut m = auto_smooth(&m, 34.0);
    uv_box(&mut m, Vec2::new(rng.range(0.0, 9.0), rng.range(0.0, 9.0)));
    cavity(&mut m, 0.4, 0.35, 5.0);
    let size = shape.size;
    let tone = rng.range(0.92, 1.12);
    paint(&mut m, |p, n| {
        // Weathered (lighter) tops, damp darker undersides, bedding layers, color drift.
        let up = 0.9 + 0.18 * n.y.max(-0.4);
        let base = smoothstep(-size.y * 0.5, -size.y * 0.15, p.y) * 0.22 + 0.78;
        let bed = Vec3::new(p.x * 0.3, p.y * 9.0 / size.y.max(0.05), p.z * 0.3);
        let layers = noise1(bed, 1.0, seed ^ 5, 2);
        let drift = noise1(p, 2.5, seed ^ 11, 2) * 0.06;
        Vec3::new(1.0 + drift, 1.0, 1.0 - drift) * up * base * tone * (1.0 + 0.07 * layers)
    });
    m
}

/// Lichen and algae: soft greenish patches over the up-facing parts of a placed rock, about
/// `coverage` (0..1) of them.
pub fn lichen(mesh: &mut Mesh, coverage: f32, seed: u64) {
    let t = 1.0 - 2.0 * coverage;
    paint(mesh, |p, n| {
        let patch = smoothstep(t, t + 0.35, noise1(p, 3.5, seed, 3) * 1.6);
        let k = patch * smoothstep(0.1, 0.7, n.y);
        Vec3::ONE.lerp(Vec3::new(0.74, 0.9, 0.5), k)
    });
}

/// A small river pebble about `size` meters across, smooth and slightly flattened.
pub fn pebble(rng: &mut Rng, size: f32) -> Mesh {
    let seed = rng.next_u64();
    let mut m = icosphere(1);
    let squash = Vec3::new(1.0, rng.range(0.45, 0.7), rng.range(0.7, 1.0));
    for v in &mut m.vertices {
        let d = Vec3::from(v.position);
        let p = d * (1.0 + noise1(d, 1.2, seed, 2) * 0.15) * squash * size * 0.5;
        v.position = p.to_array();
    }
    m.compute_normals();
    let shade = rng.range(0.7, 1.15);
    let warm = rng.range(-0.06, 0.06);
    paint(&mut m, |_, _| {
        Vec3::new(1.0 + warm, 1.0, 1.0 - warm) * shade
    });
    uv_box(&mut m, Vec2::new(rng.range(0.0, 9.0), 0.0));
    m
}

/// A single leaf about `len` long and `width` wide, base at the origin pointing along +Z,
/// face up (+Y), folded along its midrib and curling down toward the tip. Two-sided.
pub fn leaf(len: f32, width: f32, fold: f32, curl: f32) -> Mesh {
    let w = width * 0.5;
    let p = |x: f32, t: f32, lift: f32| Vec3::new(x, lift - curl * t * t * len, t * len);
    let pts = [
        p(0.0, 0.0, 0.0),       // base
        p(-w, 0.42, -fold * w), // left edge
        p(0.0, 0.42, 0.0),      // midrib
        p(w, 0.42, -fold * w),  // right edge
        p(-w * 0.45, 0.78, -fold * w * 0.5),
        p(w * 0.45, 0.78, -fold * w * 0.5),
        p(0.0, 1.0, 0.0), // tip
    ];
    let tris: [[u32; 3]; 6] = [
        [0, 2, 1],
        [0, 3, 2],
        [1, 2, 4],
        [2, 5, 4],
        [2, 3, 5],
        [4, 5, 6],
    ];
    let mut m = Mesh::default();
    for q in pts {
        m.vertices.push(Vertex::new(
            q,
            Vec3::Y,
            Vec2::new(q.x / width + 0.5, q.z / len),
        ));
    }
    for t in tris {
        // Wind each triangle to face up.
        let (a, b, c) = (pts[t[0] as usize], pts[t[1] as usize], pts[t[2] as usize]);
        if (b - a).cross(c - a).y >= 0.0 {
            m.indices.extend_from_slice(&t);
        } else {
            m.indices.extend_from_slice(&[t[0], t[2], t[1]]);
        }
    }
    m.compute_normals();
    two_sided(m)
}

/// A leafy bush: woody stems (first) and leaf clusters (second), on the ground at the origin,
/// `radius` wide and `height` tall, with `leaves` leaves of about `leaf_len`.
pub fn bush(seed: u64, radius: f32, height: f32, leaves: u32, leaf_len: f32) -> (Mesh, Mesh) {
    let mut rng = Rng::new(seed);
    let mut wood = Mesh::default();
    let mut foliage = Mesh::default();
    let center = Vec3::new(0.0, height * 0.45, 0.0);
    // Canopy surface: a squashed, lumpy ellipsoid.
    let canopy = |dir: Vec3| {
        let lump = 1.0 + noise1(dir, 1.6, seed, 3) * 0.22;
        Vec3::new(dir.x * radius, dir.y * height * 0.55, dir.z * radius) * lump
    };
    // Twigs to hang leaf clusters on.
    let mut twigs: Vec<(Vec3, Vec3)> = Vec::new();
    let stems = rng.range_i32(10, 15);
    for s in 0..stems {
        let a = s as f32 / stems as f32 * std::f32::consts::TAU + rng.range(-0.3, 0.3);
        // Some stems arch out low so the bush is leafy down to the ground.
        let up = rng.range(-0.2, 0.95);
        let dir = Vec3::new(a.cos() * (1.0 - up * 0.6), up, a.sin() * (1.0 - up * 0.6)).normalize();
        let tip = center + canopy(dir) * rng.range(0.75, 0.95);
        let base = Vec3::new(rng.range(-0.06, 0.06), 0.0, rng.range(-0.06, 0.06));
        let bend = Vec3::new(0.0, height * 0.15, 0.0) + rng.unit_vector() * 0.08;
        let path: Vec<Vec3> = (0..=6)
            .map(|i| {
                let t = i as f32 / 6.0;
                base.lerp(tip, t) + bend * (t * (1.0 - t) * 4.0)
            })
            .collect();
        let r0 = rng.range(0.016, 0.026);
        wood.merge(&tube(&path, |t| r0 * (1.0 - t * 0.75), 5, false));
        // Side branches from the upper half.
        for k in 0..3 {
            let t = rng.range(0.45, 0.85);
            let i = (t * 6.0) as usize;
            let from = path[i];
            let out = (dir + rng.unit_vector() * 0.7).normalize();
            let to = center + canopy(out) * rng.range(0.7, 0.95);
            let mid = from.lerp(to, 0.5) + Vec3::Y * 0.05;
            wood.merge(&tube(
                &[from, mid, to],
                |t| r0 * 0.45 * (1.0 - t * 0.7),
                4,
                false,
            ));
            twigs.push((from, to));
            if k == 0 {
                twigs.push((path[3], tip));
            }
        }
        twigs.push((path[4], tip));
    }
    // Leaf clusters along the outer part of each twig.
    let per = (leaves as usize / twigs.len().max(1)).max(4);
    for (from, to) in &twigs {
        let hue = rng.range(-1.0, 1.0);
        for _ in 0..per {
            let t = rng.range(0.25, 1.0).sqrt();
            let mut at = from.lerp(*to, t) + rng.unit_vector() * leaf_len * 0.9;
            at.y = at.y.max(leaf_len * 0.6);
            let out = (at - center).normalize_or(Vec3::Y);
            let facing = (out + Vec3::Y * 0.9 + rng.unit_vector() * 0.8).normalize_or(Vec3::Y);
            let along = (out + rng.unit_vector() * 0.9).normalize_or(Vec3::X);
            let len = leaf_len * rng.range(0.7, 1.25);
            let l = leaf(
                len,
                len * rng.range(0.45, 0.6),
                rng.range(0.2, 0.5),
                rng.range(0.05, 0.3),
            );
            let rot = frame(along, facing);
            let mut l = placed(l, at, rot, Vec3::ONE);
            // Deeper leaves are shaded by the canopy; tips are young and lighter.
            let depth = ((at - center) / Vec3::new(radius, height * 0.55, radius)).length();
            let shade = 0.42 + 0.68 * smoothstep(0.35, 1.05, depth);
            let young = rng.range(0.0, 1.0) * smoothstep(0.8, 1.05, depth);
            let k = Vec3::new(
                1.0 + 0.1 * hue + 0.25 * young,
                1.0 + 0.12 * young,
                1.0 - 0.15 * hue,
            ) * shade
                * rng.range(0.85, 1.12);
            paint(&mut l, |_, _| k);
            foliage.merge(&l);
        }
    }
    (wood, foliage)
}

/// A rotation that turns +Z toward `along` and +Y toward `up` (as near as possible).
pub fn frame(along: Vec3, up: Vec3) -> Quat {
    let z = along.normalize_or(Vec3::Z);
    let x = up.cross(z).normalize_or(z.any_orthonormal_vector());
    let y = z.cross(x);
    Quat::from_mat3(&engine_core::math::Mat3::from_cols(x, y, z))
}

/// A cushion of moss about `size` across: a flattened lumpy blob, base at y = 0.
pub fn moss_cushion(rng: &mut Rng, size: f32) -> Mesh {
    let seed = rng.next_u64();
    let mut m = icosphere(1);
    let h = rng.range(0.25, 0.45);
    for v in &mut m.vertices {
        let d = Vec3::from(v.position);
        let mut p = d * (1.0 + noise1(d, 2.0, seed, 2) * 0.3);
        p.y = (p.y + 0.15).max(0.0) * h;
        v.position = (p * size * 0.5).to_array();
    }
    m.compute_normals();
    let shade = rng.range(0.75, 1.15);
    paint(&mut m, |p, _| {
        Vec3::new(1.0, 1.0, 0.9) * shade * (0.75 + 0.35 * (p.y / (size * 0.2)).min(1.0))
    });
    uv_box(&mut m, Vec2::new(rng.range(0.0, 5.0), 0.0));
    m
}

/// A tuft of `count` grass blades up to `height` tall, rooted at the origin. Both faces keep
/// up-facing normals, the usual trick for thin grass.
pub fn grass_tuft(rng: &mut Rng, count: u32, height: f32) -> Mesh {
    let mut m = Mesh::default();
    for _ in 0..count {
        let a = rng.range(0.0, std::f32::consts::TAU);
        let lean = rng.range(0.15, 0.6);
        let h = height * rng.range(0.5, 1.0);
        let w = h * rng.range(0.05, 0.09);
        let dir = Vec3::new(a.cos(), 0.0, a.sin());
        let side = Vec3::new(-a.sin(), 0.0, a.cos()) * w * 0.5;
        let root = dir * rng.range(0.0, height * 0.12);
        let pt =
            |t: f32| root + dir * (lean * h * t * t) + Vec3::Y * (h * t * (1.0 - 0.25 * lean * t));
        let base = m.vertices.len() as u32;
        let (p0, p1, p2) = (pt(0.0), pt(0.5), pt(1.0));
        let n = (Vec3::Y + dir * 0.35).normalize();
        let dark = rng.range(0.55, 0.8);
        let mk = |p: Vec3, t: f32, k: f32| {
            Vertex::new(p, n, Vec2::new(t, t * h)).with_color(engine_core::math::Vec4::new(
                k,
                k,
                k * 0.9,
                1.0,
            ))
        };
        m.vertices.extend_from_slice(&[
            mk(p0 - side, 0.0, dark),
            mk(p0 + side, 0.0, dark),
            mk(p1 - side * 0.7, 0.5, 0.9),
            mk(p1 + side * 0.7, 0.5, 0.9),
            mk(p2, 1.0, 1.1),
        ]);
        m.indices
            .extend([0, 1, 3, 0, 3, 2, 2, 3, 4].iter().map(|i| base + i));
    }
    double_faced(m)
}

/// Reeds: `count` long, narrow, arching blades up to `height`, rooted around the origin
/// within `spread` meters. Two-sided.
pub fn reeds(rng: &mut Rng, count: u32, height: f32, spread: f32) -> Mesh {
    let mut m = Mesh::default();
    for _ in 0..count {
        let root = rng.in_unit_disk() * spread;
        let root = Vec3::new(root.x, 0.0, root.y);
        let a = rng.range(0.0, std::f32::consts::TAU);
        let dir = Vec3::new(a.cos(), 0.0, a.sin());
        let h = height * rng.range(0.55, 1.0);
        let lean = rng.range(0.05, 0.35);
        let w = rng.range(0.012, 0.022);
        let side = Vec3::new(-a.sin(), 0.0, a.cos());
        let segs = 5;
        let base = m.vertices.len() as u32;
        for i in 0..=segs {
            let t = i as f32 / segs as f32;
            let c = root + dir * (lean * h * t * t) + Vec3::Y * (h * t);
            let ww = w * (1.0 - t * 0.9);
            let n = (dir.cross(side).cross(side) * -1.0).normalize_or(dir);
            let k = 0.55 + 0.5 * t;
            for s in [-1.0f32, 1.0] {
                m.vertices.push(
                    Vertex::new(c + side * (ww * s), n, Vec2::new(s * 0.5 + 0.5, t * h))
                        .with_color(engine_core::math::Vec4::new(k * 1.05, k, k * 0.75, 1.0)),
                );
            }
        }
        for i in 0..segs {
            let a = base + i * 2;
            m.indices
                .extend_from_slice(&[a, a + 1, a + 3, a, a + 3, a + 2]);
        }
    }
    m.compute_normals();
    two_sided(m)
}

/// A faceted crystal along +Y, centered on the origin: a `sides`-sided prism `length` long
/// with pointed ends, its facets a little uneven. Hard normals.
pub fn crystal(rng: &mut Rng, radius: f32, length: f32, sides: u32) -> Mesh {
    let h = length * 0.5;
    let tip = (length * 0.28).min(radius * 2.5);
    let ring = |rng: &mut Rng, y: f32, k: f32| -> Vec<Vec3> {
        (0..sides)
            .map(|i| {
                let a = (i as f32 + rng.range(-0.12, 0.12)) / sides as f32 * std::f32::consts::TAU;
                let r = radius * k * rng.range(0.88, 1.08);
                Vec3::new(
                    a.cos() * r,
                    y + rng.range(-0.02, 0.02) * length,
                    a.sin() * r,
                )
            })
            .collect()
    };
    let lower = ring(rng, -h + tip, 0.95);
    let upper = ring(rng, h - tip * 1.1, 1.0);
    let rows = vec![
        vec![Vec3::new(0.0, -h, 0.0); sides as usize],
        lower,
        upper,
        vec![Vec3::new(rng.range(-0.1, 0.1) * radius, h, 0.0); sides as usize],
    ];
    let m = super::grid(&rows, true);
    super::faceted(m)
}

/// Moss growing on the up-facing parts of `base` (a rock already placed): the triangles that
/// face up more than `up` (0..1) where noise allows, lifted `thickness` meters off it.
/// `coverage` (0..1) is roughly how much of the eligible area is mossy.
pub fn moss_cap(base: &Mesh, up: f32, coverage: f32, thickness: f32, seed: u64) -> Mesh {
    let mut out = Mesh::default();
    let threshold = 1.0 - 2.0 * coverage;
    let mut map = std::collections::HashMap::new();
    for t in base.indices.as_chunks::<3>().0 {
        let vs = t.map(|i| base.vertices[i as usize]);
        let ok = vs.iter().all(|v| {
            let p = Vec3::from(v.position);
            let n = Vec3::from(v.normal);
            n.y > up && noise1(p, 3.0, seed, 3) * 1.6 > threshold
        });
        if !ok {
            continue;
        }
        for i in t {
            let next = out.vertices.len() as u32;
            let idx = *map.entry(*i).or_insert_with(|| {
                let mut v = base.vertices[*i as usize];
                let p = Vec3::from(v.position) + Vec3::from(v.normal) * thickness;
                v.position = p.to_array();
                v.color = [1.0; 4];
                out.vertices.push(v);
                next
            });
            out.indices.push(idx);
        }
    }
    let k = 0.85;
    paint(&mut out, |p, _| {
        Vec3::splat(k + 0.25 * noise1(p, 6.0, seed ^ 5, 2))
    });
    out
}
