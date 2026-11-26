//! The body as a person's hair feels it: [`HairBody`], the signed distance to the drawn skin
//! and to the clothes drawn over it, in boxes that ride the bones.
//!
//! The hair simulation only sees a skeleton, and held hair out of capsules fitted to it; the
//! skin stands up to two centimetres out of those, and a shirt a few millimetres more, so
//! locks lay in the chest and the shoulders. Here the drawn surface itself is measured: every
//! drawn vertex of the built skin (the clothes' layer lifted as the shaders lift it, the skin
//! hidden under it left out), carried to the model space the hair's guides are in, and the
//! bones that carry them noted. Each bone that moves a fair share of the surface the hair can
//! reach gets a box around what it moves, and in it, sampled on a lattice, the distance to
//! the whole body as it stood when the hair was grown, and how much of the skin nearest each
//! sample the bone carries. The readers ([`at`]) ask where a point was when the hair was
//! grown: as the bones that carry the skin near it have moved since, blended by their shares
//! as the skin itself is skinned, undone. There the body is read.
//!
//! (Two readings before this one failed where it mattered. Boxes that held the whole body and
//! were blended by distance: under a turned head the head's box, carried round, found a lock
//! on the shoulder clear of the neck, the shoulder's box found it inside the shoulder, and
//! their mean let it sink in. Boxes that each held only the skin their own bone carries,
//! the nearest of them the body: with the arms lowered from how the hair was grown, an arm's
//! box swung its side of the shoulder up beside the neck, and the hair was thrown off a
//! shoulder that was not there.)

use std::collections::HashMap;
use std::sync::Arc;

use engine_core::prelude::*;
use engine_world::animation::SkeletonPose;
use engine_world::{HairBody, HairBodyPart, HairGuides};

use crate::appearance::Appearance;
use crate::build::CharacterMeshes;
use crate::clothes::{Cut, Top};
use crate::mesh::smoothstep;

/// Spacing of the samples, m.
const CELL: f32 = 0.008;
/// How far around the surface a bone's box reaches, m.
const BAND: f32 = 0.032;
/// A bone gets a box when it carries at least this share of this many drawn vertices.
const CARRIES: u8 = 26;
const FEW: usize = 40;
/// At most this many boxes (the bones that carry most of the surface).
pub const MOST_PARTS: usize = 12;
/// How far the shaders lift the clothes' layer off its vertices (`cloth_lift` in common.wgsl:
/// a shirt, a padded coat, mail over one, trousers), m.
const LIFT_SHIRT: f32 = 0.004;
const LIFT_PADDED: f32 = 0.011;
const LIFT_MAIL: f32 = 0.013;
const LIFT_TROUSERS: f32 = 0.006;
/// The lift fades in over this distance from the layer's edges (neckline, cuffs), m.
const LIFT_EDGE: f32 = 0.006;
/// Rest height of the waistband (`WAIST` in common.wgsl).
const WAIST: f32 = 0.105;
/// Within this of the nearest vertex the distance is measured off its tangent plane (the
/// surface between vertices), beyond twice this straight to it.
const ON_PLANE: f32 = 0.006;

/// The drawn surface's points for the hair: model-space position and outward normal, and the
/// pose bones that carry each (weights out of 255).
struct Surface {
    at: Vec<Vec3>,
    out: Vec<Vec3>,
    bones: Vec<[u16; 4]>,
    weights: Vec<[u8; 4]>,
    /// The drawn triangles between them (and those that reach under the clothes' edges).
    faces: Vec<[Vec3; 3]>,
}

fn key(p: Vec3, cell: f32) -> [i32; 3] {
    [
        (p.x / cell).floor() as i32,
        (p.y / cell).floor() as i32,
        (p.z / cell).floor() as i32,
    ]
}

