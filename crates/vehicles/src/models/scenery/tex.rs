//! Procedural PBR textures for the scenery: asphalt (plain and grooved), cast concrete,
//! concrete pavement slabs with joints, road paint with wear, gravel, packed dirt, galvanized
//! steel, ribbed cladding, grime, chain-link fence, tire marks, soot, painted glyphs and
//! sponsor boards.
//!
//! Every texture tiles seamlessly over a stated real-world size, so meshes carry UVs in meters
//! and materials set `uv_scale = 1 / tile size`. [`bake`] evaluates a per-texel function on
//! all cores and derives the normal map from the height channel.

use engine_render::Image;

use super::font;
use super::noise::{Cell, fbm, gradient, hash2, mix, rand1, rand2, smoothstep, value, worley};

/// One texel of a material: linear albedo, alpha, height (meters), roughness, metallic and
/// ambient occlusion.
#[derive(Clone, Copy, Debug)]
pub struct Texel {
    pub rgb: [f32; 3],
    pub alpha: f32,
    pub height: f32,
    pub rough: f32,
    pub metal: f32,
    pub ao: f32,
}

impl Default for Texel {
    fn default() -> Self {
        Self {
            rgb: [0.5; 3],
            alpha: 1.0,
            height: 0.0,
            rough: 0.8,
            metal: 0.0,
            ao: 1.0,
        }
    }
}

/// The images of one baked material.
pub struct Maps {
    /// sRGB albedo with alpha.
    pub base: Image,
    /// Tangent-space normal map (+Y up), when asked for.
    pub normal: Option<Image>,
    /// R = occlusion, G = roughness, B = metallic, when asked for.
    pub orm: Option<Image>,
}

/// What to bake.
#[derive(Clone, Copy, Debug)]
pub struct Spec {
    pub width: u32,
    pub height: u32,
    /// Real-world size the texture covers (meters, x and y), for texel positions and normal
    /// strength.
    pub size_m: [f32; 2],
    pub normal: bool,
    pub orm: bool,
}

impl Spec {
    /// A square texture covering `meters`, with normal and ORM maps.
    pub fn square(px: u32, meters: f32) -> Self {
        Self {
            width: px,
            height: px,
            size_m: [meters, meters],
            normal: true,
            orm: true,
        }
    }

    /// Base color only.
    pub fn color_only(mut self) -> Self {
        self.normal = false;
        self.orm = false;
        self
    }
}

fn srgb_lut() -> &'static [u8; 4096] {
    static LUT: std::sync::OnceLock<[u8; 4096]> = std::sync::OnceLock::new();
    LUT.get_or_init(|| {
        let mut t = [0u8; 4096];
        for (i, v) in t.iter_mut().enumerate() {
            let l = i as f32 / 4095.0;
            let s = if l <= 0.003_130_8 {
                l * 12.92
            } else {
                1.055 * l.powf(1.0 / 2.4) - 0.055
            };
            *v = (s * 255.0 + 0.5).clamp(0.0, 255.0) as u8;
        }
        t
    })
}

#[inline]
fn to_srgb8(l: f32) -> u8 {
    srgb_lut()[(l.clamp(0.0, 1.0) * 4095.0) as usize]
}

#[inline]
fn to_u8(v: f32) -> u8 {
    (v.clamp(0.0, 1.0) * 255.0 + 0.5) as u8
}

/// A band of rows handed to a bake thread: its first row, the albedo bytes and the
/// optional ORM bytes and heights.
type Band<'a> = (
    usize,
    &'a mut [u8],
    Option<&'a mut [u8]>,
    Option<&'a mut [f32]>,
);

/// Evaluate `f(x_m, y_m)` at every texel center (meters inside the tile, y down the image)
/// on all cores and pack the results into images.
pub fn bake(spec: Spec, f: impl Fn(f32, f32) -> Texel + Sync) -> Maps {
    let (w, h) = (spec.width as usize, spec.height as usize);
    let px = [spec.size_m[0] / w as f32, spec.size_m[1] / h as f32];
    let mut base = vec![0u8; w * h * 4];
    let mut orm = if spec.orm {
        vec![0u8; w * h * 4]
    } else {
        Vec::new()
    };
    let mut height = if spec.normal {
        vec![0f32; w * h]
    } else {
        Vec::new()
    };
    let threads = std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(4)
        .clamp(1, 8);
    let rows = h.div_ceil(threads * 4).max(1);
    let f = &f;
    std::thread::scope(|scope| {
        let base_chunks = base.chunks_mut(rows * w * 4);
        let mut orm_chunks = orm.chunks_mut((rows * w * 4).max(1));
        let mut height_chunks = height.chunks_mut((rows * w).max(1));
        let mut handles = Vec::new();
        let mut row0 = 0usize;
        // Work is split into small row bands handed out round-robin to a few threads.
        let mut bands: Vec<Vec<Band<'_>>> = (0..threads).map(|_| Vec::new()).collect();
        for (k, b) in base_chunks.enumerate() {
            let o = if spec.orm { orm_chunks.next() } else { None };
            let hgt = if spec.normal {
                height_chunks.next()
            } else {
                None
            };
            let n = b.len() / (w * 4);
            bands[k % threads].push((row0, b, o, hgt));
            row0 += n;
        }
        for list in bands {
            handles.push(scope.spawn(move || {
                for (row0, b, mut o, mut hgt) in list {
                    let n = b.len() / (w * 4);
                    for r in 0..n {
                        let y = row0 + r;
                        let ym = (y as f32 + 0.5) * px[1];
                        for x in 0..w {
                            let xm = (x as f32 + 0.5) * px[0];
                            let t = f(xm, ym);
                            let i = (r * w + x) * 4;
                            b[i] = to_srgb8(t.rgb[0]);
                            b[i + 1] = to_srgb8(t.rgb[1]);
                            b[i + 2] = to_srgb8(t.rgb[2]);
                            b[i + 3] = to_u8(t.alpha);
                            if let Some(o) = o.as_deref_mut() {
                                o[i] = to_u8(t.ao);
                                o[i + 1] = to_u8(t.rough);
                                o[i + 2] = to_u8(t.metal);
                                o[i + 3] = 255;
                            }
                            if let Some(hh) = hgt.as_deref_mut() {
                                hh[r * w + x] = t.height;
                            }
                        }
                    }
                }
            }));
        }
        for h in handles {
            let _ = h.join();
        }
    });
    let normal = spec.normal.then(|| {
        let mut n = vec![0u8; w * h * 4];
        let at = |x: isize, y: isize| -> f32 {
            let xx = x.rem_euclid(w as isize) as usize;
            let yy = y.rem_euclid(h as isize) as usize;
            height[yy * w + xx]
        };
        for y in 0..h as isize {
            for x in 0..w as isize {
                // Sobel-weighted slopes (meters per meter).
                let dx = ((at(x + 1, y) - at(x - 1, y)) * 2.0 + at(x + 1, y - 1)
                    - at(x - 1, y - 1)
                    + at(x + 1, y + 1)
                    - at(x - 1, y + 1))
                    / (8.0 * px[0]);
                let dy = ((at(x, y + 1) - at(x, y - 1)) * 2.0 + at(x - 1, y + 1)
                    - at(x - 1, y - 1)
                    + at(x + 1, y + 1)
                    - at(x + 1, y - 1))
                    / (8.0 * px[1]);
                // Tangent space: +X = +u, +Y = up the image (toward decreasing v).
                let (nx, ny, nz) = (-dx, dy, 1.0);
                let l = (nx * nx + ny * ny + nz * nz).sqrt();
                let i = (y as usize * w + x as usize) * 4;
                n[i] = to_u8(nx / l * 0.5 + 0.5);
                n[i + 1] = to_u8(ny / l * 0.5 + 0.5);
                n[i + 2] = to_u8(nz / l * 0.5 + 0.5);
                n[i + 3] = 255;
            }
        }
        Image::new(spec.width, spec.height, n, false)
    });
    Maps {
        base: Image::new(spec.width, spec.height, base, true),
        normal,
        orm: spec
            .orm
            .then(|| Image::new(spec.width, spec.height, orm, false)),
    }
}

