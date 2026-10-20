//! Shoes: a closed leather shell over each foot, so people stop walking on shoe-colored bare
//! feet with every toe showing.
//!
//! The shell is lofted along the foot from heel to toe. Each cross-section is the convex hull
//! of the foot there (which bridges the gaps between the toes into one toe box), smoothed
//! along the foot and around it, and set a few millimetres out from the skin. Every shell
//! vertex follows the three nearest foot vertices, like the teeth follow the jaw, so the shoes
//! bend with the feet and fit any body. The skin under the shoes (rest height below
//! [`SHOE_TOP`], as in `common.wgsl`) stays inside the shell.

use engine_core::math::{Vec2, Vec3};

use crate::mouth::{MouthMesh, MouthPart, push, smooth_normals};
use crate::pack::HumanPack;

/// Rest height below which the skin is inside a shoe (`SHOE_TOP` in common.wgsl).
pub const SHOE_TOP: f32 = -0.715;

/// Cross-sections from heel to toe, and points around each.
const SLICES: usize = 28;
const AROUND: usize = 32;

/// How far the leather stands off the skin, m.
const LEATHER: f32 = 0.004;

/// Both shoes of a morphed body (`morphed`: the pack's vertices after the morphs). Empty when
/// the body has no feet.
pub fn build(pack: &HumanPack, morphed: &[Vec3]) -> MouthMesh {
    let mut m = MouthMesh::default();
    let body = pack.group_vertices("body");
    for side in [-1.0f32, 1.0] {
        let ids: Vec<u32> = body
            .iter()
            .copied()
            .filter(|&v| {
                let r = pack.positions[v as usize];
                r.y < SHOE_TOP && r.x * side > 0.0
            })
            .collect();
        if ids.len() < 16 {
            continue;
        }
        let pts: Vec<Vec3> = ids.iter().map(|&v| morphed[v as usize]).collect();
        shoe(&mut m, &pts, &ids);
    }
    m
}

/// Convex hull of 2D points (counter-clockwise).
fn hull(mut p: Vec<Vec2>) -> Vec<Vec2> {
    p.sort_by(|a, b| a.x.total_cmp(&b.x).then(a.y.total_cmp(&b.y)));
    p.dedup();
    if p.len() < 3 {
        return p;
    }
    let cross = |o: Vec2, a: Vec2, b: Vec2| (a - o).perp_dot(b - o);
    let mut h: Vec<Vec2> = Vec::with_capacity(p.len() * 2);
    for pass in 0..2 {
        let start = h.len();
        let order: Vec<Vec2> = if pass == 0 {
            p.clone()
        } else {
            p.iter().rev().copied().collect()
        };
        for q in order {
            while h.len() >= start + 2 && cross(h[h.len() - 2], h[h.len() - 1], q) <= 0.0 {
                h.pop();
            }
            h.push(q);
        }
        h.pop();
    }
    h
}

/// Distance from `c` (inside) to the hull's edge along `d`.
fn reach(h: &[Vec2], c: Vec2, d: Vec2) -> f32 {
    let mut best = f32::MAX;
    for i in 0..h.len() {
        let (a, b) = (h[i] - c, h[(i + 1) % h.len()] - c);
        let e = b - a;
        let den = d.perp_dot(e);
        if den.abs() < 1e-9 {
            continue;
        }
        let t = a.perp_dot(e) / den;
        let u = a.perp_dot(d) / den;
        if t > 0.0 && (-1e-4..=1.0 + 1e-4).contains(&u) {
            best = best.min(t);
        }
    }
    if best == f32::MAX { 0.0 } else { best }
}

