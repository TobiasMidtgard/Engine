//! Sprites, sprite sheets, tile maps and pixel-perfect cameras, with all art drawn by code
//! here: a 2D side-scroller (a pixel-art tile map, parallax layers, an animated hero, a slime
//! and coins, seen through a `PixelPerfect` camera) and a 2.5D diorama (lit paper sprites and
//! billboard trees among 3D meshes, casting and receiving shadows), by day, at sunset and by
//! lantern light at night.
//!
//! ```sh
//! cargo run -p engine_render --release --example sprites_2d -- [out-dir]
//! ```
//!
//! Saves `side_scroller_a.png`, `side_scroller_b.png`, `diorama_day.png`, `diorama_dusk.png`
//! and `diorama_night.png` (1280 × 720) to `out-dir` (default `sprites_2d`); a second
//! argument (`side,day,dusk,night`) picks scenes.

use std::f32::consts::{FRAC_PI_2, PI};
use std::path::Path;

use engine_assets::{Assets, Handle};
use engine_core::prelude::*;
use engine_core::time::TimeSource;
use engine_render::prelude::*;
use engine_render::{TILE_FLIP_X, capture_now};

// ---------------------------------------------------------------------------------------------
// Pixel canvas, noise and palette.

type Rgba = [u8; 4];

const fn hex(v: u32) -> Rgba {
    [(v >> 16) as u8, (v >> 8) as u8, v as u8, 255]
}

const CLEAR: Rgba = [0, 0, 0, 0];
// A sixteen-color palette (Sweetie 16) plus earth, bark and skin tones.
const INK: Rgba = hex(0x1a1c2c);
const PLUM: Rgba = hex(0x5d275d);
const RED: Rgba = hex(0xb13e53);
const ORANGE: Rgba = hex(0xef7d57);
const YELLOW: Rgba = hex(0xffcd75);
const LIME: Rgba = hex(0xa7f070);
const GREEN: Rgba = hex(0x38b764);
const TEAL: Rgba = hex(0x257179);
const NAVY: Rgba = hex(0x29366f);
const BLUE: Rgba = hex(0x3b5dc9);
const SKY: Rgba = hex(0x41a6f6);
const CYAN: Rgba = hex(0x73eff7);
const WHITE: Rgba = hex(0xf4f4f4);
const SILVER: Rgba = hex(0x94b0c2);
const SLATE: Rgba = hex(0x566c86);
const STEEL: Rgba = hex(0x333c57);
const BARK_DARK: Rgba = hex(0x3b2027);
const BARK: Rgba = hex(0x5e3a2e);
const DIRT: Rgba = hex(0x8a5638);
const SAND: Rgba = hex(0xb47a4a);
const TAN: Rgba = hex(0xd9a066);
const SKIN: Rgba = hex(0xf3b98a);
const SKIN_SHADE: Rgba = hex(0xc98a64);
const HAIR: Rgba = hex(0x6b3e2e);
const DEEP: Rgba = hex(0x262b44);

fn with_alpha(c: Rgba, a: u8) -> Rgba {
    [c[0], c[1], c[2], a]
}

/// An RGBA texel grid, y down (row 0 at the top), like the images it becomes.
#[derive(Clone)]
struct Canvas {
    w: i32,
    h: i32,
    px: Vec<Rgba>,
}

impl Canvas {
    fn new(w: i32, h: i32) -> Self {
        Self {
            w,
            h,
            px: vec![CLEAR; (w * h) as usize],
        }
    }

    fn get(&self, x: i32, y: i32) -> Rgba {
        if x < 0 || y < 0 || x >= self.w || y >= self.h {
            CLEAR
        } else {
            self.px[(y * self.w + x) as usize]
        }
    }

    fn opaque(&self, x: i32, y: i32) -> bool {
        self.get(x, y)[3] > 0
    }

    fn set(&mut self, x: i32, y: i32, c: Rgba) {
        if x >= 0 && y >= 0 && x < self.w && y < self.h {
            self.px[(y * self.w + x) as usize] = c;
        }
    }

    /// Set, wrapping around the edges (for seamless tiles and repeating layers).
    fn set_wrap(&mut self, x: i32, y: i32, c: Rgba) {
        self.set(x.rem_euclid(self.w), y.rem_euclid(self.h), c);
    }

    fn rect(&mut self, x: i32, y: i32, w: i32, h: i32, c: Rgba) {
        for yy in y..y + h {
            for xx in x..x + w {
                self.set(xx, yy, c);
            }
        }
    }

    fn ellipse(&mut self, cx: f32, cy: f32, rx: f32, ry: f32, c: Rgba) {
        for y in (cy - ry).floor() as i32..=(cy + ry).ceil() as i32 {
            for x in (cx - rx).floor() as i32..=(cx + rx).ceil() as i32 {
                let dx = (x as f32 + 0.5 - cx) / rx.max(0.01);
                let dy = (y as f32 + 0.5 - cy) / ry.max(0.01);
                if dx * dx + dy * dy <= 1.0 {
                    self.set(x, y, c);
                }
            }
        }
    }

    fn disc(&mut self, cx: f32, cy: f32, r: f32, c: Rgba) {
        self.ellipse(cx, cy, r, r, c);
    }

    /// A line drawn with a `width × width` square brush.
    fn line(&mut self, a: (f32, f32), b: (f32, f32), width: i32, c: Rgba) {
        let n = ((b.0 - a.0).abs().max((b.1 - a.1).abs()).ceil() as i32).max(1);
        for i in 0..=n {
            let t = i as f32 / n as f32;
            let x = (a.0 + (b.0 - a.0) * t).floor() as i32;
            let y = (a.1 + (b.1 - a.1) * t).floor() as i32;
            self.rect(x - (width - 1) / 2, y - (width - 1) / 2, width, width, c);
        }
    }

    /// Paint the transparent texels that touch opaque ones (4-neighbours) with `c`.
    fn outline(&mut self, c: Rgba) {
        let src = self.clone();
        for y in 0..self.h {
            for x in 0..self.w {
                if !src.opaque(x, y)
                    && [(1, 0), (-1, 0), (0, 1), (0, -1)]
                        .iter()
                        .any(|&(dx, dy)| src.opaque(x + dx, y + dy))
                {
                    self.set(x, y, c);
                }
            }
        }
    }

    /// Copy the opaque texels of `src` with its top-left corner at (`x`, `y`).
    fn blit(&mut self, src: &Canvas, x: i32, y: i32) {
        for sy in 0..src.h {
            for sx in 0..src.w {
                let p = src.get(sx, sy);
                if p[3] > 0 {
                    self.set(x + sx, y + sy, p);
                }
            }
        }
    }

    fn image(&self) -> Image {
        Image::new(
            self.w as u32,
            self.h as u32,
            self.px.iter().flatten().copied().collect(),
            true,
        )
    }
}

/// A hash of a lattice point: 0..1.
fn hash(x: i32, y: i32, seed: u32) -> f32 {
    let mut h = (x as u32).wrapping_mul(0x8da6_b343)
        ^ (y as u32).wrapping_mul(0xd816_3841)
        ^ seed.wrapping_mul(0xcb1a_b31f).wrapping_add(0x9e37_79b9);
    h ^= h >> 13;
    h = h.wrapping_mul(0x5bd1_e995);
    h ^= h >> 15;
    h = h.wrapping_mul(0x27d4_eb2d);
    h ^= h >> 16;
    (h & 0xff_ffff) as f32 / 16_777_216.0
}

/// Smooth value noise along `x` (one lattice cell per unit), repeating every `period` cells.
fn noise1(x: f32, period: i32, seed: u32) -> f32 {
    let i = x.floor() as i32;
    let f = x - i as f32;
    let a = hash(i.rem_euclid(period), 0, seed);
    let b = hash((i + 1).rem_euclid(period), 0, seed);
    a + (b - a) * f * f * (3.0 - 2.0 * f)
}

/// Ordered (Bayer 4 × 4) dither threshold of a texel: 0..1.
fn bayer(x: i32, y: i32) -> f32 {
    const B: [[f32; 4]; 4] = [
        [0.0, 8.0, 2.0, 10.0],
        [12.0, 4.0, 14.0, 6.0],
        [3.0, 11.0, 1.0, 9.0],
        [15.0, 7.0, 13.0, 5.0],
    ];
    (B[(y & 3) as usize][(x & 3) as usize] + 0.5) / 16.0
}

fn add_image(app: &mut App, image: Image) -> Handle<Image> {
    app.world.resource_mut::<Assets<Image>>().add(image)
}

// ---------------------------------------------------------------------------------------------
// Side-scroller art: 16 texels per meter, one tile per meter.

const PPU: f32 = 16.0;

/// The sky: bands of blue dithered into each other, and the sun. The size of the view
/// (320 × 180 texels at 4 screen pixels per texel).
fn sky() -> Canvas {
    let mut c = Canvas::new(320, 180);
    let bands = [NAVY, BLUE, SKY, CYAN, hex(0xc9f6ee)];
    for y in 0..c.h {
        for x in 0..c.w {
            let t = (y as f32 / 150.0).min(1.0).powf(0.9) * (bands.len() - 1) as f32;
            let i = (t.floor() as usize).min(bands.len() - 1);
            let f = t - i as f32;
            let mix = ((f - 0.5) / 0.5).clamp(0.0, 1.0);
            let col = if i + 1 < bands.len() && bayer(x, y) < mix {
                bands[i + 1]
            } else {
                bands[i]
            };
            c.set(x, y, col);
        }
    }
    // The sun with a dithered glow.
    let (sx, sy) = (246.0, 42.0);
    for y in 0..90 {
        for x in 180..320 {
            let d = ((x as f32 + 0.5 - sx).powi(2) + (y as f32 + 0.5 - sy).powi(2)).sqrt();
            if d < 26.0 && bayer(x, y) < (26.0 - d) / 14.0 * 0.6 {
                c.set(x, y, CYAN);
            }
        }
    }
    c.disc(sx, sy, 13.0, YELLOW);
    c.disc(sx - 1.0, sy - 1.0, 10.0, hex(0xfff1c2));
    c
}

/// One fluffy cloud centered at (`x`, `y`) (its flat bottom), `scale` × 60 texels wide.
fn cloud(c: &mut Canvas, x: f32, y: f32, scale: f32, seed: u32) {
    let mut mask = Canvas::new(c.w, c.h);
    let puffs = 6;
    for i in 0..puffs {
        let t = i as f32 / (puffs - 1) as f32;
        let bump = 1.0 - (2.0 * t - 1.0).powi(2);
        let r = (6.0 + 9.0 * bump + 4.0 * hash(i, 1, seed)) * scale;
        let px = x + (t - 0.5) * 56.0 * scale;
        mask.disc(px, y - r * 0.55, r, WHITE);
    }
    let bottom = y.round() as i32;
    for yy in 0..c.h {
        for xx in 0..c.w {
            if !mask.opaque(xx, yy) || yy > bottom {
                continue;
            }
            let from_bottom = (bottom - yy) as f32 / scale;
            let col = if from_bottom < 1.5 {
                SILVER
            } else if from_bottom < 6.0 {
                if bayer(xx, yy) < (6.0 - from_bottom) / 4.5 {
                    SILVER
                } else {
                    WHITE
                }
            } else {
                WHITE
            };
            c.set_wrap(xx, yy, col);
        }
    }
}

/// Clouds for a layer repeating every 640 texels (40 m).
fn clouds() -> Canvas {
    let mut c = Canvas::new(640, 64);
    for (i, &(x, y, s)) in [
        (70.0, 40.0, 1.0),
        (200.0, 26.0, 0.65),
        (330.0, 52.0, 1.25),
        (470.0, 30.0, 0.8),
        (585.0, 46.0, 0.95),
    ]
    .iter()
    .enumerate()
    {
        cloud(&mut c, x, y, s, i as u32 + 3);
    }
    c
}

