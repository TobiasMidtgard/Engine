//! Building a person's skin mesh: morph the base mesh, cut out the part to draw, subdivide it
//! into a smooth surface, and carry along what the shaders need per vertex: the rest position
//! (where skin detail is anchored), region masks (lips, brows, scalp, beard, …) and curvature
//! (how far light diffuses around a bend).

use std::collections::HashMap;

use engine_core::math::{Vec2, Vec3};

use crate::appearance::{Appearance, Rng};
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
    /// The whole body, dressed (people in the world).
    Body,
}

impl Region {
    /// Lowest rest-space height kept, m (the base mesh's neck joint is at 0.589).
    fn cut(self) -> f32 {
        match self {
            Region::Head => 0.55,
            Region::Bust => 0.30,
            Region::Body => f32::NEG_INFINITY,
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
    /// Clothing: 1 where clothes may cover (all but the hands); the shaders cut the shirt's
    /// neckline per pixel and pick shirt, trousers or shoes from the height.
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

pub(crate) fn smoothstep(e0: f32, e1: f32, x: f32) -> f32 {
    let t = ((x - e0) / (e1 - e0)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// Piecewise-linear lookup in `(x, y)` pairs sorted by x.
pub(crate) fn lerp_table(table: &[(f32, f32)], x: f32) -> f32 {
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

/// The brow arch over each eye, by `u` along it (0 at its inner end, 0.017 m inward of the eye's
/// center, to 1 at the tail, [`BROW_LENGTH`] further out): its middle's height over the eye and
/// half its height, m.
pub(crate) const BROW_CENTER: [(f32, f32); 3] = [(0.0, 0.0165), (0.6, 0.0205), (1.0, 0.0165)];
pub(crate) const BROW_HALF: [(f32, f32); 3] = [(0.0, 0.0058), (0.35, 0.0050), (1.0, 0.0022)];
pub(crate) const BROW_INNER: f32 = 0.017;
pub(crate) const BROW_LENGTH: f32 = 0.041;

/// The ear's outline from the side, rest space from the head's center: center (height, depth)
/// and half its height and depth, m.
const EAR_CENTER: Vec2 = Vec2::new(-0.016, 0.03);
const EAR_HALF: Vec2 = Vec2::new(0.034, 0.022);

/// Height of the scalp hairline above the head center at azimuth `theta` (0 straight ahead,
/// π behind; symmetric), m, for a full hairline (`recession` 0) or a receded one (1).
pub fn hairline(theta: f32, recession: f32) -> f32 {
    let t = theta.abs();
    // Around the side: the temple point (where the line from the front hairline to the tragus
    // crosses the one from the nose tip over the pupil), the sideburn down the front of the ear
    // (the ear, about 1.17..1.68 here, is left out by the scalp mask), half a centimetre over
    // the ear's rim, and behind it down to about the earlobe before the nape.
    let full = [
        (0.0, 0.054),
        (0.45, 0.048),
        (0.62, 0.046),
        (0.78, 0.026),
        (0.95, 0.012),
        (1.1, 0.0),
        (1.18, -0.006),
        (1.3, 0.018),
        (1.45, 0.022),
        (1.62, 0.018),
        (1.8, -0.015),
        (2.05, -0.045),
        (2.4, -0.06),
        (std::f32::consts::PI, -0.082),
    ];
    let receded = [
        (0.0, 0.098),
        (0.45, 0.104),
        (0.85, 0.07),
        (1.05, 0.02),
        (1.18, 0.012),
        (1.3, 0.022),
        (1.45, 0.024),
        (1.62, 0.02),
        (1.8, -0.015),
        (2.05, -0.045),
        (2.4, -0.06),
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
            best
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
        // How far the ear target moves each vertex (it moves the skin around the ear too).
        let ear_moves = from_targets(
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
                let u = (p.x * side - (e.x * side - BROW_INNER)) / BROW_LENGTH;
                if (-0.1..1.1).contains(&u) && p.z > e.z - 0.01 {
                    let center = e.y + lerp_table(&BROW_CENTER, u);
                    let half = lerp_table(&BROW_HALF, u);
                    let across = 1.0 - smoothstep(half * 0.6, half * 1.25, (p.y - center).abs());
                    let along = smoothstep(-0.1, 0.05, u) * (1.0 - smoothstep(0.95, 1.1, u));
                    brow = brow.max(across * along);
                }
            }
            m[Mask::Brows as usize] = brow;
            // Scalp: above the hairline, on the head.
            let line = hairline(theta, 0.0);
            // Hair grows up to the ear, not just near it: only the ear itself is left out (the
            // ear target moves the skin around it too, over the temple and the sideburn): what
            // moves and lies within the ear's outline (rest space, from its vertices).
            let outline = Vec2::new(
                (d.y - EAR_CENTER.x) / EAR_HALF.x,
                (d.z - EAR_CENTER.y) / EAR_HALF.y,
            );
            let ear = smoothstep(0.3, 0.45, ear_moves[v])
                * (1.0 - smoothstep(0.9, 1.1, outline.length()));
            m[Mask::Scalp as usize] = smoothstep(line - 0.004, line + 0.006, d.y)
                * smoothstep(0.55, 0.6, p.y)
                * (1.0 - ear);
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
            // Clothes everywhere but the hands; the shaders cut the neckline per pixel
            // (`collar_depth` in common.wgsl).
            m[Mask::Cloth as usize] = 1.0 - bare_hands(pack, p);
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

/// 1 on the hands, past the shirt's cuffs (0 without a rig, which places the wrists).
fn bare_hands(pack: &HumanPack, p: Vec3) -> f32 {
    let joint = |name: &str| {
        let b = pack.bone(name)?;
        centroid(pack, pack.joints.get(b)?)
    };
    let mut bare = 0.0f32;
    for side in ["l", "r"] {
        let (Some(elbow), Some(wrist)) = (
            joint(&format!("lower_arm_{side}")),
            joint(&format!("hand_{side}")),
        ) else {
            continue;
        };
        // Past the cuff, within a hand's length of the wrist.
        let along = (p - wrist).dot((wrist - elbow).normalize_or_zero());
        let near = 1.0 - smoothstep(0.2, 0.24, (p - wrist).length());
        bare = bare.max(smoothstep(-0.012, -0.006, along) * near);
    }
    bare
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

/// Floats per vertex while subdividing: position, rest position, masks (then one weight per
/// bone when the pack has a rig).
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
    /// Per vertex: the four bones (indices into the pack's bones) that bend it most and their
    /// weights out of 255 (empty when the pack has no rig).
    pub bones: Vec<[u8; 4]>,
    pub weights: Vec<[u8; 4]>,
    /// The cut-out base mesh before subdivision: its quads over `base` (pack vertex per
    /// vertex), and how many times it was subdivided. Anything defined on the pack's vertices
    /// (expression shapes) is subdivided the same way from these.
    pub base: Vec<u32>,
    pub base_quads: Vec<[u32; 4]>,
    pub levels: u32,
    /// Vertices after the subdivided skin (the mouth: teeth, gums, tongue), each following
    /// three pack vertices (and weights), in order at the end of the vertex lists.
    pub extra: Vec<[(u32, f32); 3]>,
    /// Per extra vertex: what it is (see [`crate::mouth::MouthPart`]); skin vertices are 0.
    pub extra_parts: Vec<u8>,
    /// Triangles for seeing the person from afar: the base mesh's own quads over the
    /// subdivided vertices (Catmull-Clark keeps each base vertex at its index), clothed where
    /// all three corners are, and the shoes, skirt and hood; no mouth. Empty when the skin
    /// isn't subdivided.
    pub coarse_indices: Vec<u32>,
}

impl SkinMesh {
    /// Start [`Self::coarse_indices`] from the base quads; `cloth`: per skin vertex the
    /// clothes vertex over it (`clothes::dress`).
    pub fn build_coarse(&mut self, cloth: &[u32]) {
        self.coarse_indices.clear();
        if self.levels == 0 {
            return;
        }
        let over = |v: u32| cloth.get(v as usize).copied().unwrap_or(u32::MAX);
        for q in &self.base_quads {
            for t in [[q[0], q[1], q[2]], [q[0], q[2], q[3]]] {
                if t.iter().all(|v| over(*v) != u32::MAX) {
                    self.coarse_indices.extend(t.map(over));
                } else {
                    self.coarse_indices.extend(t);
                }
            }
        }
    }

    /// Append a part (shoes, a skirt, a hood) to the skin and to its far-away triangles.
    pub fn add_part(&mut self, pack: &HumanPack, part: &crate::mouth::MouthMesh) {
        let before = self.indices.len();
        self.add_mouth(pack, part);
        if !self.coarse_indices.is_empty() {
            let added = self.indices[before..].to_vec();
            self.coarse_indices.extend(added);
        }
    }

    /// Vertices of the subdivided skin (the rest are [`Self::extra`]).
    pub fn skin_vertices(&self) -> usize {
        self.positions.len() - self.extra.len()
    }

    /// Append the mouth (after the hair has grown, so no hair roots on the teeth).
    pub fn add_mouth(&mut self, pack: &HumanPack, mouth: &crate::mouth::MouthMesh) {
        let first = self.positions.len() as u32;
        for (i, p) in mouth.positions.iter().enumerate() {
            let a = mouth.anchors[i];
            self.positions.push(*p);
            self.normals.push(mouth.normals[i]);
            self.rest.push(
                a.iter()
                    .map(|(v, w)| pack.positions[*v as usize] * *w)
                    .sum(),
            );
            self.masks.extend([0.0; MASKS]);
            self.curvature.push(0.0);
            if !self.bones.is_empty() {
                let s = pack.skin.get(a[0].0 as usize).copied().unwrap_or_default();
                self.bones.push(s.bones);
                self.weights.push(s.weights);
            }
            self.extra.push(a);
            self.extra_parts.push(mouth.parts[i] as u8);
        }
        self.indices.extend(mouth.indices.iter().map(|i| i + first));
    }

    /// Subdivide per-pack-vertex values (`channels` floats each) the way the skin was, giving
    /// `channels` floats per skin vertex.
    pub fn subdivide_values(&self, channels: usize, value: impl Fn(usize, &mut [f32])) -> Vec<f32> {
        let mut attrs = vec![0.0f32; self.base.len() * channels];
        for (i, v) in self.base.iter().enumerate() {
            value(*v as usize, &mut attrs[i * channels..(i + 1) * channels]);
        }
        let mut mesh = QuadMesh {
            quads: self.base_quads.clone(),
            attrs,
            stride: channels,
        };
        for _ in 0..self.levels {
            mesh = mesh.subdivide();
        }
        mesh.attrs
    }
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
    let mut morphed = morph::morph(pack, a);
    seal_lips(pack, regions, &mut morphed);
    asymmetry(pack, regions, a, &mut morphed);
    let skin = build_from(pack, regions, &morphed, region, levels);
    (skin, morphed)
}

/// Make the face a little uneven, the same way every time for the same seed: no real face is
/// mirrored. One eye sits higher (with its lids, brow and the eyeball), one brow higher still,
/// the nose tip and chin lean to a side, one mouth corner is higher, one cheek fuller and one
/// ear higher. Magnitudes are around the population means in anthropometry (eye and brow
/// heights differ by about 0.5 to 2 mm, the chin and nose tip lean about 1 mm; 3 to 4 mm is
/// where people start to notice), growing somewhat with age. Shading reads rest space, so pores,
/// lip outlines and brows move with the skin.
pub fn asymmetry(pack: &HumanPack, regions: &Regions, a: &Appearance, morphed: &mut [Vec3]) {
    let mut r = Rng::new(a.seed ^ 0xA5F1_3E2D_77C0_19B4);
    let age = 0.8 + 0.4 * smoothstep(20.0, 75.0, a.age);
    let mut draw = |mean_mm: f32, sigma_mm: f32, max_mm: f32| {
        (mean_mm + r.normal() * sigma_mm).clamp(-max_mm, max_mm) * 0.001 * age
    };
    // Each the left side (+X) against the right, mm: left eye, brow, mouth corner and ear up,
    // left cheek out, nose tip and chin toward the left. The right half of the face is wider in
    // about four of five people and the chin leans left as often (Haraguchi 2008).
    let eye = draw(0.0, 0.8, 1.8);
    let brow = draw(0.0, 1.1, 2.4);
    let nose = draw(0.0, 0.7, 1.6);
    let chin = draw(0.6, 1.0, 3.0);
    let corner = draw(0.0, 0.8, 1.8);
    let cheek = draw(-0.5, 0.6, 1.5);
    let ear = draw(0.0, 2.0, 4.0);
    let lm = regions.landmarks;
    let mask = |v: usize, m: Mask| regions.masks[v * MASKS + m as usize];
    let mut helper = vec![false; morphed.len()];
    for side in ["l", "r"] {
        for v in pack.group_vertices(&format!("helper-{side}-eye")) {
            helper[v as usize] = true;
        }
    }
    for i in 0..morphed.len().min(pack.positions.len()) {
        let p = pack.positions[i];
        let d = p - lm.head;
        // Only the head: nothing below the jaw's underside, nothing behind the ears.
        if d.y < -0.17 || d.z < -0.13 {
            continue;
        }
        let mut shift = Vec3::ZERO;
        for (k, e) in lm.eyes.iter().enumerate() {
            let sign = if k == 0 { 0.5 } else { -0.5 };
            let near = 1.0 - smoothstep(0.018, 0.036, (p - *e).length());
            shift.y += sign * eye * near;
            if helper[i] {
                continue;
            }
            // The brow: above the lid crease, over the eye's width and a little past it.
            let out = (p.x - e.x) * e.x.signum();
            let over = smoothstep(e.y + 0.008, e.y + 0.016, p.y)
                * (1.0 - smoothstep(0.03, 0.045, p.y - e.y))
                * (1.0 - smoothstep(0.022, 0.04, out.abs()))
                * smoothstep(e.z - 0.03, e.z - 0.005, p.z);
            shift.y += sign * brow * over;
            // The cheek below the eye fills out sideways.
            let c = *e + Vec3::new(0.012 * e.x.signum(), -0.035, -0.005);
            let full = (-(p - c).length_squared() / (2.0 * 0.018 * 0.018)).exp();
            shift.x += sign * cheek * full * e.x.signum();
            // The ear on this side sits higher or lower.
            if d.x * e.x > 0.0 {
                shift.y += sign * ear * mask(i, Mask::Ears);
            }
        }
        if !helper[i] {
            let m = p - lm.mouth;
            // The nose tip leans; its bridge stays where the eyes are.
            let down = smoothstep(lm.eyes[0].y - 0.005, lm.eyes[0].y - 0.035, p.y);
            shift.x += nose * mask(i, Mask::Nose) * down;
            // The chin and jaw lean, from nothing at the lips.
            let jaw = smoothstep(LIP_LINE - 0.006, LIP_LINE - 0.03, m.y)
                * (1.0 - smoothstep(0.035, 0.07, m.x.abs()));
            shift.x += chin * jaw.max(mask(i, Mask::Chin));
            // One mouth corner higher, both lips together
            // (flat across the lips' height, so the line where they meet keeps sealed).
            let at = |x: f32| (-(m.x - x).powi(2) / (2.0 * 0.01 * 0.01)).exp();
            let lift = (at(0.023) - at(-0.023))
                * (1.0 - smoothstep(0.008, 0.02, (m.y - LIP_LINE).abs()))
                * smoothstep(-0.035, -0.015, m.z);
            shift.y += corner * lift * 0.5;
        }
        morphed[i] += shift;
    }
}

/// Where the base mesh's lips part, m below the mouth landmark (the shaders' `LIP_LINE`).
pub const LIP_LINE: f32 = -0.0071;

/// Close the base mesh's lips: at rest they stand almost a millimeter apart, which shows a
/// row of teeth through the slit. The lips are drawn together onto the line where they part,
/// less toward the corners (which already meet); expressions still open them from there.
pub fn seal_lips(pack: &HumanPack, regions: &Regions, morphed: &mut [Vec3]) {
    let Some(body) = pack.group("body") else {
        return;
    };
    let mouth = regions.landmarks.mouth;
    let mut seen = vec![false; morphed.len()];
    for q in pack.quads.iter().filter(|q| q.group == body) {
        for v in q.v {
            let i = v as usize;
            if std::mem::replace(&mut seen[i], true) {
                continue;
            }
            let r = pack.positions[i] - mouth;
            let d = r.y - LIP_LINE;
            if r.x.abs() > 0.026 || d.abs() > 0.005 || r.z < -0.012 {
                continue;
            }
            let reach = 1.0 - smoothstep(0.021, 0.026, r.x.abs());
            let pull = (0.0007 * (1.0 - smoothstep(0.0008, 0.005, d.abs())) * reach)
                .min((d.abs() - 0.00002).max(0.0));
            morphed[i].y -= pull * d.signum();
        }
    }
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
    let bones = if pack.has_rig() { pack.bones.len() } else { 0 };
    let stride = STRIDE + bones;
    let mut remap: HashMap<u32, u32> = HashMap::new();
    let mut attrs = Vec::new();
    let mut base = Vec::new();
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
                if bones > 0 {
                    let mut w = vec![0.0f32; bones];
                    let s = pack.skin[i];
                    for k in 0..4 {
                        w[s.bones[k] as usize] += s.weights[k] as f32 / 255.0;
                    }
                    attrs.extend(w);
                }
                base.push(*v);
                (attrs.len() / stride - 1) as u32
            });
        }
        quads.push(out);
    }
    let base_quads = quads.clone();
    let mut mesh = QuadMesh {
        quads,
        attrs,
        stride,
    };
    for _ in 0..levels {
        mesh = mesh.subdivide();
    }
    let nv = mesh.vertex_count();
    let mut out = SkinMesh {
        positions: Vec::with_capacity(nv),
        rest: Vec::with_capacity(nv),
        masks: Vec::with_capacity(nv * MASKS),
        base,
        base_quads,
        levels,
        ..Default::default()
    };
    for i in 0..nv {
        let v = mesh.vertex(i);
        out.positions.push(Vec3::new(v[0], v[1], v[2]));
        out.rest.push(Vec3::new(v[3], v[4], v[5]));
        out.masks.extend(&v[6..STRIDE]);
        if bones > 0 {
            let (b, w) = top_four(&v[STRIDE..]);
            out.bones.push(b);
            out.weights.push(w);
        }
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

/// The four largest of per-bone weights, renormalized to sum to 255.
pub fn top_four(weights: &[f32]) -> ([u8; 4], [u8; 4]) {
    let mut best = [(0usize, 0.0f32); 4];
    for (b, w) in weights.iter().enumerate() {
        if *w <= best[3].1 {
            continue;
        }
        let mut k = 3;
        while k > 0 && *w > best[k - 1].1 {
            best[k] = best[k - 1];
            k -= 1;
        }
        best[k] = (b, *w);
    }
    let total: f32 = best.iter().map(|b| b.1).sum();
    if total <= 0.0 {
        return ([0; 4], [0; 4]);
    }
    let mut w = best.map(|b| (b.1 / total * 255.0).round() as i32);
    w[0] += 255 - w.iter().sum::<i32>();
    (best.map(|b| b.0 as u8), w.map(|x| x.clamp(0, 255) as u8))
}

/// Area-weighted vertex normals of a triangle list.
pub fn vertex_normals(positions: &[Vec3], indices: &[u32]) -> Vec<Vec3> {
    let mut n = vec![Vec3::ZERO; positions.len()];
    for t in indices.as_chunks::<3>().0 {
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
    fn the_four_largest_weights_are_kept_and_sum_to_255() {
        let (b, w) = top_four(&[0.1, 0.0, 0.5, 0.05, 0.3, 0.05]);
        assert_eq!(b[..3], [2, 4, 0]);
        assert_eq!(w.iter().map(|x| *x as u32).sum::<u32>(), 255);
        assert!(w[0] > w[1] && w[1] > w[2]);
        assert_eq!(top_four(&[0.0; 3]), ([0; 4], [0; 4]));
        let (b, w) = top_four(&[0.0, 1.0]);
        assert_eq!((b[0], w[0]), (1, 255));
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

    #[test]
    fn asymmetry_is_small_keeps_eyes_whole_and_lips_closed() {
        use crate::pack::Quad;
        let lm = Landmarks {
            eyes: [Vec3::new(0.032, 1.65, 0.08), Vec3::new(-0.032, 1.65, 0.08)],
            eye_radius: 0.012,
            head: Vec3::new(0.0, 1.65, 0.0),
            mouth: Vec3::new(0.0, 1.57, 0.1),
        };
        let mut positions = Vec::new();
        for e in lm.eyes {
            for o in [(1.0, 0.0), (-1.0, 0.0), (0.0, 1.0), (0.0, -1.0)] {
                positions.push(e + Vec3::new(o.0, o.1, 0.0) * 0.012);
            }
        }
        // Upper and lower lip at both corners and the middle.
        for x in [0.02, 0.0, -0.02] {
            for dy in [0.002, -0.002] {
                positions.push(lm.mouth + Vec3::new(x, LIP_LINE + dy, 0.0));
            }
        }
        let quad = |v: u32, group: u16| Quad {
            v: [v, v + 1, v + 2, v + 3],
            uv: [0; 4],
            group,
        };
        let pack = HumanPack::new(
            positions.clone(),
            vec![Vec2::ZERO],
            vec!["helper-l-eye".into(), "helper-r-eye".into()],
            vec![quad(0, 0), quad(4, 1)],
            vec![],
        );
        let regions = Regions {
            masks: vec![0.0; positions.len() * MASKS],
            landmarks: lm,
            lid_margins: Default::default(),
        };
        let mut moved = 0;
        for seed in 0..200 {
            let a = Appearance {
                seed,
                ..Default::default()
            };
            let mut p = positions.clone();
            asymmetry(&pack, &regions, &a, &mut p);
            let mut again = positions.clone();
            asymmetry(&pack, &regions, &a, &mut again);
            assert_eq!(p, again);
            let shift: Vec<Vec3> = p.iter().zip(&positions).map(|(a, b)| *a - *b).collect();
            assert!(
                shift.iter().all(|s| s.length() < 0.003),
                "{seed}: {shift:?}"
            );
            for eye in [0..4, 4..8] {
                let s = &shift[eye];
                assert!(
                    s.iter().all(|x| (*x - s[0]).length() < 1e-6),
                    "{seed}: {s:?}"
                );
            }
            for pair in shift[8..].chunks(2) {
                assert!((pair[0].y - pair[1].y).abs() < 1e-6, "{seed}: {pair:?}");
            }
            moved += (shift[0].y - shift[4].y).abs().gt(&0.0003) as u32;
        }
        assert!(moved > 100, "{moved}");
    }

    #[test]
    fn the_shader_parts_the_lips_on_the_same_line() {
        let src = include_str!("shaders/skin.wgsl");
        let line = src
            .lines()
            .find_map(|l| l.strip_prefix("const LIP_LINE: f32 = "))
            .and_then(|v| v.trim_end_matches(';').parse::<f32>().ok())
            .unwrap();
        assert!((line - LIP_LINE * 1000.0).abs() < 1e-4, "{line}");
    }
}
