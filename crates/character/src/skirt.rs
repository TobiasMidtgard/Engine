//! Skirts: one shell of cloth from the waist to above the knees, round both legs at once
//! (a skirt can't be a copy of the skin the way the trousers are: it bridges the gap between
//! the legs).
//!
//! The shell is lofted down the body like the shoes are along the foot. Each cross-section is
//! the convex hull of the hips and thighs there; below the fullest one (the seat) the skirt
//! hangs straight and then flares, with soft folds that deepen toward the hem. Every shell
//! vertex follows the three nearest body vertices, so it fits any body and moves with the hips
//! and thighs. The legs underneath are bare below briefs (`cover_depth` in clothes.rs).

use std::f32::consts::TAU;

use engine_core::math::{Vec2, Vec3};

use crate::mesh::SkinMesh;
use crate::mouth::{MouthMesh, MouthPart, push, smooth_normals};
use crate::pack::HumanPack;
use crate::shoes::{hull, reach};

/// Rest height of the waistband (`WAIST` in common.wgsl).
const WAIST: f32 = 0.105;
/// Cross-sections from waist to hem, and points around each.
const SLICES: usize = 24;
const AROUND: usize = 48;

/// How a shell of cloth round both legs is cut: rest heights of its top and hem, how far it
/// stands off the body (m) and flares out beyond the seat at the hem, its folds round the hem
/// (how many, how deep), what it is (for the shader), and for a tabard the half width of its
/// front and back panels (`None`: all the way round).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Shell {
    pub top: f32,
    pub hem: f32,
    pub ease: f32,
    pub flare: f32,
    pub folds: f32,
    pub fold_depth: f32,
    pub part: MouthPart,
    pub panels: Option<f32>,
}

/// A skirt, from the waistband to a hand above the knees.
pub const SKIRT: Shell = Shell {
    top: WAIST,
    hem: -0.3,
    ease: 0.006,
    flare: 0.05,
    folds: 11.0,
    fold_depth: 0.012,
    part: MouthPart::Skirt,
    panels: None,
};
/// The skirt of a gambeson or a mail hauberk: padded, from over the belt to mid-thigh, standing
/// clear of the padded torso above it.
pub const COAT: Shell = Shell {
    top: WAIST + 0.03,
    hem: -0.25,
    ease: 0.016,
    flare: 0.03,
    folds: 7.0,
    fold_depth: 0.005,
    part: MouthPart::Coat,
    panels: None,
};
/// A tabard's panels, front and back, from the belt to above the knees, over a coat.
pub const TABARD: Shell = Shell {
    top: WAIST + 0.02,
    hem: -0.34,
    ease: 0.03,
    flare: 0.06,
    folds: 5.0,
    fold_depth: 0.004,
    part: MouthPart::Tabard,
    panels: Some(0.125),
};

impl Shell {
    /// A tabard over trousers rather than a coat lies closer.
    pub fn without_coat(mut self) -> Self {
        self.ease = 0.012;
        self.flare = 0.04;
        self.top = WAIST + 0.005;
        self
    }
}

/// A shell of a morphed body (`morphed`: the pack's vertices after the morphs). Empty when the
/// body has no hips and legs.
pub fn build(pack: &HumanPack, morphed: &[Vec3], shell: &Shell) -> MouthMesh {
    let mut m = MouthMesh::default();
    // (Not the hands, which hang at the hips.)
    let ids: Vec<u32> = pack
        .group_vertices("body")
        .into_iter()
        .filter(|&v| {
            (shell.hem - 0.02..shell.top + 0.01).contains(&pack.positions[v as usize].y)
                && crate::clothes::arm_weight(pack, v) < 0.3
        })
        .collect();
    if ids.len() < 32 {
        return m;
    }
    let pts: Vec<Vec3> = ids.iter().map(|&v| morphed[v as usize]).collect();
    // The top and hem heights on this body: where its rest top and hem went.
    let height_of = |rest_y: f32| {
        let near: Vec<f32> = ids
            .iter()
            .filter(|&&v| (pack.positions[v as usize].y - rest_y).abs() < 0.01)
            .map(|&v| morphed[v as usize].y)
            .collect();
        near.iter().sum::<f32>() / near.len().max(1) as f32
    };
    skirt(
        &mut m,
        &pts,
        &ids,
        height_of(shell.top),
        height_of(shell.hem),
        shell,
    );
    m
}