fn shoe(m: &mut MouthMesh, pts: &[Vec3], ids: &[u32]) {
    let ground = pts.iter().map(|p| p.y).fold(f32::MAX, f32::min);
    let (z0, z1) = pts
        .iter()
        .fold((f32::MAX, f32::MIN), |(a, b), p| (a.min(p.z), b.max(p.z)));
    let step = (z1 - z0) / SLICES as f32;
    // Each cross-section: its center and its reach in every direction around.
    let mut centers: Vec<Option<Vec2>> = Vec::with_capacity(SLICES);
    let mut radii = vec![[0.0f32; AROUND]; SLICES];
    for (k, row) in radii.iter_mut().enumerate() {
        let z = z0 + (k as f32 + 0.5) * step;
        let near: Vec<Vec2> = pts
            .iter()
            .filter(|p| (p.z - z).abs() < step * 0.9)
            .map(|p| Vec2::new(p.x, p.y))
            .collect();
        let h = hull(near);
        if h.len() < 3 {
            centers.push(None);
            continue;
        }
        let (lo, hi) = h
            .iter()
            .fold((Vec2::MAX, Vec2::MIN), |(a, b), q| (a.min(*q), b.max(*q)));
        let c = Vec2::new((lo.x + hi.x) * 0.5, (ground + hi.y) * 0.5);
        for (j, r) in row.iter_mut().enumerate() {
            let a = j as f32 / AROUND as f32 * std::f32::consts::TAU;
            *r = reach(&h, c, Vec2::new(a.cos(), a.sin()));
        }
        centers.push(Some(c));
    }
    // Empty sections take their nearest neighbor's shape.
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
    // Smooth along the foot and around each section (a shoe has no toe-sized bumps).
    for _ in 0..3 {
        let r0 = radii.clone();
        let c0 = centers.clone();
        for k in 0..SLICES {
            let (a, b) = (k.saturating_sub(1), (k + 1).min(SLICES - 1));
            for j in 0..AROUND {
                let around = (r0[k][(j + AROUND - 1) % AROUND] + r0[k][(j + 1) % AROUND]) * 0.5;
                radii[k][j] = (r0[a][j] + r0[b][j] + 2.0 * r0[k][j] + 2.0 * around) / 6.0;
            }
            centers[k] = (c0[a] + c0[b] + c0[k] * 2.0) * 0.25;
        }
    }
    // A rounded toe box: the last sections narrow like a shoe's, not to the point of the big
    // toe.
    let ball = radii[SLICES - 5];
    for (k, f) in [
        (SLICES - 4, 0.96),
        (SLICES - 3, 0.9),
        (SLICES - 2, 0.8),
        (SLICES - 1, 0.6),
    ] {
        for j in 0..AROUND {
            radii[k][j] = radii[k][j].max(ball[j] * f);
        }
    }
    let z_at = |k: usize| z0 + (k as f32 + 0.5) * step;
    let point = |k: usize, j: usize| {
        let a = j as f32 / AROUND as f32 * std::f32::consts::TAU;
        let q = centers[k] + Vec2::new(a.cos(), a.sin()) * (radii[k][j] + LEATHER);
        // The sole stays on the ground.
        Vec3::new(q.x, q.y.max(ground), z_at(k))
    };
    let first = m.positions.len() as u32;
    for k in 0..SLICES {
        for j in 0..AROUND {
            push(m, point(k, j), MouthPart::Shoe, pts, ids);
        }
    }
    // Rounded heel and toe: a cap point just past each end, low on the foot.
    let cap = |k: usize, out: f32| {
        let c = centers[k];
        Vec3::new(
            c.x,
            (c.y - 0.25 * (c.y - ground)).max(ground),
            z_at(k) + out,
        )
    };
    let heel = m.positions.len() as u32;
    push(m, cap(0, -step * 0.5 - LEATHER), MouthPart::Shoe, pts, ids);
    let toe = m.positions.len() as u32;
    push(
        m,
        cap(SLICES - 1, step * 0.5 + LEATHER),
        MouthPart::Shoe,
        pts,
        ids,
    );
    let at = |k: usize, j: usize| first + (k * AROUND + j % AROUND) as u32;
    let start = m.indices.len();
    for k in 0..SLICES - 1 {
        for j in 0..AROUND {
            let (a, b, c, d) = (at(k, j), at(k, j + 1), at(k + 1, j), at(k + 1, j + 1));
            m.indices.extend([a, c, b, b, c, d]);
        }
    }
    for j in 0..AROUND {
        m.indices.extend([heel, at(0, j), at(0, j + 1)]);
        m.indices
            .extend([toe, at(SLICES - 1, j + 1), at(SLICES - 1, j)]);
    }
    // Face outward: flip if a side band's normal points into the shoe.
    let mid = SLICES / 2;
    let probe = (mid * AROUND * 6) + start;
    let (a, b, c) = (
        m.positions[m.indices[probe] as usize],
        m.positions[m.indices[probe + 1] as usize],
        m.positions[m.indices[probe + 2] as usize],
    );
    let center = Vec3::new(centers[mid].x, centers[mid].y, z_at(mid));
    if (b - a).cross(c - a).dot(a - center) < 0.0 {
        for t in m.indices[start..].as_chunks_mut::<3>().0 {
            t.swap(1, 2);
        }
    }
    smooth_normals(m, first);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_hull_bridges_gaps_and_reaches_its_edges() {
        // Two "toes" with a gap between them: the hull spans both.
        let h = hull(vec![
            Vec2::new(0.0, 0.0),
            Vec2::new(0.01, 0.0),
            Vec2::new(0.005, 0.01),
            Vec2::new(0.03, 0.0),
            Vec2::new(0.04, 0.0),
            Vec2::new(0.035, 0.01),
            Vec2::new(0.02, 0.002),
        ]);
        assert_eq!(h.len(), 4);
        let r = reach(&h, Vec2::new(0.02, 0.004), Vec2::Y);
        assert!((r - 0.006).abs() < 1e-4, "{r}");
    }

    #[test]
    fn a_shoe_encloses_its_foot_and_faces_out() {
        // A box-shaped foot of points: 10 cm wide, 5 cm high, 25 cm long, on the ground.
        let mut pts = Vec::new();
        for i in 0..6 {
            for j in 0..4 {
                for k in 0..12 {
                    pts.push(Vec3::new(
                        i as f32 * 0.02,
                        -0.8 + j as f32 * 0.0167,
                        k as f32 * 0.0227,
                    ));
                }
            }
        }
        let ids: Vec<u32> = (0..pts.len() as u32).collect();
        let mut m = MouthMesh::default();
        shoe(&mut m, &pts, &ids);
        assert_eq!(m.positions.len(), SLICES * AROUND + 2);
        for (p, n) in m.positions.iter().zip(&m.normals) {
            assert!(p.y >= -0.8 - 1e-6);
            assert!(p.is_finite() && n.is_finite());
        }
        // The shell is outside the foot, and its normals point away from the middle.
        let center = Vec3::new(0.05, -0.775, 0.125);
        for (i, (p, n)) in m.positions[..SLICES * AROUND]
            .iter()
            .zip(&m.normals)
            .enumerate()
        {
            // Straight out to the sides and up, the shell clears the foot by the leather.
            match i % AROUND {
                0 => assert!(p.x > 0.102, "{p}"),
                8 => assert!(p.y > -0.749, "{p}"),
                16 => assert!(p.x < -0.002, "{p}"),
                _ => {}
            }
            if p.y > -0.79 {
                assert!((*p - center).dot(*n) > 0.0, "{p} {n}");
            }
        }
        assert!(
            m.anchors
                .iter()
                .all(|a| a.iter().all(|(v, _)| *v < pts.len() as u32))
        );
    }
}
