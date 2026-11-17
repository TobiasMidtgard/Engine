//! Hair at a distance as locks: each clump of hairs drawn as one soft strand along their middle,
//! as wide as they spread and as dense as they are.
//!
//! Thinning a head of hair to a random share of wider hairs ([`crate::lod::strands`]) keeps its
//! colour and coverage, but a few thousand stray hairs scattered over the hairdo dissolve its
//! shape: the locks it falls in, its parting and its outline go to grain. Games keep a groom's
//! shape far away by drawing its clusters instead of its hairs (the "thick hairs" of Real-time
//! LOD strand hair, 2024: each cluster an elliptical strand sized to cover its members, chosen
//! by how wide it shows on screen). Here a lock is one per guide (the hairs that move with it, so
//! it swings with them) or, for hair without guides, the hairs whose roots share a cell of the
//! scalp. Each point of a lock has an elliptical cross-section measured from its hairs (wide
//! along the scalp, thin off it, so a lock lying flat on the head doesn't puff out the
//! silhouette) and their summed width, from which the shader takes how much of what lies behind
//! the lock shows through: a lock of a few hairs is wispy, a lock of a hundred is solid in the
//! middle and soft at its edges.

use std::collections::HashMap;

use engine_core::math::Vec3;

use crate::face::f16_bits;
use crate::strands::{Follow, PackedStrands, StrandKind, StrandSet};

/// Hairs a lock gathers (about) where hairs grow without guides.
pub const HAIRS_PER_LOCK: usize = 64;
/// Half a lock's width, in standard deviations of its hairs' spread (a lock's hairs fall about
/// evenly across it, and an even spread of half width `a` has a deviation of `a / √3`).
const SPREAD: f32 = 1.75;
/// Hair of this many points or fewer (a buzz cut) keeps its hairs: they are cheap, and a crop
/// shorter than its locks are wide has no locks to speak of. Beards and stubble likewise: a
/// beard's curls drawn as locks read as dashes.
const SHORT_POINTS: usize = 8;
/// A lock whose hairs' tips spread wider than this (deviation, m) is split in two along the
/// spread: hairs from one guide can part over a shoulder, and short broken hairs end far from
/// the long ones, and drawn as one their lock would be a wide flat slab.
const MAX_TIP_SPREAD: f32 = 0.015;
/// The fewest hairs a split lock keeps.
const MIN_SPLIT: usize = 6;
/// Sets with fewer hairs than this keep them (they are cheap, and too few to clump).
const MIN_HAIRS: usize = 2 * HAIRS_PER_LOCK;

/// A strand set's locks.
#[derive(Debug, Clone)]
pub struct HairLocks {
    /// One strand per lock: points along the middle of its hairs (radius: half its width
    /// along the major axis), attrs as for hairs (mean occlusion, a member's random value).
    /// `follow` holds the guides it moves with (empty for hair that holds its shape).
    pub set: StrandSet,
    /// Per point: the cross-section's major axis (octahedral, snorm16 x and y), then the minor
    /// half width over the major and the hairs' summed radii over the major half width (half
    /// floats). See `lock_width` in strand_data.wgsl.
    pub shape: Vec<[u32; 2]>,
    /// Per lock, the hair whose root it takes (the one nearest the lock's middle).
    pub members: Vec<u32>,
    /// `set` packed for the GPU.
    pub packed: PackedStrands,
}

impl HairLocks {
    pub fn count(&self) -> usize {
        self.members.len()
    }
}

