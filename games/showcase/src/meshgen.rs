//! Procedural meshes for the showcase scenery: grass and flower patches (the
//! village's buildings are in `village.rs`, rocks and the cliff in `rocks.rs`). Everything is
//! built from code (no asset files) with vertex colors, so a few shared materials draw the whole
//! valley in a handful of instanced batches.

use engine::core::math::smoothstep;
use engine::prelude::*;
use engine::render::Vertex;

/// Mesh under construction with helpers for boxes, tubes and blobs.
#[derive(Default)]
pub struct Builder {
    pub mesh: Mesh,
}

fn v(p: Vec3, n: Vec3, c: Vec3) -> Vertex {
    Vertex::new(p, n, Vec2::new(p.x + p.z, p.y)).with_color(c.extend(1.0))
}

impl Builder {
    pub fn finish(self) -> Mesh {
        self.mesh
    }

    fn base(&self) -> u32 {
        self.mesh.vertices.len() as u32
    }

    /// Flat-shaded quad (corners counter-clockwise seen from the front).
    pub fn quad(&mut self, p: [Vec3; 4], c: Vec3) {
        let n = (p[1] - p[0]).cross(p[2] - p[0]).normalize_or(Vec3::Y);
        let b = self.base();
        for q in p {
            self.mesh.vertices.push(v(q, n, c));
        }
        self.mesh
            .indices
            .extend_from_slice(&[b, b + 1, b + 2, b, b + 2, b + 3]);
    }

    /// Flat-shaded triangle (counter-clockwise seen from the front).
    pub fn tri(&mut self, p: [Vec3; 3], c: Vec3) {
        let n = (p[1] - p[0]).cross(p[2] - p[0]).normalize_or(Vec3::Y);
        let b = self.base();
        for q in p {
            self.mesh.vertices.push(v(q, n, c));
        }
        self.mesh.indices.extend_from_slice(&[b, b + 1, b + 2]);
    }

    /// Smooth tapered tube from `a` (radius `ra`) to `b` (radius `rb`), colors per end.
    #[allow(clippy::too_many_arguments)]
    pub fn tube(
        &mut self,
        a: Vec3,
        b: Vec3,
        ra: f32,
        rb: f32,
        sides: u32,
        ca: Vec3,
        cb: Vec3,
        cap: bool,
    ) {
        let axis = (b - a).normalize_or(Vec3::Y);
        let side = if axis.y.abs() < 0.95 {
            Vec3::Y
        } else {
            Vec3::X
        };
        let u = axis.cross(side).normalize();
        let w = axis.cross(u);
        let base = self.base();
        let slope = (ra - rb) / (b - a).length().max(1e-4);
        for i in 0..=sides {
            let t = i as f32 / sides as f32 * TAU;
            let dir = u * t.cos() + w * t.sin();
            let n = (dir + axis * slope).normalize();
            let uv_x = i as f32 / sides as f32;
            self.mesh.vertices.push(
                Vertex::new(a + dir * ra, n, Vec2::new(uv_x, 0.0)).with_color(ca.extend(1.0)),
            );
            self.mesh.vertices.push(
                Vertex::new(b + dir * rb, n, Vec2::new(uv_x, 1.0)).with_color(cb.extend(1.0)),
            );
        }
        for i in 0..sides {
            let k = base + i * 2;
            self.mesh
                .indices
                .extend_from_slice(&[k, k + 2, k + 1, k + 1, k + 2, k + 3]);
        }
        if cap && rb > 0.0 {
            let center = self.base();
            self.mesh.vertices.push(v(b, axis, cb));
            for i in 0..=sides {
                let t = i as f32 / sides as f32 * TAU;
                self.mesh
                    .vertices
                    .push(v(b + (u * t.cos() + w * t.sin()) * rb, axis, cb));
            }
            for i in 0..sides {
                self.mesh
                    .indices
                    .extend_from_slice(&[center, center + 1 + i, center + 2 + i]);
            }
        }
    }
}