/// Number of lattice cells of size `cell_m` that tile `tile_m` exactly (at least 1).
fn cells(tile_m: f32, cell_m: f32) -> i32 {
    (tile_m / cell_m).round().max(1.0) as i32
}

fn lerp3(a: [f32; 3], b: [f32; 3], t: f32) -> [f32; 3] {
    [mix(a[0], b[0], t), mix(a[1], b[1], t), mix(a[2], b[2], t)]
}

fn scale3(a: [f32; 3], s: f32) -> [f32; 3] {
    [a[0] * s, a[1] * s, a[2] * s]
}

// ---------------------------------------------------------------- road surfaces

/// Dense asphalt: dark binder with 3–10 mm aggregate, tonal blotches, a few hairline cracks.
/// With `grooves` (pitch in meters) it gets transverse runway grooves 6 mm wide running
/// along the texture's x axis (constant y).
pub fn asphalt(px: u32, tile_m: f32, grooves: Option<f32>, seed: u32) -> Maps {
    let fine = cells(tile_m, 0.0065);
    let coarse = cells(tile_m, 0.016);
    let blot = cells(tile_m, 0.7);
    let big = cells(tile_m, 2.6);
    let crack_p = cells(tile_m, 1.6);
    let groove_n = grooves.map(|p| cells(tile_m, p));
    bake(Spec::square(px, tile_m), move |x, y| {
        let (u, v) = (x / tile_m, y / tile_m);
        // Aggregate.
        let c = worley(u * fine as f32, v * fine as f32, (fine, fine), 0.95, seed);
        let r = 0.30 + 0.16 * ((c.id >> 3) & 255) as f32 / 255.0;
        let stone = smoothstep(r + 0.08, r - 0.06, c.f1);
        let shade = ((c.id >> 11) & 1023) as f32 / 1023.0;
        let mut tone = 0.07 + 0.22 * shade * shade;
        // A few bright quartz and warm stones.
        if (c.id >> 21) & 15 == 0 {
            tone = 0.36;
        }
        let k = worley(
            u * coarse as f32,
            v * coarse as f32,
            (coarse, coarse),
            0.9,
            seed ^ 0x9e37,
        );
        let big_stone = if (k.id & 3) == 0 {
            smoothstep(0.36, 0.24, k.f1)
        } else {
            0.0
        };
        let big_tone = 0.08 + 0.16 * ((k.id >> 8) & 255) as f32 / 255.0;
        let binder = 0.032;
        let mut a = mix(binder, tone, stone * 0.85);
        a = mix(a, big_tone, big_stone);
        // Tonal blotches: binder wear, fines, old spills.
        let b1 = fbm(
            u * blot as f32,
            v * blot as f32,
            (blot, blot),
            4,
            0.55,
            seed + 7,
        );
        let b2 = fbm(u * big as f32, v * big as f32, (big, big), 3, 0.5, seed + 9);
        let mut gray = a * (1.0 + 0.22 * b1 + 0.16 * b2);
        // Hairline cracks: zero lines of warped noise, only in some places.
        let wx = gradient(u * 7.0, v * 7.0, (7, 7), seed + 21) * 0.08;
        let n = gradient(
            (u + wx) * crack_p as f32,
            (v - wx) * crack_p as f32,
            (crack_p, crack_p),
            seed + 23,
        );
        let mask = smoothstep(0.25, 0.5, fbm(u * 3.0, v * 3.0, (3, 3), 2, 0.5, seed + 25));
        let crack = smoothstep(0.022, 0.004, n.abs()) * mask;
        gray = mix(gray, 0.012, crack * 0.9);
        let mut height = stone * 0.0016 + big_stone * 0.0022 - crack * 0.003 + b1 * 0.0006;
        let mut ao = 1.0 - (1.0 - stone) * 0.25 - crack * 0.5;
        let mut rough = mix(0.93, 0.74, stone.max(big_stone));
        if let Some(gn) = groove_n {
            let g = (v * gn as f32).fract();
            let groove = smoothstep(0.0, 0.04, g) * smoothstep(0.2, 0.16, g);
            height -= groove * 0.004;
            ao *= 1.0 - groove * 0.45;
            gray *= 1.0 - groove * 0.35;
            rough = mix(rough, 0.95, groove);
        }
        let warm = 1.0 + 0.04 * b2;
        Texel {
            rgb: [gray * warm, gray, gray / warm],
            height,
            rough,
            ao,
            ..Default::default()
        }
    })
}

