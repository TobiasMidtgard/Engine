//! Clothes: the shirt and trousers as a layer of their own over the skin, so they stand off
//! the body and hang from it like cloth instead of being painted on like leggings.
//!
//! The layer is a copy of the covered skin (everything the shirt and trousers paint covers in
//! `common.wgsl`), moved out from the body: the trousers' lower legs hang straight down from
//! the knees, hollows such as the small of the back and under the chest are bridged (the
//! cloth only ever moves outward), and the whole layer stands a few millimetres off the skin,
//! closing in at the neckline and the cuffs. It keeps the skin's rest positions, region masks
//! and bone weights, so the same shading draws the fabric and it bends with the body. The skin
//! well inside the clothes is no longer drawn (it could only poke through).

use std::collections::{HashMap, VecDeque};

use std::f32::consts::TAU;

use engine_core::math::{Vec2, Vec3};

use crate::appearance::Appearance;
use crate::mesh::{MASKS, Mask, Regions, SkinMesh, vertex_normals};
use crate::pack::HumanPack;
use crate::shoes::{SHOE_TOP, hull, reach};

/// The trousers' hem (rest height): a little over the shoes.
const HEM: f32 = SHOE_TOP - 0.03;
/// Rest height of the trousers' waistband (`WAIST` in common.wgsl).
const WAIST: f32 = 0.105;
/// How far the cloth stands off the body, m (the shaders lift it a little more).
const EASE_SHIRT: f32 = 0.003;
const EASE_TROUSERS: f32 = 0.004;
const EASE_SLEEVES: f32 = 0.005;
/// Cross-sections of each trouser leg below the knee, and directions around every section.
const SLICES: usize = 20;
const AROUND: usize = 32;

/// Joints of the base mesh's left arm at rest (`SHOULDER`, `ELBOW` in common.wgsl).
const SHOULDER: Vec3 = Vec3::new(0.168, 0.525, 0.015);
const ELBOW: Vec3 = Vec3::new(0.313, 0.349, 0.013);
/// A t-shirt's sleeves end this far down the upper arm from the shoulder joint, m.
const SLEEVE_HEM: f32 = 0.12;
/// Rest height of the shorts' hems, mid-thigh, and of the briefs' under a skirt.
const SHORTS_HEM: f32 = -0.17;
const BRIEFS_HEM: f32 = -0.07;
/// The band a woman wears with no top: rest heights of its lower and upper edges.
const BAND: [f32; 2] = [0.32, 0.45];

/// The upper garment (`Outfit::top`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Top {
    /// Long sleeves.
    #[default]
    Shirt,
    Tshirt,
    /// Bare-chested; women wear a band across the chest.
    None,
}

/// The lower garment (`Outfit::bottom`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Bottom {
    #[default]
    Trousers,
    Shorts,
    /// To above the knees (`crate::skirt`), over briefs.
    Skirt,
}

/// Where the outfit's accent colour shows (`Outfit::accent_on`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Accent {
    #[default]
    None,
    ArmbandL,
    ArmbandR,
    Collar,
}

/// How a person's clothes are cut, from their outfit's ids (unknown ids fall back to the
/// defaults). The shaders cut the same way (`cover_depth` in common.wgsl).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Cut {
    pub top: Top,
    pub bottom: Bottom,
    pub accent: Accent,
    /// Men go bare-chested with no top.
    pub male: bool,
}

impl Cut {
    pub fn of(a: &Appearance) -> Self {
        let o = &a.outfit;
        Self {
            top: match o.top.as_str() {
                "tshirt" => Top::Tshirt,
                "none" => Top::None,
                _ => Top::Shirt,
            },
            bottom: match o.bottom.as_str() {
                "shorts" => Bottom::Shorts,
                "skirt" => Bottom::Skirt,
                _ => Bottom::Trousers,
            },
            accent: match o.accent_on.as_str() {
                "armband_l" => Accent::ArmbandL,
                "armband_r" => Accent::ArmbandR,
                "collar" => Accent::Collar,
                _ => Accent::None,
            },
            male: a.sex > 0.5,
        }
    }

    /// The styles as the shaders read them: top, bottom, accent place.
    pub fn ids(&self) -> [f32; 3] {
        [
            self.top as u8 as f32,
            self.bottom as u8 as f32,
            self.accent as u8 as f32,
        ]
    }
}