/// Every drawn vertex of the skin in the rest pose's model space, clothes lifted, hidden skin
/// and the mouth's inside left out.
fn surface(meshes: &CharacterMeshes, a: &Appearance, pose: &SkeletonPose) -> Option<Surface> {
    let rig = meshes.rig.as_ref()?;
    let skin = &meshes.skin;
    let n = skin.positions.len();
    if skin.bones.len() != n || skin.weights.len() != n {
        return None;
    }
    let rest = rig.rest_pose();
    let mut palette = Vec::new();
    rig.palette(&rest, &rig.map(&rest), &mut palette);
    // Each rig bone's bone in the pose: its own, or the nearest parent's the pose has.
    let map = rig.map(pose);
    let in_pose: Vec<Option<u16>> = (0..rig.bones.len())
        .map(|mut b| {
            loop {
                if let Some(p) = map[b] {
                    return Some(p as u16);
                }
                b = rig.parents[b]?;
            }
        })
        .collect();

    let mut drawn = vec![false; n];
    for &i in &skin.indices {
        drawn[i as usize] = true;
    }
    let first_extra = n - skin.extra.len().min(n);
    let worn = |i: usize| i >= first_extra && skin.extra_parts[i - first_extra] == 0;
    let mouth = |i: usize| i >= first_extra && skin.extra_parts[i - first_extra] != 0;

    // The clothes' layer: how far each of its vertices is from the layer's edge.
    let mut edge_count: HashMap<(u32, u32), u32> = HashMap::new();
    for t in skin.indices.as_chunks::<3>().0 {
        if t.iter().all(|i| worn(*i as usize)) {
            for k in 0..3 {
                let (a, b) = (t[k], t[(k + 1) % 3]);
                *edge_count.entry((a.min(b), a.max(b))).or_default() += 1;
            }
        }
    }
    let mut edges: HashMap<[i32; 3], Vec<Vec3>> = HashMap::new();
    for ((a, b), count) in &edge_count {
        if *count == 1 {
            for i in [*a, *b] {
                let p = skin.positions[i as usize];
                edges.entry(key(p, LIFT_EDGE)).or_default().push(p);
            }
        }
    }
    let from_edge = |p: Vec3| {
        let k = key(p, LIFT_EDGE);
        let mut best = f32::MAX;
        for dx in -1..=1 {
            for dy in -1..=1 {
                for dz in -1..=1 {
                    if let Some(list) = edges.get(&[k[0] + dx, k[1] + dy, k[2] + dz]) {
                        for e in list {
                            best = best.min((*e - p).length_squared());
                        }
                    }
                }
            }
        }
        best.sqrt()
    };
    let cut = Cut::of(a);
    let lift_top = match cut.top {
        Top::None => 0.0,
        Top::Shirt | Top::Tshirt => LIFT_SHIRT,
        Top::Gambeson => LIFT_PADDED,
        Top::Mail => LIFT_MAIL,
    };

    let place = |i: usize, p: Vec3, vector: bool| {
        let (b, w) = (skin.bones[i], skin.weights[i]);
        let mut sum = Vec3::ZERO;
        let mut total = 0.0;
        for k in 0..4 {
            if w[k] == 0 {
                continue;
            }
            let m = &palette[b[k] as usize];
            let q = if vector {
                m.transform_vector3(p)
            } else {
                m.transform_point3(p)
            };
            sum += q * w[k] as f32;
            total += w[k] as f32;
        }
        if total > 0.0 { sum / total } else { p }
    };

    // The layer's vertices first, so the skin under them can be left out.
    let mut s = Surface {
        at: Vec::new(),
        out: Vec::new(),
        bones: Vec::new(),
        weights: Vec::new(),
        faces: Vec::new(),
    };
    let mut kept = vec![u32::MAX; n];
    let mut push = |s: &mut Surface, i: usize, lift: f32| {
        kept[i] = s.at.len() as u32;
        let nrm = skin.normals[i];
        s.at.push(place(i, skin.positions[i] + nrm * lift, false));
        s.out.push(place(i, nrm, true).normalize_or(Vec3::Y));
        let (b, w) = (skin.bones[i], skin.weights[i]);
        let mut bones = [0u16; 4];
        let mut weights = [0u8; 4];
        for k in 0..4 {
            if let Some(p) = in_pose.get(b[k] as usize).copied().flatten() {
                bones[k] = p;
                weights[k] = w[k];
            }
        }
        s.bones.push(bones);
        s.weights.push(weights);
    };
    let mut over: HashMap<[i32; 3], Vec<Vec3>> = HashMap::new();
    const OVER: f32 = 0.012;
    for i in (first_extra..n).filter(|i| drawn[*i] && worn(*i)) {
        let p = skin.positions[i];
        let thick = if skin.rest[i].y >= WAIST {
            lift_top
        } else {
            LIFT_TROUSERS
        };
        let lift = thick * smoothstep(0.0, LIFT_EDGE, from_edge(p));
        over.entry(key(p, OVER)).or_default().push(p);
        push(&mut s, i, lift);
    }
    for i in (0..n).filter(|i| drawn[*i] && !worn(*i) && !mouth(*i)) {
        let p = skin.positions[i];
        let nrm = skin.normals[i];
        let k = key(p, OVER);
        let mut covered = false;
        'cells: for dx in -1..=1 {
            for dy in -1..=1 {
                for dz in -1..=1 {
                    let Some(list) = over.get(&[k[0] + dx, k[1] + dy, k[2] + dz]) else {
                        continue;
                    };
                    for c in list {
                        let d = *c - p;
                        let up = d.dot(nrm);
                        if up > 0.0 && up < OVER && d.length_squared() - up * up < 0.004 * 0.004 {
                            covered = true;
                            break 'cells;
                        }
                    }
                }
            }
        }
        if !covered {
            push(&mut s, i, 0.0);
        }
    }
    for t in skin.indices.as_chunks::<3>().0 {
        let t = t.map(|i| i as usize);
        if t.iter().any(|i| mouth(*i)) || t.iter().all(|i| kept[*i] == u32::MAX) {
            continue;
        }
        s.faces.push(t.map(|i| match kept[i] {
            u32::MAX => place(i, skin.positions[i], false),
            k => s.at[k as usize],
        }));
    }
    (!s.at.is_empty()).then_some(s)
}