/// Cast concrete (walls, barriers, footings, buildings): light gray with mottling, sand grain
/// and small air pores.
pub fn concrete(px: u32, tile_m: f32, seed: u32) -> Maps {
    let m1 = cells(tile_m, 0.9);
    let m2 = cells(tile_m, 0.18);
    let pore = cells(tile_m, 0.009);
    bake(Spec::square(px, tile_m), move |x, y| {
        let (u, v) = (x / tile_m, y / tile_m);
        let a = fbm(u * m1 as f32, v * m1 as f32, (m1, m1), 4, 0.5, seed);
        let b = fbm(u * m2 as f32, v * m2 as f32, (m2, m2), 3, 0.5, seed + 1);
        let c = worley(
            u * pore as f32,
            v * pore as f32,
            (pore, pore),
            1.0,
            seed + 2,
        );
        let is_pore = if c.id & 7 == 0 {
            smoothstep(0.22, 0.12, c.f1)
        } else {
            0.0
        };
        let grain = rand2((x * 977.0) as i32, (y * 977.0) as i32, seed) - 0.5;
        let g = 0.34 * (1.0 + 0.10 * a + 0.05 * b + 0.05 * grain) * (1.0 - 0.55 * is_pore);
        Texel {
            rgb: [g * 1.02, g, g * 0.96],
            height: a * 0.0008 + b * 0.0003 - is_pore * 0.0015 + grain * 0.0001,
            rough: 0.86 + 0.06 * b,
            ao: 1.0 - is_pore * 0.5,
            ..Default::default()
        }
    })
}

/// Concrete pavement: a 2 × 2 block of `panel_m` slabs with sealed expansion joints on the
/// tile borders and through the middle, broomed surface texture (alternating direction per
/// slab), corner cracks and stains. Covers `2 * panel_m`.
pub fn slabs(px: u32, panel_m: f32, seed: u32) -> Maps {
    let tile = panel_m * 2.0;
    let m1 = cells(tile, 1.2);
    let m2 = cells(tile, 0.25);
    let broom = cells(tile, 0.012);
    let agg = cells(tile, 0.007);
    bake(Spec::square(px, tile), move |x, y| {
        let (u, v) = (x / tile, y / tile);
        let (pi, pj) = ((x / panel_m) as i32, (y / panel_m) as i32);
        let (lx, ly) = (x - pi as f32 * panel_m, y - pj as f32 * panel_m);
        let edge = lx.min(panel_m - lx).min(ly).min(panel_m - ly);
        let panel_tone = rand2(pi, pj, seed) - 0.5;
        // Joint sealant 12 mm wide, with chipped arrises beside it.
        let joint = smoothstep(0.0075, 0.0045, edge);
        let chip_n = value(
            x * 60.0,
            y * 60.0,
            (cells(tile, 1.0 / 60.0), cells(tile, 1.0 / 60.0)),
            seed + 3,
        );
        let chip = smoothstep(0.03, 0.008, edge) * smoothstep(0.62, 0.8, chip_n);
        let a = fbm(u * m1 as f32, v * m1 as f32, (m1, m1), 4, 0.5, seed + 1);
        let b = fbm(u * m2 as f32, v * m2 as f32, (m2, m2), 3, 0.5, seed + 2);
        // Broom striations: across the slab, alternating per slab.
        let across = (pi + pj) % 2 == 0;
        let (s1, s2) = if across { (x, y) } else { (y, x) };
        let stria = value(
            s2 / tile * broom as f32,
            s1 / tile * 6.0,
            (broom, 6),
            seed + 4 + (pi * 3 + pj) as u32,
        ) - 0.5;
        let c = worley(u * agg as f32, v * agg as f32, (agg, agg), 1.0, seed + 5);
        let speck = if c.id & 3 == 0 {
            smoothstep(0.3, 0.15, c.f1)
        } else {
            0.0
        };
        // Diagonal corner cracks in some slabs.
        let mut crack = 0.0;
        if hash2(pi, pj, seed + 9) & 3 == 0 {
            let w = gradient(
                x * 2.0,
                y * 2.0,
                (cells(tile, 0.5), cells(tile, 0.5)),
                seed + 10,
            ) * 0.06;
            let d = ((lx + ly + w) - panel_m * 0.55).abs();
            let reach = smoothstep(panel_m * 0.75, panel_m * 0.45, (lx - ly).abs() + lx.min(ly));
            crack = smoothstep(0.004, 0.001, d) * reach;
        }
        // Oil and fuel stains.
        let stain = smoothstep(0.35, 0.65, fbm(u * 3.0, v * 3.0, (3, 3), 3, 0.5, seed + 11));
        let mut g = 0.36 * (1.0 + 0.09 * a + 0.04 * b + 0.08 * panel_tone + 0.05 * stria);
        g = mix(g, 0.30, speck * 0.4);
        g *= 1.0 - 0.25 * stain;
        g *= 1.0 - 0.25 * chip;
        g = mix(g, 0.03, joint);
        g = mix(g, 0.05, crack);
        Texel {
            rgb: [g * 1.02, g, g * 0.95],
            height: a * 0.0008 + stria * 0.0004 - joint * 0.006 - chip * 0.004 - crack * 0.002,
            rough: mix(0.82 + 0.06 * b, 0.62, stain * 0.6).min(1.0) * (1.0 - joint * 0.25),
            ao: 1.0 - joint * 0.6 - crack * 0.4 - chip * 0.3,
            ..Default::default()
        }
    })
}

/// Road paint on top of a surface: white with dirt, worn through in patches and specks
/// (alpha = paint left). Shares the UVs and normal map of the surface below. Covers `tile_m`.
pub fn paint(px: u32, tile_m: f32, wear: f32, seed: u32) -> Maps {
    let p1 = cells(tile_m, 0.35);
    let p2 = cells(tile_m, 0.05);
    let p3 = cells(tile_m, 0.012);
    let mut spec = Spec::square(px, tile_m);
    spec.normal = false;
    bake(spec, move |x, y| {
        let (u, v) = (x / tile_m, y / tile_m);
        let a = fbm(u * p1 as f32, v * p1 as f32, (p1, p1), 4, 0.55, seed);
        let b = value(u * p2 as f32, v * p2 as f32, (p2, p2), seed + 1);
        let c = value(u * p3 as f32, v * p3 as f32, (p3, p3), seed + 2);
        let worn = a * 0.7 + (b - 0.5) * 0.35 + (c - 0.5) * 0.3;
        let alpha = smoothstep(wear + 0.05, wear - 0.05, worn);
        let dirt = 1.0 - 0.18 * smoothstep(-0.4, 0.5, a) - 0.08 * (c - 0.5);
        Texel {
            rgb: [0.86 * dirt, 0.86 * dirt, 0.84 * dirt],
            alpha,
            rough: 0.55 + 0.15 * smoothstep(-0.2, 0.6, a),
            ..Default::default()
        }
    })
}

