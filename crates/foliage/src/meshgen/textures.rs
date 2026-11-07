//! Procedural leaf textures: per-species atlases of leaf and needle sprays (lobed oak leaves,
//! small serrated birch leaves, rounded toothed hazel leaves, oval beech leaves in flat rows,
//! round long-stalked aspen leaves, compound rowan leaves, long pine needles in brushes, short
//! dense spruce needles, soft larch needles in rosettes), plus fern fronds, painted on the CPU
//! into RGBA images with alpha cut-outs and matching tangent-space normal maps.
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

use engine_core::math::{Rng, Vec2, Vec3, hash01, noise::perlin2, smoothstep};
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
    /// Oval leaves with straight parallel side veins and a wavy, finely toothed edge, in flat
    /// rows along zigzag twigs (beech; goat willow's broader leaves, more of them showing
    /// their silvery underside).
    Oval,
    /// Nearly round, coarsely round-toothed leaves on long petioles, turned every which way
    /// (aspen, alder).
    Round,
    /// Compound leaves: pairs of narrow toothed leaflets along a reddish stalk, fanning from
    /// the twig tips (rowan).
    Pinnate,
    /// Soft short needles in rosettes on spur shoots along drooping twigs, single needles on
    /// the young shoots (larch).
    SoftNeedles,
    /// A stretch of a palm frond: a stout midrib up the middle with long, narrow, stiff
    /// leaflets close-set along both sides, reaching forward (coconut palm). The three spray
    /// cells are the frond's base (bare midrib at the foot, leaflets lengthening), its middle
    /// and its tip (leaflets shortening to the end); the cluster cell is another middle.
    Palm,
    /// Palmate leaves: five pointed, coarsely toothed lobes spreading from the top of a long
    /// petiole, each with its own main vein, round sinuses between them (sugar maple).
    Palmate,
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
        LeafStyle::Oval => LeafKind::Oval,
        LeafStyle::Round => LeafKind::Round,
        LeafStyle::Pinnate => LeafKind::Pinnate,
        LeafStyle::SoftNeedles => LeafKind::SoftNeedles,
        LeafStyle::Palm => LeafKind::Palm,
        LeafStyle::Palmate => LeafKind::Palmate,
    }
}

impl LeafKind {
    /// True for the atlases of trees and shrubs ([`ATLAS_CELLS`]² sprays); fronds fill the
    /// whole image.
    pub fn is_atlas(self) -> bool {
        self != LeafKind::Frond
    }

    /// Pine, spruce and larch needles.
    pub fn is_needles(self) -> bool {
        matches!(
            self,
            LeafKind::LongNeedles | LeafKind::ShortNeedles | LeafKind::SoftNeedles
        )
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

    /// Flip left to right (normals too).
    fn mirror(&mut self) {
        let s = self.size as usize;
        for y in 0..s {
            self.px[y * s..(y + 1) * s].reverse();
            self.nrm[y * s..(y + 1) * s].reverse();
        }
        for n in &mut self.nrm {
            n[0] = -n[0];
        }
    }

    /// Darken everything painted by `f`.
    fn shade(&mut self, f: f32) {
        for p in &mut self.px {
            for c in &mut p[..3] {
                *c *= f;
            }
        }
    }

    /// Lay this canvas over `under` (the same size): what this one leaves uncovered shows
    /// `under`.
    fn over(&mut self, under: &Canvas) {
        for ((p, n), (q, m)) in self
            .px
            .iter_mut()
            .zip(&mut self.nrm)
            .zip(under.px.iter().zip(&under.nrm))
        {
            let a = p[3];
            for c in 0..3 {
                // Colors are blended onto black, so they are already weighted by coverage.
                p[c] += q[c] * (1.0 - a);
                n[c] = m[c] + (n[c] - m[c]) * a;
            }
            p[3] = a + q[3] * (1.0 - a);
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
    /// Tooth profile: 0 sharp, saw-like teeth pointing to the tip; 1 rounded scallops
    /// (crenate: aspen, alder).
    crenate: f32,
    /// Where along the midrib (0 base .. 1 tip) the teeth start (rowan's leaflets are entire
    /// near their base).
    teeth_from: f32,
    /// One small tooth where each side vein meets the edge instead of `teeth` per side
    /// (beech's wavy edge).
    teeth_at_veins: bool,
    /// Rounded lobes per side and the depth of the sinuses between them (oak).
    lobes: f32,
    lobe_depth: f32,
    /// Ear-like lobes at the base (oak).
    auricle: f32,
    /// Side veins per side, how far they sweep toward the tip across the blade, and how they
    /// curve doing it (1: straight, as beech's; more: leaving the midrib squarer and bending
    /// toward the tip at the edge).
    veins: f32,
    vein_sweep: f32,
    vein_curve: f32,
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
            let ramp =
                smoothstep(s.teeth_from, s.teeth_from + 0.17, u) * (1.0 - smoothstep(0.92, 1.0, u));
            let g = (u * s.teeth * 3.0 + self.phase * 7.3).fract();
            let notch = if s.teeth_at_veins {
                // The side veins reach the edge where their phase is whole (see `point`): a
                // small point there, the edge sagging gently in between.
                let f = (u * s.veins + self.phase * 0.5).fract();
                (PI * f).sin().max(0.0).powf(1.5)
            } else {
                let f = (u * s.teeth + self.phase * 3.1 + side * 0.25).fract();
                let saw = 1.0 - f.powf(1.4);
                let scallop = 1.0 - (PI * f).sin().max(0.0).powf(0.6);
                saw + (scallop - saw) * s.crenate
            };
            w *= 1.0 - ramp * (s.tooth_depth * notch + s.fine_teeth * (1.0 - g));
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
        let sweep = s.vein_sweep * (across.abs().powf(s.vein_curve) - 1.0);
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
    /// Petiole (rowan: leaf stalk) color and its width at the twig and at the blade, canvas
    /// units of a spray.
    stalk: [f32; 3],
    stalk_width: (f32, f32),
    /// Base colors (linear, near neutral).
    green: [f32; 3],
    /// Share of leaves showing their underside, the color it is paler toward and how far.
    under_share: f32,
    underside: ([f32; 3], f32),
    /// How far a leaf may turn on its petiole, edge-on to the viewer (its painted width shrinks
    /// by up to this fraction), and its tilt out of the card plane (normal-map slope).
    turn: f32,
    tilt: f32,
}

/// Leaflets of a compound (rowan) leaf: width as a fraction of their length.
const PINNA_ASPECT: f32 = 0.3;

/// Rowan's leaf stalk: reddish.
const RACHIS: [f32; 3] = [0.5, 0.31, 0.26];

fn broad_style(kind: LeafKind) -> BroadStyle {
    match kind {
        LeafKind::Palmate => BroadStyle {
            // Sugar maple: the shape is painted lobe by lobe (`palmate_leaf`); these are the
            // lobes' outline and the leaf's size, a little wider than long, on a long stalk.
            shape: LeafShape {
                a: 0.7,
                b: 0.8,
                tip: 0.55,
                teeth: 1.6,
                tooth_depth: 0.28,
                fine_teeth: 0.0,
                crenate: 0.0,
                teeth_from: 0.35,
                teeth_at_veins: false,
                lobes: 0.0,
                lobe_depth: 0.0,
                auricle: 0.0,
                veins: 4.0,
                vein_sweep: 0.25,
                vein_curve: 1.3,
                fold: 0.15,
                blister: 0.3,
            },
            length: (0.13, 0.18),
            aspect: 1.05,
            petiole: 0.55,
            stalk: [0.45, 0.33, 0.24],
            stalk_width: (0.004, 0.003),
            green: [0.76, 0.9, 0.72],
            under_share: 0.14,
            underside: PALE_UNDERSIDE,
            turn: 0.15,
            tilt: 0.5,
        },
        LeafKind::Lobed => BroadStyle {
            shape: LeafShape {
                a: 0.95,
                b: 0.55,
                tip: 0.15,
                teeth: 0.0,
                tooth_depth: 0.0,
                fine_teeth: 0.0,
                crenate: 0.0,
                teeth_from: 0.08,
                teeth_at_veins: false,
                lobes: 5.2,
                lobe_depth: 0.45,
                auricle: 0.32,
                veins: 4.6,
                vein_sweep: 0.12,
                vein_curve: 1.3,
                fold: 0.28,
                blister: 0.5,
            },
            length: (0.14, 0.19),
            aspect: 0.62,
            petiole: 0.05,
            stalk: TWIG,
            stalk_width: (0.0045, 0.0035),
            green: [0.76, 0.9, 0.74],
            under_share: 0.16,
            underside: PALE_UNDERSIDE,
            turn: 0.0,
            tilt: 0.55,
        },
        LeafKind::Small => BroadStyle {
            shape: LeafShape {
                a: 0.32,
                b: 0.95,
                tip: 0.35,
                teeth: 11.0,
                tooth_depth: 0.09,
                fine_teeth: 0.035,
                crenate: 0.0,
                teeth_from: 0.08,
                teeth_at_veins: false,
                lobes: 0.0,
                lobe_depth: 0.0,
                auricle: 0.0,
                veins: 6.5,
                vein_sweep: 0.2,
                vein_curve: 1.3,
                fold: 0.22,
                blister: 0.2,
            },
            length: (0.075, 0.1),
            aspect: 0.78,
            petiole: 0.35,
            stalk: TWIG,
            stalk_width: (0.0045, 0.0035),
            green: [0.8, 0.92, 0.72],
            under_share: 0.16,
            underside: PALE_UNDERSIDE,
            turn: 0.0,
            tilt: 0.55,
        },
        LeafKind::Oval => BroadStyle {
            // Beech: elliptic, widest at the middle with a short point; a wavy edge with a
            // tiny tooth at each vein end; 7-ish straight parallel vein pairs; smooth and
            // glossy, so little blistering. Goat willow shares it (broader, wrinkled).
            shape: LeafShape {
                a: 0.6,
                b: 0.62,
                tip: 0.22,
                teeth: 7.5,
                tooth_depth: 0.045,
                fine_teeth: 0.0,
                crenate: 0.0,
                teeth_from: 0.12,
                teeth_at_veins: true,
                lobes: 0.0,
                lobe_depth: 0.0,
                auricle: 0.0,
                veins: 7.5,
                vein_sweep: 0.2,
                vein_curve: 1.0,
                fold: 0.26,
                blister: 0.55,
            },
            length: (0.125, 0.16),
            aspect: 0.62,
            petiole: 0.08,
            stalk: TWIG,
            stalk_width: (0.0045, 0.0035),
            green: [0.8, 0.92, 0.62],
            // Many leaves show an underside paler mostly in blue: under a saturated green
            // (beech) it barely shows, under a grey-green (goat willow) it reads as the felted
            // silver underside, so the species color decides how silvery the crown is.
            under_share: 0.3,
            underside: ([0.94, 1.0, 1.16], 0.5),
            turn: 0.15,
            tilt: 0.45,
        },
        LeafKind::Round => BroadStyle {
            // Aspen, alder: a round blade, coarse rounded teeth (with finer ones between:
            // alder's double teeth), a blunt tip, 5-6 vein pairs, on a long thin petiole.
            shape: LeafShape {
                a: 0.5,
                b: 0.46,
                tip: 0.08,
                teeth: 9.0,
                tooth_depth: 0.075,
                fine_teeth: 0.018,
                crenate: 1.0,
                teeth_from: 0.1,
                teeth_at_veins: false,
                lobes: 0.0,
                lobe_depth: 0.0,
                auricle: 0.0,
                veins: 5.5,
                vein_sweep: 0.3,
                vein_curve: 1.3,
                fold: 0.2,
                blister: 0.5,
            },
            length: (0.1, 0.13),
            aspect: 0.94,
            petiole: 0.75,
            stalk: TWIG,
            stalk_width: (0.0032, 0.0026),
            green: [0.8, 0.92, 0.7],
            // Hanging loosely and turning in the wind: more undersides, many leaves seen
            // edge-on or tipped well out of the card.
            under_share: 0.24,
            underside: PALE_UNDERSIDE,
            turn: 0.5,
            tilt: 0.9,
        },
        LeafKind::Pinnate => BroadStyle {
            // Rowan: one "leaf" is a whole compound leaf (see `pinnate_leaf`); `shape` is its
            // leaflets' (narrow oblong, a sharp point, sharply toothed in the outer two
            // thirds), `aspect` the width of the whole leaf.
            shape: LeafShape {
                a: 0.55,
                b: 0.6,
                tip: 0.35,
                teeth: 10.0,
                tooth_depth: 0.13,
                fine_teeth: 0.0,
                crenate: 0.0,
                teeth_from: 0.3,
                teeth_at_veins: false,
                lobes: 0.0,
                lobe_depth: 0.0,
                auricle: 0.0,
                veins: 9.0,
                vein_sweep: 0.25,
                vein_curve: 1.3,
                fold: 0.3,
                blister: 0.25,
            },
            length: (0.3, 0.38),
            aspect: 0.5,
            petiole: 0.12,
            stalk: RACHIS,
            stalk_width: (0.0058, 0.0046),
            green: [0.78, 0.9, 0.7],
            under_share: 0.16,
            underside: PALE_UNDERSIDE,
            turn: 0.25,
            tilt: 0.5,
        },
        // Needles and fronds have no blades: hazel's numbers only keep the single-leaf
        // helpers ([`hero_leaf_frame`]) defined for every kind.
        LeafKind::Broad
        | LeafKind::LongNeedles
        | LeafKind::ShortNeedles
        | LeafKind::SoftNeedles
        | LeafKind::Frond
        | LeafKind::Palm => BroadStyle {
            shape: LeafShape {
                a: 0.42,
                b: 0.62,
                tip: 0.3,
                teeth: 13.0,
                tooth_depth: 0.06,
                fine_teeth: 0.04,
                crenate: 0.0,
                teeth_from: 0.08,
                teeth_at_veins: false,
                lobes: 0.0,
                lobe_depth: 0.0,
                auricle: 0.0,
                veins: 7.0,
                vein_sweep: 0.22,
                vein_curve: 1.3,
                fold: 0.3,
                blister: 0.9,
            },
            length: (0.13, 0.17),
            aspect: 0.86,
            petiole: 0.12,
            stalk: TWIG,
            stalk_width: (0.0045, 0.0035),
            green: [0.78, 0.9, 0.72],
            under_share: 0.16,
            underside: PALE_UNDERSIDE,
            turn: 0.0,
            tilt: 0.55,
        },
    }
}

/// The paler, greyer, matte underside of most leaves, and how far toward it they go.
const PALE_UNDERSIDE: ([f32; 3], f32) = ([0.95, 1.0, 0.88], 0.32);

/// A leaf's color: per-leaf brightness and hue, some older yellower leaves, some showing
/// their paler underside.
fn leaf_color(rng: &mut Rng, st: &BroadStyle, under: bool) -> [f32; 3] {
    let green = st.green;
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
        c = mix3(c, st.underside.0, st.underside.1);
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
            let under = rng.chance(st.under_share);
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
                width: length * st.aspect * rng.range(0.9, 1.1) * turned(rng, st),
                color: leaf_color(rng, st, under),
                under,
                tilt: Vec2::new(rng.range(-st.tilt, st.tilt), rng.range(-st.tilt, st.tilt)),
                phase: rng.range(0.0, 1.0),
                spots,
                seed: rng.next_u64(),
            });
            return;
        }
        length *= 0.82;
    }
}

