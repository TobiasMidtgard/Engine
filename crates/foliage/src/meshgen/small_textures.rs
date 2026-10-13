//! Procedural textures for small plants and cave fungi, painted on the CPU.
//!
//! - The **meadow atlas** ([`meadow_atlas`]) holds everything grass, wildflowers and bluebells
//!   need in one image, so each species still draws with one material: a grass blade, an airy
//!   grass seed head, a tuft of blades that fills a clump, a narrow (daisy) and a broad (poppy,
//!   buttercup) petal, a toothed leaf, a disc of florets and a bluebell's six tepals, plus two
//!   round mats of short grass seen from above (lush sward and drier thatch) that lie under
//!   grass clumps and merge into a continuous turf.
//!   [`Cell::uv`] maps a part's own coordinates (across, along) into the atlas.
//! - The **fern pinna** ([`fern_pinna`]): one leaflet of a frond, cut into lobed pinnules
//!   with a costa and veins. Each pinna of a frond is a small card with this texture.
//! - **Fungus maps** ([`fungus`]) for glowcaps and lantern caps: albedo, normal and emission
//!   over a lathe layout (cap top, gills, stem; see [`cap_uv`], [`gill_uv`], [`stem_uv`]), so
//!   gills, rims, spots and translucent membranes glow while cap tops stay darker.
//!
//! Colors are near-neutral: vertex and material colors give hue, so one texture serves every
//! tint. Normal maps are tangent space with +Y up (glTF/OpenGL), derived from painted height.
//! Alpha-masked images carry their own mip chain that keeps the coverage of the full-size
//! image, so distant grass and fronds stay full instead of dissolving.

use engine_core::math::{TAU, Vec2, hash01, noise, smoothstep};
use engine_render::Image;

/// Alpha cutoff of the alpha-masked small-plant textures.
pub const SMALL_ALPHA_CUTOFF: f32 = 0.5;

/// Albedo (sRGB, alpha cut-out), tangent-space normal map (linear) and, for fungi, a linear
/// emission mask (white = full glow).
pub struct TextureSet {
    pub albedo: Image,
    pub normal: Image,
    pub emissive: Option<Image>,
}

/// A region of the meadow atlas: 4 columns × 2 rows of tall cells over a row of two square
/// mats.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Cell {
    /// An opaque grass blade (also stems and bluebell leaves): parallel veins, a folded midrib.
    Blade,
    /// An airy grass panicle on its culm (alpha).
    SeedHead,
    /// A tuft of grass blades for filling a clump (alpha).
    Tuft,
    /// A narrow ray petal with a notched tip (daisies).
    PetalNarrow,
    /// A broad obovate petal with radiating veins (poppies, buttercups, cranesbills).
    PetalBroad,
    /// A lanceolate, toothed leaf with a midrib and side veins on a short petiole.
    Leaf,
    /// Packed disc florets (flower centers, opaque).
    Florets,
    /// Six bluebell tepals side by side (around = across), scalloped at the mouth.
    Bell,
    /// A round mat of short, lush grass seen from above over dark thatch (alpha at the rim).
    /// Map it with `uv(0.5 + x/2, 0.5 + z/2)` for a point (x, z) of the unit disc.
    Sward,
    /// Like [`Cell::Sward`], drier: more straw and thatch between the blades.
    Thatch,
}

impl Cell {
    pub const ALL: [Cell; 10] = [
        Cell::Blade,
        Cell::SeedHead,
        Cell::Tuft,
        Cell::PetalNarrow,
        Cell::PetalBroad,
        Cell::Leaf,
        Cell::Florets,
        Cell::Bell,
        Cell::Sward,
        Cell::Thatch,
    ];

    fn index(self) -> u32 {
        Cell::ALL.iter().position(|c| *c == self).unwrap_or(0) as u32
    }

    /// The cell's rectangle in the atlas: `[u0, v0, width, height]` in texture coordinates.
    pub fn rect(self) -> [f32; 4] {
        const ROW: f32 = 1.0 / 3.0;
        match self {
            Cell::Sward => [0.0, 2.0 * ROW, 0.5, ROW],
            Cell::Thatch => [0.5, 2.0 * ROW, 0.5, ROW],
            _ => {
                let i = self.index();
                [(i % 4) as f32 * 0.25, (i / 4) as f32 * ROW, 0.25, ROW]
            }
        }
    }

    /// Atlas coordinates of a point given across the part (`s`, 0 left .. 1 right) and along
    /// it (`t`, 0 base .. 1 tip). Values are kept a texel inside the cell.
    pub fn uv(self, s: f32, t: f32) -> Vec2 {
        let r = self.rect();
        let s = s.clamp(0.01, 0.99);
        let t = t.clamp(0.005, 0.995);
        Vec2::new(r[0] + s * r[2], r[1] + (1.0 - t) * r[3])
    }

    fn pixels(self, w: u32, h: u32) -> [i32; 4] {
        let r = self.rect();
        let (w, h) = (w as f32, h as f32);
        [
            (r[0] * w).round() as i32,
            (r[1] * h).round() as i32,
            (r[2] * w).round() as i32,
            (r[3] * h).round() as i32,
        ]
    }
}

/// Texture coordinates on a mushroom cap's top: `around` 0..1, `r` 0 at the apex .. 1 at the
/// margin.
pub fn cap_uv(around: f32, r: f32) -> Vec2 {
    Vec2::new(around, 0.005 + r.clamp(0.0, 1.0) * 0.46)
}

/// Texture coordinates on the gills under a cap: `g` 0 at the margin .. 1 at the stem.
pub fn gill_uv(around: f32, g: f32) -> Vec2 {
    Vec2::new(around, 0.5 + g.clamp(0.0, 1.0) * 0.25)
}

/// Texture coordinates on a stem: `q` 0 just under the cap .. 1 at the base.
pub fn stem_uv(around: f32, q: f32) -> Vec2 {
    Vec2::new(around, 0.78 + q.clamp(0.0, 1.0) * 0.215)
}

/// Width of the fern texture taken by the pinna; an opaque strip for stems sits beside it.
const PINNA_WIDTH: f32 = 0.84;

/// Texture coordinates on a fern pinna: `s` across 0..1, `t` along 0 (base) .. 1 (tip).
pub fn pinna_uv(s: f32, t: f32) -> Vec2 {
    Vec2::new(
        s.clamp(0.01, 0.99) * PINNA_WIDTH,
        1.0 - t.clamp(0.005, 0.995),
    )
}

/// Texture coordinates on the fern texture's opaque stem strip (rachis, stipe, fiddleheads):
/// `around` 0..1, `along` 0..1.
pub fn fern_stem_uv(around: f32, along: f32) -> Vec2 {
    Vec2::new(
        0.9 + 0.08 * around.clamp(0.0, 1.0),
        0.03 + 0.94 * along.clamp(0.0, 1.0),
    )
}

