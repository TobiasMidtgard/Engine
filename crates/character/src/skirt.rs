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

/// Rest height of the waistband (`WAIST` in common.wgsl), and of the hem, a hand above the
/// knees.
const WAIST: f32 = 0.105;
const HEM: f32 = -0.3;
/// Cross-sections from waist to hem, and points around each.
const SLICES: usize = 24;
const AROUND: usize = 48;
/// How far the cloth stands off the body, m, and how far out the hem flares beyond the seat.
const EASE: f32 = 0.006;
const FLARE: f32 = 0.05;
/// Folds round the hem: how many, and how deep, m.
const FOLDS: f32 = 11.0;
const FOLD_DEPTH: f32 = 0.012;

/// The skirt of a morphed body (`morphed`: the pack's vertices after the morphs). Empty when
/// the body has no hips and legs.
pub fn build(pack: &HumanPack, morphed: &[Vec3]) -> MouthMesh {
    let mut m = MouthMesh::default();
    let ids: Vec<u32> = pack
        .group_vertices("body")
        .into_iter()
        .filter(|&v| (HEM - 0.02..WAIST + 0.01).contains(&pack.positions[v as usize].y))
        .collect();
    if ids.len() < 32 {
        return m;
    }
    let pts: Vec<Vec3> = ids.iter().map(|&v| morphed[v as usize]).collect();
    // The waist and hem heights on this body: where its rest waist and hem went.
    let height_of = |rest_y: f32| {
        let near: Vec<f32> = ids
            .iter()
            .filter(|&&v| (pack.positions[v as usize].y - rest_y).abs() < 0.01)
            .map(|&v| morphed[v as usize].y)
            .collect();
        near.iter().sum::<f32>() / near.len().max(1) as f32
    };
    skirt(&mut m, &pts, &ids, height_of(WAIST), height_of(HEM));
    m
}

fn skirt(m: &mut MouthMesh, pts: &[Vec3], ids: &[u32], top: f32, hem: f32) {
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
            *r = r.max(s) + FLARE * t * t * front_back(j);
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
        let depth = FOLD_DEPTH * t.powf(1.5);
        for (j, r) in row.iter().enumerate() {
            let a = j as f32 / AROUND as f32 * TAU;
            let fold = (a * FOLDS + 0.6 * (a * 3.0).sin()).cos() * 0.5 + 0.5;
            let q = *c + dir(j) * (r + EASE + depth * fold * front_back(j));
            push(m, Vec3::new(q.x, y_at(k), q.y), MouthPart::Skirt, pts, ids);
        }
    }
    let at = |k: usize, j: usize| first + (k * AROUND + j % AROUND) as u32;
    let start = m.indices.len();
    for k in 0..SLICES - 1 {
        for j in 0..AROUND {
            let (a, b, c, d) = (at(k, j), at(k, j + 1), at(k + 1, j), at(k + 1, j + 1));
            m.indices.extend([a, c, b, b, c, d]);
        }
    }
    // Face outward: flip if a band's normal points into the skirt.
    let mid = SLICES / 2;
    let probe = mid * AROUND * 6 + start;
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
    let ring = |i: usize| (i - first) / AROUND;
    let rows = (skin.positions.len() - first).div_ceil(AROUND);
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
        skirt(&mut m, &pts, &ids, 0.105, -0.3);
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
        assert!(side > 0.17 + FLARE * 0.3, "{side}");
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
}