/// A mountain range `w × h` texels repeating along x: peaks lit from the left, shaded on the
/// right, with snow above `snow_line` texels.
#[allow(clippy::too_many_arguments)]
fn mountains(
    w: i32,
    h: i32,
    peaks: i32,
    heights: (f32, f32),
    snow_line: f32,
    seed: u32,
    [lit, shade, snow, snow_shade]: [Rgba; 4],
) -> Canvas {
    let mut c = Canvas::new(w, h);
    let list: Vec<(f32, f32, f32)> = (0..peaks)
        .map(|i| {
            (
                (i as f32 + 0.15 + 0.7 * hash(i, 0, seed)) * w as f32 / peaks as f32,
                heights.0 + (heights.1 - heights.0) * hash(i, 1, seed),
                0.5 + 0.45 * hash(i, 2, seed),
            )
        })
        .collect();
    for x in 0..w {
        let crag = (noise1(x as f32 / 4.0, w / 4, seed + 7) - 0.5) * 4.0
            + (noise1(x as f32 / 16.0, w / 16, seed + 8) - 0.5) * 6.0;
        let mut best = (f32::MIN, 0.0);
        for &(px, ph, slope) in &list {
            for k in [-1.0, 0.0, 1.0] {
                let px = px + k * w as f32;
                let top = ph - (x as f32 + 0.5 - px).abs() * slope;
                if top > best.0 {
                    best = (top, px);
                }
            }
        }
        let (top, peak_x) = best;
        let top = top + crag;
        for y in 0..h {
            let alt = (h - y) as f32;
            if alt > top {
                continue;
            }
            let depth = top - alt;
            let wobble = (noise1(y as f32 / 2.5, h, seed + 11 + peak_x as u32) - 0.5) * 3.0;
            let lit_side = (x as f32 + 0.5) < peak_x + wobble + depth * 0.25;
            let snow_edge =
                snow_line + (noise1(x as f32 / 2.0, w / 2, seed + 5) - 0.5) * 8.0 + depth * 0.4;
            let col = match (alt > snow_edge, lit_side) {
                (true, true) => snow,
                (true, false) => snow_shade,
                (false, true) => lit,
                (false, false) => shade,
            };
            c.set(x, y, col);
        }
    }
    c
}

/// A pine silhouette standing at (`x`, `base`) (bottom center), `height` texels tall, with a
/// lighter rim on its sunny left side.
fn pine(c: &mut Canvas, x: f32, base: f32, height: f32, body: Rgba, rim: Rgba) {
    let top = base - height;
    let tiers = 3;
    for t in 0..tiers {
        let t0 = top + height * 0.28 * t as f32;
        let t1 = t0 + height * (0.42 + 0.06 * t as f32);
        let width = height * (0.2 + 0.1 * t as f32);
        for y in t0.floor() as i32..t1.min(base - 2.0).ceil() as i32 {
            let f = ((y as f32 + 0.5 - t0) / (t1 - t0)).clamp(0.0, 1.0);
            let half = (width * f).max(0.5);
            let x0 = (x - half).round() as i32;
            let x1 = (x + half).round() as i32;
            for xx in x0..=x1 {
                c.set_wrap(xx, y, body);
            }
            c.set_wrap(x0, y, rim);
        }
    }
    for y in (base - 3.0) as i32..base as i32 {
        c.set_wrap(x as i32, y, body);
    }
}

/// Forested hills repeating every `w` texels.
fn forest(w: i32, h: i32, seed: u32, body: Rgba, rim: Rgba) -> Canvas {
    let mut c = Canvas::new(w, h);
    let ground = |x: f32| h as f32 - 26.0 - 14.0 * noise1(x / 64.0, w / 64, seed);
    for x in 0..w {
        let g = ground(x as f32 + 0.5);
        for y in g.round() as i32..h {
            c.set(x, y, body);
        }
    }
    let mut x = 0.0;
    let mut i = 0;
    while x < w as f32 {
        let height = 16.0 + 18.0 * hash(i, 0, seed + 1);
        pine(&mut c, x, ground(x) + 4.0, height, body, rim);
        x += 4.0 + 7.0 * hash(i, 1, seed + 1);
        i += 1;
    }
    c
}

/// Dark foliage (bush mounds and fern fronds over a strip of grass) for a foreground layer
/// repeating every `w` texels, with a dim rim of light along its upper edges.
fn foreground_foliage(w: i32) -> Canvas {
    let h = 40;
    let mut c = Canvas::new(w, h);
    let (dark, rim) = (hex(0x0f2027), hex(0x1d3d44));
    for (i, x) in [30.0f32, 122.0, 200.0].into_iter().enumerate() {
        let i = i as i32;
        let r = 8.0 + hash(i, 0, 91) * 5.0;
        for k in -2..=2 {
            let kx = x + k as f32 * r * 0.7;
            let kr = r * (1.0 - 0.18 * (k as f32).abs()) + hash(i, k + 10, 91) * 2.0;
            let ky = h as f32 + 1.0 - kr * 0.75;
            c.disc(kx, ky, kr, dark);
            // Leafy bumps along the top.
            for b in 0..4 {
                let a = (b as f32 / 3.0 - 0.5) * 2.2 + (hash(i, k * 7 + b, 94) - 0.5) * 0.4;
                let br = 1.5 + hash(i, k * 7 + b, 95) * 1.5;
                c.disc(kx + a.sin() * kr, ky - a.cos() * kr, br, dark);
            }
        }
        // Fronds arching out of the mound, with leaflets.
        let (sx, sy) = (x, h as f32 + 4.0 - r * 1.75);
        for f in 0..5 {
            let a = (f as f32 / 4.0 - 0.5) * 1.5 + (hash(i, f + 30, 91) - 0.5) * 0.3;
            let len = 14.0 + hash(i, f + 20, 91) * 10.0;
            let mut prev = (sx, sy);
            for step in 1..=10 {
                let t = step as f32 / 10.0;
                let ang = a * (1.0 + t * 0.6);
                let p = (
                    sx + ang.sin() * len * t,
                    sy - ang.cos() * len * t + t * t * 3.0,
                );
                c.line(prev, p, if t < 0.5 { 2 } else { 1 }, dark);
                if step % 2 == 0 && t < 0.95 {
                    let side = 3.5 * (1.0 - t) + 0.5;
                    c.line(
                        p,
                        (p.0 - ang.cos() * side, p.1 - ang.sin() * side - 1.0),
                        1,
                        dark,
                    );
                    c.line(
                        p,
                        (p.0 + ang.cos() * side, p.1 + ang.sin() * side - 1.0),
                        1,
                        dark,
                    );
                }
                prev = p;
            }
        }
    }
    for x in 0..w {
        let spike = if hash(x, 0, 93) < 0.3 { 2 } else { 0 };
        let gh = (2.0 + 4.0 * noise1(x as f32 / 8.0, w / 8, 92)) as i32 + spike;
        c.rect(x, h - gh, 1, gh, dark);
    }
    let src = c.clone();
    for y in 0..h {
        for x in 0..w {
            if src.opaque(x, y) && !src.opaque(x, y - 1) && !src.opaque(x - 1, y - 1) {
                c.set(x, y, rim);
            }
        }
    }
    c
}

// Tile set frames (8 columns of 16 × 16 texels).
const GRASS: u32 = 0;
const GRASS_LEFT: u32 = 1;
const DIRT_A: u32 = 2;
const DIRT_B: u32 = 3;
const DIRT_LEFT: u32 = 4;
const DIRT_DEEP: u32 = 5;
const BRICK: u32 = 6;
const BRICK_MOSS: u32 = 7;
const PLANK: u32 = 8;
const PLANK_END: u32 = 9;
const CRATE: u32 = 10;
const TUFT: u32 = 11;
const FLOWERS_RED: u32 = 12;
const FLOWERS_YELLOW: u32 = 13;
const BUSH: u32 = 14;
const MUSHROOMS: u32 = 15;
const ROCK: u32 = 16;
const SIGN: u32 = 17;
const WATER_TOP: u32 = 18;
const WATER: u32 = 19;
const BRICK_DARK: u32 = 20;
const BRICK_CRACKED: u32 = 21;
const BRICK_DARK_BROKEN: u32 = 22;
const TILESET_COLUMNS: u32 = 8;
const TILESET_ROWS: u32 = 3;

/// Earth with a few darker clods and light grains, seamless.
fn dirt_tile(t: &mut Canvas, seed: u32, [base, dark, light]: [Rgba; 3]) {
    t.rect(0, 0, 16, 16, base);
    for i in 0..7 {
        let x = (hash(i, 0, seed) * 16.0) as i32;
        let y = (hash(i, 1, seed) * 16.0) as i32;
        let len = 2 + (hash(i, 2, seed) * 2.0) as i32;
        for k in 0..len {
            t.set_wrap(x + k, y, dark);
        }
        t.set_wrap(x, y - 1, light);
    }
    // A pebble.
    let x = 2 + (hash(9, 0, seed) * 10.0) as i32;
    let y = 3 + (hash(9, 1, seed) * 9.0) as i32;
    t.rect(x, y, 3, 2, SLATE);
    t.set(x, y, SILVER);
    t.rect(x, y + 2, 3, 1, dark);
}

const EARTH: [Rgba; 3] = [DIRT, BARK, SAND];
const DEEP_EARTH: [Rgba; 3] = [BARK, BARK_DARK, DIRT];

fn grass_tile(t: &mut Canvas, seed: u32) {
    dirt_tile(t, seed, EARTH);
    for x in 0..16 {
        let depth = 3 + (hash(x, 0, seed + 50) * 3.0) as i32;
        t.rect(x, 0, 1, depth, GREEN);
        t.set(x, depth, TEAL);
        if hash(x, 1, seed + 50) < 0.8 {
            t.set(x, 0, LIME);
        }
        if hash(x, 2, seed + 50) < 0.25 {
            t.set(x, 1, LIME);
        }
    }
}

fn brick_tile(t: &mut Canvas, [face, light, mortar, dark]: [Rgba; 4]) {
    t.rect(0, 0, 16, 16, mortar);
    for row in 0..4 {
        let offset = if row % 2 == 0 { 0 } else { 4 };
        for b in -1..2 {
            let x0 = b * 8 + offset;
            let y0 = row * 4;
            for y in y0..y0 + 3 {
                for x in x0..x0 + 7 {
                    if !(0..16).contains(&x) {
                        continue;
                    }
                    let col = if y == y0 {
                        light
                    } else if y == y0 + 2 && x > x0 + 1 {
                        dark
                    } else {
                        face
                    };
                    t.set(x, y, col);
                }
            }
        }
    }
}

fn plank_tile(t: &mut Canvas, end: bool) {
    t.rect(0, 0, 16, 1, TAN);
    t.rect(0, 1, 16, 4, SAND);
    t.rect(0, 5, 16, 1, DIRT);
    t.rect(0, 6, 16, 1, BARK_DARK);
    for x in [3, 9, 13] {
        t.set(x, 2, DIRT);
        t.set(x + 1, 2, DIRT);
    }
    t.rect(7, 1, 1, 5, DIRT);
    t.set(2, 3, BARK_DARK);
    t.set(12, 3, BARK_DARK);
    if end {
        // Rounded left end and a rope binding.
        t.set(0, 0, CLEAR);
        t.set(0, 6, CLEAR);
        t.rect(0, 1, 1, 5, BARK_DARK);
        t.rect(3, 0, 2, 7, BARK);
        t.rect(3, 7, 2, 1, BARK_DARK);
    }
}

fn crate_tile(t: &mut Canvas) {
    t.rect(0, 0, 16, 16, BARK_DARK);
    t.rect(1, 1, 14, 14, SAND);
    t.rect(3, 3, 10, 10, DIRT);
    t.line((3.0, 12.0), (12.0, 3.0), 2, SAND);
    t.line((3.0, 11.0), (11.0, 3.0), 1, TAN);
    t.rect(1, 1, 14, 1, TAN);
    t.rect(1, 1, 1, 14, TAN);
    for (x, y) in [(2, 2), (13, 2), (2, 13), (13, 13)] {
        t.set(x, y, BARK_DARK);
    }
}

/// Decorations stand on the bottom edge of their tile and get a dark outline.
fn tuft_tile(t: &mut Canvas, seed: u32) {
    for b in 0..6 {
        let x = 3.0 + b as f32 * 2.0;
        let lean = (hash(b, 0, seed) - 0.5) * 6.0;
        let len = 4.0 + hash(b, 1, seed) * 6.0;
        t.line((x, 15.0), (x + lean, 15.0 - len), 1, GREEN);
        t.set((x + lean) as i32, (15.0 - len) as i32, LIME);
    }
}