fn skirt(m: &mut MouthMesh, pts: &[Vec3], ids: &[u32], top: f32, hem: f32, shell: &Shell) {
    if top - hem < 0.05 {
        return;
    }
    let step = (top - hem) / (SLICES - 1) as f32;
    let y_at = |k: usize| top - k as f32 * step;
    let mut centers = Vec::with_capacity(SLICES);
    let mut radii = vec![[0.0f32; AROUND]; SLICES];
    for (k, row) in radii.iter_mut().enumerate() {
        let y = y_at(k);
        let h = hull(
            pts.iter()
                .filter(|p| (p.y - y).abs() < step * 0.9)
                .map(|p| Vec2::new(p.x, p.z))
                .collect(),
        );
        if h.len() < 3 {
            centers.push(None);
            continue;
        }
        let (lo, hi) = h
            .iter()
            .fold((Vec2::MAX, Vec2::MIN), |(a, b), q| (a.min(*q), b.max(*q)));
        let c = (lo + hi) * 0.5;
        for (j, r) in row.iter_mut().enumerate() {
            *r = reach(&h, c, dir(j));
        }
        centers.push(Some(c));
    }
    let filled: Vec<usize> = (0..SLICES).filter(|k| centers[*k].is_some()).collect();
    if filled.is_empty() {
        return;
    }
    let mut centers: Vec<Vec2> = (0..SLICES)
        .map(|k| {
            let from = *filled.iter().min_by_key(|j| j.abs_diff(k)).unwrap();
            if from != k {
                radii[k] = radii[from];
            }
            centers[from].unwrap()
        })
        .collect();
    // Below the seat (the fullest section) the skirt hangs from it: never narrower, the
    // center held over the seat's, and flaring out toward the hem.
    let seat = (0..SLICES / 2 + 1)
        .max_by(|a, b| {
            let area = |k: usize| radii[k].iter().map(|r| r * r).sum::<f32>();
            area(*a).total_cmp(&area(*b))
        })
        .unwrap_or(0);
    let (seat_r, seat_c) = (radii[seat], centers[seat]);
    for k in seat + 1..SLICES {
        let t = (k - seat) as f32 / (SLICES - 1 - seat).max(1) as f32;
        for (j, (r, s)) in radii[k].iter_mut().zip(seat_r).enumerate() {
            // Flaring front and back more than at the sides, where the hands hang.
            *r = r.max(s) + shell.flare * t * t * front_back(j);
        }
        centers[k] = seat_c;
    }
    // Smooth around each section and down the skirt (no thigh-sized dents).
    for _ in 0..4 {
        let r0 = radii.clone();
        for k in 0..SLICES {
            let (a, b) = (k.saturating_sub(1), (k + 1).min(SLICES - 1));
            for j in 0..AROUND {
                let around = (r0[k][(j + AROUND - 1) % AROUND] + r0[k][(j + 1) % AROUND]) * 0.5;
                radii[k][j] = (r0[a][j] + r0[b][j] + 2.0 * r0[k][j] + 2.0 * around) / 6.0;
            }
        }
    }
    let first = m.positions.len() as u32;
    for (k, (row, c)) in radii.iter().zip(&centers).enumerate() {
        // Soft folds, none at the waistband and deepest at the hem, wavering around.
        let t = (k as f32 / (SLICES - 1) as f32).max(0.0);
        let depth = shell.fold_depth * t.powf(1.5);
        for (j, r) in row.iter().enumerate() {
            let a = j as f32 / AROUND as f32 * TAU;
            let fold = (a * shell.folds + 0.6 * (a * 3.0).sin()).cos() * 0.5 + 0.5;
            let q = *c + dir(j) * (r + shell.ease + depth * fold * front_back(j));
            push(m, Vec3::new(q.x, y_at(k), q.y), shell.part, pts, ids);
        }
    }
    let at = |k: usize, j: usize| first + (k * AROUND + j % AROUND) as u32;
    // A tabard's panels: the columns within its half width of the middle, front and back (at
    // the seat, so the panels hang straight however the hem flares).
    let column = |j: usize| {
        shell.panels.is_none_or(|w| {
            let x = dir(j).x * radii[seat][j];
            x.abs() < w && dir(j).y.abs() > 0.3
        })
    };
    let start = m.indices.len();
    for k in 0..SLICES - 1 {
        for j in 0..AROUND {
            if !(column(j) && column((j + 1) % AROUND)) {
                continue;
            }
            let (a, b, c, d) = (at(k, j), at(k, j + 1), at(k + 1, j), at(k + 1, j + 1));
            m.indices.extend([a, c, b, b, c, d]);
        }
    }
    // Face outward: flip if a band's normal points into the skirt.
    let mid = SLICES / 2;
    let per_band = (m.indices.len() - start) / (SLICES - 1);
    if per_band == 0 {
        return;
    }
    let probe = mid * per_band + start;
    let (a, b, c) = (
        m.positions[m.indices[probe] as usize],
        m.positions[m.indices[probe + 1] as usize],
        m.positions[m.indices[probe + 2] as usize],
    );
    let center = Vec3::new(centers[mid].x, y_at(mid), centers[mid].y);
    if (b - a).cross(c - a).dot(a - center) < 0.0 {
        for t in m.indices[start..].as_chunks_mut::<3>().0 {
            t.swap(1, 2);
        }
    }
    smooth_normals(m, first);
    if shell.panels.is_some() {
        back_faces(m, first, start);
    }
}

