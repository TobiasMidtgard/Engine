//! Procedural leaf textures: per-species atlases of leaf and needle sprays (lobed oak leaves,
//! small serrated birch leaves, rounded toothed hazel leaves, long pine needles in brushes,
//! short dense spruce needles), plus fern fronds, painted on the CPU into RGBA images with
//! alpha cut-outs and matching tangent-space normal maps.
//!
//! Trees and shrubs use an atlas of [`ATLAS_CELLS`]² cells: different twig sprays, plus one
//! compact cluster seen from the shoot tip ([`CLUSTER_CELL`]: a rosette of leaves or a tuft
//! of needles, centered in its cell). In a spray the twig enters at the bottom center of the
//! cell and the leaves grow toward its top, so cards can attach their bottom edge to a branch.
//!
//! Every leaf is painted with its own outline (lobes, teeth), midrib, side veins, color,
//! upper or paler lower face, a soft shadow on the leaves below it, and in the normal map its
//! own tilt, a fold along the midrib and sunken veins, so a spray catches the light leaf by
//! leaf instead of reading as a flat sticker. Colors stay near-neutral (light yellow-green
//! luminance); the species' leaf color multiplies them, so one texture serves every tint and
//! autumn can recolor leaves by changing a material color.
//!
//! Mip levels keep the alpha coverage of the full-size image, so distant crowns stay full
//! instead of thinning out into see-through ghosts (the classic alpha-test mip problem).
//! Needle atlases paint their larger mip levels again at that size instead of averaging the
//! level above: averaged, hair-thin needles melt into solid leaf-shaped blobs, so each level
//! keeps a share of its needles as sharp strokes a pixel wide (see [`Canvas::needle_width`]).

use engine_core::math::{Rng, Vec2, Vec3, noise::perlin2, smoothstep};
use engine_render::Image;

const PI: f32 = std::f32::consts::PI;
const TAU: f32 = std::f32::consts::TAU;

/// Which kind of leaves to paint.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum LeafKind {
    /// Rounded, double-toothed leaves alternating along arching shoots (hazel; the default
    /// broadleaf).
    Broad,
    /// Small triangular, double-toothed leaves on thin hanging twigs (birch).
    Small,
    /// Long needles in brush-like tufts along a twig (pine).
    LongNeedles,
    /// Short dense needles on flat, comb-like sprays (spruce).
    ShortNeedles,
    /// A fern frond: a stem with leaflets that shrink toward the tip (one image, no atlas).
    Frond,
    /// Deeply lobed leaves crowded at the shoot tips (oak).
    Lobed,
}

/// The texture kind a species' leaf style is painted as.
pub fn kind_of(style: crate::species::LeafStyle) -> LeafKind {
    use crate::species::LeafStyle;
    match style {
        LeafStyle::Broad => LeafKind::Broad,
        LeafStyle::Small => LeafKind::Small,
        LeafStyle::LongNeedles => LeafKind::LongNeedles,
        LeafStyle::ShortNeedles => LeafKind::ShortNeedles,
        LeafStyle::Frond => LeafKind::Frond,
        LeafStyle::Lobed => LeafKind::Lobed,
    }
}

impl LeafKind {
    /// True for the atlases of trees and shrubs ([`ATLAS_CELLS`]² sprays); fronds fill the
    /// whole image.
    pub fn is_atlas(self) -> bool {
        self != LeafKind::Frond
    }

    /// Pine and spruce needles.
    pub fn is_needles(self) -> bool {
        matches!(self, LeafKind::LongNeedles | LeafKind::ShortNeedles)
    }
}

/// The smallest atlas cell (pixels) whose mip level is painted rather than averaged.
const MIN_PAINTED_CELL: u32 = 8;

/// Alpha cutoff used by leaf materials.
pub const ALPHA_CUTOFF: f32 = 0.5;

/// Cells per side of a leaf atlas.
pub const ATLAS_CELLS: u32 = 2;

/// The atlas cell holding a compact cluster seen from the shoot tip (a rosette of leaves, a
/// tuft of needles), centered in its cell. The other cells hold twig sprays.
pub const CLUSTER_CELL: usize = 2;

/// UV rectangle (min, max) of atlas cell `cell` of a texture of kind `kind` (the whole image
/// for fronds), inset by half a texel of a 1024 atlas so filtering doesn't bleed between cells.
pub fn atlas_cell(kind: LeafKind, cell: usize) -> (Vec2, Vec2) {
    if !kind.is_atlas() {
        return (Vec2::ZERO, Vec2::ONE);
    }
    let n = ATLAS_CELLS as usize;
    let c = cell % (n * n);
    let size = 1.0 / n as f32;
    let min = Vec2::new((c % n) as f32 * size, (c / n) as f32 * size);
    let inset = Vec2::splat(0.5 / 1024.0);
    (min + inset, min + Vec2::splat(size) - inset)
}

/// A painted leaf texture: sRGB color with alpha cut-out, and a linear tangent-space normal
/// map (+Y up, OpenGL convention) of the same size.
pub struct LeafTextures {
    pub color: Image,
    pub normal: Image,
}

/// RGBA float canvas with a normal layer. Painting functions take coordinates in canvas
/// units (0..1, y down).
struct Canvas {
    size: u32,
    px: Vec<[f32; 4]>,
    /// Tangent-space normal (x right, y up), blended by coverage.
    nrm: Vec<[f32; 3]>,
    /// Needles thinner than this many pixels are widened to it, and only a matching share of
    /// them painted (0: paint every needle as it is). See [`Canvas::needle_width`].
    min_needle_px: f32,
    /// Picks which thin needles to keep, apart from the painter's own random numbers so the
    /// layout stays the same at every size.
    thin: Rng,
}

impl Canvas {
    fn new(size: u32) -> Self {
        Self {
            size,
            px: vec![[0.0; 4]; (size * size) as usize],
            nrm: vec![[0.0, 0.0, 1.0]; (size * size) as usize],
            min_needle_px: 0.0,
            thin: Rng::new(0x7e1e ^ size as u64),
        }
    }

    /// How much wider to paint a needle `w` wide and `l` long (canvas units), or `None` to
    /// leave it out. On a small canvas (a painted mip level) a needle under `min_needle_px`
    /// pixels wide is widened to that, and kept with the chance that leaves the needles' total
    /// coverage as it was: the brush keeps reading as separate sharp needles instead of a
    /// blurred blob. Needles too short to show as a stroke (spruce's, far off) stay as they
    /// are and blend into a fuzzy shoot.
    fn needle_width(&mut self, w: f32, l: f32) -> Option<f32> {
        let px = w * self.size as f32;
        if px >= self.min_needle_px || l * self.size as f32 <= 3.0 {
            return Some(1.0);
        }
        self.thin
            .chance(px / self.min_needle_px)
            .then_some(self.min_needle_px / px)
    }

    fn index(&self, x: i32, y: i32) -> Option<usize> {
        let s = self.size as i32;
        (x >= 0 && y >= 0 && x < s && y < s).then(|| (y * s + x) as usize)
    }

    /// Paint over with `color` and `normal` at coverage `a`.
    fn put(&mut self, x: i32, y: i32, color: [f32; 3], normal: Vec3, a: f32) {
        let Some(i) = self.index(x, y) else {
            return;
        };
        if a <= 0.0 {
            return;
        }
        let a = a.min(1.0);
        let p = &mut self.px[i];
        for c in 0..3 {
            p[c] = p[c] * (1.0 - a) + color[c] * a;
        }
        p[3] = p[3] + a * (1.0 - p[3]);
        let n = &mut self.nrm[i];
        n[0] = n[0] * (1.0 - a) + normal.x * a;
        n[1] = n[1] * (1.0 - a) + normal.y * a;
        n[2] = n[2] * (1.0 - a) + normal.z * a;
    }

    /// Paint over with `color` (flat normal) at coverage `a`.
    fn blend(&mut self, x: i32, y: i32, color: [f32; 3], a: f32) {
        self.put(x, y, color, Vec3::Z, a);
    }

    /// Darken what is already painted (a soft shadow cast by something on top).
    fn darken(&mut self, x: i32, y: i32, f: f32) {
        if let Some(i) = self.index(x, y) {
            let p = &mut self.px[i];
            for c in &mut p[..3] {
                *c *= f;
            }
        }
    }