fn flowers_tile(t: &mut Canvas, petals: Rgba, center: Rgba, seed: u32) {
    for f in 0..3 {
        let x = 3 + f * 5 + (hash(f, 0, seed) * 2.0) as i32;
        let top = 6 + (hash(f, 1, seed) * 5.0) as i32;
        t.rect(x, top, 1, 16 - top, GREEN);
        t.set(x - 1, top + 4, LIME);
        t.set(x + 1, top + 5, GREEN);
        for (dx, dy) in [(0, -1), (-1, 0), (1, 0), (0, 1)] {
            t.set(x + dx, top + dy, petals);
        }
        t.set(x, top, center);
    }
    t.outline(TEAL);
}

fn bush_tile(t: &mut Canvas) {
    t.ellipse(8.0, 11.0, 7.0, 5.0, TEAL);
    t.ellipse(7.0, 10.0, 6.0, 4.0, GREEN);
    t.ellipse(5.0, 8.5, 3.0, 2.0, LIME);
    t.rect(4, 15, 9, 1, TEAL);
    for (x, y) in [(10, 9), (12, 11), (8, 12)] {
        t.set(x, y, TEAL);
    }
    t.outline(hex(0x1e4d4f));
}

fn mushrooms_tile(t: &mut Canvas) {
    for &(x, y, r) in &[(5.0f32, 10.0f32, 3.5f32), (11.0, 12.0, 2.5)] {
        t.rect(x as i32 - 1, y as i32, 2, 16 - y as i32, WHITE);
        for yy in (y - r) as i32..y as i32 + 1 {
            for xx in (x - r - 1.0) as i32..=(x + r) as i32 {
                let dx = (xx as f32 + 0.5 - x) / (r + 0.5);
                let dy = (yy as f32 + 0.5 - (y + 0.5)) / r;
                if dx * dx + dy * dy <= 1.0 {
                    t.set(xx, yy, RED);
                }
            }
        }
        t.set(x as i32 - 2, y as i32 - 1, WHITE);
        t.set(x as i32 + 1, y as i32 - 2, WHITE);
    }
    t.outline(PLUM);
}

fn rock_tile(t: &mut Canvas) {
    t.ellipse(8.0, 13.0, 6.5, 4.0, SLATE);
    t.ellipse(6.5, 12.0, 4.0, 2.5, SILVER);
    t.rect(3, 15, 11, 1, STEEL);
    t.set(11, 13, STEEL);
    t.set(12, 14, STEEL);
    t.outline(INK);
}

fn sign_tile(t: &mut Canvas) {
    t.rect(7, 8, 2, 8, BARK);
    t.rect(2, 3, 12, 6, SAND);
    t.rect(2, 3, 12, 1, TAN);
    t.rect(2, 8, 12, 1, DIRT);
    // An arrow pointing right.
    t.rect(5, 5, 5, 2, BARK_DARK);
    t.rect(10, 4, 1, 4, BARK_DARK);
    t.rect(11, 5, 1, 2, BARK_DARK);
    t.outline(BARK_DARK);
}

fn water_tile(t: &mut Canvas, top: bool) {
    let deep = with_alpha(BLUE, 200);
    let shallow = with_alpha(SKY, 170);
    for y in 0..16 {
        for x in 0..16 {
            let col = if top && y < 5 { shallow } else { deep };
            t.set(x, y, col);
        }
    }
    if top {
        for x in 0..16 {
            let wave = ((x as f32 / 16.0 * 2.0 * PI).sin() * 1.2).round() as i32;
            t.set(x, 1 + wave, with_alpha(CYAN, 220));
            if x % 8 == 2 {
                t.set(x, wave.max(0), with_alpha(WHITE, 230));
            }
        }
        // Clear above the waves.
        for x in 0..16 {
            let wave = ((x as f32 / 16.0 * 2.0 * PI).sin() * 1.2).round() as i32;
            for y in 0..(1 + wave).max(0) {
                t.set(x, y, CLEAR);
            }
        }
    } else {
        for i in 0..4 {
            let x = (hash(i, 0, 77) * 16.0) as i32;
            let y = (hash(i, 1, 77) * 16.0) as i32;
            for k in 0..4 {
                t.set_wrap(x + k, y, with_alpha(SKY, 190));
            }
        }
    }
}

fn tileset() -> Canvas {
    let mut set = Canvas::new(16 * TILESET_COLUMNS as i32, 16 * TILESET_ROWS as i32);
    for frame in 0..TILESET_COLUMNS * TILESET_ROWS {
        let mut t = Canvas::new(16, 16);
        match frame {
            GRASS => grass_tile(&mut t, 1),
            GRASS_LEFT => {
                grass_tile(&mut t, 1);
                for y in 0..16 {
                    let grassy = y < 6;
                    t.set(0, y, if grassy { TEAL } else { BARK_DARK });
                    t.set(1, y, if grassy { GREEN } else { BARK });
                }
                t.set(1, 6, TEAL);
                t.set(1, 7, TEAL);
                t.set(0, 0, CLEAR);
                t.set(1, 0, CLEAR);
                t.set(0, 1, CLEAR);
                t.set(1, 1, LIME);
                t.set(2, 0, LIME);
            }
            DIRT_A => dirt_tile(&mut t, 2, EARTH),
            DIRT_B => dirt_tile(&mut t, 3, EARTH),
            DIRT_LEFT => {
                dirt_tile(&mut t, 2, EARTH);
                t.rect(0, 0, 1, 16, BARK_DARK);
                t.rect(1, 0, 1, 16, BARK);
            }
            DIRT_DEEP => dirt_tile(&mut t, 4, DEEP_EARTH),
            BRICK => brick_tile(&mut t, [SLATE, SILVER, STEEL, hex(0x47566f)]),
            BRICK_MOSS => {
                brick_tile(&mut t, [SLATE, SILVER, STEEL, hex(0x47566f)]);
                for x in 0..16 {
                    let drip = 2 + (hash(x, 0, 61) * 4.0) as i32;
                    t.rect(x, 0, 1, drip, GREEN);
                    t.set(x, drip, TEAL);
                    if hash(x, 1, 61) < 0.7 {
                        t.set(x, 0, LIME);
                    }
                }
            }
            BRICK_DARK => brick_tile(&mut t, [STEEL, SLATE, DEEP, INK]),
            BRICK_CRACKED => {
                brick_tile(&mut t, [SLATE, SILVER, STEEL, hex(0x47566f)]);
                // A missing brick and a crack running from it.
                t.rect(8, 4, 7, 3, DEEP);
                t.rect(8, 4, 7, 1, STEEL);
                t.line((8.0, 7.0), (5.0, 10.0), 1, STEEL);
                t.line((5.0, 10.0), (6.0, 14.0), 1, STEEL);
            }
            BRICK_DARK_BROKEN => {
                brick_tile(&mut t, [STEEL, SLATE, DEEP, INK]);
                // Broken away in steps toward the right.
                for x in 0..16 {
                    let gone = [0, 0, 0, 0, 1, 1, 4, 4, 4, 4, 5, 8, 8, 8, 9, 12][x as usize];
                    for y in 0..gone {
                        t.set(x, y, CLEAR);
                    }
                    if gone > 0 && gone < 16 {
                        t.set(x, gone, SLATE);
                    }
                }
            }
            PLANK => plank_tile(&mut t, false),
            PLANK_END => plank_tile(&mut t, true),
            CRATE => crate_tile(&mut t),
            TUFT => tuft_tile(&mut t, 5),
            FLOWERS_RED => flowers_tile(&mut t, RED, YELLOW, 6),
            FLOWERS_YELLOW => flowers_tile(&mut t, YELLOW, ORANGE, 7),
            BUSH => bush_tile(&mut t),
            MUSHROOMS => mushrooms_tile(&mut t),
            ROCK => rock_tile(&mut t),
            SIGN => sign_tile(&mut t),
            WATER_TOP => water_tile(&mut t, true),
            WATER => water_tile(&mut t, false),
            _ => {}
        }
        let (col, row) = (
            (frame % TILESET_COLUMNS) as i32,
            (frame / TILESET_COLUMNS) as i32,
        );
        set.blit(&t, col * 16, row * 16);
    }
    set
}

/// The hero's sheet: 24 × 24 frames, six running then two idle.
fn hero_sheet() -> Canvas {
    let mut sheet = Canvas::new(24 * 8, 24);
    for f in 0..8 {
        sheet.blit(&hero_frame(f), f * 24, 0);
    }
    sheet
}

fn hero_frame(f: i32) -> Canvas {
    let mut c = Canvas::new(24, 24);
    let running = f < 6;
    // Feet (dx from the hip, lift) of the near and far legs.
    const RUN: [[(f32, f32); 2]; 6] = [
        [(4.0, 0.0), (-4.0, 2.0)],
        [(2.0, 0.0), (-2.0, 4.0)],
        [(-1.0, 1.0), (1.0, 3.0)],
        [(-4.0, 2.0), (4.0, 0.0)],
        [(-2.0, 4.0), (2.0, 0.0)],
        [(1.0, 3.0), (-1.0, 1.0)],
    ];
    let (bob, [near, far]) = if running {
        ([1, 0, 0, 1, 0, 0][f as usize], RUN[f as usize])
    } else {
        ([0, 1][(f - 6) as usize], [(2.0, 0.0), (-2.0, 0.0)])
    };
    let b = bob as f32;
    let hip = (11.0, 16.0 + b);
    let leg = |c: &mut Canvas, foot: (f32, f32), pants: Rgba, boot: Rgba| {
        let fx = hip.0 + foot.0;
        let fy = 21.0 - foot.1;
        c.line((hip.0, hip.1), (fx, fy - 1.0), 2, pants);
        c.rect(fx as i32 - 1, fy as i32 - 1, 3, 2, boot);
        c.set(fx as i32 + 2, fy as i32, boot);
    };
    let arm_swing = -near.0 * 0.6;
    // Far side first.
    leg(&mut c, far, DEEP, BARK_DARK);
    c.line((11.0, 11.0 + b), (11.0 - arm_swing, 15.0 + b), 2, NAVY);
    // Scarf trailing behind, fluttering.
    let flutter = [0.0, 1.0, 1.0, 0.0, -1.0, -1.0, 0.0, 0.0][f as usize];
    c.line((9.0, 10.0 + b), (3.0, 10.0 + b + flutter), 2, RED);
    c.set(2, (11.0 + b + flutter) as i32, RED);
    // Body.
    c.rect(8, 10 + bob, 7, 7, BLUE);
    c.rect(8, 10 + bob, 1, 7, NAVY);
    c.rect(8, 16 + bob, 7, 1, NAVY);
    c.rect(8, 15 + bob, 7, 1, DIRT);
    c.set(13, 15 + bob, YELLOW);
    c.rect(9, 9 + bob, 6, 1, RED);
    // Head: skin, hair, eye.
    c.rect(8, 2 + bob, 8, 7, SKIN);
    c.rect(8, 7 + bob, 8, 1, SKIN_SHADE);
    c.rect(8, 1 + bob, 8, 3, HAIR);
    c.rect(7, 2 + bob, 3, 6, HAIR);
    c.rect(10, 1 + bob, 4, 1, DIRT);
    c.set(15, 3 + bob, HAIR);
    c.rect(13, 4 + bob, 1, 2, INK);
    c.set(14, 6 + bob, ORANGE);
    c.set(10, 5 + bob, SKIN_SHADE);
    // Near leg and arm.
    leg(&mut c, near, STEEL, BARK);
    c.line((13.0, 11.0 + b), (13.0 + arm_swing, 14.0 + b), 2, BLUE);
    let hand = (13.0 + arm_swing, 15.0 + b);
    c.rect(hand.0 as i32, hand.1 as i32, 2, 1, SKIN);
    c.outline(INK);
    c
}

