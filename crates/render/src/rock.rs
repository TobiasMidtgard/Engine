//! Procedural rocks: faceted, weathered boulders and stones, different for every seed.
//!
//! A real rock is a fractured block worn down by weather: flat break faces meeting at rounded
//! edges, pitted and layered surfaces, and a flat-ish base where it rests. The generator builds
//! that shape directly on a finely subdivided icosphere:
//!
//! 1. the intersection of 9–14 random half-spaces (the break faces), with the edges between
//!    them rounded by a soft minimum;
//! 2. erosion: a few octaves of gradient noise, ridged at the coarse scale, plus faint
//!    sedimentary layers along a random axis;
//! 3. a softly flattened base;
//! 4. cavity darkening baked into the vertex color (dirt and shadow in cracks).
//!
//! UVs are a box projection in meters by the dominant normal axis; materials that should flow
//! seamlessly over the rock use world-space (triplanar) mapping.

use std::collections::HashMap;

use engine_core::math::{Vec2, Vec3, Vec4};

use crate::mesh::{Mesh, Vertex};

/// Rock mesh `size` meters across (x, y, z), centered on the origin, resting on y = -size.y / 2.
/// `detail` is the icosphere subdivision level (1..=7, about 20·4^detail triangles); 0 picks
/// one from the size so facets stay well under a centimeter on boulders.
pub fn rock(size: Vec3, seed: u32, detail: u32) -> Mesh {
    let detail = if detail == 0 {
        let span = size.max_element();
        if span <= 0.3 {
            4
        } else if span <= 2.0 {
            5
        } else {
            6
        }
    } else {
        detail.clamp(1, 7)
    };
    let mut rng = Rng::new(seed);
    let (dirs, indices) = icosphere(detail);

    // Break faces: random planes, unit normal and distance from the center. Large faces cut
    // deep; small chips near the surface knock off edges and corners.
    let mut planes: Vec<(Vec3, f32)> = (0..rng.range(8.0, 12.99) as usize)
        .map(|_| (rng.unit_vector(), rng.range(0.55, 0.85)))
        .collect();
    planes.extend(
        (0..rng.range(6.0, 12.99) as usize).map(|_| (rng.unit_vector(), rng.range(0.8, 0.95))),
    );
    let strata = rng.unit_vector().lerp(Vec3::Y, 0.6).normalize();
    let strata_phase = rng.range(0.0, 100.0);
    let noise_offset = Vec3::new(
        rng.range(-500.0, 500.0),
        rng.range(-500.0, 500.0),
        rng.range(-500.0, 500.0),
    );
    // Relative scale of erosion detail: larger rocks show more fine structure per meter.
    let roughness = rng.range(0.8, 1.2);

    let mut vertices: Vec<Vertex> = dirs
        .iter()
        .map(|&d| {
            // 1. Convex break faces with rounded edges (soft minimum of the plane distances).
            let k = 0.012;
            let mut sum = 0.0;
            for &(n, offset) in &planes {
                let c = d.dot(n);
                if c > 1e-3 {
                    sum += (-(offset / c) / k).exp();
                }
            }
            let mut r = if sum > 0.0 {
                (-k * sum.ln()).min(1.0)
            } else {
                1.0
            };
            // Blend toward a sphere a little so no face is perfectly flat.
            r = r.clamp(0.35, 1.0) * 0.96 + 0.04;

            // 2. Erosion: ridged coarse noise, finer pitting, faint layers.
            let p = d * 2.2 + noise_offset;
            let coarse = 1.0 - (gradient_noise(p) * 2.0).abs();
            let mut fine = 0.0;
            let mut amp = 0.5;
            let mut q = p * 4.0;
            for _ in 0..4 {
                fine += gradient_noise(q) * amp;
                q = q * 2.13 + Vec3::new(3.1, 1.7, 5.3);
                amp *= 0.5;
            }
            let layers = (d.dot(strata) * 45.0 + strata_phase + gradient_noise(p * 1.5) * 4.0)
                .sin()
                * 0.0015;
            r += (coarse - 0.5) * 0.035 * roughness + fine * 0.03 * roughness + layers;

            let mut pos = d * r;
            // 3. A flattened base with a rounded rim (smooth maximum with the floor plane).
            let (floor, k) = (-0.7, 0.12);
            let h = (0.5 + 0.5 * (pos.y - floor) / k).clamp(0.0, 1.0);
            pos.y = floor + (pos.y - floor) * h + k * h * (1.0 - h);
            Vertex::new(pos, d, Vec2::ZERO)
        })
        .collect();

    // Fit the bounds to `size` exactly (the shape is irregular, so scale per axis).
    let (min, max) = vertices.iter().fold(
        (Vec3::splat(f32::MAX), Vec3::splat(f32::MIN)),
        |(lo, hi), v| {
            let p = Vec3::from(v.position);
            (lo.min(p), hi.max(p))
        },
    );
    let center = (min + max) * 0.5;
    let scale = size / (max - min).max(Vec3::splat(1e-4));
    for v in &mut vertices {
        v.position = ((Vec3::from(v.position) - center) * scale).to_array();
    }
    let mut mesh = Mesh::new(vertices, indices);
    mesh.compute_normals();

    // 4. Cavity: how far each vertex sits below its neighbors' average, along its normal.
    let mut neighbor_sum = vec![Vec3::ZERO; mesh.vertices.len()];
    let mut neighbor_count = vec![0.0f32; mesh.vertices.len()];
    for tri in mesh.indices.as_chunks::<3>().0 {
        for (a, b) in [(tri[0], tri[1]), (tri[1], tri[2]), (tri[2], tri[0])] {
            let (a, b) = (a as usize, b as usize);
            neighbor_sum[a] += Vec3::from(mesh.vertices[b].position);
            neighbor_count[a] += 1.0;
            neighbor_sum[b] += Vec3::from(mesh.vertices[a].position);
            neighbor_count[b] += 1.0;
        }
    }
    // About one edge length of the subdivided mesh.
    let edge = 0.5 * size.max_element() / 2f32.powi(detail as i32);
    for (i, v) in mesh.vertices.iter_mut().enumerate() {
        let p = Vec3::from(v.position);
        let n = Vec3::from(v.normal);
        let avg = neighbor_sum[i] / neighbor_count[i].max(1.0);
        let cavity = ((avg - p).dot(n) * 6.0 / edge).clamp(0.0, 1.0);
        let shade = 1.0 - 0.35 * cavity;
        v.color = Vec4::new(shade, shade, shade, 1.0).to_array();
        // Box projection in meters by the dominant normal axis.
        let a = n.abs();
        v.uv = if a.y >= a.x && a.y >= a.z {
            Vec2::new(p.x, p.z)
        } else if a.x >= a.z {
            Vec2::new(p.z, p.y)
        } else {
            Vec2::new(p.x, p.y)
        }
        .to_array();
    }
    mesh
}

