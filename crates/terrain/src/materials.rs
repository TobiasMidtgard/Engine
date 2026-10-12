//! Ground materials: detailed PBR textures for the terrain shader, generated procedurally.
//!
//! Each [`GroundMaterial`] (turf, forest floor, soil, mud, rock, snow, sand, gravel, concrete)
//! is built the way a material artist builds one in a node graph: a height field made from
//! noise, cells and thousands of scattered pieces (grass blades in tufts, leaves, needles,
//! twigs, pebbles), then normals, ambient occlusion, cavity tint and roughness derived from
//! it. The result is two texture arrays with full mip chains:
//!
//! - `albedo_height`: sRGB albedo, linear height (0..1) in alpha, used for height blending
//!   between materials (snow and mud fill the low spots first, stones poke through).
//! - `normal_rough_ao`: tangent-space normal x/y, roughness, occlusion. Mips widen the
//!   roughness where the normals they average disagree, so distant ground does not sparkle.
//!
//! Everything tiles seamlessly and is deterministic. The shader multiplies each material's
//! detail (texel / mean) into the planet palette's color for it, so the same textures serve
//! the Earth, the Moon and Mars, and the ground looks the same from near and far.
//!
//! [`shared`] bakes once per process on a background thread (about a second in release
//! builds for 1024² textures); the terrain draws plain colors until it is ready.

use std::collections::HashMap;
use std::f32::consts::TAU;
use std::sync::{Arc, Mutex, OnceLock};

/// Number of ground materials (texture array layers).
pub const MATERIAL_COUNT: usize = 9;

/// A ground material. The value is its texture array layer and its index in the shader.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[repr(u32)]
pub enum GroundMaterial {
    /// Short turf seen from above: tufts of blades over soil and dead thatch.
    Grass,
    /// Leaf litter, needles, twigs and moss.
    ForestFloor,
    /// Dry earth with clods, grit, pebbles and roots.
    Soil,
    /// Wet, smeared soil with glossy hollows and half-sunk stones.
    Mud,
    /// Fractured, faceted rock with strata, cracks, lichen and streaks (projected on three axes).
    Rock,
    /// Soft snow with wind ripples and glinting crystals.
    Snow,
    /// Rippled sand with mineral grains.
    Sand,
    /// Rounded pebbles and stones of mixed size and color.
    Gravel,
    /// Cast concrete slabs with pores, stains and joints.
    Concrete,
}

impl GroundMaterial {
    /// All materials in layer order.
    pub const ALL: [GroundMaterial; MATERIAL_COUNT] = [
        GroundMaterial::Grass,
        GroundMaterial::ForestFloor,
        GroundMaterial::Soil,
        GroundMaterial::Mud,
        GroundMaterial::Rock,
        GroundMaterial::Snow,
        GroundMaterial::Sand,
        GroundMaterial::Gravel,
        GroundMaterial::Concrete,
    ];

    /// Short lowercase name.
    pub fn name(self) -> &'static str {
        match self {
            GroundMaterial::Grass => "grass",
            GroundMaterial::ForestFloor => "forest_floor",
            GroundMaterial::Soil => "soil",
            GroundMaterial::Mud => "mud",
            GroundMaterial::Rock => "rock",
            GroundMaterial::Snow => "snow",
            GroundMaterial::Sand => "sand",
            GroundMaterial::Gravel => "gravel",
            GroundMaterial::Concrete => "concrete",
        }
    }

    /// Meters of ground one texture tile covers. Powers of two, so patterns stay put across
    /// the renderer's 4096 m camera tiles (`tiling_pos`).
    pub fn tile_m(self) -> f32 {
        match self {
            GroundMaterial::Grass | GroundMaterial::Soil | GroundMaterial::Gravel => 2.0,
            GroundMaterial::Rock => 8.0,
            _ => 4.0,
        }
    }
}

/// Averages the shader falls back to once a material's texture is too small on screen to
/// matter (taken from the 1×1 mip, so the fade is seamless).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct MaterialStats {
    /// Mean albedo, linear RGB.
    pub mean_albedo: [f32; 3],
    pub mean_roughness: f32,
    pub mean_occlusion: f32,
    /// Height range of the texture, meters (lowest to highest point).
    pub height_m: f32,
}

/// Baked textures for every [`GroundMaterial`], ready to upload.
#[derive(Debug)]
pub struct BakedMaterials {
    /// Width and height of mip 0, texels.
    pub size: u32,
    pub mip_count: u32,
    /// `[material][mip]`: RGBA8 texels, sRGB albedo with linear height in alpha.
    pub albedo_height: Vec<Vec<Vec<u8>>>,
    /// `[material][mip]`: RGBA8 texels, normal x and y (0..1 = -1..1), roughness, occlusion.
    pub normal_rough_ao: Vec<Vec<Vec<u8>>>,
    pub stats: Vec<MaterialStats>,
    /// Wall-clock bake time, seconds.
    pub bake_secs: f32,
}

type Slot = Arc<OnceLock<Arc<BakedMaterials>>>;

/// The process-wide baked set at `size` (a power of two, at least 16). The first call starts
/// the bake on a background thread and returns `None`; later calls return it once done.
pub fn shared(size: u32) -> Option<Arc<BakedMaterials>> {
    static BAKES: OnceLock<Mutex<HashMap<u32, Slot>>> = OnceLock::new();
    let size = size.next_power_of_two().max(16);
    let slot = {
        let mut map = BAKES
            .get_or_init(Default::default)
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        if let Some(slot) = map.get(&size) {
            slot.clone()
        } else {
            let slot: Slot = Arc::new(OnceLock::new());
            map.insert(size, slot.clone());
            let worker = slot.clone();
            std::thread::Builder::new()
                .name("terrain materials".into())
                .spawn(move || {
                    let _ = worker.set(Arc::new(bake(size)));
                })
                .expect("spawn the terrain material baker");
            slot
        }
    };
    slot.get().cloned()
}

/// Bake every material at `size`² texels (a power of two), one thread per material.
pub fn bake(size: u32) -> BakedMaterials {
    let started = std::time::Instant::now();
    let size = size.next_power_of_two().max(16);
    let mut results: Vec<Option<Finished>> = (0..MATERIAL_COUNT).map(|_| None).collect();
    std::thread::scope(|scope| {
        for (slot, material) in results.iter_mut().zip(GroundMaterial::ALL) {
            scope.spawn(move || {
                *slot = Some(bake_one(material, size as usize));
            });
        }
    });
    let mut out = BakedMaterials {
        size,
        mip_count: size.trailing_zeros() + 1,
        albedo_height: Vec::new(),
        normal_rough_ao: Vec::new(),
        stats: Vec::new(),
        bake_secs: 0.0,
    };
    for f in results.into_iter().flatten() {
        out.albedo_height.push(f.albedo_height);
        out.normal_rough_ao.push(f.normal_rough_ao);
        out.stats.push(f.stats);
    }
    out.bake_secs = started.elapsed().as_secs_f32();
    out
}

/// Bake one material: its height, albedo and roughness, then the derived maps and mips.
pub fn bake_one(material: GroundMaterial, size: usize) -> Finished {
    let mut l = Layer::new(
        size,
        material.tile_m(),
        0x5eed_0000 + material as u32 * 7919,
    );
    let cavity = match material {
        GroundMaterial::Grass => grass(&mut l),
        GroundMaterial::ForestFloor => forest_floor(&mut l),
        GroundMaterial::Soil => soil(&mut l),
        GroundMaterial::Mud => mud(&mut l),
        GroundMaterial::Rock => rock(&mut l),
        GroundMaterial::Snow => snow(&mut l),
        GroundMaterial::Sand => sand(&mut l),
        GroundMaterial::Gravel => gravel(&mut l),
        GroundMaterial::Concrete => concrete(&mut l),
    };
    finish(l, cavity)
}

