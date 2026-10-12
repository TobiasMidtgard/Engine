//! Procedural generators for the trail's grounds and props: hashing and tileable noise
//! (value, gradient, fractal, Worley), texture sets baked from a painted height field
//! (albedo, tangent-space normal, occlusion/roughness/metal), and mesh builders (displaced
//! rocks, beveled boxes, tubes, grass blades, leaf cards).
//!
//! Everything is deterministic: the same seed gives the same texture or mesh, so results
//! can be cached by name.

use engine_core::math::{Quat, Vec2, Vec3, Vec4};
use engine_render::{Image, Mesh, Vertex};

// ------------------------------------------------------------------------------- random

/// Integer hash of a lattice point (well mixed, cheap).
#[inline]
pub fn hash(x: i32, y: i32, seed: u32) -> u32 {
    let mut h = seed.wrapping_mul(0x9e37_79b9) ^ 0x85eb_ca6b;
    h ^= (x as u32).wrapping_mul(0xcc9e_2d51);
    h = h.rotate_left(15).wrapping_mul(0x1b87_3593);
    h ^= (y as u32).wrapping_mul(0x27d4_eb2f);
    h = h.rotate_left(13).wrapping_mul(5).wrapping_add(0xe654_6b64);
    h ^= h >> 16;
    h = h.wrapping_mul(0x85eb_ca6b);
    h ^= h >> 13;
    h = h.wrapping_mul(0xc2b2_ae35);
    h ^ (h >> 16)
}

/// Hash of a 3D lattice point.
#[inline]
pub fn hash3(x: i32, y: i32, z: i32, seed: u32) -> u32 {
    hash(
        x,
        y ^ z.wrapping_mul(0x2545_f491),
        seed ^ (z as u32).wrapping_mul(0x9e37),
    )
}

/// Uniform in [0, 1) from a hash.
#[inline]
pub fn unit(h: u32) -> f32 {
    (h >> 8) as f32 * (1.0 / 16_777_216.0)
}

/// A small deterministic random source (xorshift).
#[derive(Clone)]
pub struct Rand(u64);

impl Rand {
    pub fn new(seed: u64) -> Self {
        Self(seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1)
    }

    /// Uniform in [0, 1).
    pub fn f(&mut self) -> f32 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        (self.0 >> 40) as f32 / (1u64 << 24) as f32
    }

    /// Uniform in [a, b).
    pub fn range(&mut self, a: f32, b: f32) -> f32 {
        a + (b - a) * self.f()
    }

    /// Roughly normal (sum of three uniforms), mean 0, deviation about `sd`.
    pub fn normal(&mut self, sd: f32) -> f32 {
        (self.f() + self.f() + self.f() - 1.5) * 2.0 * sd
    }

    /// An index below `n`.
    pub fn below(&mut self, n: usize) -> usize {
        ((self.f() * n as f32) as usize).min(n.saturating_sub(1))
    }

    /// True with probability `p`.
    pub fn chance(&mut self, p: f32) -> bool {
        self.f() < p
    }

    pub fn seed(&mut self) -> u32 {
        self.f();
        (self.0 >> 20) as u32
    }
}

// -------------------------------------------------------------------------------- noise

#[inline]
fn quintic(t: f32) -> f32 {
    t * t * t * (t * (t * 6.0 - 15.0) + 10.0)
}

#[inline]
pub fn lerp(a: f32, b: f32, t: f32) -> f32 {
    a + (b - a) * t
}