/// How much narrower a leaf turned on its petiole looks (1 for kinds whose leaves lie flat).
fn turned(rng: &mut Rng, st: &BroadStyle) -> f32 {
    if st.turn > 0.0 {
        // Most leaves face the viewer more or less; a few are nearly edge-on.
        1.0 - st.turn * rng.range(0.0, 1.0).powi(2)
    } else {
        1.0
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

/// Twigs of a spray: each a path (canvas units) and its width at the base.
type Twigs = Vec<(Vec<Vec2>, f32)>;

/// A twig spray of broad leaves (oak, hazel, birch, beech, aspen, rowan), growing up from
/// the bottom center.
fn broad_spray(cv: &mut Canvas, kind: LeafKind, rng: &mut Rng) {
    let (leaves, twigs) = broad_spray_layout(kind, rng);
    let st = broad_style(kind);
    for (path, w) in &twigs {
        cv.stem_path(path, *w, *w * 0.45, TWIG);
    }
    // Petioles, then the leaves (in growth order: later ones overlap earlier ones).
    for l in &leaves {
        let at = l.base - l.dir * l.length * st.petiole;
        cv.stem(
            at,
            l.base + l.dir * l.length * 0.04,
            st.stalk_width.0,
            st.stalk_width.1,
            st.stalk,
            mix3(st.stalk, l.color, 0.5),
        );
    }
    for l in &leaves {
        paint_leaf(cv, kind, l);
    }
}

/// Paint one leaf of a layout: a blade, or for rowan a whole compound leaf.
fn paint_leaf(cv: &mut Canvas, kind: LeafKind, l: &Blade) {
    match kind {
        LeafKind::Pinnate => pinnate_leaf(cv, l),
        LeafKind::Palmate => palmate_leaf(cv, l),
        LeafKind::Broad
        | LeafKind::Small
        | LeafKind::Lobed
        | LeafKind::Oval
        | LeafKind::Round
        | LeafKind::LongNeedles
        | LeafKind::ShortNeedles
        | LeafKind::SoftNeedles
        | LeafKind::Frond
        | LeafKind::Palm => cv.blade(l),
    }
}

/// A compound leaf (rowan) in the frame of `l`: a reddish stalk from `l.base` along `l.dir`
/// with 5-8 opposite pairs of narrow leaflets and one at the end, the whole leaf `l.length`
/// long and about `l.width` wide. Its leaflets share `l`'s color, face and tilt, each a
/// little different; everything else follows from `l.seed`.
fn pinnate_leaf(cv: &mut Canvas, l: &Blade) {
    let mut rng = Rng::new(l.seed ^ 0x9177);
    let pairs = rng.range_i32(5, 9);
    let pinna = l.width * 0.55;
    let terminal = (pinna * 0.92).min(l.length * 0.3);
    // The stalk arches a little and runs to the end leaflet's base.
    let rachis = shoot(
        l.base,
        l.dir,
        l.length - terminal,
        rng.range(-0.22, 0.22),
        6,
    );
    let leaflet = |rng: &mut Rng, at: Vec2, dir: Vec2, length: f32| {
        let lum = rng.range(0.92, 1.07);
        let warm = rng.range(-0.04, 0.04);
        Blade {
            shape: l.shape,
            base: at,
            dir,
            length,
            width: length * PINNA_ASPECT * rng.range(0.88, 1.1),
            color: [
                l.color[0] * lum * (1.0 + warm),
                l.color[1] * lum,
                l.color[2] * lum * (1.0 - warm),
            ],
            under: l.under,
            tilt: l.tilt * 0.7 + Vec2::new(rng.range(-0.25, 0.25), rng.range(-0.25, 0.25)),
            phase: rng.range(0.0, 1.0),
            spots: if l.spots[0].2 > 0.0 && rng.chance(0.3) {
                l.spots
            } else {
                [(0.0, 0.0, 0.0); 2]
            },
            seed: rng.next_u64(),
        }
    };
    let mut leaflets = Vec::new();
    for k in 0..pairs {
        let t = 0.04 + 0.94 * k as f32 / pairs as f32;
        let (p, d) = along(&rachis, t);
        // Leaflets lean further forward toward the tip, and are a little shorter at both ends.
        let size = (0.8 + 0.2 * (PI * t).sin()) * rng.range(0.92, 1.06);
        for side in [-1.0f32, 1.0] {
            let a = side * rng.range(1.05, 1.3) * (1.0 - 0.22 * t);
            let dir = rot(d, a);
            leaflets.push(leaflet(
                &mut rng,
                p + dir * 0.002 * l.length,
                dir,
                pinna * size,
            ));
        }
    }
    let (end, d) = along(&rachis, 1.0);
    leaflets.push(leaflet(&mut rng, end, d, terminal));
    for b in &leaflets {
        cv.blade(b);
    }
    // The stalk on top of the leaflets' bases.
    let w = l.length * 0.018;
    cv.stem_path(&rachis, w, w * 0.45, RACHIS);
}

/// A palmate leaf (sugar maple) in the frame of `l`: from the top of the stalk at `l.base`,
/// a middle lobe along `l.dir` the leaf's whole `l.length`, two side lobes spreading at about
/// 50° and two small basal ones nearly sideways, each a pointed, coarsely toothed blade with
/// its own main vein (palmate venation); a rounded heart joins them, leaving round sinuses
/// between the lobes. The lobes share `l`'s color, face and tilt, each a little different.
fn palmate_leaf(cv: &mut Canvas, l: &Blade) {
    let mut rng = Rng::new(l.seed ^ 0x3a91);
    let spread = l.width / l.length;
    let lobe = |rng: &mut Rng, angle: f32, length: f32| {
        let lum = rng.range(0.95, 1.05);
        Blade {
            shape: l.shape,
            base: l.base,
            dir: rot(l.dir, angle),
            length: l.length * length,
            width: l.length * length * rng.range(0.5, 0.58),
            color: [l.color[0] * lum, l.color[1] * lum, l.color[2] * lum],
            under: l.under,
            tilt: l.tilt + Vec2::new(rng.range(-0.15, 0.15), rng.range(-0.15, 0.15)),
            phase: rng.range(0.0, 1.0),
            spots: if rng.chance(0.2) {
                l.spots
            } else {
                [(0.0, 0.0, 0.0); 2]
            },
            seed: rng.next_u64(),
        }
    };
    let side = 0.82 * spread.clamp(0.8, 1.2) + rng.range(-0.06, 0.06);
    // Basal lobes first (under), then the side lobes, the heart, and the middle lobe on top.
    let mut blades = vec![
        lobe(&mut rng, -1.75, 0.4),
        lobe(&mut rng, 1.75, 0.4),
        lobe(&mut rng, -side, 0.78),
        lobe(&mut rng, side, 0.78),
    ];
    const HEART: LeafShape = LeafShape {
        a: 0.5,
        b: 0.6,
        tip: 0.0,
        teeth: 0.0,
        tooth_depth: 0.0,
        fine_teeth: 0.0,
        crenate: 0.0,
        teeth_from: 0.1,
        teeth_at_veins: false,
        lobes: 0.0,
        lobe_depth: 0.0,
        auricle: 0.0,
        veins: 0.0,
        vein_sweep: 0.0,
        vein_curve: 1.0,
        fold: 0.1,
        blister: 0.2,
    };
    let mut heart = lobe(&mut rng, 0.0, 0.42);
    heart.shape = HEART;
    heart.base = l.base - l.dir * l.length * 0.06;
    heart.width = l.length * 0.62;
    blades.push(heart);
    let lean = rng.range(-0.06, 0.06);
    blades.push(lobe(&mut rng, lean, 1.0));
    for b in &blades {
        cv.blade(b);
    }
}

/// A zigzag shoot (beech): it turns a little at every node, alternately one way and the other,
/// while bending by `bend` overall. The nodes are its points.
fn zigzag(start: Vec2, dir: Vec2, length: f32, bend: f32, segs: usize, zig: f32) -> Vec<Vec2> {
    let mut pts = vec![start];
    for k in 0..segs {
        let f = (k as f32 + 0.5) / segs as f32;
        let z = if k % 2 == 0 { zig } else { -zig };
        let d = rot(dir, bend * f + z);
        pts.push(*pts.last().unwrap() + d * length / segs as f32);
    }
    pts
}

/// Where the leaves and twigs of a broadleaf spray go (see [`broad_spray`]).
/// A broadleaf card's spray: a branchlet of two shoots (a leading one and a side one), each a
/// [`one_spray_layout`] at about 0.6 size. Leaves drawn one spray to a card came out
/// twice their real size (an oak leaf 17-24 cm, a beech leaf 17-22 cm, against 7-14 and 5-10
/// cm in the field), which made crowns read as a few big flat blobs; compound leaves (rowan,
/// ash) are big for real and keep one spray.
fn broad_spray_layout(kind: LeafKind, rng: &mut Rng) -> (Vec<Blade>, Twigs) {
    if kind == LeafKind::Pinnate {
        return one_spray_layout(kind, rng);
    }
    let base = Vec2::new(0.5, 0.995);
    let up = Vec2::NEG_Y;
    let main = shoot(
        base,
        rot(up, rng.range(-0.08, 0.08)),
        rng.range(0.34, 0.4),
        rng.range(-0.2, 0.2),
        4,
    );
    let (fork, lead_dir) = along(&main, 1.0);
    let mut twigs: Twigs = vec![(main.clone(), 0.016)];
    let mut leaves = Vec::new();
    // The side shoot first (painted under the leading one), then the leader.
    let side = if rng.chance(0.5) { 1.0 } else { -1.0 };
    let sprays = [
        (
            along(&main, rng.range(0.5, 0.7)),
            side * rng.range(0.55, 0.8),
            0.55,
        ),
        (
            (fork, rot(lead_dir, -side * 0.15)),
            rng.range(-0.12, 0.12),
            0.62,
        ),
    ];
    for ((at, d), turn, k) in sprays {
        let (blades, sub) = one_spray_layout(kind, rng);
        let dir = rot(d, turn);
        // `one_spray_layout` grows from `base` toward `up`: turn that onto `dir` at `at`.
        let a = up.y.atan2(up.x);
        let b = dir.y.atan2(dir.x);
        let place = |q: Vec2| at + rot(q - base, b - a) * k;
        for mut l in blades {
            l.base = place(l.base);
            l.dir = rot(l.dir, b - a);
            l.length *= k;
            l.width *= k;
            leaves.push(l);
        }
        for (path, w) in sub {
            twigs.push((path.into_iter().map(place).collect(), w * (0.4 + 0.6 * k)));
        }
    }
    (leaves, twigs)
}

/// One shoot of a broadleaf spray filling its card: the twig and its leaves.
fn one_spray_layout(kind: LeafKind, rng: &mut Rng) -> (Vec<Blade>, Twigs) {
    let st = broad_style(kind);
    let base = Vec2::new(0.5, 0.995);
    let up = Vec2::NEG_Y;
    let mut leaves = Vec::new();
    let mut twigs: Twigs = Vec::new();
    match kind {
        LeafKind::Lobed | LeafKind::Palmate => {
            // Oak: a twig that forks into shoots; leaves crowd at every shoot tip. (Maple's
            // long-stalked leaves crowd the same way.)
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
        LeafKind::Oval => {
            // Beech (and goat willow): zigzag shoots turning at every leaf, the leaves
            // alternating in two flat rows nearly square to the shoot, each on the outside of
            // its bend, so a spray is a flat plate of leaves.
            let up_dir = rot(up, rng.range(-0.1, 0.1));
            let main = zigzag(
                base,
                up_dir,
                rng.range(0.82, 0.9),
                rng.range(-0.3, 0.3),
                8,
                0.17,
            );
            let mut shoots = vec![(main.clone(), 0.011)];
            let flip = if rng.chance(0.5) { 1.0 } else { -1.0 };
            for (k, node) in [2usize, 3, 5, 6].iter().enumerate() {
                let p = main[*node];
                let d = (main[node + 1] - main[node - 1]).normalize_or(up);
                let sgn = flip * if k % 2 == 0 { 1.0 } else { -1.0 };
                let len = rng.range(0.34, 0.44) * (1.0 - 0.18 * k as f32);
                let segs = 6 - k / 2;
                shoots.push((
                    zigzag(
                        p,
                        rot(d, sgn * rng.range(0.7, 0.95)),
                        len,
                        -sgn * 0.3,
                        segs,
                        0.2,
                    ),
                    0.007,
                ));
            }
            for (s, w) in &shoots {
                let n = s.len() - 1;
                for j in 1..n {
                    let d = (s[j + 1] - s[j - 1]).normalize_or(up);
                    // The shoot turns away from each leaf.
                    let sgn = if j % 2 == 0 { -1.0 } else { 1.0 };
                    let t = j as f32 / n as f32;
                    let ld = rot(d, sgn * rng.range(0.95, 1.3));
                    push_leaf(&mut leaves, rng, &st, s[j], ld, 1.0 - 0.3 * t);
                }
                let (tip, d) = along(s, 1.0);
                push_leaf(&mut leaves, rng, &st, tip, d, 0.7);
                twigs.push((s.clone(), *w));
            }
        }
        LeafKind::Round => {
            // Aspen, alder: a shoot with two side shoots, the leaves on long petioles turned
            // every which way (some hanging back past their twig), crowding at the tips.
            let main = shoot(
                base,
                rot(up, rng.range(-0.15, 0.15)),
                rng.range(0.6, 0.72),
                rng.range(-0.3, 0.3),
                6,
            );
            let mut shoots = vec![(main.clone(), 0.011)];
            let flip = if rng.chance(0.5) { 1.0 } else { -1.0 };
            for (k, t) in [0.35f32, 0.6].iter().enumerate() {
                let (p, d) = along(&main, *t);
                let sgn = flip * if k % 2 == 0 { 1.0 } else { -1.0 };
                shoots.push((
                    shoot(
                        p,
                        rot(d, sgn * rng.range(0.6, 0.95)),
                        rng.range(0.3, 0.4),
                        -sgn * 0.2,
                        4,
                    ),
                    0.007,
                ));
            }
            for (s, w) in &shoots {
                let steps = 6;
                for j in 1..=steps {
                    let t = j as f32 / steps as f32 * 0.78 + 0.06;
                    let (p, d) = along(s, t);
                    let sgn = if j % 2 == 0 { 1.0 } else { -1.0 };
                    let ld = rot(d, sgn * rng.range(0.5, 2.3));
                    let size = rng.range(0.8, 1.05);
                    push_leaf(&mut leaves, rng, &st, p, ld, size);
                }
                let (tip, d) = along(s, 1.0);
                let k = rng.range_i32(4, 6);
                for j in 0..k {
                    let a = (j as f32 / (k - 1) as f32 - 0.5) * rng.range(2.4, 3.2)
                        + rng.range(-0.2, 0.2);
                    let size = rng.range(0.75, 1.0);
                    push_leaf(&mut leaves, rng, &st, tip, rot(d, a), size);
                }
                twigs.push((s.clone(), *w));
            }
        }
        LeafKind::Pinnate => {
            // Rowan: a stout twig whose tip and short side shoots each hold a fan of compound
            // leaves; the leader's fan on top.
            let main = shoot(
                base,
                rot(up, rng.range(-0.12, 0.12)),
                rng.range(0.4, 0.5),
                rng.range(-0.25, 0.25),
                4,
            );
            let mut tips = vec![along(&main, 1.0)];
            let flip = if rng.chance(0.5) { 1.0 } else { -1.0 };
            for (k, t) in [0.4f32, 0.7].iter().enumerate() {
                let (p, d) = along(&main, *t);
                let sgn = flip * if k % 2 == 0 { 1.0 } else { -1.0 };
                let s = shoot(
                    p,
                    rot(d, sgn * rng.range(0.8, 1.1)),
                    rng.range(0.16, 0.22),
                    -sgn * 0.3,
                    3,
                );
                tips.push(along(&s, 1.0));
                twigs.push((s, 0.009));
            }
            twigs.push((main, 0.014));
            for (i, (tip, d)) in tips.iter().enumerate().rev() {
                let k = if i == 0 {
                    rng.range_i32(3, 6)
                } else {
                    rng.range_i32(2, 4)
                };
                for j in 0..k {
                    let a = (j as f32 / (k - 1) as f32 - 0.5) * rng.range(2.0, 2.7)
                        + rng.range(-0.15, 0.15);
                    let size = rng.range(0.85, 1.05);
                    push_leaf(&mut leaves, rng, &st, *tip, rot(*d, a), size);
                }
            }
        }
        // Needles and fronds are not painted as broadleaf sprays.
        LeafKind::LongNeedles
        | LeafKind::ShortNeedles
        | LeafKind::SoftNeedles
        | LeafKind::Frond
        | LeafKind::Palm => {}
        LeafKind::Broad => {
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
    (leaves, twigs)
}

/// A cluster of shoot tips seen from outside the crown: rosettes of leaves filling a disc.
fn broad_cluster(cv: &mut Canvas, kind: LeafKind, rng: &mut Rng) {
    let leaves = broad_cluster_layout(kind, rng);
    let st = broad_style(kind);
    // Long stalks show between the rosettes' leaves (aspen's petioles, rowan's leaf stalks).
    let stalks = match kind {
        LeafKind::Round | LeafKind::Pinnate | LeafKind::Palmate => true,
        LeafKind::Broad
        | LeafKind::Small
        | LeafKind::Lobed
        | LeafKind::Oval
        | LeafKind::LongNeedles
        | LeafKind::ShortNeedles
        | LeafKind::SoftNeedles
        | LeafKind::Frond
        | LeafKind::Palm => false,
    };
    if stalks {
        for l in &leaves {
            cv.stem(
                l.base - l.dir * l.length * st.petiole,
                l.base + l.dir * l.length * 0.04,
                st.stalk_width.0,
                st.stalk_width.1,
                st.stalk,
                mix3(st.stalk, l.color, 0.5),
            );
        }
    }
    for l in &leaves {
        paint_leaf(cv, kind, l);
    }
}

/// Where the leaves of a cluster go (see [`broad_cluster`]).
fn broad_cluster_layout(kind: LeafKind, rng: &mut Rng) -> Vec<Blade> {
    let st = broad_style(kind);
    let center = Vec2::splat(0.5);
    let mut leaves = Vec::new();
    // Rosettes, leaves in each (and one more in the middle one), their size, and how far the
    // outer rosettes sit from the middle.
    let (rosettes, per, middle, size, reach) = match kind {
        LeafKind::Small => (7, 6, 0, 1.1, (0.16, 0.24)),
        LeafKind::Broad | LeafKind::Lobed | LeafKind::Palmate => (5, 6, 1, 0.95, (0.16, 0.24)),
        // Beech: smaller leaves in more, flatter rosettes.
        LeafKind::Oval => (6, 6, 1, 1.0, (0.16, 0.24)),
        // Aspen: long petioles spread each rosette wide.
        LeafKind::Round => (6, 5, 1, 0.95, (0.17, 0.25)),
        // Rowan: a few big rosettes of compound leaves, foreshortened toward the viewer.
        LeafKind::Pinnate => (4, 5, 1, 0.78, (0.12, 0.18)),
        // Needles and fronds have no leaf rosettes.
        LeafKind::LongNeedles
        | LeafKind::ShortNeedles
        | LeafKind::SoftNeedles
        | LeafKind::Frond
        | LeafKind::Palm => {
            return leaves;
        }
    };
    let mut centers = vec![center];
    for k in 1..rosettes {
        let a = k as f32 * 2.4 + rng.range(-0.3, 0.3);
        centers.push(center + Vec2::new(a.cos(), a.sin()) * rng.range(reach.0, reach.1));
    }
    for (i, c) in centers.iter().enumerate().rev() {
        let k = per + middle * (i == 0) as i32;
        let a0 = rng.range(0.0, TAU);
        for j in 0..k {
            let a = a0 + j as f32 / k as f32 * TAU + rng.range(-0.25, 0.25);
            push_leaf(&mut leaves, rng, &st, *c, Vec2::new(a.cos(), a.sin()), size);
        }
    }
    leaves
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

const LARCH_NEEDLE: [f32; 3] = [0.8, 0.94, 0.68];

/// How long a larch needle is on a spray card (canvas units): a card spans about a meter of
/// branch, and a European larch's needles are 2-4 cm; drawn about twice as long (as spruce's
/// are) so the rosettes read as soft tufts covering the twigs rather than specks on a pole.
const LARCH_NEEDLE_LENGTH: f32 = 0.058;

/// How wide a larch needle is painted (canvas units): softer and finer than spruce's.
const LARCH_NEEDLE_WIDTH: f32 = 0.0027;

/// A larch spur shoot at `at`: 25-40 soft needles splaying from a knob, around `dir` and up to
/// `spread` radians either side of it (π: a rosette seen face-on, splayed all round).
fn larch_rosette(cv: &mut Canvas, rng: &mut Rng, at: Vec2, dir: Vec2, spread: f32) {
    let n = rng.range_i32(25, 41);
    let fresh = rng.range(0.1, 0.6);
    for _ in 0..n {
        let nd = rot(dir, rng.range(-spread, spread));
        // Needles pointing toward or away from the viewer look shorter.
        let l = LARCH_NEEDLE_LENGTH * rng.range(0.65, 1.15) * rng.range(0.4, 1.0);
        let bend = rot(nd, rng.range(-0.3, 0.3));
        let c = needle_color(rng, LARCH_NEEDLE, fresh);
        let p = at + nd * rng.range(0.0, 0.003);
        let Some(k) = cv.needle_width(LARCH_NEEDLE_WIDTH, l) else {
            continue;
        };
        let (w, mid) = (LARCH_NEEDLE_WIDTH * k, p + nd * l * 0.5);
        cv.stem(p, mid, w, w * 0.9, scale3(c, 0.8), c);
        cv.stem(mid, mid + bend * l * 0.5, w * 0.9, w * 0.4, c, c);
    }
    // The spur: a dark knob the needles spring from.
    cv.stem(
        at - dir * 0.003,
        at + dir * 0.002,
        0.0075,
        0.0065,
        [0.3, 0.24, 0.18],
        [0.38, 0.3, 0.22],
    );
}

/// Larch spur shoots along `path` from `t0` to `t1`, about `step` (canvas units) apart on
/// alternating sides: most rosettes splay out sideways, some face the viewer.
fn larch_spurs(cv: &mut Canvas, rng: &mut Rng, path: &[Vec2], t0: f32, t1: f32, step: f32) {
    let total = path_length(path);
    let n = ((total * (t1 - t0)) / step).round().max(1.0) as usize;
    for i in 0..n {
        let t = t0 + (t1 - t0) * (i as f32 + rng.range(0.2, 0.8)) / n as f32;
        let (p, d) = along(path, t);
        let sgn = if i % 2 == 0 { 1.0 } else { -1.0 };
        let out = rot(d, sgn * rng.range(0.5, 1.1));
        let spread = if rng.chance(0.3) {
            PI
        } else {
            rng.range(0.9, 1.4)
        };
        let at = p + out * rng.range(0.003, 0.007);
        larch_rosette(cv, rng, at, out, spread);
    }
}

/// Single needles spiralling round the last stretch of a young long shoot, from `t0` to its
/// tip.
fn larch_long_shoot(cv: &mut Canvas, rng: &mut Rng, path: &[Vec2], t0: f32) {
    needle_brush(
        cv,
        rng,
        path,
        t0,
        LARCH_NEEDLE_LENGTH * 0.75,
        (0.45, 1.0),
        LARCH_NEEDLE_WIDTH,
        0.0085,
        LARCH_NEEDLE,
        0.5,
    );
}

/// Larch: a branch end of thin twigs drooping from a leader, spur shoots with their rosettes
/// of needles spaced along the older wood and single needles on the young tips; far more
/// sky shows through than through a spruce spray.
fn larch_spray(cv: &mut Canvas, rng: &mut Rng) {
    let base = Vec2::new(0.5, 0.995);
    let main = shoot(
        base,
        rot(Vec2::NEG_Y, rng.range(-0.12, 0.12)),
        rng.range(0.82, 0.9),
        rng.range(-0.3, 0.3),
        8,
    );
    let mut shoots: Vec<Vec<Vec2>> = Vec::new();
    let n = rng.range_i32(7, 10);
    let flip = if rng.chance(0.5) { 1.0 } else { -1.0 };
    for k in 0..n {
        let t = 0.1 + 0.74 * (k as f32 + rng.range(0.0, 0.5)) / n as f32;
        let (p, d) = along(&main, t);
        let sgn = flip * if k % 2 == 0 { 1.0 } else { -1.0 };
        let l = rng.range(0.3, 0.42) * (1.0 - 0.35 * t);
        // Side twigs leave the leader at a wide angle and droop, hanging like a curtain, with
        // a fine branchlet or two hanging from them.
        let s = shoot(
            p,
            rot(d, sgn * rng.range(0.6, 0.95)),
            l,
            sgn * rng.range(1.0, 1.6),
            6,
        );
        for _ in 0..rng.range_i32(1, 4) {
            let (q, e) = along(&s, rng.range(0.25, 0.7));
            let b = shoot(q, rot(e, sgn * rng.range(0.6, 1.0)), l * 0.4, sgn * 0.5, 3);
            shoots.push(b);
        }
        shoots.push(s);
    }
    cv.stem_path(&main, 0.011, 0.004, TWIG);
    for s in &shoots {
        cv.stem_path(s, 0.0055, 0.0025, TWIG);
    }
    shoots.push(main);
    for s in &shoots {
        // The last 8 cm or so grew this year: single needles; spurs on the wood behind.
        let young = (1.0 - 0.08 / path_length(s).max(1e-3)).clamp(0.4, 0.9);
        larch_spurs(cv, rng, s, 0.06, young, 0.023);
        larch_long_shoot(cv, rng, s, young);
    }
}

/// A larch branch end seen from its tip: twigs radiating from the middle with spurs along
/// them, rosettes seen face-on between them and one in the middle.
fn larch_tuft(cv: &mut Canvas, rng: &mut Rng) {
    let c = Vec2::splat(0.5);
    let k = rng.range_i32(7, 11);
    let a0 = rng.range(0.0, TAU);
    let shoots: Vec<_> = (0..k)
        .map(|i| {
            let a = a0 + i as f32 / k as f32 * TAU + rng.range(-0.25, 0.25);
            shoot(
                c,
                Vec2::new(a.cos(), a.sin()),
                rng.range(0.26, 0.38),
                rng.range(-0.4, 0.4),
                4,
            )
        })
        .collect();
    for s in &shoots {
        cv.stem_path(s, 0.007, 0.003, TWIG);
    }
    for s in &shoots {
        larch_spurs(cv, rng, s, 0.12, 0.8, 0.022);
        larch_long_shoot(cv, rng, s, 0.8);
    }
    for _ in 0..rng.range_i32(14, 22) {
        let a = rng.range(0.0, TAU);
        let at = c + Vec2::new(a.cos(), a.sin()) * rng.range(0.08, 0.3);
        larch_rosette(cv, rng, at, Vec2::new(a.cos(), a.sin()), PI);
    }
    larch_rosette(cv, rng, c, Vec2::NEG_Y, PI);
}

/// Paint one cell of an atlas (or the whole frond image) on its own canvas.
fn paint_cell(kind: LeafKind, cell: usize, size: u32, seed: u64, min_needle_px: f32) -> Canvas {
    let mut cv = Canvas::new(size);
    cv.min_needle_px = min_needle_px;
    let mut rng = cell_rng(seed, cell);
    match kind {
        LeafKind::Broad
        | LeafKind::Small
        | LeafKind::Lobed
        | LeafKind::Oval
        | LeafKind::Round
        | LeafKind::Pinnate
        | LeafKind::Palmate => {
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
        LeafKind::SoftNeedles => {
            if cell == CLUSTER_CELL {
                larch_tuft(&mut cv, &mut rng);
            } else {
                larch_spray(&mut cv, &mut rng);
            }
        }
        LeafKind::Palm => palm_segment(&mut cv, &mut rng, cell),
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

/// Palm leaflets: linear, widest just above the base, tapering to a long point, untoothed,
/// with one strong midrib and a sharp fold along it.
const PALM_LEAFLET: LeafShape = LeafShape {
    a: 0.12,
    b: 0.55,
    tip: 0.75,
    teeth: 0.0,
    tooth_depth: 0.0,
    fine_teeth: 0.0,
    crenate: 0.0,
    teeth_from: 0.1,
    teeth_at_veins: false,
    lobes: 0.0,
    lobe_depth: 0.0,
    auricle: 0.0,
    veins: 0.0,
    vein_sweep: 0.0,
    vein_curve: 1.0,
    fold: 0.45,
    blister: 0.0,
};

/// Palm frond midrib: pale yellow-green.
const PALM_RACHIS: [f32; 3] = [0.78, 0.8, 0.52];

/// One stretch of a palm frond ([`LeafKind::Palm`]): the midrib from the bottom of the cell to
/// its top with leaflets close-set along both sides, each leaving it at 35-55° forward and
/// running out toward the cell's side. Cell 0 is the frond's foot, cell 3 its tip; the others
/// are middles. Now and then a leaflet is missing or torn short (wind-ragged fronds), and the
/// odd one is yellowing at its tip.
fn palm_segment(cv: &mut Canvas, rng: &mut Rng, cell: usize) {
    let (foot, tip) = (cell == 0, cell == 3);
    let rachis = shoot(
        Vec2::new(0.5, 1.0),
        rot(Vec2::NEG_Y, rng.range(-0.03, 0.03)),
        if tip { 0.92 } else { 1.0 },
        rng.range(-0.06, 0.06),
        8,
    );
    // The midrib's own width tapers along the frond: wide at the foot, thin at the tip.
    let (w0, w1) = match (foot, tip) {
        (true, _) => (0.05, 0.034),
        (_, true) => (0.022, 0.006),
        _ => (0.034, 0.022),
    };
    let per_side = 30;
    let mut blades = Vec::new();
    for k in 0..per_side {
        for side in [-1.0f32, 1.0] {
            let t = (k as f32 + rng.range(0.0, 0.7) + if side > 0.0 { 0.0 } else { 0.35 })
                / per_side as f32;
            if foot && t < 0.18 {
                continue;
            }
            let t = t.min(0.995);
            let (p, d) = along(&rachis, t);
            // Leaflets lengthen out of the foot and shorten toward the tip.
            let mut len = if foot {
                0.3 + 0.32 * smoothstep(0.18, 0.75, t)
            } else if tip {
                0.6 * (1.0 - t).powf(0.7) + 0.06
            } else {
                0.62
            } * rng.range(0.88, 1.08);
            if rng.chance(0.05) {
                continue;
            }
            if rng.chance(0.07) {
                len *= rng.range(0.35, 0.7);
            }
            let a = side * rng.range(0.62, 0.95);
            let dir = rot(d, a);
            let lum = rng.range(0.86, 1.08);
            let warm = rng.range(-0.03, 0.06);
            let base = [0.74, 0.86, 0.6];
            blades.push(Blade {
                shape: PALM_LEAFLET,
                base: p + dir * w0 * 0.3,
                dir,
                length: len,
                width: len * rng.range(0.05, 0.065),
                color: [
                    base[0] * lum * (1.0 + warm),
                    base[1] * lum,
                    base[2] * lum * (1.0 - warm * 2.0),
                ],
                under: rng.chance(0.25),
                tilt: Vec2::new(rng.range(-0.3, 0.3), rng.range(-0.3, 0.3)),
                phase: rng.range(0.0, 1.0),
                spots: if rng.chance(0.12) {
                    [
                        (rng.range(0.8, 0.95), 0.0, rng.range(0.5, 0.9)),
                        (0.0, 0.0, 0.0),
                    ]
                } else {
                    [(0.0, 0.0, 0.0); 2]
                },
                seed: rng.next_u64(),
            });
        }
    }
    for b in &blades {
        cv.blade(b);
    }
    cv.stem_path(&rachis, w0, w1, PALM_RACHIS);
}

fn cell_rng(seed: u64, cell: usize) -> Rng {
    Rng::new(seed ^ (cell as u64).wrapping_mul(0x9e37_79b9_7f4a_7c15))
}

/// One leaf of a painted spray (canvas units of its atlas cell, y down).
#[derive(Debug, Clone, Copy)]
pub struct SprayLeaf {
    /// Where its petiole leaves the twig.
    pub at: Vec2,
    /// Base of the blade.
    pub base: Vec2,
    /// Unit direction from base to tip.
    pub dir: Vec2,
    /// Blade length.
    pub length: f32,
    /// Shows its paler underside.
    pub under: bool,
    /// For picking a variant.
    pub seed: u64,
}

/// The layout of a broadleaf atlas cell: the leaves (bottom first, as painted) and twigs.
#[derive(Debug, Clone, Default)]
pub struct SprayLayout {
    pub leaves: Vec<SprayLeaf>,
    /// Twig paths (canvas units) and their widths at the base.
    pub twigs: Vec<(Vec<Vec2>, f32)>,
}

/// The layout of atlas cell `cell` of a broadleaf atlas painted with `seed`
/// ([`leaf_textures`]), for building the same spray from single leaves
/// ([`hero_leaf_textures`]). `None` for needles and fronds.
pub fn spray_layout(kind: LeafKind, cell: usize, seed: u64) -> Option<SprayLayout> {
    match kind {
        LeafKind::Broad
        | LeafKind::Small
        | LeafKind::Lobed
        | LeafKind::Oval
        | LeafKind::Round
        | LeafKind::Pinnate
        | LeafKind::Palmate => {}
        LeafKind::LongNeedles
        | LeafKind::ShortNeedles
        | LeafKind::SoftNeedles
        | LeafKind::Frond
        | LeafKind::Palm => {
            return None;
        }
    }
    let mut rng = cell_rng(seed, cell % (ATLAS_CELLS * ATLAS_CELLS) as usize);
    let (blades, twigs) = if cell % (ATLAS_CELLS * ATLAS_CELLS) as usize == CLUSTER_CELL {
        (broad_cluster_layout(kind, &mut rng), Vec::new())
    } else {
        broad_spray_layout(kind, &mut rng)
    };
    let petiole = broad_style(kind).petiole;
    let leaves = blades
        .iter()
        .map(|l| SprayLeaf {
            at: l.base - l.dir * l.length * petiole,
            base: l.base,
            dir: l.dir,
            length: l.length,
            under: l.under,
            seed: l.seed,
        })
        .collect();
    Some(SprayLayout { leaves, twigs })
}

/// Cells per side of a single-leaf atlas ([`hero_leaf_textures`]).
pub const HERO_CELLS: u32 = 4;

/// The single-leaf atlas cell painted solid twig color, for twigs drawn as narrow strips.
pub const HERO_TWIG_CELL: usize = (HERO_CELLS * HERO_CELLS) as usize - 1;

/// Texels per side of a single-leaf atlas (256 per leaf: a few texels a millimeter).
pub const HERO_ATLAS_SIZE: u32 = 1024;

/// UV rectangle (min, max) of cell `cell` of a single-leaf atlas, inset by half a texel.
pub fn hero_cell(cell: usize) -> (Vec2, Vec2) {
    let n = HERO_CELLS as usize;
    let c = cell % (n * n);
    let size = 1.0 / n as f32;
    let min = Vec2::new((c % n) as f32 * size, (c / n) as f32 * size);
    let inset = Vec2::splat(0.5 / HERO_ATLAS_SIZE as f32);
    (min + inset, min + Vec2::splat(size) - inset)
}

/// Where a single-leaf atlas cell holds its leaf (cell units, y down): the petiole starts at
/// the bottom center (0.5, [`HERO_PETIOLE_Y`]) and the blade runs straight up, starting at
/// `base_y` and `length` long.
pub fn hero_leaf_frame(kind: LeafKind) -> (f32, f32) {
    let st = broad_style(kind);
    // Long enough to fill the cell, narrow enough that lobes and teeth stay inside it.
    let length = (0.94 / (1.0 + st.petiole)).min(0.62 / (st.aspect * 1.1));
    (HERO_PETIOLE_Y - length * st.petiole, length)
}

/// Height (cell units, y down) where a single leaf's petiole starts.
pub const HERO_PETIOLE_Y: f32 = 0.98;

/// The part of a single-leaf cell its leaf covers (min, max; cell units, y down).
pub fn hero_leaf_rect(kind: LeafKind) -> (Vec2, Vec2) {
    let st = broad_style(kind);
    let (base_y, length) = hero_leaf_frame(kind);
    let half = (0.75 * length * st.aspect * 1.08 + 0.01).min(0.5);
    (
        Vec2::new(0.5 - half, (base_y - length * 1.03).max(0.0)),
        Vec2::new(0.5 + half, (HERO_PETIOLE_Y + 0.015).min(1.0)),
    )
}

/// The seed a leaf style's atlas is painted with (so meshes can follow its layout).
pub fn atlas_seed(style: crate::species::LeafStyle) -> u64 {
    0x1eaf ^ style as u64
}

/// An atlas of single leaves, [`HERO_CELLS`]² cells, each one leaf of the kind's spray
/// painting (shape, color, spots) upright on its petiole ([`hero_leaf_frame`]); the last
/// cell is solid twig. For crowns built leaf by leaf up close. A rowan "leaf" is a whole
/// compound leaf, as in its sprays' layouts ([`spray_layout`]).
pub fn hero_leaf_textures(kind: LeafKind, size: u32, seed: u64) -> LeafTextures {
    let n = HERO_CELLS;
    let cs = (size / n).max(1);
    let st = broad_style(kind);
    let (base_y, length) = hero_leaf_frame(kind);
    let cells: Vec<Canvas> = std::thread::scope(|scope| {
        let st = &st;
        let handles: Vec<_> = (0..(n * n) as usize)
            .map(|c| {
                scope.spawn(move || {
                    let mut cv = Canvas::new(cs);
                    let mut rng = cell_rng(seed ^ 0x4e70, c);
                    if c == HERO_TWIG_CELL {
                        cv.line(Vec2::new(0.5, 1.0), Vec2::new(0.5, 0.0), 0.9, 0.9, TWIG);
                        return cv;
                    }
                    // Every n-th leaf shows its underside, as often as in the sprays.
                    let every = (1.0 / st.under_share).round().max(1.0) as usize;
                    let under = c % every == every - 1;
                    let blade = Blade {
                        shape: st.shape,
                        base: Vec2::new(0.5, base_y),
                        dir: Vec2::NEG_Y,
                        length,
                        width: length * st.aspect * rng.range(0.92, 1.08),
                        color: leaf_color(&mut rng, st, under),
                        under,
                        tilt: Vec2::ZERO,
                        phase: rng.range(0.0, 1.0),
                        spots: if c % 7 == 3 {
                            [
                                (rng.range(0.3, 0.8), rng.range(-0.6, 0.6), 0.05),
                                (rng.range(0.3, 0.8), rng.range(-0.6, 0.6), 0.03),
                            ]
                        } else {
                            [(0.0, 0.0, 0.0); 2]
                        },
                        seed: rng.next_u64(),
                    };
                    let (w0, w1) = match kind {
                        // Running on into the compound leaf's stalk (see `pinnate_leaf`).
                        LeafKind::Pinnate => (length * 0.024, length * 0.019),
                        // Long and thin.
                        LeafKind::Round | LeafKind::Palmate => (0.014, 0.011),
                        LeafKind::Broad
                        | LeafKind::Small
                        | LeafKind::Lobed
                        | LeafKind::Oval
                        | LeafKind::LongNeedles
                        | LeafKind::ShortNeedles
                        | LeafKind::SoftNeedles
                        | LeafKind::Frond
                        | LeafKind::Palm => (0.02, 0.015),
                    };
                    cv.stem(
                        Vec2::new(0.5, HERO_PETIOLE_Y),
                        blade.base + blade.dir * length * 0.04,
                        w0,
                        w1,
                        st.stalk,
                        mix3(st.stalk, blade.color, 0.5),
                    );
                    paint_leaf(&mut cv, kind, &blade);
                    cv
                })
            })
            .collect();
        handles
            .into_iter()
            .map(|h| h.join().expect("leaf painter"))
            .collect()
    });
    let mut atlas = Canvas::new(cs * n);
    for (c, cell) in cells.iter().enumerate() {
        let (ox, oy) = ((c as u32 % n) * cs, (c as u32 / n) * cs);
        for y in 0..cs {
            let src = (y * cs) as usize..((y + 1) * cs) as usize;
            let dst = ((oy + y) * cs * n + ox) as usize..((oy + y) * cs * n + ox + cs) as usize;
            atlas.px[dst.clone()].copy_from_slice(&cell.px[src.clone()]);
            atlas.nrm[dst].copy_from_slice(&cell.nrm[src]);
        }
    }
    atlas.into_textures()
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

/// Texels per side of a mass atlas ([`mass_textures`]): far cards are a few dozen pixels on
/// screen.
pub const MASS_ATLAS_SIZE: u32 = 512;

/// Paint the atlas far and distant crowns draw with: each cell's spray (the same layout as
/// [`leaf_textures`], so the cards keep their UVs) in front of two darker sprays from deeper
/// in the crown, over a dark lumpy mass of shaded leaves rounded at its edges. A far crown
/// keeps only a third of the cards, so each must read as a clump of foliage rather than one
/// twig with sky showing through it.
pub fn mass_textures(kind: LeafKind, size: u32, seed: u64) -> LeafTextures {
    let n = ATLAS_CELLS;
    let cs = (size / n).max(1);
    // Needles are painted at least a pixel wide (see `Canvas::needle_width`).
    let min_px = if kind.is_needles() { 1.0 } else { 0.0 };
    let cells: Vec<Canvas> = std::thread::scope(|scope| {
        let handles: Vec<_> = (0..(n * n) as usize)
            .map(|c| scope.spawn(move || mass_cell(kind, c, cs, seed, min_px)))
            .collect();
        handles
            .into_iter()
            .map(|h| h.join().expect("leaf painter"))
            .collect()
    });
    let mut atlas = Canvas::new(cs * n);
    for (c, cell) in cells.iter().enumerate() {
        let (ox, oy) = ((c as u32 % n) * cs, (c as u32 / n) * cs);
        for y in 0..cs {
            let src = (y * cs) as usize..((y + 1) * cs) as usize;
            let dst = ((oy + y) * cs * n + ox) as usize..((oy + y) * cs * n + ox + cs) as usize;
            atlas.px[dst.clone()].copy_from_slice(&cell.px[src.clone()]);
            atlas.nrm[dst].copy_from_slice(&cell.nrm[src]);
        }
    }
    atlas.into_textures()
}

/// One cell of [`mass_textures`].
fn mass_cell(kind: LeafKind, cell: usize, cs: u32, seed: u64, min_px: f32) -> Canvas {
    let mut front = paint_cell(kind, cell, cs, seed, min_px);
    // Two sprays from deeper in the crown, mirrored so their twigs don't line up with the
    // front one's, in its shade.
    for (layer, shade) in [(1u64, 0.62f32), (2, 0.45)] {
        let mut back = paint_cell(kind, cell, cs, seed ^ (0xb0d1 * layer), min_px);
        if layer == 1 {
            back.mirror();
        }
        back.shade(shade);
        front.over(&back);
    }
    // The mass behind them: leaves in deep shade, packed where the sprays' coverage blurred
    // says the crown is thick. Each leaf is a cell of jittered points with its own shade and
    // tilt, darker where it tucks under its neighbors; whole leaves drop out toward the
    // thin edges, so the outline stays ragged instead of a smooth blob.
    let s = cs as usize;
    let r = (0.06 * cs as f32).max(1.0) as usize;
    let blurred = box_blur(&front.px, s, r);
    let alpha_at = |x: f32, y: f32| {
        let (xx, yy) = (
            (x as i32).clamp(0, s as i32 - 1) as usize,
            (y as i32).clamp(0, s as i32 - 1) as usize,
        );
        blurred[yy * s + xx][3]
    };
    let leaf = match kind {
        LeafKind::Lobed | LeafKind::Palmate => 0.07,
        LeafKind::Broad => 0.06,
        LeafKind::Oval => 0.055,
        LeafKind::Round => 0.05,
        LeafKind::Small => 0.04,
        // The leaflets.
        LeafKind::Pinnate => 0.04,
        LeafKind::LongNeedles => 0.035,
        LeafKind::ShortNeedles | LeafKind::SoftNeedles | LeafKind::Frond | LeafKind::Palm => 0.03,
    } * cs as f32;
    let edge = (0.04 * cs as f32).max(1.0);
    let cell_seed = seed ^ (cell as u64).wrapping_mul(0x51ed_270b) ^ 0x3a55;
    let mut fill = Canvas::new(cs);
    for y in 0..s {
        for x in 0..s {
            // Fade out at the cell's border so it never bleeds into its neighbors.
            let border = (x.min(y).min(s - 1 - x).min(s - 1 - y) as f32 / edge).min(1.0);
            let a = blurred[y * s + x][3];
            if border <= 0.0 || a < 0.05 {
                continue;
            }
            let p = Vec2::new(x as f32, y as f32) / leaf;
            let (gx, gy) = (p.x.floor() as i64, p.y.floor() as i64);
            let (mut d1, mut d2, mut id) = (f32::MAX, f32::MAX, (0i64, 0i64));
            for (ox, oy) in (-1..=1).flat_map(|ox| (-1..=1).map(move |oy| (ox, oy))) {
                let (cx, cy) = (gx + ox, gy + oy);
                let c = Vec2::new(
                    cx as f32 + hash01(cx, cy, 0, cell_seed),
                    cy as f32 + hash01(cx, cy, 1, cell_seed),
                );
                let d = (p - c).length_squared();
                if d < d1 {
                    (d2, d1, id) = (d1, d, (cx, cy));
                } else if d < d2 {
                    d2 = d;
                }
            }
            let (cx, cy) = id;
            let center = Vec2::new(
                cx as f32 + hash01(cx, cy, 0, cell_seed),
                cy as f32 + hash01(cx, cy, 1, cell_seed),
            ) * leaf;
            let thick = alpha_at(center.x, center.y);
            if thick < 0.3 + 0.25 * hash01(cx, cy, 2, cell_seed) {
                continue;
            }
            let gap = (d2.sqrt() - d1.sqrt()).min(1.0);
            let tuck = 0.55 + 0.45 * smoothstep(0.0, 0.25, gap);
            let shade = (0.2 + 0.2 * hash01(cx, cy, 3, cell_seed)) * tuck;
            let base = blurred[y * s + x];
            let color = [0, 1, 2].map(|c| base[c] / a.max(1e-3) * shade);
            let tilt = Vec2::new(
                hash01(cx, cy, 4, cell_seed) - 0.5,
                hash01(cx, cy, 5, cell_seed) - 0.5,
            ) * 0.7;
            let normal = Vec3::new(tilt.x, tilt.y, 1.0).normalize();
            fill.put(x as i32, y as i32, color, normal, border);
        }
    }
    front.over(&fill);
    front
}

/// Box blur of a square image `s` texels wide, radius `r` texels (two passes of running sums).
fn box_blur(px: &[[f32; 4]], s: usize, r: usize) -> Vec<[f32; 4]> {
    let pass = |src: &[[f32; 4]], horizontal: bool| {
        let mut out = vec![[0.0f32; 4]; s * s];
        let idx = |line: usize, k: usize| {
            if horizontal {
                line * s + k
            } else {
                k * s + line
            }
        };
        let norm = 1.0 / (2 * r + 1) as f32;
        for line in 0..s {
            let mut acc = [0.0f32; 4];
            for k in 0..=r.min(s - 1) {
                for c in 0..4 {
                    acc[c] += src[idx(line, k)][c];
                }
            }
            for k in 0..s {
                for c in 0..4 {
                    out[idx(line, k)][c] = acc[c] * norm;
                }
                if k + r + 1 < s {
                    let add = src[idx(line, k + r + 1)];
                    for c in 0..4 {
                        acc[c] += add[c];
                    }
                }
                if k >= r {
                    let sub = src[idx(line, k - r)];
                    for c in 0..4 {
                        acc[c] -= sub[c];
                    }
                }
            }
        }
        out
    };
    pass(&pass(px, true), false)
}

/// Pixels per side of the cell paintings card outlines are found in ([`cell_outline`]).
const OUTLINE_CELL_PX: u32 = 128;

/// The outline of what atlas cell `cell` (of the leaf atlas, or with `mass` of the mass atlas)
/// paints, for a card to cover only that: a convex polygon inside the cell with at most
/// `corners` corners around every texel the painting covers at all, grown by a margin for its
/// mip levels. Cell coordinates, x right and y down, counter-clockwise in (x, y). A card drawn
/// as the whole cell paid its pixel shader for every texel, though a spray covers 11-31% of
/// its cell (2026-11-07). Persson's particle trimming (GPU Pro 6) weighs the corners: more cut
/// more empty pixels but cost vertices. `None` for one-image textures and for cells the
/// outline would barely trim. Painted once per kind, cell and atlas and kept.
pub fn cell_outline(
    kind: LeafKind,
    cell: usize,
    seed: u64,
    mass: bool,
    corners: usize,
) -> Option<std::sync::Arc<Vec<Vec2>>> {
    use std::collections::HashMap;
    use std::sync::{Arc, Mutex, OnceLock};
    type Key = (LeafKind, usize, u64, bool, usize);
    type Outlines = HashMap<Key, Option<Arc<Vec<Vec2>>>>;
    static CACHE: OnceLock<Mutex<Outlines>> = OnceLock::new();
    if !kind.is_atlas() {
        return None;
    }
    let cell = cell % (ATLAS_CELLS * ATLAS_CELLS) as usize;
    let key = (kind, cell, seed, mass, corners);
    let cache = CACHE.get_or_init(Default::default);
    if let Some(found) = cache.lock().unwrap_or_else(|e| e.into_inner()).get(&key) {
        return found.clone();
    }
    let min_px = if kind.is_needles() { 1.0 } else { 0.0 };
    let canvas = if mass {
        mass_cell(kind, cell, OUTLINE_CELL_PX, seed, min_px)
    } else {
        paint_cell(kind, cell, OUTLINE_CELL_PX, seed, min_px)
    };
    // Needles are painted at least a pixel wide in every mip level (8 pixel cells at the
    // least): their coverage spreads further out than level 0's.
    let margin = if kind.is_needles() { 0.04 } else { 0.02 };
    let outline = outline_of(&canvas, margin, corners).map(Arc::new);
    cache
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .insert(key, outline.clone());
    outline
}

/// The trimmed outline of a painted cell ([`cell_outline`]).
fn outline_of(canvas: &Canvas, margin: f32, corners: usize) -> Option<Vec<Vec2>> {
    let s = canvas.size as usize;
    let px = 1.0 / s as f32;
    // The covered texels' outer corners: each row's first and last covered texel suffice.
    let mut pts = Vec::new();
    for y in 0..s {
        let row = &canvas.px[y * s..(y + 1) * s];
        let (Some(first), Some(last)) = (
            row.iter().position(|p| p[3] > 0.0),
            row.iter().rposition(|p| p[3] > 0.0),
        ) else {
            continue;
        };
        for x in [first, last + 1] {
            for yy in [y, y + 1] {
                pts.push(Vec2::new(x as f32 * px, yy as f32 * px));
            }
        }
    }
    let hull = convex_hull(pts);
    if hull.len() < 3 {
        return None;
    }
    // The grown outline's sides, each a line the painting lies wholly on one side of.
    let grown = grow_polygon(&reduce_polygon(hull, 20), margin);
    let n = grown.len();
    let sides: Vec<(Vec2, Vec2)> = (0..n).map(|i| (grown[i], grown[(i + 1) % n])).collect();
    // The cell cut by the few of them leaving the smallest polygon with `corners` corners or
    // fewer: it stays inside the cell (beyond it a card would show its neighbor's painting),
    // and draws with as many triangles as the cell's quad when `corners` is four. Every set
    // of up to `corners` sides is tried (a few thousand cuts of a small polygon).
    let cell = vec![Vec2::ZERO, Vec2::X, Vec2::ONE, Vec2::Y];
    let mut best = (1.0f32, cell.clone());
    let mut pick = Vec::with_capacity(corners);
    try_cuts(&cell, &sides, 0, corners, corners, &mut pick, &mut best);
    // Barely smaller than the cell: the whole cell's quad draws it about as cheaply.
    (best.0 < 0.85).then_some(best.1)
}

/// Cut `poly` by every set of up to `left` more of `sides` from `from` on, keeping in `best`
/// the smallest result with at most `corners` corners ([`outline_of`]).
fn try_cuts(
    poly: &[Vec2],
    sides: &[(Vec2, Vec2)],
    from: usize,
    left: usize,
    corners: usize,
    pick: &mut Vec<usize>,
    best: &mut (f32, Vec<Vec2>),
) {
    if poly.len() >= 3 && poly.len() <= corners {
        let area = polygon_area(poly);
        if area < best.0 {
            *best = (area, poly.to_vec());
        }
    }
    if left == 0 {
        return;
    }
    for i in from..sides.len() {
        let (a, b) = sides[i];
        let cut = clip_half(poly, a, b);
        if cut.len() < 3 || polygon_area(&cut) >= best.0 {
            continue;
        }
        pick.push(i);
        try_cuts(&cut, sides, i + 1, left - 1, corners, pick, best);
        pick.pop();
    }
}

/// The part of convex polygon `p` left of the line from `a` to `b` (Sutherland-Hodgman), with
/// corners closer than a hair merged.
fn clip_half(p: &[Vec2], a: Vec2, b: Vec2) -> Vec<Vec2> {
    let side = |v: Vec2| cross3(a, b, v);
    let mut out: Vec<Vec2> = Vec::with_capacity(p.len() + 1);
    let n = p.len();
    for i in 0..n {
        let (u, v) = (p[i], p[(i + 1) % n]);
        let (su, sv) = (side(u), side(v));
        if sv >= 0.0 {
            if su < 0.0 {
                out.push(u + (v - u) * (su / (su - sv)));
            }
            out.push(v);
        } else if su >= 0.0 {
            out.push(u + (v - u) * (su / (su - sv)));
        }
    }
    out.dedup_by(|x, y| x.distance(*y) < 1e-5);
    if out.len() > 1 && out[0].distance(out[out.len() - 1]) < 1e-5 {
        out.pop();
    }
    out
}

/// Twice the signed area of triangle `a b c` (positive counter-clockwise in x right, y up).
fn cross3(a: Vec2, b: Vec2, c: Vec2) -> f32 {
    (b - a).perp_dot(c - a)
}

/// The convex hull of `pts`, counter-clockwise (Andrew's monotone chain).
fn convex_hull(mut pts: Vec<Vec2>) -> Vec<Vec2> {
    pts.sort_by(|a, b| a.x.total_cmp(&b.x).then(a.y.total_cmp(&b.y)));
    pts.dedup();
    if pts.len() < 3 {
        return pts;
    }
    let mut hull: Vec<Vec2> = Vec::with_capacity(pts.len() + 1);
    for pass in 0..2 {
        let start = hull.len();
        let ordered: Box<dyn Iterator<Item = &Vec2>> = if pass == 0 {
            Box::new(pts.iter())
        } else {
            Box::new(pts.iter().rev())
        };
        for &p in ordered {
            while hull.len() >= start + 2
                && cross3(hull[hull.len() - 2], hull[hull.len() - 1], p) <= 0.0
            {
                hull.pop();
            }
            hull.push(p);
        }
        hull.pop();
    }
    hull
}

/// Area of a simple polygon.
fn polygon_area(p: &[Vec2]) -> f32 {
    let n = p.len();
    (0..n)
        .map(|i| p[i].perp_dot(p[(i + 1) % n]))
        .sum::<f32>()
        .abs()
        * 0.5
}

/// Where the lines through `a` along `da` and through `b` along `db` cross: `a + da * t`,
/// with `t` and the other line's parameter.
fn line_cross(a: Vec2, da: Vec2, b: Vec2, db: Vec2) -> Option<(f32, f32)> {
    let d = da.perp_dot(db);
    if d.abs() < 1e-12 {
        return None;
    }
    let w = b - a;
    Some((w.perp_dot(db) / d, w.perp_dot(da) / d))
}

/// A convex counter-clockwise polygon with at most `k` corners containing `hull`: edges are dropped one by one, extending their neighbors to meet,
/// each time the one adding the least area (Aggarwal et al.'s minimal enclosing k-gon,
/// greedily).
fn reduce_polygon(mut hull: Vec<Vec2>, k: usize) -> Vec<Vec2> {
    while hull.len() > k {
        let n = hull.len();
        let mut best: Option<(usize, Vec2, f32)> = None;
        for i in 0..n {
            let (a, b, c, d) = (
                hull[(i + n - 1) % n],
                hull[i],
                hull[(i + 1) % n],
                hull[(i + 2) % n],
            );
            // Extend a→b past b and d→c past c.
            let Some((t, u)) = line_cross(a, b - a, d, c - d) else {
                continue;
            };
            if t < 1.0 || u < 1.0 {
                continue;
            }
            let q = a + (b - a) * t;
            let added = cross3(b, q, c).abs() * 0.5;
            if best.is_none_or(|(_, _, area)| added < area) {
                best = Some((i, q, added));
            }
        }
        let Some((i, q, _)) = best else {
            break;
        };
        hull[i] = q;
        hull.remove((i + 1) % n);
    }
    hull
}

/// A convex counter-clockwise polygon with every edge moved `d` outward.
fn grow_polygon(p: &[Vec2], d: f32) -> Vec<Vec2> {
    let n = p.len();
    let edge = |i: usize| {
        let (a, b) = (p[i], p[(i + 1) % n]);
        let dir = (b - a).normalize_or_zero();
        // Outward of a counter-clockwise polygon: to the right of each edge.
        (a + Vec2::new(dir.y, -dir.x) * d, dir)
    };
    (0..n)
        .map(|i| {
            let (a, da) = edge((i + n - 1) % n);
            let (b, db) = edge(i);
            match line_cross(a, da, b, db) {
                Some((t, _)) => a + da * t,
                None => b,
            }
        })
        .collect()
}

/// Paint a leaf texture (color and alpha only; see [`leaf_textures`] for the normal map).
pub fn leaf_texture(kind: LeafKind, size: u32, seed: u64) -> Image {
    leaf_textures(kind, size, seed).color
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A card's outline holds everything its cell draws, in every level of the leaf and mass
    /// atlases the crowns use, in four corners, and trims most cells by a quarter or more.
    #[test]
    fn card_outlines_hold_the_painting_and_trim_the_cell() {
        let mut areas = Vec::new();
        for kind in [
            LeafKind::Broad,
            LeafKind::Small,
            LeafKind::Lobed,
            LeafKind::Oval,
            LeafKind::Round,
            LeafKind::Pinnate,
            LeafKind::LongNeedles,
            LeafKind::ShortNeedles,
            LeafKind::SoftNeedles,
            LeafKind::Palm,
            LeafKind::Palmate,
        ] {
            let seed = 7;
            for (mass, img) in [
                (false, leaf_textures(kind, 512, seed).color),
                (true, mass_textures(kind, 256, seed).color),
            ] {
                let levels = std::iter::once((img.width, &img.data)).chain(
                    img.mips
                        .iter()
                        .enumerate()
                        .map(|(m, d)| (img.width >> (m + 1), d)),
                );
                let corners = 4;
                let outlines: Vec<_> = (0..4)
                    .map(|c| cell_outline(kind, c, seed, mass, corners))
                    .collect();
                for (size, data) in levels.take(4) {
                    let cs = size / ATLAS_CELLS;
                    for y in 0..size {
                        for x in 0..size {
                            let a = data[((y * size + x) * 4 + 3) as usize] as f32 / 255.0;
                            if a < ALPHA_CUTOFF {
                                continue;
                            }
                            let c = ((y / cs) * ATLAS_CELLS + x / cs) as usize;
                            let Some(o) = &outlines[c] else { continue };
                            let q = Vec2::new(
                                ((x % cs) as f32 + 0.5) / cs as f32,
                                ((y % cs) as f32 + 0.5) / cs as f32,
                            );
                            let inside = (0..o.len())
                                .all(|i| cross3(o[i], o[(i + 1) % o.len()], q) >= -1e-4);
                            assert!(
                                inside,
                                "{kind:?} mass {mass} cell {c} texel {x},{y} of {size}"
                            );
                        }
                    }
                }
                for o in outlines.iter().flatten() {
                    assert!(o.len() <= corners, "{kind:?} {}", o.len());
                    areas.push(polygon_area(o));
                }
            }
        }
        let mean = areas.iter().sum::<f32>() / areas.len() as f32;
        assert!(areas.len() > 60, "{} cells trimmed", areas.len());
        // 0.65 of the leaf atlas's cells and 0.71 of the mass atlas's, with four corners
        // (2026-11-07).
        assert!(mean < 0.75, "outlines cover {mean} of their cells");
        println!("{} outlines, mean area {mean:.2}", areas.len());
    }

    #[test]
    fn leaf_textures_have_cutouts_and_keep_coverage_in_mips() {
        for kind in [
            LeafKind::Broad,
            LeafKind::Small,
            LeafKind::Lobed,
            LeafKind::Oval,
            LeafKind::Round,
            LeafKind::Pinnate,
            LeafKind::LongNeedles,
            LeafKind::ShortNeedles,
            LeafKind::SoftNeedles,
            LeafKind::Frond,
            LeafKind::Palm,
            LeafKind::Palmate,
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
    fn mass_atlases_are_denser_clumps_inside_their_cells() {
        let cover = |img: &Image, cell: Option<u32>| {
            let s = img.width;
            let mut n = 0;
            let mut total = 0;
            for (i, p) in img.data.as_chunks::<4>().0.iter().enumerate() {
                let (x, y) = (i as u32 % s, i as u32 / s);
                let half = s / ATLAS_CELLS;
                // `Some(k)`: only texels within k of a cell's border.
                if let Some(k) = cell {
                    let (cx, cy) = (x % half, y % half);
                    if cx.min(cy).min(half - 1 - cx).min(half - 1 - cy) >= k {
                        continue;
                    }
                }
                total += 1;
                if p[3] as f32 / 255.0 >= ALPHA_CUTOFF {
                    n += 1;
                }
            }
            n as f32 / total.max(1) as f32
        };
        for kind in [
            LeafKind::Broad,
            LeafKind::Small,
            LeafKind::Lobed,
            LeafKind::Oval,
            LeafKind::Round,
            LeafKind::Pinnate,
            LeafKind::LongNeedles,
            LeafKind::ShortNeedles,
            LeafKind::SoftNeedles,
        ] {
            let leaves = leaf_textures(kind, 256, 7).color;
            let mass = mass_textures(kind, 256, 7).color;
            let (l, m) = (cover(&leaves, None), cover(&mass, None));
            assert!(m > l * 1.6 && m < 0.85, "{kind:?}: mass {m} vs leaves {l}");
            // Nothing at the cells' borders, so filtering doesn't bleed into a neighbor.
            assert!(cover(&mass, Some(1)) < 0.01, "{kind:?} border");
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
        for kind in [
            LeafKind::LongNeedles,
            LeafKind::ShortNeedles,
            LeafKind::SoftNeedles,
        ] {
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
    fn every_leaf_style_has_its_own_kind() {
        use crate::species::LeafStyle;
        let styles = [
            LeafStyle::Broad,
            LeafStyle::Small,
            LeafStyle::LongNeedles,
            LeafStyle::ShortNeedles,
            LeafStyle::Frond,
            LeafStyle::Lobed,
            LeafStyle::Oval,
            LeafStyle::Round,
            LeafStyle::Pinnate,
            LeafStyle::SoftNeedles,
            LeafStyle::Palm,
            LeafStyle::Palmate,
        ];
        let kinds: std::collections::HashSet<_> = styles.iter().map(|s| kind_of(*s)).collect();
        assert_eq!(kinds.len(), styles.len());
        assert!(kind_of(LeafStyle::SoftNeedles).is_needles());
    }

    #[test]
    fn new_broadleaf_kinds_lay_out_sprays_and_single_leaves() {
        for kind in [LeafKind::Oval, LeafKind::Round, LeafKind::Pinnate] {
            for cell in 0..(ATLAS_CELLS * ATLAS_CELLS) as usize {
                let layout = spray_layout(kind, cell, 7).expect("broadleaf layout");
                assert!(layout.leaves.len() >= 4, "{kind:?} cell {cell}");
                assert_eq!(layout.twigs.is_empty(), cell == CLUSTER_CELL, "{kind:?}");
                for l in &layout.leaves {
                    for p in [l.at, l.base, l.base + l.dir * l.length] {
                        assert!(p.cmpge(Vec2::ZERO).all() && p.cmple(Vec2::ONE).all());
                    }
                }
            }
            // Each single leaf stays inside the part of its cell the meshes draw.
            let t = hero_leaf_textures(kind, 256, 7);
            let (r0, r1) = hero_leaf_rect(kind);
            let cs = 256 / HERO_CELLS;
            let mut inside = 0;
            for (i, p) in t.color.data.as_chunks::<4>().0.iter().enumerate() {
                let (x, y) = (i as u32 % 256, i as u32 / 256);
                if (y / cs) * HERO_CELLS + x / cs == HERO_TWIG_CELL as u32 || p[3] < 128 {
                    continue;
                }
                let q = Vec2::new((x % cs) as f32 + 0.5, (y % cs) as f32 + 0.5) / cs as f32;
                let ok = q.cmpge(r0 - 0.02).all() && q.cmple(r1 + 0.02).all();
                assert!(ok, "{kind:?} leaf texel {q} outside {r0}..{r1}");
                inside += 1;
            }
            assert!(inside > 256 * 256 / 20, "{kind:?} {inside}");
        }
        assert!(spray_layout(LeafKind::SoftNeedles, 0, 7).is_none());
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