/// Unit icosphere with shared vertices.
pub fn icosphere(subdivisions: u32) -> (Vec<Vec3>, Vec<u32>) {
    let t = (1.0 + 5.0f32.sqrt()) / 2.0;
    let mut verts: Vec<Vec3> = [
        [-1.0, t, 0.0],
        [1.0, t, 0.0],
        [-1.0, -t, 0.0],
        [1.0, -t, 0.0],
        [0.0, -1.0, t],
        [0.0, 1.0, t],
        [0.0, -1.0, -t],
        [0.0, 1.0, -t],
        [t, 0.0, -1.0],
        [t, 0.0, 1.0],
        [-t, 0.0, -1.0],
        [-t, 0.0, 1.0],
    ]
    .iter()
    .map(|p| Vec3::from(*p).normalize())
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
    for _ in 0..subdivisions {
        let mut cache = std::collections::HashMap::new();
        let mut mid = |a: u32, b: u32, verts: &mut Vec<Vec3>| -> u32 {
            let key = (a.min(b), a.max(b));
            *cache.entry(key).or_insert_with(|| {
                verts.push(((verts[a as usize] + verts[b as usize]) * 0.5).normalize());
                verts.len() as u32 - 1
            })
        };
        let mut next = Vec::with_capacity(tris.len() * 4);
        for [a, b, c] in tris {
            let ab = mid(a, b, &mut verts);
            let bc = mid(b, c, &mut verts);
            let ca = mid(c, a, &mut verts);
            next.extend_from_slice(&[[a, ab, ca], [b, bc, ab], [c, ca, bc], [ab, bc, ca]]);
        }
        tris = next;
    }
    (verts, tris.into_iter().flatten().collect())
}

/// Kinds of tree in the valley (drawn as Foliage's scenery trees, see `scenery::spawn_tree`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TreeKind {
    Oak,
    Spruce,
    Birch,
    Bush,
}