/// Which fungus to paint.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Fungus {
    /// Small bell-to-dome caps: striate margins, pale scales, glowing gills and rims.
    Glowcap,
    /// Tall translucent bell: ribbed, veined membrane glowing from inside, glowing gills.
    Lantern,
}

// ---------------------------------------------------------------- canvas

#[derive(Debug, Clone, Copy, Default)]
struct Texel {
    /// Linear, straight (not premultiplied) color.
    color: [f32; 3],
    alpha: f32,
    /// Height in pixels (for the normal map).
    height: f32,
}

impl Texel {
    fn new(lum: [f32; 3], alpha: f32, height: f32) -> Self {
        Self {
            color: lum,
            alpha,
            height,
        }
    }
}

struct Canvas {
    w: u32,
    h: u32,
    px: Vec<Texel>,
}

impl Canvas {
    fn new(w: u32, h: u32) -> Self {
        Self {
            w,
            h,
            px: vec![Texel::default(); (w * h) as usize],
        }
    }

    fn idx(&self, x: i32, y: i32) -> Option<usize> {
        (x >= 0 && y >= 0 && x < self.w as i32 && y < self.h as i32)
            .then(|| (y as u32 * self.w + x as u32) as usize)
    }

    /// Paint `t` over the texel with coverage `a` (straight-alpha "over").
    fn blend(&mut self, x: i32, y: i32, t: Texel, a: f32) {
        let Some(i) = self.idx(x, y) else { return };
        let a = a.clamp(0.0, 1.0);
        if a <= 0.0 {
            return;
        }
        let p = &mut self.px[i];
        let out = a + p.alpha * (1.0 - a);
        for c in 0..3 {
            p.color[c] = (t.color[c] * a + p.color[c] * p.alpha * (1.0 - a)) / out.max(1e-6);
        }
        p.height = p.height * (1.0 - a) + t.height * a;
        p.alpha = out;
    }

    /// Evaluate `f(s, t)` over a cell rectangle (`s` across 0..1, `t` along 0..1 bottom to top)
    /// and write the result.
    fn fill(&mut self, rect: [i32; 4], mut f: impl FnMut(f32, f32) -> Texel) {
        for y in rect[1]..rect[1] + rect[3] {
            for x in rect[0]..rect[0] + rect[2] {
                let s = (x - rect[0]) as f32 + 0.5;
                let t = (y - rect[1]) as f32 + 0.5;
                let texel = f(s / rect[2] as f32, 1.0 - t / rect[3] as f32);
                if let Some(i) = self.idx(x, y) {
                    self.px[i] = texel;
                }
            }
        }
    }

    /// An anti-aliased tapered stroke from `a` to `b` (pixels), clipped to `clip`; its height
    /// is a rounded ridge `height` pixels high.
    #[allow(clippy::too_many_arguments)]
    fn stroke(
        &mut self,
        a: Vec2,
        b: Vec2,
        w0: f32,
        w1: f32,
        color: [f32; 3],
        height: f32,
        clip: [i32; 4],
    ) {
        let w = w0.max(w1);
        let lo = a.min(b) - Vec2::splat(w + 1.0);
        let hi = a.max(b) + Vec2::splat(w + 1.0);
        let ab = b - a;
        let len2 = ab.length_squared().max(1e-6);
        let x0 = (lo.x as i32).max(clip[0]);
        let x1 = (hi.x as i32).min(clip[0] + clip[2] - 1);
        let y0 = (lo.y as i32).max(clip[1]);
        let y1 = (hi.y as i32).min(clip[1] + clip[3] - 1);
        for y in y0..=y1 {
            for x in x0..=x1 {
                let p = Vec2::new(x as f32 + 0.5, y as f32 + 0.5);
                let t = ((p - a).dot(ab) / len2).clamp(0.0, 1.0);
                let d = p.distance(a + ab * t);
                let half = (w0 + (w1 - w0) * t) * 0.5;
                let cov = (half + 0.5 - d).clamp(0.0, 1.0);
                if cov > 0.0 {
                    let ridge = (1.0 - (d / half.max(0.5)).powi(2)).max(0.0).sqrt();
                    self.blend(x, y, Texel::new(color, 1.0, height * ridge), cov);
                }
            }
        }
    }

    /// A tapered stroke along a quadratic Bézier (`a` → `c` with control `m`); `color(t)` and
    /// widths vary along it.
    #[allow(clippy::too_many_arguments)]
    fn curve(
        &mut self,
        a: Vec2,
        m: Vec2,
        c: Vec2,
        w0: f32,
        w1: f32,
        color: &dyn Fn(f32) -> [f32; 3],
        height: f32,
        clip: [i32; 4],
    ) {
        let n = 14;
        let at = |t: f32| a * (1.0 - t) * (1.0 - t) + m * 2.0 * t * (1.0 - t) + c * t * t;
        for i in 0..n {
            let (t0, t1) = (i as f32 / n as f32, (i + 1) as f32 / n as f32);
            self.stroke(
                at(t0),
                at(t1),
                w0 + (w1 - w0) * t0,
                w0 + (w1 - w0) * t1,
                color(t0),
                height,
                clip,
            );
        }
    }
}

// ---------------------------------------------------------------- encoding

fn linear_to_srgb(c: f32) -> f32 {
    if c <= 0.003_130_8 {
        c * 12.92
    } else {
        1.055 * c.powf(1.0 / 2.4) - 0.055
    }
}

fn to_u8(v: f32) -> u8 {
    (v.clamp(0.0, 1.0) * 255.0 + 0.5) as u8
}

/// Give see-through texels the color of their nearest painted neighbors, so filtering never
/// pulls a dark fringe into the cut-out edges.
fn dilate(px: &mut [[f32; 4]], w: u32, h: u32) {
    let mut known: Vec<bool> = px.iter().map(|p| p[3] > 0.02).collect();
    for _ in 0..16 {
        let mut updates = Vec::new();
        for y in 0..h as i32 {
            for x in 0..w as i32 {
                let i = (y as u32 * w + x as u32) as usize;
                if known[i] {
                    continue;
                }
                let (mut sum, mut n) = ([0.0f32; 3], 0.0f32);
                for (dx, dy) in [(-1, 0), (1, 0), (0, -1), (0, 1)] {
                    let (nx, ny) = (x + dx, y + dy);
                    if nx < 0 || ny < 0 || nx >= w as i32 || ny >= h as i32 {
                        continue;
                    }
                    let j = (ny as u32 * w + nx as u32) as usize;
                    if known[j] {
                        for c in 0..3 {
                            sum[c] += px[j][c];
                        }
                        n += 1.0;
                    }
                }
                if n > 0.0 {
                    updates.push((i, sum.map(|v| v / n)));
                }
            }
        }
        if updates.is_empty() {
            break;
        }
        for (i, c) in updates {
            px[i][..3].copy_from_slice(&c);
            known[i] = true;
        }
    }
    let (mut sum, mut n) = ([0.0f32; 3], 0.0f32);
    for (p, k) in px.iter().zip(&known) {
        if *k {
            for c in 0..3 {
                sum[c] += p[c];
            }
            n += 1.0;
        }
    }
    let avg = sum.map(|v| v / n.max(1.0));
    for (p, k) in px.iter_mut().zip(&known) {
        if !*k {
            p[..3].copy_from_slice(&avg);
        }
    }
}