/// One baked material: mips of both textures and its averages.
#[derive(Debug)]
pub struct Finished {
    pub albedo_height: Vec<Vec<u8>>,
    pub normal_rough_ao: Vec<Vec<u8>>,
    pub stats: MaterialStats,
}

// ------------------------------------------------------------------------------ noise

#[inline]
fn hash(mut x: u32) -> u32 {
    x ^= x >> 16;
    x = x.wrapping_mul(0x7feb_352d);
    x ^= x >> 15;
    x = x.wrapping_mul(0x846c_a68b);
    x ^= x >> 16;
    x
}

#[inline]
fn hash2(a: i32, b: i32, seed: u32) -> u32 {
    hash((a as u32).wrapping_mul(0x8da6_b343) ^ (b as u32).wrapping_mul(0xd816_3841) ^ hash(seed))
}

/// Uniform 0..1 from a hash.
#[inline]
fn unit(h: u32) -> f32 {
    (h >> 8) as f32 * (1.0 / 16_777_216.0)
}

const GRADS: [(f32, f32); 16] = {
    let mut g = [(0.0, 0.0); 16];
    // Unit vectors every 22.5° (const-evaluated from a table of cosines).
    const A: f32 = 0.923_88; // cos 22.5°
    const B: f32 = std::f32::consts::FRAC_1_SQRT_2;
    const D: f32 = 0.382_68; // cos 67.5°
    const C: [f32; 16] = [
        1.0, A, B, D, 0.0, -D, -B, -A, -1.0, -A, -B, -D, 0.0, D, B, A,
    ];
    let mut i = 0;
    while i < 16 {
        g[i] = (C[i], C[(i + 12) % 16]);
        i += 1;
    }
    g
};

#[inline]
fn quintic(t: f32) -> f32 {
    t * t * t * (t * (t * 6.0 - 15.0) + 10.0)
}

#[inline]
fn lerp(a: f32, b: f32, t: f32) -> f32 {
    a + (b - a) * t
}

