//! A hood (`Appearance::headgear = "hood"`): loose cloth over the head and neck, open around
//! the face and draped onto the shoulders as a cowl, in the outfit's top colour.
//!
//! The hood is the base mesh's head, neck and collar pushed out from the skin. Each height is
//! widened to the convex hull of the head there, so the cloth bridges the ears and the hollows
//! of the skull, and the back falls from the crown to the shoulders the way cloth hangs
//! instead of following the nape in. It is thin at the face opening, fuller over the head,
//! smoothed, and never closer to the skin than a few millimetres. The edges of the opening and
//! the cowl are slid along the skin onto smooth curves, so they don't step along the mesh's
//! quads. Its vertices follow the skin vertices they came from, so it turns and nods with the
//! head. The scalp hair is left out under it (`Headgear::cover_hair`).

use std::collections::HashMap;

use engine_core::math::{Vec2, Vec3};

use crate::mesh::vertex_normals;
use crate::mouth::{MouthMesh, MouthPart, smooth_normals};
use crate::pack::HumanPack;
use crate::shoes::{hull, reach};

/// Rest height of the cowl's lower edge, over the shirt's neckline, and how far out from the
/// neck it lies on the shoulders.
const LOWER_EDGE: f32 = 0.49;
const COWL: f32 = 0.11;
/// The face opening, seen from an axis just behind the rest head center (between the ears,
/// level with the eyes): an ellipse this many radians to either side, this tall (half
/// extent, m), centered this far below the eyes, with the axis this far behind the center.
const OPENING: [f32; 4] = [0.96, 0.105, 0.035, 0.01];
/// How far the cloth stands off the skin, m: at the opening, over the head and on the cowl,
/// and the extra room at the back of the head.
const THIN: f32 = 0.004;
const FULL: f32 = 0.02;
const BACK: f32 = 0.03;
/// How fast the hood narrows going down, m per m: barely behind (it hangs), more at the sides.
const DROP_BACK: f32 = 0.1;
const DROP_SIDE: f32 = 0.25;
/// The cowl's edge lies this close over the shirt, m.
const HEM: f32 = 0.008;
/// The ears, relative to the rest head center: out past this far to the side, between these
/// heights and depths. The cloth spans over them instead of taking their shape.
const EAR_OUT: f32 = 0.058;
const EAR_Y: [f32; 2] = [-0.065, 0.025];
const EAR_Z: [f32; 2] = [-0.02, 0.055];
/// The most the hull widens the hood anywhere, m.
const MAX_GROW: f32 = 0.07;
/// Height of a slice of the head, m.
const SLICE: f32 = 0.01;
/// Directions around a slice.
const AROUND: usize = 48;

/// Where a rest position is relative to the face opening: below 1 inside it (bare), above 1
/// outside.
fn opening(r: Vec3, head: Vec3) -> f32 {
    let d = r - head;
    // Around the axis rather than across, so the narrower neck and the ears, off to the side,
    // are outside it, and so is everything behind.
    let side = d.x.atan2(d.z + OPENING[3]).abs() / OPENING[0];
    let up = (d.y + OPENING[2]) / OPENING[1];
    (side * side + up * up).sqrt()
}

/// The point on the opening's edge nearest a rest position, as seen from the axis.
fn onto_opening(r: Vec3, head: Vec3) -> Vec3 {
    let d = r - head;
    let around = Vec2::new(d.x, d.z + OPENING[3]);
    let side = d.x.atan2(d.z + OPENING[3]) / OPENING[0];
    let up = (d.y + OPENING[2]) / OPENING[1];
    let q = (side * side + up * up).sqrt().max(1e-3);
    let a = side / q * OPENING[0];
    let len = around.length();
    head + Vec3::new(
        len * a.sin(),
        up / q * OPENING[1] - OPENING[2],
        len * a.cos() - OPENING[3],
    )
}

/// How far inside the hood a rest position is, roughly in m (negative outside).
fn inside(r: Vec3, head: Vec3) -> f32 {
    let face = (opening(r, head) - 1.0) * 0.08;
    let around = Vec2::new(r.x, r.z - head.z).length();
    let cowl = (r.y - 0.6).max((r.y - LOWER_EDGE).min(COWL - around));
    face.min(cowl)
}