/// A slime's sheet: 16 × 16 frames squashing and stretching.
fn slime_sheet() -> Canvas {
    let mut sheet = Canvas::new(16 * 4, 16);
    for (f, &(rx, ry)) in [(5.5f32, 5.5f32), (6.0, 5.0), (6.5, 4.5), (7.0, 4.0)]
        .iter()
        .enumerate()
    {
        let mut c = Canvas::new(16, 16);
        let (cx, cy) = (8.0, 15.0 - ry);
        c.ellipse(cx, cy, rx, ry, GREEN);
        c.rect((cx - rx) as i32 + 1, 14, (2.0 * rx) as i32 - 1, 1, TEAL);
        c.ellipse(cx + 1.0, cy + ry * 0.55, rx * 0.8, ry * 0.3, TEAL);
        c.ellipse(cx - rx * 0.45, cy - ry * 0.45, 1.6, 1.0, LIME);
        c.set((cx - rx * 0.5) as i32, (cy - ry * 0.6) as i32, WHITE);
        c.rect(cx as i32 + 1, cy as i32 - 1, 1, 2, INK);
        c.rect(cx as i32 + 4, cy as i32 - 1, 1, 2, INK);
        c.outline(INK);
        sheet.blit(&c, f as i32 * 16, 0);
    }
    sheet
}

/// A spinning coin: 8 × 8 frames.
fn coin_sheet() -> Canvas {
    let frames = 6;
    let mut sheet = Canvas::new(8 * frames, 8);
    for f in 0..frames {
        let mut c = Canvas::new(8, 8);
        let angle = f as f32 / frames as f32 * PI;
        let rx = (3.5 * angle.cos().abs()).max(0.6);
        c.ellipse(4.0, 4.0, rx, 3.5, ORANGE);
        if rx > 1.5 {
            c.ellipse(4.0, 4.0, rx - 1.0, 2.5, YELLOW);
            c.rect(3, 2, 1, 3, WHITE);
        } else {
            c.rect(3, 1, 2, 6, YELLOW);
        }
        sheet.blit(&c, f * 8, 0);
    }
    sheet
}

// ---------------------------------------------------------------------------------------------
// The side-scroller.

const VIEW_HEIGHT: f32 = 11.25; // 180 texel rows: 4 screen pixels per texel at 720 rows.
const CAMERA_Y: f64 = VIEW_HEIGHT as f64 * 0.5;
const MAP_W: u32 = 96;
const MAP_H: u32 = 12;

/// Where to put a parallax layer with `factor` so it shows at `at` while the camera is at
/// `camera`.
fn layer_position(at: DVec3, factor: f32, camera: DVec3) -> DVec3 {
    let k = 1.0 - factor as f64;
    DVec3::new(at.x - camera.x * k, at.y - camera.y * k, at.z)
}

/// Ground height (tiles) of each column, and the pits.
fn ground_height(x: u32) -> u32 {
    match x {
        14..=19 => 4,
        22..=25 => 0,
        33..=38 => 5,
        39..=41 => 4,
        52..=55 => 0,
        60..=70 => 2,
        _ => 3,
    }
}

fn side_scroller_map(tileset: Handle<Image>) -> (TileMap, TileMap, TileMap) {
    let atlas = SpriteAtlas::grid(TILESET_COLUMNS, TILESET_ROWS);
    let new_map = || {
        TileMap::new(tileset, atlas.clone(), [MAP_W, MAP_H], [1.0, 1.0])
            .with_filter(SpriteFilter::PixelArt)
    };
    let mut ground = new_map();
    let mut back = new_map().with_color([0.55, 0.5, 0.7, 1.0]);
    let mut water = new_map().with_alpha(SpriteAlpha::Blend);
    let tile = |frame: u32| frame + 1;
    // `y` counts rows from the bottom.
    let set = |map: &mut TileMap, x: u32, y: u32, v: u32| map.set(x, MAP_H - 1 - y, v);
    for x in 0..MAP_W {
        let h = ground_height(x);
        let left = x.checked_sub(1).map_or(h, ground_height);
        let right = if x + 1 < MAP_W {
            ground_height(x + 1)
        } else {
            h
        };
        let brick = (33..=41).contains(&x);
        for y in 0..h {
            let top = y + 1 == h;
            let edge_left = left <= y;
            let edge_right = right <= y;
            let (frame, flip) = if brick {
                let cracked = hash(x as i32, y as i32, 23) < 0.22;
                let frame = match (top, cracked) {
                    (true, _) => BRICK_MOSS,
                    (false, true) => BRICK_CRACKED,
                    (false, false) => BRICK,
                };
                (
                    frame,
                    if hash(x as i32, y as i32, 24) < 0.5 {
                        TILE_FLIP_X
                    } else {
                        0
                    },
                )
            } else if top {
                match (edge_left, edge_right) {
                    (true, _) => (GRASS_LEFT, 0),
                    (false, true) => (GRASS_LEFT, TILE_FLIP_X),
                    _ => (GRASS, 0),
                }
            } else if edge_left {
                (DIRT_LEFT, 0)
            } else if edge_right {
                (DIRT_LEFT, TILE_FLIP_X)
            } else if y + 3 < h || (y == 0 && h >= 2) {
                (DIRT_DEEP, 0)
            } else if hash(x as i32, y as i32, 5) < 0.5 {
                (DIRT_A, 0)
            } else {
                (DIRT_B, 0)
            };
            set(&mut ground, x, y, tile(frame) | flip);
        }
        // Decorations on the grass.
        if h > 0 && !brick {
            let r = hash(x as i32, 0, 17);
            let deco = match r {
                r if r < 0.16 => Some(TUFT),
                r if r < 0.24 => Some(FLOWERS_RED),
                r if r < 0.32 => Some(FLOWERS_YELLOW),
                r if r < 0.38 => Some(BUSH),
                r if r < 0.42 => Some(MUSHROOMS),
                r if r < 0.47 => Some(ROCK),
                _ => None,
            };
            if let Some(d) = deco {
                let flip = if hash(x as i32, 1, 17) < 0.5 {
                    TILE_FLIP_X
                } else {
                    0
                };
                set(&mut ground, x, h, tile(d) | flip);
            }
        }
        // Water in the pits.
        if h == 0 {
            set(&mut water, x, 1, tile(WATER_TOP));
            set(&mut water, x, 0, tile(WATER));
        }
    }
    // Floating platforms, crates and a sign.
    for (x0, x1, y) in [(8u32, 11u32, 6u32), (22, 25, 5), (44, 48, 6), (52, 55, 4)] {
        for x in x0..=x1 {
            let v = if x == x0 {
                tile(PLANK_END)
            } else if x == x1 {
                tile(PLANK_END) | TILE_FLIP_X
            } else {
                tile(PLANK)
            };
            set(&mut ground, x, y, v);
        }
    }
    for (x, y) in [(29, 3), (30, 3), (29, 4), (46, 7), (64, 2)] {
        set(&mut ground, x, y, tile(CRATE));
    }
    set(&mut ground, 4, 3, tile(SIGN));
    // A ruined wall behind the brick hill (a second map, tinted darker, behind the first):
    // broken tops, two window openings.
    let tops = [6u32, 7, 9, 9, 8, 9, 10, 10, 9, 8, 9, 7, 6];
    for (i, x) in (31..=43u32).enumerate() {
        let top = tops[i];
        for y in 3..top {
            let window = (x == 35 || x == 39) && (6..=7).contains(&y);
            if window {
                continue;
            }
            let v = if y + 1 == top {
                let flip = if hash(x as i32, 5, 9) < 0.5 {
                    TILE_FLIP_X
                } else {
                    0
                };
                tile(BRICK_DARK_BROKEN) | flip
            } else {
                tile(BRICK_DARK)
            };
            set(&mut back, x, y, v);
        }
    }
    (ground, back, water)
}

fn headless_app(width: u32, height: u32) -> App {
    engine_core::log::init("warn");
    let mut app = App::new();
    app.world.resource_mut::<EngineConfig>().headless = true;
    {
        let mut w = app.world.resource_mut::<Window>();
        w.width = width;
        w.height = height;
    }
    app.add_plugins(RenderPlugin);
    app.world
        .resource_mut::<Time>()
        .set_source(TimeSource::Manual {
            step_secs: 1.0 / 30.0,
        });
    app
}

struct SideScroller {
    app: App,
    camera: Entity,
}

fn side_scroller() -> SideScroller {
    let mut app = headless_app(1280, 720);
    {
        let mut settings = app.world.resource_mut::<RenderSettings>();
        *settings = settings.clone().with_flat_colors();
    }
    app.world.resource_mut::<FogSettings>().density = 0.0;
    let camera_at = DVec3::new(12.0, CAMERA_Y, 50.0);
    let camera = app
        .world
        .spawn((
            Camera {
                exposure: Exposure::Manual { ev100: 0.0 },
                ..Camera::orthographic(VIEW_HEIGHT)
            },
            PixelPerfect::new(PPU),
            Transform::from_translation(camera_at),
        ))
        .id();

    // Background layers, far to near.
    let sky = add_image(&mut app, sky().image());
    app.world.spawn((
        Transform::from_translation(layer_position(
            DVec3::new(camera_at.x, camera_at.y, -40.0),
            0.0,
            camera_at,
        )),
        Sprite::pixel_art(sky, [320, 180], PPU).with_layer(-10),
        Parallax::new(0.0),
    ));
    let clouds = add_image(&mut app, clouds().image());
    app.world.spawn((
        Transform::from_translation(layer_position(DVec3::new(0.0, 6.5, -35.0), 0.06, camera_at)),
        Sprite::pixel_art(clouds, [640, 64], PPU)
            .with_anchor([0.0, 0.0])
            .with_layer(-9),
        Parallax::new(0.06).repeating(40.0),
    ));
    let far = mountains(
        768,
        110,
        7,
        (55.0, 100.0),
        78.0,
        21,
        [hex(0x9fb6d0), hex(0x7f93b5), WHITE, hex(0xc2d3e6)],
    );
    let far = add_image(&mut app, far.image());
    app.world.spawn((
        Transform::from_translation(layer_position(DVec3::new(0.0, 1.5, -30.0), 0.12, camera_at)),
        Sprite::pixel_art(far, [768, 110], PPU)
            .with_anchor([0.0, 0.0])
            .with_layer(-8),
        Parallax::new(0.12).repeating(48.0),
    ));
    let near = mountains(
        640,
        96,
        6,
        (40.0, 80.0),
        66.0,
        33,
        [hex(0x6f86a8), SLATE, hex(0xdfe8f0), SILVER],
    );
    let near = add_image(&mut app, near.image());
    app.world.spawn((
        Transform::from_translation(layer_position(DVec3::new(0.0, 1.0, -25.0), 0.25, camera_at)),
        Sprite::pixel_art(near, [640, 96], PPU)
            .with_anchor([0.0, 0.0])
            .with_layer(-7),
        Parallax::new(0.25).repeating(40.0),
    ));
    let hills = add_image(
        &mut app,
        forest(512, 96, 41, hex(0x2a6f73), hex(0x3d9488)).image(),
    );
    app.world.spawn((
        Transform::from_translation(layer_position(DVec3::new(0.0, 0.5, -20.0), 0.45, camera_at)),
        Sprite::pixel_art(hills, [512, 96], PPU)
            .with_anchor([0.0, 0.0])
            .with_layer(-6),
        Parallax::new(0.45).repeating(32.0),
    ));
    let woods = add_image(
        &mut app,
        forest(384, 80, 57, hex(0x1d4e55), hex(0x2a6f6b)).image(),
    );
    app.world.spawn((
        Transform::from_translation(layer_position(DVec3::new(0.0, 0.0, -15.0), 0.65, camera_at)),
        Sprite::pixel_art(woods, [384, 80], PPU)
            .with_anchor([0.0, 0.0])
            .with_layer(-5),
        Parallax::new(0.65).repeating(24.0),
    ));

    // The level.
    let tiles = add_image(&mut app, tileset().image());
    let (ground, back, water) = side_scroller_map(tiles);
    app.world
        .spawn((Transform::from_xyz(0.0, 0.0, -1.0), back.with_layer(-1)));
    app.world
        .spawn((Transform::from_xyz(0.0, 0.0, 0.0), ground));
    app.world
        .spawn((Transform::from_xyz(0.0, 0.0, 2.0), water.with_layer(2)));

    // The hero, mid-stride, a slime and a row of coins over the pit.
    let hero = add_image(&mut app, hero_sheet().image());
    app.world.spawn((
        Transform::from_xyz(9.0, 3.0, 1.0),
        Sprite::pixel_art(hero, [24, 24], PPU)
            .with_atlas(SpriteAtlas::grid(8, 1))
            .with_anchor([0.5, 0.0])
            .with_layer(1),
        SpriteAnimation::new(0..6, 12.0),
    ));
    let slime = add_image(&mut app, slime_sheet().image());
    for (x, y, phase) in [(17.5, 4.0, 0.0), (36.0, 5.0, 0.3)] {
        let mut animation =
            SpriteAnimation::new(0..4, 6.0).with_mode(SpriteAnimationMode::PingPong);
        animation.time = phase;
        app.world.spawn((
            Transform::from_xyz(x, y, 1.0),
            Sprite::pixel_art(slime, [16, 16], PPU)
                .with_atlas(SpriteAtlas::grid(4, 1))
                .with_anchor([0.5, 0.0])
                .with_layer(1)
                .flipped_x(true),
            animation,
        ));
    }
    let coin = add_image(&mut app, coin_sheet().image());
    for i in 0..7 {
        let x = 21.5 + i as f64 * 0.75;
        let y = 7.0 + (1.0 - ((i as f64 - 3.0) / 3.0).powi(2)) * 1.5;
        let mut animation = SpriteAnimation::new(0..6, 10.0);
        animation.time = i as f32 * 0.05;
        app.world.spawn((
            Transform::from_xyz(x, y, 1.0),
            Sprite::pixel_art(coin, [8, 8], PPU)
                .with_atlas(SpriteAtlas::grid(6, 1))
                .with_layer(1),
            animation,
        ));
    }

    // Dark foliage in front of everything, moving faster than the level.
    let foliage = add_image(&mut app, foreground_foliage(256).image());
    app.world.spawn((
        Transform::from_translation(layer_position(DVec3::new(0.0, -0.25, 5.0), 1.35, camera_at)),
        Sprite::pixel_art(foliage, [256, 40], PPU)
            .with_anchor([0.0, 0.0])
            .with_layer(10),
        Parallax::new(1.35).repeating(16.0),
    ));
    SideScroller { app, camera }
}

