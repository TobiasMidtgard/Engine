//! Teeth, gums and tongue, for a mouth that opens.
//!
//! The base mesh only has rough helper shapes inside the mouth (a band for each row of teeth
//! and a tongue). The teeth are made here instead: fourteen crowns per jaw (incisors, canines,
//! premolars and molars, sized like real ones) standing along the dental arch the helper band
//! traces, with a band of gum along their necks. The helper tongue is used as it is.
//!
//! Every vertex is tied to the three nearest helper vertices, so the mouth follows whatever
//! moves those: face shape targets (morphed with the face) and the jaw's expression shapes.

use engine_core::math::Vec3;

use crate::mesh::vertex_normals;
use crate::pack::HumanPack;

/// What a mouth vertex is, for the skin shader (stored in the normal's w).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum MouthPart {
    Teeth = 1,
    Gums = 2,
    Tongue = 3,
    /// Not in the mouth: shoes are built and drawn the same way (see `crate::shoes`).
    Shoe = 4,
    /// A skirt, built the same way (see `crate::skirt`).
    Skirt = 5,
}

/// Mouth geometry in the character's model space.
#[derive(Debug, Clone, Default)]
pub struct MouthMesh {
    pub positions: Vec<Vec3>,
    pub normals: Vec<Vec3>,
    pub parts: Vec<MouthPart>,
    /// Per vertex: three pack vertices and weights (summing to 1) it follows.
    pub anchors: Vec<[(u32, f32); 3]>,
    pub indices: Vec<u32>,
}

/// One kind of tooth: width along the arch, thickness front to back, crown height (m), and
/// shape (0 chisel edge, 1 pointed, 2 flat with cusps).
struct Tooth {
    width: f32,
    thickness: f32,
    height: f32,
    shape: u8,
}

const fn tooth(width_mm: f32, thickness_mm: f32, height_mm: f32, shape: u8) -> Tooth {
    Tooth {
        width: width_mm * 0.001,
        thickness: thickness_mm * 0.001,
        height: height_mm * 0.001,
        shape,
    }
}

/// From the middle outward: central and lateral incisor, canine, two premolars, two molars.
const UPPER: [Tooth; 7] = [
    tooth(8.6, 7.0, 10.5, 0),
    tooth(6.6, 6.2, 9.0, 0),
    tooth(7.6, 8.0, 10.0, 1),
    tooth(7.0, 9.0, 8.2, 2),
    tooth(6.6, 9.0, 7.8, 2),
    tooth(10.2, 11.0, 7.0, 2),
    tooth(9.0, 10.5, 6.5, 2),
];
const LOWER: [Tooth; 7] = [
    tooth(5.4, 6.0, 9.0, 0),
    tooth(5.9, 6.2, 9.0, 0),
    tooth(6.9, 7.6, 10.0, 1),
    tooth(7.0, 7.8, 8.0, 2),
    tooth(7.1, 8.2, 7.6, 2),
    tooth(11.2, 10.5, 7.0, 2),
    tooth(10.5, 10.0, 6.5, 2),
];

/// The mouth of a morphed body (`morphed`: the pack's vertices after the morphs). Empty when
/// the pack has no mouth helpers.
pub fn build(pack: &HumanPack, morphed: &[Vec3]) -> MouthMesh {
    let mut m = MouthMesh::default();
    let upper = pack.group_vertices("helper-upper-teeth");
    let lower = pack.group_vertices("helper-lower-teeth");
    if upper.is_empty() || lower.is_empty() {
        return m;
    }
    let up: Vec<Vec3> = upper.iter().map(|v| morphed[*v as usize]).collect();
    let lo: Vec<Vec3> = lower.iter().map(|v| morphed[*v as usize]).collect();
    let upper_arch = Arch::of(&up, true);
    // The lower front teeth bite just behind the upper ones and a little above their edges
    // (a normal overjet and overbite).
    let mut lower_arch = Arch::of(&lo, false);
    lower_arch.edge_y = upper_arch.edge_y + 0.0028;
    lower_arch.front_z = lower_arch.front_z.min(upper_arch.front_z - 0.0028);
    jaw(&mut m, &upper_arch, &up, &upper, &UPPER, true);
    jaw(&mut m, &lower_arch, &lo, &lower, &LOWER, false);
    tongue(&mut m, pack, morphed, (&lower_arch, &lo, &lower));
    m
}