/// Unit icosphere subdivided `level` times: vertex directions and counter-clockwise triangles.
fn icosphere(level: u32) -> (Vec<Vec3>, Vec<u32>) {
    let t = (1.0 + 5f32.sqrt()) / 2.0;
    let mut dirs: Vec<Vec3> = [
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
    let mut tris: Vec<[u32; 3]> = vec![
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
    for _ in 0..level {
        let mut midpoints: HashMap<(u32, u32), u32> = HashMap::new();
        let mut midpoint = |a: u32, b: u32, dirs: &mut Vec<Vec3>| {
            *midpoints.entry((a.min(b), a.max(b))).or_insert_with(|| {
                dirs.push((dirs[a as usize] + dirs[b as usize]).normalize());
                dirs.len() as u32 - 1
            })
        };
        let mut next = Vec::with_capacity(tris.len() * 4);
        for [a, b, c] in tris {
            let ab = midpoint(a, b, &mut dirs);
            let bc = midpoint(b, c, &mut dirs);
            let ca = midpoint(c, a, &mut dirs);
            next.extend_from_slice(&[[a, ab, ca], [b, bc, ab], [c, ca, bc], [ab, bc, ca]]);
        }
        tris = next;
    }
    (dirs, tris.into_iter().flatten().collect())
}

/// Small deterministic random numbers (PCG-style) so rocks are reproducible from their seed.
struct Rng(u64);

impl Rng {
    fn new(seed: u32) -> Self {
        Self((seed as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15) ^ 0x2545_F491_4F6C_DD1D)
    }

    fn next(&mut self) -> f32 {
        self.0 = self
            .0
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        ((self.0 >> 40) as f32) / (1u64 << 24) as f32
    }

    fn range(&mut self, lo: f32, hi: f32) -> f32 {
        lo + (hi - lo) * self.next()
    }

    fn unit_vector(&mut self) -> Vec3 {
        let z = self.range(-1.0, 1.0);
        let a = self.range(0.0, std::f32::consts::TAU);
        let r = (1.0 - z * z).max(0.0).sqrt();
        Vec3::new(r * a.cos(), z, r * a.sin())
    }
}

fn hash3(i: [i32; 3]) -> u32 {
    let mut h = (i[0] as u32).wrapping_mul(0x8DA6_B343)
        ^ (i[1] as u32).wrapping_mul(0xD816_3841)
        ^ (i[2] as u32).wrapping_mul(0xCB1A_B31F);
    h ^= h >> 13;
    h = h.wrapping_mul(0x5BD1_E995);
    h ^ (h >> 15)
}

/// Gradient (Perlin-style) noise in about -0.5..0.5.
fn gradient_noise(p: Vec3) -> f32 {
    let i = p.floor();
    let f = p - i;
    let u = f * f * f * (f * (f * 6.0 - 15.0) + 10.0);
    let grad = |c: [i32; 3], d: Vec3| {
        let h = hash3(c);
        let g = Vec3::new(
            (h & 0xFF) as f32 / 127.5 - 1.0,
            ((h >> 8) & 0xFF) as f32 / 127.5 - 1.0,
            ((h >> 16) & 0xFF) as f32 / 127.5 - 1.0,
        );
        g.dot(d)
    };
    let (x, y, z) = (i.x as i32, i.y as i32, i.z as i32);
    let mut corners = [0.0f32; 8];
    for (k, corner) in corners.iter_mut().enumerate() {
        let o = [(k & 1) as i32, ((k >> 1) & 1) as i32, ((k >> 2) & 1) as i32];
        *corner = grad(
            [x + o[0], y + o[1], z + o[2]],
            f - Vec3::new(o[0] as f32, o[1] as f32, o[2] as f32),
        );
    }
    let lerp = |a: f32, b: f32, t: f32| a + (b - a) * t;
    let x0 = lerp(corners[0], corners[1], u.x);
    let x1 = lerp(corners[2], corners[3], u.x);
    let x2 = lerp(corners[4], corners[5], u.x);
    let x3 = lerp(corners[6], corners[7], u.x);
    lerp(lerp(x0, x1, u.y), lerp(x2, x3, u.y), u.z) * 0.6
}

#[cfg(test)]
mod tests {
    use super::*;

    fn closed(m: &Mesh) -> bool {
        let mut edges: HashMap<(u32, u32), i32> = HashMap::new();
        for tri in m.indices.as_chunks::<3>().0 {
            for (a, b) in [(tri[0], tri[1]), (tri[1], tri[2]), (tri[2], tri[0])] {
                *edges.entry((a.min(b), a.max(b))).or_default() += 1;
            }
        }
        edges.values().all(|&n| n == 2)
    }

    #[test]
    fn rocks_fit_their_size_and_are_closed() {
        let size = Vec3::new(2.0, 1.2, 1.6);
        let m = rock(size, 7, 4);
        let b = m.aabb();
        assert!((b.max - b.min).abs_diff_eq(size, 1e-3), "{b:?}");
        assert!((b.min.y + 0.6).abs() < 1e-3, "rests on its base: {b:?}");
        assert!(closed(&m), "the rock has holes");
        assert!(m.vertices.iter().all(|v| {
            let n = Vec3::from(v.normal);
            n.is_finite() && (n.length() - 1.0).abs() < 1e-3
        }));
    }

    #[test]
    fn seeds_make_different_rocks_and_the_same_seed_the_same_rock() {
        let a = rock(Vec3::ONE, 1, 3);
        let b = rock(Vec3::ONE, 1, 3);
        let c = rock(Vec3::ONE, 2, 3);
        assert_eq!(a.vertices, b.vertices);
        let diff: f32 = a
            .vertices
            .iter()
            .zip(&c.vertices)
            .map(|(x, y)| Vec3::from(x.position).distance(Vec3::from(y.position)))
            .sum::<f32>()
            / a.vertices.len() as f32;
        assert!(diff > 0.02, "seeds 1 and 2 look alike: {diff}");
    }

    #[test]
    fn rocks_have_flat_faces_and_rough_detail() {
        // Break faces: many normals cluster around a few directions, unlike a noisy sphere.
        let m = rock(Vec3::ONE, 3, 5);
        let mut buckets: HashMap<(i32, i32, i32), usize> = HashMap::new();
        for v in &m.vertices {
            let n = Vec3::from(v.normal) * 4.0;
            *buckets
                .entry((n.x.round() as i32, n.y.round() as i32, n.z.round() as i32))
                .or_default() += 1;
        }
        let mut counts: Vec<usize> = buckets.into_values().collect();
        counts.sort_unstable_by(|a, b| b.cmp(a));
        let top: usize = counts.iter().take(12).sum();
        assert!(
            top as f32 > m.vertices.len() as f32 * 0.35,
            "no dominant faces: {top} of {}",
            m.vertices.len()
        );
    }
}