#[inline]
fn smoothstep(e0: f32, e1: f32, x: f32) -> f32 {
    let t = ((x - e0) / (e1 - e0)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

#[inline]
fn mix3(a: [f32; 3], b: [f32; 3], t: f32) -> [f32; 3] {
    [
        lerp(a[0], b[0], t),
        lerp(a[1], b[1], t),
        lerp(a[2], b[2], t),
    ]
}

#[inline]
fn scale3(a: [f32; 3], s: f32) -> [f32; 3] {
    [a[0] * s, a[1] * s, a[2] * s]
}

/// Gradient noise on a lattice that repeats every `period` cells, about -1..1.
fn gnoise(x: f32, y: f32, period: i32, seed: u32) -> f32 {
    let xf = x.floor();
    let yf = y.floor();
    let (fx, fy) = (x - xf, y - yf);
    let (ix, iy) = (xf as i32, yf as i32);
    let p = period.max(1);
    let corner = |cx: i32, cy: i32, dx: f32, dy: f32| {
        let g = GRADS[(hash2(cx.rem_euclid(p), cy.rem_euclid(p), seed) & 15) as usize];
        g.0 * dx + g.1 * dy
    };
    let (u, v) = (quintic(fx), quintic(fy));
    let a = lerp(corner(ix, iy, fx, fy), corner(ix + 1, iy, fx - 1.0, fy), u);
    let b = lerp(
        corner(ix, iy + 1, fx, fy - 1.0),
        corner(ix + 1, iy + 1, fx - 1.0, fy - 1.0),
        u,
    );
    lerp(a, b, v) * 1.42
}

/// Fractal noise over tile coordinates (0..1 per tile, seamless), starting at `freq` cycles
/// per tile and doubling each octave. About -1..1.
fn fbm(u: f32, v: f32, freq: u32, octaves: u32, gain: f32, seed: u32) -> f32 {
    let mut sum = 0.0;
    let mut norm = 0.0;
    let mut amp = 1.0;
    let mut f = freq.max(1);
    for o in 0..octaves {
        sum += amp
            * gnoise(
                u * f as f32,
                v * f as f32,
                f as i32,
                seed.wrapping_add(o * 101),
            );
        norm += amp;
        amp *= gain;
        f *= 2;
    }
    sum / norm
}

/// Ridged fractal noise (sharp crests), 0..1.
fn ridged(u: f32, v: f32, freq: u32, octaves: u32, seed: u32) -> f32 {
    let mut sum = 0.0;
    let mut norm = 0.0;
    let mut amp = 1.0;
    let mut f = freq.max(1);
    for o in 0..octaves {
        let n = 1.0
            - gnoise(
                u * f as f32,
                v * f as f32,
                f as i32,
                seed.wrapping_add(o * 131),
            )
            .abs();
        sum += amp * n * n;
        norm += amp;
        amp *= 0.5;
        f *= 2;
    }
    sum / norm
}

/// Nearest and second-nearest feature points of a seamless cell pattern.
#[derive(Clone, Copy, Debug)]
struct Cells {
    /// Distance to the nearest point, cells.
    f1: f32,
    /// Distance to the second nearest point, cells.
    f2: f32,
    /// Hash of the nearest cell.
    id: u32,
    /// Offset from the nearest point to the sample, cells.
    dx: f32,
    dy: f32,
}

fn cells(u: f32, v: f32, freq: i32, jitter: f32, seed: u32) -> Cells {
    let x = u * freq as f32;
    let y = v * freq as f32;
    let (ix, iy) = (x.floor() as i32, y.floor() as i32);
    let mut best = Cells {
        f1: 9.0,
        f2: 9.0,
        id: 0,
        dx: 0.0,
        dy: 0.0,
    };
    for oy in -1..=1 {
        for ox in -1..=1 {
            let cx = ix + ox;
            let cy = iy + oy;
            let h = hash2(cx.rem_euclid(freq), cy.rem_euclid(freq), seed);
            let px = cx as f32 + 0.5 + (unit(h) - 0.5) * jitter;
            let py = cy as f32 + 0.5 + (unit(hash(h)) - 0.5) * jitter;
            let (dx, dy) = (x - px, y - py);
            let d = (dx * dx + dy * dy).sqrt();
            if d < best.f1 {
                best.f2 = best.f1;
                best.f1 = d;
                best.id = h;
                best.dx = dx;
                best.dy = dy;
            } else if d < best.f2 {
                best.f2 = d;
            }
        }
    }
    best
}

/// A small deterministic random stream for scattering.
struct Rng(u32);

impl Rng {
    fn next(&mut self) -> f32 {
        self.0 = hash(self.0.wrapping_add(0x9e37_79b9));
        unit(self.0)
    }

    fn range(&mut self, a: f32, b: f32) -> f32 {
        lerp(a, b, self.next())
    }

    fn pick<T: Copy>(&mut self, items: &[T]) -> T {
        items[((self.next() * items.len() as f32) as usize).min(items.len() - 1)]
    }
}

// ------------------------------------------------------------------------------ layers

/// The working buffers of one material while it is generated.
struct Layer {
    n: usize,
    /// Meters per tile.
    tile: f32,
    seed: u32,
    /// Height, meters.
    h: Vec<f32>,
    /// Linear albedo.
    albedo: Vec<[f32; 3]>,
    roughness: Vec<f32>,
}

impl Layer {
    fn new(n: usize, tile: f32, seed: u32) -> Self {
        Self {
            n,
            tile,
            seed,
            h: vec![0.0; n * n],
            albedo: vec![[0.5; 3]; n * n],
            roughness: vec![0.8; n * n],
        }
    }

    /// Meters per texel.
    fn texel(&self) -> f32 {
        self.tile / self.n as f32
    }

    #[inline]
    fn idx(&self, x: i32, y: i32) -> usize {
        let n = self.n as i32;
        (y.rem_euclid(n) * n + x.rem_euclid(n)) as usize
    }

    /// Set every texel from its tile coordinates (u, v in 0..1).
    fn fill(&mut self, f: impl Fn(f32, f32) -> (f32, [f32; 3], f32)) {
        let n = self.n;
        for y in 0..n {
            for x in 0..n {
                let (u, v) = ((x as f32 + 0.5) / n as f32, (y as f32 + 0.5) / n as f32);
                let (h, a, r) = f(u, v);
                let i = y * n + x;
                self.h[i] = h;
                self.albedo[i] = a;
                self.roughness[i] = r;
            }
        }
    }

    /// Height at a texel position (wrapping, nearest).
    fn height_at(&self, x: f32, y: f32) -> f32 {
        self.h[self.idx(x.round() as i32, y.round() as i32)]
    }

    /// Draw a piece over a box of texels around `(cx, cy)` (texels): `f(dx, dy)` returns its
    /// height, color and roughness where it covers the texel. A texel only changes where the
    /// piece is higher than what is there (pieces rest on and hide each other).
    fn stamp(
        &mut self,
        cx: f32,
        cy: f32,
        radius: f32,
        f: impl Fn(f32, f32) -> Option<(f32, [f32; 3], f32)>,
    ) {
        let r = radius.ceil() as i32 + 1;
        let (x0, y0) = (cx.floor() as i32, cy.floor() as i32);
        for y in y0 - r..=y0 + r {
            for x in x0 - r..=x0 + r {
                let (dx, dy) = (x as f32 + 0.5 - cx, y as f32 + 0.5 - cy);
                if let Some((h, a, rough)) = f(dx, dy) {
                    let i = self.idx(x, y);
                    if h > self.h[i] {
                        self.h[i] = h;
                        self.albedo[i] = a;
                        self.roughness[i] = rough;
                    }
                }
            }
        }
    }

    /// A tapered strand (grass blade, needle, twig) from `(x0, y0)` to `(x1, y1)` (texels):
    /// half-width `w0` texels at the root narrowing by `taper` (0 = constant, 1 = to a point);
    /// height rises from `h0` to `h1` meters along it, plus `round` meters across its width.
    #[allow(clippy::too_many_arguments)]
    fn strand(
        &mut self,
        (x0, y0): (f32, f32),
        (x1, y1): (f32, f32),
        w0: f32,
        taper: f32,
        (h0, h1): (f32, f32),
        round: f32,
        color: impl Fn(f32, f32) -> [f32; 3],
        rough: f32,
    ) {
        let (dx, dy) = (x1 - x0, y1 - y0);
        let len2 = (dx * dx + dy * dy).max(1e-6);
        let len = len2.sqrt();
        let (cx, cy) = ((x0 + x1) * 0.5, (y0 + y1) * 0.5);
        self.stamp(cx, cy, len * 0.5 + w0 + 1.0, |px, py| {
            let (rx, ry) = (px + cx - x0, py + cy - y0);
            let s = (rx * dx + ry * dy) / len2;
            if !(0.0..=1.0).contains(&s) {
                return None;
            }
            let perp = (rx * dy - ry * dx).abs() / len;
            let w = w0 * (1.0 - taper * s).max(0.0).powf(0.7) + 0.35;
            if perp > w {
                return None;
            }
            let across = perp / w;
            let h = lerp(h0, h1, s) + round * (1.0 - across * across).sqrt();
            Some((h, color(s, across), rough))
        });
    }
}

impl Layer {
    /// Press an elliptical hollow (radii in texels, depth in meters) into the ground, pushing
    /// a rim of `rim` meters up around it.
    #[allow(clippy::too_many_arguments)]
    fn carve(
        &mut self,
        cx: f32,
        cy: f32,
        rx: f32,
        ry: f32,
        angle: f32,
        depth: f32,
        rim: f32,
        seed: u32,
    ) {
        let (ca, sa) = (angle.cos(), angle.sin());
        let reach = rx.max(ry) * 1.5 + 1.0;
        let r = reach.ceil() as i32;
        let (x0, y0) = (cx.floor() as i32, cy.floor() as i32);
        for y in y0 - r..=y0 + r {
            for x in x0 - r..=x0 + r {
                let (dx, dy) = (x as f32 + 0.5 - cx, y as f32 + 0.5 - cy);
                let a = (dx * ca + dy * sa) / rx;
                let b = (-dx * sa + dy * ca) / ry;
                // Ragged outline and a floor that is deeper toward one end (heel or toe).
                let wobble = 1.0 + 0.25 * gnoise(dx / rx * 2.0, dy / rx * 2.0, 1 << 20, seed);
                let rho = (a * a + b * b).sqrt() / wobble;
                let depth = depth * (1.0 + 0.4 * a.clamp(-1.0, 1.0));
                let dh = if rho < 1.0 {
                    -depth * (1.0 - rho * rho).sqrt()
                } else if rho < 1.5 {
                    rim * (std::f32::consts::PI * (rho - 1.0) / 0.5).sin()
                } else {
                    continue;
                };
                let i = self.idx(x, y);
                self.h[i] += dh;
            }
        }
    }
}

// ------------------------------------------------------------------------------ materials

/// Short grass: soil, a mat of dead thatch, then tufts of living blades rising out of it.
fn grass(l: &mut Layer) -> f32 {
    let s = l.seed;
    l.fill(|u, v| {
        let n = fbm(u, v, 8, 5, 0.5, s);
        let c = fbm(u, v, 4, 4, 0.5, s + 1) * 0.5 + 0.5;
        let soil = mix3([0.085, 0.062, 0.042], [0.15, 0.11, 0.072], c);
        (0.004 * n, soil, 0.92)
    });
    let texel = l.texel();
    let mut rng = Rng(s ^ 0xa11ce);
    // Thatch: dead blades lying flat.
    let thatch = (l.n * l.n) / 120;
    for _ in 0..thatch {
        let (x, y) = (rng.range(0.0, l.n as f32), rng.range(0.0, l.n as f32));
        let a = rng.range(0.0, TAU);
        let len = rng.range(0.025, 0.07) / texel;
        let base = l.height_at(x, y) + 0.001;
        let tone = rng.range(0.0, 1.0);
        let col = mix3([0.20, 0.16, 0.09], [0.36, 0.31, 0.19], tone);
        l.strand(
            (x, y),
            (x + a.cos() * len, y + a.sin() * len),
            rng.range(0.6, 1.1),
            0.6,
            (base, base + 0.0015),
            0.0008,
            |s, across| scale3(col, 0.8 + 0.25 * (1.0 - across) - 0.1 * s),
            0.85,
        );
    }
    // Living blades in tufts, leaning outward from each tuft's center.
    let tufts = (l.n * l.n) / 600;
    let greens = [
        [0.075, 0.13, 0.03],
        [0.09, 0.15, 0.035],
        [0.11, 0.16, 0.04],
        [0.065, 0.12, 0.04],
        [0.13, 0.17, 0.05],
    ];
    for _ in 0..tufts {
        let (tx, ty) = (rng.range(0.0, l.n as f32), rng.range(0.0, l.n as f32));
        let spread = rng.range(0.005, 0.02) / texel;
        let blades = rng.range(8.0, 20.0) as usize;
        let tuft_tone = rng.pick(&greens);
        let dry_tuft = rng.next() < 0.08;
        for _ in 0..blades {
            let a = rng.range(0.0, TAU);
            let off = rng.next().sqrt() * spread;
            let (bx, by) = (tx + a.cos() * off, ty + a.sin() * off);
            let lean = a + rng.range(-0.6, 0.6);
            let len = rng.range(0.02, 0.075) / texel;
            let tall = rng.range(0.02, 0.06);
            let base = l.height_at(bx, by);
            let dead = dry_tuft || rng.next() < 0.05;
            let tip_col = if dead {
                mix3([0.30, 0.26, 0.13], [0.40, 0.34, 0.18], rng.next())
            } else {
                mix3(tuft_tone, [0.17, 0.20, 0.06], rng.range(0.0, 0.5))
            };
            let root_col = if dead {
                scale3(tip_col, 0.7)
            } else {
                [0.03, 0.05, 0.012]
            };
            let jitter = rng.range(0.85, 1.15);
            l.strand(
                (bx, by),
                (bx + lean.cos() * len, by + lean.sin() * len),
                rng.range(0.7, 1.4),
                0.95,
                (base, base + tall),
                0.0006,
                |s, across| {
                    let c = mix3(root_col, tip_col, s.powf(0.6));
                    scale3(c, jitter * (0.85 + 0.3 * (1.0 - across)))
                },
                if dead { 0.8 } else { 0.62 },
            );
        }
    }
    0.35
}

/// Leaf litter over humus: needles, twigs, moss and fallen leaves of many colors.
fn forest_floor(l: &mut Layer) -> f32 {
    let s = l.seed;
    l.fill(|u, v| {
        let n = fbm(u, v, 4, 5, 0.5, s);
        let c = fbm(u, v, 8, 4, 0.5, s + 1) * 0.5 + 0.5;
        let humus = mix3([0.05, 0.036, 0.025], [0.11, 0.078, 0.05], c);
        // Moss cushions.
        let m = smoothstep(0.15, 0.4, fbm(u, v, 3, 5, 0.55, s + 2));
        let bumps = fbm(u, v, 64, 3, 0.5, s + 3) * 0.5 + 0.5;
        let moss = mix3([0.06, 0.09, 0.02], [0.13, 0.17, 0.045], bumps);
        (
            0.012 * n + m * (0.006 + 0.004 * bumps),
            mix3(humus, moss, m),
            lerp(0.9, 0.82, m),
        )
    });
    let texel = l.texel();
    let n = l.n as f32;
    let mut rng = Rng(s ^ 0xf0e5);
    // Needles.
    for _ in 0..(l.n * l.n / 70) {
        let (x, y) = (rng.range(0.0, n), rng.range(0.0, n));
        let a = rng.range(0.0, TAU);
        let len = rng.range(0.02, 0.05) / texel;
        let base = l.height_at(x, y) + 0.0008;
        let col = mix3([0.11, 0.065, 0.03], [0.24, 0.15, 0.07], rng.next());
        l.strand(
            (x, y),
            (x + a.cos() * len, y + a.sin() * len),
            0.45,
            0.3,
            (base, base + 0.0005),
            0.0006,
            |_, across| scale3(col, 0.85 + 0.2 * (1.0 - across)),
            0.75,
        );
    }
    // Twigs with a side branch.
    for _ in 0..(l.n * l.n / 15_000).max(4) {
        let (x, y) = (rng.range(0.0, n), rng.range(0.0, n));
        let a = rng.range(0.0, TAU);
        let len = rng.range(0.08, 0.35) / texel;
        let r = rng.range(0.003, 0.009);
        let w = r / texel;
        let base = l.height_at(x, y) + 0.002;
        let col = mix3([0.10, 0.08, 0.06], [0.22, 0.17, 0.12], rng.next());
        let (ex, ey) = (x + a.cos() * len, y + a.sin() * len);
        l.strand(
            (x, y),
            (ex, ey),
            w,
            0.4,
            (base, base),
            r,
            |s, across| scale3(col, 0.7 + 0.45 * (1.0 - across) + 0.1 * (s * 40.0).sin()),
            0.8,
        );
        let t = rng.range(0.3, 0.7);
        let (mx, my) = (x + (ex - x) * t, y + (ey - y) * t);
        let b = a + rng.pick(&[-0.7, 0.7]);
        let bl = len * rng.range(0.2, 0.4);
        l.strand(
            (mx, my),
            (mx + b.cos() * bl, my + b.sin() * bl),
            w * 0.6,
            0.5,
            (base, base),
            r * 0.6,
            |_, across| scale3(col, 0.7 + 0.45 * (1.0 - across)),
            0.8,
        );
    }
    // Leaves: mostly browns and ochres, some still yellow or red, a few dark and rotting.
    let colors = [
        [0.24, 0.12, 0.045],
        [0.29, 0.18, 0.06],
        [0.17, 0.095, 0.04],
        [0.11, 0.07, 0.035],
        [0.27, 0.23, 0.08],
        [0.32, 0.11, 0.04],
        [0.20, 0.155, 0.075],
        [0.14, 0.11, 0.07],
    ];
    for _ in 0..(l.n * l.n / 160) {
        let (x, y) = (rng.range(0.0, n), rng.range(0.0, n));
        let a = rng.range(0.0, TAU);
        let len = rng.range(0.04, 0.09) / texel;
        let wid = len * rng.range(0.42, 0.62);
        let (ca, sa) = (a.cos(), a.sin());
        let col = scale3(rng.pick(&colors), rng.range(0.8, 1.15));
        let base = l.height_at(x, y) + 0.0015;
        let curl = rng.range(0.002, 0.006);
        let holes = rng.next() < 0.25;
        let hs = s ^ hash(rng.0);
        // Each leaf lies at its own tilt, so it catches the light differently.
        let (tx, ty) = (
            rng.range(-0.25, 0.25) * texel,
            rng.range(-0.25, 0.25) * texel,
        );
        l.stamp(x, y, len * 0.6, |dx, dy| {
            let along = (dx * ca + dy * sa) / (len * 0.5);
            let across = (-dx * sa + dy * ca) / (wid * 0.5);
            if along.abs() > 1.0 {
                return None;
            }
            let half = (1.0 - along * along).max(0.0).powf(0.75) * (1.0 + 0.15 * along);
            if across.abs() > half {
                return None;
            }
            let t = across / half.max(1e-3);
            if holes && gnoise(dx * 0.35, dy * 0.35, 1 << 20, hs) > 0.45 {
                return None;
            }
            let vein = (1.0 - (across.abs() / 0.06).min(1.0)) * 0.35;
            let side_veins =
                ((along * 7.0 - t.abs() * 3.0).sin().abs() < 0.12) as i32 as f32 * 0.12;
            let edge = smoothstep(0.75, 1.0, t.abs()) * 0.25;
            let shade = 1.0 - vein - side_veins - edge;
            let h = base + curl * t * t + 0.002 * (1.0 - along * along) + tx * dx + ty * dy;
            Some((h, scale3(col, shade), 0.7))
        });
    }
    0.45
}

/// Dry earth: lumpy ground covered in crumbs, grit, a few pebbles and fine roots.
fn soil(l: &mut Layer) -> f32 {
    let s = l.seed;
    l.fill(|u, v| {
        let base = fbm(u, v, 4, 6, 0.5, s);
        let lumps = smoothstep(-0.1, 0.6, fbm(u, v, 20, 3, 0.5, s + 1));
        let grit = fbm(u, v, 256, 2, 0.5, s + 2);
        let h = 0.014 * base + 0.004 * lumps + 0.0006 * grit;
        let tone = fbm(u, v, 3, 4, 0.5, s + 3) * 0.5 + 0.5;
        let mut a = mix3([0.15, 0.105, 0.07], [0.24, 0.18, 0.12], tone);
        a = scale3(a, 0.82 + 0.25 * lumps + 0.12 * grit);
        (h, a, 0.93)
    });
    let texel = l.texel();
    let n = l.n as f32;
    let mut rng = Rng(s ^ 0x5011);
    let area = l.n * l.n;
    let crumbs = [
        [0.17, 0.12, 0.08],
        [0.21, 0.155, 0.10],
        [0.25, 0.19, 0.13],
        [0.13, 0.095, 0.065],
        [0.28, 0.22, 0.15],
    ];
    scatter_stones(
        l,
        &mut rng,
        area / 260,
        (0.0015, 0.006),
        0.35,
        &crumbs,
        (0.85, 0.95),
    );
    scatter_stones(
        l,
        &mut rng,
        area / 6000,
        (0.002, 0.007),
        0.5,
        &PEBBLES,
        (0.5, 0.85),
    );
    scatter_stones(
        l,
        &mut rng,
        (area / 60_000).max(2),
        (0.01, 0.022),
        0.45,
        &PEBBLES,
        (0.5, 0.8),
    );
    // Fine roots.
    for _ in 0..(area / 18_000).max(3) {
        let (mut x, mut y) = (rng.range(0.0, n), rng.range(0.0, n));
        let mut a = rng.range(0.0, TAU);
        let seg = 0.012 / texel;
        let col = mix3([0.10, 0.07, 0.045], [0.22, 0.16, 0.1], rng.next());
        for _ in 0..(rng.range(6.0, 18.0) as usize) {
            a += rng.range(-0.5, 0.5);
            let (nx, ny) = (x + a.cos() * seg, y + a.sin() * seg);
            let base = l.height_at(x, y) + 0.0006;
            l.strand(
                (x, y),
                (nx, ny),
                0.6,
                0.0,
                (base, base),
                0.0005,
                |_, _| col,
                0.85,
            );
            (x, y) = (nx, ny);
        }
    }
    0.5
}

/// Wet mud churned by feet and hooves: overlapping hollows with pushed-up rims, smears,
/// glossy water in the low spots, sunken stones and bits of straw.
fn mud(l: &mut Layer) -> f32 {
    let s = l.seed;
    l.fill(|u, v| {
        let base = fbm(u, v, 3, 5, 0.5, s);
        let wu = u + 0.04 * fbm(u, v, 2, 3, 0.5, s + 1);
        let wv = v + 0.04 * fbm(u, v, 2, 3, 0.5, s + 2);
        let smear = ridged(wu * 0.5 + wv * 0.5, wv * 2.0 - wu * 0.5, 6, 3, s + 3);
        let smear_mask = smoothstep(-0.1, 0.4, fbm(u, v, 2, 3, 0.5, s + 4));
        let lumps = fbm(u, v, 16, 3, 0.5, s + 5);
        let fine = fbm(u, v, 96, 3, 0.5, s + 6);
        let h = 0.02 * base + 0.006 * smear * smear_mask + 0.004 * lumps + 0.0006 * fine;
        (h, [0.0; 3], 0.0)
    });
    // Old prints: hollows with a rim of pushed-up mud.
    let texel = l.texel();
    let n = l.n as f32;
    let mut rng = Rng(s ^ 0x3d);
    let area = l.n * l.n;
    for _ in 0..(area / 6000).max(4) {
        let (x, y) = (rng.range(0.0, n), rng.range(0.0, n));
        let rx = rng.range(0.05, 0.12) / texel;
        let ry = rx * rng.range(0.45, 0.9);
        let a = rng.range(0.0, TAU);
        let depth = rng.range(0.003, 0.012);
        let rim = depth * rng.range(0.2, 0.5);
        l.carve(x, y, rx, ry, a, depth, rim, hash(rng.0));
    }
    // Wet mud slumps: soften everything a little.
    let r = (0.004 / texel).round().max(1.0) as usize;
    l.h = blur(&l.h, l.n, r);
    // Color follows the broad height (low ground is wetter and darker), gloss the local
    // height (water sits in every hollow).
    let (lo, hi) = range(&l.h);
    let span = (hi - lo).max(1e-6);
    let broad = blur(&l.h, l.n, (0.04 / texel).round().max(2.0) as usize);
    let n_us = l.n;
    for (i, &broad_h) in broad.iter().enumerate() {
        let (x, y) = (i % n_us, i / n_us);
        let (u, v) = ((x as f32 + 0.5) / n, (y as f32 + 0.5) / n);
        let t = ((l.h[i] - lo) / span).clamp(0.0, 1.0);
        let tb = ((broad_h - lo) / span).clamp(0.0, 1.0);
        let tone = fbm(u, v, 5, 3, 0.5, s + 7) * 0.5 + 0.5;
        let dry = smoothstep(0.3, 0.75, tb);
        l.albedo[i] = scale3(
            mix3([0.07, 0.052, 0.036], [0.13, 0.10, 0.07], dry),
            0.9 + 0.2 * tone,
        );
        l.roughness[i] = lerp(0.12, 0.62, smoothstep(0.25, 0.6, t));
    }
    scatter_stones(
        l,
        &mut rng,
        area / 9000,
        (0.004, 0.02),
        0.2,
        &PEBBLES,
        (0.3, 0.6),
    );
    for _ in 0..(area / 2500) {
        let (x, y) = (rng.range(0.0, n), rng.range(0.0, n));
        let a = rng.range(0.0, TAU);
        let len = rng.range(0.03, 0.08) / texel;
        let base = l.height_at(x, y) + 0.0004;
        let col = mix3([0.10, 0.08, 0.04], [0.21, 0.17, 0.09], rng.next());
        l.strand(
            (x, y),
            (x + a.cos() * len, y + a.sin() * len),
            0.5,
            0.5,
            (base, base),
            0.0005,
            |_, _| col,
            0.5,
        );
    }
    0.3
}

/// Rock: craggy warped ridges, curved blocks with ledges, meandering fractures, layered
/// steps, pitting, mineral veins, lichen and water streaks.
fn rock(l: &mut Layer) -> f32 {
    let s = l.seed;
    let tile = l.tile;
    l.fill(|u, v| {
        // Heavy domain warp so nothing reads as a straight line or a cell.
        let w1 = fbm(u, v, 2, 4, 0.5, s);
        let w2 = fbm(u, v, 2, 4, 0.5, s + 1);
        let wu = u + 0.12 * w1;
        let wv = v + 0.12 * w2;
        let crag = ridged(wu, wv, 3, 6, s + 2);
        // Blocks with bent edges: tilted planes on warped cells.
        let c = cells(wu + 0.05 * w2, wv - 0.05 * w1, 4, 0.95, s + 3);
        let slope = 0.25;
        let gx = (unit(hash(c.id ^ 1)) - 0.5) * 2.0 * slope;
        let gy = (unit(hash(c.id ^ 2)) - 0.5) * 2.0 * slope;
        let cell_m = tile / 4.0;
        let block = unit(hash(c.id ^ 3)) * slope * cell_m * 0.4 + (gx * c.dx + gy * c.dy) * cell_m;
        let edge = smoothstep(0.0, 0.08, c.f2 - c.f1);
        // Meandering fractures (lines where warped noise crosses zero), broken up.
        let f1 = gnoise(wu * 6.0, wv * 6.0, 6, s + 4).abs();
        let f2 = gnoise(wu * 15.0 + 3.0 * w1, wv * 15.0, 15, s + 5).abs();
        let crack = (1.0 - smoothstep(0.0, 0.035, f1))
            * smoothstep(-0.2, 0.25, fbm(u, v, 3, 3, 0.5, s + 6))
            + (1.0 - smoothstep(0.0, 0.03, f2))
                * 0.5
                * smoothstep(0.0, 0.4, fbm(u, v, 4, 3, 0.5, s + 7));
        let crack = crack.min(1.0);
        let grain = fbm(u, v, 48, 4, 0.6, s + 8);
        let pits = cells(u, v, 96, 0.9, s + 9);
        let pit = (1.0 - smoothstep(0.12, 0.3, pits.f1))
            * smoothstep(0.1, 0.5, fbm(u, v, 5, 3, 0.5, s + 10));
        let mut h = 0.35 * crag + 0.5 * block - 0.02 * (1.0 - edge) + 0.01 * grain
            - 0.06 * crack
            - 0.004 * pit;
        // Layered steps: quantize part of the height into ledges.
        let k = h * 10.0;
        let step = (k.floor() + smoothstep(0.6, 1.0, k - k.floor())) / 10.0;
        h = lerp(h, step, 0.35);
        // Color: warm and cool grays by region and by block.
        let region = fbm(u, v, 2, 4, 0.5, s + 11) * 0.5 + 0.5;
        let mut a = mix3([0.34, 0.31, 0.27], [0.235, 0.24, 0.245], region);
        let tint = unit(hash(c.id ^ 9)) - 0.5;
        a = [a[0] * (1.0 + 0.1 * tint), a[1], a[2] * (1.0 - 0.08 * tint)];
        a = scale3(a, 0.85 + 0.2 * crag + 0.1 * grain);
        let vein = (1.0 - smoothstep(0.0, 0.02, gnoise(wu * 4.0, wv * 4.0, 4, s + 12).abs()))
            * smoothstep(0.2, 0.5, fbm(u, v, 2, 3, 0.5, s + 13));
        a = mix3(a, [0.55, 0.53, 0.5], vein * 0.55);
        a = scale3(a, 1.0 - 0.55 * crack - 0.25 * pit);
        let streak = smoothstep(0.25, 0.75, gnoise(u * 40.0, v * 3.0, 40, s + 14))
            * smoothstep(0.0, 0.5, fbm(u, v, 2, 3, 0.5, s + 15));
        a = scale3(a, 1.0 - 0.3 * streak);
        let lichen = smoothstep(0.3, 0.5, fbm(u, v, 10, 4, 0.55, s + 16))
            * smoothstep(0.05, 0.45, fbm(u, v, 3, 3, 0.5, s + 17))
            * (1.0 - crack);
        a = mix3(a, [0.37, 0.39, 0.30], lichen * 0.75);
        let orange = smoothstep(0.5, 0.65, fbm(u, v, 40, 3, 0.5, s + 18))
            * smoothstep(0.25, 0.55, fbm(u, v, 4, 3, 0.5, s + 19));
        a = mix3(a, [0.52, 0.31, 0.10], orange * 0.8);
        let r = (0.74 + 0.1 * grain + 0.1 * lichen - 0.1 * streak).clamp(0.4, 0.98);
        (h, a, r)
    });
    0.55
}

/// Snow: soft drifts, wind-carved ripples, fine grain and a few glinting crystals.
fn snow(l: &mut Layer) -> f32 {
    let s = l.seed;
    l.fill(|u, v| {
        let drift = fbm(u, v, 2, 4, 0.5, s);
        let warp = 1.6 * fbm(u, v, 2, 3, 0.5, s + 1);
        let phase = 19.0 * u + 7.0 * v + warp;
        let ripple = (0.5 + 0.5 * (phase * TAU).sin()).powf(2.5);
        let mask = smoothstep(-0.2, 0.4, fbm(u, v, 3, 3, 0.5, s + 2));
        let grain = fbm(u, v, 128, 3, 0.5, s + 3);
        let h = 0.05 * drift + 0.01 * ripple * mask + 0.0012 * grain;
        let tone = fbm(u, v, 6, 3, 0.5, s + 4) * 0.5 + 0.5;
        let a = mix3([0.86, 0.89, 0.93], [0.93, 0.94, 0.96], tone);
        // Sparkle: rare texels that are glassy ice.
        let sparkle = unit(hash2((u * 4096.0) as i32, (v * 4096.0) as i32, s + 5)) > 0.985;
        let r = if sparkle { 0.12 } else { 0.55 + 0.15 * grain };
        (h, a, r)
    });
    0.15
}

/// Sand: asymmetric wind ripples with heavy minerals in the troughs, grains and shell bits.
fn sand(l: &mut Layer) -> f32 {
    let s = l.seed;
    l.fill(|u, v| {
        let warp = 2.2 * fbm(u, v, 2, 3, 0.5, s);
        let phase = 37.0 * u + 11.0 * v + warp;
        let f = phase - phase.floor();
        // Gentle windward slope, steep lee face.
        let profile = if f < 0.72 {
            smoothstep(0.0, 0.72, f)
        } else {
            1.0 - smoothstep(0.72, 1.0, f)
        };
        let amp = 0.6 + 0.4 * fbm(u, v, 3, 3, 0.5, s + 1);
        let dune = fbm(u, v, 2, 4, 0.5, s + 2);
        let grain = gnoise(u * 900.0, v * 900.0, 900, s + 3);
        let h = 0.007 * profile * amp + 0.025 * dune + 0.0003 * grain;
        let tone = fbm(u, v, 4, 3, 0.5, s + 4) * 0.5 + 0.5;
        let mut a = mix3([0.50, 0.41, 0.29], [0.60, 0.50, 0.36], tone);
        let trough = (1.0 - profile) * smoothstep(0.0, 0.5, fbm(u, v, 3, 3, 0.5, s + 5));
        a = mix3(a, [0.33, 0.27, 0.20], trough * 0.45);
        let speck = unit(hash2((u * 2048.0) as i32, (v * 2048.0) as i32, s + 6));
        if speck > 0.95 {
            a = scale3(a, 0.45);
        } else if speck < 0.03 {
            a = [0.80, 0.77, 0.70];
        }
        (h, scale3(a, 0.95 + 0.1 * grain), 0.88)
    });
    let mut rng = Rng(s ^ 0x5a);
    let area = l.n * l.n;
    scatter_stones(
        l,
        &mut rng,
        (area / 30_000).max(2),
        (0.003, 0.01),
        0.35,
        &PEBBLES,
        (0.5, 0.8),
    );
    0.25
}

/// Gravel: stones of three sizes over dark grit, smaller ones filling the gaps.
fn gravel(l: &mut Layer) -> f32 {
    let s = l.seed;
    l.fill(|u, v| {
        let g = fbm(u, v, 64, 3, 0.5, s);
        (
            -0.004 + 0.002 * g,
            scale3([0.10, 0.085, 0.07], 0.85 + 0.3 * g),
            0.9,
        )
    });
    let mut rng = Rng(s ^ 0x6a);
    let area = l.n * l.n;
    scatter_stones(
        l,
        &mut rng,
        area / 1800,
        (0.012, 0.03),
        0.6,
        &PEBBLES,
        (0.45, 0.85),
    );
    scatter_stones(
        l,
        &mut rng,
        area / 220,
        (0.005, 0.012),
        0.6,
        &PEBBLES,
        (0.45, 0.85),
    );
    scatter_stones(
        l,
        &mut rng,
        area / 70,
        (0.002, 0.005),
        0.55,
        &PEBBLES,
        (0.5, 0.85),
    );
    0.55
}

/// Concrete slabs: stains, pores, aggregate and joints every two meters.
fn concrete(l: &mut Layer) -> f32 {
    let s = l.seed;
    let tile = l.tile;
    l.fill(|u, v| {
        let base = fbm(u, v, 8, 5, 0.5, s);
        let pores = cells(u, v, 220, 0.9, s + 1);
        let pore = (1.0 - smoothstep(0.08, 0.16, pores.f1))
            * smoothstep(0.0, 0.3, fbm(u, v, 6, 3, 0.5, s + 2));
        // Distance to the nearest slab joint (every half tile), meters.
        let to_joint = |t: f32| {
            let f = (t * 2.0).fract();
            f.min(1.0 - f) * tile * 0.5
        };
        let joint = 1.0 - smoothstep(0.002, 0.005, to_joint(u).min(to_joint(v)));
        let h = 0.0015 * base - 0.002 * pore - 0.006 * joint;
        let stain = fbm(u, v, 2, 5, 0.55, s + 3) * 0.5 + 0.5;
        let agg = unit(hash2((u * 1024.0) as i32, (v * 1024.0) as i32, s + 4));
        let mut a = scale3([0.43, 0.42, 0.40], 0.82 + 0.3 * stain);
        if agg > 0.92 {
            a = scale3(a, 0.8 + 0.4 * unit(hash(agg.to_bits())));
        }
        a = scale3(a, 1.0 - 0.5 * pore - 0.6 * joint);
        (h, a, 0.82 + 0.1 * stain)
    });
    0.3
}

/// Stone colors for pebbles and gravel: grays, browns, a few pale quartz and dark basalt.
const PEBBLES: [[f32; 3]; 8] = [
    [0.30, 0.29, 0.27],
    [0.22, 0.21, 0.20],
    [0.38, 0.34, 0.29],
    [0.26, 0.20, 0.15],
    [0.45, 0.43, 0.40],
    [0.16, 0.15, 0.15],
    [0.34, 0.24, 0.17],
    [0.55, 0.53, 0.50],
];

/// Rounded stones (or crumbs) with radii between `r` meters, each with its own color from
/// `colors`, speckle and roughness (within `polish`). `rise` is how far a stone stands out
/// of the ground (fraction of its radius).
fn scatter_stones(
    l: &mut Layer,
    rng: &mut Rng,
    count: usize,
    r: (f32, f32),
    rise: f32,
    colors: &[[f32; 3]],
    polish: (f32, f32),
) {
    let texel = l.texel();
    let n = l.n as f32;
    for _ in 0..count {
        let (x, y) = (rng.range(0.0, n), rng.range(0.0, n));
        let radius_m = lerp(r.0, r.1, rng.next().powf(1.5));
        let rad = radius_m / texel;
        let elong = rng.range(1.0, 1.6);
        let a = rng.range(0.0, TAU);
        let (ca, sa) = (a.cos(), a.sin());
        let col = scale3(rng.pick(colors), rng.range(0.8, 1.2));
        let rough = rng.range(polish.0, polish.1);
        let base = l.height_at(x, y) - radius_m * 0.2;
        let top = radius_m * rise * rng.range(0.7, 1.1);
        let seed = hash(rng.0);
        let lumpy = rng.range(0.05, 0.2);
        l.stamp(x, y, rad * elong + 1.0, |dx, dy| {
            let along = (dx * ca + dy * sa) / (rad * elong);
            let across = (-dx * sa + dy * ca) / rad;
            let wobble = 1.0 + lumpy * gnoise(dx / rad * 1.5, dy / rad * 1.5, 1 << 20, seed);
            let rho2 = (along * along + across * across) / (wobble * wobble);
            if rho2 >= 1.0 {
                return None;
            }
            let dome = (1.0 - rho2).sqrt().powf(0.7);
            let speck = gnoise(dx * 0.9, dy * 0.9, 1 << 20, seed ^ 7);
            let c = scale3(col, 0.9 + 0.12 * speck - 0.15 * rho2);
            Some((base + top * dome + radius_m * 0.2, c, rough))
        });
    }
}

fn range(v: &[f32]) -> (f32, f32) {
    v.iter()
        .fold((f32::MAX, f32::MIN), |(lo, hi), &x| (lo.min(x), hi.max(x)))
}

// ------------------------------------------------------------------------------ finishing

/// Seamless box blur (two passes per axis approximate a Gaussian), radius in texels.
fn blur(src: &[f32], n: usize, radius: usize) -> Vec<f32> {
    let mut a = src.to_vec();
    let mut b = vec![0.0; n * n];
    let r = radius.max(1) as i32;
    let norm = 1.0 / (2 * r + 1) as f32;
    let ni = n as i32;
    for _ in 0..2 {
        // Rows.
        for y in 0..n {
            let row = &a[y * n..(y + 1) * n];
            let mut sum: f32 = (-r..=r).map(|k| row[k.rem_euclid(ni) as usize]).sum();
            for x in 0..n {
                b[y * n + x] = sum * norm;
                let add = row[(x as i32 + r + 1).rem_euclid(ni) as usize];
                let sub = row[(x as i32 - r).rem_euclid(ni) as usize];
                sum += add - sub;
            }
        }
        // Columns.
        for x in 0..n {
            let col = |y: i32| b[y.rem_euclid(ni) as usize * n + x];
            let mut sum: f32 = (-r..=r).map(col).sum();
            for y in 0..n {
                a[y * n + x] = sum * norm;
                sum += col(y as i32 + r + 1) - col(y as i32 - r);
            }
        }
    }
    a
}

fn to_srgb8(c: f32) -> u8 {
    let c = c.clamp(0.0, 1.0);
    let s = if c <= 0.003_130_8 {
        c * 12.92
    } else {
        1.055 * c.powf(1.0 / 2.4) - 0.055
    };
    (s * 255.0 + 0.5) as u8
}

fn to_unorm8(c: f32) -> u8 {
    (c.clamp(0.0, 1.0) * 255.0 + 0.5) as u8
}

/// Derive normals, occlusion and cavity tint from the height field, then build the mips.
fn finish(mut l: Layer, cavity: f32) -> Finished {
    let n = l.n;
    let texel = l.texel();
    let (lo, hi) = range(&l.h);
    let span = (hi - lo).max(1e-6);
    // Occlusion: how far a texel sits below its surroundings, at two radii.
    let r_small = (0.006 / texel).round().clamp(1.0, 8.0) as usize;
    let r_large = (0.05 / texel).round().clamp(2.0, 48.0) as usize;
    let small = blur(&l.h, n, r_small);
    let large = blur(&l.h, n, r_large);
    let rs_m = r_small as f32 * texel;
    let rl_m = r_large as f32 * texel;
    let mut normal = vec![[0.0f32; 3]; n * n];
    let mut ao = vec![1.0f32; n * n];
    for y in 0..n {
        for x in 0..n {
            let i = y * n + x;
            let (xi, yi) = (x as i32, y as i32);
            let dhdx = (l.h[l.idx(xi + 1, yi)] - l.h[l.idx(xi - 1, yi)]) / (2.0 * texel);
            let dhdy = (l.h[l.idx(xi, yi + 1)] - l.h[l.idx(xi, yi - 1)]) / (2.0 * texel);
            let len = (dhdx * dhdx + dhdy * dhdy + 1.0).sqrt();
            normal[i] = [-dhdx / len, -dhdy / len, 1.0 / len];
            let occ_s = ((small[i] - l.h[i]) / rs_m * 0.8).clamp(0.0, 0.5);
            let occ_l = ((large[i] - l.h[i]) / rl_m * 1.5).clamp(0.0, 0.5);
            ao[i] = (1.0 - occ_s) * (1.0 - occ_l);
            // Cavity tint: crevices darker, edges and crests lighter.
            let curv = ((l.h[i] - small[i]) / rs_m * 3.0).clamp(-0.6, 0.6);
            let tint = 1.0 + cavity * curv;
            l.albedo[i] = scale3(l.albedo[i], tint.max(0.2));
        }
    }
    let height01: Vec<f32> = l.h.iter().map(|h| (h - lo) / span).collect();

    // Mip chain in linear space; roughness widens where averaged normals disagree.
    let mip_count = n.trailing_zeros() as usize + 1;
    let mut albedo_height = Vec::with_capacity(mip_count);
    let mut normal_rough_ao = Vec::with_capacity(mip_count);
    let mut size = n;
    let mut alb = l.albedo;
    let mut hgt = height01;
    let mut nrm = normal;
    let mut rough = l.roughness;
    let mut occ = ao;
    loop {
        let mut a8 = Vec::with_capacity(size * size * 4);
        let mut b8 = Vec::with_capacity(size * size * 4);
        for i in 0..size * size {
            let [r, g, b] = alb[i];
            a8.extend_from_slice(&[to_srgb8(r), to_srgb8(g), to_srgb8(b), to_unorm8(hgt[i])]);
            let nn = nrm[i];
            b8.extend_from_slice(&[
                to_unorm8(nn[0] * 0.5 + 0.5),
                to_unorm8(nn[1] * 0.5 + 0.5),
                to_unorm8(rough[i]),
                to_unorm8(occ[i]),
            ]);
        }
        albedo_height.push(a8);
        normal_rough_ao.push(b8);
        if size == 1 {
            break;
        }
        let half = size / 2;
        let mut alb2 = vec![[0.0; 3]; half * half];
        let mut hgt2 = vec![0.0; half * half];
        let mut nrm2 = vec![[0.0; 3]; half * half];
        let mut rough2 = vec![0.0; half * half];
        let mut occ2 = vec![0.0; half * half];
        for y in 0..half {
            for x in 0..half {
                let o = y * half + x;
                let mut sum_n = [0.0f32; 3];
                let mut sum_a2 = 0.0;
                for (dx, dy) in [(0, 0), (1, 0), (0, 1), (1, 1)] {
                    let i = (2 * y + dy) * size + 2 * x + dx;
                    for c in 0..3 {
                        alb2[o][c] += alb[i][c] * 0.25;
                        sum_n[c] += nrm[i][c] * 0.25;
                    }
                    hgt2[o] += hgt[i] * 0.25;
                    occ2[o] += occ[i] * 0.25;
                    let a = rough[i] * rough[i];
                    sum_a2 += a * a * 0.25;
                }
                let len = (sum_n[0] * sum_n[0] + sum_n[1] * sum_n[1] + sum_n[2] * sum_n[2])
                    .sqrt()
                    .max(1e-4);
                nrm2[o] = [sum_n[0] / len, sum_n[1] / len, sum_n[2] / len];
                let variance = ((1.0 - len) / len).max(0.0);
                rough2[o] = (sum_a2 + variance).sqrt().sqrt().min(1.0);
            }
        }
        alb = alb2;
        hgt = hgt2;
        nrm = nrm2;
        rough = rough2;
        occ = occ2;
        size = half;
    }
    let stats = MaterialStats {
        mean_albedo: alb[0],
        mean_roughness: rough[0],
        mean_occlusion: occ[0],
        height_m: span,
    };
    Finished {
        albedo_height,
        normal_rough_ao,
        stats,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn noise_tiles_seamlessly() {
        for (u, v) in [(0.0, 0.3), (0.25, 0.0), (0.7, 0.9)] {
            let a = fbm(u, v, 4, 5, 0.5, 9);
            let b = fbm(u + 1.0, v, 4, 5, 0.5, 9);
            let c = fbm(u, v + 1.0, 4, 5, 0.5, 9);
            assert!((a - b).abs() < 1e-4 && (a - c).abs() < 1e-4, "{a} {b} {c}");
            let w0 = cells(u, v, 7, 0.9, 3);
            let w1 = cells(u + 1.0, v - 1.0, 7, 0.9, 3);
            assert!((w0.f1 - w1.f1).abs() < 1e-3 && w0.id == w1.id);
        }
    }

    #[test]
    fn every_material_bakes_with_plausible_values() {
        let baked = bake(64);
        assert_eq!(baked.albedo_height.len(), MATERIAL_COUNT);
        assert_eq!(baked.mip_count, 7);
        for (m, stats) in GroundMaterial::ALL.iter().zip(&baked.stats) {
            let mips = &baked.albedo_height[*m as usize];
            assert_eq!(mips.len(), 7);
            assert_eq!(mips[0].len(), 64 * 64 * 4);
            assert_eq!(mips[6].len(), 4);
            let lum = stats.mean_albedo[0] * 0.2126
                + stats.mean_albedo[1] * 0.7152
                + stats.mean_albedo[2] * 0.0722;
            // Real-world albedos: dark soil and humus around 0.05, fresh snow around 0.9.
            assert!((0.02..0.95).contains(&lum), "{}: albedo {lum}", m.name());
            assert!(
                (0.05..=1.0).contains(&stats.mean_roughness),
                "{}: roughness {}",
                m.name(),
                stats.mean_roughness
            );
            assert!(
                (0.4..=1.0).contains(&stats.mean_occlusion),
                "{}: occlusion {}",
                m.name(),
                stats.mean_occlusion
            );
            assert!(stats.height_m > 0.0005, "{} is flat", m.name());
        }
        let snow = baked.stats[GroundMaterial::Snow as usize].mean_albedo[1];
        let mud = baked.stats[GroundMaterial::Mud as usize].mean_albedo[1];
        assert!(snow > 0.7 && mud < 0.2, "snow {snow}, mud {mud}");
    }

    #[test]
    fn baking_is_deterministic() {
        let a = bake_one(GroundMaterial::Gravel, 32);
        let b = bake_one(GroundMaterial::Gravel, 32);
        assert_eq!(a.albedo_height, b.albedo_height);
        assert_eq!(a.normal_rough_ao, b.normal_rough_ao);
    }
}