/// The arch of a jaw from its helper band: its front, its half width and how far back it
/// reaches (at the occlusal side), and the heights of its gum line and biting edge.
struct Arch {
    center_x: f32,
    front_z: f32,
    back_z: f32,
    half_width: f32,
    gum_y: f32,
    edge_y: f32,
}

impl Arch {
    fn of(points: &[Vec3], upper: bool) -> Self {
        let min = points.iter().fold(Vec3::MAX, |a, p| a.min(*p));
        let max = points.iter().fold(Vec3::MIN, |a, p| a.max(*p));
        let (gum_y, edge_y) = if upper {
            // The upper front teeth show a little below the lip line.
            (max.y - 0.002, min.y - 0.0012)
        } else {
            (min.y + 0.002, max.y - 0.0035)
        };
        Self {
            center_x: (min.x + max.x) * 0.5,
            // The crowns' outer faces sit on the band's outer face.
            front_z: max.z - 0.0035,
            back_z: min.z + 0.004,
            half_width: (max.x - min.x) * 0.5 - 0.0035,
            gum_y,
            edge_y,
        }
    }

    fn point(&self, t: f32, side: f32) -> Vec3 {
        // A U: x grows fast at the front, z falls slowly then fast (past the back, it carries
        // on straight back).
        let signed = |v: f32, e: f32| v.signum() * v.abs().powf(e);
        let phi = t * std::f32::consts::FRAC_PI_2;
        let x = self.center_x + side * self.half_width * signed(phi.sin(), 0.8);
        let z = self.back_z + (self.front_z - self.back_z) * signed(phi.cos(), 1.25);
        Vec3::new(x, 0.0, z)
    }

    /// Point on the arch at parameter `t` (0 front center .. 1 the back, on side `side`), and
    /// the arch's outward direction there.
    fn at(&self, t: f32, side: f32) -> (Vec3, Vec3) {
        // A parabola-like U: x grows fast at the front, z falls slowly then fast.
        let p = self.point(t, side);
        let p2 = self.point(t + 0.01, side);
        let along = (p2 - p).normalize_or(Vec3::X * side);
        let out = Vec3::new(along.z, 0.0, -along.x) * -side;
        (p, out.normalize_or(Vec3::Z))
    }

    /// Arch parameter after walking `dist` (m) from `t` toward the back.
    fn walk(&self, t: f32, dist: f32) -> f32 {
        let mut t = t;
        let mut left = dist;
        let step = 0.002;
        while left > 0.0 && t < 1.2 {
            let (a, _) = self.at(t, 1.0);
            let (b, _) = self.at(t + step, 1.0);
            let d = (b - a).length().max(1e-6);
            if d >= left {
                return t + step * left / d;
            }
            left -= d;
            t += step;
        }
        t
    }
}

fn jaw(
    m: &mut MouthMesh,
    arch: &Arch,
    band: &[Vec3],
    ids: &[u32],
    teeth: &[Tooth; 7],
    upper: bool,
) {
    // Fit the row to the arch's length (the helper arch is a little longer than a real one).
    let mut arch_len = 0.0;
    let mut t = 0.0;
    while t < 1.0 {
        let (a, _) = arch.at(t, 1.0);
        let (b, _) = arch.at((t + 0.01).min(1.0), 1.0);
        arch_len += (b - a).length();
        t += 0.01;
    }
    let row: f32 = teeth.iter().map(|t| t.width).sum();
    let scale = (arch_len * 0.96 / row).clamp(0.8, 1.25);
    let dir = if upper { -1.0 } else { 1.0 };
    for side in [-1.0f32, 1.0] {
        let mut t = 0.0;
        for (k, tooth) in teeth.iter().enumerate() {
            let w = tooth.width * scale;
            let t_mid = arch.walk(t, w * 0.5);
            t = arch.walk(t, w);
            let (base, out) = arch.at(t_mid, side);
            let along = Vec3::Y.cross(out).normalize();
            // Back teeth are shorter and sit a little higher (the curve of Spee).
            let edge = arch.edge_y - dir * 0.0006 * k as f32;
            let neck = edge - dir * tooth.height;
            let gum = if upper {
                neck.min(arch.gum_y)
            } else {
                neck.max(arch.gum_y)
            };
            crown(m, base, out, along, edge, gum, w, tooth, dir, band, ids);
        }
        gum_band(m, arch, side, dir, band, ids);
    }
}