/// How far up the upper arm past a sleeve's hem a rest position is (`hem`: m from the shoulder
/// joint), or `None` off the arms (`sleeve_past` in common.wgsl).
fn sleeve_past(r: Vec3, hem: f32) -> Option<f32> {
    let m = Vec3::new(if r.x > 0.0 { 1.0 } else { -1.0 }, 1.0, 1.0);
    let a = SHOULDER * m;
    let ax = ((ELBOW - SHOULDER) * m).normalize();
    let t = (r - a).dot(ax);
    let off = (r - a - ax * t).length();
    // Around the upper arm (the torso is over 0.1 m from its axis), or out on the forearm.
    (off < 0.075 || r.x.abs() > 0.19).then_some(t - hem)
}

/// How far inside the clothes' edges a rest position is, m (negative outside): the neckline,
/// and the sleeves, shorts and band where the outfit has them.
fn cover_depth(r: Vec3, head_z: f32, cut: Cut) -> f32 {
    let mut d = collar_depth(r, head_z);
    if r.y >= WAIST {
        let bare_arms = |hem| sleeve_past(r, hem).map_or(1.0, |past| -past);
        match cut.top {
            Top::Shirt => {}
            Top::Tshirt => d = d.min(bare_arms(SLEEVE_HEM)),
            Top::None if cut.male => d = d.min(WAIST - r.y),
            Top::None => d = d.min(bare_arms(0.0)).min(r.y - BAND[0]).min(BAND[1] - r.y),
        }
    } else {
        match cut.bottom {
            Bottom::Trousers => {}
            Bottom::Shorts => d = d.min(r.y - SHORTS_HEM),
            Bottom::Skirt => d = d.min(r.y - BRIEFS_HEM),
        }
    }
    d
}

/// How far inside the shirt's neckline a rest position is (`collar_depth` in common.wgsl;
/// `head_z`: the rest head center's depth).
fn collar_depth(r: Vec3, head_z: f32) -> f32 {
    let d = Vec2::new(r.x, r.z - head_z + 0.012);
    let theta = d.x.atan2(d.y);
    let neckline = 0.565 - 0.03 * (theta.cos() * 0.5 + 0.5);
    let shoulders = (d.length() - 0.082).min(0.6 - r.y);
    let a = neckline - r.y;
    let k = 0.008;
    let h = (0.5 + 0.5 * (a - shoulders) / k).clamp(0.0, 1.0);
    shoulders + (a - shoulders) * h + k * h * (1.0 - h)
}

