//! Glowing surfaces: dark stone with carved, glowing rune inlays, and faceted magic crystal.

use engine_core::math::{Vec2, Vec3, Vec4};
use engine_render::StandardMaterial;

use super::Generated;
use super::noise::{mix32, rand, sub, unit};
use super::stone::{basalt, basalt_layers, facets};
use super::textures::{Maps, Texel, Tile, hex, mix, mix3, plausible, relief, smooth, white};

/// Which glowing inlay a rune surface has.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum RuneKind {
    /// Violet sigils with rings.
    Arcane,
    /// Blue-white runes with lightning staves.
    Storm,
    /// Green runes with curling branches.
    Life,
    /// Orange angular runes.
    Ember,
}

/// A stroke of a glyph in glyph space (0..1 across, 0..1 down).
#[derive(Clone, Copy)]
enum Stroke {
    Line(Vec2, Vec2),
    /// Center, radius, start angle, sweep (radians).
    Arc(Vec2, f32, f32, f32),
}

impl Stroke {
    fn dist(&self, q: Vec2) -> f32 {
        match *self {
            Stroke::Line(a, b) => {
                let ab = b - a;
                let t = ((q - a).dot(ab) / ab.length_squared().max(1e-9)).clamp(0.0, 1.0);
                (q - a - ab * t).length()
            }
            Stroke::Arc(c, r, a0, sweep) => {
                let d = q - c;
                let a = (d.y.atan2(d.x) - a0).rem_euclid(std::f32::consts::TAU);
                if a <= sweep {
                    (d.length() - r).abs()
                } else {
                    let e0 = c + Vec2::new(a0.cos(), a0.sin()) * r;
                    let e1 = c + Vec2::new((a0 + sweep).cos(), (a0 + sweep).sin()) * r;
                    (q - e0).length().min((q - e1).length())
                }
            }
        }
    }
}

/// A random rune: a stave and branches between points of a 3×4 grid, with touches of the
/// kind's style.
fn glyph(kind: RuneKind, seed: u32) -> Vec<Stroke> {
    let r = |k: u32| rand(seed, k);
    let pt = |gx: i32, gy: i32| Vec2::new(gx as f32 * 0.5, gy as f32 / 3.0);
    let mut out = Vec::new();
    let stave = if r(0) < 0.5 {
        1
    } else if r(1) < 0.5 {
        0
    } else {
        2
    };
    let side = if stave == 2 { -1 } else { 1 };
    if kind == RuneKind::Storm && r(2) < 0.6 {
        // A lightning stave.
        out.push(Stroke::Line(pt(stave, 0), pt(stave + side, 1)));
        out.push(Stroke::Line(pt(stave + side, 1), pt(stave, 2)));
        out.push(Stroke::Line(pt(stave, 2), pt(stave + side, 3)));
    } else {
        out.push(Stroke::Line(pt(stave, 0), pt(stave, 3)));
    }
    let branches = 2 + (r(3) * 2.5) as u32;
    for b in 0..branches {
        let gy = (r(10 + b) * 4.0) as i32 % 4;
        let dir = match stave {
            0 => 1,
            2 => -1,
            _ if r(20 + b) < 0.5 => 1,
            _ => -1,
        };
        let dy = [-1, 0, 1][(r(30 + b) * 3.0) as usize % 3];
        let gy2 = (gy + dy).clamp(0, 3);
        let far = stave + 2 * dir;
        let end = if r(40 + b) < 0.3 && (0..=2).contains(&far) {
            pt(far, gy2)
        } else {
            pt(stave + dir, gy2)
        };
        let start = pt(stave, gy);
        if kind == RuneKind::Life && r(50 + b) < 0.6 {
            // A curling branch: an arc bulging up from the stave to the end point.
            let mid = (start + end) * 0.5;
            let n = Vec2::new(-(end - start).y, (end - start).x).normalize_or_zero();
            let c = mid + n * 0.15 * if r(55 + b) < 0.5 { 1.0 } else { -1.0 };
            let rad = (start - c).length();
            let a0 = (start - c).y.atan2((start - c).x);
            let a1 = (end - c).y.atan2((end - c).x);
            let sweep = (a1 - a0).rem_euclid(std::f32::consts::TAU);
            if sweep < std::f32::consts::PI {
                out.push(Stroke::Arc(c, rad, a0, sweep));
            } else {
                out.push(Stroke::Arc(c, rad, a1, std::f32::consts::TAU - sweep));
            }
        } else {
            out.push(Stroke::Line(start, end));
        }
    }
    match kind {
        RuneKind::Arcane => {
            if r(60) < 0.45 {
                out.push(Stroke::Arc(
                    Vec2::new(0.5, 0.5),
                    0.62,
                    0.0,
                    std::f32::consts::TAU,
                ));
            } else {
                let c = pt((r(61) * 3.0) as i32 % 3, 1 + (r(62) * 2.0) as i32 % 2);
                out.push(Stroke::Arc(c, 0.12, 0.0, std::f32::consts::TAU));
            }
        }
        RuneKind::Ember => {
            if r(63) < 0.5 {
                let y0 = 0.15 + r(64) * 0.6;
                out.push(Stroke::Line(Vec2::new(0.0, y0 + 0.2), Vec2::new(0.5, y0)));
                out.push(Stroke::Line(Vec2::new(0.5, y0), Vec2::new(1.0, y0 + 0.2)));
            }
        }
        RuneKind::Life => {
            if r(65) < 0.4 {
                out.push(Stroke::Arc(pt(stave, 3), 0.2, 3.6, 2.2));
            }
        }
        RuneKind::Storm => {
            if r(66) < 0.5 {
                let y0 = r(67) * 0.8 + 0.1;
                out.push(Stroke::Line(Vec2::new(0.1, y0), Vec2::new(0.9, y0 + 0.08)));
            }
        }
    }
    out
}