#[allow(clippy::too_many_arguments)]
fn crown(
    m: &mut MouthMesh,
    base: Vec3,
    out: Vec3,
    along: Vec3,
    edge_y: f32,
    neck_y: f32,
    width: f32,
    tooth: &Tooth,
    dir: f32,
    band: &[Vec3],
    ids: &[u32],
) {
    const AROUND: usize = 16;
    const RINGS: usize = 7;
    let first = m.positions.len() as u32;
    let center = base - out * (tooth.thickness * 0.5);
    for r in 0..RINGS {
        // s: 0 at the gum .. 1 at the biting edge.
        let s = r as f32 / (RINGS - 1) as f32;
        let y = neck_y + (edge_y - neck_y) * s;
        // Narrow at the neck, widest two thirds up; incisors thin to an edge, canines to a
        // point, back teeth round over their cusps.
        let bulge = (s * std::f32::consts::PI * 0.75).sin().max(0.0) * 0.15 + 0.85;
        let (wx, tz) = match tooth.shape {
            0 => (bulge, 1.0 - 0.75 * s * s),
            1 => (bulge * (1.0 - 0.55 * s.powi(3)), 1.0 - 0.45 * s * s),
            _ => (
                bulge * (1.0 - 0.15 * s.powi(4)),
                bulge * (1.0 - 0.15 * s.powi(4)),
            ),
        };
        for a in 0..AROUND {
            let ang = a as f32 / AROUND as f32 * std::f32::consts::TAU;
            // A rounded rectangle (a superellipse).
            let (c, sn) = (ang.cos(), ang.sin());
            let e = 0.55;
            let cx = c.signum() * c.abs().powf(e);
            let sz = sn.signum() * sn.abs().powf(e);
            let mut p = center
                + along * (cx * width * 0.5 * wx * 1.0)
                + out * (sz * tooth.thickness * 0.5 * tz);
            p.y = y;
            // A canine's point and a back tooth's cusps.
            if tooth.shape == 1 {
                p.y += dir * 0.0015 * (1.0 - cx.abs()) * s;
            } else if tooth.shape == 2 {
                p.y -= dir * 0.0008 * (cx * 2.0).cos().abs() * (sz * 2.0).cos().abs() * s * s;
            }
            push(m, p, MouthPart::Teeth, band, ids);
        }
    }
    // Close the biting end with a cap at its center.
    let cap = m.positions.len() as u32;
    let mut c = center;
    c.y = edge_y + if tooth.shape == 2 { -dir * 0.0006 } else { 0.0 };
    push(m, c, MouthPart::Teeth, band, ids);
    let quad = |m: &mut MouthMesh, a: u32, b: u32, c: u32, d: u32| {
        if dir < 0.0 {
            m.indices.extend([a, b, c, a, c, d]);
        } else {
            m.indices.extend([a, c, b, a, d, c]);
        }
    };
    for r in 0..RINGS as u32 - 1 {
        for a in 0..AROUND as u32 {
            let a1 = (a + 1) % AROUND as u32;
            let i = |r: u32, a: u32| first + r * AROUND as u32 + a;
            quad(m, i(r, a), i(r, a1), i(r + 1, a1), i(r + 1, a));
        }
    }
    let last = first + (RINGS as u32 - 1) * AROUND as u32;
    for a in 0..AROUND as u32 {
        let a1 = (a + 1) % AROUND as u32;
        if dir < 0.0 {
            m.indices.extend([last + a, last + a1, cap]);
        } else {
            m.indices.extend([last + a, cap, last + a1]);
        }
    }
    smooth_normals(m, first);
}