    /// A thick anti-aliased line in a flat color (fern stems).
    fn line(&mut self, a: Vec2, b: Vec2, width0: f32, width1: f32, color: [f32; 3]) {
        let s = self.size as f32;
        let (a, b) = (a * s, b * s);
        let w = width0.max(width1) * s;
        let min = a.min(b) - Vec2::splat(w + 1.0);
        let max = a.max(b) + Vec2::splat(w + 1.0);
        let ab = b - a;
        let len2 = ab.length_squared().max(1e-6);
        for y in min.y as i32..=max.y as i32 {
            for x in min.x as i32..=max.x as i32 {
                let p = Vec2::new(x as f32 + 0.5, y as f32 + 0.5);
                let t = ((p - a).dot(ab) / len2).clamp(0.0, 1.0);
                let d = p.distance(a + ab * t);
                let half = (width0 + (width1 - width0) * t) * s * 0.5;
                let cov = (half + 0.5 - d).clamp(0.0, 1.0);
                self.blend(x, y, color, cov);
            }
        }
    }

    /// A tapered, round stem from `a` to `b` (twigs, needles), colored from `c0` to `c1` and
    /// shaded as a cylinder in the normal map. Widths in canvas units.
    #[allow(clippy::too_many_arguments)]
    fn stem(&mut self, a: Vec2, b: Vec2, w0: f32, w1: f32, c0: [f32; 3], c1: [f32; 3]) {
        let s = self.size as f32;
        let (pa, pb) = (a * s, b * s);
        let r = w0.max(w1) * s * 0.5 + 1.5;
        let min = pa.min(pb) - Vec2::splat(r);
        let max = pa.max(pb) + Vec2::splat(r);
        let ab = pb - pa;
        let len2 = ab.length_squared().max(1e-6);
        let perp = Vec2::new(-ab.y, ab.x).normalize_or_zero();
        for y in min.y.floor() as i32..=max.y.ceil() as i32 {
            for x in min.x.floor() as i32..=max.x.ceil() as i32 {
                let p = Vec2::new(x as f32 + 0.5, y as f32 + 0.5);
                let t = ((p - pa).dot(ab) / len2).clamp(0.0, 1.0);
                let q = pa + ab * t;
                let d = p.distance(q);
                let half = (w0 + (w1 - w0) * t) * s * 0.5;
                // Lines thinner than a pixel cover only part of it.
                let cov = (half + 0.5 - d).clamp(0.0, 1.0).min(2.0 * half);
                if cov <= 0.0 {
                    continue;
                }
                let across = ((p - q).dot(perp) / half.max(0.5)).clamp(-1.0, 1.0);
                let round = (1.0 - across * across).max(0.0).sqrt();
                let nxy = perp * across * 0.8;
                let n = Vec3::new(nxy.x, -nxy.y, round.max(0.35)).normalize();
                let shade = 0.78 + 0.22 * round;
                let c = [0, 1, 2].map(|i| (c0[i] + (c1[i] - c0[i]) * t) * shade);
                self.put(x, y, c, n, cov);
            }
        }
    }

    /// A stem along a polyline with a width tapering from `w0` to `w1`.
    fn stem_path(&mut self, path: &[Vec2], w0: f32, w1: f32, color: [f32; 3]) {
        let n = path.len().max(2) - 1;
        for i in 0..path.len().saturating_sub(1) {
            let (f0, f1) = (i as f32 / n as f32, (i + 1) as f32 / n as f32);
            let c0 = color.map(|c| c * (0.9 + 0.1 * f0));
            let c1 = color.map(|c| c * (0.9 + 0.1 * f1));
            self.stem(
                path[i],
                path[i + 1],
                w0 + (w1 - w0) * f0,
                w0 + (w1 - w0) * f1,
                c0,
                c1,
            );
        }
    }

    /// A simple leaf from `base` along `dir` (unit), `length` and `width` in texture units
    /// (fern pinnae). `lobes` adds a wavy edge.
    #[allow(clippy::too_many_arguments)]
    fn leaf(
        &mut self,
        base: Vec2,
        dir: Vec2,
        length: f32,
        width: f32,
        lum: f32,
        lobes: f32,
        warm: f32,
    ) {
        let s = self.size as f32;
        let side = Vec2::new(-dir.y, dir.x);
        let tip = base + dir * length;
        let r = length.max(width) * s + 2.0;
        let c = (base + tip) * 0.5 * s;
        for y in (c.y - r) as i32..=(c.y + r) as i32 {
            for x in (c.x - r) as i32..=(c.x + r) as i32 {
                let p = Vec2::new(x as f32 + 0.5, y as f32 + 0.5) / s - base;
                let u = p.dot(dir) / length;
                let v = p.dot(side);
                if !(0.0..=1.0).contains(&u) {
                    continue;
                }
                // Ovate outline, pointed tip, optional lobes.
                let mut half = width * 0.5 * (std::f32::consts::PI * u.powf(0.75)).sin().max(0.0);
                if lobes > 0.0 {
                    half *= 1.0 - lobes * (0.5 + 0.5 * (u * 5.0 * std::f32::consts::TAU).cos()) * u;
                }
                let edge = (half - v.abs()) * s;
                let cov = (edge + 0.5).clamp(0.0, 1.0);
                if cov <= 0.0 {
                    continue;
                }
                // Shading: midrib, side veins, a fold that darkens one half, a lighter tip.
                let across = (v / half.max(1e-4)).clamp(-1.0, 1.0);
                let midrib = 1.0 - 0.25 * (1.0 - (v.abs() * s / 1.2).min(1.0));
                let veins = 1.0
                    - 0.08
                        * ((u * 9.0 + across.abs() * 2.5) * std::f32::consts::TAU)
                            .cos()
                            .max(0.0);
                let fold = 1.0 - 0.18 * across.max(0.0);
                let l = lum * midrib * veins * fold * (0.92 + 0.12 * u);
                let color = [l * (1.0 + 0.06 * warm), l, l * (0.82 - 0.08 * warm)];
                self.blend(x, y, color, cov);
            }
        }
    }

    /// Paint a detailed leaf blade (see [`Blade`]), with a soft shadow on what lies below.
    fn blade(&mut self, l: &Blade) {
        let s = self.size as f32;
        let base = l.base * s;
        let len = l.length * s;
        let wid = l.width * s;
        let tip = base + l.dir * len;
        let shadow = Vec2::new(0.7, 1.0) * (0.006 * s).max(0.6);
        let r = wid * 0.75 + shadow.length() + 4.0;
        let min = base.min(tip) - Vec2::splat(r);
        let max = base.max(tip) + Vec2::splat(r);
        for y in min.y.floor() as i32..=max.y.ceil() as i32 {
            for x in min.x.floor() as i32..=max.x.ceil() as i32 {
                let p = Vec2::new(x as f32 + 0.5, y as f32 + 0.5);
                // Soft shadow of this leaf on the leaves and twigs already painted.
                let es = l.edge(p - shadow, base, len, wid);
                if es > -3.0 {
                    let k = smoothstep(-3.0, 2.5 + 0.01 * s, es) * 0.38;
                    self.darken(x, y, 1.0 - k);
                }
                let e = l.edge(p, base, len, wid);
                let cov = (e + 0.5).clamp(0.0, 1.0);
                if cov <= 0.0 {
                    continue;
                }
                let q = l.point(p, base, len, wid);
                let color = l.color(&q, p, len, wid, e);
                // Normal from the height field (finite differences, one pixel apart).
                let h0 = l.height(&q, len);
                let hx = l.height(&l.point(p + Vec2::X, base, len, wid), len);
                let hy = l.height(&l.point(p + Vec2::Y, base, len, wid), len);
                let n = Vec3::new(-(hx - h0), hy - h0, 1.0).normalize();
                self.put(x, y, color, n, cov);
            }
        }
    }

    /// Finish: fill the color of transparent texels from their neighbors (so filtering doesn't
    /// pull in a dark fringe), build coverage-preserving mips, encode both images.
    fn into_textures(self) -> LeafTextures {
        Canvas::into_mipped_textures(vec![self], false)
    }

    /// Finish a painted image and its painted mip levels (each half the size of the one
    /// before); the levels below the last painted one are averaged from it. `dither`: see
    /// [`encode_color`].
    fn into_mipped_textures(levels: Vec<Canvas>, dither: bool) -> LeafTextures {
        let size = levels[0].size;
        let (px, nrm): (Vec<_>, Vec<_>) = levels
            .into_iter()
            .map(|mut c| {
                c.prepare();
                (c.px, c.nrm)
            })
            .unzip();
        LeafTextures {
            color: encode_color(size, px, dither),
            normal: encode_normal(size, nrm),
        }
    }