/// Curb paint: near-white with rubber streaks, grime in the dips and fine paint texture
/// (vertex colors give the red and white).
pub fn curb_paint(px: u32, tile_m: f32, seed: u32) -> Maps {
    let p = cells(tile_m, 0.4);
    let fine = cells(tile_m, 0.01);
    bake(Spec::square(px, tile_m), move |x, y| {
        let (u, v) = (x / tile_m, y / tile_m);
        let a = fbm(u * p as f32, v * p as f32, (p, p), 4, 0.5, seed);
        // Tire streaks run along the curb (x).
        let streak = value(u * 3.0, v * 90.0, (3, 90), seed + 1);
        let rubber = smoothstep(0.55, 0.9, streak) * smoothstep(-0.2, 0.4, a);
        let grain = value(u * fine as f32, v * fine as f32, (fine, fine), seed + 2) - 0.5;
        let g = (0.95 - 0.12 * smoothstep(0.0, 0.7, a) + 0.04 * grain) * (1.0 - 0.75 * rubber);
        Texel {
            rgb: [g, g, g],
            height: grain * 0.0004 + a * 0.0005,
            rough: 0.5 + 0.2 * rubber + 0.08 * grain,
            ao: 1.0,
            ..Default::default()
        }
    })
}

/// Rounded river gravel, 8–20 mm, for run-off areas. Covers `tile_m`.
pub fn gravel(px: u32, tile_m: f32, seed: u32) -> Maps {
    let n1 = cells(tile_m, 0.017);
    let n2 = cells(tile_m, 0.0095);
    let blot = cells(tile_m, 0.5);
    const PALETTE: [[f32; 3]; 6] = [
        [0.42, 0.37, 0.30],
        [0.33, 0.31, 0.29],
        [0.50, 0.46, 0.40],
        [0.27, 0.21, 0.16],
        [0.56, 0.53, 0.48],
        [0.38, 0.33, 0.26],
    ];
    bake(Spec::square(px, tile_m), move |x, y| {
        let (u, v) = (x / tile_m, y / tile_m);
        let pebble = |c: Cell, size: f32| -> (f32, f32, u32) {
            let r = 0.36 + 0.14 * ((c.id >> 4) & 255) as f32 / 255.0;
            // Elliptical pebbles: squash along a per-stone direction.
            let ang = ((c.id >> 12) & 255) as f32 / 255.0 * std::f32::consts::PI;
            let (s, co) = ang.sin_cos();
            let a = c.dx * co + c.dy * s;
            let b = (-c.dx * s + c.dy * co) * 1.35;
            let d = (a * a + b * b).sqrt() / r;
            let dome = (1.0 - d * d).max(0.0).sqrt();
            (dome, dome * r * size, c.id)
        };
        let c1 = worley(u * n1 as f32, v * n1 as f32, (n1, n1), 1.0, seed);
        let c2 = worley(u * n2 as f32, v * n2 as f32, (n2, n2), 1.0, seed + 1);
        let (d1, h1, id1) = pebble(c1, 0.017);
        let (d2, h2, id2) = pebble(c2, 0.0095);
        let (dome, height, id) = if h1 >= h2 * 0.9 {
            (d1, h1, id1)
        } else {
            (d2, h2, id2)
        };
        let base = PALETTE[(id >> 24) as usize % PALETTE.len()];
        let tone = 0.8 + 0.4 * ((id >> 16) & 255) as f32 / 255.0;
        let b = fbm(
            u * blot as f32,
            v * blot as f32,
            (blot, blot),
            3,
            0.5,
            seed + 2,
        );
        let covered = smoothstep(0.0, 0.35, dome);
        let gap = [0.05, 0.045, 0.04];
        let rgb = lerp3(gap, scale3(base, tone * (1.0 + 0.12 * b)), covered);
        Texel {
            rgb,
            height,
            rough: mix(0.95, 0.72, covered),
            ao: mix(0.35, 1.0, covered.sqrt()),
            ..Default::default()
        }
    })
}

/// Packed earth with small stones and dried cracks (the drift pad). Covers `tile_m`.
pub fn dirt(px: u32, tile_m: f32, seed: u32) -> Maps {
    let m1 = cells(tile_m, 1.1);
    let m2 = cells(tile_m, 0.2);
    let st = cells(tile_m, 0.014);
    let cr = cells(tile_m, 0.22);
    let gp = cells(tile_m, 0.16);
    bake(Spec::square(px, tile_m), move |x, y| {
        let (u, v) = (x / tile_m, y / tile_m);
        let a = fbm(u * m1 as f32, v * m1 as f32, (m1, m1), 4, 0.5, seed);
        let b = fbm(u * m2 as f32, v * m2 as f32, (m2, m2), 4, 0.55, seed + 1);
        let dry = smoothstep(-0.3, 0.5, a);
        let wet = [0.16, 0.105, 0.065];
        let dusty = [0.34, 0.26, 0.18];
        let mut rgb = lerp3(wet, dusty, dry);
        rgb = scale3(rgb, 1.0 + 0.12 * b);
        let c = worley(u * st as f32, v * st as f32, (st, st), 1.0, seed + 2);
        let stone = if c.id & 3 == 0 {
            smoothstep(0.32, 0.18, c.f1)
        } else {
            0.0
        };
        let stone_tone = 0.25 + 0.25 * ((c.id >> 8) & 255) as f32 / 255.0;
        rgb = lerp3(
            rgb,
            [stone_tone, stone_tone * 0.93, stone_tone * 0.85],
            stone * 0.8,
        );
        // Dried cracks only in patches of the driest earth, and broken up so they never
        // form a complete net of tiles.
        let k = worley(u * cr as f32, v * cr as f32, (cr, cr), 0.8, seed + 3);
        let patch = fbm(u * m1 as f32, v * m1 as f32, (m1, m1), 3, 0.5, seed + 4);
        let gaps = fbm(u * gp as f32, v * gp as f32, (gp, gp), 3, 0.5, seed + 5);
        let crack = smoothstep(0.035, 0.008, k.f2 - k.f1)
            * smoothstep(0.35, 0.8, dry)
            * smoothstep(0.05, 0.45, patch)
            * smoothstep(-0.1, 0.25, gaps);
        rgb = scale3(rgb, 1.0 - 0.4 * crack);
        Texel {
            rgb,
            height: b * 0.004 + stone * 0.004 - crack * 0.004 + a * 0.003,
            rough: 0.93 - 0.06 * (1.0 - dry),
            ao: 1.0 - crack * 0.5 - (1.0 - stone) * 0.05,
            ..Default::default()
        }
    })
}