/// A rounded ridge of gum along the teeth's necks.
fn gum_band(m: &mut MouthMesh, arch: &Arch, side: f32, dir: f32, band: &[Vec3], ids: &[u32]) {
    const ALONG: usize = 24;
    const ACROSS: usize = 7;
    let first = m.positions.len() as u32;
    for i in 0..ALONG {
        let t = i as f32 / (ALONG - 1) as f32 * 1.02;
        let (p, out) = arch.at(t, side);
        for j in 0..ACROSS {
            // Across: from outside (lip side) over the ridge to inside (tongue side).
            let a = j as f32 / (ACROSS - 1) as f32 * std::f32::consts::PI;
            let mut q = p - out * 0.0045 + out * (a.cos() * 0.0062);
            q.y = arch.gum_y + dir * 0.0008 - dir * a.sin() * 0.0045;
            push(m, q, MouthPart::Gums, band, ids);
        }
    }
    for i in 0..ALONG as u32 - 1 {
        for j in 0..ACROSS as u32 - 1 {
            let k = |i: u32, j: u32| first + i * ACROSS as u32 + j;
            let (a, b, c, d) = (k(i, j), k(i + 1, j), k(i + 1, j + 1), k(i, j + 1));
            if (dir < 0.0) == (side > 0.0) {
                m.indices.extend([a, b, c, a, c, d]);
            } else {
                m.indices.extend([a, c, b, a, d, c]);
            }
        }
    }
    smooth_normals(m, first);
}

fn tongue(m: &mut MouthMesh, pack: &HumanPack, morphed: &[Vec3], jaw: (&Arch, &[Vec3], &[u32])) {
    let Some(g) = pack.group("helper-tongue") else {
        return;
    };
    let (arch, band, ids) = jaw;
    let quads: Vec<_> = pack.quads.iter().filter(|q| q.group == g).collect();
    // It lies in the floor of the mouth, its top just under the lower teeth's edges, and rides
    // the lower jaw (the expressions don't move the tongue helper with the jaw).
    // It also fits inside the lower arch, behind the teeth.
    let points: Vec<Vec3> = quads
        .iter()
        .flat_map(|q| q.v)
        .map(|v| morphed[v as usize])
        .collect();
    let min = points.iter().fold(Vec3::MAX, |a, p| a.min(*p));
    let max = points.iter().fold(Vec3::MIN, |a, p| a.max(*p));
    let drop = (max.y - (arch.edge_y - 0.0025)).max(0.0);
    let back = (max.z - (arch.front_z - 0.007)).max(0.0);
    let half = ((max.x - min.x) * 0.5).max(1e-4);
    let narrow = ((arch.half_width - 0.006) / half).min(1.0);
    let fit = |p: Vec3| {
        Vec3::new(
            arch.center_x + (p.x - arch.center_x) * narrow,
            p.y - drop,
            p.z - back,
        )
    };
    let first = m.positions.len() as u32;
    let mut remap = std::collections::HashMap::new();
    for q in quads {
        let mut idx = [0u32; 4];
        for (k, v) in q.v.iter().enumerate() {
            idx[k] = *remap.entry(*v).or_insert_with(|| {
                let p = fit(morphed[*v as usize]);
                push(m, p, MouthPart::Tongue, band, ids);
                m.positions.len() as u32 - 1
            });
        }
        m.indices
            .extend([idx[0], idx[1], idx[2], idx[0], idx[2], idx[3]]);
    }
    smooth_normals(m, first);
}

/// Add a vertex tied to the nearest three vertices of a helper band.
pub(crate) fn push(m: &mut MouthMesh, p: Vec3, part: MouthPart, band: &[Vec3], ids: &[u32]) {
    let mut best = [(f32::MAX, 0usize); 3];
    for (i, b) in band.iter().enumerate() {
        let d = (*b - p).length_squared();
        if d < best[2].0 {
            best[2] = (d, i);
            best.sort_by(|a, b| a.0.total_cmp(&b.0));
        }
    }
    let w: Vec<f32> = best.iter().map(|(d, _)| 1.0 / (d.sqrt() + 1e-4)).collect();
    let total: f32 = w.iter().sum();
    m.positions.push(p);
    m.normals.push(Vec3::Y);
    m.parts.push(part);
    m.anchors.push([
        (ids[best[0].1], w[0] / total),
        (ids[best[1].1], w[1] / total),
        (ids[best[2].1], w[2] / total),
    ]);
}