/// A point of the drawn surface the hair body is measured from: its place and outward normal
/// in the rest pose's model space, and the pose's bones that carry it (weights of 255).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct DrawnPoint {
    pub at: Vec3,
    pub out: Vec3,
    pub bones: [u16; 4],
    pub weights: [u8; 4],
}

/// The drawn surface the hair body is measured from (for checks and tests).
pub fn drawn_surface(
    meshes: &CharacterMeshes,
    a: &Appearance,
    pose: &SkeletonPose,
) -> Vec<DrawnPoint> {
    surface(meshes, a, pose).map_or(Vec::new(), |s| {
        (0..s.at.len())
            .map(|i| DrawnPoint {
                at: s.at[i],
                out: s.out[i],
                bones: s.bones[i],
                weights: s.weights[i],
            })
            .collect()
    })
}

/// The lattice: per sample the nearest surface point.
struct Lattice {
    min: Vec3,
    size: [usize; 3],
    near: Vec<u32>,
}

impl Lattice {
    fn at(&self, x: usize, y: usize, z: usize) -> Vec3 {
        self.min + Vec3::new(x as f32, y as f32, z as f32) * CELL
    }

    fn index(&self, x: usize, y: usize, z: usize) -> usize {
        x + self.size[0] * (y + self.size[1] * z)
    }

    /// The nearest of `points` to every sample: each point claims the samples around it, then
    /// claims spread sample to sample, forth and back.
    fn new(min: Vec3, size: [usize; 3], points: &[Vec3]) -> Self {
        let mut l = Self {
            min,
            size,
            near: vec![u32::MAX; size[0] * size[1] * size[2]],
        };
        let mut best = vec![f32::MAX; l.near.len()];
        for (i, p) in points.iter().enumerate() {
            let c = (*p - min) / CELL;
            let (cx, cy, cz) = (c.x.floor() as i32, c.y.floor() as i32, c.z.floor() as i32);
            for z in cz..=cz + 1 {
                for y in cy..=cy + 1 {
                    for x in cx..=cx + 1 {
                        if x < 0
                            || y < 0
                            || z < 0
                            || x >= size[0] as i32
                            || y >= size[1] as i32
                            || z >= size[2] as i32
                        {
                            continue;
                        }
                        let (x, y, z) = (x as usize, y as usize, z as usize);
                        let d = (l.at(x, y, z) - *p).length_squared();
                        let k = l.index(x, y, z);
                        if d < best[k] {
                            best[k] = d;
                            l.near[k] = i as u32;
                        }
                    }
                }
            }
        }
        // The 13 neighbors a forward scan has already passed (the other 13 mirrored).
        let mut before = Vec::with_capacity(13);
        for dz in -1i32..=1 {
            for dy in -1i32..=1 {
                for dx in -1i32..=1 {
                    if (dz, dy, dx) < (0, 0, 0) {
                        before.push([dx, dy, dz]);
                    }
                }
            }
        }
        let [sx, sy, sz] = size.map(|s| s as i32);
        for _ in 0..2 {
            for forward in [true, false] {
                let sign = if forward { 1 } else { -1 };
                for step in 0..l.near.len() {
                    let k = if forward {
                        step
                    } else {
                        l.near.len() - 1 - step
                    };
                    let x = (k % size[0]) as i32;
                    let y = ((k / size[0]) % size[1]) as i32;
                    let z = (k / (size[0] * size[1])) as i32;
                    let p = l.at(x as usize, y as usize, z as usize);
                    for o in &before {
                        let (nx, ny, nz) = (x + o[0] * sign, y + o[1] * sign, z + o[2] * sign);
                        if nx < 0 || ny < 0 || nz < 0 || nx >= sx || ny >= sy || nz >= sz {
                            continue;
                        }
                        let near = l.near[l.index(nx as usize, ny as usize, nz as usize)];
                        if near == u32::MAX || near == l.near[k] {
                            continue;
                        }
                        let d = (p - points[near as usize]).length_squared();
                        if d < best[k] {
                            best[k] = d;
                            l.near[k] = near;
                        }
                    }
                }
            }
        }
        l
    }
}