/// Mown grass seen from above: three layers of blades (short strokes at random angles) over
/// dark soil, lush and dry tufts, a few clover leaves. Covers `tile_m`.
pub fn grass(px: u32, tile_m: f32, seed: u32) -> Maps {
    let layers: [(f32, u32); 3] = [(0.035, 0), (0.024, 1), (0.017, 2)];
    let blot = cells(tile_m, 0.6);
    let clov = cells(tile_m, 0.09);
    bake(Spec::square(px, tile_m), move |x, y| {
        let (u, v) = (x / tile_m, y / tile_m);
        let mut cover = 0.0f32;
        let mut tone = 0.0f32;
        let mut layer_h = 0.0f32;
        for (cell_m, li) in layers {
            let n = cells(tile_m, cell_m);
            let (gx, gy) = (u * n as f32, v * n as f32);
            let (ix, iy) = (gx.floor() as i32, gy.floor() as i32);
            for dy in -1..=1 {
                for dx in -1..=1 {
                    let (cx, cy) = (ix + dx, iy + dy);
                    let (wx, wy) = (cx.rem_euclid(n), cy.rem_euclid(n));
                    let s = seed + li * 101;
                    let h = hash2(wx, wy, s);
                    let ang = (h & 1023) as f32 / 1023.0 * std::f32::consts::PI;
                    let len = 0.55 + 0.6 * ((h >> 10) & 255) as f32 / 255.0;
                    let ox = cx as f32 + rand2(wx, wy, s + 1);
                    let oy = cy as f32 + rand2(wx, wy, s + 2);
                    let (dxv, dyv) = (ang.cos() * len * 0.5, ang.sin() * len * 0.5);
                    // Distance to the blade's segment.
                    let (px_, py_) = (gx - ox, gy - oy);
                    let t = ((px_ * dxv + py_ * dyv) / (dxv * dxv + dyv * dyv)).clamp(-1.0, 1.0);
                    let (qx, qy) = (px_ - dxv * t, py_ - dyv * t);
                    let d = (qx * qx + qy * qy).sqrt();
                    let w = 0.09 * (1.0 - t.abs() * 0.6);
                    let c = smoothstep(w, w * 0.4, d);
                    if c > 0.0 && li as f32 + c > layer_h {
                        layer_h = li as f32 + c;
                        cover = cover.max(c);
                        tone = ((h >> 18) & 255) as f32 / 255.0 * 0.7 + 0.3 * (1.0 - t.abs());
                    }
                }
            }
        }
        let b = fbm(
            u * blot as f32,
            v * blot as f32,
            (blot, blot),
            4,
            0.55,
            seed + 7,
        );
        let dry = smoothstep(0.1, 0.7, b);
        let lush = [0.04, 0.1, 0.018];
        let mid = [0.07, 0.14, 0.028];
        let pale = [0.14, 0.16, 0.06];
        let mut rgb = lerp3(lerp3(lush, mid, tone), pale, dry * (0.4 + 0.6 * tone));
        // Clover leaves: rounder, bluer green.
        let k = worley(
            u * clov as f32,
            v * clov as f32,
            (clov, clov),
            0.9,
            seed + 9,
        );
        let clover = if k.id & 7 == 0 {
            smoothstep(0.32, 0.22, k.f1)
        } else {
            0.0
        };
        rgb = lerp3(rgb, [0.05, 0.11, 0.035], clover);
        let soil = [0.035, 0.026, 0.018];
        let covered = cover.max(clover);
        rgb = lerp3(soil, rgb, smoothstep(0.0, 0.5, covered).max(0.35));
        Texel {
            rgb,
            height: layer_h * 0.004 + clover * 0.006,
            rough: mix(0.95, 0.78, covered),
            ao: mix(0.45, 1.0, (layer_h / 3.0).min(1.0).max(covered * 0.6)),
            ..Default::default()
        }
    })
}

// ---------------------------------------------------------------- metals and buildings

/// Hot-dip galvanized steel: spangle grains, dull patches, a little white rust. Covers
/// `tile_m`.
pub fn galvanized(px: u32, tile_m: f32, seed: u32) -> Maps {
    let sp = cells(tile_m, 0.02);
    let m = cells(tile_m, 0.3);
    bake(Spec::square(px, tile_m), move |x, y| {
        let (u, v) = (x / tile_m, y / tile_m);
        let c = worley(u * sp as f32, v * sp as f32, (sp, sp), 1.0, seed);
        let grain = ((c.id >> 8) & 255) as f32 / 255.0;
        let a = fbm(u * m as f32, v * m as f32, (m, m), 4, 0.5, seed + 1);
        let rust = smoothstep(0.35, 0.6, a);
        let g = mix(0.55 + 0.12 * grain, 0.62, rust);
        Texel {
            rgb: [g, g * 1.01, g * 1.03],
            height: (grain - 0.5) * 0.00005 + rust * 0.0002,
            rough: mix(0.28 + 0.2 * grain, 0.8, rust),
            metal: mix(1.0, 0.2, rust),
            ao: 1.0,
            ..Default::default()
        }
    })
}

/// Grime for painted structures: an albedo multiplier around 0.85–1, rougher dirty patches,
/// vertical run-off streaks (y down the texture). Covers `tile_m`.
pub fn grime(px: u32, tile_m: f32, seed: u32) -> Maps {
    let m = cells(tile_m, 0.8);
    let f = cells(tile_m, 0.05);
    bake(Spec::square(px, tile_m), move |x, y| {
        let (u, v) = (x / tile_m, y / tile_m);
        let a = fbm(u * m as f32, v * m as f32, (m, m), 4, 0.5, seed);
        let streak = value(u * 40.0, v * 2.0, (40, 2), seed + 1);
        let s = smoothstep(0.55, 0.95, streak) * 0.5;
        let fine = value(u * f as f32, v * f as f32, (f, f), seed + 2) - 0.5;
        let dirt = smoothstep(0.0, 0.8, a) * 0.6 + s * 0.4;
        let g = 1.0 - 0.18 * dirt + 0.03 * fine;
        Texel {
            rgb: [g, g * 0.99, g * 0.97],
            height: fine * 0.0002 + a * 0.0004,
            rough: 0.42 + 0.3 * dirt + 0.05 * fine,
            ao: 1.0,
            ..Default::default()
        }
    })
}