fn coverage(level: &[[f32; 4]], scale: f32) -> f32 {
    level
        .iter()
        .filter(|p| p[3] * scale >= SMALL_ALPHA_CUTOFF)
        .count() as f32
        / level.len().max(1) as f32
}

/// sRGB albedo with a mip chain. With `cutout`, each mip's alpha is scaled so as many texels
/// pass the alpha test as in the full-size image.
fn encode_albedo(cv: &Canvas, cutout: bool) -> Image {
    let mut px: Vec<[f32; 4]> = cv
        .px
        .iter()
        .map(|t| {
            let a = if cutout { t.alpha } else { 1.0 };
            [t.color[0], t.color[1], t.color[2], a]
        })
        .collect();
    if cutout {
        dilate(&mut px, cv.w, cv.h);
    }
    let mut levels = vec![(cv.w, cv.h, px)];
    loop {
        let (w, h, prev) = levels.last().unwrap();
        let (w, h) = (*w, *h);
        if w == 1 && h == 1 {
            break;
        }
        let (nw, nh) = ((w / 2).max(1), (h / 2).max(1));
        let mut next = vec![[0.0f32; 4]; (nw * nh) as usize];
        for y in 0..nh {
            for x in 0..nw {
                let mut acc = [0.0f32; 4];
                for (dx, dy) in [(0, 0), (1, 0), (0, 1), (1, 1)] {
                    let sx = (x * 2 + dx).min(w - 1);
                    let sy = (y * 2 + dy).min(h - 1);
                    let q = prev[(sy * w + sx) as usize];
                    for c in 0..4 {
                        acc[c] += q[c] * 0.25;
                    }
                }
                next[(y * nw + x) as usize] = acc;
            }
        }
        levels.push((nw, nh, next));
    }
    let target = coverage(&levels[0].2, 1.0);
    let encode = |level: &[[f32; 4]], alpha_scale: f32| -> Vec<u8> {
        let mut out = Vec::with_capacity(level.len() * 4);
        for p in level {
            for c in &p[..3] {
                out.push(to_u8(linear_to_srgb(*c)));
            }
            out.push(to_u8(p[3] * alpha_scale));
        }
        out
    };
    let data = encode(&levels[0].2, 1.0);
    let mips = levels[1..]
        .iter()
        .map(|(_, _, level)| {
            if !cutout {
                return encode(level, 1.0);
            }
            let (mut lo, mut hi) = (0.5f32, 8.0f32);
            for _ in 0..14 {
                let mid = 0.5 * (lo + hi);
                if coverage(level, mid) < target {
                    lo = mid;
                } else {
                    hi = mid;
                }
            }
            encode(level, 0.5 * (lo + hi))
        })
        .collect();
    let mut image = Image::new(cv.w, cv.h, data, true);
    image.mips = mips;
    image
}

/// Tangent-space normal map (+Y up) from the painted height; `strength` scales the slopes
/// across (u) and along (v). `wrap_u` makes the left and right edges meet (lathed surfaces).
fn encode_normal(cv: &Canvas, strength: [f32; 2], wrap_u: bool) -> Image {
    let (w, h) = (cv.w as i32, cv.h as i32);
    let height = |x: i32, y: i32| {
        let x = if wrap_u {
            x.rem_euclid(w)
        } else {
            x.clamp(0, w - 1)
        };
        cv.px[(y.clamp(0, h - 1) * w + x) as usize].height
    };
    let mut data = Vec::with_capacity((w * h * 4) as usize);
    for y in 0..h {
        for x in 0..w {
            let du = (height(x + 1, y) - height(x - 1, y)) * 0.5 * strength[0];
            let dv = (height(x, y + 1) - height(x, y - 1)) * 0.5 * strength[1];
            // Rows run down the image (+v), so "up" in tangent space is −v.
            let n = engine_core::math::Vec3::new(-du, dv, 1.0).normalize();
            data.extend_from_slice(&[
                to_u8(n.x * 0.5 + 0.5),
                to_u8(n.y * 0.5 + 0.5),
                to_u8(n.z * 0.5 + 0.5),
                255,
            ]);
        }
    }
    Image::new(cv.w, cv.h, data, false)
}

/// A gray mask stored sRGB-encoded, so the faint values (cap tops glow at about 1% of the
/// gills) keep their precision.
fn encode_gray(values: &[f32], w: u32, h: u32) -> Image {
    let mut data = Vec::with_capacity(values.len() * 4);
    for v in values {
        let b = to_u8(linear_to_srgb(v.clamp(0.0, 1.0)));
        data.extend_from_slice(&[b, b, b, 255]);
    }
    Image::new(w, h, data, true)
}

/// Gradient noise that wraps around in `u` (`periods` cells around), for lathed surfaces.
fn wrap_noise(u: f32, v: f32, periods: f32, v_freq: f32, seed: u64) -> f32 {
    let a = u * TAU;
    let r = periods / TAU;
    noise::perlin3(
        (a.cos() * r) as f64,
        (a.sin() * r) as f64,
        (v * v_freq) as f64,
        seed,
    )
}

fn fbm(x: f32, y: f32, seed: u64) -> f32 {
    noise::fbm2(x as f64, y as f64, seed, 3, 2.0, 0.5)
}

/// Neutral, slightly warm leaf tone of luminance `l`.
fn leafy(l: f32) -> [f32; 3] {
    [l * 1.02, l, l * 0.86]
}

/// Anti-aliased inside test: signed distance in pixels → coverage.
fn aa(d_px: f32) -> f32 {
    (d_px + 0.5).clamp(0.0, 1.0)
}

// ---------------------------------------------------------------- meadow atlas

