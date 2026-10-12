//! Procedural leaf textures: sprays of broad leaves, small birch leaves, pine and spruce
//! needles, and fern fronds, painted on the CPU into RGBA images with alpha cut-outs.
//!
//! Colors are near-neutral (light yellow-green luminance with veins and shading); the species'
//! leaf color multiplies them, so one texture serves every tint and autumn can recolor leaves by
//! changing a material color.
//!
//! Mip levels keep the alpha coverage of the full-size image, so distant crowns stay full
//! instead of thinning out into see-through ghosts (the classic alpha-test mip problem).

use engine_core::math::{Rng, Vec2};
use engine_render::Image;

/// Which kind of leaves to paint.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum LeafKind {
    /// Ovate leaves on a twig spray (oak, hazel).
    Broad,
    /// Many small leaves (birch).
    Small,
    /// Long needles in tufts along a twig (pine).
    LongNeedles,
    /// Short dense needles on flat sprays (spruce).
    ShortNeedles,
    /// A fern frond: a stem with leaflets that shrink toward the tip.
    Frond,
}

/// Alpha cutoff used by leaf materials.
pub const ALPHA_CUTOFF: f32 = 0.5;

/// RGBA float canvas.
struct Canvas {
    size: u32,
    px: Vec<[f32; 4]>,
}

impl Canvas {
    fn new(size: u32) -> Self {
        Self {
            size,
            px: vec![[0.0; 4]; (size * size) as usize],
        }
    }

    /// Paint over with `color` at coverage `a`.
    fn blend(&mut self, x: i32, y: i32, color: [f32; 3], a: f32) {
        if x < 0 || y < 0 || x >= self.size as i32 || y >= self.size as i32 || a <= 0.0 {
            return;
        }
        let p = &mut self.px[(y as u32 * self.size + x as u32) as usize];
        let a = a.min(1.0);
        for c in 0..3 {
            p[c] = p[c] * (1.0 - a) + color[c] * a;
        }
        p[3] = p[3] + a * (1.0 - p[3]);
    }

    /// A thick anti-aliased line (twigs, stems, needles).
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

    /// A leaf from `base` along `dir` (unit), `length` and `width` in texture units.
    /// `shape` > 1 makes it slimmer at the base (ovate), `lobes` adds an oak-like wavy edge.
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