/// Whether the hood covers a rest position.
pub fn covers(r: Vec3, head: Vec3) -> bool {
    inside(r, head) > 0.0
}

fn smooth(e0: f32, e1: f32, x: f32) -> f32 {
    let t = ((x - e0) / (e1 - e0)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// The hood of a morphed body (`morphed`: the pack's vertices after the morphs; `head`: the
/// rest head center).
pub fn build(pack: &HumanPack, morphed: &[Vec3], head: Vec3) -> MouthMesh {
    let mut m = MouthMesh::default();
    let Some(body) = pack.group("body") else {
        return m;
    };
    let quads: Vec<[u32; 4]> = pack
        .quads
        .iter()
        .filter(|q| q.group == body)
        .filter(|q| {
            q.v.iter()
                .all(|v| covers(pack.positions[*v as usize], head))
        })
        .map(|q| q.v)
        .collect();
    if quads.len() < 16 {
        return m;
    }
    // Local vertices and triangles; the edges of the hood are the quad edges used once.
    let mut local: HashMap<u32, u32> = HashMap::new();
    let mut verts: Vec<u32> = Vec::new();
    let mut tris: Vec<u32> = Vec::with_capacity(quads.len() * 6);
    let mut uses: HashMap<(u32, u32), u32> = HashMap::new();
    for q in &quads {
        let l = q.map(|v| {
            *local.entry(v).or_insert_with(|| {
                verts.push(v);
                verts.len() as u32 - 1
            })
        });
        tris.extend([l[0], l[1], l[2], l[0], l[2], l[3]]);
        for k in 0..4 {
            let (a, b) = (l[k], l[(k + 1) % 4]);
            *uses.entry((a.min(b), a.max(b))).or_default() += 1;
        }
    }
    let mut edge = vec![false; verts.len()];
    for ((a, b), n) in &uses {
        if *n == 1 {
            edge[*a as usize] = true;
            edge[*b as usize] = true;
        }
    }
    let rest: Vec<Vec3> = verts.iter().map(|v| pack.positions[*v as usize]).collect();
    let mut skin: Vec<Vec3> = verts.iter().map(|v| morphed[*v as usize]).collect();
    let rest_normals = vertex_normals(&rest, &tris);
    let mut near: Vec<Vec<u32>> = vec![Vec::new(); verts.len()];
    for t in tris.as_chunks::<3>().0 {
        for k in 0..3 {
            let (a, b) = (t[k], t[(k + 1) % 3]);
            if !near[a as usize].contains(&b) {
                near[a as usize].push(b);
                near[b as usize].push(a);
            }
        }
    }
    // Slide the edges along the skin onto the opening and the cowl's curves.
    for i in 0..verts.len() {
        if !edge[i] {
            continue;
        }
        let mut r = rest[i];
        let n = rest_normals[i];
        if (opening(r, head) - 1.0) * 0.08 < inside(r, head) + 1e-4 {
            // The opening's edge: straight onto the ellipse around the axis.
            skin[i] += (onto_opening(r, head) - r).clamp_length_max(0.02);
            continue;
        }
        for _ in 0..4 {
            let f = inside(r, head);
            let e = 5e-4;
            let g = Vec3::new(
                inside(r + Vec3::X * e, head) - f,
                inside(r + Vec3::Y * e, head) - f,
                inside(r + Vec3::Z * e, head) - f,
            ) / e;
            let g = g - n * g.dot(n);
            if g.length_squared() < 0.1 {
                break;
            }
            r -= g * (f / g.length_squared());
        }
        // A step of the mesh is never more than a few millimetres off the curve.
        skin[i] += (r - rest[i]).clamp_length_max(0.02);
    }
    // And even out what's left of the steps along each edge.
    let mut along: Vec<Vec<u32>> = vec![Vec::new(); verts.len()];
    for ((a, b), n) in &uses {
        if *n == 1 {
            along[*a as usize].push(*b);
            along[*b as usize].push(*a);
        }
    }
    for _ in 0..10 {
        let s0 = skin.clone();
        for i in 0..verts.len() {
            if along[i].len() == 2 {
                let avg = (s0[along[i][0] as usize] + s0[along[i][1] as usize]) * 0.5;
                skin[i] = s0[i] * 0.5 + avg * 0.5;
            }
        }
    }
    // The two rows inside the edges follow, so no quad folds over the moved edge.
    let mut ring: Vec<u8> = edge.iter().map(|e| if *e { 0 } else { u8::MAX }).collect();
    for k in 1..=2u8 {
        for i in 0..verts.len() {
            if ring[i] == u8::MAX && near[i].iter().any(|o| ring[*o as usize] == k - 1) {
                ring[i] = k;
            }
        }
    }
    for _ in 0..10 {
        let s0 = skin.clone();
        for i in 0..verts.len() {
            if (1..=2).contains(&ring[i]) {
                let avg =
                    near[i].iter().map(|o| s0[*o as usize]).sum::<Vec3>() / near[i].len() as f32;
                skin[i] = s0[i] * 0.5 + avg * 0.5;
            }
        }
    }
    let normals = vertex_normals(&skin, &tris);
    // How far each vertex stands off: thin at the opening, full over the head and the cowl,
    // roomier behind.
    let face: Vec<f32> = rest
        .iter()
        .map(|r| smooth(1.0, 1.25, opening(*r, head)))
        .collect();
    let ear: Vec<bool> = rest
        .iter()
        .map(|r| {
            let d = *r - head;
            d.x.abs() > EAR_OUT
                && (EAR_Y[0]..EAR_Y[1]).contains(&d.y)
                && (EAR_Z[0]..EAR_Z[1]).contains(&d.z)
        })
        .collect();
    let off: Vec<f32> = rest
        .iter()
        .zip(&face)
        .map(|(r, face)| {
            let d = *r - head;
            let back = smooth(0.0, -0.08, d.z) * smooth(-0.1, 0.05, d.y);
            // Down to the hem, the cowl settles onto the shirt.
            let hem = HEM + (FULL - HEM) * smooth(0.0, 0.03, r.y - LOWER_EDGE);
            THIN + (hem - THIN) * face + BACK * back * face
        })
        .collect();
    // The cloth off the skin (the ears' folds point every which way, so they count as skin),
    // then hung from its widest.
    let off_skin: Vec<Vec3> = (0..verts.len())
        .map(|i| {
            if ear[i] {
                skin[i]
            } else {
                skin[i] + normals[i] * off[i]
            }
        })
        .collect();
    let radial = drape(&off_skin, &face);
    let lifted: Vec<Vec3> = (0..verts.len())
        .map(|i| {
            let p = skin[i] + normals[i] * off[i];
            let (c, want) = radial(p);
            let d = Vec2::new(p.x - c.x, p.z - c.y);
            let have = d.length();
            if have < 0.02 {
                return p;
            }
            // Out to the draped hull (the opening too, so the hood frames the face instead of
            // hugging it), and only behind on the neck: in front the cowl lies on the chest.
            let r = rest[i];
            let behind = smooth(0.0, -0.05, r.z - head.z);
            let neck = smooth(0.55, 0.62, r.y).max(behind * smooth(LOWER_EDGE, 0.55, r.y));
            let grow = ((want - have) * (0.6 + 0.4 * face[i]) * neck).clamp(0.0, MAX_GROW);
            let d = d / have * grow;
            p + Vec3::new(d.x, 0.0, d.y)
        })
        .collect();
    // Span the ears: their vertices settle between the cloth around them.
    let mut p = lifted;
    for _ in 0..200 {
        let p0 = p.clone();
        for i in 0..p.len() {
            if ear[i] && !edge[i] && !near[i].is_empty() {
                p[i] = near[i].iter().map(|o| p0[*o as usize]).sum::<Vec3>() / near[i].len() as f32;
            }
        }
    }
    // Smooth out what's left of the bumps (Taubin: no shrinking), keeping the edges where they
    // are and the cloth outside the skin.
    for _ in 0..30 {
        for factor in [0.5f32, -0.53] {
            let p0 = p.clone();
            for i in 0..p.len() {
                if edge[i] || near[i].is_empty() {
                    continue;
                }
                let avg =
                    near[i].iter().map(|o| p0[*o as usize]).sum::<Vec3>() / near[i].len() as f32;
                p[i] = p0[i] + (avg - p0[i]) * factor;
            }
        }
        for i in 0..p.len() {
            if ear[i] {
                // Out past the ear and the jaw: the hull holds them both.
                let (c, want) = radial(p[i]);
                let d = Vec2::new(p[i].x - c.x, p[i].z - c.y);
                let have = d.length();
                if have > 0.02 && have < want + THIN {
                    let d = d / have * (want + THIN - have);
                    p[i] += Vec3::new(d.x, 0.0, d.y);
                }
                continue;
            }
            let out = (p[i] - skin[i]).dot(normals[i]);
            if out < off[i] * 0.6 {
                p[i] += normals[i] * (off[i] * 0.6 - out);
            }
        }
    }
    let first = m.positions.len() as u32;
    for (i, v) in verts.iter().enumerate() {
        m.positions.push(p[i]);
        m.normals.push(normals[i]);
        m.parts.push(MouthPart::Hood);
        m.anchors.push([(*v, 1.0), (*v, 0.0), (*v, 0.0)]);
    }
    m.indices.extend(tris.iter().map(|t| t + first));
    smooth_normals(&mut m, first);
    line_edges(&mut m, &uses, &tris, first, &verts, &skin, &normals);
    m
}

/// The cloth's thickness at its edges: a band from each edge in to the skin, so looking into
/// the hood or under the cowl shows the inside of the cloth, not a gap. The band carries on the
/// hood's own winding (each edge the other way round), so its outside folds round the edge
/// toward whoever looks in.
fn line_edges(
    m: &mut MouthMesh,
    uses: &HashMap<(u32, u32), u32>,
    tris: &[u32],
    first: u32,
    verts: &[u32],
    skin: &[Vec3],
    normals: &[Vec3],
) {
    let mut outer: HashMap<u32, u32> = HashMap::new();
    let mut inner: HashMap<u32, u32> = HashMap::new();
    let copy = |m: &mut MouthMesh, map: &mut HashMap<u32, u32>, i: u32, p: Vec3| {
        *map.entry(i).or_insert_with(|| {
            m.positions.push(p);
            m.normals.push(Vec3::ZERO);
            m.parts.push(MouthPart::Hood);
            let v = verts[i as usize];
            m.anchors.push([(v, 1.0), (v, 0.0), (v, 0.0)]);
            m.positions.len() as u32 - 1
        })
    };
    let start = m.indices.len();
    for t in tris.as_chunks::<3>().0 {
        for k in 0..3 {
            let (a, b) = (t[k], t[(k + 1) % 3]);
            if uses.get(&(a.min(b), a.max(b))) != Some(&1) {
                continue;
            }
            let oa = copy(m, &mut outer, a, m.positions[(first + a) as usize]);
            let ob = copy(m, &mut outer, b, m.positions[(first + b) as usize]);
            let lift = |i: u32| skin[i as usize] + normals[i as usize] * THIN * 0.5;
            let ia = copy(m, &mut inner, a, lift(a));
            let ib = copy(m, &mut inner, b, lift(b));
            m.indices.extend([ob, oa, ia, ob, ia, ib]);
        }
    }
    for t in m.indices[start..].to_vec().as_chunks::<3>().0 {
        let [a, b, c] = t.map(|v| m.positions[v as usize]);
        let n = (b - a).cross(c - a);
        for v in t {
            m.normals[*v as usize] += n;
        }
    }
    for v in outer.values().chain(inner.values()) {
        m.normals[*v as usize] = m.normals[*v as usize].normalize_or(Vec3::Y);
    }
}

/// How far out the hood hangs around each height: the convex hull of the cloth at that height (away from the opening, which would otherwise pull the hull across the face),
/// allowed to narrow going down only as fast as cloth falls. Returns, for a position, the
/// axis (x, z) and the hull's reach from it toward the position.
fn drape(skin: &[Vec3], face: &[f32]) -> impl Fn(Vec3) -> (Vec2, f32) {
    let (lo, hi) = skin
        .iter()
        .fold((f32::MAX, f32::MIN), |(a, b), p| (a.min(p.y), b.max(p.y)));
    let slices = (((hi - lo) / SLICE).ceil() as usize).max(1);
    let y_at = |k: usize| lo + (k as f32 + 0.5) * SLICE;
    // One axis for every slice, through the middle of the skull (whose slices are whole rings;
    // lower down the opening leaves crescents that needn't hold their own middle).
    let skull =
        skin.iter()
            .filter(|p| p.y > (lo + hi) * 0.5)
            .fold((Vec2::MAX, Vec2::MIN), |(a, b), p| {
                let q = Vec2::new(p.x, p.z);
                (a.min(q), b.max(q))
            });
    let c = (skull.0 + skull.1) * 0.5;
    let mut radii = vec![[0.0f32; AROUND]; slices];
    let dir = |j: usize| {
        let a = j as f32 / AROUND as f32 * std::f32::consts::TAU;
        Vec2::new(a.cos(), a.sin())
    };
    for (k, row) in radii.iter_mut().enumerate() {
        let y = y_at(k);
        let mut pts: Vec<Vec2> = skin
            .iter()
            .zip(face)
            .filter(|(p, f)| (p.y - y).abs() < SLICE * 1.5 && **f > 0.5)
            .map(|(p, _)| Vec2::new(p.x, p.z))
            .collect();
        if pts.len() < 3 {
            continue;
        }
        // With the axis in it, so every direction from the axis meets the hull once.
        pts.push(c);
        let h = hull(pts);
        if h.len() < 3 {
            continue;
        }
        for (j, r) in row.iter_mut().enumerate() {
            *r = reach(&h, c, dir(j));
        }
    }
    // Hang from above: each slice reaches at least as far as the one over it, less the drop.
    for k in (0..slices.saturating_sub(1)).rev() {
        let (below, above) = radii.split_at_mut(k + 1);
        for j in 0..AROUND {
            let back = smooth(0.0, -0.8, dir(j).y);
            let drop = DROP_SIDE + (DROP_BACK - DROP_SIDE) * back;
            below[k][j] = below[k][j].max(above[0][j] - drop * SLICE);
        }
    }
    move |p: Vec3| {
        let k = ((p.y - lo) / SLICE).floor().clamp(0.0, (slices - 1) as f32) as usize;
        let d = Vec2::new(p.x, p.z) - c;
        let a = d.y.atan2(d.x).rem_euclid(std::f32::consts::TAU) / std::f32::consts::TAU;
        let f = a * AROUND as f32;
        let j = f.floor() as usize % AROUND;
        let t = f.fract();
        (c, radii[k][j] * (1.0 - t) + radii[k][(j + 1) % AROUND] * t)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_hood_leaves_the_face_open() {
        let head = Vec3::new(0.0, 0.728, 0.02);
        // Nose and chin are in the opening; crown, ears, back of the head and nape are covered.
        assert!(!covers(Vec3::new(0.0, 0.70, 0.13), head));
        assert!(!covers(Vec3::new(0.0, 0.63, 0.1), head));
        assert!(!covers(Vec3::new(0.0, 0.6, 0.08), head));
        for r in [
            Vec3::new(0.0, 0.84, 0.0),
            Vec3::new(0.08, 0.72, 0.01),
            Vec3::new(0.0, 0.72, -0.08),
            Vec3::new(0.0, 0.58, -0.05),
            // The sides of the neck, and the cowl over the shirt's neckline.
            Vec3::new(0.045, 0.6, 0.0),
            Vec3::new(0.0, 0.52, 0.07),
        ] {
            assert!(covers(r, head), "{r}");
        }
        // The face's edge lands on the opening.
        for r in [
            Vec3::new(0.05, 0.72, 0.09),
            Vec3::new(0.02, 0.6, 0.06),
            Vec3::new(-0.03, 0.83, 0.07),
        ] {
            let q = opening(onto_opening(r, head), head);
            assert!((q - 1.0).abs() < 1e-3, "{r} {q}");
        }
        // Not out on the shoulders, nor down the chest.
        assert!(!covers(Vec3::new(0.15, 0.53, 0.0), head));
        assert!(!covers(Vec3::new(0.0, 0.45, 0.08), head));
    }
}