/// The meadow atlas: `size` wide and `1.5 × size` tall (tall cells of `size/4` × `size/2`,
/// mats of `size/2` square). See [`Cell`].
pub fn meadow_atlas(size: u32) -> TextureSet {
    let size = size.max(64);
    let (w, h) = (size, size * 3 / 2);
    let mut cv = Canvas::new(w, h);
    let mut rng = engine_core::math::Rng::new(0x6d65_6164);
    paint_blade(&mut cv, Cell::Blade.pixels(w, h));
    paint_seed_head(&mut cv, Cell::SeedHead.pixels(w, h), &mut rng);
    paint_tuft(&mut cv, Cell::Tuft.pixels(w, h), &mut rng);
    paint_petal_narrow(&mut cv, Cell::PetalNarrow.pixels(w, h));
    paint_petal_broad(&mut cv, Cell::PetalBroad.pixels(w, h));
    paint_leaf(&mut cv, Cell::Leaf.pixels(w, h));
    paint_florets(&mut cv, Cell::Florets.pixels(w, h));
    paint_bell(&mut cv, Cell::Bell.pixels(w, h));
    paint_sward(&mut cv, Cell::Sward.pixels(w, h), &mut rng, 0.0, 0x5157);
    paint_sward(&mut cv, Cell::Thatch.pixels(w, h), &mut rng, 1.0, 0x7468);
    TextureSet {
        normal: encode_normal(&cv, [1.0, 1.0], false),
        albedo: encode_albedo(&cv, true),
        emissive: None,
    }
}

fn paint_blade(cv: &mut Canvas, r: [i32; 4]) {
    let cw = r[2] as f32;
    cv.fill(r, |s, t| {
        let x = s - 0.5;
        let vein = (0.5 + 0.5 * (s * 11.0 * TAU).cos()).powf(6.0);
        let mid = 1.0 - smoothstep(0.0, 0.05, x.abs());
        let edge = smoothstep(0.5, 0.33, x.abs());
        let streak = fbm(s * 9.0, t * 2.2, 11);
        let lum = 0.8
            * (1.0 - 0.12 * vein)
            * (1.0 + 0.14 * mid)
            * (0.84 + 0.16 * edge)
            * (1.0 + 0.12 * streak);
        // Keeled blade: a shallow V across, the midrib a crease, veins as fine ridges.
        let h = (-0.32 * x.abs() + 0.006 * vein - 0.012 * mid) * cw;
        Texel::new(leafy(lum), 1.0, h)
    });
}

fn paint_seed_head(cv: &mut Canvas, r: [i32; 4], rng: &mut engine_core::math::Rng) {
    let (cw, ch) = (r[2] as f32, r[3] as f32);
    let p = |s: f32, t: f32| Vec2::new(r[0] as f32 + s * cw, r[1] as f32 + (1.0 - t) * ch);
    let straw = |l: f32| [l * 1.05, l, l * 0.8];
    // The culm with a slight curve, then whorls of branches holding spikelets.
    let bend = 0.04;
    cv.curve(
        p(0.5, 0.0),
        p(0.5 + bend, 0.5),
        p(0.5 + bend * 0.5, 0.99),
        3.2,
        1.2,
        &|_| straw(0.7),
        1.5,
        r,
    );
    let whorls = 8;
    for k in 0..whorls {
        let t = 0.12 + 0.84 * k as f32 / whorls as f32;
        let axis = p(0.5 + bend * (1.0 - (t - 0.5).abs()), t);
        let reach = 0.46 * (1.0 - t).powf(0.6) + 0.06;
        for side in [-1.0f32, 1.0] {
            if rng.chance(0.15) {
                continue;
            }
            let ang = rng.range(0.35, 0.7);
            let dir = Vec2::new(side * ang.sin(), -ang.cos());
            let len = reach * cw * rng.range(0.75, 1.1);
            let end = axis + dir * len;
            cv.stroke(axis, end, 1.6, 0.9, straw(0.66), 1.0, r);
            let spikelets = 3 + (len / 14.0) as i32;
            for j in 0..spikelets {
                let f = 0.3 + 0.7 * j as f32 / spikelets as f32;
                let at = axis + dir * len * f;
                let sd = (dir + Vec2::new(-dir.y, dir.x) * rng.range(-0.5, 0.5) * 0.6
                    - Vec2::Y * 0.4)
                    .normalize();
                let l = rng.range(7.0, 11.0);
                let lum = rng.range(0.72, 0.95);
                cv.stroke(at, at + sd * l, 4.2, 1.2, straw(lum), 2.5, r);
            }
        }
    }
    // A spikelet at the very top.
    let top = p(0.5 + bend * 0.5, 0.99);
    cv.stroke(top + Vec2::Y * 12.0, top, 4.0, 1.2, straw(0.9), 2.5, r);
}

fn paint_tuft(cv: &mut Canvas, r: [i32; 4], rng: &mut engine_core::math::Rng) {
    let (cw, ch) = (r[2] as f32, r[3] as f32);
    let p = |s: f32, t: f32| Vec2::new(r[0] as f32 + s * cw, r[1] as f32 + (1.0 - t) * ch);
    let blades = 34;
    let mut order: Vec<(f32, f32, f32, f32, f32, f32, bool)> = (0..blades)
        .map(|_| {
            let s0 = 0.5 + rng.range(-0.14, 0.14);
            let s1 = 0.5 + rng.range(-0.49, 0.49);
            let t1 = rng.range(0.4, 0.985);
            let w = rng.range(2.5, 5.0);
            let lum = rng.range(0.55, 1.0);
            let bend = rng.range(-0.15, 0.15);
            (s0, s1, t1, w, lum, bend, rng.chance(0.12))
        })
        .collect();
    // Short blades in front (painted last).
    order.sort_by(|a, b| b.2.total_cmp(&a.2));
    for (s0, s1, t1, w, lum, bend, dry) in order {
        let mid = p(s0 + (s1 - s0) * 0.35 + bend, t1 * 0.72);
        let color = move |t: f32| {
            let l = lum * (0.45 + 0.55 * smoothstep(0.0, 0.5, t * t1));
            if dry {
                [l * 1.25, l * 1.1, l * 0.62]
            } else {
                leafy(l)
            }
        };
        cv.curve(p(s0, 0.0), mid, p(s1, t1), w, 0.6, &color, w * 0.5, r);
    }
}

