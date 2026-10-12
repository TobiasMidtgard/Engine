//! Small tileable textures made in code: fabric weaves, knit, fur, hair and leather.
//!
//! Each is a height field turned into a tangent-space normal map (+Y up), sometimes with an
//! albedo variation map that multiplies the material colour. Texture coordinates on the
//! walkers' meshes are in meters, so a material's `uv_scale` is `1 / tile size`.

use engine_assets::{Assets, Handle};
use engine_core::math::{Vec2, hash01};
use engine_core::prelude::World;
use engine_render::{Image, StandardMaterial};

/// Pixels per side of every texture.
const N: usize = 128;

/// A tileable height field.
struct Height(Vec<f32>);

impl Height {
    fn new(f: impl Fn(f32, f32) -> f32) -> Self {
        let mut h = Vec::with_capacity(N * N);
        for y in 0..N {
            for x in 0..N {
                h.push(f(x as f32 / N as f32, y as f32 / N as f32));
            }
        }
        Self(h)
    }

    fn at(&self, x: isize, y: isize) -> f32 {
        let n = N as isize;
        self.0[(y.rem_euclid(n) * n + x.rem_euclid(n)) as usize]
    }

    /// Normal map; `depth` is the height range in pixels' widths.
    fn normals(&self, depth: f32) -> Image {
        Image::from_fn(N as u32, N as u32, false, |x, y| {
            let (x, y) = (x as isize, y as isize);
            let dx = (self.at(x + 1, y) - self.at(x - 1, y)) * 0.5 * depth;
            let dy = (self.at(x, y + 1) - self.at(x, y - 1)) * 0.5 * depth;
            let n = engine_core::math::Vec3::new(-dx, dy, 1.0).normalize();
            let e = |v: f32| ((v * 0.5 + 0.5) * 255.0 + 0.5) as u8;
            [e(n.x), e(n.y), e(n.z), 255]
        })
    }

    /// Grey albedo multiplier: 1 at the top of the range, `low` at the bottom.
    fn albedo(&self, low: f32) -> Image {
        let (mut lo, mut hi) = (f32::MAX, f32::MIN);
        for &v in &self.0 {
            lo = lo.min(v);
            hi = hi.max(v);
        }
        Image::from_fn(N as u32, N as u32, true, |x, y| {
            let v = (self.at(x as isize, y as isize) - lo) / (hi - lo).max(1e-6);
            let g = ((low + (1.0 - low) * v) * 255.0) as u8;
            [g, g, g, 255]
        })
    }
}

/// Tileable value noise with `cells` cells across the tile.
fn vnoise(x: f32, y: f32, cells: i64, seed: u64) -> f32 {
    let (fx, fy) = (x * cells as f32, y * cells as f32);
    let (ix, iy) = (fx.floor() as i64, fy.floor() as i64);
    let (tx, ty) = (fx - ix as f32, fy - iy as f32);
    let s = |t: f32| t * t * (3.0 - 2.0 * t);
    let h = |x: i64, y: i64| hash01(x.rem_euclid(cells), y.rem_euclid(cells), 0, seed);
    let a = h(ix, iy) + (h(ix + 1, iy) - h(ix, iy)) * s(tx);
    let b = h(ix, iy + 1) + (h(ix + 1, iy + 1) - h(ix, iy + 1)) * s(tx);
    a + (b - a) * s(ty)
}

fn fbm(x: f32, y: f32, cells: i64, seed: u64) -> f32 {
    let mut sum = 0.0;
    let mut amp = 0.5;
    let mut c = cells;
    for i in 0..4 {
        sum += amp * vnoise(x, y, c, seed + i);
        amp *= 0.5;
        c *= 2;
    }
    sum
}

/// Threads: a raised cosine across each thread.
fn thread(t: f32) -> f32 {
    let f = t.fract();
    (std::f32::consts::PI * f).sin().powf(0.7)
}