/// The far side of open panels (the skin is drawn with back faces culled): a copy of the
/// panels' vertices just behind them, facing the other way.
fn back_faces(m: &mut MouthMesh, first: u32, start: usize) {
    let n = m.positions.len() as u32 - first;
    for i in first..first + n {
        let (p, nrm) = (m.positions[i as usize], m.normals[i as usize]);
        m.positions.push(p - nrm * 0.0015);
        m.normals.push(-nrm);
        m.parts.push(m.parts[i as usize]);
        m.anchors.push(m.anchors[i as usize]);
    }
    let flipped: Vec<u32> = m.indices[start..]
        .as_chunks::<3>()
        .0
        .iter()
        .flat_map(|t| [t[0] + n, t[2] + n, t[1] + n])
        .collect();
    m.indices.extend(flipped);
}

/// How much of the flare and folds direction `j` gets: all of it front and back, a third at
/// the sides.
fn front_back(j: usize) -> f32 {
    0.35 + 0.65 * dir(j).y.abs()
}

/// The person's left thigh as a capsule for the skirt to stay outside (`thigh`, `knee` in
/// common.wgsl): the hip and knee joints (base mesh space, after the morphs) and the thigh's
/// radius at each end. The right thigh mirrors it. `None` without a rig.
pub fn thigh(pack: &HumanPack, morphed: &[Vec3]) -> Option<[[f32; 4]; 2]> {
    let joint = |name: &str| {
        let verts = pack.joints.get(pack.bone(name)?)?;
        (!verts.is_empty())
            .then(|| verts.iter().map(|v| morphed[*v as usize]).sum::<Vec3>() / verts.len() as f32)
    };
    let (hip, knee) = (joint("upper_leg_l")?, joint("lower_leg_l")?);
    let leg = pack.bone("upper_leg_l")? as u8;
    let axis = knee - hip;
    let len2 = axis.length_squared().max(1e-6);
    // Distances from the axis of the vertices the thigh moves, near each end.
    let mut near_hip = Vec::new();
    let mut near_knee = Vec::new();
    for (v, s) in pack.skin.iter().enumerate() {
        let w: u32 = (0..4)
            .filter(|k| s.bones[*k] == leg)
            .map(|k| s.weights[k] as u32)
            .sum();
        if w < 160 || v >= morphed.len() {
            continue;
        }
        let d = morphed[v] - hip;
        let t = d.dot(axis) / len2;
        let r = (d - axis * t).length();
        if (0.15..0.35).contains(&t) {
            near_hip.push(r);
        } else if (0.7..0.9).contains(&t) {
            near_knee.push(r);
        }
    }
    let typical = |mut r: Vec<f32>| {
        r.sort_by(f32::total_cmp);
        r.get(r.len() * 3 / 4).copied()
    };
    let (rh, rk) = (typical(near_hip)?, typical(near_knee)?);
    Some([[hip.x, hip.y, hip.z, rh], [knee.x, knee.y, knee.z, rk]])
}

/// Skin a skirt added to `skin` from vertex `first` on to the hips and thighs: the waistband
/// rides the hips, the hem the thighs, and around the front and back both thighs in turn, so
/// the skirt swings with a stride without tearing open between the legs. Without a rig, the
/// skin keeps the anchors' weights.
pub fn reweight(pack: &HumanPack, skin: &mut SkinMesh, first: usize) {
    let (Some(hips), Some(left), Some(right)) = (
        pack.bone("hips"),
        pack.bone("upper_leg_l"),
        pack.bone("upper_leg_r"),
    ) else {
        return;
    };
    if skin.bones.len() != skin.positions.len() || first >= skin.positions.len() {
        return;
    }
    // (A tabard's far side repeats the rows.)
    let ring = |i: usize| (i - first) / AROUND % SLICES;
    let rows = SLICES.min((skin.positions.len() - first).div_ceil(AROUND));
    let center_x = |k: usize| {
        let row = &skin.positions
            [first + k * AROUND..(first + (k + 1) * AROUND).min(skin.positions.len())];
        row.iter().map(|p| p.x).sum::<f32>() / row.len().max(1) as f32
    };
    for i in first..skin.positions.len() {
        let k = ring(i);
        let t = k as f32 / (rows.max(2) - 1) as f32;
        let legs = 0.85 * t * t * (3.0 - 2.0 * t);
        // Base mesh +X is the person's left.
        let lean = ((skin.positions[i].x - center_x(k)) / 0.08).clamp(-1.0, 1.0);
        let l = 0.5 + 0.5 * lean;
        let w = [1.0 - legs, legs * l, legs * (1.0 - l)];
        let bytes = w.map(|w| (w * 255.0).round() as u8);
        let rest = 255u8.saturating_sub(bytes[1]).saturating_sub(bytes[2]);
        skin.bones[i] = [hips as u8, left as u8, right as u8, 0];
        skin.weights[i] = [rest, bytes[1], bytes[2], 0];
    }
}