/// Short grass seen from above: a dense mat of foreshortened blades leaning out from the
/// middle over dark thatch, its rim broken into blade tips. `dry` (0..1) mixes in straw.
fn paint_sward(
    cv: &mut Canvas,
    r: [i32; 4],
    rng: &mut engine_core::math::Rng,
    dry: f32,
    seed: u64,
) {
    let (cw, ch) = (r[2] as f32, r[3] as f32);
    let rad = 0.5 * cw.min(ch);
    let center = Vec2::new(r[0] as f32 + 0.5 * cw, r[1] as f32 + 0.5 * ch);
    // The mat's ragged outline (fraction of the cell's half width).
    let edge = |a: f32| {
        0.78 + 0.14 * wrap_noise(a / TAU, 0.5, 5.0, 1.0, seed)
            + 0.07 * wrap_noise(a / TAU, 0.5, 17.0, 1.0, seed + 1)
    };
    // Thatch under the blades: matted dead leaves, dark and fibrous, solid inside.
    cv.fill(r, |s, t| {
        let p = Vec2::new(s - 0.5, t - 0.5) * 2.0;
        let (d, a) = (p.length(), p.y.atan2(p.x));
        let cov = aa((edge(a) * 0.84 - d) * rad);
        let n = fbm(s * 11.0, t * 11.0, seed);
        let fiber = (0.5 + 0.5 * ((p.x * 0.8 + p.y * 0.6) * 90.0 + 6.0 * n).sin()).powi(3);
        let l = 0.3 + 0.08 * n + 0.06 * fiber;
        let straw = [l * 1.3, l * 1.1, l * 0.62];
        let green = leafy(l * 0.9);
        let k = (0.35 + 0.5 * dry + 0.3 * n).clamp(0.0, 1.0);
        let c = [
            green[0] + (straw[0] - green[0]) * k,
            green[1] + (straw[1] - green[1]) * k,
            green[2] + (straw[2] - green[2]) * k,
        ];
        Texel::new(c, cov, (0.8 * n + 0.6 * fiber) * (1.0 - d).max(0.0))
    });
    // Blades: from above, the upright ones in the middle are short; toward the rim they lean
    // out and show more of their length.
    let blades = (cw * ch / 34.0) as usize;
    for _ in 0..blades {
        let u = rng.f32().sqrt();
        let a = rng.range(0.0, TAU);
        let rim = edge(a);
        let d0 = u * rim * 0.93;
        let lean = a + rng.range(-1.0, 1.0) * (1.2 - 0.6 * u);
        let len = (rad * rng.range(0.035, 0.09) * (0.5 + 1.1 * u)).min(rad * (0.99 - d0) / 1.1);
        let dir = Vec2::new(lean.cos(), lean.sin());
        let start = center + Vec2::new(a.cos(), a.sin()) * d0 * rad;
        let end = start + dir * len;
        let mid = (start + end) * 0.5 + dir.perp() * len * rng.range(-0.3, 0.3);
        let w = rng.range(1.3, 2.7) * (cw / 256.0).max(0.5);
        let lum = rng.range(0.6, 1.0);
        let straw = rng.chance(0.07 + 0.38 * dry);
        let color = move |t: f32| {
            let l = lum * (0.5 + 0.5 * t);
            if straw {
                [l * 1.22, l * 1.06, l * 0.6]
            } else {
                leafy(l)
            }
        };
        cv.curve(start, mid, end, w, w * 0.3, &color, w * 0.6, r);
    }
}

fn paint_petal_narrow(cv: &mut Canvas, r: [i32; 4]) {
    let (cw, ch) = (r[2] as f32, r[3] as f32);
    cv.fill(r, |s, t| {
        let x = s - 0.5;
        let mut hw = 0.44 * (0.38 + 0.62 * smoothstep(0.0, 0.32, t));
        if t > 0.78 {
            hw *= (1.0 - ((t - 0.78) / 0.2).powi(2)).max(0.0).sqrt();
        }
        // Two or three small teeth at the tip.
        let xn = (x / 0.44).clamp(-1.0, 1.0);
        let tip = 0.985 - 0.035 * (0.5 + 0.5 * (xn * 3.0 * std::f32::consts::PI).cos());
        let cov = aa((hw - x.abs()) * cw).min(aa((tip - t) * ch));
        let across = (x / hw.max(1e-3)).clamp(-1.0, 1.0);
        let veins = (0.5 + 0.5 * (across * 2.5 * TAU).cos()).powf(4.0);
        let lum = 0.94 * (1.0 - 0.08 * veins) * (0.82 + 0.18 * smoothstep(0.0, 0.3, t));
        let h = (-0.05 * (1.0 - across.abs()) - 0.01 * veins) * cw;
        Texel::new([lum, lum, lum * 0.97], cov, h)
    });
}

fn paint_petal_broad(cv: &mut Canvas, r: [i32; 4]) {
    let (cw, ch) = (r[2] as f32, r[3] as f32);
    cv.fill(r, |s, t| {
        let x = s - 0.5;
        // Obovate: a narrow claw at the base, widest near the top, a wavy rim.
        let mut hw = 0.48 * (0.16 + 0.84 * smoothstep(0.0, 0.72, t).sqrt());
        if t > 0.68 {
            hw *= (1.0 - ((t - 0.68) / 0.31).powi(2)).max(0.0).sqrt();
        }
        hw *= 1.0 + 0.03 * (t * 40.0).sin();
        let rim = 0.99 - 0.02 * (s * 5.0 * TAU).sin().abs();
        let cov = aa((hw - x.abs()) * cw).min(aa((rim - t) * ch));
        let ang = x.atan2(t + 0.08);
        let veins = (0.5 + 0.5 * (ang * 26.0).cos()).powf(6.0) * smoothstep(0.05, 0.5, t);
        let crinkle = fbm(s * 7.0, t * 7.0, 23);
        let lum =
            0.93 * (1.0 - 0.13 * veins) * (0.78 + 0.22 * smoothstep(0.0, 0.3, t)) + 0.03 * crinkle;
        let h = (-0.012 * veins + 0.02 * crinkle) * cw;
        Texel::new([lum, lum, lum * 0.97], cov, h)
    });
}

fn paint_leaf(cv: &mut Canvas, r: [i32; 4]) {
    let (cw, ch) = (r[2] as f32, r[3] as f32);
    cv.fill(r, |s, t| {
        let x = s - 0.5;
        let u = ((t - 0.08) / 0.9).clamp(0.0, 1.0);
        let mut hw = if t < 0.08 {
            0.03
        } else {
            (0.46 * (std::f32::consts::PI * u.powf(0.75)).sin()).max(0.03)
        };
        // Saw-toothed margin.
        hw *= 1.0 - 0.07 * (u * 18.0).fract() * smoothstep(0.05, 0.2, u);
        let tip = 0.985;
        let cov = aa((hw - x.abs()) * cw).min(aa((tip - t) * ch));
        let across = (x / hw.max(1e-3)).clamp(-1.0, 1.0);
        let midrib = 1.0 - smoothstep(0.0, 0.025, x.abs());
        let side = (0.5 + 0.5 * ((u - across.abs() * 0.35) * 14.0 * TAU).cos()).powf(10.0);
        let mottle = fbm(s * 6.0, t * 9.0, 31);
        let lum = 0.72
            * (1.0 + 0.3 * midrib)
            * (1.0 - 0.12 * side)
            * (1.0 + 0.08 * mottle)
            * (0.88 + 0.12 * (1.0 - across.abs()));
        // Blade halves bulge between a sunken midrib and veins.
        let h = (0.05 * (1.0 - across.abs()).sqrt() - 0.03 * midrib - 0.008 * side) * cw;
        Texel::new(leafy(lum), cov, h)
    });
}

