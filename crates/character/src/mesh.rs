//! Building a person's skin mesh: morph the base mesh, cut out the part to draw, subdivide it
//! into a smooth surface, and carry along what the shaders need per vertex: the rest position
//! (where skin detail is anchored), region masks (lips, brows, scalp, beard, …) and curvature
//! (how far light diffuses around a bend).

use std::collections::HashMap;

use engine_core::math::Vec3;

use crate::appearance::Appearance;
use crate::morph;
use crate::pack::HumanPack;
use crate::subdiv::QuadMesh;

/// How much of the body to build.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Region {
    /// Head and neck.
    Head,
    /// Head, neck, shoulders and upper chest (portraits, the creator).
    #[default]
    Bust,
}

impl Region {
    /// Lowest rest-space height kept, m (the base mesh's neck joint is at 0.589).
    fn cut(self) -> f32 {
        match self {
            Region::Head => 0.55,
            Region::Bust => 0.30,
        }
    }
}

/// Number of region mask channels per vertex (see [`Masks`]).
pub const MASKS: usize = 12;

/// Region masks, 0..1 per vertex, in channel order.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(usize)]
pub enum Mask {
    /// Red of the lips (vermilion).
    Lips = 0,
    /// Nose.
    Nose = 1,
    /// Eyelids and the skin around the eyes.
    Eyelids = 2,
    /// Where brow hairs grow.
    Brows = 3,
    /// Cheeks (they flush).
    Cheeks = 4,
    /// Where beard grows.
    Beard = 5,
    /// Where scalp hair grows.
    Scalp = 6,
    /// Ears.
    Ears = 7,
    /// Clothing (the shirt of a bust).
    Cloth = 8,
    /// Forehead.
    Forehead = 9,
    /// Chin.
    Chin = 10,
    /// The wet rim of the eyelids and the inner eye corner.
    LidRim = 11,
}

/// Rest-space landmarks of the base mesh (m), from its helper and joint groups.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Landmarks {
    /// Eye centers, left (+X) then right.
    pub eyes: [Vec3; 2],
    pub eye_radius: f32,
    /// Center of the head (between the ears, level with the eyes).
    pub head: Vec3,
    pub mouth: Vec3,
}

/// Per-vertex region masks and landmarks of a pack, computed once from its rest mesh and
/// targets (a lip-volume target moves the lips, so its weights outline them).
#[derive(Debug, Clone)]
pub struct Regions {
    /// `MASKS` floats per base vertex.
    pub masks: Vec<f32>,
    pub landmarks: Landmarks,
    /// Eyelid margins of each eye (left, right), each `[upper, lower]`: base vertices along
    /// the lid edge, inner corner to outer.
    pub lid_margins: [[Vec<u32>; 2]; 2],
}

fn centroid(pack: &HumanPack, verts: &[u32]) -> Option<Vec3> {
    if verts.is_empty() {
        return None;
    }
    let sum: Vec3 = verts.iter().map(|v| pack.positions[*v as usize]).sum();
    Some(sum / verts.len() as f32)
}