/// Plain weave with `n` threads across; `rip` adds a ripstop grid every `rip` threads.
fn weave(n: usize, rip: usize) -> Height {
    Height::new(|x, y| {
        let (u, v) = (x * n as f32, y * n as f32);
        let (cu, cv) = (u.floor() as i64, v.floor() as i64);
        let over = (cu + cv) % 2 == 0;
        let warp = thread(u) * if over { 1.0 } else { 0.55 };
        let weft = thread(v) * if over { 0.55 } else { 1.0 };
        let mut h = warp.max(weft);
        if rip > 0 {
            let r = rip as i64;
            if cu % r == 0 || cv % r == 0 {
                h += 0.6 * thread(u.min(v));
            }
        }
        h + 0.15 * vnoise(x, y, 16, 3)
    })
}

/// Diagonal twill ribs (trousers).
fn twill() -> Height {
    Height::new(|x, y| {
        let d = (x + y) * 24.0;
        let rib = thread(d);
        rib + 0.25 * thread(x * 48.0) + 0.2 * vnoise(x, y, 16, 5)
    })
}

/// Knitted ribs: columns of V stitches.
fn knit() -> Height {
    Height::new(|x, y| {
        let cols = 8.0;
        let rows = 12.0;
        let cx = x * cols;
        let fx = cx.fract() - 0.5;
        let fy = (y * rows + fx.abs() * 0.8).fract();
        let stitch = (1.0 - (fx.abs() * 2.0).powi(2)).max(0.0) * (std::f32::consts::PI * fy).sin();
        let rib = if (cx.floor() as i64) % 4 == 3 {
            0.4
        } else {
            1.0
        };
        stitch * rib
    })
}

/// Strands along the tile's v direction: `count` strands of `len` (fraction of the tile).
fn strands(count: usize, len: f32, width: f32, seed: u64) -> Height {
    let mut h = vec![0.0f32; N * N];
    for i in 0..count {
        let r = |k: u64| hash01(i as i64, k as i64, 0, seed);
        let x0 = r(1) * N as f32;
        let y0 = r(2) * N as f32;
        let l = len * N as f32 * (0.6 + 0.8 * r(3));
        let bend = (r(4) - 0.5) * 3.0;
        let w = width * (0.7 + 0.6 * r(5));
        let top = 0.5 + 0.5 * r(6);
        let steps = (l * 1.5) as usize;
        for s in 0..steps {
            let t = s as f32 / steps as f32;
            let cx = x0 + bend * t * t;
            let cy = y0 + l * t;
            // Taper toward the tip.
            let height = top * (1.0 - t * 0.7);
            let rw = w * (1.0 - t * 0.5);
            for dx in -3..=3 {
                let px = cx + dx as f32;
                let d = (px.floor() + 0.5 - cx) / rw.max(0.3);
                if d.abs() > 1.0 {
                    continue;
                }
                let v = height * (1.0 - d * d).sqrt();
                let (ix, iy) = (
                    (px.floor() as isize).rem_euclid(N as isize) as usize,
                    (cy as isize).rem_euclid(N as isize) as usize,
                );
                let cell = &mut h[iy * N + ix];
                *cell = cell.max(v);
            }
        }
    }
    let mut out = Height(h);
    // A little noise under the strands so gaps aren't flat.
    for (i, v) in out.0.iter_mut().enumerate() {
        let (x, y) = ((i % N) as f32 / N as f32, (i / N) as f32 / N as f32);
        *v += 0.15 * fbm(x, y, 8, seed + 9);
    }
    out
}

/// Pebbled leather grain.
fn leather() -> Height {
    Height::new(|x, y| {
        let n = fbm(x, y, 16, 21);
        let cells = vnoise(x, y, 32, 22);
        0.6 * n + 0.4 * (1.0 - (cells - 0.5).abs() * 2.0)
    })
}

/// Sports mesh: a grid of small holes.
fn sport_mesh() -> Height {
    Height::new(|x, y| {
        let (u, v) = (
            (x * 16.0).fract() - 0.5,
            (y * 16.0 + 0.5 * (x * 16.0).floor()).fract() - 0.5,
        );
        let r = (u * u + v * v).sqrt();
        let hole = 1.0 - smooth(0.18, 0.28, r);
        1.0 - hole + 0.3 * thread(x * 64.0).max(thread(y * 64.0)) * 0.3
    })
}