/// A patch of grass `size` meters across: `blades` blades grown in clumps, each a tapered
/// strip of `segments` segments (1 = a single triangle for distant patches) that bends over
/// along its length. `dry` (0–1) shifts the color from lush green toward straw.
pub fn grass_patch(seed: u64, size: f32, blades: u32, dry: f32, segments: u32) -> Mesh {
    let mut rng = Rng::new(seed);
    let mut mesh = Mesh::default();
    let segments = segments.max(1);
    let lush = Vec3::new(0.05, 0.13, 0.025);
    let fresh = Vec3::new(0.09, 0.2, 0.035);
    let straw = Vec3::new(0.22, 0.19, 0.08);
    let mut clump = (Vec3::ZERO, 0.0f32, 0.0f32, 1.0f32);
    for b in 0..blades {
        // A new clump every eight blades: blades share a center, a lean and a height.
        if b % 8 == 0 {
            clump = (
                Vec3::new(
                    rng.range(-0.5, 0.5) * size,
                    0.0,
                    rng.range(-0.5, 0.5) * size,
                ),
                rng.range(0.0, TAU),
                rng.range(0.15, 0.6),
                rng.range(0.7, 1.3),
            );
        }
        let (center, clump_dir, clump_lean, clump_h) = clump;
        let r = rng.range(0.0, 1.0).sqrt() * 0.28;
        let ra = rng.range(0.0, TAU);
        let mut base = center + Vec3::new(ra.cos() * r, 0.0, ra.sin() * r);
        base.x = base.x.clamp(-0.5 * size, 0.5 * size);
        base.z = base.z.clamp(-0.5 * size, 0.5 * size);
        let h = rng.range(0.22, 0.48) * clump_h * if rng.chance(0.06) { 1.5 } else { 1.0 };
        let half_w = rng.range(0.008, 0.016) * if segments == 1 { 2.2 } else { 1.0 };
        // Blades lean outward from the clump center and with the clump.
        let out = Vec3::new(ra.cos(), 0.0, ra.sin());
        let lean_dir = (Vec3::new(clump_dir.cos(), 0.0, clump_dir.sin()) * 0.6 + out * 0.4)
            .normalize_or(Vec3::X);
        let lean = (clump_lean + rng.range(-0.1, 0.25)).max(0.05);
        // The blade's flat side faces mostly across its lean, with some twist.
        let face = rng.range(-0.6, 0.6);
        let side = (Vec3::new(-lean_dir.z, 0.0, lean_dir.x) * face.cos() + lean_dir * face.sin())
            .normalize();
        let tint = rng.range(-0.25, 0.25);
        let dryness = (dry + tint).clamp(0.0, 1.0);
        let tip_col = fresh.lerp(straw, dryness) * rng.range(0.85, 1.25);
        let root_col = lush * 0.45;
        let point = |t: f32| -> Vec3 {
            // Rises, then bends over: the tip droops by `lean` of the height.
            let bend = lean * t * t;
            base + Vec3::Y * h * (t - 0.35 * bend * t) + lean_dir * h * bend
        };
        let i0 = mesh.vertices.len() as u32;
        for k in 0..=segments {
            let t = k as f32 / segments as f32;
            let p = point(t);
            let tangent = (point((t + 0.02).min(1.0)) - point((t - 0.02).max(0.0))).normalize();
            let blade_n = side.cross(tangent).normalize_or(Vec3::Y);
            // Lean shading normals toward the sky and round the edges, like a curved blade.
            let n = (blade_n * 0.55 + Vec3::Y * 0.6).normalize();
            let col = root_col.lerp(tip_col, smoothstep(0.0, 0.8, t)).extend(1.0);
            let v = t;
            if k == segments {
                mesh.vertices
                    .push(Vertex::new(p, n, Vec2::new(0.5, v)).with_color(col * 1.15));
            } else {
                let w = half_w * (1.0 - t.powf(1.6));
                let edge = side * 0.35;
                mesh.vertices.push(
                    Vertex::new(p - side * w, (n - edge).normalize(), Vec2::new(0.0, v))
                        .with_color(col),
                );
                mesh.vertices.push(
                    Vertex::new(p + side * w, (n + edge).normalize(), Vec2::new(1.0, v))
                        .with_color(col),
                );
            }
        }
        for k in 0..segments {
            let a = i0 + k * 2;
            if k + 1 == segments {
                mesh.indices.extend_from_slice(&[a, a + 1, a + 2]);
            } else {
                mesh.indices
                    .extend_from_slice(&[a, a + 1, a + 2, a + 2, a + 1, a + 3]);
            }
        }
    }
    mesh
}

/// Wildflowers: thin stems with small colored heads, scattered over a patch.
pub fn flower_patch(seed: u64, size: f32, count: u32, color: Vec3) -> Mesh {
    let mut rng = Rng::new(seed);
    let mut b = Builder::default();
    for _ in 0..count {
        let x = rng.range(-0.5, 0.5) * size;
        let z = rng.range(-0.5, 0.5) * size;
        let h = rng.range(0.25, 0.5);
        let stem = Vec3::new(0.05, 0.1, 0.03);
        b.tube(
            Vec3::new(x, 0.0, z),
            Vec3::new(x, h, z),
            0.008,
            0.006,
            3,
            stem,
            stem,
            false,
        );
        let c = color * rng.range(0.85, 1.15);
        let r = rng.range(0.03, 0.05);
        let p = Vec3::new(x, h, z);
        // A small flat star of petals.
        for k in 0..5 {
            let a = k as f32 / 5.0 * TAU + rng.range(0.0, 0.5);
            let d = Vec3::new(a.cos(), 0.15, a.sin());
            let e = Vec3::new((a + 0.6).cos(), 0.15, (a + 0.6).sin());
            b.tri([p, p + e * r * 1.8, p + d * r * 1.8], c);
        }
    }
    b.finish()
}