/// Trapezoidal steel cladding: ribs every `pitch_m` along x, running along y, 35 mm deep,
/// with two stiffening minor ribs in each pan and soft vertical grime. White (tint it with
/// vertex colors). Covers `pitch_m * ribs` square.
pub fn cladding(px: u32, pitch_m: f32, ribs: u32, seed: u32) -> Maps {
    let tile = pitch_m * ribs as f32;
    bake(Spec::square(px, tile), move |x, y| {
        let p = (x / pitch_m).fract();
        // Profile: crest (0..0.18), web (0.18..0.3), pan (0.3..0.88), web (0.88..1).
        let crest = smoothstep(0.30, 0.20, p) + smoothstep(0.88, 0.98, p);
        let minor = (0.0f32).max(1.0 - ((p - 0.5).abs() / 0.03)) * 0.12
            + (0.0f32).max(1.0 - ((p - 0.68).abs() / 0.03)) * 0.12;
        let h = crest.min(1.0) * 0.035 + minor * 0.004;
        let (u, v) = (x / tile, y / tile);
        let streak = value(u * ribs as f32 * 3.0, v * 3.0, (ribs as i32 * 3, 3), seed);
        let a = fbm(u * 4.0, v * 4.0, (4, 4), 4, 0.5, seed + 1);
        let dirt = smoothstep(0.5, 0.9, streak) * 0.5 + smoothstep(0.1, 0.8, a) * 0.5;
        let g = 0.92 - 0.2 * dirt;
        Texel {
            rgb: [g, g, g * 0.98],
            height: h,
            rough: 0.38 + 0.25 * dirt,
            ao: 1.0 - (1.0 - crest.min(1.0)) * 0.12,
            ..Default::default()
        }
    })
}

/// Chain-link fence: 50 mm diamonds of 3.5 mm galvanized wire (alpha = wire). Covers
/// `tile_m` (a multiple of the 50 mm mesh).
pub fn chain_link(px: u32, tile_m: f32) -> Maps {
    let n = cells(tile_m, 0.05) as f32;
    let aa = 1.5 / px as f32 * n;
    bake(Spec::square(px, tile_m), move |x, y| {
        let (u, v) = (x / tile_m * n, y / tile_m * n);
        // Diagonals at ±45° through the cell grid: lines where u + v or u − v is an integer.
        let d1 = ((u + v) - (u + v).round()).abs();
        let d2 = ((u - v) - (u - v).round()).abs();
        let w = 0.05;
        let a = smoothstep(w + aa, w - aa, d1).max(smoothstep(w + aa, w - aa, d2));
        // Wire twists at the knuckles read slightly darker.
        let g = 0.62 - 0.1 * smoothstep(0.2, 0.0, d1.min(d2));
        Texel {
            rgb: [g, g, g * 1.02],
            alpha: a.max(0.0),
            height: a * 0.002,
            rough: 0.4,
            metal: 1.0,
            ao: 1.0,
        }
    })
}

/// Pressure-locked steel bar grating: 5 mm bearing bars every 33 mm along y, cross bars every
/// 100 mm, sooty and heat-tinted (alpha = steel, holes transparent). Covers `tile_m`.
pub fn grating(px: u32, tile_m: f32, seed: u32) -> Maps {
    // Opaque: the gaps are painted as the dark void below, so the deck keeps its look in the
    // mips (alpha-tested 6 mm bars vanish a few meters away).
    let bars = cells(tile_m, 0.04) as f32;
    let cross = cells(tile_m, 0.1) as f32;
    let aa = 1.2 / px as f32;
    bake(Spec::square(px, tile_m), move |x, y| {
        let (u, v) = (x / tile_m, y / tile_m);
        let fu = (u * bars).fract();
        let fv = (v * cross).fract();
        let bw = 0.008 / tile_m * bars;
        let cw = 0.006 / tile_m * cross;
        let bar = smoothstep(bw * 0.5 + aa * bars, bw * 0.5 - aa * bars, (fu - 0.5).abs());
        let xb = smoothstep(
            cw * 0.5 + aa * cross,
            cw * 0.5 - aa * cross,
            (fv - 0.5).abs(),
        );
        let a = bar.max(xb);
        let soot = fbm(u * 6.0, v * 6.0, (6, 6), 4, 0.5, seed) * 0.5 + 0.5;
        let heat = smoothstep(0.55, 0.8, fbm(u * 3.0, v * 3.0, (3, 3), 3, 0.5, seed + 1));
        let rust = smoothstep(0.62, 0.85, fbm(u * 9.0, v * 9.0, (9, 9), 3, 0.5, seed + 2));
        let g = mix(0.42, 0.12, soot);
        let steel = lerp3([g, g, g * 1.02], [g * 1.25, g * 0.95, g * 0.7], heat);
        let steel = lerp3(steel, [0.3, 0.16, 0.08], rust * 0.6);
        // Light catching the top edge of each bar.
        let edge = smoothstep(bw * 0.5, 0.0, (fu - 0.5).abs()) * 0.15;
        let steel = [steel[0] + edge, steel[1] + edge, steel[2] + edge];
        let void = [0.018, 0.017, 0.016];
        Texel {
            rgb: lerp3(void, steel, a),
            height: bar * 0.006 + xb * 0.004,
            rough: mix(1.0, mix(0.5, 0.9, soot.max(rust)), a),
            metal: mix(0.0, mix(0.9, 0.35, soot.max(rust)), a),
            ao: mix(0.25, 1.0, a),
            ..Default::default()
        }
    })
}

// ---------------------------------------------------------------- decals

/// A tire mark: the strip's width along x (0..1), `length_m` along y. Alpha = rubber laid
/// down (tread grooves lighter, intermittent along the strip, soft edges).
pub fn skid(w: u32, h: u32, length_m: f32, seed: u32) -> Maps {
    let spec = Spec {
        width: w,
        height: h,
        size_m: [1.0, length_m],
        normal: false,
        orm: false,
    };
    let along = cells(length_m, 0.5);
    bake(spec, move |x, y| {
        let v = y / length_m;
        let edge = smoothstep(0.0, 0.12, x) * smoothstep(1.0, 0.88, x);
        // Four tread grooves.
        let mut tread = 1.0;
        for g in [0.22, 0.42, 0.6, 0.78] {
            tread -= 0.55 * smoothstep(0.035, 0.01, (x - g).abs());
        }
        let streak = value(x * 64.0, v * 6.0, (64, 6), seed);
        let along_n = fbm(0.5, v * along as f32, (1, along), 3, 0.5, seed + 1);
        let a = edge * tread.max(0.2) * (0.55 + 0.45 * streak) * smoothstep(-0.6, 0.2, along_n);
        Texel {
            rgb: [0.012, 0.011, 0.010],
            alpha: a.clamp(0.0, 1.0),
            ..Default::default()
        }
    })
}