/// The locks of `set` (`None` for brows and lashes, and sets too small to gather).
pub fn locks(set: &StrandSet) -> Option<HairLocks> {
    let n = set.points_per as usize;
    let hairs = set.strand_count();
    if set.kind != StrandKind::Scalp
        || hairs < MIN_HAIRS
        || n <= SHORT_POINTS
        || set.points.len() != hairs * n
    {
        return None;
    }
    let at = |h: usize, k: usize| {
        let p = set.points[h * n + k];
        Vec3::new(p[0], p[1], p[2])
    };
    let guided = set.follow.len() == hairs && !set.guides.is_empty();
    let clusters = if guided {
        let mut by_guide: HashMap<u32, Vec<u32>> = HashMap::new();
        for (h, f) in set.follow.iter().enumerate() {
            by_guide.entry(f.guides[0]).or_default().push(h as u32);
        }
        let mut c: Vec<(u32, Vec<u32>)> = by_guide.into_iter().collect();
        c.sort_by_key(|(g, _)| *g);
        c.into_iter().map(|(_, m)| m).collect()
    } else {
        grid_clusters(&(0..hairs).map(|h| at(h, 0)).collect::<Vec<_>>())
    };
    let tips: Vec<Vec3> = (0..hairs).map(|h| at(h, n - 1)).collect();
    let mut split = Vec::with_capacity(clusters.len() * 2);
    for c in clusters {
        split_by_tips(c, &tips, &mut split, 0);
    }
    let clusters = split;
    // Longer hairs pull a lock's middle toward them: the outline of a hairdo is its longest
    // hairs, and a mean of long and short ones would crop it.
    let length: Vec<f32> = (0..hairs)
        .map(|h| {
            (1..n)
                .map(|k| (at(h, k) - at(h, k - 1)).length())
                .sum::<f32>()
                .max(1e-4)
        })
        .collect();
    let mut out = HairLocks {
        set: StrandSet::new(set.kind, set.points_per),
        shape: Vec::new(),
        members: Vec::new(),
        packed: PackedStrands::default(),
    };
    let mut middle = vec![Vec3::ZERO; n];
    for members in clusters.iter().filter(|m| !m.is_empty()) {
        let total: f32 = members.iter().map(|h| length[*h as usize]).sum();
        for (k, m) in middle.iter_mut().enumerate() {
            *m = members
                .iter()
                .map(|h| at(*h as usize, k) * length[*h as usize])
                .fold(Vec3::ZERO, |a, b| a + b)
                / total;
        }
        let rep = *members
            .iter()
            .min_by(|a, b| {
                let da = (at(**a as usize, 0) - middle[0]).length_squared();
                let db = (at(**b as usize, 0) - middle[0]).length_squared();
                da.total_cmp(&db)
            })
            .unwrap();
        let random = set.attrs[rep as usize * n][1];
        for k in 0..n {
            let ahead = middle[(k + 1).min(n - 1)];
            let behind = middle[k.saturating_sub(1)];
            let t = (ahead - behind).normalize_or(Vec3::Y);
            let (mut cov, mut radius, mut occ) = ([[0.0f32; 3]; 3], 0.0, 0.0);
            for h in members {
                let i = *h as usize * n + k;
                let w = length[*h as usize] / total;
                let d = at(*h as usize, k) - middle[k];
                let d = d - t * d.dot(t);
                let d = d.to_array();
                for (r, row) in cov.iter_mut().enumerate() {
                    for (c, v) in row.iter_mut().enumerate() {
                        *v += w * d[r] * d[c];
                    }
                }
                radius += set.points[i][3];
                occ += w * set.attrs[i][0];
            }
            let apply = |v: Vec3| {
                Vec3::new(
                    cov[0][0] * v.x + cov[0][1] * v.y + cov[0][2] * v.z,
                    cov[1][0] * v.x + cov[1][1] * v.y + cov[1][2] * v.z,
                    cov[2][0] * v.x + cov[2][1] * v.y + cov[2][2] * v.z,
                )
            };
            // The spread's widest direction (power iteration, kept square to the lock).
            // (Started between two axes square to it, so it is no eigenvector itself.)
            let a = t.any_orthonormal_vector();
            let mut u = (a + t.cross(a)).normalize();
            for _ in 0..16 {
                let next = apply(u);
                let next = next - t * next.dot(t);
                if next.length_squared() < 1e-30 {
                    break;
                }
                u = next.normalize();
            }
            let v = t.cross(u).normalize_or(Vec3::X);
            let hair_r = radius / members.len() as f32;
            // A lock narrows to its tip, as its hairs end one by one, instead of ending square.
            let along = k as f32 / (n - 1) as f32;
            let end = ((along - 0.75) / 0.25).clamp(0.0, 1.0);
            let taper = 1.0 - 0.5 * end * end * (3.0 - 2.0 * end);
            let major = (SPREAD * u.dot(apply(u)).max(0.0).sqrt()) * taper + hair_r;
            let minor = ((SPREAD * v.dot(apply(v)).max(0.0).sqrt()) * taper + hair_r).min(major);
            let p = middle[k];
            out.set.points.push([p.x, p.y, p.z, major]);
            out.set
                .attrs
                .push([occ, random, k as f32 / (n - 1) as f32, 0.0]);
            let snorm = |x: f32| ((x.clamp(-1.0, 1.0) * 32767.0).round() as i32 as u32) & 0xffff;
            let o = octahedral(u);
            out.shape.push([
                snorm(o[0]) | snorm(o[1]) << 16,
                f16_bits(minor / major) as u32 | (f16_bits(radius / major) as u32) << 16,
            ]);
        }
        if guided {
            out.set.follow.push(blend_follows(
                members.iter().map(|h| &set.follow[*h as usize]),
            ));
        }
        out.members.push(rep);
    }
    out.packed = out.set.packed();
    Some(out)
}