/// Direction `j` of [`AROUND`] in the horizontal (x, z) plane.
fn dir(j: usize) -> Vec2 {
    let a = j as f32 / AROUND as f32 * TAU;
    Vec2::new(a.cos(), a.sin())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_skirt_wraps_both_legs_and_flares_to_the_hem() {
        // Hips 34 cm wide down to the crotch, then two thighs 14 cm across with a gap.
        let mut pts = Vec::new();
        for i in 0..30 {
            let y = 0.11 - i as f32 * 0.014;
            for j in 0..24 {
                let a = j as f32 / 24.0 * TAU;
                if y > -0.05 {
                    pts.push(Vec3::new(0.17 * a.cos(), y, 0.11 * a.sin()));
                } else {
                    for x in [-0.08f32, 0.08] {
                        pts.push(Vec3::new(x + 0.07 * a.cos(), y, 0.07 * a.sin()));
                    }
                }
            }
        }
        let ids: Vec<u32> = (0..pts.len() as u32).collect();
        let mut m = MouthMesh::default();
        skirt(&mut m, &pts, &ids, 0.105, -0.3, &SKIRT);
        assert_eq!(m.positions.len(), SLICES * AROUND);
        assert!(m.parts.iter().all(|p| *p == MouthPart::Skirt));
        // Front and center at the hem: the skirt spans the gap between the legs, outside both.
        let hem = &m.positions[(SLICES - 1) * AROUND..];
        let front = hem
            .iter()
            .filter(|p| p.x.abs() < 0.03 && p.z > 0.0)
            .map(|p| p.z)
            .fold(f32::MIN, f32::max);
        assert!(front > 0.11, "{front}");
        let side = hem.iter().map(|p| p.x).fold(f32::MIN, f32::max);
        assert!(side > 0.17 + SKIRT.flare * 0.3, "{side}");
        // Normals face out of the skirt.
        let mid = SLICES / 2;
        for (p, n) in m.positions[mid * AROUND..(mid + 1) * AROUND]
            .iter()
            .zip(&m.normals[mid * AROUND..])
        {
            assert!(
                Vec2::new(p.x, p.z).dot(Vec2::new(n.x, n.z)) > 0.0,
                "{p} {n}"
            );
        }
    }

    #[test]
    fn a_tabard_hangs_in_two_panels_seen_from_both_sides() {
        let mut pts = Vec::new();
        for i in 0..36 {
            let y = 0.13 - i as f32 * 0.014;
            for j in 0..24 {
                let a = j as f32 / 24.0 * TAU;
                pts.push(Vec3::new(0.17 * a.cos(), y, 0.11 * a.sin()));
            }
        }
        let ids: Vec<u32> = (0..pts.len() as u32).collect();
        let mut m = MouthMesh::default();
        skirt(&mut m, &pts, &ids, 0.125, -0.34, &TABARD);
        assert!(m.parts.iter().all(|p| *p == MouthPart::Tabard));
        // Every drawn point is in front or behind, within the panels' width (at the sides
        // there is nothing).
        let drawn: Vec<Vec3> = m.indices.iter().map(|i| m.positions[*i as usize]).collect();
        assert!(!drawn.is_empty());
        for p in &drawn {
            assert!(p.x.abs() < 0.18 && p.z.abs() > 0.05, "{p}");
        }
        // Each panel is drawn twice, its far side facing the other way.
        let front = m.indices.len() / 2;
        let n = |t: &[u32]| {
            let [a, b, c] = [t[0], t[1], t[2]].map(|i| m.positions[i as usize]);
            (b - a).cross(c - a)
        };
        let (near, far) = (n(&m.indices[..3]), n(&m.indices[front..front + 3]));
        assert!(near.dot(far) < 0.0);
    }
}