    /// Normalize the normals, un-premultiply the edges and fill transparent texels' color.
    fn prepare(&mut self) {
        let size = self.size as usize;
        // Normalize the blended normals.
        for n in &mut self.nrm {
            let v = Vec3::from(*n).normalize_or(Vec3::Z);
            *n = if v.z < 0.05 {
                Vec3::new(v.x, v.y, 0.05).normalize().into()
            } else {
                v.into()
            };
        }
        // Un-premultiply partially covered texels' color? Colors are already "over" blended
        // onto black, so divide by coverage at the edges.
        for p in &mut self.px {
            if p[3] > 0.0 && p[3] < 1.0 {
                for c in 0..3 {
                    p[c] = (p[c] / p[3]).min(1.5);
                }
            }
        }
        // Dilate color into transparent texels a few times, then fill the rest with the mean.
        let (mut sum, mut n) = ([0.0f32; 3], 0.0f32);
        for p in &self.px {
            if p[3] > 0.5 {
                for c in 0..3 {
                    sum[c] += p[c];
                }
                n += 1.0;
            }
        }
        let avg = sum.map(|v| v / n.max(1.0));
        let mut known: Vec<bool> = self.px.iter().map(|p| p[3] > 0.02).collect();
        for _ in 0..6 {
            let mut next = known.clone();
            let src = self.px.clone();
            for y in 0..size {
                for x in 0..size {
                    let i = y * size + x;
                    if known[i] {
                        continue;
                    }
                    let (mut acc, mut k) = ([0.0f32; 3], 0.0f32);
                    for (dx, dy) in [(-1i32, 0i32), (1, 0), (0, -1), (0, 1)] {
                        let (nx, ny) = (x as i32 + dx, y as i32 + dy);
                        if nx < 0 || ny < 0 || nx >= size as i32 || ny >= size as i32 {
                            continue;
                        }
                        let j = ny as usize * size + nx as usize;
                        if known[j] {
                            for c in 0..3 {
                                acc[c] += src[j][c];
                            }
                            k += 1.0;
                        }
                    }
                    if k > 0.0 {
                        for (dst, a) in self.px[i][..3].iter_mut().zip(acc) {
                            *dst = a / k;
                        }
                        next[i] = true;
                    }
                }
            }
            known = next;
        }
        for (p, k) in self.px.iter_mut().zip(&known) {
            if !k {
                p[..3].copy_from_slice(&avg);
            }
        }
    }
}

/// Mip chain of a square image of `[f32; N]` pixels: the given levels (each half the size
/// of the one before), then box-filtered down from the last of them to 1×1.
fn mip_chain<const N: usize>(mut levels: Vec<Vec<[f32; N]>>) -> Vec<Vec<[f32; N]>> {
    while levels.last().unwrap().len() > 1 {
        let prev = levels.last().unwrap();
        let ps = (prev.len() as f32).sqrt() as u32;
        let ns = (ps / 2).max(1);
        let mut next = vec![[0.0f32; N]; (ns * ns) as usize];
        for y in 0..ns {
            for x in 0..ns {
                let mut acc = [0.0f32; N];
                for (dx, dy) in [(0, 0), (1, 0), (0, 1), (1, 1)] {
                    let q =
                        prev[((y * 2 + dy).min(ps - 1) * ps + (x * 2 + dx).min(ps - 1)) as usize];
                    for c in 0..N {
                        acc[c] += q[c] * 0.25;
                    }
                }
                next[(y * ns + x) as usize] = acc;
            }
        }
        levels.push(next);
    }
    levels
}

/// Encode the color with its mips. Each mip keeps the full-size image's alpha coverage:
/// scaled until as many texels pass the cutoff, or with `dither` error-diffused to 0 or 1 at
/// the same mean, so a thin, see-through brush of needles stays a speckle of texels (which
/// the temporal anti-aliasing's jitter resolves to the sky showing through) instead of
/// swelling into a solid blob.
fn encode_color(size: u32, px: Vec<Vec<[f32; 4]>>, dither: bool) -> Image {
    let levels = mip_chain(px);
    let target = coverage(&levels[0], 1.0);
    let encode = |level: &[[f32; 4]], alpha: Vec<f32>| -> Vec<u8> {
        let mut out = Vec::with_capacity(level.len() * 4);
        for (p, a) in level.iter().zip(alpha) {
            for c in &p[..3] {
                out.push((linear_to_srgb(*c).clamp(0.0, 1.0) * 255.0 + 0.5) as u8);
            }
            out.push((a.clamp(0.0, 1.0) * 255.0 + 0.5) as u8);
        }
        out
    };
    let scaled = |level: &[[f32; 4]], scale: f32| level.iter().map(|p| p[3] * scale).collect();
    let data = encode(&levels[0], scaled(&levels[0], 1.0));
    let mips = levels[1..]
        .iter()
        .map(|level| {
            if dither {
                let mean = level.iter().map(|p| p[3]).sum::<f32>() / level.len() as f32;
                let alpha = dither_alpha(scaled(level, target / mean.max(1e-6)));
                return encode(level, alpha);
            }
            // Find the alpha scale that keeps the coverage above the cutoff.
            let (mut lo, mut hi) = (0.5f32, 6.0f32);
            for _ in 0..14 {
                let mid = 0.5 * (lo + hi);
                if coverage(level, mid) < target {
                    lo = mid;
                } else {
                    hi = mid;
                }
            }
            encode(level, scaled(level, 0.5 * (lo + hi)))
        })
        .collect();
    let mut image = Image::new(size, size, data, true);
    image.mips = mips;
    image
}

/// Floyd-Steinberg error diffusion (serpentine) of a square image's alpha to 0 or 1.
fn dither_alpha(mut a: Vec<f32>) -> Vec<f32> {
    let n = (a.len() as f32).sqrt() as usize;
    for y in 0..n {
        let forward = y % 2 == 0;
        for i in 0..n {
            let x = if forward { i } else { n - 1 - i };
            let old = a[y * n + x];
            let new = if old >= ALPHA_CUTOFF { 1.0 } else { 0.0 };
            a[y * n + x] = new;
            let e = old - new;
            let ahead = if forward { x + 1 } else { x.wrapping_sub(1) };
            let behind = if forward { x.wrapping_sub(1) } else { x + 1 };
            for (xx, yy, w) in [
                (ahead, y, 7.0 / 16.0),
                (behind, y + 1, 3.0 / 16.0),
                (x, y + 1, 5.0 / 16.0),
                (ahead, y + 1, 1.0 / 16.0),
            ] {
                if xx < n && yy < n {
                    a[yy * n + xx] += e * w;
                }
            }
        }
    }
    a
}

fn encode_normal(size: u32, nrm: Vec<Vec<[f32; 3]>>) -> Image {
    let levels = mip_chain(nrm);
    let encode = |level: &[[f32; 3]]| -> Vec<u8> {
        let mut out = Vec::with_capacity(level.len() * 4);
        for n in level {
            let v = Vec3::from(*n).normalize_or(Vec3::Z);
            for c in [v.x, v.y, v.z] {
                out.push(((c * 0.5 + 0.5).clamp(0.0, 1.0) * 255.0 + 0.5) as u8);
            }
            out.push(255);
        }
        out
    };
    let data = encode(&levels[0]);
    let mips = levels[1..].iter().map(|l| encode(l)).collect();
    let mut image = Image::new(size, size, data, false);
    image.mips = mips;
    image
}

fn coverage(level: &[[f32; 4]], scale: f32) -> f32 {
    level
        .iter()
        .filter(|p| p[3] * scale >= ALPHA_CUTOFF)
        .count() as f32
        / level.len() as f32
}

fn linear_to_srgb(c: f32) -> f32 {
    if c <= 0.003_130_8 {
        c * 12.92
    } else {
        1.055 * c.powf(1.0 / 2.4) - 0.055
    }
}

fn rot(v: Vec2, a: f32) -> Vec2 {
    let (s, c) = a.sin_cos();
    Vec2::new(v.x * c - v.y * s, v.x * s + v.y * c)
}

/// Mix two colors.
fn mix3(a: [f32; 3], b: [f32; 3], t: f32) -> [f32; 3] {
    [0, 1, 2].map(|i| a[i] + (b[i] - a[i]) * t)
}

fn scale3(a: [f32; 3], s: f32) -> [f32; 3] {
    a.map(|c| c * s)
}