fn smooth(e0: f32, e1: f32, x: f32) -> f32 {
    let t = ((x - e0) / (e1 - e0)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// Skin: pores and fine wrinkles.
fn skin() -> Height {
    Height::new(|x, y| {
        let pores = vnoise(x, y, 64, 31);
        let fine = fbm(x, y, 8, 32);
        0.5 * fine - 0.3 * smooth(0.75, 0.95, pores)
    })
}

/// Fur locks for shells: RGB a grey albedo variation, alpha the strand height (0..1), in
/// tapering clumps on a jittered `cells`×`cells` grid, drawn out `stretch` times longer down
/// the texture (v, along the limbs and the body) the way combed fur lies.
fn locks(cells: i64, stretch: f32, seed: u64) -> Image {
    let n = N as f32;
    let mut height = vec![0.0f32; N * N];
    let mut tone = vec![0.0f32; N * N];
    let cell = n / cells as f32;
    let wrap = |v: isize| v.rem_euclid(N as isize);
    for cy in 0..cells {
        for cx in 0..cells {
            for layer in 0..2 {
                let r = |k: i64| hash01(cx, cy, k + 10 * layer, seed);
                let (x0, y0) = ((cx as f32 + r(1)) * cell, (cy as f32 + r(2)) * cell);
                let radius = cell * (0.6 + 0.35 * r(3));
                let len = 0.6 + 0.4 * r(4);
                let shade = 0.86 + 0.14 * r(5);
                let (rx, ry) = (
                    radius.ceil() as isize + 1,
                    (radius * stretch).ceil() as isize + 1,
                );
                for dy in -ry..=ry {
                    for dx in -rx..=rx {
                        let (px, py) = (x0.floor() as isize + dx, y0.floor() as isize + dy);
                        let off = Vec2::new(px as f32 + 0.5 - x0, (py as f32 + 0.5 - y0) / stretch);
                        let d = off.length() / radius;
                        if d >= 1.0 {
                            continue;
                        }
                        // Each lock tapers to a point; strands in it (streaks down v) vary.
                        let strand = 0.78
                            + 0.22
                                * hash01(wrap(px) as i64, wrap(py).div_euclid(5) as i64, 3, seed);
                        let h = len * (1.0 - d * d).sqrt() * strand;
                        let i = wrap(py) as usize * N + wrap(px) as usize;
                        if h > height[i] {
                            height[i] = h;
                            tone[i] = shade * (0.92 + 0.08 * strand);
                        }
                    }
                }
            }
        }
    }
    Image::from_fn(N as u32, N as u32, true, |x, y| {
        let i = y as usize * N + x as usize;
        let g = (tone[i] * 255.0) as u8;
        [g, g, g, (height[i].clamp(0.0, 1.0) * 255.0) as u8]
    })
}

/// Long hair for cards (manes, tails): strands running down the texture (v), each ending at
/// its own length; alpha is coverage, RGB a grey albedo variation.
fn hair_strands(count: usize, seed: u64) -> Image {
    let mut cover = vec![0.0f32; N * N];
    let mut tone = vec![0.0f32; N * N];
    for i in 0..count {
        let r = |k: i64| hash01(i as i64, k, 0, seed);
        let x0 = r(1) * N as f32;
        let end = (0.55 + 0.45 * r(2)).min(0.985) * N as f32;
        let wave = (r(3) - 0.5) * 4.0;
        let width = 0.6 + 0.6 * r(4);
        let shade = 0.6 + 0.4 * r(5);
        for y in 0..end as usize {
            let t = y as f32 / end;
            let cx = x0 + wave * (t * 3.0).sin();
            let w = width * (1.0 - 0.6 * t * t);
            for dx in -2..=2 {
                let px = cx.floor() + dx as f32;
                let d = ((px + 0.5 - cx) / w.max(0.35)).abs();
                if d >= 1.0 {
                    continue;
                }
                let a = (1.0 - d * d) * (1.0 - smooth(0.85, 1.0, t) * 0.7);
                let k = y * N + (px as isize).rem_euclid(N as isize) as usize;
                if a > cover[k] {
                    cover[k] = a;
                    tone[k] = shade;
                }
            }
        }
    }
    Image::from_fn(N as u32, N as u32, true, |x, y| {
        let i = y as usize * N + x as usize;
        let g = (tone[i] * 255.0) as u8;
        [g, g, g, (cover[i].clamp(0.0, 1.0) * 255.0) as u8]
    })
}

/// The textures the walkers use (handles into `Assets<Image>`; `None` without a renderer).
#[derive(Clone, Copy, Debug, Default)]
pub struct Textures {
    pub ripstop: Option<Handle<Image>>,
    pub cordura: Option<Handle<Image>>,
    pub twill: Option<Handle<Image>>,
    pub knit: Option<Handle<Image>>,
    pub fur: Option<Handle<Image>>,
    pub fur_albedo: Option<Handle<Image>>,
    pub coat: Option<Handle<Image>>,
    pub hair: Option<Handle<Image>>,
    pub hair_albedo: Option<Handle<Image>>,
    pub leather: Option<Handle<Image>>,
    pub sport: Option<Handle<Image>>,
    pub skin: Option<Handle<Image>>,
    pub locks: Option<Handle<Image>>,
    pub hair_cards: Option<Handle<Image>>,
}

impl Textures {
    /// Make (or reuse) every texture.
    pub fn get(world: &mut World) -> Self {
        let Some(mut images) = world.get_resource_mut::<Assets<Image>>() else {
            return Self::default();
        };
        let mut make = |key: &str, f: &dyn Fn() -> Image| Some(images.get_or_insert_keyed(key, f));
        Self {
            ripstop: make("audio/trail/ripstop", &|| weave(32, 8).normals(1.5)),
            cordura: make("audio/trail/cordura", &|| weave(24, 0).normals(2.0)),
            twill: make("audio/trail/twill", &|| twill().normals(1.2)),
            knit: make("audio/trail/knit", &|| knit().normals(3.0)),
            fur: make("audio/trail/fur", &|| {
                strands(900, 0.35, 1.3, 41).normals(2.5)
            }),
            fur_albedo: make("audio/trail/fur_albedo", &|| {
                strands(900, 0.35, 1.3, 41).albedo(0.62)
            }),
            coat: make("audio/trail/coat", &|| {
                strands(1400, 0.18, 0.9, 43).normals(0.8)
            }),
            hair: make("audio/trail/hair", &|| {
                strands(1200, 0.6, 1.0, 47).normals(2.5)
            }),
            hair_albedo: make("audio/trail/hair_albedo", &|| {
                strands(1200, 0.6, 1.0, 47).albedo(0.55)
            }),
            leather: make("audio/trail/leather", &|| leather().normals(1.2)),
            sport: make("audio/trail/sport_mesh", &|| sport_mesh().normals(1.5)),
            skin: make("audio/trail/skin", &|| skin().normals(0.6)),
            locks: make("audio/trail/locks", &|| locks(14, 3.0, 51)),
            hair_cards: make("audio/trail/hair_cards", &|| hair_strands(700, 53)),
        }
    }
}

/// Add a normal map tiled every `tile` meters.
pub fn normal(
    m: StandardMaterial,
    tex: Option<Handle<Image>>,
    tile: f32,
    scale: f32,
) -> StandardMaterial {
    match tex {
        Some(t) => {
            let mut m = m
                .with_normal_texture(t)
                .with_uv_scale(1.0 / tile, 1.0 / tile);
            m.normal_scale = scale;
            m
        }
        None => m,
    }
}

/// Add an albedo variation map (same tiling as the normal map).
pub fn albedo(m: StandardMaterial, tex: Option<Handle<Image>>) -> StandardMaterial {
    match tex {
        Some(t) => m.with_base_color_texture(t),
        None => m,
    }
}