/// Where a sample of the whole lattice is: at the skin (within a cell of a surface point),
/// or clear of it outside or inside the body.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Side {
    Skin,
    Out,
    In,
}

/// Which side of the body every sample of `whole` is on: the samples within a cell of a
/// surface point are the skin; of the others, those from which most of the six rays along the
/// axes cross the surface's `faces` an odd number of times are inside. (The tangent plane of
/// the nearest vertex, which decided this before, takes a sample behind a sweater for outside
/// where bare skin above the neckline is the nearer and faces it: locks lay in such pockets,
/// 15 mm under the cloth. And the surface is no closed wall to flood the outside against: the
/// lattice cuts it at the chest, and the clothes' edges leave gaps.) A ray out through the cut
/// chest, or through a gap, is one wrong voice of six; three against three, the nearest
/// vertex's plane decides.
fn inside(
    whole: &Lattice,
    points: &[Vec3],
    out: impl Fn(usize) -> Vec3,
    faces: &[[Vec3; 3]],
) -> Vec<Side> {
    let size = whole.size;
    let mut odd = vec![0u8; whole.near.len()];
    for axis in 0..3 {
        let (u, v) = ((axis + 1) % 3, (axis + 2) % 3);
        let mut crossed: Vec<Vec<f32>> = vec![Vec::new(); size[u] * size[v]];
        for f in faces {
            let f = f.map(|p| (p - whole.min) / CELL);
            let (lo, hi) = (f[0].min(f[1]).min(f[2]), f[0].max(f[1]).max(f[2]));
            if hi[u] < 0.0 || hi[v] < 0.0 || lo[u] > size[u] as f32 || lo[v] > size[v] as f32 {
                continue;
            }
            let edge = |a: Vec3, b: Vec3, x: f32, y: f32| {
                (b[u] - a[u]) * (y - a[v]) - (b[v] - a[v]) * (x - a[u])
            };
            let area = edge(f[0], f[1], f[2][u], f[2][v]);
            if area.abs() < 1e-9 {
                continue;
            }
            for iv in lo[v].ceil().max(0.0) as usize..=(hi[v].floor() as usize).min(size[v] - 1) {
                for iu in lo[u].ceil().max(0.0) as usize..=(hi[u].floor() as usize).min(size[u] - 1)
                {
                    let (x, y) = (iu as f32, iv as f32);
                    let w = [
                        edge(f[1], f[2], x, y) / area,
                        edge(f[2], f[0], x, y) / area,
                        edge(f[0], f[1], x, y) / area,
                    ];
                    if w.iter().all(|w| *w >= 0.0) {
                        crossed[iu + size[u] * iv]
                            .push(w[0] * f[0][axis] + w[1] * f[1][axis] + w[2] * f[2][axis]);
                    }
                }
            }
        }
        for iv in 0..size[v] {
            for iu in 0..size[u] {
                let list = &mut crossed[iu + size[u] * iv];
                if list.is_empty() {
                    continue;
                }
                list.sort_by(f32::total_cmp);
                let mut below = 0;
                for i in 0..size[axis] {
                    while below < list.len() && list[below] < i as f32 {
                        below += 1;
                    }
                    let mut at = [0; 3];
                    (at[axis], at[u], at[v]) = (i, iu, iv);
                    odd[whole.index(at[0], at[1], at[2])] +=
                        (below % 2 + (list.len() - below) % 2) as u8;
                }
            }
        }
    }
    let [sx, sy, sz] = size;
    let mut side = vec![Side::Out; whole.near.len()];
    for z in 0..sz {
        for y in 0..sy {
            for x in 0..sx {
                let k = whole.index(x, y, z);
                let near = whole.near[k];
                if near == u32::MAX {
                    continue;
                }
                let off = whole.at(x, y, z) - points[near as usize];
                side[k] = if off.length_squared() <= CELL * CELL {
                    Side::Skin
                } else if odd[k] > 3 || (odd[k] == 3 && off.dot(out(near as usize)) < 0.0) {
                    Side::In
                } else {
                    Side::Out
                };
            }
        }
    }
    side
}