/// Split `members` along their tips' widest spread until it is under [`MAX_TIP_SPREAD`].
fn split_by_tips(members: Vec<u32>, tips: &[Vec3], out: &mut Vec<Vec<u32>>, depth: u32) {
    if members.len() < 2 * MIN_SPLIT || depth > 8 {
        out.push(members);
        return;
    }
    let mean = members.iter().map(|h| tips[*h as usize]).sum::<Vec3>() / members.len() as f32;
    let mut cov = [[0.0f32; 3]; 3];
    for h in &members {
        let d = (tips[*h as usize] - mean).to_array();
        for (r, row) in cov.iter_mut().enumerate() {
            for (c, v) in row.iter_mut().enumerate() {
                *v += d[r] * d[c] / members.len() as f32;
            }
        }
    }
    let apply = |v: Vec3| {
        Vec3::new(
            cov[0][0] * v.x + cov[0][1] * v.y + cov[0][2] * v.z,
            cov[1][0] * v.x + cov[1][1] * v.y + cov[1][2] * v.z,
            cov[2][0] * v.x + cov[2][1] * v.y + cov[2][2] * v.z,
        )
    };
    let mut axis = Vec3::new(0.577, 0.577, 0.577);
    for _ in 0..16 {
        let next = apply(axis);
        if next.length_squared() < 1e-30 {
            break;
        }
        axis = next.normalize();
    }
    if axis.dot(apply(axis)) < MAX_TIP_SPREAD * MAX_TIP_SPREAD {
        out.push(members);
        return;
    }
    let (a, b): (Vec<u32>, Vec<u32>) = members
        .iter()
        .partition(|h| (tips[**h as usize] - mean).dot(axis) < 0.0);
    if a.len() < MIN_SPLIT || b.len() < MIN_SPLIT {
        out.push(members);
        return;
    }
    split_by_tips(a, tips, out, depth + 1);
    split_by_tips(b, tips, out, depth + 1);
}

/// Gather hairs by their roots into cells of the scalp sized for about [`HAIRS_PER_LOCK`] each.
fn grid_clusters(roots: &[Vec3]) -> Vec<Vec<u32>> {
    let target = (roots.len() / HAIRS_PER_LOCK).max(1) as f32;
    let key = |p: Vec3, cell: f32| (p / cell).floor().as_ivec3().to_array();
    let mut cell = 0.01f32;
    let mut cells: HashMap<[i32; 3], Vec<u32>> = HashMap::new();
    for _ in 0..5 {
        cells.clear();
        for (h, p) in roots.iter().enumerate() {
            cells.entry(key(*p, cell)).or_default().push(h as u32);
        }
        let ratio = cells.len() as f32 / target;
        if (0.85..1.2).contains(&ratio) {
            break;
        }
        // Cells on a surface: their count goes with the inverse square of their size.
        cell *= ratio.sqrt().clamp(0.5, 2.0);
    }
    let mut c: Vec<([i32; 3], Vec<u32>)> = cells.into_iter().collect();
    c.sort_by_key(|(k, _)| *k);
    c.into_iter().map(|(_, m)| m).collect()
}

/// The guides a lock moves with: its hairs' guides, weighted by how much of each they take,
/// the three strongest.
fn blend_follows<'a>(follows: impl Iterator<Item = &'a Follow>) -> Follow {
    let mut sum: HashMap<u32, f32> = HashMap::new();
    for f in follows {
        for (g, w) in f.guides.iter().zip(f.weights) {
            if *g != u32::MAX && w > 0.0 {
                *sum.entry(*g).or_default() += w;
            }
        }
    }
    let mut best: Vec<(u32, f32)> = sum.into_iter().collect();
    best.sort_by(|a, b| b.1.total_cmp(&a.1).then(a.0.cmp(&b.0)));
    best.truncate(3);
    let total: f32 = best.iter().map(|b| b.1).sum::<f32>().max(1e-6);
    let mut out = Follow::NONE;
    for (i, (g, w)) in best.into_iter().enumerate() {
        out.guides[i] = g;
        out.weights[i] = w / total;
    }
    out
}