fn paint_florets(cv: &mut Canvas, r: [i32; 4]) {
    let cell = 9.0f32;
    cv.fill(r, |s, t| {
        let x = s * r[2] as f32;
        let y = t * r[3] as f32;
        let row = (y / (cell * 0.87)).floor();
        let mut best = f32::MAX;
        for dr in -1..=1 {
            let rr = row + dr as f32;
            let off = if (rr as i32).rem_euclid(2) == 1 {
                cell * 0.5
            } else {
                0.0
            };
            let col = ((x - off) / cell).floor();
            for dc in -1..=1 {
                let cc = col + dc as f32;
                let jx = hash01(cc as i64, rr as i64, 0, 5) - 0.5;
                let jy = hash01(cc as i64, rr as i64, 1, 5) - 0.5;
                let cx = cc * cell + off + cell * 0.5 + jx * 1.5;
                let cy = rr * cell * 0.87 + cell * 0.5 + jy * 1.5;
                best = best.min(Vec2::new(x - cx, y - cy).length());
            }
        }
        let d = (best / (cell * 0.55)).min(1.0);
        let dome = (1.0 - d * d).max(0.0);
        let lum = 0.45 + 0.55 * dome.sqrt() - 0.25 * (1.0 - smoothstep(0.0, 0.18, d));
        Texel::new([lum, lum * 0.97, lum * 0.9], 1.0, dome * 3.0)
    });
}

fn paint_bell(cv: &mut Canvas, r: [i32; 4]) {
    let ch = r[3] as f32;
    cv.fill(r, |s, t| {
        let k = s * 6.0;
        let x = k.fract() - 0.5;
        // Scalloped mouth: each tepal ends in a rounded, slightly pointed tip.
        let tip = 0.78 + 0.21 * (1.0 - (2.0 * x).powi(2)).max(0.0).powf(0.6);
        let cov = aa((tip - t) * ch);
        let midvein = 1.0 - smoothstep(0.0, 0.06, x.abs());
        let veins = (0.5 + 0.5 * (x * 5.0 * TAU).cos()).powf(4.0);
        let lum = 0.9 * (1.0 - 0.16 * midvein) * (1.0 - 0.05 * veins);
        let h = (0.06 * (1.0 - (2.0 * x).powi(2)).max(0.0) - 0.02 * midvein) * r[2] as f32;
        Texel::new([lum, lum, lum], cov, h)
    });
}

// ---------------------------------------------------------------- fern

/// One fern pinna (see [`pinna_uv`]) and a strip of stem (see [`fern_stem_uv`]): `size` wide
/// and `2 * size` tall, the pinna's base at the bottom and its tip at the top.
pub fn fern_pinna(size: u32) -> TextureSet {
    let (w, h) = (size.max(32), size.max(32) * 2);
    let mut cv = Canvas::new(w, h);
    let pw = (w as f32 * PINNA_WIDTH) as i32;
    let (wf, hf) = (pw as f32, h as f32);
    let n = 13;
    // Pinnules: (base point, direction, length, half width, tint) in pixels (y up).
    let mut lobes = Vec::new();
    for side in [-1.0f32, 1.0] {
        for i in 0..n {
            let t = 0.05 + 0.88 * (i as f32 + if side > 0.0 { 0.0 } else { 0.5 }) / n as f32;
            let costa = Vec2::new(wf * (0.5 + 0.015 * (t * 5.0).sin()), t * hf);
            let len =
                wf * 0.47 * (1.0 - t.powf(1.7)).powf(0.75) * (0.8 + 0.2 * (t / 0.12).min(1.0));
            let ang = (64.0f32 - 10.0 * t).to_radians();
            let dir = Vec2::new(side * ang.sin(), ang.cos());
            let half = hf * 0.88 / n as f32 * 0.36 * (1.0 - 0.35 * t);
            let tint = 0.92 + 0.14 * hash01(i as i64, side as i64, 0, 77);
            lobes.push((costa, dir, len.max(4.0), half.max(2.0), tint));
        }
    }
    cv.fill([0, 0, pw, h as i32], |s, t| {
        let p = Vec2::new(s * wf, t * hf);
        let costa_x = wf * (0.5 + 0.015 * (t * 5.0).sin());
        let dx = p.x - costa_x;
        // The winged costa and the pointed tip of the pinna.
        let wing_half = (5.0 * (1.0 - t) + 1.5) * if t < 0.97 { 1.0 } else { 0.0 };
        let mut cov = aa(wing_half - dx.abs());
        let tip_t = (t - 0.88) / 0.11;
        if (0.0..1.0).contains(&tip_t) {
            let half = 9.0 * (1.0 - tip_t).powf(0.7);
            cov = cov.max(aa(half - dx.abs()));
        }
        // The lobe covering this texel most: (coverage, across -1..1, tint, along 0..1).
        let mut lobe: Option<(f32, f32, f32, f32)> = None;
        for &(base, dir, len, half, tint) in &lobes {
            let q = p - base;
            let along = q.dot(dir);
            if along < -half || along > len + 2.0 {
                continue;
            }
            let u = (along / len).clamp(0.0, 1.0);
            let across = q.perp_dot(dir);
            // Oblong lobe with a rounded tip and a crenate edge, wider where it joins the costa.
            let mut hw = half * (1.0 + 0.35 * (1.0 - smoothstep(0.0, 0.2, u)));
            if u > 0.62 {
                hw *= (1.0 - ((u - 0.62) / 0.38).powi(2)).max(0.0).sqrt();
            }
            hw *= 1.0 - 0.1 * (0.5 + 0.5 * (u * 7.0 * TAU).cos());
            let c = aa(hw - across.abs()).min(aa(len - along));
            if c > 0.0 && lobe.is_none_or(|l| c > l.0) {
                lobe = Some((c, (across / hw.max(0.5)).clamp(-1.0, 1.0), tint, u));
            }
        }
        let (across, tint, u) = lobe.map_or((0.0, 1.0, 0.0), |l| (l.1, l.2, l.3));
        let cov = cov.max(lobe.map_or(0.0, |l| l.0));
        let costa = 1.0 - smoothstep(1.0, 3.0, dx.abs());
        let midvein = 1.0 - smoothstep(0.0, 0.14, across.abs());
        let veinlets = (0.5 + 0.5 * ((u * 5.0 - across.abs() * 0.8) * TAU).cos()).powf(8.0);
        let lum = 0.74
            * tint
            * (1.0 + 0.16 * midvein)
            * (1.0 - 0.08 * veinlets)
            * (0.84 + 0.16 * (1.0 - across.abs()));
        let color = if costa > 0.0 {
            let c = leafy(lum);
            let k = costa * 0.7;
            [
                c[0] * (1.0 - k) + 0.86 * k,
                c[1] * (1.0 - k) + 0.9 * k,
                c[2] * (1.0 - k) + 0.52 * k,
            ]
        } else {
            leafy(lum)
        };
        // Each pinnule bulges; the midvein and costa are creased and ridged.
        let h = 5.0 * (1.0 - across * across).max(0.0).sqrt() - 1.2 * midvein + 2.5 * costa;
        Texel::new(color, cov, h)
    });
    // The stem strip: smooth, a little fibrous, rounded across.
    let x0 = (w as f32 * 0.88) as i32;
    cv.fill([x0, 0, w as i32 - x0, h as i32], |s, t| {
        let streak = fbm(s * 4.0, t * 30.0, 71);
        let lum = 0.8 * (1.0 + 0.08 * streak) * (0.85 + 0.15 * (s * std::f32::consts::PI).sin());
        Texel::new([lum * 0.98, lum, lum * 0.78], 1.0, 2.0 * streak)
    });
    TextureSet {
        normal: encode_normal(&cv, [0.8, 1.6], false),
        albedo: encode_albedo(&cv, true),
        emissive: None,
    }
}