    fn into_image(mut self) -> Image {
        // Fill transparent texels with the average leaf color so filtering doesn't pull in a
        // dark fringe.
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
        for p in &mut self.px {
            if p[3] < 1.0 {
                let a = p[3];
                for c in 0..3 {
                    p[c] = p[c] * a + avg[c] * (1.0 - a);
                }
            }
        }
        let size = self.size;
        let mut levels = vec![self.px];
        while levels.last().unwrap().len() > 1 {
            let prev = levels.last().unwrap();
            let ps = (prev.len() as f32).sqrt() as u32;
            let ns = (ps / 2).max(1);
            let mut next = vec![[0.0f32; 4]; (ns * ns) as usize];
            for y in 0..ns {
                for x in 0..ns {
                    let mut acc = [0.0f32; 4];
                    for (dx, dy) in [(0, 0), (1, 0), (0, 1), (1, 1)] {
                        let q = prev
                            [((y * 2 + dy).min(ps - 1) * ps + (x * 2 + dx).min(ps - 1)) as usize];
                        for c in 0..4 {
                            acc[c] += q[c] * 0.25;
                        }
                    }
                    next[(y * ns + x) as usize] = acc;
                }
            }
            levels.push(next);
        }
        let target = coverage(&levels[0], 1.0);
        let encode = |level: &[[f32; 4]], alpha_scale: f32| -> Vec<u8> {
            let mut out = Vec::with_capacity(level.len() * 4);
            for p in level {
                for c in &p[..3] {
                    out.push((linear_to_srgb(*c).clamp(0.0, 1.0) * 255.0 + 0.5) as u8);
                }
                out.push(((p[3] * alpha_scale).clamp(0.0, 1.0) * 255.0 + 0.5) as u8);
            }
            out
        };
        let data = encode(&levels[0], 1.0);
        let mips = levels[1..]
            .iter()
            .map(|level| {
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
                encode(level, 0.5 * (lo + hi))
            })
            .collect();
        let mut image = Image::new(size, size, data, true);
        image.mips = mips;
        image
    }
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

/// Paint a leaf texture. The twig enters at the bottom center (v = 1) and the spray grows
/// toward the top (v = 0), so cards can attach their bottom edge to a branch.
pub fn leaf_texture(kind: LeafKind, size: u32, seed: u64) -> Image {
    let mut cv = Canvas::new(size);
    let mut rng = Rng::new(seed);
    let twig = [0.32, 0.28, 0.22];
    let base = Vec2::new(0.5, 0.99);
    match kind {
        LeafKind::Broad | LeafKind::Small => {
            let (twigs, leaves_per, len, wid, lobes) = match kind {
                LeafKind::Broad => (5, 7, 0.2, 0.11, 0.18),
                _ => (6, 11, 0.11, 0.075, 0.0),
            };
            let mut leaves = Vec::new();
            for t in 0..twigs {
                // Twigs fan out from the base.
                let spread = (t as f32 / (twigs - 1) as f32 - 0.5) * 1.6 + rng.range(-0.1, 0.1);
                let dir = Vec2::new(spread.sin(), -spread.cos()).normalize();
                let length = rng.range(0.6, 0.85);
                let bend = rng.range(-0.15, 0.15);
                let mut prev = base;
                let segs = 6;
                for s in 1..=segs {
                    let f = s as f32 / segs as f32;
                    let d = (dir + Vec2::new(-dir.y, dir.x) * bend * f).normalize();
                    let p = base + d * length * f;
                    cv.line(
                        prev,
                        p,
                        0.012 * (1.0 - f * 0.6),
                        0.012 * (1.0 - f * 0.6),
                        twig,
                    );
                    prev = p;
                    // Leaves alternate along the twig, pointing outward and forward.
                    for k in 0..(leaves_per / segs).max(1) + usize::from(s % 2 == 0) {
                        let sidedness = if (s + k) % 2 == 0 { 1.0 } else { -1.0 };
                        let out = Vec2::new(-d.y, d.x) * sidedness;
                        let ld = (d * 0.6 + out * rng.range(0.6, 1.1)).normalize();
                        let size_f = rng.range(0.75, 1.15) * (1.0 - 0.35 * f);
                        leaves.push((p, ld, len * size_f, wid * size_f, rng.range(0.72, 1.0)));
                    }
                }
                // A terminal leaf.
                leaves.push((prev, dir, len * 1.1, wid * 1.05, rng.range(0.8, 1.0)));
            }
            // Paint back-to-front so the outer leaves sit on top.
            for (p, d, l, w, lum) in leaves {
                let warm = rng.range(-1.0, 1.0);
                cv.leaf(p, d, l, w, lum, lobes, warm);
            }
        }
        LeafKind::LongNeedles | LeafKind::ShortNeedles => {
            let long = kind == LeafKind::LongNeedles;
            let twigs = if long { 3 } else { 5 };
            for t in 0..twigs {
                let spread =
                    (t as f32 / (twigs - 1).max(1) as f32 - 0.5) * if long { 0.9 } else { 1.4 };
                let dir = Vec2::new(spread.sin(), -spread.cos()).normalize();
                let length = rng.range(0.7, 0.92);
                let segs = if long { 9 } else { 22 };
                let mut prev = base;
                for s in 1..=segs {
                    let f = s as f32 / segs as f32;
                    let p = base + dir * length * f;
                    cv.line(
                        prev,
                        p,
                        0.014 * (1.0 - 0.5 * f),
                        0.012 * (1.0 - 0.5 * f),
                        twig,
                    );
                    prev = p;
                    let tufts = if long { 9 } else { 4 };
                    for k in 0..tufts {
                        let side = if k % 2 == 0 { 1.0 } else { -1.0 };
                        let ang = if long {
                            rng.range(0.25, 0.9) * side
                        } else {
                            rng.range(0.7, 1.2) * side
                        };
                        let (sa, ca) = ang.sin_cos();
                        let nd = Vec2::new(dir.x * ca - dir.y * sa, dir.x * sa + dir.y * ca);
                        let nl = if long {
                            rng.range(0.12, 0.2)
                        } else {
                            rng.range(0.035, 0.06) * (1.2 - 0.4 * f)
                        };
                        let lum = rng.range(0.65, 1.0);
                        let w = if long { 0.006 } else { 0.007 };
                        cv.line(p, p + nd * nl, w, w * 0.4, [lum * 0.95, lum, lum * 0.85]);
                    }
                }
            }
        }
        LeafKind::Frond => {
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
    cv.into_image()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn leaf_textures_have_cutouts_and_keep_coverage_in_mips() {
        for kind in [
            LeafKind::Broad,
            LeafKind::Small,
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
}