// ---------------------------------------------------------------------------------------------
// Diorama art: lit paper sprites, 16 texels per meter.

const LEAF_DARK: Rgba = hex(0x1e4d4f);
const LEAF_OUTLINE: Rgba = hex(0x163638);

/// A tangent-space normal map (+y up, as the sprite shader reads it) from a height field in
/// texels.
fn normal_map(height: &[f32], w: i32, h: i32) -> Image {
    let at = |x: i32, y: i32| height[(y.clamp(0, h - 1) * w + x.clamp(0, w - 1)) as usize];
    let mut data = Vec::with_capacity((w * h * 4) as usize);
    for y in 0..h {
        for x in 0..w {
            let dx = (at(x + 1, y) - at(x - 1, y)) * 0.5;
            let dy = (at(x, y - 1) - at(x, y + 1)) * 0.5;
            let n = Vec3::new(-dx, -dy, 1.0).normalize();
            data.extend_from_slice(&[
                ((n.x * 0.5 + 0.5) * 255.0).round() as u8,
                ((n.y * 0.5 + 0.5) * 255.0).round() as u8,
                ((n.z * 0.5 + 0.5) * 255.0).round() as u8,
                255,
            ]);
        }
    }
    Image::new(w as u32, h as u32, data, false)
}

/// Foliage made of round puffs `(x, y, radius)`: each texel shows the puff whose surface is
/// highest there, shaded in four tones for light from the upper left and broken into leaf
/// clumps. Writes the puffs' height into `height`.
fn foliage(
    c: &mut Canvas,
    height: &mut [f32],
    puffs: &[(f32, f32, f32)],
    tones: [Rgba; 4],
    seed: u32,
) {
    let light = Vec3::new(-0.55, 0.6, 0.58).normalize();
    for y in 0..c.h {
        for x in 0..c.w {
            let mut best: Option<(f32, Vec3)> = None;
            for (i, &(px, py, r)) in puffs.iter().enumerate() {
                let dx = x as f32 + 0.5 - px;
                let dy = y as f32 + 0.5 - py;
                let d2 = dx * dx + dy * dy;
                if d2 < r * r {
                    // Lower puffs sit in front.
                    let z = (r * r - d2).sqrt() + py * 0.25 + i as f32 * 0.01;
                    if best.is_none_or(|b| z > b.0) {
                        let n = Vec3::new(dx / r, -dy / r, (1.0 - d2 / (r * r)).sqrt());
                        best = Some((z, n));
                    }
                }
            }
            let Some((z, n)) = best else { continue };
            let clump = hash(x / 3, (y + (x / 3) % 2) / 2, seed);
            let shade = n.dot(light) * 0.5 + 0.5 + (clump - 0.5) * 0.35;
            let tone = ((shade * 4.3 - 0.9).floor().clamp(0.0, 3.0)) as usize;
            c.set(x, y, tones[tone]);
            height[(y * c.w + x) as usize] = z + clump * 1.5;
        }
    }
}

/// A round broadleaf tree, 48 × 64 texels (3 × 4 m), and its normal map.
fn round_tree(seed: u32) -> (Canvas, Image) {
    let (w, h) = (48, 64);
    let mut c = Canvas::new(w, h);
    let mut height = vec![0.0f32; (w * h) as usize];
    for y in 30..h {
        let t = (y - 30) as f32 / (h - 31) as f32;
        let half = 2.0 + t.powi(4) * 4.0;
        let x0 = (24.0 - half).round() as i32;
        let x1 = (24.0 + half).round() as i32;
        for x in x0..x1 {
            let f = (x - x0) as f32 / (x1 - x0 - 1).max(1) as f32;
            let col = if f < 0.25 {
                DIRT
            } else if f < 0.7 {
                BARK
            } else {
                BARK_DARK
            };
            c.set(x, y, col);
            height[(y * w + x) as usize] = (1.0 - (2.0 * f - 1.0).powi(2)).max(0.0).sqrt() * half;
        }
    }
    c.line((23.0, 40.0), (15.0, 31.0), 2, BARK);
    c.line((25.0, 38.0), (33.0, 30.0), 2, BARK);
    let mut puffs = vec![
        (24.0, 19.0, 15.0),
        (12.5, 25.0, 9.5),
        (35.5, 24.0, 10.0),
        (16.0, 12.0, 9.0),
        (32.0, 11.0, 9.5),
        (22.0, 30.0, 9.0),
        (24.0, 7.0, 7.5),
    ];
    for (i, p) in puffs.iter_mut().enumerate() {
        p.0 += (hash(i as i32, 0, seed) - 0.5) * 3.0;
        p.1 += (hash(i as i32, 1, seed) - 0.5) * 3.0;
        p.2 *= 0.9 + 0.2 * hash(i as i32, 2, seed);
    }
    foliage(
        &mut c,
        &mut height,
        &puffs,
        [LEAF_DARK, TEAL, GREEN, LIME],
        seed,
    );
    c.outline(LEAF_OUTLINE);
    let normals = normal_map(&height, w, h);
    (c, normals)
}

/// A pine, 32 × 64 texels (2 × 4 m), and its normal map.
fn pine_tree(seed: u32) -> (Canvas, Image) {
    let (w, h) = (32, 64);
    let mut c = Canvas::new(w, h);
    let mut height = vec![0.0f32; (w * h) as usize];
    c.rect(14, 52, 4, 12, BARK);
    c.rect(14, 52, 1, 12, DIRT);
    c.rect(17, 52, 1, 12, BARK_DARK);
    let light = Vec3::new(-0.55, 0.6, 0.58).normalize();
    let tiers = 4;
    for t in 0..tiers {
        let top = 2.0 + t as f32 * 11.0;
        let bottom = top + 18.0 + t as f32 * 2.0;
        let width = 6.0 + t as f32 * 2.6;
        for y in top as i32..bottom as i32 {
            let f = (y as f32 + 0.5 - top) / (bottom - top);
            // Ragged lower edge.
            let half = width * f
                + if f > 0.8 {
                    (hash(y, t, seed) - 0.5) * 3.0
                } else {
                    0.0
                };
            for x in (16.0 - half).round() as i32..(16.0 + half).round() as i32 {
                let u = (x as f32 + 0.5 - 16.0) / half.max(0.5);
                let n = Vec3::new(
                    u * 0.8,
                    0.45 - f * 0.4,
                    (1.0 - (u * 0.8).powi(2)).max(0.05).sqrt(),
                )
                .normalize();
                let clump = hash(x / 2, y / 2 + t * 7, seed);
                let shade = n.dot(light) * 0.5 + 0.5 + (clump - 0.5) * 0.3 - f * 0.15;
                let tone = ((shade * 4.0 - 0.7).floor().clamp(0.0, 3.0)) as usize;
                c.set(x, y, [LEAF_DARK, hex(0x1d5e5a), TEAL, GREEN][tone]);
                height[(y * w + x) as usize] =
                    (1.0 - u * u).max(0.0).sqrt() * half * 0.8 + (1.0 - f) * 3.0 + clump;
            }
        }
    }
    c.outline(LEAF_OUTLINE);
    let normals = normal_map(&height, w, h);
    (c, normals)
}

/// A bush, 32 × 20 texels (2 × 1.25 m), and its normal map.
fn bush(seed: u32, flowers: Option<Rgba>) -> (Canvas, Image) {
    let (w, h) = (32, 20);
    let mut c = Canvas::new(w, h);
    let mut height = vec![0.0f32; (w * h) as usize];
    let puffs = [
        (16.0, 12.0, 8.5),
        (8.5, 14.0, 6.5),
        (23.5, 14.0, 7.0),
        (12.0, 8.5, 5.5),
        (20.0, 8.0, 6.0),
    ];
    foliage(
        &mut c,
        &mut height,
        &puffs,
        [LEAF_DARK, TEAL, GREEN, LIME],
        seed,
    );
    for x in 0..w {
        for y in 18..h {
            c.set(x, y, CLEAR);
            height[(y * w + x) as usize] = 0.0;
        }
    }
    if let Some(petal) = flowers {
        for i in 0..9 {
            let x = 4 + (hash(i, 0, seed + 3) * 24.0) as i32;
            let y = 4 + (hash(i, 1, seed + 3) * 11.0) as i32;
            if c.opaque(x, y) {
                c.set(x, y, petal);
                c.set(x + 1, y, WHITE);
            }
        }
    }
    c.outline(LEAF_OUTLINE);
    let normals = normal_map(&height, w, h);
    (c, normals)
}

/// A villager facing the viewer, 16 × 24 texels, two idle frames.
fn villager_sheet(hair: Rgba, shirt: Rgba, shirt_shade: Rgba, skirt: bool) -> Canvas {
    let mut sheet = Canvas::new(32, 24);
    for f in 0..2 {
        let mut c = Canvas::new(16, 24);
        let bob = f;
        // Legs and shoes.
        if skirt {
            c.rect(5, 15 + bob, 6, 5 - bob, shirt_shade);
            c.rect(6, 20, 1, 2, SKIN_SHADE);
            c.rect(9, 20, 1, 2, SKIN_SHADE);
        } else {
            c.rect(5, 16 + bob, 2, 6 - bob, STEEL);
            c.rect(9, 16 + bob, 2, 6 - bob, STEEL);
        }
        c.rect(4, 22, 3, 2, BARK);
        c.rect(9, 22, 3, 2, BARK);
        // Body and arms.
        c.rect(4, 9 + bob, 8, 7, shirt);
        c.rect(4, 9 + bob, 1, 7, shirt_shade);
        c.rect(11, 9 + bob, 1, 7, shirt_shade);
        c.rect(3, 10 + bob, 1, 5, shirt_shade);
        c.rect(12, 10 + bob, 1, 5, shirt_shade);
        c.set(3, 15 + bob, SKIN);
        c.set(12, 15 + bob, SKIN);
        c.rect(4, 14 + bob, 8, 1, DIRT);
        // Head.
        c.rect(4, 2 + bob, 8, 7, SKIN);
        c.rect(4, 8 + bob, 8, 1, SKIN_SHADE);
        c.rect(4, 1 + bob, 8, 3, hair);
        c.rect(3, 2 + bob, 1, 5, hair);
        c.rect(12, 2 + bob, 1, 5, hair);
        c.rect(6, 5 + bob, 1, 2, INK);
        c.rect(9, 5 + bob, 1, 2, INK);
        c.set(5, 7 + bob, ORANGE);
        c.set(10, 7 + bob, ORANGE);
        c.outline(INK);
        sheet.blit(&c, f * 16, 0);
    }
    sheet
}