// ---------------------------------------------------------------- fungi

/// Periodic Voronoi in u (`nu` cells around, `nv` down): distance to the nearest feature, the
/// gap to the second nearest (small on cell edges), in cell units, and a random 0..1 value of
/// the nearest feature.
fn wrap_voronoi(u: f32, v: f32, nu: i32, nv: f32, seed: u64) -> (f32, f32, f32) {
    let x = u * nu as f32;
    let y = v * nv;
    let (cx, cy) = (x.floor() as i32, y.floor() as i32);
    let (mut d1, mut d2, mut id) = (f32::MAX, f32::MAX, 0.0);
    for dy in -1..=1 {
        for dx in -1..=1 {
            let ix = cx + dx;
            let iy = cy + dy;
            let wx = ix.rem_euclid(nu) as i64;
            let fx = ix as f32 + 0.15 + 0.7 * hash01(wx, iy as i64, 0, seed);
            let fy = iy as f32 + 0.15 + 0.7 * hash01(wx, iy as i64, 1, seed);
            let d = Vec2::new(x - fx, y - fy).length();
            if d < d1 {
                d2 = d1;
                d1 = d;
                id = hash01(wx, iy as i64, 2, seed);
            } else if d < d2 {
                d2 = d;
            }
        }
    }
    (d1, d2 - d1, id)
}

/// Albedo, normal and emission maps for a fungus, `size` around and `2 * size` down: cap top
/// (v 0..0.47), gills (0.5..0.75) and stem (0.78..1).
pub fn fungus(kind: Fungus, size: u32) -> TextureSet {
    let (w, h) = (size.max(32), size.max(32) * 2);
    let mut cv = Canvas::new(w, h);
    let mut glow = vec![0.0f32; (w * h) as usize];
    let lantern = kind == Fungus::Lantern;
    for y in 0..h {
        for x in 0..w {
            let u = (x as f32 + 0.5) / w as f32;
            let v = (y as f32 + 0.5) / h as f32;
            let (texel, e) = if v < 0.485 {
                let r = ((v - 0.005) / 0.46).clamp(0.0, 1.0);
                if lantern {
                    lantern_top(u, r)
                } else {
                    glowcap_top(u, r)
                }
            } else if v < 0.765 {
                gills(u, ((v - 0.5) / 0.25).clamp(0.0, 1.0), lantern)
            } else {
                stem(u, ((v - 0.78) / 0.215).clamp(0.0, 1.0), lantern)
            };
            cv.px[(y * w + x) as usize] = texel;
            glow[(y * w + x) as usize] = e;
        }
    }
    TextureSet {
        normal: encode_normal(&cv, [1.0, 1.0], true),
        albedo: encode_albedo(&cv, false),
        emissive: Some(encode_gray(&glow, w, h)),
    }
}

/// Glowcap cap top: darker pigmented center, pale scales, striate translucent margin.
fn glowcap_top(u: f32, r: f32) -> (Texel, f32) {
    // Radial striations (gill shadows through the thin margin).
    let wobble = 0.15 * wrap_noise(u, r, 6.0, 3.0, 41);
    let stri = (0.5 + 0.5 * ((u * 44.0 + wobble) * TAU).cos()).powf(5.0);
    let stri_w = smoothstep(0.35, 0.95, r);
    // Pale scales scattered over the upper cap.
    let (d, _, id) = wrap_voronoi(u, r, 16, 7.0, 43);
    let size = 0.6 + 0.8 * id;
    let spot = if id < 0.6 {
        (1.0 - smoothstep(0.1 * size, 0.24 * size, d * (0.6 + 0.8 * r)))
            * (1.0 - smoothstep(0.6, 0.8, r))
    } else {
        0.0
    };
    let mottle = wrap_noise(u, r, 9.0, 6.0, 45);
    let lum = (0.55 + 0.4 * smoothstep(0.0, 1.0, r)) * (1.0 - 0.22 * stri * stri_w)
        + 0.32 * spot
        + 0.05 * mottle;
    let rim = smoothstep(0.82, 1.0, r);
    // Emission spans two orders of magnitude: a faint top, glowing scales and striations, a
    // bright rim (the gills shine through the thin margin).
    let e = 0.008
        + 0.025 * smoothstep(0.3, 0.9, r)
        + 0.12 * spot
        + 0.07 * stri * stri_w
        + 0.45 * rim * rim;
    let h = 2.5 * spot - 1.2 * stri * stri_w + 0.6 * mottle;
    (Texel::new([lum, lum, lum], 1.0, h), e.min(1.0))
}

/// Lantern cap top: a translucent membrane over ribs, with a fine net of veins, glowing more
/// toward the margin where it is thinnest.
fn lantern_top(u: f32, r: f32) -> (Texel, f32) {
    let ribs = 16.0;
    let rib = (0.5 + 0.5 * ((u * ribs + 0.08 * wrap_noise(u, r, 5.0, 2.0, 51)) * TAU).cos())
        .powf(30.0)
        * smoothstep(0.08, 0.3, r);
    let (_, edge, _) = wrap_voronoi(u, r, 22, 7.0, 52);
    let vein = (1.0 - smoothstep(0.02, 0.12, edge)) * smoothstep(0.1, 0.4, r);
    let mottle = wrap_noise(u, r, 12.0, 8.0, 53);
    let apex = 1.0 - smoothstep(0.0, 0.22, r);
    let lum = 0.82 * (1.0 - 0.45 * rib) * (1.0 - 0.25 * vein) * (1.0 - 0.35 * apex) + 0.04 * mottle;
    // The membrane is lit from inside: faint at the thick apex, brighter toward the thin
    // margin, crossed by dark ribs and veins.
    let membrane = 0.012 + 0.11 * smoothstep(0.15, 0.95, r).powi(2);
    let e = membrane * (1.0 - 0.85 * rib) * (1.0 - 0.55 * vein) * (1.0 + 0.2 * mottle)
        + 0.45 * smoothstep(0.88, 1.0, r).powi(2);
    let h = 3.0 * rib + 0.8 * vein + 0.5 * mottle;
    (Texel::new([lum, lum, lum], 1.0, h), e.clamp(0.0, 1.0))
}