/// Tileable value noise in about [0, 1] on a lattice that repeats every `period` cells.
fn tile_noise(x: f32, y: f32, period: i64, seed: u64) -> f32 {
    use engine::core::math::hash01;
    let (xi, yi) = (x.floor() as i64, y.floor() as i64);
    let (fx, fy) = (x - xi as f32, y - yi as f32);
    let (sx, sy) = (fx * fx * (3.0 - 2.0 * fx), fy * fy * (3.0 - 2.0 * fy));
    let h = |i: i64, j: i64| hash01(i.rem_euclid(period), j.rem_euclid(period), 0, seed);
    let a = h(xi, yi) + (h(xi + 1, yi) - h(xi, yi)) * sx;
    let b = h(xi, yi + 1) + (h(xi + 1, yi + 1) - h(xi, yi + 1)) * sx;
    a + (b - a) * sy
}

/// Tileable fractal noise over `u, v` in [0, 1), about [0, 1].
fn tile_fbm(u: f32, v: f32, base: i64, octaves: u32, seed: u64) -> f32 {
    let (mut sum, mut amp, mut norm) = (0.0, 1.0, 0.0);
    for k in 0..octaves {
        let p = base << k;
        sum += amp * tile_noise(u * p as f32, v * p as f32, p, seed + k as u64);
        norm += amp;
        amp *= 0.55;
    }
    sum / norm
}

/// Ground detail textures that tile: a brightness map (multiplied with the terrain's vertex
/// colors) and a matching normal map of small bumps, clumps and pebbles.
pub fn ground_detail(size: u32) -> (Image, Image) {
    let n = size as usize;
    let mut height = vec![0.0f32; n * n];
    let mut albedo = Vec::with_capacity(n * n * 4);
    for y in 0..n {
        for x in 0..n {
            let (u, v) = (x as f32 / size as f32, y as f32 / size as f32);
            let clumps = tile_fbm(u, v, 8, 5, 11);
            let grain = tile_fbm(u, v, 64, 2, 12);
            let h = clumps * 0.7 + grain * 0.3;
            height[y * n + x] = h;
            let b = (0.62 + 0.42 * clumps + 0.12 * (grain - 0.5)).clamp(0.0, 1.0);
            let c = (b * 255.0) as u8;
            albedo.extend_from_slice(&[c, c, c, 255]);
        }
    }
    let mut normals = Vec::with_capacity(n * n * 4);
    let at = |x: isize, y: isize| {
        height[(y.rem_euclid(n as isize) as usize) * n + x.rem_euclid(n as isize) as usize]
    };
    for y in 0..n as isize {
        for x in 0..n as isize {
            let dx = (at(x + 1, y) - at(x - 1, y)) * 6.0;
            let dy = (at(x, y + 1) - at(x, y - 1)) * 6.0;
            let nrm = Vec3::new(-dx, dy, 1.0).normalize();
            let e = |c: f32| ((c * 0.5 + 0.5) * 255.0) as u8;
            normals.extend_from_slice(&[e(nrm.x), e(nrm.y), e(nrm.z), 255]);
        }
    }
    (
        Image::new(size, size, albedo, false),
        Image::new(size, size, normals, false),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn meshes_are_valid() {
        let check = |m: &Mesh, name: &str| {
            assert!(!m.indices.is_empty(), "{name} is empty");
            assert!(
                m.indices.iter().all(|&i| (i as usize) < m.vertices.len()),
                "{name} has bad indices"
            );
            assert!(
                m.vertices
                    .iter()
                    .all(|v| v.position.iter().all(|c| c.is_finite())),
                "{name} has NaNs"
            );
        };
        check(&grass_patch(1, 4.0, 300, 0.3, 3), "grass");
        check(&grass_patch(1, 4.0, 300, 0.3, 1), "far grass");
        check(&flower_patch(1, 4.0, 20, Vec3::X), "flowers");
    }
}