/// Hermite step from 0 at `a` to 1 at `b`.
#[inline]
pub fn smooth(a: f32, b: f32, x: f32) -> f32 {
    let t = ((x - a) / (b - a)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// A period that never repeats in practice, for world-space noise.
pub const NO_WRAP: i32 = 1 << 20;

const GRADS: [(f32, f32); 16] = [
    (1.0, 0.0),
    (0.924, 0.383),
    (0.707, 0.707),
    (0.383, 0.924),
    (0.0, 1.0),
    (-0.383, 0.924),
    (-0.707, 0.707),
    (-0.924, 0.383),
    (-1.0, 0.0),
    (-0.924, -0.383),
    (-0.707, -0.707),
    (-0.383, -0.924),
    (0.0, -1.0),
    (0.383, -0.924),
    (0.707, -0.707),
    (0.924, -0.383),
];

/// Gradient (Perlin) noise in about [-1, 1], repeating every `period` cells.
pub fn perlin(x: f32, y: f32, period: i32, seed: u32) -> f32 {
    perlin_p(x, y, period, period, seed)
}

/// Gradient noise repeating every `px` cells in x and `py` cells in y.
pub fn perlin_p(x: f32, y: f32, px: i32, py: i32, seed: u32) -> f32 {
    let (fx, fy) = (x.floor(), y.floor());
    let (rx, ry) = (x - fx, y - fy);
    let (ix, iy) = (fx as i32, fy as i32);
    let g = |dx: i32, dy: i32| {
        let h = hash((ix + dx).rem_euclid(px), (iy + dy).rem_euclid(py), seed);
        let (gx, gy) = GRADS[(h >> 7) as usize & 15];
        gx * (rx - dx as f32) + gy * (ry - dy as f32)
    };
    let (tx, ty) = (quintic(rx), quintic(ry));
    1.45 * lerp(lerp(g(0, 0), g(1, 0), tx), lerp(g(0, 1), g(1, 1), tx), ty)
}

/// Stretched fractal noise over a tile: `fu` cells across u and `fv` across v for the first
/// octave (whole numbers tile exactly). About [-1, 1].
pub fn fbm2(u: f32, v: f32, fu: i32, fv: i32, octaves: u32, seed: u32) -> f32 {
    let mut sum = 0.0;
    let mut amp = 1.0;
    let mut norm = 0.0;
    let (mut a, mut b) = (fu.max(1), fv.max(1));
    for o in 0..octaves {
        sum += amp
            * perlin_p(
                u * a as f32,
                v * b as f32,
                a,
                b,
                seed.wrapping_add(o * 7919),
            );
        norm += amp;
        amp *= 0.5;
        a *= 2;
        b *= 2;
    }
    sum / norm
}

/// Fractal gradient noise over a tile: `(u, v)` in [0, 1) repeat exactly when `freq` is a
/// whole number. `freq` cells across the tile for the first octave, doubling each octave
/// with half the amplitude. About [-1, 1].
pub fn fbm(u: f32, v: f32, freq: i32, octaves: u32, seed: u32) -> f32 {
    let mut sum = 0.0;
    let mut amp = 1.0;
    let mut norm = 0.0;
    let mut f = freq.max(1);
    for o in 0..octaves {
        sum += amp * perlin(u * f as f32, v * f as f32, f, seed.wrapping_add(o * 7919));
        norm += amp;
        amp *= 0.5;
        f *= 2;
    }
    sum / norm
}

/// Fractal noise in world space (meters), not tiling: `scale` is the size of the first
/// octave's features. About [-1, 1].
pub fn fbm_world(x: f32, z: f32, scale: f32, octaves: u32, seed: u32) -> f32 {
    let mut sum = 0.0;
    let mut amp = 1.0;
    let mut norm = 0.0;
    let mut f = 1.0 / scale;
    for o in 0..octaves {
        sum += amp * perlin(x * f, z * f, NO_WRAP, seed.wrapping_add(o * 7919));
        norm += amp;
        amp *= 0.5;
        f *= 2.0;
    }
    sum / norm
}

/// One Worley (cellular) sample: distances to the nearest and second nearest feature points
/// (in cells), the nearest cell's hash, and the offset from the sample to that point.
#[derive(Clone, Copy, Debug)]
pub struct Cell {
    pub f1: f32,
    pub f2: f32,
    pub id: u32,
    pub to: Vec2,
}

/// Worley noise at `(x, y)` cells, repeating every `period` cells; `jitter` (0..1) is how
/// far feature points wander from the cell centres.
pub fn worley(x: f32, y: f32, period: i32, seed: u32, jitter: f32) -> Cell {
    let (fx, fy) = (x.floor(), y.floor());
    let (ix, iy) = (fx as i32, fy as i32);
    let mut best = Cell {
        f1: 9.0,
        f2: 9.0,
        id: 0,
        to: Vec2::ZERO,
    };
    for dy in -1..=1 {
        for dx in -1..=1 {
            let h = hash(
                (ix + dx).rem_euclid(period),
                (iy + dy).rem_euclid(period),
                seed,
            );
            let px = fx + dx as f32 + 0.5 + (unit(h) - 0.5) * jitter;
            let py = fy + dy as f32 + 0.5 + (unit(h.rotate_left(16) ^ 0x5bd1_e995) - 0.5) * jitter;
            let to = Vec2::new(px - x, py - y);
            let d = to.length();
            if d < best.f1 {
                best.f2 = best.f1;
                best.f1 = d;
                best.id = h;
                best.to = to;
            } else if d < best.f2 {
                best.f2 = d;
            }
        }
    }
    best
}

/// 3D value noise in [-1, 1] (not tiling), for rocks and other solid shapes.
pub fn value3(p: Vec3, seed: u32) -> f32 {
    let f = p.floor();
    let t = p - f;
    let (tx, ty, tz) = (quintic(t.x), quintic(t.y), quintic(t.z));
    let (ix, iy, iz) = (f.x as i32, f.y as i32, f.z as i32);
    let c = |dx: i32, dy: i32, dz: i32| unit(hash3(ix + dx, iy + dy, iz + dz, seed)) * 2.0 - 1.0;
    let a = lerp(
        lerp(c(0, 0, 0), c(1, 0, 0), tx),
        lerp(c(0, 1, 0), c(1, 1, 0), tx),
        ty,
    );
    let b = lerp(
        lerp(c(0, 0, 1), c(1, 0, 1), tx),
        lerp(c(0, 1, 1), c(1, 1, 1), tx),
        ty,
    );
    lerp(a, b, tz)
}

/// Fractal 3D value noise, about [-1, 1].
pub fn fbm3(p: Vec3, octaves: u32, seed: u32) -> f32 {
    let mut sum = 0.0;
    let mut amp = 1.0;
    let mut norm = 0.0;
    let mut q = p;
    for o in 0..octaves {
        sum += amp * value3(q, seed.wrapping_add(o * 131));
        norm += amp;
        amp *= 0.5;
        q *= 2.03;
    }
    sum / norm
}

// ----------------------------------------------------------------------------- colours

/// sRGB hex colour to linear RGB.
pub fn hex(rgb: u32) -> Vec3 {
    let c = engine_render::material::srgb_hex_to_linear(rgb);
    Vec3::new(c[0], c[1], c[2])
}

fn linear_to_srgb8(v: f32) -> u8 {
    static LUT: std::sync::OnceLock<Vec<u8>> = std::sync::OnceLock::new();
    let lut = LUT.get_or_init(|| {
        (0..4096)
            .map(|i| {
                let l = i as f32 / 4095.0;
                let s = if l <= 0.003_130_8 {
                    l * 12.92
                } else {
                    1.055 * l.powf(1.0 / 2.4) - 0.055
                };
                (s * 255.0 + 0.5).clamp(0.0, 255.0) as u8
            })
            .collect()
    });
    lut[(v.clamp(0.0, 1.0) * 4095.0 + 0.5) as usize]
}

fn byte(v: f32) -> u8 {
    (v.clamp(0.0, 1.0) * 255.0 + 0.5) as u8
}

// ---------------------------------------------------------------------------- textures

/// One texel of a material as a recipe paints it.
#[derive(Clone, Copy, Debug)]
pub struct Texel {
    /// Height above the tile's mean, meters (becomes the normal map and cavity occlusion).
    pub height: f32,
    /// Linear RGB albedo.
    pub color: Vec3,
    /// Coverage for alpha-masked materials (the trail's grounds use 0.5..1 as the ragged edge
    /// pattern, see the ribbon builder).
    pub alpha: f32,
    pub rough: f32,
    pub metal: f32,
    /// Extra baked occlusion multiplied with the cavity term.
    pub ao: f32,
}

impl Default for Texel {
    fn default() -> Self {
        Self {
            height: 0.0,
            color: Vec3::splat(0.5),
            alpha: 1.0,
            rough: 0.8,
            metal: 0.0,
            ao: 1.0,
        }
    }
}

/// The images of one material: sRGB albedo (alpha = coverage), tangent-space normal, and
/// occlusion (R) / roughness (G) / metal (B).
pub struct TexSet {
    pub albedo: Image,
    pub normal: Image,
    pub orm: Image,
}

/// A tileable texture being painted: a grid of [`Texel`]s covering `tile` meters.
pub struct Canvas {
    pub w: u32,
    pub h: u32,
    /// Meters the texture covers in u and v (pixels should be square in meters).
    pub tile: Vec2,
    pub px: Vec<Texel>,
}

impl Canvas {
    pub fn new(w: u32, h: u32, tile: Vec2) -> Self {
        Self {
            w,
            h,
            tile,
            px: vec![Texel::default(); (w * h) as usize],
        }
    }

    /// Meters per pixel.
    pub fn texel_m(&self) -> f32 {
        self.tile.x / self.w as f32
    }

    /// Paint every texel from its `(u, v)` in [0, 1).
    pub fn fill(&mut self, f: impl Fn(f32, f32) -> Texel) {
        let (w, h) = (self.w, self.h);
        for y in 0..h {
            let v = (y as f32 + 0.5) / h as f32;
            for x in 0..w {
                let u = (x as f32 + 0.5) / w as f32;
                self.px[(y * w + x) as usize] = f(u, v);
            }
        }
    }

    /// Paint a stamp centred at `(u, v)` reaching `r` pixels out, wrapping round the edges.
    /// `f(dx, dy, texel)` gets the offset from the centre in pixels.
    pub fn stamp(&mut self, u: f32, v: f32, r: f32, mut f: impl FnMut(f32, f32, &mut Texel)) {
        let (w, h) = (self.w as i32, self.h as i32);
        let cx = u.rem_euclid(1.0) * w as f32;
        let cy = v.rem_euclid(1.0) * h as f32;
        let (x0, x1) = ((cx - r).floor() as i32, (cx + r).ceil() as i32);
        let (y0, y1) = ((cy - r).floor() as i32, (cy + r).ceil() as i32);
        for y in y0..=y1 {
            let row = y.rem_euclid(h) * w;
            let dy = y as f32 + 0.5 - cy;
            for x in x0..=x1 {
                let dx = x as f32 + 0.5 - cx;
                f(dx, dy, &mut self.px[(row + x.rem_euclid(w)) as usize]);
            }
        }
    }

    /// Box-blurred heights (wrapping), radius in pixels.
    fn blurred_height(&self, r: i32) -> Vec<f32> {
        let (w, h) = (self.w as i32, self.h as i32);
        let n = (2 * r + 1) as f32;
        let mut tmp = vec![0.0f32; (w * h) as usize];
        for y in 0..h {
            let row = y * w;
            let mut acc: f32 = (-r..=r)
                .map(|k| self.px[(row + k.rem_euclid(w)) as usize].height)
                .sum();
            for x in 0..w {
                tmp[(row + x) as usize] = acc / n;
                acc += self.px[(row + (x + r + 1).rem_euclid(w)) as usize].height
                    - self.px[(row + (x - r).rem_euclid(w)) as usize].height;
            }
        }
        let mut out = vec![0.0f32; (w * h) as usize];
        for x in 0..w {
            let mut acc: f32 = (-r..=r)
                .map(|k| tmp[(k.rem_euclid(h) * w + x) as usize])
                .sum();
            for y in 0..h {
                out[(y * w + x) as usize] = acc / n;
                acc += tmp[((y + r + 1).rem_euclid(h) * w + x) as usize]
                    - tmp[((y - r).rem_euclid(h) * w + x) as usize];
            }
        }
        out
    }

    /// Bake the images. `normal_strength` scales the slopes (1 = physical), `cavity` is how
    /// much occlusion a meter of depth below the surroundings gives (about 20–60).
    pub fn bake(&self, normal_strength: f32, cavity: f32) -> TexSet {
        let (w, h) = (self.w as i32, self.h as i32);
        let m = self.texel_m();
        let short = self.tile.x.min(self.tile.y);
        let (su, sv) = (self.tile.x / short, self.tile.y / short);
        let r_small = ((0.012 / m).round() as i32).clamp(1, 12);
        let r_large = ((0.05 / m).round() as i32).clamp(2, 40);
        let near = self.blurred_height(r_small);
        let far = self.blurred_height(r_large);
        let mut albedo = Vec::with_capacity((w * h * 4) as usize);
        let mut normal = Vec::with_capacity((w * h * 4) as usize);
        let mut orm = Vec::with_capacity((w * h * 4) as usize);
        let at = |x: i32, y: i32| self.px[(y.rem_euclid(h) * w + x.rem_euclid(w)) as usize].height;
        for y in 0..h {
            for x in 0..w {
                let i = (y * w + x) as usize;
                let t = &self.px[i];
                let dx = (at(x + 1, y) - at(x - 1, y)) / (2.0 * m);
                let dy = (at(x, y + 1) - at(x, y - 1)) / (2.0 * m);
                let n = Vec3::new(-dx * normal_strength * su, dy * normal_strength * sv, 1.0)
                    .normalize();
                normal.extend_from_slice(&[
                    byte(n.x * 0.5 + 0.5),
                    byte(n.y * 0.5 + 0.5),
                    byte(n.z * 0.5 + 0.5),
                    255,
                ]);
                let depth = (near[i] - t.height).max(0.0) * 0.6 + (far[i] - t.height).max(0.0);
                let ao = (1.0 - depth * cavity).clamp(0.15, 1.0) * t.ao;
                orm.extend_from_slice(&[byte(ao), byte(t.rough), byte(t.metal), 255]);
                albedo.extend_from_slice(&[
                    linear_to_srgb8(t.color.x),
                    linear_to_srgb8(t.color.y),
                    linear_to_srgb8(t.color.z),
                    byte(t.alpha),
                ]);
            }
        }
        let image = |data: Vec<u8>, srgb: bool| {
            let mut img = Image::new(self.w, self.h, data, srgb);
            img.generate_mips();
            img
        };
        TexSet {
            albedo: image(albedo, true),
            normal: image(normal, false),
            orm: image(orm, false),
        }
    }
}

// ------------------------------------------------------------------------------- meshes

/// A vertex with a linear RGBA colour.
#[inline]
pub fn vert(p: Vec3, n: Vec3, uv: Vec2, c: Vec4) -> Vertex {
    Vertex::new(p, n, uv).with_color(c)
}

/// Append `other` to `m`, rotated, scaled and moved, optionally tinted (multiplied).
pub fn append(m: &mut Mesh, other: &Mesh, rot: Quat, scale: Vec3, at: Vec3, tint: Vec4) {
    let base = m.vertices.len() as u32;
    let inv = Vec3::ONE / scale;
    for v in &other.vertices {
        let p = rot * (Vec3::from(v.position) * scale) + at;
        let n = (rot * (Vec3::from(v.normal) * inv)).normalize_or(Vec3::Y);
        let c = Vec4::from(v.color) * tint;
        m.vertices.push(vert(p, n, Vec2::from(v.uv), c));
    }
    m.indices.extend(other.indices.iter().map(|i| i + base));
}

/// Unit icosphere: positions and triangles (counter-clockwise from outside).
pub fn icosphere(subdiv: u32) -> (Vec<Vec3>, Vec<[u32; 3]>) {
    let t = (1.0 + 5f32.sqrt()) * 0.5;
    let mut pos: Vec<Vec3> = [
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
    for _ in 0..subdiv {
        let mut mid = std::collections::HashMap::new();
        let mut next = Vec::with_capacity(tris.len() * 4);
        let mut midpoint = |a: u32, b: u32, pos: &mut Vec<Vec3>| -> u32 {
            let key = (a.min(b), a.max(b));
            *mid.entry(key).or_insert_with(|| {
                pos.push(((pos[a as usize] + pos[b as usize]) * 0.5).normalize());
                pos.len() as u32 - 1
            })
        };
        for [a, b, c] in tris {
            let ab = midpoint(a, b, &mut pos);
            let bc = midpoint(b, c, &mut pos);
            let ca = midpoint(c, a, &mut pos);
            next.extend_from_slice(&[[a, ab, ca], [b, bc, ab], [c, ca, bc], [ab, bc, ca]]);
        }
        tris = next;
    }
    (pos, tris)
}

/// A rock: a displaced, chipped icosphere `size` across (meters), resting on y = 0 with
/// about `bury` of its height below. Smooth normals, box-projected UVs in meters, and vertex
/// colours from `color` with mottling and darker undersides.
pub fn rock(seed: u32, subdiv: u32, size: Vec3, lump: f32, bury: f32, color: Vec3) -> Mesh {
    let (unit_pos, tris) = icosphere(subdiv);
    let mut rng = Rand::new(seed as u64 + 17);
    // A few flat chips cut into the shape, like broken stone.
    let cuts: Vec<(Vec3, f32)> = (0..rng.below(4) + 2)
        .map(|_| {
            let n = Vec3::new(rng.normal(1.0), rng.normal(0.6) + 0.2, rng.normal(1.0))
                .normalize_or(Vec3::Y);
            (n, rng.range(0.62, 0.9))
        })
        .collect();
    let offset = Vec3::new(
        rng.range(0.0, 50.0),
        rng.range(0.0, 50.0),
        rng.range(0.0, 50.0),
    );
    let pos: Vec<Vec3> = unit_pos
        .iter()
        .map(|&p| {
            let mut r = 1.0
                + lump * fbm3(p * 1.4 + offset, 3, seed)
                + lump * 0.25 * fbm3(p * 5.0 + offset, 2, seed + 1);
            for &(n, d) in &cuts {
                let k = p.dot(n);
                if k > 1e-3 {
                    r = r.min(d / k + (1.0 - d / k) * 0.15);
                }
            }
            let mut q = p * r;
            // Flatter underneath, so it sits.
            if q.y < 0.0 {
                q.y *= 0.55;
            }
            q
        })
        .collect();
    let min_y = pos.iter().map(|p| p.y).fold(f32::MAX, f32::min);
    let max_y = pos.iter().map(|p| p.y).fold(f32::MIN, f32::max);
    let lift = -min_y - (max_y - min_y) * bury;
    let scaled: Vec<Vec3> = pos
        .iter()
        .map(|p| (*p + Vec3::Y * lift) * size * 0.5)
        .collect();
    let mut normals = vec![Vec3::ZERO; scaled.len()];
    for t in &tris {
        let [a, b, c] = t.map(|i| scaled[i as usize]);
        let n = (b - a).cross(c - a);
        for &i in t {
            normals[i as usize] += n;
        }
    }
    let h = (max_y - min_y + 1e-3) * size.y * 0.5;
    let mut m = Mesh::default();
    for t in &tris {
        let [a, b, c] = t.map(|i| scaled[i as usize]);
        let fnorm = (b - a).cross(c - a).abs();
        // Project UVs along the face's main axis (meters).
        let proj = |p: Vec3| {
            if fnorm.y >= fnorm.x && fnorm.y >= fnorm.z {
                Vec2::new(p.x, p.z)
            } else if fnorm.x >= fnorm.z {
                Vec2::new(p.z, p.y)
            } else {
                Vec2::new(p.x, p.y)
            }
        };
        let uv_off = Vec2::new(unit(seed) * 3.0, unit(seed ^ 0xabc) * 3.0);
        for &i in t {
            let p = scaled[i as usize];
            let up = ((p.y / h) + 0.15).clamp(0.0, 1.0);
            let mottle = 0.82 + 0.3 * fbm3(unit_pos[i as usize] * 3.0 + offset, 2, seed + 5);
            let c = color * mottle * (0.55 + 0.45 * up.sqrt());
            m.vertices.push(vert(
                p,
                normals[i as usize].normalize_or(Vec3::Y),
                proj(p) + uv_off,
                c.extend(1.0),
            ));
        }
        let base = m.vertices.len() as u32 - 3;
        m.indices.extend_from_slice(&[base, base + 1, base + 2]);
    }
    m
}

/// A box `size` meters with rounded (beveled) edges of radius `bevel`, centred on the origin.
/// Faces are split every `seg` meters (for vertex colours and bending); UVs are in meters
/// with u along the longest axis, offset by `uv_offset`. `color(p)` colours each vertex.
pub fn beveled_box(
    size: Vec3,
    bevel: f32,
    seg: f32,
    uv_offset: Vec2,
    color: impl Fn(Vec3) -> Vec4,
) -> Mesh {
    let h = size * 0.5;
    let b = bevel.min(h.min_element() * 0.95).max(1e-4);
    let inner = h - Vec3::splat(b);
    let coords = |axis: usize| -> Vec<f32> {
        let (ha, ia) = (h[axis], inner[axis]);
        let mut c = vec![-ha, -ha + b * 0.3, -ia];
        let n = ((2.0 * ia) / seg).ceil().max(1.0) as usize;
        for k in 1..n {
            c.push(-ia + 2.0 * ia * k as f32 / n as f32);
        }
        c.extend_from_slice(&[ia, ha - b * 0.3, ha]);
        c
    };
    let longest = if size.x >= size.y && size.x >= size.z {
        0
    } else if size.y >= size.z {
        1
    } else {
        2
    };
    let mut m = Mesh::default();
    for axis in 0..3 {
        for sign in [1.0f32, -1.0] {
            let (ua, va) = if sign > 0.0 {
                ((axis + 1) % 3, (axis + 2) % 3)
            } else {
                ((axis + 2) % 3, (axis + 1) % 3)
            };
            let (cu, cv) = (coords(ua), coords(va));
            let base = m.vertices.len() as u32;
            for &v in &cv {
                for &u in &cu {
                    let mut p = Vec3::ZERO;
                    p[axis] = sign * h[axis];
                    p[ua] = u;
                    p[va] = v;
                    let q = p.clamp(-inner, inner);
                    let d = (p - q).normalize_or(Vec3::Y);
                    // Round the bevel: points on it move onto the quarter circle.
                    let pos = q + d * b;
                    let uv = if va == longest {
                        Vec2::new(pos[va], pos[ua])
                    } else {
                        Vec2::new(pos[ua], pos[va])
                    };
                    m.vertices.push(vert(pos, d, uv + uv_offset, color(pos)));
                }
            }
            let row = cu.len() as u32;
            for j in 0..cv.len() as u32 - 1 {
                for i in 0..row - 1 {
                    let k = base + j * row + i;
                    m.indices
                        .extend_from_slice(&[k, k + 1, k + row + 1, k, k + row + 1, k + row]);
                }
            }
        }
    }
    m
}

/// A tube through `path` with radius `radius(t, angle)` (t from 0 to 1 along the path),
/// `sides` around, optionally capped. UVs: u along the tube and v round it, in meters.
pub fn tube(
    path: &[Vec3],
    sides: u32,
    caps: bool,
    radius: impl Fn(f32, f32) -> f32,
    color: impl Fn(f32) -> Vec4,
) -> Mesh {
    let mut m = Mesh::default();
    let n = path.len();
    if n < 2 {
        return m;
    }
    let tangent = |i: usize| {
        let a = path[i.saturating_sub(1)];
        let b = path[(i + 1).min(n - 1)];
        (b - a).normalize_or(Vec3::X)
    };
    // Parallel-transported frame.
    let t0 = tangent(0);
    let mut normal = if t0.y.abs() < 0.9 {
        t0.cross(Vec3::Y).normalize()
    } else {
        t0.cross(Vec3::X).normalize()
    };
    let mut along = 0.0;
    let mut rings = Vec::with_capacity(n);
    for i in 0..n {
        let t = tangent(i);
        normal = (normal - t * normal.dot(t)).normalize_or(normal);
        let bin = t.cross(normal);
        if i > 0 {
            along += path[i].distance(path[i - 1]);
        }
        rings.push((t, normal, bin, along));
    }
    let total = along.max(1e-4);
    for (i, &(_, nrm, bin, a)) in rings.iter().enumerate() {
        let tt = a / total;
        let c = color(tt);
        for s in 0..=sides {
            let ang = s as f32 / sides as f32 * std::f32::consts::TAU;
            let dir = nrm * ang.cos() + bin * ang.sin();
            let r = radius(tt, ang);
            m.vertices
                .push(vert(path[i] + dir * r, dir, Vec2::new(a, ang * r), c));
        }
    }
    let row = sides + 1;
    for i in 0..n as u32 - 1 {
        for s in 0..sides {
            let k = i * row + s;
            m.indices
                .extend_from_slice(&[k, k + 1, k + row, k + 1, k + row + 1, k + row]);
        }
    }
    if caps {
        for (end, flip) in [(0usize, true), (n - 1, false)] {
            let (t, nrm, bin, a) = rings[end];
            let tt = a / total;
            let nn = if flip { -t } else { t };
            let c = color(tt);
            let center = m.vertices.len() as u32;
            m.vertices.push(vert(path[end], nn, Vec2::ZERO, c));
            for s in 0..=sides {
                let ang = s as f32 / sides as f32 * std::f32::consts::TAU;
                let dir = nrm * ang.cos() + bin * ang.sin();
                let r = radius(tt, ang);
                m.vertices.push(vert(
                    path[end] + dir * r,
                    nn,
                    Vec2::new(ang.cos() * r, ang.sin() * r),
                    c,
                ));
            }
            for s in 0..sides {
                let (p, q) = (center + 1 + s, center + 2 + s);
                if flip {
                    m.indices.extend_from_slice(&[center, q, p]);
                } else {
                    m.indices.extend_from_slice(&[center, p, q]);
                }
            }
        }
    }
    m
}

/// One grass blade, both faces (so either side lights correctly without a double-sided
/// material). `lean` is the blade's angle from vertical at the root and `curl` how much more
/// it bends by the tip (radians), toward `toward` (horizontal unit vector). Colours run from
/// `root` to `tip`, darkened near the ground by `ao`.
#[allow(clippy::too_many_arguments)]
pub fn blade(
    m: &mut Mesh,
    root_at: Vec3,
    toward: Vec3,
    length: f32,
    width: f32,
    lean: f32,
    curl: f32,
    twist: f32,
    root: Vec3,
    tip: Vec3,
    ao: f32,
) {
    // Short blades need fewer joints.
    let segs: usize = if length > 0.22 {
        4
    } else if length > 0.1 {
        3
    } else {
        2
    };
    let side0 = Vec3::Y.cross(toward).normalize_or(Vec3::X);
    let mut p = root_at;
    let mut rows = [(Vec3::ZERO, Vec3::ZERO, Vec3::ZERO, 0.0f32); 5];
    for (i, row) in rows.iter_mut().enumerate().take(segs + 1) {
        let t = i as f32 / segs as f32;
        let a = lean + curl * t * t;
        let dir = toward * a.sin() + Vec3::Y * a.cos();
        let side = Quat::from_axis_angle(dir, twist * t) * side0;
        let w = width * (1.0 - t).powf(0.7) * (1.0 - 0.25 * t);
        *row = (p, dir, side, w);
        p += dir * (length / segs as f32);
    }
    let rows = &rows[..=segs];
    for face in [1.0f32, -1.0] {
        let base = m.vertices.len() as u32;
        for (i, &(pt, dir, side, w)) in rows.iter().enumerate() {
            let t = i as f32 / segs as f32;
            let nf = (side.cross(dir) * face).normalize_or(Vec3::Y);
            let n = (nf + Vec3::Y * 0.45).normalize();
            let shade = ao + (1.0 - ao) * smooth(0.0, 0.55, t);
            let c = (root.lerp(tip, t.powf(0.8)) * shade).extend(1.0);
            if i == segs {
                m.vertices.push(vert(pt, n, Vec2::new(0.5, 0.0), c));
            } else {
                m.vertices
                    .push(vert(pt - side * w * 0.5, n, Vec2::new(0.0, 1.0 - t), c));
                m.vertices
                    .push(vert(pt + side * w * 0.5, n, Vec2::new(1.0, 1.0 - t), c));
            }
        }
        // Wind each triangle so it faces this side's normal.
        let (_, dir0, side0, _) = rows[0];
        let nf = side0.cross(dir0) * face;
        let mut tri = |a: u32, b: u32, c: u32| {
            let [pa, pb, pc] = [a, b, c].map(|i| Vec3::from(m.vertices[i as usize].position));
            if (pb - pa).cross(pc - pa).dot(nf) >= 0.0 {
                m.indices.extend_from_slice(&[a, b, c]);
            } else {
                m.indices.extend_from_slice(&[a, c, b]);
            }
        };
        for i in 0..segs as u32 {
            let (a, b) = (base + i * 2, base + i * 2 + 1);
            if i + 1 == segs as u32 {
                tri(a, b, base + i * 2 + 2);
            } else {
                let (c, d) = (base + i * 2 + 2, base + i * 2 + 3);
                tri(a, b, d);
                tri(a, d, c);
            }
        }
    }
}

/// A leaf card: a curved grid `length` × `width` meters with UVs in one quarter of a 2×2
/// atlas (`cell` 0..3), lying in the XZ plane before `rot`, centred at `at`. `droop` bends
/// the tip and stem down, `cup` raises the sides (meters).
#[allow(clippy::too_many_arguments)]
pub fn leaf(
    m: &mut Mesh,
    at: Vec3,
    rot: Quat,
    length: f32,
    width: f32,
    droop: f32,
    cup: f32,
    cell: u32,
    color: Vec4,
) {
    const NA: usize = 4;
    const NC: usize = 3;
    let base = m.vertices.len() as u32;
    let (cx, cy) = ((cell % 2) as f32 * 0.5, (cell / 2 % 2) as f32 * 0.5);
    let local = |a: f32, c: f32| {
        let y = droop * (2.0 * a).powi(2) + cup * (2.0 * c).powi(2);
        Vec3::new(c * width, y, a * length)
    };
    for i in 0..NA {
        let a = i as f32 / (NA - 1) as f32 - 0.5;
        for j in 0..NC {
            let c = j as f32 / (NC - 1) as f32 - 0.5;
            let p = local(a, c);
            let e = 0.02;
            let da = local(a + e, c) - local(a - e, c);
            let dc = local(a, c + e) - local(a, c - e);
            let n = da.cross(dc).normalize_or(Vec3::Y);
            let n = if n.y < 0.0 { -n } else { n };
            let uv = Vec2::new(cx + (c + 0.5) * 0.5, cy + (0.5 - a) * 0.5);
            m.vertices.push(vert(rot * p + at, rot * n, uv, color));
        }
    }
    for i in 0..NA as u32 - 1 {
        for j in 0..NC as u32 - 1 {
            let k = base + i * NC as u32 + j;
            let n = NC as u32;
            // Counter-clockwise seen from +Y (before rotation).
            m.indices
                .extend_from_slice(&[k, k + n, k + 1, k + 1, k + n, k + n + 1]);
        }
    }
}

/// Smooth normals for the vertices `from..` of `m` (leaves earlier vertices alone).
pub fn smooth_normals_from(m: &mut Mesh, from_index: usize) {
    let first = m.indices[from_index..].iter().copied().min().unwrap_or(0) as usize;
    let mut normals = vec![Vec3::ZERO; m.vertices.len() - first];
    for tri in m.indices[from_index..].as_chunks::<3>().0 {
        let [a, b, c] =
            [tri[0], tri[1], tri[2]].map(|i| Vec3::from(m.vertices[i as usize].position));
        let n = (b - a).cross(c - a);
        for &i in tri {
            normals[i as usize - first] += n;
        }
    }
    for (k, n) in normals.into_iter().enumerate() {
        m.vertices[first + k].normal = n.normalize_or(Vec3::Y).to_array();
    }
}

/// Rotation that turns +Y to `up` and then spins `yaw` round it.
pub fn tilt(up: Vec3, yaw: f32) -> Quat {
    Quat::from_rotation_arc(Vec3::Y, up.normalize_or(Vec3::Y)) * Quat::from_rotation_y(yaw)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn noise_tiles_and_stays_in_range() {
        for &(u, v) in &[(0.0, 0.3), (0.25, 0.75), (0.9, 0.1)] {
            let a = fbm(u, v, 4, 4, 7);
            let b = fbm(u + 1.0, v, 4, 4, 7);
            let c = fbm(u, v + 1.0, 4, 4, 7);
            assert!((a - b).abs() < 1e-4 && (a - c).abs() < 1e-4, "{a} {b} {c}");
            assert!(a.abs() <= 1.2);
            let w0 = worley(u * 8.0, v * 8.0, 8, 3, 1.0);
            let w1 = worley(u * 8.0 + 8.0, v * 8.0, 8, 3, 1.0);
            assert!((w0.f1 - w1.f1).abs() < 1e-4 && w0.id == w1.id);
        }
    }

    #[test]
    fn meshes_face_outward() {
        let check = |m: &Mesh| {
            let mut bad = 0;
            for tri in m.indices.as_chunks::<3>().0 {
                let [a, b, c] = [tri[0], tri[1], tri[2]].map(|i| m.vertices[i as usize]);
                let g = (Vec3::from(b.position) - Vec3::from(a.position))
                    .cross(Vec3::from(c.position) - Vec3::from(a.position));
                let n = Vec3::from(a.normal) + Vec3::from(b.normal) + Vec3::from(c.normal);
                if g.length_squared() > 1e-14 && g.dot(n) < 0.0 {
                    bad += 1;
                }
            }
            bad
        };
        let b = beveled_box(Vec3::new(2.0, 0.05, 0.2), 0.006, 0.5, Vec2::ZERO, |_| {
            Vec4::ONE
        });
        assert_eq!(check(&b), 0, "beveled box");
        let bb = b.aabb();
        assert!(
            bb.max.abs_diff_eq(Vec3::new(1.0, 0.025, 0.1), 1e-4),
            "{bb:?}"
        );
        let r = rock(3, 2, Vec3::splat(0.1), 0.3, 0.2, Vec3::splat(0.5));
        assert_eq!(check(&r), 0, "rock");
        let t = tube(
            &[Vec3::ZERO, Vec3::X, Vec3::new(2.0, 0.5, 0.0)],
            8,
            true,
            |_, _| 0.05,
            |_| Vec4::ONE,
        );
        assert_eq!(check(&t), 0, "tube");
        let mut g = Mesh::default();
        blade(
            &mut g,
            Vec3::ZERO,
            Vec3::X,
            0.3,
            0.006,
            0.2,
            0.6,
            0.4,
            Vec3::ONE,
            Vec3::ONE,
            0.4,
        );
        assert_eq!(g.triangle_count(), 14);
        // Shorter blades get fewer joints.
        for (len, tris) in [(0.15, 10), (0.06, 6)] {
            let mut g = Mesh::default();
            blade(
                &mut g,
                Vec3::ZERO,
                Vec3::X,
                len,
                0.006,
                0.2,
                0.6,
                0.4,
                Vec3::ONE,
                Vec3::ONE,
                0.4,
            );
            assert_eq!(g.triangle_count(), tris);
        }
        let mut l = Mesh::default();
        leaf(
            &mut l,
            Vec3::ZERO,
            Quat::IDENTITY,
            0.08,
            0.06,
            -0.005,
            0.004,
            1,
            Vec4::ONE,
        );
        assert_eq!(check(&l), 0, "leaf");
    }

    #[test]
    fn baked_normals_lean_away_from_slopes() {
        // Height rising with u: the normal must lean toward -u (red below 128).
        let mut c = Canvas::new(16, 16, Vec2::splat(1.0));
        c.fill(|u, _| Texel {
            height: (u * std::f32::consts::TAU).sin() * 0.02,
            ..Default::default()
        });
        let set = c.bake(1.0, 0.0);
        // At u ≈ 0 the slope is rising.
        assert!(set.normal.data[0] < 120, "{}", set.normal.data[0]);
        let mut c = Canvas::new(16, 16, Vec2::splat(1.0));
        c.fill(|_, v| Texel {
            height: (v * std::f32::consts::TAU).sin() * 0.02,
            ..Default::default()
        });
        let set = c.bake(1.0, 0.0);
        // Height rising with v (downwards in the image): the normal leans to -v = +green.
        assert!(set.normal.data[1] > 136, "{}", set.normal.data[1]);
    }
}