/// Dark stone with carved glyph grooves filled with a glowing inlay (the emissive texture
/// carries the color; a faint halo stands for light thrown onto the groove walls). 0.5 m per
/// tile: four rows of four glyphs between frieze lines.
pub fn rune(kind: RuneKind, shrink: u32) -> Generated {
    let tile = Tile::new(512 >> shrink, 512 >> shrink, 0.5);
    let (seed, color, strength) = match kind {
        RuneKind::Arcane => (0x4a1c_0001u32, Vec3::new(0.62, 0.32, 1.0), 60.0),
        RuneKind::Storm => (0x4a1c_0002, Vec3::new(0.5, 0.72, 1.0), 60.0),
        RuneKind::Life => (0x4a1c_0003, Vec3::new(0.4, 1.0, 0.45), 55.0),
        RuneKind::Ember => (0x4a1c_0004, Vec3::new(1.0, 0.42, 0.12), 70.0),
    };
    let (cols, rows) = (4usize, 4usize);
    let glyphs: Vec<Vec<Stroke>> = (0..cols * rows)
        .map(|g| glyph(kind, sub(seed, 500 + g as u32)))
        .collect();
    let layers = basalt_layers(&tile, seed);
    let cell = Vec2::new(tile.size.x / cols as f32, tile.size.y / rows as f32);
    let glyph_size = Vec2::new(0.06, 0.075);
    let groove = 0.003;
    #[derive(Clone, Copy, Default)]
    struct P {
        h: f32,
        albedo: Vec3,
        rough: f32,
        d: f32,
    }
    let px = tile.par(|p, x, y| {
        let i = y * tile.w + x;
        let b = layers.at(x, y);
        let (mut h, c, ro) = basalt(&tile, p, i, b, seed);
        let m = p * tile.size;
        let cx = ((m.x / cell.x) as usize).min(cols - 1);
        let cy = ((m.y / cell.y) as usize).min(rows - 1);
        let local = m - Vec2::new(cx as f32, cy as f32) * cell;
        // Glyph space, with a hand-cut wobble.
        let wob = Vec2::new(
            tile.noise(p, 0.03, sub(seed, 20)),
            tile.noise(p, 0.03, sub(seed, 21)),
        ) * 0.0006;
        let q = (local + wob - (cell - glyph_size) * 0.5) / glyph_size;
        let mut d = f32::MAX;
        for s in &glyphs[cy * cols + cx] {
            d = d.min(s.dist(q) * glyph_size.x);
        }
        // Frieze lines framing each row.
        let fy = local.y.min(cell.y - local.y);
        d = d.min((fy - 0.008).abs());
        let w = groove * (1.0 + tile.noise(p, 0.01, sub(seed, 22)) * 0.2);
        let cut = (1.0 - d / w).max(0.0);
        // V-cut with a slightly rounded floor.
        h -= 0.003 * cut.powf(0.8);
        P {
            h,
            albedo: c,
            rough: ro,
            d: d / w,
        }
    });
    let height: Vec<f32> = px.iter().map(|v| v.h).collect();
    let r = relief(&tile, &height, &[0.0015, 0.005, 0.02], 1.5, 0.0015);
    let texels = tile.par_idx(|i| {
        let v = px[i];
        let ao = r.ao[i];
        // Inlay fill sits in the bottom of the groove.
        let fill = smooth(0.75, 0.35, v.d);
        let core = smooth(0.45, 0.0, v.d);
        let halo = (-(v.d - 0.6).max(0.0) * 1.6).exp() * 0.1;
        let glow = (fill * 0.85 + core * 0.15).max(halo);
        let e = mix3(color, mix3(color, Vec3::ONE, 0.4), core) * glow;
        let mut c = v.albedo * mix(0.55, 1.0, ao);
        c = mix3(c, color * 0.4 + Vec3::splat(0.06), fill);
        let rough = mix(v.rough, 0.2, fill);
        Texel {
            albedo: plausible(c),
            rough,
            metal: 0.0,
            ao,
            emissive: e,
            alpha: 1.0,
        }
    });
    let maps = Maps::new(tile, r.normal, &texels, true, false);
    Generated::new(
        maps,
        StandardMaterial::default().with_emissive([1.0, 1.0, 1.0], strength),
    )
}