/// Normals of the vertices from `first` on, from the triangles that use them.
pub(crate) fn smooth_normals(m: &mut MouthMesh, first: u32) {
    let n = vertex_normals(&m.positions, &m.indices);
    let first = first as usize;
    m.normals[first..].copy_from_slice(&n[first..]);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pack::{HumanPack, Quad};
    use engine_core::math::Vec2;

    /// A helper band shaped like hm08's: an arch 62 mm wide and 55 mm deep.
    fn band(y0: f32, y1: f32) -> Vec<Vec3> {
        let mut v = Vec::new();
        for k in 0..=8 {
            let phi = k as f32 / 8.0 * std::f32::consts::PI - std::f32::consts::FRAC_PI_2;
            for y in [y0, y1] {
                v.push(Vec3::new(0.031 * phi.sin(), y, 0.097 + 0.054 * phi.cos()));
            }
        }
        v
    }

    #[test]
    fn two_rows_of_fourteen_teeth_fit_the_arch() {
        let up = band(0.66, 0.6834);
        let lo = band(0.6419, 0.6713);
        let mut positions = up.clone();
        positions.extend(&lo);
        let quad = |a: u32, g: u16| Quad {
            v: [a, a + 1, a + 3, a + 2],
            uv: [0; 4],
            group: g,
        };
        let n = up.len() as u32;
        let quads = vec![quad(0, 0), quad(n, 1)];
        let mut pack = HumanPack::new(
            positions.clone(),
            vec![Vec2::ZERO],
            vec!["helper-upper-teeth".into(), "helper-lower-teeth".into()],
            quads,
            vec![],
        );
        // Every band vertex in its group.
        pack.quads = (0..up.len() as u32 / 2 - 1)
            .map(|i| quad(i * 2, 0))
            .chain((0..lo.len() as u32 / 2 - 1).map(|i| quad(n + i * 2, 1)))
            .collect();
        let m = build(&pack, &positions);
        let teeth = m.parts.iter().filter(|p| **p == MouthPart::Teeth).count();
        // 28 crowns of 16 x 7 ring points and a cap.
        assert_eq!(teeth, 28 * (16 * 7 + 1));
        assert!(m.parts.contains(&MouthPart::Gums));
        // Everything inside the helper's box, give or take a few millimeters.
        for p in &m.positions {
            assert!(p.x.abs() < 0.036 && p.z > 0.09 && p.z < 0.155, "{p}");
            assert!(p.y > 0.63 && p.y < 0.695, "{p}");
        }
        // Upper front teeth come down past the lower front teeth's edges (an overbite).
        let front = |part_y: fn(f32, f32) -> bool| {
            m.positions
                .iter()
                .zip(&m.parts)
                .filter(|(p, part)| **part == MouthPart::Teeth && p.x.abs() < 0.004 && p.z > 0.14)
                .map(|(p, _)| p.y)
                .fold(
                    f32::NAN,
                    |a, y| if a.is_nan() || part_y(y, a) { y } else { a },
                )
        };
        let upper_edge = front(|y, a| y < a);
        assert!(upper_edge < 0.664, "{upper_edge}");
        for a in &m.anchors {
            let total: f32 = a.iter().map(|x| x.1).sum();
            assert!((total - 1.0).abs() < 1e-4);
        }
        // Every crown's normals point out of it.
        let crowns: Vec<(Vec3, Vec3)> = m
            .positions
            .iter()
            .zip(&m.normals)
            .zip(&m.parts)
            .filter(|(_, p)| **p == MouthPart::Teeth)
            .map(|((p, n), _)| (*p, *n))
            .collect();
        for (k, tooth) in crowns.chunks(113).enumerate() {
            let center = tooth.iter().map(|t| t.0).sum::<Vec3>() / 113.0;
            let out = tooth
                .iter()
                .filter(|(p, n)| n.dot(*p - center) > 0.0)
                .count();
            assert!(out > 100, "tooth {k}: {out} of 113 normals point out");
        }
        // The gums' normals face away from the teeth.
        let gums: Vec<(Vec3, Vec3)> = m
            .positions
            .iter()
            .zip(&m.normals)
            .zip(&m.parts)
            .filter(|(_, p)| **p == MouthPart::Gums)
            .map(|((p, n), _)| (*p, *n))
            .collect();
        let up = gums
            .iter()
            .filter(|(p, n)| (p.y > 0.67) == (n.y > 0.0))
            .count();
        assert!(up * 10 > gums.len() * 6, "{up} of {}", gums.len());
    }
}