fn flower_sprite(petals: Rgba, center: Rgba) -> Canvas {
    let mut c = Canvas::new(8, 10);
    c.rect(4, 4, 1, 6, GREEN);
    c.set(3, 7, LIME);
    c.set(5, 6, GREEN);
    for (dx, dy) in [(0, -1), (-1, 0), (1, 0), (0, 1), (-1, -1), (1, 1)] {
        c.set(4 + dx, 3 + dy, petals);
    }
    c.set(4, 3, center);
    c.outline(LEAF_OUTLINE);
    c
}

/// A tuft of grass fanning out from its root, 12 × 10 texels.
fn tuft_sprite(seed: u32) -> Canvas {
    let mut c = Canvas::new(12, 10);
    let blades = 7;
    for b in 0..blades {
        let fan = b as f32 / (blades - 1) as f32 - 0.5;
        let len = 5.0 + hash(b, 1, seed) * 4.0 - fan.abs() * 3.0;
        let tip = (6.0 + fan * 10.0 + (hash(b, 0, seed) - 0.5) * 2.0, 9.5 - len);
        let root = (6.0 + fan * 3.0, 9.5);
        let mid = ((root.0 + tip.0) * 0.5, (root.1 + tip.1) * 0.5);
        let shade = if b % 3 == 1 { hex(0x2f7448) } else { GREEN };
        c.line(root, mid, 1, hex(0x2f7448));
        c.line(mid, tip, 1, shade);
        c.set(tip.0 as i32, tip.1 as i32, LIME);
    }
    c.rect(4, 9, 5, 1, hex(0x2f7448));
    c
}

/// A fence section, 32 × 16 texels (2 × 1 m).
fn fence() -> Canvas {
    let mut c = Canvas::new(32, 16);
    for x in [2, 28] {
        c.rect(x, 1, 3, 15, SAND);
        c.rect(x + 2, 1, 1, 15, DIRT);
        c.set(x, 1, TAN);
        c.set(x + 1, 0, SAND);
    }
    for y in [4, 10] {
        c.rect(0, y, 32, 2, TAN);
        c.rect(0, y + 1, 32, 1, SAND);
    }
    c.outline(BARK_DARK);
    c
}

fn signpost() -> Canvas {
    let mut c = Canvas::new(24, 24);
    c.rect(11, 8, 2, 16, BARK);
    c.rect(2, 3, 20, 7, SAND);
    c.rect(2, 3, 20, 1, TAN);
    c.rect(2, 9, 20, 1, DIRT);
    // Lettering: little dark strokes.
    for (x, w) in [(4, 3), (8, 2), (11, 4), (16, 3)] {
        c.rect(x, 5, w, 1, BARK_DARK);
        c.rect(x + 1, 7, w - 1, 1, BARK_DARK);
    }
    c.outline(BARK_DARK);
    c
}

/// A lantern head, 8 × 12 texels: warm glass in a dark frame.
fn lantern() -> Canvas {
    let mut c = Canvas::new(8, 12);
    c.rect(2, 0, 4, 1, STEEL);
    c.rect(1, 1, 6, 2, STEEL);
    c.rect(1, 3, 6, 7, STEEL);
    c.rect(2, 3, 4, 6, YELLOW);
    c.rect(3, 4, 2, 4, hex(0xfff1c2));
    c.rect(1, 10, 6, 1, STEEL);
    c.rect(3, 11, 2, 1, STEEL);
    c
}

/// Flames, 16 × 24 texels, four frames.
fn flame_sheet() -> Canvas {
    let mut sheet = Canvas::new(64, 24);
    for f in 0..4 {
        let mut c = Canvas::new(16, 24);
        for y in 0..24 {
            for x in 0..16 {
                let v = (23 - y) as f32 / 23.0;
                let sway = (noise1(v * 2.5 + f as f32 * 1.3, 64, 5) - 0.5) * 5.0 * v;
                let half = 6.5 * (1.0 - v).powf(0.8) * (0.35 + v * 3.0).min(1.0);
                let dx = (x as f32 + 0.5 - 8.0 - sway).abs();
                let heat = 1.0 - dx / half.max(0.01) - v * 0.55
                    + (hash(x / 2, y / 2 + f * 13, 9) - 0.5) * 0.35;
                if dx < half && heat > 0.0 {
                    let col = if heat > 0.62 {
                        WHITE
                    } else if heat > 0.42 {
                        YELLOW
                    } else if heat > 0.2 {
                        ORANGE
                    } else {
                        RED
                    };
                    c.set(x, y, col);
                }
            }
        }
        sheet.blit(&c, f * 16, 0);
    }
    sheet
}

/// A soft round glow for additive halos (smooth, drawn with linear filtering).
fn glow() -> Image {
    Image::from_fn(64, 64, true, |x, y| {
        let d = ((x as f32 + 0.5 - 32.0).powi(2) + (y as f32 + 0.5 - 32.0).powi(2)).sqrt() / 32.0;
        let a = (1.0 - d).max(0.0).powi(2);
        [255, 255, 255, (a * 255.0) as u8]
    })
}

// Ground tiles (8 columns × 2 rows of 16 × 16 texels).
const G_GRASS: u32 = 0; // three variants: 0, 1, 2
const G_PATH: u32 = 3;
const G_PATH_EDGE: u32 = 4; // grass along the top edge
const G_PATH_CORNER: u32 = 5; // grass along the top and left edges
const G_COBBLE: u32 = 6;
const G_COBBLE_EDGE: u32 = 7;
const G_SOIL: u32 = 8;
const G_SPROUTS: u32 = 9;
const G_FLOWERS: u32 = 10;

fn meadow_tile(t: &mut Canvas, seed: u32) {
    let (base, dark, light) = (hex(0x419a52), hex(0x2f7448), hex(0x74c262));
    t.rect(0, 0, 16, 16, base);
    for i in 0..7 {
        let x = (hash(i, 0, seed) * 16.0) as i32;
        let y = (hash(i, 1, seed) * 16.0) as i32;
        // A little V of darker blades with a light tip.
        t.set_wrap(x, y, dark);
        t.set_wrap(x + 1, y - 1, dark);
        t.set_wrap(x - 1, y - 1, dark);
        if i % 2 == 0 {
            t.set_wrap(x + 1, y - 2, light);
        }
    }
    for i in 0..3 {
        let x = (hash(i, 2, seed) * 16.0) as i32;
        let y = (hash(i, 3, seed) * 16.0) as i32;
        t.set_wrap(x, y, light);
        t.set_wrap(x + 1, y, light);
    }
}

fn path_tile(t: &mut Canvas, seed: u32) {
    t.rect(0, 0, 16, 16, SAND);
    for i in 0..8 {
        let x = (hash(i, 0, seed) * 16.0) as i32;
        let y = (hash(i, 1, seed) * 16.0) as i32;
        t.set_wrap(x, y, DIRT);
        t.set_wrap(x + 1, y, DIRT);
        t.set_wrap(x, y - 1, TAN);
    }
    for i in 0..2 {
        let x = (hash(i, 4, seed) * 14.0) as i32;
        let y = (hash(i, 5, seed) * 14.0) as i32;
        t.rect(x, y, 2, 1, SILVER);
        t.rect(x, y + 1, 2, 1, SLATE);
    }
}

/// Grass over the top `depth` rows (jagged), with a shadow line where it meets the ground.
fn grass_edge(t: &mut Canvas, seed: u32, left: bool) {
    let mut grass = Canvas::new(16, 16);
    meadow_tile(&mut grass, seed);
    for x in 0..16 {
        let depth = 3 + (hash(x, 0, seed + 9) * 2.5) as i32;
        for y in 0..depth {
            t.set(x, y, grass.get(x, y));
        }
        t.set(x, depth, TEAL);
    }
    if left {
        for y in 0..16 {
            let depth = 3 + (hash(y, 1, seed + 9) * 2.5) as i32;
            for x in 0..depth {
                t.set(x, y, grass.get(x, y));
            }
            if t.get(depth, y) != grass.get(depth, y) && y > 3 {
                t.set(depth, y, TEAL);
            }
        }
    }
}

fn cobble_tile(t: &mut Canvas) {
    t.rect(0, 0, 16, 16, STEEL);
    for row in 0..3 {
        let y0 = row * 5 + 1;
        let offset = [0, 3, 1][row as usize];
        let mut x = -offset;
        let mut i = 0;
        while x < 16 {
            let w = 4 + (hash(i, row, 31) * 3.0) as i32;
            for y in y0..y0 + 4 {
                for xx in x..x + w - 1 {
                    let col = if y == y0 {
                        hex(0xb7cad6)
                    } else if y == y0 + 3 {
                        SLATE
                    } else {
                        SILVER
                    };
                    t.set_wrap(xx, y, col);
                }
            }
            // Rounded corners.
            t.set_wrap(x, y0, STEEL);
            t.set_wrap(x + w - 2, y0 + 3, STEEL);
            x += w;
            i += 1;
        }
    }
}

fn soil_tile(t: &mut Canvas, sprouts: bool) {
    t.rect(0, 0, 16, 16, BARK);
    for y in [3, 11] {
        t.rect(0, y, 16, 2, BARK_DARK);
        t.rect(0, y - 1, 16, 1, DIRT);
    }
    if sprouts {
        for (x, y) in [(2, 6), (7, 7), (12, 6), (4, 14), (10, 15), (14, 14)] {
            t.set(x, y, GREEN);
            t.set(x - 1, y - 1, LIME);
            t.set(x + 1, y - 1, GREEN);
        }
    }
}

fn ground_tileset() -> Canvas {
    let mut set = Canvas::new(128, 32);
    for frame in 0..16u32 {
        let mut t = Canvas::new(16, 16);
        match frame {
            0..=2 => meadow_tile(&mut t, 100 + frame),
            G_PATH => path_tile(&mut t, 7),
            G_PATH_EDGE => {
                path_tile(&mut t, 8);
                grass_edge(&mut t, 101, false);
            }
            G_PATH_CORNER => {
                path_tile(&mut t, 9);
                grass_edge(&mut t, 102, true);
            }
            G_COBBLE => cobble_tile(&mut t),
            G_COBBLE_EDGE => {
                cobble_tile(&mut t);
                grass_edge(&mut t, 103, false);
            }
            G_SOIL => soil_tile(&mut t, false),
            G_SPROUTS => soil_tile(&mut t, true),
            G_FLOWERS => {
                meadow_tile(&mut t, 104);
                for (i, (x, y)) in [(3, 4), (11, 2), (7, 10), (13, 12), (2, 13)]
                    .iter()
                    .enumerate()
                {
                    let c = [YELLOW, WHITE, ORANGE, WHITE, YELLOW][i];
                    t.set(*x, *y, c);
                    t.set(x + 1, *y, c);
                    t.set(*x, y + 1, c);
                }
            }
            _ => {}
        }
        set.blit(&t, (frame % 8) as i32 * 16, (frame / 8) as i32 * 16);
    }
    set
}

// The board: 22 × 14 tiles of 1 m, centered on the origin; row 0 is the far edge.
const BOARD_W: u32 = 22;
const BOARD_H: u32 = 14;

/// The world position of the center of tile (`col`, `row`) on the board.
fn board(col: f64, row: f64) -> DVec3 {
    DVec3::new(
        col + 0.5 - BOARD_W as f64 * 0.5,
        0.0,
        row + 0.5 - BOARD_H as f64 * 0.5,
    )
}