/// Faceted, glowing magic crystal: a few broad, slightly tilted facets with fine growth
/// striations, milky veils and veins, bright inclusions, and an inner glow that is stronger
/// in the veins. 0.3 m per tile.
pub fn crystal(shrink: u32) -> Generated {
    let tile = Tile::new(512 >> shrink, 512 >> shrink, 0.3);
    let seed = 0xc7a5_0002;
    let fper = tile.period(0.15);
    // Whole striations per tile (about 1.6 mm apart) so they tile.
    let stripes = (tile.size.y / 0.0016).round();
    let cloud = tile.coarse(2, |p| {
        let q = tile.warp(p, 0.1, 0.03, 3, sub(seed, 6));
        Vec4::new(
            tile.fbm(q, 0.1, 4, 0.55, sub(seed, 1)),
            tile.ridged(q, 0.08, 4, 0.5, sub(seed, 2)),
            tile.fbm(p, 0.05, 3, 0.5, sub(seed, 3)),
            tile.fbm(p, 0.2, 2, 0.5, sub(seed, 7)),
        )
    });
    #[derive(Clone, Copy, Default)]
    struct C {
        h: f32,
        facet: u32,
        cl: Vec4,
        incl: f32,
        edge: f32,
    }
    let px = tile.par(|p, x, y| {
        let cl = cloud.at(x, y);
        let (fh, gap, facet) = facets(p, fper, 0.3, 0.006, sub(seed, 4));
        // Growth striations across the faces.
        let stri = (p.y * std::f32::consts::TAU * stripes + cl.z * 8.0).sin()
            * (0.5 + 0.5 * cl.w).max(0.0);
        let incl = smooth(0.955, 0.985, tile.value(p, 0.003, sub(seed, 5)));
        // Facets are real planes (cell units to meters), the edges between them sharp.
        let h = fh * 0.15 * 0.3 + stri * 0.00002;
        C {
            h,
            facet,
            cl,
            incl,
            edge: smooth(0.03, 0.0, gap),
        }
    });
    let height: Vec<f32> = px.iter().map(|v| v.h).collect();
    let r = relief(&tile, &height, &[0.002, 0.01], 0.3, 0.002);
    let texels = tile.par_idx(|i| {
        let v = px[i];
        let cl = v.cl;
        let facet_tone = unit(mix32(v.facet ^ 11));
        // Each facet looks into the stone at its own depth: some glow from deep inside.
        let depth = smooth(-0.3, 0.4, cl.w + (facet_tone - 0.5) * 0.5);
        let veil = smooth(0.0, 0.6, cl.x) * 0.6;
        // Sparse internal fracture planes catch the light.
        let crack = smooth(0.86, 0.97, cl.y) * smooth(-0.1, 0.3, cl.z);
        let albedo = mix3(
            hex(0x3c5aa8),
            hex(0xa8bce8),
            (veil * 0.5 + crack * 0.6 + v.incl * 0.5).min(1.0),
        ) * mix(0.85, 1.15, facet_tone);
        let glow = 0.15 + depth * 0.6 + veil * 0.2 + crack * 0.5 + v.incl * 0.5;
        let e = mix3(
            Vec3::new(0.25, 0.45, 1.0),
            Vec3::new(0.8, 0.9, 1.0),
            (crack + v.incl).min(1.0),
        ) * glow.clamp(0.0, 1.2);
        let rough = 0.04 + crack * 0.08 + v.edge * 0.15 + (white(i, 1) - 0.5) * 0.02;
        Texel {
            albedo: plausible(albedo),
            rough,
            metal: 0.0,
            ao: mix(1.0, r.ao[i], 0.3),
            emissive: e,
            alpha: 1.0,
        }
    });
    let maps = Maps::new(tile, r.normal, &texels, true, false);
    let mut base = StandardMaterial::default().with_emissive([1.0, 1.0, 1.0], 60.0);
    base.receives_wetness = false;
    Generated::new(maps, base)
}