/// Gills: thin radial plates with shorter lamellulae between them toward the margin.
fn gills(u: f32, g: f32, lantern: bool) -> (Texel, f32) {
    let n = if lantern { 56.0 } else { 40.0 };
    let plate = |phase: f32| (0.5 + 0.5 * ((u * n + phase) * TAU).cos()).powf(10.0);
    let main = plate(0.0);
    let short = plate(0.5) * (1.0 - smoothstep(0.35, 0.55, g));
    let ridge = main.max(short * 0.85);
    let lum = 0.5 + 0.48 * ridge;
    let e = (0.22 + 0.78 * ridge) * (1.0 - 0.6 * smoothstep(0.6, 1.0, g));
    let h = 4.0 * ridge;
    (Texel::new([lum, lum, lum * 0.98], 1.0, h), e)
}

/// Stem: fibrous lengthwise streaks; lantern stems carry zig-zag bands of darker scales.
fn stem(u: f32, q: f32, lantern: bool) -> (Texel, f32) {
    let fibers = wrap_noise(u, q, 28.0, 2.0, 61) * 0.6 + wrap_noise(u, q, 60.0, 4.0, 62) * 0.4;
    let mut lum = 0.86 + 0.1 * fibers;
    let mut h = 1.5 * fibers;
    let mut e = 0.035 * (1.0 - smoothstep(0.0, 0.85, q)) + 0.003;
    if lantern {
        let zig = ((u * 22.0).fract() - 0.5).abs() * 0.08;
        let band = (0.5 + 0.5 * ((q * 9.0 + zig) * TAU).cos()).powf(6.0) * smoothstep(0.2, 0.5, q);
        lum *= 1.0 - 0.3 * band;
        h += 2.0 * band;
        e *= 1.0 - 0.6 * band;
    }
    e *= 1.0 + 0.3 * fibers;
    (Texel::new([lum, lum, lum], 1.0, h), e.clamp(0.0, 1.0))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn alpha_coverage(img: &Image, level: Option<usize>, rect: [f32; 4]) -> f32 {
        let (w, h, data) = match level {
            None => (img.width, img.height, &img.data),
            Some(l) => {
                let (w, h) = img.level_size(l as u32 + 1);
                (w, h, &img.mips[l])
            }
        };
        let (x0, y0) = ((rect[0] * w as f32) as u32, (rect[1] * h as f32) as u32);
        let (x1, y1) = (
            ((rect[0] + rect[2]) * w as f32) as u32,
            ((rect[1] + rect[3]) * h as f32) as u32,
        );
        let mut n = 0;
        let mut on = 0;
        for y in y0..y1 {
            for x in x0..x1 {
                n += 1;
                if data[((y * w + x) * 4 + 3) as usize] as f32 / 255.0 >= SMALL_ALPHA_CUTOFF {
                    on += 1;
                }
            }
        }
        on as f32 / n.max(1) as f32
    }

    #[test]
    fn meadow_atlas_cells_have_their_shapes() {
        let set = meadow_atlas(256);
        assert!(set.albedo.is_valid() && set.normal.is_valid());
        for cell in Cell::ALL {
            let c = alpha_coverage(&set.albedo, None, cell.rect());
            match cell {
                Cell::Blade | Cell::Florets => assert!(c > 0.99, "{cell:?} {c}"),
                Cell::Bell => assert!(c > 0.8 && c < 0.99, "{cell:?} {c}"),
                _ => assert!(c > 0.05 && c < 0.8, "{cell:?} {c}"),
            }
        }
        // Mips keep the cut-out coverage.
        let c0 = alpha_coverage(&set.albedo, None, [0.0, 0.0, 1.0, 1.0]);
        let c3 = alpha_coverage(&set.albedo, Some(2), [0.0, 0.0, 1.0, 1.0]);
        assert!((c3 - c0).abs() < 0.1, "mip coverage {c3} vs {c0}");
        // Cell UVs stay inside their cells.
        for cell in Cell::ALL {
            let r = cell.rect();
            for (s, t) in [(0.0, 0.0), (1.0, 1.0), (0.5, 0.5)] {
                let uv = cell.uv(s, t);
                assert!(uv.x > r[0] && uv.x < r[0] + r[2] && uv.y > r[1] && uv.y < r[1] + r[3]);
            }
        }
    }

    #[test]
    fn fern_pinna_is_lobed() {
        let set = fern_pinna(128);
        assert!(set.albedo.is_valid() && set.normal.is_valid());
        let c = alpha_coverage(&set.albedo, None, [0.0, 0.0, 1.0, 1.0]);
        assert!(c > 0.3 && c < 0.85, "coverage {c}");
        // The costa runs up the middle; the lobes leave gaps near the edges; the stem strip is
        // solid.
        let costa = pinna_uv(0.5, 0.5).x;
        assert!(alpha_coverage(&set.albedo, None, [costa - 0.01, 0.1, 0.02, 0.7]) > 0.95);
        let edge = alpha_coverage(&set.albedo, None, [costa + 0.12, 0.45, 0.12, 0.4]);
        assert!(edge > 0.05 && edge < 0.8, "edge {edge}");
        let strip = fern_stem_uv(0.0, 0.0).x;
        assert!(alpha_coverage(&set.albedo, None, [strip, 0.05, 0.07, 0.9]) > 0.99);
    }

    #[test]
    fn fungus_gills_and_rims_glow_more_than_cap_tops() {
        for kind in [Fungus::Glowcap, Fungus::Lantern] {
            let set = fungus(kind, 64);
            let e = set.emissive.expect("emission");
            assert!(e.is_valid() && set.albedo.is_valid() && set.normal.is_valid());
            let mean = |v0: f32, v1: f32| {
                let (y0, y1) = ((v0 * e.height as f32) as u32, (v1 * e.height as f32) as u32);
                let mut sum = 0.0;
                for y in y0..y1 {
                    for x in 0..e.width {
                        let b = e.data[((y * e.width + x) * 4) as usize] as f32 / 255.0;
                        sum += if b <= 0.04045 {
                            b / 12.92
                        } else {
                            ((b + 0.055) / 1.055).powf(2.4)
                        };
                    }
                }
                sum / ((y1 - y0) * e.width) as f32
            };
            let apex = mean(0.0, 0.1);
            let rim = mean(0.43, 0.47);
            let gills = mean(0.52, 0.65);
            assert!(rim > apex * 5.0, "{kind:?}: rim {rim} apex {apex}");
            assert!(gills > apex * 10.0, "{kind:?}: gills {gills} apex {apex}");
        }
    }
}