/// The outline and surface of a kind of leaf.
#[derive(Clone, Copy)]
struct LeafShape {
    /// Half-width ∝ u^a (1-u)^b along the midrib (u 0 base .. 1 tip): `a < b` widest near the
    /// base (birch), `a > b` widest toward the tip (oak).
    a: f32,
    b: f32,
    /// Narrowing into a drawn-out (acuminate) tip.
    tip: f32,
    /// Teeth per side and their depth (fraction of the half-width), and finer second teeth.
    teeth: f32,
    tooth_depth: f32,
    fine_teeth: f32,
    /// Rounded lobes per side and the depth of the sinuses between them (oak).
    lobes: f32,
    lobe_depth: f32,
    /// Ear-like lobes at the base (oak).
    auricle: f32,
    /// Side veins per side, and how far they sweep toward the tip across the blade.
    veins: f32,
    vein_sweep: f32,
    /// Fold along the midrib (slope of each half), and the blistered relief between veins.
    fold: f32,
    blister: f32,
}

/// One leaf blade to paint (canvas units).
struct Blade {
    shape: LeafShape,
    base: Vec2,
    dir: Vec2,
    length: f32,
    width: f32,
    color: [f32; 3],
    /// Shows its paler, matte underside.
    under: bool,
    /// Slope of the whole leaf (its tilt out of the card plane), per pixel.
    tilt: Vec2,
    /// Lobe and tooth phase, so neighbors differ.
    phase: f32,
    /// Brown spots: (u, across, radius as a fraction of the width).
    spots: [(f32, f32, f32); 2],
    seed: u64,
}

impl Blade {
    /// Half-width of the outline at `u` on side `side` (±1), as a fraction of the half width.
    fn half(&self, u: f32, side: f32) -> f32 {
        if !(0.0..=1.0).contains(&u) {
            return 0.0;
        }
        let s = &self.shape;
        let us = s.a / (s.a + s.b);
        let mut w = (u / us).powf(s.a) * ((1.0 - u) / (1.0 - us)).powf(s.b);
        w *= 1.0 - s.tip * smoothstep(0.6, 1.0, u);
        if s.lobes > 0.0 {
            // Alternate lobes on the two sides.
            let f = (u * s.lobes + self.phase + if side > 0.0 { 0.0 } else { 0.45 }).fract();
            let lobe = (PI * f).sin().max(0.0).powf(0.55);
            let depth =
                s.lobe_depth * smoothstep(0.1, 0.3, u) * (1.0 - 0.7 * smoothstep(0.78, 1.0, u));
            w *= 1.0 - depth * (1.0 - lobe);
        }
        if s.auricle > 0.0 && u < 0.16 {
            w = w.max(s.auricle * (PI * u / 0.16).sin());
        }
        if s.teeth > 0.0 {
            let ramp = smoothstep(0.08, 0.25, u) * (1.0 - smoothstep(0.92, 1.0, u));
            let f = (u * s.teeth + self.phase * 3.1 + side * 0.25).fract();
            let g = (u * s.teeth * 3.0 + self.phase * 7.3).fract();
            w *= 1.0 - ramp * (s.tooth_depth * (1.0 - f.powf(1.4)) + s.fine_teeth * (1.0 - g));
        }
        w.max(0.0)
    }

    /// Signed distance (pixels, positive inside) from the outline at pixel `p`.
    fn edge(&self, p: Vec2, base: Vec2, len: f32, wid: f32) -> f32 {
        let side_dir = Vec2::new(-self.dir.y, self.dir.x);
        let d = p - base;
        let u = d.dot(self.dir) / len;
        let v = d.dot(side_dir);
        if !(0.0..=1.0).contains(&u) {
            return -10.0 - (u - u.clamp(0.0, 1.0)).abs() * len;
        }
        let side = if v >= 0.0 { 1.0 } else { -1.0 };
        self.half(u, side) * wid * 0.5 - v.abs()
    }

    /// Where pixel `p` lies on the blade.
    fn point(&self, p: Vec2, base: Vec2, len: f32, wid: f32) -> BladePoint {
        let s = &self.shape;
        let side_dir = Vec2::new(-self.dir.y, self.dir.x);
        let d = p - base;
        let u = (d.dot(self.dir) / len).clamp(0.0, 1.0);
        let v = d.dot(side_dir);
        let side = if v >= 0.0 { 1.0 } else { -1.0 };
        let hw = (self.half(u, side) * wid * 0.5).max(0.75);
        let across = (v / hw).clamp(-1.0, 1.0);
        let px = len / 100.0;
        // Midrib and side veins sweeping toward the tip; on lobed leaves each vein runs into
        // the middle of a lobe.
        let mid_w = (1.6 * px * (1.0 - 0.65 * u)).max(0.7);
        let mid = (-(v / mid_w) * (v / mid_w)).exp();
        let sweep = s.vein_sweep * (across.abs().powf(1.3) - 1.0);
        let phase = if s.lobes > 0.0 {
            (u - sweep) * s.lobes + self.phase + if side > 0.0 { 0.0 } else { 0.45 } - 0.5
        } else {
            (u - sweep) * s.veins + self.phase * 0.5
        };
        let count = if s.lobes > 0.0 { s.lobes } else { s.veins };
        let dv = (phase - phase.round()) / count * len;
        let vein_w = (0.7 * px).max(0.55);
        let vein = (-(dv / vein_w) * (dv / vein_w)).exp()
            * smoothstep(0.0, 2.5 * mid_w, v.abs())
            * (1.0 - smoothstep(0.75, 1.0, across.abs()))
            * smoothstep(0.03, 0.12, u);
        BladePoint {
            d,
            u,
            v,
            hw,
            across,
            px,
            mid,
            vein,
            phase,
        }
    }

    /// Surface height (pixels): the whole leaf tilts, the halves fold up from the midrib, the
    /// edges curl down, the tip droops, veins sink in (stand out on the underside) and the
    /// blade bulges between them.
    fn height(&self, q: &BladePoint, len: f32) -> f32 {
        let s = &self.shape;
        let relief = if self.under { -1.0 } else { 1.0 };
        let blister =
            s.blister * (PI * (q.phase - q.phase.floor())).sin() * (1.0 - q.across.abs()) * q.px;
        self.tilt.dot(q.d) + s.fold * q.v.abs()
            - 0.12 * q.across * q.across * q.hw
            - 0.025 * q.u * q.u * len
            - relief * (1.1 * q.px * q.mid + 0.6 * q.px * q.vein)
            + relief * blister
    }

    /// Color at pixel `p` (`e` is its distance inside the outline): veins and midrib lighter
    /// and yellower, mottled, a darker rim, a yellower base, the odd brown spot.
    fn color(&self, q: &BladePoint, p: Vec2, len: f32, wid: f32, e: f32) -> [f32; 3] {
        let n = perlin2((p.x * 0.09) as f64, (p.y * 0.09) as f64, self.seed);
        let n2 = perlin2((p.x * 0.35) as f64, (p.y * 0.35) as f64, self.seed ^ 77);
        let mut c = scale3(self.color, 1.0 + 0.09 * n + 0.04 * n2);
        let vein_color = [c[0] * 1.35, c[1] * 1.28, c[2] * 0.95];
        let k = if self.under { 0.55 } else { 0.75 };
        c = mix3(c, vein_color, (q.mid * k + q.vein * k * 0.6).min(1.0));
        c = mix3(
            c,
            [c[0] * 1.12, c[1] * 1.05, c[2] * 0.7],
            0.5 * (1.0 - smoothstep(0.0, 0.18, q.u)),
        );
        c = scale3(c, 0.84 + 0.16 * smoothstep(0.0, 2.2, e));
        for (su, sa, sr) in self.spots {
            if sr > 0.0 {
                let sp = Vec2::new((q.u - su) * len, (q.across - sa) * q.hw);
                let r = sr * wid;
                let t = 1.0 - smoothstep(r * 0.4, r, sp.length());
                c = mix3(c, [0.55, 0.38, 0.16], t * 0.8);
            }
        }
        c
    }
}

/// A pixel's place on a blade (see [`Blade::point`]).
struct BladePoint {
    /// Offset from the leaf base, pixels.
    d: Vec2,
    /// 0 base .. 1 tip.
    u: f32,
    /// Signed distance from the midrib, pixels.
    v: f32,
    /// Half-width of the blade here, pixels.
    hw: f32,
    /// -1 .. 1 across the blade.
    across: f32,
    /// Pixels per percent of the leaf length.
    px: f32,
    mid: f32,
    vein: f32,
    phase: f32,
}