/// How much of each vertex in `within` each part's bone carries (of 255). What a bone without
/// a part carries (a hand, where only the forearm has a box) counts for the nearest bone above
/// it that has one.
fn shares(s: &Surface, within: &[usize], pose: &SkeletonPose, parts: &[u16]) -> Vec<Vec<u8>> {
    within
        .iter()
        .map(|&v| {
            let mut share = vec![0u32; parts.len()];
            for k in 0..4 {
                let w = s.weights[v][k] as u32;
                let mut bone = (w > 0).then_some(s.bones[v][k] as usize);
                while let Some(b) = bone {
                    if let Some(part) = parts.iter().position(|p| *p as usize == b) {
                        share[part] += w;
                        break;
                    }
                    bone = pose.parents.get(b).copied().flatten().map(usize::from);
                }
            }
            share.into_iter().map(|w| w.min(255) as u8).collect()
        })
        .collect()
}

/// The [`HairBody`] of a built person for its `pose` skeleton and `guides` (`None` without a
/// rig or a surface near the hair).
pub fn hair_body(
    meshes: &CharacterMeshes,
    a: &Appearance,
    pose: &SkeletonPose,
    guides: &HairGuides,
) -> Option<HairBody> {
    let s = surface(meshes, a, pose)?;
    // What the hair can reach: around its roots as far as its longest guide.
    let mut lo = Vec3::splat(f32::MAX);
    let mut hi = Vec3::splat(f32::MIN);
    let mut reach = 0.0f32;
    for g in &guides.guides {
        let Some(root) = g.points.first() else {
            continue;
        };
        lo = lo.min(*root);
        hi = hi.max(*root);
        reach = reach.max(g.points.windows(2).map(|w| (w[1] - w[0]).length()).sum());
    }
    if lo.x > hi.x {
        return None;
    }
    let (lo, hi) = (
        lo - Vec3::splat(reach + BAND),
        hi + Vec3::splat(reach + BAND),
    );
    let within: Vec<usize> = (0..s.at.len())
        .filter(|i| s.at[*i].cmpge(lo).all() && s.at[*i].cmple(hi).all())
        .collect();
    if within.is_empty() {
        return None;
    }
    let points: Vec<Vec3> = within.iter().map(|i| s.at[*i]).collect();
    let (mut min, mut max) = (Vec3::splat(f32::MAX), Vec3::splat(f32::MIN));
    for p in &points {
        min = min.min(*p);
        max = max.max(*p);
    }
    let min = min - Vec3::splat(BAND);
    let size = ((max + Vec3::splat(BAND) - min) / CELL).ceil();
    let size = [
        size.x as usize + 1,
        size.y as usize + 1,
        size.z as usize + 1,
    ];
    // The whole surface's lattice: the nearest vertex to every sample, and which side of the
    // body it is on.
    let whole = Lattice::new(min, size, &points);
    let inner = inside(&whole, &points, |near| s.out[within[near]], &s.faces);

    // The bones that carry the surface, most first.
    let mut carried: HashMap<u16, usize> = HashMap::new();
    for &v in &within {
        for k in 0..4 {
            if s.weights[v][k] >= CARRIES {
                *carried.entry(s.bones[v][k]).or_default() += 1;
            }
        }
    }
    let mut bones: Vec<(u16, usize)> = carried.into_iter().filter(|(_, n)| *n >= FEW).collect();
    bones.sort_by_key(|(b, n)| (std::cmp::Reverse(*n), *b));
    bones.truncate(MOST_PARTS);
    let bones: Vec<u16> = bones.into_iter().map(|(b, _)| b).collect();
    let shares = shares(&s, &within, pose, &bones);
    let mut parts = Vec::with_capacity(bones.len());
    for (k, bone) in bones.iter().enumerate() {
        // Its box: around the skin it carries a fair share of, on the whole lattice's samples.
        let (mut lo, mut hi) = (Vec3::splat(f32::MAX), Vec3::splat(f32::MIN));
        for (i, share) in shares.iter().enumerate() {
            if share[k] >= CARRIES {
                lo = lo.min(points[i]);
                hi = hi.max(points[i]);
            }
        }
        if lo.x > hi.x {
            continue;
        }
        let first = ((lo - Vec3::splat(BAND) - min) / CELL)
            .floor()
            .max(Vec3::ZERO);
        let last = ((hi + Vec3::splat(BAND) - min) / CELL).ceil();
        let first = [first.x as usize, first.y as usize, first.z as usize];
        let last = [
            (last.x as usize).min(size[0] - 1),
            (last.y as usize).min(size[1] - 1),
            (last.z as usize).min(size[2] - 1),
        ];
        let dims = [
            last[0] - first[0] + 1,
            last[1] - first[1] + 1,
            last[2] - first[2] + 1,
        ];
        let corner = whole.at(first[0], first[1], first[2]);
        let mut d = Vec::with_capacity(dims[0] * dims[1] * dims[2]);
        let mut w = Vec::with_capacity(d.capacity());
        for z in 0..dims[2] {
            for y in 0..dims[1] {
                for x in 0..dims[0] {
                    let at = whole.index(first[0] + x, first[1] + y, first[2] + z);
                    let near = whole.near[at];
                    if near == u32::MAX {
                        d.push(i16::MAX);
                        w.push(0);
                        continue;
                    }
                    let v = within[near as usize];
                    let p = whole.at(first[0] + x, first[1] + y, first[2] + z);
                    let off = p - s.at[v];
                    let len = off.length();
                    let plane = off.dot(s.out[v]);
                    let m = if inner[at] == Side::Skin && len <= 2.0 * ON_PLANE {
                        // At the skin: off the tangent plane (the surface between vertices),
                        // by degrees straight to the vertex.
                        let straight = len.copysign(plane);
                        plane + (straight - plane) * smoothstep(ON_PLANE, 2.0 * ON_PLANE, len)
                    } else {
                        // Off it: inside where the sample is inside the body.
                        let inside = match inner[at] {
                            Side::In => true,
                            Side::Out => false,
                            Side::Skin => plane < 0.0,
                        };
                        if inside { -len } else { len }
                    };
                    d.push((m / HairBody::UNIT).clamp(i16::MIN as f32, i16::MAX as f32) as i16);
                    w.push(shares[near as usize][k]);
                }
            }
        }
        parts.push(HairBodyPart {
            bone: *bone,
            min: corner,
            cell: CELL,
            size: dims.map(|n| n as u16),
            distance: Arc::from(d),
            weight: Arc::from(w),
        });
    }
    (!parts.is_empty()).then_some(HairBody {
        version: guides.version,
        parts,
    })
}