/// Rocket soot around a pad's center: the texture covers a disc (center at the middle),
/// alpha = soot, with radial streaks, a heat-tinted ring and a dark core.
pub fn soot(px: u32, seed: u32) -> Maps {
    let spec = Spec {
        width: px,
        height: px,
        size_m: [1.0, 1.0],
        normal: false,
        orm: false,
    };
    bake(spec, move |x, y| {
        let (dx, dy) = (x * 2.0 - 1.0, y * 2.0 - 1.0);
        let r = (dx * dx + dy * dy).sqrt();
        let ang = dy.atan2(dx) / std::f32::consts::TAU + 0.5;
        let rays = fbm(ang * 48.0, r * 3.0, (48, 1 << 16), 4, 0.6, seed) * 0.5 + 0.5;
        let fine = value(x * 300.0, y * 300.0, (300, 300), seed + 1);
        let blot = fbm(x * 8.0, y * 8.0, (8, 8), 4, 0.5, seed + 2);
        let reach = 0.55 + 0.4 * rays + 0.1 * blot;
        let a = smoothstep(reach, reach * 0.35, r) * (0.75 + 0.25 * fine) * (0.85 + 0.3 * blot);
        let core = smoothstep(0.35, 0.05, r);
        let ring = smoothstep(0.25, 0.4, r) * smoothstep(0.75, 0.5, r);
        let rgb = lerp3(
            lerp3([0.02, 0.018, 0.016], [0.07, 0.05, 0.035], ring),
            [0.008, 0.008, 0.008],
            core,
        );
        Texel {
            rgb,
            alpha: (a * 0.95).clamp(0.0, 1.0),
            ..Default::default()
        }
    })
}

// ---------------------------------------------------------------- glyphs and signs

/// Characters in the [`glyph_atlas`], in cell order.
pub const GLYPHS: &str = "0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZ-./<>^";
/// Atlas layout: columns × rows of glyph cells.
pub const GLYPH_GRID: (u32, u32) = (16, 3);

/// UV rectangle `[u0, v0, u1, v1]` of `c` in the glyph atlas: the cell maps the glyph box
/// (4 × 6 units) with a 1-unit margin on every side.
pub fn glyph_uv(c: char) -> Option<[f32; 4]> {
    let i = GLYPHS.find(c.to_ascii_uppercase())? as u32;
    let (col, row) = (i % GLYPH_GRID.0, i / GLYPH_GRID.0);
    let (cw, ch) = (1.0 / GLYPH_GRID.0 as f32, 1.0 / GLYPH_GRID.1 as f32);
    Some([
        col as f32 * cw,
        row as f32 * ch,
        (col + 1) as f32 * cw,
        (row + 1) as f32 * ch,
    ])
}

/// Painted glyphs (runway designators, grid numbers, pad codes): white strokes on
/// transparent, worn like road paint. Each cell holds one glyph with a 1-unit margin.
pub fn glyph_atlas(cell_px: u32, weight: f32, seed: u32) -> Maps {
    let (cols, rows) = GLYPH_GRID;
    let (cw, chh) = (cell_px, cell_px * 8 / 6);
    let spec = Spec {
        width: cw * cols,
        height: chh * rows,
        size_m: [1.0, 1.0],
        normal: false,
        orm: true,
    };
    let (w, h) = (spec.width as f32, spec.height as f32);
    let units_per_px = 6.0 / cw as f32;
    bake(spec, move |x, y| {
        let (px, py) = (x * w, y * h);
        let (col, row) = ((px / cw as f32) as u32, (py / chh as f32) as u32);
        let i = (row * cols + col) as usize;
        let Some(c) = GLYPHS.chars().nth(i) else {
            return Texel {
                alpha: 0.0,
                ..Default::default()
            };
        };
        // Glyph units: 6 wide (4 + margins), 8 tall (6 + margins), y up.
        let gx = (px - col as f32 * cw as f32) * units_per_px - 1.0;
        let gy = 7.0 - (py - row as f32 * chh as f32) * units_per_px;
        let ink = font::coverage(c, gx, gy, weight, units_per_px * 1.2);
        let worn = fbm(x * 40.0, y * 20.0, (40, 20), 4, 0.55, seed) * 0.7
            + (value(x * 900.0, y * 450.0, (900, 450), seed + 1) - 0.5) * 0.4;
        let paint = smoothstep(0.42, 0.32, worn);
        Texel {
            rgb: [0.86, 0.86, 0.84],
            alpha: ink * paint,
            rough: 0.6,
            ..Default::default()
        }
    })
}

/// One board in the [`sign_atlas`].
#[derive(Clone, Copy, Debug)]
pub struct Board {
    pub text: &'static str,
    /// Linear background colors (left to right gradient).
    pub bg: [[f32; 3]; 2],
    pub ink: [f32; 3],
    /// Logo mark drawn left of the text: 0 none, 1 disc, 2 chevrons, 3 stripes, 4 ring.
    pub mark: u32,
    pub mark_color: [f32; 3],
}

/// Fictional sponsors and track boards, in atlas order (2 columns × 8 rows).
pub const BOARDS: [Board; 16] = [
    board(
        "VELOCE",
        [0.55, 0.02, 0.02],
        [0.30, 0.01, 0.01],
        [0.95, 0.95, 0.92],
        2,
        [0.95, 0.75, 0.05],
    ),
    board(
        "APEX OIL",
        [0.01, 0.06, 0.25],
        [0.01, 0.02, 0.10],
        [0.95, 0.80, 0.10],
        1,
        [0.90, 0.30, 0.02],
    ),
    board(
        "NORDLINE",
        [0.90, 0.90, 0.88],
        [0.75, 0.76, 0.78],
        [0.02, 0.05, 0.20],
        3,
        [0.02, 0.25, 0.60],
    ),
    board(
        "KESTREL",
        [0.02, 0.02, 0.02],
        [0.06, 0.06, 0.07],
        [0.98, 0.98, 0.98],
        2,
        [0.85, 0.05, 0.05],
    ),
    board(
        "TITAN TYRES",
        [0.95, 0.70, 0.02],
        [0.85, 0.45, 0.01],
        [0.02, 0.02, 0.02],
        4,
        [0.02, 0.02, 0.02],
    ),
    board(
        "HELIX",
        [0.02, 0.20, 0.10],
        [0.01, 0.08, 0.04],
        [0.92, 0.95, 0.90],
        1,
        [0.60, 0.85, 0.10],
    ),
    board(
        "ORBITAL",
        [0.05, 0.02, 0.12],
        [0.20, 0.03, 0.35],
        [0.95, 0.95, 0.98],
        4,
        [0.30, 0.70, 1.00],
    ),
    board(
        "QUANTA",
        [0.92, 0.92, 0.92],
        [0.92, 0.92, 0.92],
        [0.80, 0.02, 0.10],
        3,
        [0.80, 0.02, 0.10],
    ),
    board(
        "MERIDIAN",
        [0.01, 0.10, 0.30],
        [0.01, 0.25, 0.55],
        [0.98, 0.98, 0.98],
        0,
        [0.0, 0.0, 0.0],
    ),
    board(
        "PULSAR",
        [0.02, 0.02, 0.02],
        [0.02, 0.02, 0.02],
        [0.10, 0.85, 0.95],
        2,
        [0.10, 0.85, 0.95],
    ),
    board(
        "FULCRUM",
        [0.80, 0.25, 0.02],
        [0.55, 0.12, 0.01],
        [0.98, 0.98, 0.98],
        1,
        [0.98, 0.98, 0.98],
    ),
    board(
        "AURORA",
        [0.90, 0.90, 0.92],
        [0.70, 0.80, 0.95],
        [0.03, 0.10, 0.35],
        4,
        [0.85, 0.20, 0.55],
    ),
    board(
        "A",
        [0.01, 0.01, 0.01],
        [0.01, 0.01, 0.01],
        [0.95, 0.75, 0.05],
        0,
        [0.0, 0.0, 0.0],
    ),
    board(
        "18-36",
        [0.60, 0.02, 0.02],
        [0.60, 0.02, 0.02],
        [0.98, 0.98, 0.98],
        0,
        [0.0, 0.0, 0.0],
    ),
    board(
        "A>",
        [0.95, 0.70, 0.02],
        [0.95, 0.70, 0.02],
        [0.01, 0.01, 0.01],
        0,
        [0.0, 0.0, 0.0],
    ),
    board(
        "DANGER ROCKET BLAST",
        [0.95, 0.75, 0.02],
        [0.95, 0.75, 0.02],
        [0.01, 0.01, 0.01],
        3,
        [0.01, 0.01, 0.01],
    ),
];