/// Leaf shapes and spray layout of a broadleaf kind.
struct BroadStyle {
    shape: LeafShape,
    /// Leaf length range, canvas units, and width as a fraction of the length.
    length: (f32, f32),
    aspect: f32,
    /// Petiole length as a fraction of the leaf length.
    petiole: f32,
    /// Base colors (linear, near neutral).
    green: [f32; 3],
}

fn broad_style(kind: LeafKind) -> BroadStyle {
    match kind {
        LeafKind::Lobed => BroadStyle {
            shape: LeafShape {
                a: 0.95,
                b: 0.55,
                tip: 0.15,
                teeth: 0.0,
                tooth_depth: 0.0,
                fine_teeth: 0.0,
                lobes: 5.2,
                lobe_depth: 0.45,
                auricle: 0.32,
                veins: 4.6,
                vein_sweep: 0.12,
                fold: 0.28,
                blister: 0.5,
            },
            length: (0.14, 0.19),
            aspect: 0.62,
            petiole: 0.05,
            green: [0.76, 0.9, 0.74],
        },
        LeafKind::Small => BroadStyle {
            shape: LeafShape {
                a: 0.32,
                b: 0.95,
                tip: 0.35,
                teeth: 11.0,
                tooth_depth: 0.09,
                fine_teeth: 0.035,
                lobes: 0.0,
                lobe_depth: 0.0,
                auricle: 0.0,
                veins: 6.5,
                vein_sweep: 0.2,
                fold: 0.22,
                blister: 0.2,
            },
            length: (0.075, 0.1),
            aspect: 0.78,
            petiole: 0.35,
            green: [0.84, 0.92, 0.66],
        },
        _ => BroadStyle {
            shape: LeafShape {
                a: 0.42,
                b: 0.62,
                tip: 0.3,
                teeth: 13.0,
                tooth_depth: 0.06,
                fine_teeth: 0.04,
                lobes: 0.0,
                lobe_depth: 0.0,
                auricle: 0.0,
                veins: 7.0,
                vein_sweep: 0.22,
                fold: 0.3,
                blister: 0.9,
            },
            length: (0.13, 0.17),
            aspect: 0.86,
            petiole: 0.12,
            green: [0.78, 0.9, 0.72],
        },
    }
}

/// A leaf's color: per-leaf brightness and hue, some older yellower leaves, some showing
/// their paler underside.
fn leaf_color(rng: &mut Rng, green: [f32; 3], under: bool) -> [f32; 3] {
    let lum = rng.range(0.74, 1.04);
    let warm = rng.range(-1.0, 1.0);
    let mut c = [
        green[0] * lum * (1.0 + 0.09 * warm),
        green[1] * lum,
        green[2] * lum * (1.0 - 0.12 * warm),
    ];
    if rng.chance(0.08) {
        // An older, yellowing leaf.
        c = [c[0] * 1.18, c[1] * 1.04, c[2] * 0.55];
    }
    if under {
        // Paler, greyer, matte.
        c = mix3(c, [0.95, 1.0, 0.88], 0.32);
    }
    c
}

/// Add a leaf to the list if it fits in the canvas (shrinking it a little if needed).
fn push_leaf(
    leaves: &mut Vec<Blade>,
    rng: &mut Rng,
    st: &BroadStyle,
    at: Vec2,
    dir: Vec2,
    size: f32,
) {
    let mut length = rng.range(st.length.0, st.length.1) * size;
    for _ in 0..4 {
        let pet = length * st.petiole;
        let base = at + dir * pet;
        let side = Vec2::new(-dir.y, dir.x) * length * st.aspect * 0.5;
        let pts = [
            base,
            base + dir * length,
            base + dir * length * 0.55 + side,
            base + dir * length * 0.55 - side,
        ];
        let inside = pts
            .iter()
            .all(|p| p.x > 0.012 && p.y > 0.012 && p.x < 0.988 && p.y < 0.988);
        if inside {
            let under = rng.chance(0.16);
            let spots = if rng.chance(0.1) {
                [
                    (
                        rng.range(0.3, 0.8),
                        rng.range(-0.6, 0.6),
                        rng.range(0.03, 0.07),
                    ),
                    (
                        rng.range(0.3, 0.8),
                        rng.range(-0.6, 0.6),
                        rng.range(0.0, 0.05),
                    ),
                ]
            } else {
                [(0.0, 0.0, 0.0); 2]
            };
            leaves.push(Blade {
                shape: st.shape,
                base,
                dir,
                length,
                width: length * st.aspect * rng.range(0.9, 1.1),
                color: leaf_color(rng, st.green, under),
                under,
                tilt: Vec2::new(rng.range(-0.55, 0.55), rng.range(-0.55, 0.55)),
                phase: rng.range(0.0, 1.0),
                spots,
                seed: rng.next_u64(),
            });
            return;
        }
        length *= 0.82;
    }
}

/// A curving shoot from `start` along `dir`: its points.
fn shoot(start: Vec2, dir: Vec2, length: f32, bend: f32, segs: usize) -> Vec<Vec2> {
    let mut pts = vec![start];
    let mut d = dir;
    for _ in 0..segs {
        d = rot(d, bend / segs as f32);
        pts.push(*pts.last().unwrap() + d * length / segs as f32);
    }
    pts
}

/// Point and direction at fraction `t` along a polyline.
fn along(path: &[Vec2], t: f32) -> (Vec2, Vec2) {
    let n = path.len() - 1;
    let f = (t.clamp(0.0, 1.0) * n as f32).min(n as f32 - 1e-4);
    let i = f as usize;
    let k = f - i as f32;
    (
        path[i].lerp(path[i + 1], k),
        (path[i + 1] - path[i]).normalize_or(Vec2::NEG_Y),
    )
}

const TWIG: [f32; 3] = [0.36, 0.3, 0.24];