/// A part reads the body at a point only where its bone carries at least this much of the
/// share the part that carries most there does (each of the skin nearest the point as the
/// part's own bone would have brought it there). The hair simulation and the strand renderer
/// read a body the same way (softbody `tress.rs`, render `strands_pose.wgsl`).
pub const READS_FROM: f32 = 0.25;
/// How many times a reader blends the bones for where a point was.
pub const REFINED: usize = 2;

/// The distance from `p` (model space where the hair was grown, nobody moved) to the body, m,
/// and the way out of it; `None` outside every box.
pub fn at_rest(body: &HairBody, p: Vec3) -> Option<(f32, Vec3)> {
    at(body, |_| Mat4::IDENTITY, p)
}

/// The distance from `p` to the body with its bones moved, m, and the way out of it: `moved`
/// gives how each part's bone has carried what it carries since the hair was grown (model
/// space to model space). `None` where no part's bone carries the skin near `p`.
///
/// Every box holds the whole body as it was grown, so the question is where `p` was then.
/// Each part answers as its own bone would have brought it (`moved` undone), and counts as
/// much as its bone carries of the skin nearest there. Where bones share that skin (a
/// shoulder between arm and trunk), the skin has moved as the blend of them by their shares,
/// as it is skinned: that blend undone gives where `p` was, and the body is read there. Of
/// the parts that count, the one that finds `p` deepest is right (skin on skin, as a chin on
/// a chest, is two places).
pub fn at(body: &HairBody, moved: impl Fn(u16) -> Mat4, p: Vec3) -> Option<(f32, Vec3)> {
    let moved: Vec<Mat4> = body.parts.iter().map(|part| moved(part.bone)).collect();
    let seen: Vec<(usize, Vec3, f32)> = body
        .parts
        .iter()
        .enumerate()
        .filter_map(|(k, part)| {
            let was = moved[k].inverse().transform_point3(p);
            sample(part, was).map(|(.., w)| (k, was, w))
        })
        .collect();
    let most = seen.iter().map(|(.., w)| *w).fold(0.0, f32::max);
    if most <= 0.0 {
        return None;
    }
    let mut found: Option<(f32, Vec3)> = None;
    for &(k, was, w) in &seen {
        if w < READS_FROM * most {
            continue;
        }
        // The bones that carry the skin there, by their shares (and once more where that
        // puts the point: the shares are those of where it really was).
        let (mut blend, mut at) = (moved[k], was);
        for _ in 0..REFINED {
            let (mut sum, mut total) = (Mat4::ZERO, 0.0);
            for (j, part) in body.parts.iter().enumerate() {
                if let Some((.., share)) = sample(part, at)
                    && share > 0.0
                {
                    sum += moved[j] * share;
                    total += share;
                }
            }
            if total <= 0.0 {
                break;
            }
            blend = sum * (1.0 / total);
            at = blend.inverse().transform_point3(p);
        }
        let Some((d, g, _)) = sample(&body.parts[k], at)
            .or_else(|| body.parts.iter().find_map(|part| sample(part, at)))
        else {
            continue;
        };
        if found.is_none_or(|(least, _)| d < least) {
            found = Some((d, blend.transform_vector3(g).normalize_or(Vec3::Y)));
        }
    }
    found
}