const fn board(
    text: &'static str,
    a: [f32; 3],
    b: [f32; 3],
    ink: [f32; 3],
    mark: u32,
    mark_color: [f32; 3],
) -> Board {
    Board {
        text,
        bg: [a, b],
        ink,
        mark,
        mark_color,
    }
}

/// Index of the board showing `text`.
pub fn board_index(text: &str) -> Option<usize> {
    BOARDS.iter().position(|b| b.text == text)
}

/// UV rectangle of board `i` in the sign atlas (2 columns × 8 rows; each board 4:1).
pub fn board_uv(i: usize) -> [f32; 4] {
    let (col, row) = ((i % 2) as f32, (i / 2) as f32);
    [col * 0.5, row / 8.0, (col + 1.0) * 0.5, (row + 1.0) / 8.0]
}

/// The sign atlas: 16 boards of 4:1 (sponsors, taxiway signs, hazard boards) with a thin
/// frame, weathered a little.
pub fn sign_atlas(px: u32, seed: u32) -> Maps {
    let spec = Spec {
        width: px,
        height: px,
        size_m: [1.0, 1.0],
        normal: false,
        orm: true,
    };
    let fpx = px as f32;
    bake(spec, move |x, y| {
        let col = (x * 2.0) as usize;
        let row = (y * 8.0) as usize;
        let i = (row * 2 + col).min(15);
        let b = BOARDS[i];
        // Board-local: s 0..4 across, t 0..1 down.
        let s = (x * 2.0 - col as f32) * 4.0;
        let t = y * 8.0 - row as f32;
        let mut rgb = lerp3(b.bg[0], b.bg[1], s / 4.0);
        let frame = (s.min(4.0 - s)).min(t.min(1.0 - t));
        let units = 1.0 / 9.0; // board heights per glyph unit (6-unit caps on 0.67 of it)
        let has_mark = b.mark != 0;
        let text_w = font::text_width(b.text) * units;
        let left = if has_mark { 1.15 } else { 0.0 };
        let avail = 4.0 - left - 0.25;
        let scale = (avail / text_w).min(1.0);
        let tw = text_w * scale;
        let x0 = left + (avail - tw) * 0.5 + if has_mark { 0.0 } else { 0.125 };
        let gx = (s - x0) / (units * scale);
        let gy = (0.5 + 3.0 * units * scale - t) / (units * scale);
        let aa = 1.4 / (fpx / 2.0 / 4.0) / (units * scale);
        let ink = font::text_coverage(b.text, gx, gy, 1.05, aa);
        rgb = lerp3(rgb, b.ink, ink);
        if has_mark {
            let (mx, my) = (0.6, 0.5);
            let (dx, dy) = (s - mx, t - my);
            let r = (dx * dx + dy * dy).sqrt();
            let m = match b.mark {
                1 => smoothstep(0.34, 0.32, r),
                2 => {
                    let k = ((dx + dy.abs() * 0.9) * 4.0).fract();
                    if r < 0.42 && k < 0.45 { 1.0 } else { 0.0 }
                }
                3 => {
                    let k = ((dx - dy) * 5.0).fract();
                    if dx.abs() < 0.4 && dy.abs() < 0.38 && k < 0.5 {
                        1.0
                    } else {
                        0.0
                    }
                }
                _ => smoothstep(0.04, 0.02, (r - 0.3).abs()),
            };
            rgb = lerp3(rgb, b.mark_color, m);
        }
        // Frame and weathering.
        let edge = smoothstep(0.03, 0.02, frame);
        rgb = lerp3(rgb, [0.6, 0.6, 0.6], edge);
        let dirt = fbm(x * 16.0, y * 16.0, (16, 16), 4, 0.5, seed) * 0.5 + 0.5;
        let fade = 1.0 - 0.12 * dirt * smoothstep(0.3, 1.0, t);
        rgb = scale3(rgb, fade);
        let noise = rand1((x * fpx) as i32 + (y * fpx) as i32 * 4096, seed) - 0.5;
        Texel {
            rgb: scale3(rgb, 1.0 + noise * 0.03),
            rough: 0.45 + 0.2 * dirt,
            ..Default::default()
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bakes_tile_and_have_sensible_values() {
        let m = asphalt(64, 8.0, Some(0.038), 1);
        assert_eq!(m.base.width, 64);
        assert!(m.normal.is_some() && m.orm.is_some());
        // Asphalt is dark: mean sRGB value well under mid gray.
        let mean: f32 = m.base.data.chunks(4).map(|p| p[0] as f32).sum::<f32>() / (64.0 * 64.0);
        assert!(mean > 25.0 && mean < 110.0, "{mean}");
        let n = m.normal.unwrap();
        // Normals point mostly out of the surface.
        assert!(n.data.chunks(4).all(|p| p[2] > 128));
        let g = glyph_atlas(32, 0.9, 3);
        let uv = glyph_uv('8').unwrap();
        assert!(uv[2] > uv[0] && uv[3] > uv[1]);
        assert!(g.base.data.chunks(4).any(|p| p[3] > 200));
        assert!(board_index("A").is_some());
    }
}