/// A twig spray of broad leaves (oak, hazel, birch), growing up from the bottom center.
fn broad_spray(cv: &mut Canvas, kind: LeafKind, rng: &mut Rng) {
    let st = broad_style(kind);
    let base = Vec2::new(0.5, 0.995);
    let up = Vec2::NEG_Y;
    let mut leaves = Vec::new();
    let mut twigs: Vec<(Vec<Vec2>, f32)> = Vec::new();
    match kind {
        LeafKind::Lobed => {
            // Oak: a twig that forks into shoots; leaves crowd at every shoot tip.
            let main = shoot(
                base,
                rot(up, rng.range(-0.15, 0.15)),
                rng.range(0.42, 0.52),
                rng.range(-0.3, 0.3),
                5,
            );
            let fork = *main.last().unwrap();
            let fork_dir = along(&main, 1.0).1;
            let mut tips = Vec::new();
            let n = rng.range_i32(3, 5);
            for k in 0..n {
                let a = (k as f32 / (n - 1) as f32 - 0.5) * 1.9 + rng.range(-0.15, 0.15);
                let s = shoot(
                    fork,
                    rot(fork_dir, a),
                    rng.range(0.16, 0.27),
                    rng.range(-0.4, 0.4),
                    3,
                );
                tips.push((*s.last().unwrap(), along(&s, 1.0).1));
                twigs.push((s, 0.009));
            }
            for (k, t) in [0.45f32, 0.7].iter().enumerate() {
                let (p, d) = along(&main, *t);
                let sgn = if k % 2 == 0 { 1.0 } else { -1.0 };
                let s = shoot(
                    p,
                    rot(d, sgn * rng.range(0.9, 1.25)),
                    rng.range(0.17, 0.25),
                    -sgn * 0.5,
                    3,
                );
                tips.push((*s.last().unwrap(), along(&s, 1.0).1));
                twigs.push((s, 0.009));
            }
            twigs.push((main, 0.014));
            // Leaves along the shoots first (below), then the tip clusters on top.
            for (path, _) in &twigs {
                let (p, d) = along(path, 0.55);
                let sgn = if rng.chance(0.5) { 1.0 } else { -1.0 };
                let ld = rot(d, sgn * rng.range(0.7, 1.2));
                push_leaf(&mut leaves, rng, &st, p, ld, 0.85);
            }
            for (tip, d) in tips {
                let k = rng.range_i32(5, 8);
                for j in 0..k {
                    let a = (j as f32 / (k - 1) as f32 - 0.5) * rng.range(2.6, 3.4)
                        + rng.range(-0.15, 0.15);
                    let size = rng.range(0.85, 1.1);
                    push_leaf(&mut leaves, rng, &st, tip, rot(d, a), size);
                }
            }
        }
        LeafKind::Small => {
            // Birch: long thin twigs fanning out, small leaves alternating and in pairs on
            // short spur shoots.
            let n = rng.range_i32(4, 6);
            for k in 0..n {
                let a = (k as f32 / (n - 1) as f32 - 0.5) * 1.5 + rng.range(-0.1, 0.1);
                let s = shoot(
                    base,
                    rot(up, a),
                    rng.range(0.7, 0.92),
                    rng.range(-0.35, 0.35),
                    6,
                );
                let steps = 10;
                for j in 1..=steps {
                    let t = j as f32 / steps as f32 * 0.95 + 0.02;
                    let (p, d) = along(&s, t);
                    let sgn = if j % 2 == 0 { 1.0 } else { -1.0 };
                    let size = 1.0 - 0.3 * t;
                    if rng.chance(0.35) {
                        // A spur with two leaves.
                        let sd = rot(d, sgn * rng.range(0.6, 1.0));
                        let sp = p + sd * 0.025;
                        twigs.push((vec![p, sp], 0.004));
                        for a in [-0.45f32, 0.45] {
                            let ld = rot(sd, a + rng.range(-0.2, 0.2));
                            push_leaf(&mut leaves, rng, &st, sp, ld, size);
                        }
                    } else {
                        let ld = rot(d, sgn * rng.range(0.55, 1.0));
                        push_leaf(&mut leaves, rng, &st, p, ld, size);
                    }
                }
                let (tip, d) = along(&s, 1.0);
                push_leaf(&mut leaves, rng, &st, tip, d, 0.85);
                twigs.push((s, 0.006));
            }
        }
        _ => {
            // Hazel: arching shoots with leaves alternating in two flat rows.
            let main = shoot(
                base,
                rot(up, rng.range(-0.12, 0.12)),
                rng.range(0.78, 0.88),
                rng.range(-0.35, 0.35),
                6,
            );
            let mut shoots = vec![(main.clone(), 0.012)];
            for (k, t) in [0.3f32, 0.5].iter().enumerate() {
                let (p, d) = along(&main, *t);
                let flip = if rng.chance(0.5) { 1.0 } else { -1.0 };
                let sgn = flip * if k % 2 == 0 { 1.0 } else { -1.0 };
                shoots.push((
                    shoot(
                        p,
                        rot(d, sgn * rng.range(0.6, 0.85)),
                        rng.range(0.35, 0.48),
                        -sgn * 0.3,
                        4,
                    ),
                    0.008,
                ));
            }
            for (s, w) in &shoots {
                let steps = 5;
                for j in 1..=steps {
                    let t = j as f32 / steps as f32 * 0.82 + 0.08;
                    let (p, d) = along(s, t);
                    let sgn = if j % 2 == 0 { 1.0 } else { -1.0 };
                    let ld = rot(d, sgn * rng.range(0.7, 1.05));
                    push_leaf(&mut leaves, rng, &st, p, ld, 1.0 - 0.25 * t);
                }
                let (tip, d) = along(s, 1.0);
                push_leaf(&mut leaves, rng, &st, tip, d, 0.75);
                twigs.push((s.clone(), *w));
            }
        }
    }
    for (path, w) in &twigs {
        cv.stem_path(path, *w, *w * 0.45, TWIG);
    }
    // Petioles, then the leaves (in growth order: later ones overlap earlier ones).
    for l in &leaves {
        let at = l.base - l.dir * l.length * broad_style(kind).petiole;
        cv.stem(
            at,
            l.base + l.dir * l.length * 0.04,
            0.0045,
            0.0035,
            TWIG,
            mix3(TWIG, l.color, 0.5),
        );
    }
    for l in &leaves {
        cv.blade(l);
    }
}

/// A cluster of shoot tips seen from outside the crown: rosettes of leaves filling a disc.
fn broad_cluster(cv: &mut Canvas, kind: LeafKind, rng: &mut Rng) {
    let st = broad_style(kind);
    let center = Vec2::splat(0.5);
    let mut leaves = Vec::new();
    let rosettes = if kind == LeafKind::Small { 7 } else { 5 };
    let mut centers = vec![center];
    for k in 1..rosettes {
        let a = k as f32 * 2.4 + rng.range(-0.3, 0.3);
        centers.push(center + Vec2::new(a.cos(), a.sin()) * rng.range(0.16, 0.24));
    }
    for (i, c) in centers.iter().enumerate().rev() {
        let k = if kind == LeafKind::Small {
            6
        } else {
            6 + (i == 0) as i32
        };
        let a0 = rng.range(0.0, TAU);
        for j in 0..k {
            let a = a0 + j as f32 / k as f32 * TAU + rng.range(-0.25, 0.25);
            let size = if kind == LeafKind::Small { 1.1 } else { 0.95 };
            push_leaf(&mut leaves, rng, &st, *c, Vec2::new(a.cos(), a.sin()), size);
        }
    }
    for l in &leaves {
        cv.blade(l);
    }
}

/// Needle color: older needles darker, new growth lighter and yellower.
fn needle_color(rng: &mut Rng, base: [f32; 3], fresh: f32) -> [f32; 3] {
    let lum = rng.range(0.7, 1.05);
    let c = scale3(base, lum);
    mix3(c, [base[0] * 1.12, base[1] * 1.1, base[2] * 0.86], fresh)
}

/// A brush of needles along `path` from `t0` to its tip.
#[allow(clippy::too_many_arguments)]
fn needle_brush(
    cv: &mut Canvas,
    rng: &mut Rng,
    path: &[Vec2],
    t0: f32,
    length: f32,
    angle: (f32, f32),
    width: f32,
    step: f32,
    color: [f32; 3],
    fresh_from: f32,
) {
    let total: f32 = path.windows(2).map(|w| w[0].distance(w[1])).sum();
    let n = ((total * (1.0 - t0)) / step).ceil().max(1.0) as usize;
    for i in 0..n {
        let t = t0 + (1.0 - t0) * (i as f32 + rng.range(0.0, 1.0)) / n as f32;
        let (p, d) = along(path, t);
        // Needles get shorter toward the tip, and lean forward there.
        let taper = 1.0 - 0.45 * smoothstep(0.82, 1.0, t);
        let fresh = smoothstep(fresh_from, 1.0, t);
        for side in [-1.0f32, 1.0] {
            let a = side * rng.range(angle.0, angle.1) * (1.0 - 0.4 * smoothstep(0.85, 1.0, t));
            let nd = rot(d, a);
            let l = length * rng.range(0.8, 1.15) * taper;
            let bend = rot(nd, side * rng.range(-0.12, 0.2));
            let c = needle_color(rng, color, fresh);
            let Some(k) = cv.needle_width(width, l) else {
                continue;
            };
            let (w, mid) = (width * k, p + nd * l * 0.5);
            cv.stem(p, mid, w, w * 0.85, scale3(c, 0.8), c);
            cv.stem(mid, mid + bend * l * 0.5, w * 0.85, w * 0.35, c, c);
        }
        // A needle pointing toward or away from the viewer: foreshortened.
        if rng.chance(0.7) {
            let nd = rot(d, rng.range(-1.2, 1.2));
            let l = length * rng.range(0.25, 0.6) * taper;
            let c = needle_color(rng, color, fresh);
            if let Some(k) = cv.needle_width(width, l) {
                let w = width * k;
                cv.stem(p, p + nd * l, w * 1.1, w * 0.5, scale3(c, 0.85), c);
            }
        }
    }
}

const PINE_NEEDLE: [f32; 3] = [0.74, 0.86, 0.74];
const SPRUCE_NEEDLE: [f32; 3] = [0.72, 0.86, 0.7];

/// Length of a polyline.
fn path_length(path: &[Vec2]) -> f32 {
    path.windows(2).map(|w| w[0].distance(w[1])).sum()
}

/// Pine needles on the last `reach` (canvas units) of a shoot: a bottle-brush of needles
/// spread all round it, gathered into a tuft at its tip. The needles behind the shoot are
/// painted first, in its shade.
fn pine_brush(cv: &mut Canvas, rng: &mut Rng, path: &[Vec2], reach: f32, shade: f32) {
    let total = path_length(path).max(1e-3);
    needle_brush(
        cv,
        rng,
        path,
        (1.0 - reach / total).max(0.0),
        PINE_NEEDLE_LENGTH,
        (0.75, 1.25),
        0.003,
        0.0045,
        scale3(PINE_NEEDLE, shade),
        (1.0 - 0.06 / total).max(0.0),
    );
}