/// One box at `p` where the hair was grown: the distance to the body (m), its gradient, and
/// how much of the skin nearest there the box's bone carries (0..1).
pub fn sample(part: &HairBodyPart, p: Vec3) -> Option<(f32, Vec3, f32)> {
    let c = (p - part.min) / part.cell;
    let [sx, sy, sz] = part.size.map(|s| s as usize);
    if c.cmplt(Vec3::ZERO).any()
        || c.x >= (sx - 1) as f32
        || c.y >= (sy - 1) as f32
        || c.z >= (sz - 1) as f32
    {
        return None;
    }
    let (x, y, z) = (c.x as usize, c.y as usize, c.z as usize);
    let f = c - Vec3::new(x as f32, y as f32, z as f32);
    let k = x + sx * (y + sy * z);
    let corner = |dx: usize, dy: usize, dz: usize| {
        let i = k + dx + sx * (dy + sy * dz);
        (
            part.distance[i] as f32 * HairBody::UNIT,
            part.weight[i] as f32 / 255.0,
        )
    };
    let c000 = corner(0, 0, 0);
    let c100 = corner(1, 0, 0);
    let c010 = corner(0, 1, 0);
    let c110 = corner(1, 1, 0);
    let c001 = corner(0, 0, 1);
    let c101 = corner(1, 0, 1);
    let c011 = corner(0, 1, 1);
    let c111 = corner(1, 1, 1);
    let mix = |a: f32, b: f32, t: f32| a + (b - a) * t;
    let x00 = mix(c000.0, c100.0, f.x);
    let x10 = mix(c010.0, c110.0, f.x);
    let x01 = mix(c001.0, c101.0, f.x);
    let x11 = mix(c011.0, c111.0, f.x);
    let y0 = mix(x00, x10, f.y);
    let y1 = mix(x01, x11, f.y);
    let d = mix(y0, y1, f.z);
    let gx = mix(
        mix(c100.0 - c000.0, c110.0 - c010.0, f.y),
        mix(c101.0 - c001.0, c111.0 - c011.0, f.y),
        f.z,
    );
    let gy = mix(x10 - x00, x11 - x01, f.z);
    let gz = y1 - y0;
    let w = mix(
        mix(mix(c000.1, c100.1, f.x), mix(c010.1, c110.1, f.x), f.y),
        mix(mix(c001.1, c101.1, f.x), mix(c011.1, c111.1, f.x), f.y),
        f.z,
    );
    Some((d, Vec3::new(gx, gy, gz) / part.cell, w))
}