fn smooth(e0: f32, e1: f32, x: f32) -> f32 {
    let t = ((x - e0) / (e1 - e0)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// Whether the clothes cover a rest position (`cloth`: its cloth mask).
fn covers(r: Vec3, cloth: f32, head_z: f32, cut: Cut) -> bool {
    cloth > 0.5 && r.y > HEM && cover_depth(r, head_z, cut) > 0.001
}

/// Put the clothes layer on a skin built from `morphed` (the pack's vertices after the
/// morphs), before any extra vertices are added. Returns, per skin vertex, the clothes vertex
/// laid over it (`u32::MAX`: bare; empty: no clothes).
pub fn dress(
    pack: &HumanPack,
    regions: &Regions,
    morphed: &[Vec3],
    skin: &mut SkinMesh,
    cut: Cut,
) -> Vec<u32> {
    if !skin.extra.is_empty() || skin.base.is_empty() {
        return Vec::new();
    }
    let head_z = regions.landmarks.head.z;
    let cloth = |v: u32| regions.masks[v as usize * MASKS + Mask::Cloth as usize];
    let n = skin.base.len();
    let pos: Vec<Vec3> = skin.base.iter().map(|v| morphed[*v as usize]).collect();
    let rest: Vec<Vec3> = skin
        .base
        .iter()
        .map(|v| pack.positions[*v as usize])
        .collect();
    let cov: Vec<bool> = (0..n)
        .map(|b| covers(rest[b], cloth(skin.base[b]), head_z, cut))
        .collect();
    if !cov.iter().any(|c| *c) {
        return Vec::new();
    }
    let mut near: Vec<Vec<u32>> = vec![Vec::new(); n];
    let mut tris = Vec::with_capacity(skin.base_quads.len() * 6);
    for q in &skin.base_quads {
        for k in 0..4 {
            let (a, b) = (q[k], q[(k + 1) % 4]);
            if !near[a as usize].contains(&b) {
                near[a as usize].push(b);
                near[b as usize].push(a);
            }
        }
        tris.extend([q[0], q[1], q[2], q[0], q[2], q[3]]);
    }
    let normals = vertex_normals(&pos, &tris);

    // Close in toward the neckline and the cuffs: hops from the edge (not from the hem, which
    // hangs free).
    let mut hops = vec![u32::MAX; n];
    let mut queue = VecDeque::new();
    for b in 0..n {
        let edge = cov[b]
            && near[b]
                .iter()
                .any(|o| !cov[*o as usize] && rest[*o as usize].y > HEM);
        if edge {
            hops[b] = 0;
            queue.push_back(b);
        }
    }
    while let Some(b) = queue.pop_front() {
        for o in &near[b] {
            let o = *o as usize;
            if cov[o] && hops[o] == u32::MAX {
                hops[o] = hops[b] + 1;
                queue.push_back(o);
            }
        }
    }
    let taper: Vec<f32> = hops
        .iter()
        .map(|h| match h {
            0 => 0.3,
            1 => 0.65,
            _ => 1.0,
        })
        .collect();

    let mut q = pos.clone();
    hang(pack, morphed, &skin.base, &rest, &cov, &mut q);
    // Bridge the hollows: move toward the neighbors' average, only ever outward.
    for _ in 0..20 {
        let q0 = q.clone();
        for b in (0..n).filter(|b| cov[*b] && !near[*b].is_empty()) {
            let avg = near[b].iter().map(|o| q0[*o as usize]).sum::<Vec3>() / near[b].len() as f32;
            let out = (avg - q0[b]).dot(normals[b]);
            if out > 0.0 {
                q[b] = q0[b] + normals[b] * (out * 0.5);
            }
        }
    }
    let disp: Vec<Vec3> = (0..n)
        .map(|b| {
            if !cov[b] {
                return Vec3::ZERO;
            }
            let ease = if rest[b].y < WAIST {
                EASE_TROUSERS
            } else if arm_weight(pack, skin.base[b]) >= 0.3 {
                EASE_SLEEVES
            } else {
                EASE_SHIRT
            };
            (q[b] - pos[b] + normals[b] * ease) * taper[b]
        })
        .collect();

    // Onto the subdivided skin.
    let local: HashMap<u32, usize> = skin.base.iter().enumerate().map(|(b, v)| (*v, b)).collect();
    let values = skin.subdivide_values(4, |v, out| {
        let b = local[&(v as u32)];
        out[..3].copy_from_slice(&disp[b].to_array());
        out[3] = taper[b];
    });
    let n_skin = skin.positions.len();
    let on: Vec<bool> = (0..n_skin)
        .map(|i| {
            covers(
                skin.rest[i],
                skin.masks[i * MASKS + Mask::Cloth as usize],
                head_z,
                cut,
            )
        })
        .collect();
    // Extra vertices follow a pack vertex's expressions: one low on the legs, which has none.
    let anchor = (0..n)
        .filter(|b| cov[*b])
        .min_by(|a, b| pos[*a].y.total_cmp(&pos[*b].y))
        .map_or(0, |b| skin.base[b]);
    let mut index = vec![u32::MAX; n_skin];
    let mut cloth_tris = Vec::new();
    let mut skin_tris = Vec::with_capacity(skin.indices.len());
    let mut layer_pos = Vec::new();
    for t in skin.indices.as_chunks::<3>().0 {
        if !t.iter().all(|i| on[*i as usize]) {
            skin_tris.extend(t);
            continue;
        }
        for &i in t {
            let i = i as usize;
            if index[i] == u32::MAX {
                index[i] = (n_skin + layer_pos.len()) as u32;
                let d = Vec3::new(values[i * 4], values[i * 4 + 1], values[i * 4 + 2]);
                // Flush with the skin at the neckline and the cuffs, so their edge doesn't
                // show its triangles (just off it, so the two don't fight).
                let r = skin.rest[i];
                let cuff = skin.masks[i * MASKS + Mask::Cloth as usize];
                let edge =
                    smooth(0.001, 0.01, cover_depth(r, head_z, cut)) * smooth(0.5, 0.95, cuff);
                let p = skin.positions[i] + d * edge + skin.normals[i] * 0.0006;
                layer_pos.push((i, p));
            }
            cloth_tris.push(index[i]);
        }
        // Skin well inside the clothes can't be seen.
        if t.iter().any(|i| values[*i as usize * 4 + 3] < 0.999) {
            skin_tris.extend(t);
        }
    }
    let local_tris: Vec<u32> = cloth_tris.iter().map(|i| i - n_skin as u32).collect();
    let local_pos: Vec<Vec3> = layer_pos.iter().map(|(_, p)| *p).collect();
    let layer_normals = vertex_normals(&local_pos, &local_tris);
    for ((i, p), nrm) in layer_pos.into_iter().zip(layer_normals) {
        skin.positions.push(p);
        skin.normals.push(nrm);
        // (The hem over the shoes is still trouser cloth.)
        let r = skin.rest[i];
        skin.rest
            .push(Vec3::new(r.x, r.y.max(SHOE_TOP + 0.002), r.z));
        let m = skin.masks[i * MASKS..(i + 1) * MASKS].to_vec();
        skin.masks.extend(m);
        skin.curvature.push(0.0);
        if !skin.bones.is_empty() {
            skin.bones.push(skin.bones[i]);
            skin.weights.push(skin.weights[i]);
        }
        skin.extra
            .push([(anchor, 1.0), (anchor, 0.0), (anchor, 0.0)]);
        skin.extra_parts.push(0);
    }
    skin_tris.extend(cloth_tris);
    skin.indices = skin_tris;
    index
}

/// Cross-sections of part of the clothes, stacked up its height: each section's center and
/// how far its convex hull reaches in each direction around.
struct Column {
    y0: f32,
    step: f32,
    centers: Vec<Vec2>,
    radii: Vec<[f32; AROUND]>,
}

impl Column {
    fn new(q: &[Vec3], verts: &[usize], slices: usize) -> Option<Self> {
        if verts.len() < 16 {
            return None;
        }
        let (y0, y1) = verts.iter().fold((f32::MAX, f32::MIN), |(a, b), v| {
            (a.min(q[*v].y), b.max(q[*v].y))
        });
        let step = (y1 - y0) / slices as f32;
        let mut col = Self {
            y0,
            step,
            centers: vec![Vec2::ZERO; slices],
            radii: vec![[0.0; AROUND]; slices],
        };
        for k in 0..slices {
            let y = y0 + (k as f32 + 0.5) * step;
            let h = hull(
                verts
                    .iter()
                    .map(|v| q[*v])
                    .filter(|p| (p.y - y).abs() < step * 0.75)
                    .map(|p| Vec2::new(p.x, p.z))
                    .collect(),
            );
            if h.len() < 3 {
                continue;
            }
            let (lo, hi) = h
                .iter()
                .fold((Vec2::MAX, Vec2::MIN), |(a, b), p| (a.min(*p), b.max(*p)));
            col.centers[k] = (lo + hi) * 0.5;
            for (j, r) in col.radii[k].iter_mut().enumerate() {
                *r = reach(&h, col.centers[k], dir(j));
            }
        }
        Some(col)
    }

    fn slice(&self, y: f32) -> usize {
        (((y - self.y0) / self.step - 0.5).round().max(0.0) as usize).min(self.radii.len() - 1)
    }

    /// Move each vertex out (never in) to its section's reach, by `w(vertex)`.
    fn wrap(&self, q: &mut [Vec3], verts: &[usize], w: impl Fn(usize) -> f32) {
        for &v in verts {
            let p = q[v];
            let k = self.slice(p.y);
            if self.radii[k][0] == 0.0 {
                continue;
            }
            let d = Vec2::new(p.x, p.z) - self.centers[k];
            let rho = d.length();
            let f = d.y.atan2(d.x).rem_euclid(TAU) / TAU * AROUND as f32;
            let (j, t) = (f.floor() as usize % AROUND, f.fract());
            let want = self.radii[k][j] * (1.0 - t) + self.radii[k][(j + 1) % AROUND] * t;
            if want > rho && rho > 1e-5 {
                let out = d / rho * ((want - rho) * w(v));
                q[v] = Vec3::new(p.x + out.x, p.y, p.z + out.y);
            }
        }
    }
}

/// Direction `j` of [`AROUND`] in the horizontal (x, z) plane.
fn dir(j: usize) -> Vec2 {
    let a = j as f32 / AROUND as f32 * TAU;
    Vec2::new(a.cos(), a.sin())
}

/// How much of a pack vertex the arms move.
fn arm_weight(pack: &HumanPack, v: u32) -> f32 {
    let Some(s) = pack.skin.get(v as usize) else {
        return 0.0;
    };
    (0..4)
        .filter(|k| {
            let b = &pack.bones[s.bones[*k] as usize];
            b.starts_with("upper_arm") || b.starts_with("lower_arm") || b.starts_with("hand")
        })
        .map(|k| s.weights[k] as f32 / 255.0)
        .sum()
}

/// The shirt hangs from the chest and shoulder blades over the hollows below them (under the
/// chest, the small of the back), tucked in at the waist; the trouser legs hang straight down
/// from the knees instead of hugging the calves and ankles.
fn hang(
    pack: &HumanPack,
    morphed: &[Vec3],
    base: &[u32],
    rest: &[Vec3],
    cov: &[bool],
    q: &mut [Vec3],
) {
    // The torso: everything covered above the waist that the arms don't move.
    if pack.has_rig() {
        let torso: Vec<usize> = (0..q.len())
            .filter(|b| cov[*b] && rest[*b].y > WAIST - 0.02 && arm_weight(pack, base[*b]) < 0.3)
            .collect();
        if let Some(mut col) = Column::new(q, &torso, 24) {
            // The fullest section in front, in the upper half: the chest.
            let n = col.radii.len();
            let chest = (n / 2..n)
                .max_by(|a, b| col.radii[*a][AROUND / 4].total_cmp(&col.radii[*b][AROUND / 4]))
                .unwrap_or(n - 1);
            let top = col.radii[chest];
            for row in &mut col.radii[..chest] {
                for (j, r) in row.iter_mut().enumerate() {
                    // Straight down in front and behind, a little in at the sides.
                    let keep = 0.9 + 0.07 * dir(j).y.abs();
                    *r = r.max(top[j] * keep);
                }
            }
            col.wrap(q, &torso, |b| ((rest[b].y - WAIST) / 0.08).clamp(0.0, 1.0));
        }
    }
    let joint = |name: &str| {
        let verts = &pack.joints[pack.bone(name)?];
        (!verts.is_empty())
            .then(|| verts.iter().map(|v| morphed[*v as usize]).sum::<Vec3>() / verts.len() as f32)
    };
    // Sleeves: sections across each arm, from the shoulder to the wrist, the forearm's no
    // narrower than most of the elbow's.
    for (s, side) in [("l", 1.0f32), ("r", -1.0)] {
        let (Some(shoulder), Some(elbow), Some(wrist)) = (
            joint(&format!("upper_arm_{s}")),
            joint(&format!("lower_arm_{s}")),
            joint(&format!("hand_{s}")),
        ) else {
            continue;
        };
        let arm: Vec<usize> = (0..q.len())
            .filter(|b| cov[*b] && q[*b].x * side > 0.0 && arm_weight(pack, base[*b]) >= 0.3)
            .collect();
        // Each part of the arm in a frame of its own whose "up" runs along it.
        for (from, to, wide) in [(elbow, wrist, true), (shoulder, elbow, false)] {
            let w = (from - to).normalize_or(Vec3::Y);
            let u = w.cross(Vec3::Z).normalize_or(Vec3::X);
            let v = u.cross(w);
            let local = |p: Vec3| {
                let d = p - to;
                Vec3::new(d.dot(u), d.dot(w), d.dot(v))
            };
            let len = (from - to).length();
            let part: Vec<usize> = arm
                .iter()
                .copied()
                .filter(|b| (0.0..len).contains(&local(q[*b]).y))
                .collect();
            let mut lq: Vec<Vec3> = q.iter().map(|p| local(*p)).collect();
            let Some(mut col) = Column::new(&lq, &part, 16) else {
                continue;
            };
            if wide {
                let top = col.radii[col.radii.len() - 1];
                for row in &mut col.radii {
                    for (j, r) in row.iter_mut().enumerate() {
                        *r = r.max(top[j] * 0.85);
                    }
                }
            }
            col.wrap(&mut lq, &part, |_| 1.0);
            for b in part {
                let l = lq[b];
                q[b] = to + u * l.x + w * l.y + v * l.z;
            }
        }
    }
    for (knee, side) in [("lower_leg_l", 1.0f32), ("lower_leg_r", -1.0)] {
        let Some(knee) = joint(knee) else {
            continue;
        };
        let leg: Vec<usize> = (0..q.len())
            .filter(|b| cov[*b] && q[*b].y < knee.y && q[*b].x * side > 0.0)
            .collect();
        let Some(mut col) = Column::new(q, &leg, SLICES) else {
            continue;
        };
        // Each section at least nearly as wide as the knee's, easing in just below the knee.
        let top = col.radii[SLICES - 1];
        for (k, row) in col.radii.iter_mut().enumerate() {
            let below = ((SLICES - 1 - k) as f32 / 3.0).min(1.0);
            for (j, r) in row.iter_mut().enumerate() {
                *r = r.max(top[j] * (1.0 - 0.07 * below));
            }
        }
        let step = col.step;
        col.wrap(q, &leg, |b| {
            ((knee.y - q_y(morphed, base, b)) / step).clamp(0.0, 1.0)
        });
    }
}

fn q_y(morphed: &[Vec3], base: &[u32], b: usize) -> f32 {
    morphed[base[b] as usize].y
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_neckline_matches_the_shader() {
        // Down the chest is inside the shirt, up the neck outside it, as in common.wgsl.
        assert!(collar_depth(Vec3::new(0.0, 0.45, 0.08), 0.0) > 0.0);
        assert!(collar_depth(Vec3::new(0.0, 0.62, 0.02), 0.0) < 0.0);
        // Hands (no cloth) and feet (below the hem) are bare.
        let shirt = Cut::default();
        assert!(covers(Vec3::new(0.2, 0.0, 0.0), 1.0, 0.0, shirt));
        assert!(!covers(Vec3::new(0.2, 0.0, 0.0), 0.0, 0.0, shirt));
        assert!(!covers(Vec3::new(0.1, HEM - 0.01, 0.0), 1.0, 0.0, shirt));
    }

    #[test]
    fn short_sleeves_and_shorts_bare_the_forearms_and_shins() {
        let mut a = Appearance::default();
        a.outfit.top = "tshirt".into();
        a.outfit.bottom = "shorts".into();
        let cut = Cut::of(&a);
        let upper_arm = Vec3::new(0.2, 0.49, 0.015);
        let elbow = Vec3::new(0.313, 0.349, 0.013);
        let side = Vec3::new(0.13, 0.35, 0.0);
        let thigh = Vec3::new(0.12, -0.05, 0.03);
        let shin = Vec3::new(0.18, -0.5, 0.03);
        for (p, covered) in [
            (upper_arm, true),
            (elbow, false),
            (elbow * Vec3::new(-1.0, 1.0, 1.0), false),
            (side, true),
            (thigh, true),
            (shin, false),
        ] {
            assert_eq!(covers(p, 1.0, 0.02, cut), covered, "{p}");
            assert!(covers(p, 1.0, 0.02, Cut::default()), "{p}");
        }
        // No top: men are bare to the waist, women wear a band across the chest.
        a.outfit.top = "none".into();
        a.sex = 1.0;
        assert!(!covers(Vec3::new(0.05, 0.4, 0.1), 1.0, 0.02, Cut::of(&a)));
        a.sex = 0.0;
        assert!(covers(Vec3::new(0.05, 0.4, 0.1), 1.0, 0.02, Cut::of(&a)));
        assert!(!covers(Vec3::new(0.05, 0.25, 0.1), 1.0, 0.02, Cut::of(&a)));
        assert!(!covers(upper_arm, 1.0, 0.02, Cut::of(&a)));
    }
}