fn diorama_ground(tiles: Handle<Image>) -> TileMap {
    use engine_render::{TILE_FLIP_DIAGONAL, TILE_FLIP_Y};
    let mut map = TileMap::new(
        tiles,
        SpriteAtlas::grid(8, 2),
        [BOARD_W, BOARD_H],
        [1.0, 1.0],
    )
    .with_anchor([0.5, 0.5])
    .with_filter(SpriteFilter::PixelArt)
    .lit();
    map.cast_shadows = false;
    map.roughness = 0.9;
    let tile = |frame: u32| frame + 1;
    for row in 0..BOARD_H {
        for col in 0..BOARD_W {
            let r = hash(col as i32, row as i32, 55);
            let v = if r < 0.12 {
                G_FLOWERS
            } else {
                G_GRASS + (r * 10.0) as u32 % 3
            };
            map.set(col, row, tile(v));
        }
    }
    // The road across the front (rows 9 and 10), grass along both edges.
    for col in 0..BOARD_W {
        map.set(col, 9, tile(G_PATH_EDGE));
        map.set(col, 10, tile(G_PATH_EDGE) | TILE_FLIP_Y);
    }
    // A lane up to the cottage door (columns 5 and 6, rows 4 to 8).
    for row in 4..=8 {
        map.set(5, row, tile(G_PATH_EDGE) | TILE_FLIP_DIAGONAL);
        map.set(6, row, tile(G_PATH_EDGE) | TILE_FLIP_DIAGONAL | TILE_FLIP_X);
    }
    map.set(5, 4, tile(G_PATH_CORNER));
    map.set(6, 4, tile(G_PATH_CORNER) | TILE_FLIP_X);
    map.set(5, 9, tile(G_PATH));
    map.set(6, 9, tile(G_PATH));
    // The square around the well (columns 9 to 13, rows 5 to 8).
    for row in 5..=8 {
        for col in 9..=13 {
            let v = if row == 5 {
                tile(G_COBBLE_EDGE)
            } else if col == 9 {
                tile(G_COBBLE_EDGE) | TILE_FLIP_DIAGONAL
            } else if col == 13 {
                tile(G_COBBLE_EDGE) | TILE_FLIP_DIAGONAL | TILE_FLIP_X
            } else {
                tile(G_COBBLE)
            };
            map.set(col, row, v);
        }
    }
    for col in 9..=13 {
        map.set(col, 9, tile(G_COBBLE));
    }
    // The vegetable garden (columns 15 to 19, rows 2 to 4).
    for row in 2..=4 {
        for col in 15..=19 {
            let v = if (col + row) % 3 == 0 {
                G_SOIL
            } else {
                G_SPROUTS
            };
            map.set(col, row, tile(v));
        }
    }
    map
}

struct Diorama {
    app: App,
}

/// The diorama's light: the sun or moon (shining along `sun_dir`), the exposure, and the
/// lanterns, windows and campfire (lit when `lamps`).
#[derive(Clone, Copy)]
struct Lighting {
    sun_dir: DVec3,
    sun: DirectionalLight,
    ev100: f32,
    lamps: bool,
    /// Lantern glass and window luminance (cd/m²) and the lanterns' and fire's light (lm).
    glow_nits: f32,
    lantern_lm: f32,
    fire_lm: f32,
}

impl Lighting {
    /// Mid-afternoon: the sun high on the left, in front.
    fn day() -> Self {
        Self {
            sun_dir: DVec3::new(0.5, -0.7, -0.5),
            sun: DirectionalLight::default(),
            ev100: 14.0,
            lamps: false,
            glow_nits: 0.0,
            lantern_lm: 0.0,
            fire_lm: 800.0,
        }
    }

    /// The sun `elevation_deg` above the horizon behind the village (the atmosphere reddens
    /// and dims it), lamps lit.
    fn dusk(elevation_deg: f64, ev100: f32) -> Self {
        let e = elevation_deg.to_radians();
        Self {
            sun_dir: DVec3::new(-0.55 * e.cos(), -e.sin(), 0.83 * e.cos()),
            sun: DirectionalLight::default(),
            ev100,
            lamps: true,
            glow_nits: 600.0,
            lantern_lm: 500.0,
            fire_lm: 2500.0,
        }
    }

    /// Night under a bright moon: the lanterns, windows and fire light the village.
    fn night() -> Self {
        Self {
            sun_dir: DVec3::new(0.35, -0.75, -0.55),
            sun: DirectionalLight {
                color: [0.6, 0.72, 1.0],
                illuminance: 1.0,
                ..Default::default()
            },
            ev100: 0.5,
            lamps: true,
            glow_nits: 2.5,
            lantern_lm: 450.0,
            fire_lm: 900.0,
        }
    }
}