fn smoothstep(e0: f32, e1: f32, x: f32) -> f32 {
    let t = ((x - e0) / (e1 - e0)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// Piecewise-linear lookup in `(x, y)` pairs sorted by x.
fn lerp_table(table: &[(f32, f32)], x: f32) -> f32 {
    if x <= table[0].0 {
        return table[0].1;
    }
    for w in table.windows(2) {
        if x <= w[1].0 {
            let t = (x - w[0].0) / (w[1].0 - w[0].0);
            return w[0].1 + (w[1].1 - w[0].1) * t;
        }
    }
    table[table.len() - 1].1
}

/// Height of the scalp hairline above the head center at azimuth `theta` (0 straight ahead,
/// π behind; symmetric), m, for a full hairline (`recession` 0) or a receded one (1).
pub fn hairline(theta: f32, recession: f32) -> f32 {
    let t = theta.abs();
    let full = [
        (0.0, 0.054),
        (0.45, 0.048),
        (0.85, 0.034),
        (1.05, 0.004),
        (1.18, 0.0),
        (1.3, 0.032),
        (1.75, 0.034),
        (2.2, -0.022),
        (std::f32::consts::PI, -0.082),
    ];
    let receded = [
        (0.0, 0.098),
        (0.45, 0.104),
        (0.85, 0.07),
        (1.05, 0.02),
        (1.18, 0.012),
        (1.3, 0.034),
        (1.75, 0.034),
        (2.2, -0.022),
        (std::f32::consts::PI, -0.082),
    ];
    let a = lerp_table(&full, t);
    let b = lerp_table(&receded, t);
    a + (b - a) * recession.clamp(0.0, 1.0)
}

impl Regions {
    pub fn new(pack: &HumanPack) -> Self {
        let n = pack.positions.len();
        let mut masks = vec![0.0f32; n * MASKS];
        let eye = |side: &str| {
            let v = pack.group_vertices(&format!("helper-{side}-eye"));
            let c = centroid(pack, &v).unwrap_or(Vec3::new(
                if side == "l" { 0.031 } else { -0.031 },
                0.728,
                0.125,
            ));
            let r = v
                .iter()
                .map(|i| (pack.positions[*i as usize] - c).length())
                .fold(0.0f32, f32::max);
            (c, r)
        };
        let (le, lr) = eye("l");
        let (re, rr) = eye("r");
        let eye_radius = if lr > 0.0 { (lr + rr) * 0.5 } else { 0.0125 };
        let mouth = centroid(pack, &pack.group_vertices("helper-upper-teeth"))
            .map(|c| c + Vec3::new(0.0, -0.006, 0.03))
            .unwrap_or(Vec3::new(0.0, 0.668, 0.155));
        let head = Vec3::new(0.0, (le.y + re.y) * 0.5, 0.02);
        let landmarks = Landmarks {
            eyes: [le, re],
            eye_radius,
            head,
            mouth,
        };

        // Masks from how far targets move each vertex (normalized per target, the strongest
        // target of a set wins).
        let mut from_targets = |channel: Mask, names: &[&str], lo: f32, hi: f32| {
            let mut best = vec![0.0f32; n];
            for name in names {
                let Some(t) = pack.target(name) else {
                    continue;
                };
                let peak = t.deltas.iter().map(|d| d.length()).fold(0.0f32, f32::max);
                if peak <= 0.0 {
                    continue;
                }
                for (i, d) in t.indices.iter().zip(&t.deltas) {
                    let w = d.length() / peak;
                    let b = &mut best[*i as usize];
                    *b = b.max(w);
                }
            }
            for (v, b) in best.iter().enumerate() {
                masks[v * MASKS + channel as usize] = smoothstep(lo, hi, *b);
            }
        };
        from_targets(
            Mask::Lips,
            &[
                "mouth/mouth-upperlip-volume-incr",
                "mouth/mouth-lowerlip-volume-incr",
            ],
            0.3,
            0.7,
        );
        from_targets(
            Mask::Nose,
            &[
                "nose/nose-volume-incr",
                "nose/nose-scale-horiz-incr",
                "nose/nose-point-width-incr",
                "nose/nose-flaring-incr",
            ],
            0.15,
            0.6,
        );
        from_targets(
            Mask::Eyelids,
            &[
                "eyes/l-eye-height2-incr",
                "eyes/r-eye-height2-incr",
                "eyes/l-eye-bag-incr",
                "eyes/r-eye-bag-incr",
            ],
            0.1,
            0.6,
        );
        from_targets(
            Mask::Cheeks,
            &["cheek/l-cheek-volume-incr", "cheek/r-cheek-volume-incr"],
            0.05,
            0.6,
        );
        from_targets(
            Mask::Ears,
            &["ears/l-ear-scale-incr", "ears/r-ear-scale-incr"],
            0.05,
            0.3,
        );
        from_targets(
            Mask::Forehead,
            &["forehead/forehead-scale-vert-incr"],
            0.1,
            0.6,
        );
        from_targets(Mask::Chin, &["chin/chin-prominent-incr"], 0.1, 0.6);

        // Geometric regions, in rest space around the landmarks.
        for v in 0..n {
            let p = pack.positions[v];
            let m = &mut masks[v * MASKS..(v + 1) * MASKS];
            let d = p - head;
            let theta = d.x.atan2(d.z);
            // Brows: an arch above each eye.
            let mut brow = 0.0f32;
            for e in [le, re] {
                let side = e.x.signum();
                let u = (p.x * side - (e.x * side - 0.017)) / 0.041;
                if (-0.1..1.1).contains(&u) && p.z > e.z - 0.01 {
                    let center =
                        e.y + lerp_table(&[(0.0, 0.0165), (0.6, 0.0205), (1.0, 0.0165)], u);
                    let half = lerp_table(&[(0.0, 0.0058), (0.35, 0.0050), (1.0, 0.0022)], u);
                    let across = 1.0 - smoothstep(half * 0.6, half * 1.25, (p.y - center).abs());
                    let along = smoothstep(-0.1, 0.05, u) * (1.0 - smoothstep(0.95, 1.1, u));
                    brow = brow.max(across * along);
                }
            }
            m[Mask::Brows as usize] = brow;
            // Scalp: above the hairline, on the head.
            let line = hairline(theta, 0.0);
            m[Mask::Scalp as usize] = smoothstep(line - 0.004, line + 0.006, d.y)
                * smoothstep(0.55, 0.6, p.y)
                * (1.0 - m[Mask::Ears as usize]);
            // Beard: jaw, chin, upper lip and the throat below the jaw; not the cheekbones.
            let ax = p.x.abs();
            let top = lerp_table(
                &[
                    (0.0, 0.69),
                    (0.02, 0.686),
                    (0.045, 0.672),
                    (0.065, 0.69),
                    (0.075, 0.715),
                ],
                ax,
            );
            let front = smoothstep(-0.02, 0.03, p.z);
            // The neckline: a finger above the Adam's apple, rising to the jaw's corners.
            let bottom = lerp_table(
                &[(0.0, 0.604), (0.03, 0.608), (0.05, 0.618), (0.07, 0.635)],
                ax,
            );
            let beard = smoothstep(top + 0.004, top - 0.004, p.y)
                * smoothstep(bottom - 0.006, bottom + 0.008, p.y)
                * front
                * (1.0 - m[Mask::Lips as usize])
                * (1.0 - smoothstep(0.0, 0.004, d.y - 0.0) * smoothstep(0.06, 0.07, ax));
            m[Mask::Beard as usize] = beard;
            // A crew-neck shirt below the collar line.
            let neckline = 0.565 - 0.03 * (theta.cos() * 0.5 + 0.5);
            m[Mask::Cloth as usize] = smoothstep(neckline + 0.004, neckline - 0.004, p.y);
        }

        // The wet rim of the lids: skin within a couple of millimeters of the lid margins.
        let lid_margins = lid_margins(pack, [le, re]);
        let margin_points: Vec<Vec3> = lid_margins
            .iter()
            .flatten()
            .flatten()
            .map(|v| pack.positions[*v as usize])
            .collect();
        if let Some(body) = pack.group("body") {
            let mut is_body = vec![false; n];
            for q in pack.quads.iter().filter(|q| q.group == body) {
                for v in q.v {
                    is_body[v as usize] = true;
                }
            }
            for (v, p) in pack.positions.iter().enumerate() {
                if !is_body[v] || ((*p - le).length() > 0.03 && (*p - re).length() > 0.03) {
                    continue;
                }
                let d = margin_points
                    .iter()
                    .map(|m| (*m - *p).length())
                    .fold(f32::MAX, f32::min);
                masks[v * MASKS + Mask::LidRim as usize] = 1.0 - smoothstep(0.0007, 0.0018, d);
            }
        }
        Self {
            masks,
            landmarks,
            lid_margins,
        }
    }
}

/// The lid margins of each eye, from the base mesh's eyelash cards (one strip on each lid):
/// the row of each card nearest the eye is its root along the lid edge.
fn lid_margins(pack: &HumanPack, eyes: [Vec3; 2]) -> [[Vec<u32>; 2]; 2] {
    let mut out: [[Vec<u32>; 2]; 2] = Default::default();
    for (e, side) in ["l", "r"].iter().enumerate() {
        let eye = eyes[e];
        let sx = if *side == "l" { 1.0 } else { -1.0 };
        let mut cards: Vec<(f32, Vec<u32>)> = Vec::new();
        for k in 1..=2 {
            let verts = pack.group_vertices(&format!("helper-{side}-eyelashes-{k}"));
            if verts.is_empty() {
                continue;
            }
            let dist = |v: &u32| (pack.positions[*v as usize] - eye).length();
            let nearest = verts.iter().map(dist).fold(f32::MAX, f32::min);
            let mut root: Vec<u32> = verts
                .iter()
                .copied()
                .filter(|v| dist(v) < nearest + 0.0012)
                .collect();
            root.sort_by(|a, b| {
                let xa = pack.positions[*a as usize].x * sx;
                let xb = pack.positions[*b as usize].x * sx;
                xa.total_cmp(&xb)
            });
            root.dedup_by(|a, b| {
                (pack.positions[*a as usize] - pack.positions[*b as usize]).length() < 1e-5
            });
            let y = root
                .iter()
                .map(|v| pack.positions[*v as usize].y)
                .sum::<f32>()
                / root.len().max(1) as f32;
            cards.push((y, root));
        }
        // The higher card is the upper lid's.
        cards.sort_by(|a, b| b.0.total_cmp(&a.0));
        for (i, (_, root)) in cards.into_iter().take(2).enumerate() {
            out[e][i] = root;
        }
    }
    out
}

/// Floats per vertex while subdividing: position, rest position, masks.
const STRIDE: usize = 6 + MASKS;

/// A built skin surface, ready to upload.
#[derive(Debug, Clone, Default)]
pub struct SkinMesh {
    pub positions: Vec<Vec3>,
    pub normals: Vec<Vec3>,
    /// Rest-space position (the base mesh before morphs), where skin detail is anchored.
    pub rest: Vec<Vec3>,
    /// `MASKS` floats per vertex.
    pub masks: Vec<f32>,
    /// Mean curvature, 1/m (positive where the surface bends away from the normal: a nose
    /// tip, an ear rim).
    pub curvature: Vec<f32>,
    pub indices: Vec<u32>,
}

/// Morph the base mesh for `a`, cut out `region` of the `body` group, and subdivide it
/// `levels` times (2 for close-ups). Returns the morphed base positions too (helpers such as
/// the eyes and teeth are placed from them).
pub fn build_skin(
    pack: &HumanPack,
    regions: &Regions,
    a: &Appearance,
    region: Region,
    levels: u32,
) -> (SkinMesh, Vec<Vec3>) {
    let morphed = morph::morph(pack, a);
    let skin = build_from(pack, regions, &morphed, region, levels);
    (skin, morphed)
}

/// [`build_skin`] from already morphed base positions.
pub fn build_from(
    pack: &HumanPack,
    regions: &Regions,
    morphed: &[Vec3],
    region: Region,
    levels: u32,
) -> SkinMesh {
    let body = pack.group("body").unwrap_or(0);
    let cut = region.cut();
    let mut remap: HashMap<u32, u32> = HashMap::new();
    let mut attrs = Vec::new();
    let mut quads = Vec::new();
    for q in pack.quads.iter().filter(|q| q.group == body) {
        if q.v.iter().any(|v| pack.positions[*v as usize].y < cut) {
            continue;
        }
        let mut out = [0u32; 4];
        for (k, v) in q.v.iter().enumerate() {
            out[k] = *remap.entry(*v).or_insert_with(|| {
                let i = *v as usize;
                attrs.extend(morphed[i].to_array());
                attrs.extend(pack.positions[i].to_array());
                attrs.extend(&regions.masks[i * MASKS..(i + 1) * MASKS]);
                (attrs.len() / STRIDE - 1) as u32
            });
        }
        quads.push(out);
    }
    let mut mesh = QuadMesh {
        quads,
        attrs,
        stride: STRIDE,
    };
    for _ in 0..levels {
        mesh = mesh.subdivide();
    }
    let nv = mesh.vertex_count();
    let mut out = SkinMesh {
        positions: Vec::with_capacity(nv),
        rest: Vec::with_capacity(nv),
        masks: Vec::with_capacity(nv * MASKS),
        ..Default::default()
    };
    for i in 0..nv {
        let v = mesh.vertex(i);
        out.positions.push(Vec3::new(v[0], v[1], v[2]));
        out.rest.push(Vec3::new(v[3], v[4], v[5]));
        out.masks.extend(&v[6..]);
    }
    out.indices = Vec::with_capacity(mesh.quads.len() * 6);
    for q in &mesh.quads {
        // The base mesh winds its quads counter-clockwise seen from outside, and so does
        // subdivision.
        out.indices.extend([q[0], q[1], q[2], q[0], q[2], q[3]]);
    }
    out.normals = vertex_normals(&out.positions, &out.indices);
    out.curvature = curvature(&out.positions, &out.normals, &mesh.quads);
    out
}

/// Area-weighted vertex normals of a triangle list.
pub fn vertex_normals(positions: &[Vec3], indices: &[u32]) -> Vec<Vec3> {
    let mut n = vec![Vec3::ZERO; positions.len()];
    for t in indices.chunks_exact(3) {
        let (a, b, c) = (t[0] as usize, t[1] as usize, t[2] as usize);
        let f = (positions[b] - positions[a]).cross(positions[c] - positions[a]);
        n[a] += f;
        n[b] += f;
        n[c] += f;
    }
    n.iter().map(|v| v.normalize_or(Vec3::Y)).collect()
}

/// Mean curvature per vertex from how the normal turns along each quad edge, 1/m.
fn curvature(positions: &[Vec3], normals: &[Vec3], quads: &[[u32; 4]]) -> Vec<f32> {
    let mut sum = vec![0.0f32; positions.len()];
    let mut count = vec![0u32; positions.len()];
    for q in quads {
        for k in 0..4 {
            let (a, b) = (q[k] as usize, q[(k + 1) % 4] as usize);
            let dp = positions[b] - positions[a];
            let len2 = dp.length_squared();
            if len2 < 1e-12 {
                continue;
            }
            // Positive when the normals spread apart along the edge (a convex bump).
            let k_ab = (normals[b] - normals[a]).dot(dp) / len2;
            sum[a] += k_ab;
            sum[b] += k_ab;
            count[a] += 1;
            count[b] += 1;
        }
    }
    sum.iter()
        .zip(&count)
        .map(|(s, c)| if *c > 0 { s / *c as f32 } else { 0.0 })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hairline_is_high_in_front_low_behind_and_recedes() {
        assert!(hairline(0.0, 0.0) > 0.05);
        assert!(hairline(std::f32::consts::PI, 0.0) < -0.05);
        assert!(hairline(0.4, 1.0) > hairline(0.4, 0.0));
        assert_eq!(hairline(-0.7, 0.3), hairline(0.7, 0.3));
    }

    #[test]
    fn curvature_of_a_sphere_is_its_inverse_radius() {
        // A subdivided cube projected onto a 0.1 m sphere.
        let mut m = QuadMesh {
            quads: vec![
                [0, 3, 2, 1],
                [4, 5, 6, 7],
                [0, 1, 5, 4],
                [2, 3, 7, 6],
                [1, 2, 6, 5],
                [0, 4, 7, 3],
            ],
            attrs: [
                [-1.0f32, -1.0, -1.0],
                [1.0, -1.0, -1.0],
                [1.0, 1.0, -1.0],
                [-1.0, 1.0, -1.0],
                [-1.0, -1.0, 1.0],
                [1.0, -1.0, 1.0],
                [1.0, 1.0, 1.0],
                [-1.0, 1.0, 1.0],
            ]
            .iter()
            .flatten()
            .copied()
            .collect(),
            stride: 3,
        };
        for _ in 0..3 {
            m = m.subdivide();
        }
        let p: Vec<Vec3> = (0..m.vertex_count())
            .map(|i| Vec3::from_slice(m.vertex(i)).normalize() * 0.1)
            .collect();
        let n: Vec<Vec3> = p.iter().map(|v| v.normalize()).collect();
        let k = curvature(&p, &n, &m.quads);
        for c in k {
            assert!((c - 10.0).abs() < 0.5, "{c}");
        }
    }
}