/// How long a pine needle is on a spray card (canvas units): a card spans a branch end of
/// about a meter, and a Scots pine's needles are 4-7 cm.
const PINE_NEEDLE_LENGTH: f32 = 0.055;

/// Pine: the end of a branch, at true scale. A shoot with a whorl of side shoots for each of
/// the last few years (the older, lower ones longer and forking again), every shoot bare
/// wood except for the brush of needles on its last 20-odd centimeters. From afar a card
/// reads as a ragged pad of small brushes, as a pine's foliage does, rather than as one big
/// leaf (which oversized brushes did).
fn pine_spray(cv: &mut Canvas, rng: &mut Rng) {
    let base = Vec2::new(0.5, 0.995);
    let main = shoot(
        base,
        rot(Vec2::NEG_Y, rng.range(-0.15, 0.15)),
        rng.range(0.78, 0.86),
        rng.range(-0.35, 0.35),
        8,
    );
    let mut shoots: Vec<Vec<Vec2>> = Vec::new();
    for (t, len, forks) in [(0.3, 0.36, true), (0.53, 0.3, true), (0.75, 0.2, false)] {
        let (p, d) = along(&main, t + rng.range(-0.04, 0.04));
        for sgn in [-1.0f32, 1.0] {
            let a = sgn * rng.range(0.55, 0.95);
            let l = len * rng.range(0.8, 1.15);
            // Side shoots curve back up toward the light.
            let s = shoot(p, rot(d, a), l, -sgn * rng.range(0.2, 0.5), 5);
            if forks {
                let (q, e) = along(&s, rng.range(0.45, 0.6));
                let b = rng.range(0.5, 0.9) * if rng.chance(0.5) { 1.0 } else { -1.0 };
                shoots.push(shoot(q, rot(e, b), l * 0.5, -b * 0.5, 3));
            }
            shoots.push(s);
        }
        // One pointing at the viewer, foreshortened.
        if rng.chance(0.6) {
            let a = rng.range(-0.3, 0.3);
            shoots.push(shoot(p, rot(d, a), len * 0.4, 0.0, 3));
        }
    }
    cv.stem_path(&main, 0.014, 0.006, TWIG);
    for s in &shoots {
        cv.stem_path(s, 0.008, 0.004, TWIG);
    }
    shoots.push(main);
    for shade in [0.6, 1.0] {
        for s in &shoots {
            pine_brush(cv, rng, s, 0.22, shade);
        }
    }
}

/// Spruce: a flat, comb-like spray of shoots densely covered in short needles.
fn spruce_spray(cv: &mut Canvas, rng: &mut Rng) {
    let base = Vec2::new(0.5, 0.995);
    let main = shoot(
        base,
        rot(Vec2::NEG_Y, rng.range(-0.1, 0.1)),
        rng.range(0.86, 0.93),
        rng.range(-0.25, 0.25),
        7,
    );
    let mut shoots: Vec<(Vec<Vec2>, f32)> = Vec::new();
    let n = 11;
    for k in 0..n {
        let t = 0.08 + 0.84 * (k as f32 + rng.range(0.0, 0.6)) / n as f32;
        let (p, d) = along(&main, t);
        let sgn = if k % 2 == 0 { 1.0 } else { -1.0 };
        let l = 0.4 * (1.0 - 0.75 * t) * rng.range(0.85, 1.1);
        let s = shoot(p, rot(d, sgn * rng.range(0.75, 1.05)), l, -sgn * 0.25, 4);
        if l > 0.17 {
            for (j, tt) in [0.4f32, 0.7].iter().enumerate() {
                let (q, e) = along(&s, *tt);
                let sg = if j % 2 == 0 { sgn } else { -sgn };
                shoots.push((
                    shoot(
                        q,
                        rot(e, sg * rng.range(0.6, 0.85)),
                        l * (1.0 - tt) * 0.7,
                        0.0,
                        2,
                    ),
                    0.004,
                ));
            }
        }
        shoots.push((s, 0.006));
    }
    cv.stem_path(&main, 0.012, 0.006, TWIG);
    for (s, w) in &shoots {
        cv.stem_path(s, *w, *w * 0.6, TWIG);
    }
    for (s, _) in &shoots {
        needle_brush(
            cv,
            rng,
            s,
            0.0,
            0.04,
            (0.6, 1.15),
            0.0044,
            0.0042,
            SPRUCE_NEEDLE,
            0.75,
        );
    }
    needle_brush(
        cv,
        rng,
        &main,
        0.05,
        0.046,
        (0.6, 1.15),
        0.0048,
        0.0042,
        SPRUCE_NEEDLE,
        0.85,
    );
}

/// A tuft of needles seen from the shoot tip: shoots radiating from the center.
fn needle_tuft(cv: &mut Canvas, rng: &mut Rng, long: bool) {
    let c = Vec2::splat(0.5);
    let color = if long { PINE_NEEDLE } else { SPRUCE_NEEDLE };
    if long {
        // A branch end seen from its tip: shoots radiating from the middle, each with its
        // brush, and the nearest brushes seen end-on as round tufts.
        let k = rng.range_i32(5, 8);
        let a0 = rng.range(0.0, TAU);
        let mut shoots = Vec::new();
        for i in 0..k {
            let a = a0 + i as f32 / k as f32 * TAU + rng.range(-0.3, 0.3);
            let len = rng.range(0.24, 0.36);
            shoots.push(shoot(
                c,
                Vec2::new(a.cos(), a.sin()),
                len,
                rng.range(-0.3, 0.3),
                4,
            ));
        }
        for s in &shoots {
            cv.stem_path(s, 0.01, 0.005, TWIG);
        }
        for shade in [0.6, 1.0] {
            for s in &shoots {
                pine_brush(cv, rng, s, 0.2, shade);
            }
        }
        // End-on brushes: needles splaying from a point, the middle one the leader's.
        let tufts = rng.range_i32(3, 6);
        for j in 0..=tufts {
            let at = if j == 0 {
                c
            } else {
                let a = rng.range(0.0, TAU);
                c + Vec2::new(a.cos(), a.sin()) * rng.range(0.1, 0.24)
            };
            for _ in 0..70 {
                let a = rng.range(0.0, TAU);
                let d = Vec2::new(a.cos(), a.sin());
                let l = PINE_NEEDLE_LENGTH * rng.range(0.5, 1.3);
                let fresh = rng.range(0.0, 0.5);
                let col = needle_color(rng, color, fresh);
                let p = at + d * rng.range(0.0, 0.012);
                let bend = rot(d, rng.range(-0.15, 0.15));
                let Some(k) = cv.needle_width(0.0032, l) else {
                    continue;
                };
                cv.stem(
                    p,
                    p + d * l * 0.5,
                    0.0032 * k,
                    0.0028 * k,
                    scale3(col, 0.75),
                    col,
                );
                cv.stem(
                    p + d * l * 0.5,
                    p + d * l * 0.5 + bend * l * 0.5,
                    0.0028 * k,
                    0.0012 * k,
                    col,
                    col,
                );
            }
            // The bud in the middle.
            cv.stem(
                at - Vec2::splat(0.006),
                at + Vec2::splat(0.006),
                0.016,
                0.016,
                [0.42, 0.3, 0.2],
                [0.5, 0.36, 0.22],
            );
        }
    } else {
        let n = 8;
        for k in 0..n {
            let a = k as f32 / n as f32 * TAU + rng.range(-0.2, 0.2);
            let s = shoot(
                c,
                Vec2::new(a.cos(), a.sin()),
                rng.range(0.3, 0.42),
                rng.range(-0.3, 0.3),
                3,
            );
            cv.stem_path(&s, 0.006, 0.004, TWIG);
            needle_brush(
                cv,
                rng,
                &s,
                0.0,
                0.036,
                (0.6, 1.15),
                0.0044,
                0.0045,
                SPRUCE_NEEDLE,
                0.7,
            );
        }
    }
}