/// The diorama under `light`.
fn diorama(light: Lighting) -> Diorama {
    let dusk = light.lamps;
    let mut app = headless_app(1280, 720);
    app.world.resource_mut::<FogSettings>().density = 0.0;
    let w = &mut app.world;
    w.spawn((
        Camera {
            exposure: Exposure::Manual { ev100: light.ev100 },
            ..Camera::perspective(32.0)
        },
        Transform::from_xyz(0.0, 10.5, 19.5).looking_at(DVec3::new(0.0, 0.4, -0.3), Vec3::Y),
    ));
    w.spawn((
        light.sun,
        Transform::default().looking_at(light.sun_dir, Vec3::Y),
    ));

    // The table and the board.
    w.spawn((
        Transform::from_xyz(0.0, -1.0, 0.0),
        MeshRenderer::new(
            Shape::Plane {
                size: [400.0, 400.0],
                subdivisions: 1,
            },
            StandardMaterial::color(0.045, 0.028, 0.018).with_roughness(0.5),
        ),
    ));
    w.spawn((
        Transform::from_xyz(0.0, -0.5, 0.0),
        MeshRenderer::new(
            Shape::RoundedCuboid {
                size: [BOARD_W as f32 + 0.6, 1.0, BOARD_H as f32 + 0.6],
                radius: 0.08,
                segments: 0,
            },
            StandardMaterial::color(0.3, 0.16, 0.08).with_roughness(0.6),
        ),
    ));

    // The cottage.
    let plaster = StandardMaterial::color(0.78, 0.68, 0.52).with_roughness(0.9);
    let timber = StandardMaterial::color(0.2, 0.11, 0.06).with_roughness(0.8);
    let stone = StandardMaterial::color(0.42, 0.42, 0.44).with_roughness(0.85);
    let house = board(5.5, 2.1);
    w.spawn((
        Transform::from_translation(house + DVec3::new(0.0, 1.2, 0.0)),
        MeshRenderer::new(
            Shape::RoundedCuboid {
                size: [3.6, 2.4, 2.8],
                radius: 0.06,
                segments: 0,
            },
            plaster,
        ),
    ));
    w.spawn((
        Transform::from_translation(house + DVec3::new(0.0, 3.2, 0.0))
            .with_rotation(Quat::from_rotation_y(std::f32::consts::FRAC_PI_4)),
        MeshRenderer::new(
            Shape::Cone {
                radius: 2.9,
                height: 1.6,
                segments: 4,
            },
            StandardMaterial::color(0.5, 0.17, 0.1).with_roughness(0.75),
        ),
    ));
    w.spawn((
        Transform::from_translation(house + DVec3::new(1.0, 3.4, -0.6)),
        MeshRenderer::new(
            Shape::Cuboid {
                size: [0.45, 1.4, 0.45],
            },
            stone,
        ),
    ));
    w.spawn((
        Transform::from_translation(house + DVec3::new(0.0, 0.7, 1.41)),
        MeshRenderer::new(
            Shape::Cuboid {
                size: [0.8, 1.4, 0.06],
            },
            timber,
        ),
    ));
    let window = if dusk {
        StandardMaterial::color(0.9, 0.7, 0.4).with_emissive([1.0, 0.62, 0.3], light.glow_nits)
    } else {
        StandardMaterial::color(0.25, 0.35, 0.45).with_roughness(0.2)
    };
    for dx in [-1.1, 1.1] {
        w.spawn((
            Transform::from_translation(house + DVec3::new(dx, 1.4, 1.41)),
            MeshRenderer::new(
                Shape::Cuboid {
                    size: [0.6, 0.6, 0.05],
                },
                window,
            ),
        ));
        w.spawn((
            Transform::from_translation(house + DVec3::new(dx, 1.4, 1.42)),
            MeshRenderer::new(
                Shape::Cuboid {
                    size: [0.7, 0.08, 0.05],
                },
                timber,
            ),
        ));
    }
    for (dx, dz) in [(-1.78, 1.38), (1.78, 1.38), (-1.78, -1.38), (1.78, -1.38)] {
        w.spawn((
            Transform::from_translation(house + DVec3::new(dx, 1.2, dz)),
            MeshRenderer::new(
                Shape::Cuboid {
                    size: [0.14, 2.4, 0.14],
                },
                timber,
            ),
        ));
    }

    // The well on the square.
    let well = board(11.0, 6.8);
    w.spawn((
        Transform::from_translation(well + DVec3::new(0.0, 0.4, 0.0)),
        MeshRenderer::new(
            Shape::Cylinder {
                radius: 0.75,
                height: 0.8,
                segments: 0,
            },
            stone,
        ),
    ));
    w.spawn((
        Transform::from_translation(well + DVec3::new(0.0, 0.41, 0.0)),
        MeshRenderer::new(
            Shape::Cylinder {
                radius: 0.58,
                height: 0.8,
                segments: 0,
            },
            StandardMaterial::color(0.02, 0.03, 0.05).with_roughness(0.05),
        ),
    ));
    for dx in [-0.7, 0.7] {
        w.spawn((
            Transform::from_translation(well + DVec3::new(dx, 1.0, 0.0)),
            MeshRenderer::new(
                Shape::Cuboid {
                    size: [0.12, 1.6, 0.12],
                },
                timber,
            ),
        ));
    }
    w.spawn((
        Transform::from_translation(well + DVec3::new(0.0, 2.05, 0.0))
            .with_rotation(Quat::from_rotation_y(std::f32::consts::FRAC_PI_4)),
        MeshRenderer::new(
            Shape::Cone {
                radius: 1.15,
                height: 0.6,
                segments: 4,
            },
            StandardMaterial::color(0.5, 0.17, 0.1).with_roughness(0.75),
        ),
    ));

    // Rocks, barrels and crates.
    for (i, (col, row, s)) in [
        (3.2, 6.2, 1.1),
        (20.3, 7.5, 0.9),
        (13.8, 1.0, 0.7),
        (2.5, 12.4, 0.8),
    ]
    .into_iter()
    .enumerate()
    {
        w.spawn((
            Transform::from_translation(board(col, row)),
            MeshRenderer::new(
                Shape::Rock {
                    size: [1.2 * s, 0.8 * s, 1.0 * s],
                    seed: i as u32 + 4,
                    detail: 0,
                },
                StandardMaterial::color(0.36, 0.36, 0.38).with_roughness(0.9),
            ),
        ));
    }
    let wood = StandardMaterial::color(0.36, 0.2, 0.1).with_roughness(0.7);
    for (dx, dz) in [(2.3, 0.6), (2.5, 1.3)] {
        w.spawn((
            Transform::from_translation(house + DVec3::new(dx, 0.42, dz)),
            MeshRenderer::new(
                Shape::Cylinder {
                    radius: 0.32,
                    height: 0.84,
                    segments: 0,
                },
                wood,
            ),
        ));
    }
    w.spawn((
        Transform::from_translation(house + DVec3::new(-2.4, 0.35, 1.0))
            .with_rotation(Quat::from_rotation_y(0.3)),
        MeshRenderer::new(
            Shape::RoundedCuboid {
                size: [0.7, 0.7, 0.7],
                radius: 0.04,
                segments: 0,
            },
            wood,
        ),
    ));

    // The campfire: logs, stones, flames (additive, unlit) and its light.
    let fire = board(16.5, 12.3);
    for a in [0.4f32, 2.0, 3.4] {
        w.spawn((
            Transform::from_translation(fire + DVec3::new(0.0, 0.12, 0.0))
                .with_rotation(Quat::from_rotation_y(a) * Quat::from_rotation_z(FRAC_PI_2 - 0.25)),
            MeshRenderer::new(
                Shape::Cylinder {
                    radius: 0.08,
                    height: 0.9,
                    segments: 0,
                },
                StandardMaterial::color(0.12, 0.07, 0.04).with_roughness(0.9),
            ),
        ));
    }
    for i in 0..8 {
        let a = i as f64 / 8.0 * std::f64::consts::TAU;
        w.spawn((
            Transform::from_translation(fire + DVec3::new(a.cos() * 0.6, 0.0, a.sin() * 0.6)),
            MeshRenderer::new(
                Shape::Rock {
                    size: [0.3, 0.2, 0.25],
                    seed: 20 + i,
                    detail: 0,
                },
                stone,
            ),
        ));
    }

    let ground = add_image(&mut app, ground_tileset().image());
    let map = diorama_ground(ground);
    app.world.spawn((
        Transform::from_xyz(0.0, 0.002, 0.0).with_rotation(Quat::from_rotation_x(-FRAC_PI_2)),
        map,
    ));

    // Paper sprites: trees and bushes stand up toward the camera (upright billboards) with
    // normal maps; fences and the sign are fixed cards.
    let paper = |app: &mut App, canvas: &Canvas, normals: Option<Image>, px: [u32; 2]| {
        let image = add_image(app, canvas.image());
        let mut sprite = Sprite::pixel_art(image, px, PPU)
            .with_anchor([0.5, 0.0])
            .lit()
            .with_billboard(Billboard::Upright);
        if let Some(n) = normals {
            sprite = sprite.with_normal_map(add_image(app, n));
        }
        sprite
    };
    let trees: Vec<Sprite> = (0..3)
        .map(|i| {
            let (c, n) = round_tree(70 + i);
            paper(&mut app, &c, Some(n), [48, 64]).with_translucency(0.35)
        })
        .collect();
    let pines: Vec<Sprite> = (0..2)
        .map(|i| {
            let (c, n) = pine_tree(80 + i);
            paper(&mut app, &c, Some(n), [32, 64]).with_translucency(0.25)
        })
        .collect();
    let bushes: Vec<Sprite> = [None, Some(hex(0xf28fb5)), Some(YELLOW)]
        .into_iter()
        .enumerate()
        .map(|(i, f)| {
            let (c, n) = bush(90 + i as u32, f);
            paper(&mut app, &c, Some(n), [32, 20]).with_translucency(0.3)
        })
        .collect();
    let place = |app: &mut App, at: DVec3, sprite: Sprite| {
        app.world.spawn((Transform::from_translation(at), sprite));
    };
    for (col, row, i) in [
        (1.0, 1.0, 0usize),
        (9.0, 0.6, 1),
        (19.5, 0.8, 2),
        (0.8, 7.0, 1),
        (21.0, 12.0, 0),
    ] {
        place(&mut app, board(col, row), trees[i].clone());
    }
    for (col, row, i) in [
        (2.6, 0.2, 0usize),
        (12.5, 0.3, 1),
        (16.0, 0.4, 0),
        (21.2, 3.5, 1),
        (0.4, 4.0, 0),
        (21.0, 7.8, 0),
    ] {
        place(&mut app, board(col, row), pines[i].clone());
    }
    for (col, row, i) in [
        (2.0, 4.6, 1usize),
        (8.2, 3.8, 0),
        (14.6, 5.4, 2),
        (18.6, 6.4, 0),
        (2.0, 7.6, 2),
        (8.5, 12.2, 0),
        (12.5, 12.6, 1),
        (19.6, 11.6, 2),
    ] {
        place(
            &mut app,
            board(col, row),
            bushes[i].clone().with_roundness(0.3),
        );
    }
    // Meadow flowers and tufts.
    let flowers: Vec<Sprite> = [(RED, YELLOW), (WHITE, YELLOW), (hex(0x8f7bd8), WHITE)]
        .iter()
        .map(|&(p, c)| paper(&mut app, &flower_sprite(p, c), None, [8, 10]))
        .collect();
    let tufts: Vec<Sprite> = (0..2)
        .map(|i| paper(&mut app, &tuft_sprite(60 + i), None, [12, 10]))
        .collect();
    for i in 0..70 {
        let col = hash(i, 0, 300) * BOARD_W as f32 - 0.5;
        let row = hash(i, 1, 300) * BOARD_H as f32 - 0.5;
        // Not on the road, the lane, the square or the garden.
        let on_paths = (8.5..11.5).contains(&row)
            || ((4.5..7.5).contains(&col) && (3.5..9.0).contains(&row))
            || ((8.5..14.5).contains(&col) && (4.5..9.5).contains(&row))
            || ((14.5..20.5).contains(&col) && (1.5..5.5).contains(&row))
            || ((2.5..9.0).contains(&col) && (0.8..4.4).contains(&row))
            || ((15.0..18.0).contains(&col) && (11.2..13.4).contains(&row));
        if on_paths {
            continue;
        }
        let sprite = if i % 3 == 0 {
            tufts[(i / 3) as usize % 2].clone()
        } else {
            flowers[i as usize % 3].clone()
        };
        place(&mut app, board(col as f64, row as f64), sprite);
    }
    // The garden fence (fixed cards along its front and sides).
    let fence_card = {
        let image = add_image(&mut app, fence().image());
        Sprite::pixel_art(image, [32, 16], PPU)
            .with_anchor([0.5, 0.0])
            .lit()
    };
    for k in 0..3 {
        place(
            &mut app,
            board(15.5 + k as f64 * 2.0, 4.95),
            fence_card.clone(),
        );
    }
    for (col, side) in [(14.5, 1.0f32), (20.5, -1.0)] {
        for k in 0..2 {
            app.world.spawn((
                Transform::from_translation(board(col, 2.0 + k as f64 * 2.0))
                    .with_rotation(Quat::from_rotation_y(FRAC_PI_2 * side)),
                fence_card.clone(),
            ));
        }
    }
    let sign = add_image(&mut app, signpost().image());
    place(
        &mut app,
        board(7.6, 8.4),
        Sprite::pixel_art(sign, [24, 24], PPU)
            .with_anchor([0.5, 0.0])
            .lit()
            .with_billboard(Billboard::Upright),
    );

    // People: two villagers by the well and the hero from the side-scroller on the road.
    let villagers = [
        villager_sheet(HAIR, RED, hex(0x8a2f42), true),
        villager_sheet(hex(0xd9a066), BLUE, NAVY, false),
    ];
    for (i, (sheet, col, row)) in villagers
        .iter()
        .zip([(9.6, 7.6), (12.5, 8.2)])
        .map(|(s, (c, r))| (s, c, r))
        .enumerate()
    {
        let image = add_image(&mut app, sheet.image());
        let mut animation = SpriteAnimation::new(0..2, 2.0);
        animation.time = i as f32 * 0.3;
        app.world.spawn((
            Transform::from_translation(board(col, row)),
            Sprite::pixel_art(image, [16, 24], PPU)
                .with_atlas(SpriteAtlas::grid(2, 1))
                .with_anchor([0.5, 0.0])
                .lit()
                .with_billboard(Billboard::Upright),
            animation,
        ));
    }
    let hero = add_image(&mut app, hero_sheet().image());
    app.world.spawn((
        Transform::from_translation(board(3.5, 9.6)),
        Sprite::pixel_art(hero, [24, 24], PPU)
            .with_atlas(SpriteAtlas::grid(8, 1))
            .with_anchor([0.5, 0.0])
            .lit()
            .with_billboard(Billboard::Upright),
        SpriteAnimation::new(6..8, 2.0),
    ));

    // Lanterns along the road and the campfire's flames.
    let lantern_image = add_image(&mut app, lantern().image());
    let glow_image = add_image(&mut app, glow());
    let post = StandardMaterial::color(0.08, 0.08, 0.09).with_roughness(0.5);
    for col in [4.0, 13.0] {
        let base = board(col, 8.3);
        app.world.spawn((
            Transform::from_translation(base + DVec3::new(0.0, 0.9, 0.0)),
            MeshRenderer::new(
                Shape::Cylinder {
                    radius: 0.05,
                    height: 1.8,
                    segments: 0,
                },
                post,
            ),
        ));
        let mut head = Sprite::pixel_art(lantern_image, [8, 12], PPU)
            .with_anchor([0.5, 0.0])
            .with_billboard(Billboard::Upright)
            .lit();
        if dusk {
            head = head.with_emissive(light.glow_nits);
            app.world.spawn((
                Transform::from_translation(base + DVec3::new(0.0, 2.15, 0.25)),
                Sprite::new(glow_image, [1.6, 1.6])
                    .with_color([1.0, 0.65, 0.3, 0.35])
                    .with_alpha(SpriteAlpha::Additive)
                    .with_billboard(Billboard::FaceCamera),
            ));
            app.world.spawn((
                Transform::from_translation(base + DVec3::new(0.0, 2.15, 0.3)),
                PointLight {
                    color: [1.0, 0.7, 0.4],
                    intensity_lm: light.lantern_lm,
                    range: 7.0,
                    radius: 0.05,
                },
            ));
        }
        app.world.spawn((
            Transform::from_translation(base + DVec3::new(0.0, 1.8, 0.0)),
            head,
        ));
    }
    let flames = add_image(&mut app, flame_sheet().image());
    app.world.spawn((
        Transform::from_translation(fire + DVec3::new(0.0, 0.05, 0.05)),
        Sprite::pixel_art(flames, [16, 24], PPU)
            .with_atlas(SpriteAtlas::grid(4, 1))
            .with_anchor([0.5, 0.0])
            .with_alpha(SpriteAlpha::Additive)
            .with_billboard(Billboard::Upright),
        SpriteAnimation::new(0..4, 8.0),
    ));
    app.world.spawn((
        Transform::from_translation(fire + DVec3::new(0.0, 0.7, 0.2)),
        Sprite::new(glow_image, [3.0, 3.0])
            .with_color([1.0, 0.5, 0.2, if dusk { 0.45 } else { 0.15 }])
            .with_alpha(SpriteAlpha::Additive)
            .with_billboard(Billboard::FaceCamera),
    ));
    app.world.spawn((
        Transform::from_translation(fire + DVec3::new(0.0, 0.6, 0.0)),
        PointLight {
            color: [1.0, 0.45, 0.18],
            intensity_lm: light.fire_lm,
            range: 9.0,
            radius: 0.2,
        },
    ));
    Diorama { app }
}

// ---------------------------------------------------------------------------------------------

fn save(app: &mut App, out: &Path, name: &str) {
    let path = out.join(format!("{name}.png"));
    capture_now(&mut app.world, &path).expect("screenshot");
    println!("{}", path.display());
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let out = std::path::PathBuf::from(args.first().map_or("sprites_2d", |s| s.as_str()));
    // Optional: which scenes to render (`side`, `day`, `dusk`, `night`; all by default).
    let only = args.get(1).cloned().unwrap_or_default();
    let wanted = |scene: &str| only.is_empty() || only.split(',').any(|s| s == scene);
    std::fs::create_dir_all(&out).expect("output directory");

    if wanted("side") {
        let SideScroller { mut app, camera } = side_scroller();
        app.update_n(5);
        save(&mut app, &out, "side_scroller_a");
        // Further along (at a fractional position: the camera snaps to whole texels).
        app.world.get_mut::<Transform>(camera).unwrap().translation =
            DVec3::new(31.37, CAMERA_Y, 50.0);
        app.update_n(4);
        save(&mut app, &out, "side_scroller_b");
    }
    if wanted("day") {
        let Diorama { mut app } = diorama(Lighting::day());
        app.update_n(8);
        save(&mut app, &out, "diorama_day");
    }
    if wanted("dusk") {
        let Diorama { mut app } = diorama(Lighting::dusk(4.0, 10.0));
        app.update_n(8);
        save(&mut app, &out, "diorama_dusk");
    }
    if wanted("night") {
        let Diorama { mut app } = diorama(Lighting::night());
        app.update_n(8);
        save(&mut app, &out, "diorama_night");
    }
    // Lighting experiments: `exp:<elevation>:<ev100>`.
    for e in only.split(',').filter_map(|s| s.strip_prefix("exp:")) {
        let v: Vec<f64> = e.split(':').filter_map(|v| v.parse().ok()).collect();
        let Diorama { mut app } = diorama(Lighting::dusk(v[0], v[1] as f32));
        app.update_n(8);
        save(&mut app, &out, &format!("diorama_exp_{}_{}", v[0], v[1]));
    }
}