/// A unit vector folded onto the octahedron and flattened (both in -1..1); the renderer's
/// `oct_decode` (lib_types.wgsl) unfolds it.
fn octahedral(v: Vec3) -> [f32; 2] {
    let v = v / (v.x.abs() + v.y.abs() + v.z.abs()).max(1e-9);
    if v.z >= 0.0 {
        [v.x, v.y]
    } else {
        [
            (1.0 - v.y.abs()) * v.x.signum(),
            (1.0 - v.x.abs()) * v.y.signum(),
        ]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Straight hairs hanging down (−Y) from roots spread over a patch `wide` × `deep` (m).
    fn patch(hairs: usize, wide: f32, deep: f32) -> StrandSet {
        let mut s = StrandSet::new(StrandKind::Scalp, 12);
        let mut r = crate::appearance::Rng::new(7);
        for _ in 0..hairs {
            let x = (r.f32() - 0.5) * wide;
            let z = (r.f32() - 0.5) * deep;
            for k in 0..12 {
                s.points.push([x, 1.6 - k as f32 * 0.02, z, 5e-5]);
                s.attrs.push([0.8, 0.3, k as f32 / 11.0, 0.0]);
            }
        }
        s
    }

    #[test]
    fn a_lock_covers_its_hairs_flat_where_they_lie_flat() {
        let set = patch(500, 0.04, 0.0005);
        // (Straight hairs: their tips spread as their roots, so the cells stay whole.)
        let locks = locks(&set).unwrap();
        // About one lock per 64 hairs.
        assert!((4..=16).contains(&locks.count()), "{}", locks.count());
        // Every lock's widest spread runs along x, its half width about half its cell's.
        for (k, p) in locks.set.points.iter().enumerate() {
            let w = locks.shape[k][0];
            let ox = (w & 0xffff) as u16 as i16 as f32 / 32767.0;
            let oy = (w >> 16) as u16 as i16 as f32 / 32767.0;
            assert!(ox.abs() > 0.9 && oy.abs() < 0.1, "{ox} {oy}");
            assert!(p[3] > 0.001 && p[3] < 0.02, "{}", p[3]);
        }
        // The summed widths of the hairs over the lock's: as many hairs lie across it.
        let ratio = |w: u32| crate::face::f16_value((w >> 16) as u16);
        let dense = ratio(locks.shape[0][1]);
        assert!(dense > 0.1, "{dense}");
    }

    #[test]
    fn hairs_that_part_make_two_locks() {
        // Roots in one cell, half the hairs falling left and half right.
        let mut s = StrandSet::new(StrandKind::Scalp, 12);
        let mut r = crate::appearance::Rng::new(3);
        for h in 0..200 {
            let root = Vec3::new(r.f32() * 0.0005, 1.6, r.f32() * 0.0005);
            let side = if h % 2 == 0 { -1.0 } else { 1.0 };
            for k in 0..12 {
                let t = k as f32 / 11.0;
                let p = root + Vec3::new(side * 0.06 * t, -0.2 * t, 0.0);
                s.points.push([p.x, p.y, p.z, 5e-5]);
                s.attrs.push([0.8, 0.3, t, 0.0]);
            }
        }
        let locks = locks(&s).unwrap();
        assert!(locks.count() >= 2, "{}", locks.count());
        // No lock's tip is as wide as the parting.
        for l in 0..locks.count() {
            assert!(locks.set.points[l * 12 + 11][3] < 0.02);
        }
    }

    #[test]
    fn locks_follow_their_hairs_guides() {
        let f = |g: [u32; 3], w: [f32; 3]| Follow {
            guides: g,
            weights: w,
        };
        let b = blend_follows(
            [
                f([3, 4, u32::MAX], [0.75, 0.25, 0.0]),
                f([3, 5, 4], [0.5, 0.3, 0.2]),
            ]
            .iter(),
        );
        assert_eq!(b.guides, [3, 4, 5]);
        assert!((b.weights.iter().sum::<f32>() - 1.0).abs() < 1e-5);
        assert!(b.weights[0] > 0.6);
    }

    #[test]
    fn octahedral_round_trips() {
        for v in [Vec3::X, -Vec3::Z, Vec3::new(0.3, -0.5, -0.8).normalize()] {
            let [x, y] = octahedral(v);
            let mut d = Vec3::new(x, y, 1.0 - x.abs() - y.abs());
            if d.z < 0.0 {
                let (a, b) = (d.x, d.y);
                d.x = (1.0 - b.abs()) * a.signum();
                d.y = (1.0 - a.abs()) * b.signum();
            }
            assert!((d.normalize() - v).length() < 1e-4, "{v} {d}");
        }
    }
}