/// Paint one cell of an atlas (or the whole frond image) on its own canvas.
fn paint_cell(kind: LeafKind, cell: usize, size: u32, seed: u64, min_needle_px: f32) -> Canvas {
    let mut cv = Canvas::new(size);
    cv.min_needle_px = min_needle_px;
    let mut rng = Rng::new(seed ^ (cell as u64).wrapping_mul(0x9e37_79b9_7f4a_7c15));
    match kind {
        LeafKind::Broad | LeafKind::Small | LeafKind::Lobed => {
            if cell == CLUSTER_CELL {
                broad_cluster(&mut cv, kind, &mut rng);
            } else {
                broad_spray(&mut cv, kind, &mut rng);
            }
        }
        LeafKind::LongNeedles | LeafKind::ShortNeedles => {
            let long = kind == LeafKind::LongNeedles;
            if cell == CLUSTER_CELL {
                needle_tuft(&mut cv, &mut rng, long);
            } else if long {
                pine_spray(&mut cv, &mut rng);
            } else {
                spruce_spray(&mut cv, &mut rng);
            }
        }
        LeafKind::Frond => {
            let base = Vec2::new(0.5, 0.99);
            // Rachis up the middle, pinnae on both sides shrinking toward the tip.
            let segs = 26;
            let mut prev = base;
            for s in 1..=segs {
                let f = s as f32 / segs as f32;
                let p = Vec2::new(0.5 + 0.02 * (f * 6.0).sin(), 0.99 - 0.96 * f);
                cv.line(
                    prev,
                    p,
                    0.014 * (1.0 - 0.6 * f),
                    0.012 * (1.0 - 0.6 * f),
                    [0.55, 0.62, 0.35],
                );
                prev = p;
                if f < 0.06 {
                    continue;
                }
                // Lanceolate outline: widest a third of the way up.
                let reach = 0.46 * (std::f32::consts::PI * f.powf(0.7)).sin().max(0.05);
                for side in [-1.0f32, 1.0] {
                    let d = Vec2::new(side * 0.93, -0.36).normalize();
                    cv.leaf(
                        p,
                        d,
                        reach,
                        0.05 + 0.02 * (1.0 - f),
                        rng.range(0.75, 1.0),
                        0.35,
                        rng.range(-1.0, 1.0),
                    );
                }
            }
        }
    }
    cv
}

/// Paint a leaf texture and its normal map. Trees and shrubs get an atlas of
/// [`ATLAS_CELLS`]² cells (see [`atlas_cell`]); fronds one image.
pub fn leaf_textures(kind: LeafKind, size: u32, seed: u64) -> LeafTextures {
    if !kind.is_atlas() {
        return paint_cell(kind, 0, size, seed, 0.0).into_textures();
    }
    let cs = (size / ATLAS_CELLS).max(1);
    let mut levels = vec![paint_atlas(kind, cs, seed, 0.0)];
    if kind.is_needles() {
        // Paint the mip levels too, needles at least a pixel wide (see `Canvas::needle_width`).
        let mut c = cs / 2;
        while c >= MIN_PAINTED_CELL {
            levels.push(paint_atlas(kind, c, seed, 1.0));
            c /= 2;
        }
    }
    Canvas::into_mipped_textures(levels, kind.is_needles())
}

/// Paint every cell of an atlas, `cs` pixels each.
fn paint_atlas(kind: LeafKind, cs: u32, seed: u64, min_needle_px: f32) -> Canvas {
    let n = ATLAS_CELLS;
    // The cells are independent: paint them in parallel.
    let cells: Vec<Canvas> = std::thread::scope(|scope| {
        let handles: Vec<_> = (0..(n * n) as usize)
            .map(|c| scope.spawn(move || paint_cell(kind, c, cs, seed, min_needle_px)))
            .collect();
        handles
            .into_iter()
            .map(|h| h.join().expect("leaf painter"))
            .collect()
    });
    let size = cs * n;
    let mut atlas = Canvas::new(size);
    for (c, cell) in cells.iter().enumerate() {
        let (ox, oy) = ((c as u32 % n) * cs, (c as u32 / n) * cs);
        for y in 0..cs {
            let src = (y * cs) as usize..((y + 1) * cs) as usize;
            let dst = ((oy + y) * size + ox) as usize..((oy + y) * size + ox + cs) as usize;
            atlas.px[dst.clone()].copy_from_slice(&cell.px[src.clone()]);
            atlas.nrm[dst].copy_from_slice(&cell.nrm[src]);
        }
    }
    atlas
}

/// Paint a leaf texture (color and alpha only; see [`leaf_textures`] for the normal map).
pub fn leaf_texture(kind: LeafKind, size: u32, seed: u64) -> Image {
    leaf_textures(kind, size, seed).color
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn leaf_textures_have_cutouts_and_keep_coverage_in_mips() {
        for kind in [
            LeafKind::Broad,
            LeafKind::Small,
            LeafKind::Lobed,
            LeafKind::LongNeedles,
            LeafKind::ShortNeedles,
            LeafKind::Frond,
        ] {
            let img = leaf_texture(kind, 128, 7);
            assert!(img.is_valid(), "{kind:?}");
            let cover = |data: &[u8]| {
                data.as_chunks::<4>()
                    .0
                    .iter()
                    .filter(|p| p[3] as f32 / 255.0 >= ALPHA_CUTOFF)
                    .count() as f32
                    / (data.len() / 4) as f32
            };
            let c0 = cover(&img.data);
            assert!(c0 > 0.08 && c0 < 0.9, "{kind:?} coverage {c0}");
            let c3 = cover(&img.mips[2]);
            assert!((c3 - c0).abs() < 0.12, "{kind:?} mip coverage {c3} vs {c0}");
        }
    }

    #[test]
    fn needle_mips_are_see_through_speckles() {
        let opaque = |data: &[u8]| {
            data.as_chunks::<4>()
                .0
                .iter()
                .filter(|p| p[3] >= 128)
                .count() as f32
                / (data.len() / 4) as f32
        };
        for kind in [LeafKind::LongNeedles, LeafKind::ShortNeedles] {
            let img = leaf_texture(kind, 256, 7);
            let c0 = opaque(&img.data);
            for (i, mip) in img.mips.iter().enumerate().take(5) {
                // Each texel passes the cutoff or doesn't (dithered), rather than a brush
                // averaging into a solid, half-transparent blob...
                let texels = mip.as_chunks::<4>().0;
                assert!(
                    texels.iter().all(|p| p[3] == 0 || p[3] == 255),
                    "{kind:?} mip {i}"
                );
                // ...and as many pass as at full size.
                let c = opaque(mip);
                assert!((c - c0).abs() < 0.05, "{kind:?} mip {i}: {c} vs {c0}");
            }
        }
        // Broad leaves keep smooth alpha edges.
        let img = leaf_texture(LeafKind::Lobed, 256, 7);
        assert!(
            img.mips[1]
                .as_chunks::<4>()
                .0
                .iter()
                .any(|p| p[3] > 0 && p[3] < 255)
        );
    }

    #[test]
    fn normal_maps_are_unit_and_flat_outside_the_leaves() {
        for kind in [LeafKind::Lobed, LeafKind::ShortNeedles] {
            let t = leaf_textures(kind, 128, 3);
            assert!(t.normal.is_valid() && !t.normal.srgb);
            assert_eq!(t.normal.mips.len(), t.color.mips.len());
            let (mut bumpy, mut flat) = (0, 0);
            for (n, c) in t
                .normal
                .data
                .as_chunks::<4>()
                .0
                .iter()
                .zip(t.color.data.as_chunks::<4>().0)
            {
                let v = Vec3::new(n[0] as f32, n[1] as f32, n[2] as f32) / 127.5 - Vec3::ONE;
                assert!((v.length() - 1.0).abs() < 0.03, "{kind:?} {v}");
                assert!(v.z > 0.0);
                if c[3] == 0 {
                    assert!(v.z > 0.98, "{kind:?} transparent texel not flat: {v}");
                    flat += 1;
                } else if c[3] >= 128 && v.z < 0.98 {
                    bumpy += 1;
                }
            }
            assert!(bumpy > 150 && flat > 200, "{kind:?} {bumpy} {flat}");
        }
    }

    #[test]
    fn atlas_cells_tile_the_texture() {
        let (a, b) = atlas_cell(LeafKind::Lobed, 0);
        assert!(a.x < 0.01 && b.x > 0.49 && b.x <= 0.5);
        let (a, _) = atlas_cell(LeafKind::Lobed, 3);
        assert!(a.x >= 0.5 && a.y >= 0.5);
        assert_eq!(atlas_cell(LeafKind::Frond, 2), (Vec2::ZERO, Vec2::ONE));
    }
}
