//! Levels of detail for meshes on the instanced path (those too small for virtual geometry:
//! props, wall stones, debris). Each dense mesh gets a chain of simplified index lists over its
//! own vertices, built once on a worker thread, and every view picks the coarsest level whose
//! error stays under a pixel (the camera) or a texel (shadow maps). Objects smaller than that
//! are left out of the view altogether.

use engine_core::math::{Vec2, Vec3, Vec4};

use crate::mesh::{Mesh, Vertex};
use crate::virtual_geometry::simplify::{Welded, simplify, simplify_welded};

/// Meshes with fewer triangles get no levels (drawing them is cheap anyway).
pub const MIN_TRIANGLES: usize = 512;
/// Most levels below the mesh itself.
pub const MAX_LEVELS: usize = 6;
/// The chain stops at about this many triangles.
const FLOOR_TRIANGLES: usize = 48;
/// Each level aims at this share of the one above.
const STEP: f32 = 0.35;
/// A level that keeps more than this share of the one above isn't worth one.
const MIN_GAIN: f32 = 0.8;
/// How far the first level of a welded mesh may stray, as a share of the mesh's size (its
/// bounding box's diagonal; 5 mm on a 10 m house): each level may stray [`ERROR_GROWTH`] times
/// as far as the one above, and a level that would gain too little ([`MIN_GAIN`]) is tried
/// again with a budget that much larger, up to [`MOST_ERROR`]. Without a budget a level took
/// whatever collapses reached its triangle target: a house's first level, a third of its
/// triangles, strayed 10 to 30 cm (its last collapses), so it was due only beyond 300 m at
/// 1440p, where the house shows its far mesh or nothing.
const FIRST_ERROR: f32 = 0.0005;
const ERROR_GROWTH: f32 = 3.0;
const MOST_ERROR: f32 = 0.2;
/// One level of a mesh: its triangles as indices into the mesh's vertices followed by the
/// chain's own ([`Lods::vertices`]), and how far its surface strays from the full mesh
/// (meters in mesh space).
#[derive(Debug, Clone)]
pub struct LodIndices {
    pub indices: Vec<u32>,
    pub error: f32,
}

/// The levels of a mesh, finest first, and the vertices they add to the mesh's own: index
/// `mesh.vertices.len() + i` is `vertices[i]`.
#[derive(Debug, Clone, Default)]
pub struct Lods {
    pub levels: Vec<LodIndices>,
    pub vertices: Vec<Vertex>,
}

impl Lods {
    pub fn len(&self) -> usize {
        self.levels.len()
    }
}

/// A corner moves onto a vertex already at its new place when that vertex's texture
/// coordinate is where the corner's triangle expects it, within this share of how far the
/// corner moves in texture space; otherwise onto a vertex of the chain's own with the texture
/// coordinate expected. So a triangle never ends up spanning a seam, and one that moves
/// across a hard edge keeps its texture where it was (generated props project their texture
/// onto every face on its own: each of their edges is a seam, and a chamfer could not
/// collapse into the face beside it while moves had to land on a vertex the mesh has).
const UV_SLACK: f32 = 0.1;

/// Where a hard edge runs through a corner's new place, the vertex there nearest the corner's
/// own in normal is on the corner's side of it when their normals are this close (cosine);
/// otherwise the corner is drawn with a vertex of its own normal and color (a flat face stays
/// flat across the chamfer it swallows).
const SAME_SIDE_COS: f32 = 0.85;

/// No collapse may turn a triangle farther than this from the way its corners' normals face
/// (cosine), or further than it already was: the simplifier itself lets a triangle turn by up
/// to 78°, and thin overlapping pieces (a roof's tiles) then ended up drawn edge-on or upside
/// down with their tops' normals, a roof a third darker.
const FACING_COS: f32 = 0.9;

/// ... where "further" is by more than this (cosine, by area).
const FACING_SLACK: f32 = 0.05;

/// Two triangles that meet at an edge fold back onto each other past this (cosine between
/// their normals): the two sides of a leaf card, not a surface going on.
const FOLD_COS: f32 = -0.9;

/// The mesh's vertices by place. Generated meshes repeat a vertex wherever the normal, texture
/// coordinate or color changes (a hard edge, flat shading, a seam), and as indexed every such
/// edge looks like a border, which a simplifier must not move: such meshes got no levels at
/// all. So vertices that sit at one position count as one place where the surface goes on
/// across them (an edge there has exactly one triangle on either side), the levels are
/// simplified over the places, and every corner of every triangle remembers which of its
/// place's vertices (or of the chain's own there, [`Made`]) it is drawn with. Surfaces that only touch, or lie back to back, stay
/// apart as they are indexed.
struct Weld {
    /// The first vertex of each vertex's place.
    place: Vec<u32>,
    /// The next vertex of the same place (a ring through them).
    next: Vec<u32>,
}

impl Weld {
    fn new(mesh: &Mesh) -> Self {
        let n = mesh.vertices.len();
        // Vertices by position.
        let mut first: std::collections::HashMap<[u32; 3], u32> =
            std::collections::HashMap::with_capacity(n);
        let spot: Vec<u32> = mesh
            .vertices
            .iter()
            .enumerate()
            .map(|(i, v)| {
                // + 0.0: -0.0 and 0.0 are one position.
                let key = v.position.map(|c| (c + 0.0).to_bits());
                *first.entry(key).or_insert(i as u32)
            })
            .collect();
        // Edges by position, with the vertices the triangle has at either end.
        let position = |i: u32| Vec3::from(mesh.vertices[i as usize].position);
        let mut edges: Vec<(u32, u32, u32, u32, u32)> = Vec::with_capacity(mesh.indices.len());
        for (t, tri) in mesh.indices.as_chunks::<3>().0.iter().enumerate() {
            for k in 0..3 {
                let (a, b) = (tri[k], tri[(k + 1) % 3]);
                let (sa, sb) = (spot[a as usize], spot[b as usize]);
                if sa < sb {
                    edges.push((sa, sb, a, b, t as u32));
                } else if sb < sa {
                    edges.push((sb, sa, b, a, t as u32));
                }
            }
        }
        edges.sort_unstable();
        // Join the vertices across every edge that has one triangle on either side.
        let mut parent: Vec<u32> = (0..n as u32).collect();
        fn find(parent: &mut [u32], mut v: u32) -> u32 {
            while parent[v as usize] != v {
                parent[v as usize] = parent[parent[v as usize] as usize];
                v = parent[v as usize];
            }
            v
        }
        let normal = |t: u32| {
            let [a, b, c] = mesh.indices.as_chunks::<3>().0[t as usize].map(position);
            (b - a).cross(c - a).normalize_or_zero()
        };
        for run in edges.chunk_by(|a, b| (a.0, a.1) == (b.0, b.1)) {
            if let [e, f] = run
                && normal(e.4).dot(normal(f.4)) > FOLD_COS
            {
                for (a, b) in [(e.2, f.2), (e.3, f.3)] {
                    let (a, b) = (find(&mut parent, a), find(&mut parent, b));
                    // The lower index leads, so a place is named by its first vertex.
                    parent[a.max(b) as usize] = a.min(b);
                }
            }
        }
        let place: Vec<u32> = (0..n as u32).map(|v| find(&mut parent, v)).collect();
        let mut next: Vec<u32> = (0..n as u32).collect();
        for (i, &p) in place.iter().enumerate() {
            if p != i as u32 {
                next[i] = next[p as usize];
                next[p as usize] = i as u32;
            }
        }
        Self { place, next }
    }

    /// The vertex at place `at` for a corner drawn with `was` so far, in a triangle whose
    /// other two corners are drawn with `b` and `c`: the vertex there on the corner's side of
    /// any hard edge ([`SAME_SIDE_COS`]) whose texture coordinate is where the triangle's own
    /// mapping puts that place ([`UV_SLACK`]); when there is none, a vertex of the chain's own
    /// there with that texture coordinate, and that side's normal and color (or the corner's
    /// own).
    fn vertex(&self, verts: &mut Made, at: u32, was: u32, b: u32, c: u32) -> u32 {
        let (vw, vb, vc) = (*verts.get(was), *verts.get(b), *verts.get(c));
        let (p0, p1, p2) = (
            Vec3::from(vw.position),
            Vec3::from(vb.position),
            Vec3::from(vc.position),
        );
        let (t0, t1, t2) = (Vec2::from(vw.uv), Vec2::from(vb.uv), Vec2::from(vc.uv));
        let target = verts.mesh[at as usize];
        // The triangle's texture mapping, carried on to the new place.
        let (e1, e2, d) = (p1 - p0, p2 - p0, Vec3::from(target.position) - p0);
        let (d00, d01, d11) = (e1.dot(e1), e1.dot(e2), e2.dot(e2));
        let det = d00 * d11 - d01 * d01;
        let expected = if det > 1e-12 * d00 * d11 {
            let (r0, r1) = (d.dot(e1), d.dot(e2));
            let (a, b) = ((d11 * r0 - d01 * r1) / det, (d00 * r1 - d01 * r0) / det);
            t0 + (t1 - t0) * a + (t2 - t0) * b
        } else {
            t0
        };
        let mesh = verts.mesh;
        let ring = |f: &mut dyn FnMut(u32)| {
            let mut i = at;
            loop {
                f(i);
                i = self.next[i as usize];
                if i == at {
                    break;
                }
            }
        };
        let uv = |i: u32| Vec2::from(mesh[i as usize].uv);
        let n = |i: u32| Vec3::from(mesh[i as usize].normal);
        let (normal, color) = (Vec3::from(vw.normal), Vec4::from(vw.color));
        let off = |i: u32| {
            1.0 - normal.dot(n(i)) + (Vec4::from(mesh[i as usize].color) - color).length_squared()
        };
        // The vertex there on the corner's side of a hard edge: nearest the corner's own in
        // normal and color. None when the place has a hard edge and no vertex on that side
        // (a face's corner moving onto a chamfer's edge): the corner keeps its own normal.
        let mut side: Option<(f32, u32)> = None;
        let mut hard = false;
        ring(&mut |i| {
            let o = off(i);
            hard |= n(i).dot(n(at)) < 0.999;
            if side.is_none_or(|(b, _)| o < b) {
                side = Some((o, i));
            }
        });
        let side = side
            .map(|(_, i)| i)
            .filter(|&i| !hard || normal.dot(n(i)) >= SAME_SIDE_COS);
        let (side_normal, side_color) = side.map_or((vw.normal, vw.color), |i| {
            (mesh[i as usize].normal, mesh[i as usize].color)
        });
        // Of the vertices on that side (one normal and color: a texture seam may still run
        // through them), the one whose texture coordinate is where the triangle expects it
        // ([`UV_SLACK`]: off by a share of how far the corner travels in the texture, so a
        // move along a seam lands on its vertex there, a move across it misses by the
        // texture's width).
        let slack = (expected - t0).length_squared() * UV_SLACK * UV_SLACK + 1e-10;
        let mut best: Option<(f32, u32)> = None;
        if side.is_some() {
            ring(&mut |i| {
                let v = &mesh[i as usize];
                if v.normal == side_normal && v.color == side_color {
                    let d = (uv(i) - expected).length_squared();
                    if d <= slack && best.is_none_or(|(b, _)| d < b) {
                        best = Some((d, i));
                    }
                }
            });
        }
        if let Some((_, i)) = best {
            return i;
        }
        let mut made = target;
        made.uv = expected.to_array();
        made.normal = side_normal;
        made.color = side_color;
        verts.make(made)
    }
}

/// How triangles face, summed by area: how far each faces the way its corners' normals say
/// (the cosine between its own normal and theirs on average; 1 for a flat-shaded face), and
/// where those normals point on the whole.
#[derive(Default)]
struct Facing {
    area: f32,
    cos: f32,
    normal: Vec3,
}

/// A level faces as the mesh does when its facing (cosine, by area) is within this much of
/// the mesh's. A six-sided rope hanging in waves loses about 0.035 as its segments get longer;
/// a roof of overlapping tiles lost 0.16 in a level that stayed within its error, and the roof
/// looked darker.
const LEVEL_FACING: f32 = 0.06;

/// ... and when its normals' sum by area, as a share of the area, moved at most this far.
/// Boxes that lose their chamfers move about 0.045; that roof moved 0.18.
const LEVEL_NORMALS: f32 = 0.08;

impl Facing {
    fn add(&mut self, verts: &Made, tri: [u32; 3]) {
        let [a, b, c] = tri.map(|i| *verts.get(i));
        let p = |v: Vertex| Vec3::from(v.position);
        let cross = (p(b) - p(a)).cross(p(c) - p(a));
        let area = cross.length() * 0.5;
        let theirs = Vec3::from(a.normal) + Vec3::from(b.normal) + Vec3::from(c.normal);
        self.area += area;
        self.cos += area * cross.normalize_or_zero().dot(theirs.normalize_or_zero());
        self.normal += theirs * (area / 3.0);
    }

    fn cos(&self) -> f32 {
        if self.area > 0.0 {
            self.cos / self.area
        } else {
            1.0
        }
    }

    fn like(&self, whole: &Facing) -> bool {
        let share = |f: &Facing| f.normal / f.area.max(1e-12);
        self.cos() >= whole.cos() - LEVEL_FACING
            && (share(self) - share(whole)).length() <= LEVEL_NORMALS
    }
}

/// The mesh's vertices and those the chain of levels made, which a level's corners may be
/// drawn with too (one copy of each).
struct Made<'a> {
    mesh: &'a [Vertex],
    made: Vec<Vertex>,
    index: std::collections::HashMap<[u32; 12], u32>,
}

impl<'a> Made<'a> {
    fn new(mesh: &'a [Vertex]) -> Self {
        Self {
            mesh,
            made: Vec::new(),
            index: std::collections::HashMap::new(),
        }
    }

    fn get(&self, i: u32) -> &Vertex {
        let n = self.mesh.len();
        match (i as usize).checked_sub(n) {
            Some(k) => &self.made[k],
            None => &self.mesh[i as usize],
        }
    }

    fn make(&mut self, v: Vertex) -> u32 {
        let b = |x: &[f32]| x.iter().map(|c| (c + 0.0).to_bits()).collect::<Vec<u32>>();
        let key: [u32; 12] = [b(&v.position), b(&v.normal), b(&v.uv), b(&v.color)]
            .concat()
            .try_into()
            .unwrap_or([0; 12]);
        let (n, made) = (self.mesh.len() as u32, &mut self.made);
        *self.index.entry(key).or_insert_with(|| {
            made.push(v);
            n + made.len() as u32 - 1
        })
    }
}

/// The levels of `mesh`, finest first (empty for a mesh too small or too ragged to simplify).
/// `ENGINE_MESH_LOD_WELD=0` in the environment builds them as before 2026-11 (over the
/// vertices as indexed, borders fixed), to compare.
pub fn build(mesh: &Mesh) -> Lods {
    static WELD: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    let weld = *WELD.get_or_init(|| std::env::var("ENGINE_MESH_LOD_WELD").is_ok_and(|v| v == "0"));
    build_as(mesh, !weld)
}

fn build_as(mesh: &Mesh, welded: bool) -> Lods {
    let corners = mesh.indices.as_chunks::<3>().0;
    if corners.len() < MIN_TRIANGLES {
        return Lods::default();
    }
    if !welded {
        return Lods {
            levels: build_as_indexed(mesh),
            vertices: Vec::new(),
        };
    }
    let positions: Vec<Vec3> = mesh.vertices.iter().map(|v| v.position.into()).collect();
    let weld = Weld::new(mesh);
    // The triangles over the places, and the vertex each of their corners is drawn with.
    let (mut current, mut drawn): (Vec<[u32; 3]>, Vec<[u32; 3]>) = corners
        .iter()
        .map(|tri| (tri.map(|i| weld.place[i as usize]), *tri))
        .filter(|(c, _)| c[0] != c[1] && c[1] != c[2] && c[0] != c[2])
        .unzip();
    // The connected parts (a merged fence is many posts and rails), and each one's surface.
    let mut part: Vec<u32> = (0..positions.len() as u32).collect();
    fn find(part: &mut [u32], mut v: u32) -> u32 {
        while part[v as usize] != v {
            part[v as usize] = part[part[v as usize] as usize];
            v = part[v as usize];
        }
        v
    }
    for tri in &current {
        let a = find(&mut part, tri[0]);
        for &v in &tri[1..] {
            let b = find(&mut part, v);
            part[b as usize] = a;
        }
    }
    for v in 0..part.len() as u32 {
        part[v as usize] = find(&mut part, v);
    }
    let mut part_area = vec![0.0f64; positions.len()];
    for tri in &current {
        let [a, b, c] = tri.map(|i| positions[i as usize].as_dvec3());
        part_area[part[tri[0] as usize] as usize] += (b - a).cross(c - a).length() * 0.5;
    }
    let locked = vec![false; positions.len()];
    let mut levels = Vec::new();
    let mut error = 0.0f32;
    let mut made = Made::new(&mesh.vertices);
    let mut staged: Vec<u32> = Vec::new();
    let size = {
        let (lo, hi) = positions.iter().fold(
            (Vec3::splat(f32::INFINITY), Vec3::splat(f32::NEG_INFINITY)),
            |(lo, hi), p| (lo.min(*p), hi.max(*p)),
        );
        (hi - lo).length().max(1e-6)
    };
    let mut budget = size * FIRST_ERROR;
    let mut whole = Facing::default();
    for tri in corners {
        whole.add(&made, *tri);
    }
    while levels.len() < MAX_LEVELS && current.len() > FLOOR_TRIANGLES * 2 {
        let target = ((current.len() as f32 * STEP) as usize).max(FLOOR_TRIANGLES);
        let before = drawn.clone();
        // Every corner a collapse moves is drawn with a vertex at its new place from then on.
        // A collapse happens only if every triangle it moves still faces the way its corners'
        // normals say ([`FACING_COS`]).
        let mut moved = |moves: &[(u32, u8)], to: u32| {
            staged.clear();
            let (mut was, mut now) = (Facing::default(), Facing::default());
            for &(t, k) in moves {
                let d = drawn[t as usize];
                let k = k as usize;
                let v = weld.vertex(&mut made, to, d[k], d[(k + 1) % 3], d[(k + 2) % 3]);
                let mut after = d;
                after[k] = v;
                was.add(&made, d);
                now.add(&made, after);
                staged.push(v);
            }
            // (By area: a chamfer's last sliver may lie in the face it joins.)
            if now.cos() < FACING_COS.min(was.cos() - FACING_SLACK) {
                return false;
            }
            for (&(t, k), &v) in moves.iter().zip(&staged) {
                drawn[t as usize][k as usize] = v;
            }
            true
        };
        let out = simplify_welded(
            &positions,
            &current,
            &locked,
            target,
            Welded {
                part: &part,
                part_area: &part_area,
                moved: &mut moved,
                max_error: budget,
            },
        );
        budget *= ERROR_GROWTH;
        // A level must face as the mesh does, by area ([`LEVEL_FACING`], [`LEVEL_NORMALS`]).
        let mut level = Facing::default();
        for &k in &out.kept {
            level.add(&made, drawn[k as usize]);
        }
        if !level.like(&whole) {
            break;
        }
        if out.triangles.is_empty() || out.triangles.len() as f32 > current.len() as f32 * MIN_GAIN
        {
            if budget > size * MOST_ERROR {
                break;
            }
            drawn = before;
            continue;
        }
        // Errors add up along the chain (each level is measured against the one above).
        error += out.error;
        drawn = out.kept.iter().map(|&k| drawn[k as usize]).collect();
        levels.push(LodIndices {
            indices: drawn.iter().flatten().copied().collect(),
            error,
        });
        current = out.triangles;
    }
    // Only the vertices some level still draws with.
    let mut keep = vec![u32::MAX; made.made.len()];
    let mut vertices = Vec::new();
    let n = mesh.vertices.len() as u32;
    for l in &mut levels {
        for i in &mut l.indices {
            if let Some(k) = i.checked_sub(n) {
                if keep[k as usize] == u32::MAX {
                    keep[k as usize] = n + vertices.len() as u32;
                    vertices.push(made.made[k as usize]);
                }
                *i = keep[k as usize];
            }
        }
    }
    Lods { levels, vertices }
}

/// The levels over the vertices as the mesh is indexed: every hard edge and seam is a border,
/// and borders don't move.
fn build_as_indexed(mesh: &Mesh) -> Vec<LodIndices> {
    let positions: Vec<Vec3> = mesh.vertices.iter().map(|v| v.position.into()).collect();
    let locked = vec![false; positions.len()];
    let mut current: Vec<[u32; 3]> = mesh.indices.as_chunks::<3>().0.to_vec();
    let mut levels = Vec::new();
    let mut error = 0.0f32;
    while levels.len() < MAX_LEVELS && current.len() > FLOOR_TRIANGLES * 2 {
        let target = ((current.len() as f32 * STEP) as usize).max(FLOOR_TRIANGLES);
        let out = simplify(&positions, &current, &locked, target);
        if out.triangles.is_empty() || out.triangles.len() as f32 > current.len() as f32 * MIN_GAIN
        {
            break;
        }
        error += out.error;
        levels.push(LodIndices {
            indices: out.triangles.iter().flatten().copied().collect(),
            error,
        });
        current = out.triangles;
    }
    levels
}

/// Why a mesh may give no levels, for the profile log: the share of its vertices the simplifier
/// may move (every edge at them shared by exactly two triangles) as the mesh is indexed, with
/// vertices of one position and texture coordinate counted as one (hard edges closed), and
/// with vertices of one position counted as one (texture seams closed too).
pub fn movable_shares(mesh: &Mesh) -> [f32; 3] {
    use std::collections::HashMap;
    let n = mesh.vertices.len();
    let share = |canon: &[u32]| {
        let mut edges: Vec<(u32, u32)> = Vec::with_capacity(mesh.indices.len());
        for tri in mesh.indices.as_chunks::<3>().0 {
            let t = tri.map(|i| canon[i as usize]);
            if t[0] == t[1] || t[1] == t[2] || t[0] == t[2] {
                continue;
            }
            for k in 0..3 {
                let (a, b) = (t[k], t[(k + 1) % 3]);
                edges.push((a.min(b), a.max(b)));
            }
        }
        edges.sort_unstable();
        let mut used = vec![false; n];
        let mut fixed = vec![false; n];
        for run in edges.chunk_by(|a, b| a == b) {
            for v in [run[0].0, run[0].1] {
                used[v as usize] = true;
                fixed[v as usize] |= run.len() != 2;
            }
        }
        let total = used.iter().filter(|u| **u).count().max(1);
        let free = used.iter().zip(&fixed).filter(|(u, f)| **u && !**f).count();
        free as f32 / total as f32
    };
    let weld = |uv: bool| {
        let mut first: HashMap<[u32; 5], u32> = HashMap::new();
        mesh.vertices
            .iter()
            .enumerate()
            .map(|(i, v)| {
                let p = v.position.map(|c| (c + 0.0).to_bits());
                let t = if uv { v.uv.map(f32::to_bits) } else { [0; 2] };
                *first
                    .entry([p[0], p[1], p[2], t[0], t[1]])
                    .or_insert(i as u32)
            })
            .collect::<Vec<u32>>()
    };
    let own: Vec<u32> = (0..n as u32).collect();
    [share(&own), share(&weld(true)), share(&weld(false))]
}

/// The coarsest level (0 = the mesh itself) whose error, scaled by `scale`, stays within
/// `tolerance` meters. `errors` are the levels' errors, finest first.
pub fn pick(errors: impl Iterator<Item = f32>, scale: f32, tolerance: f32) -> u8 {
    let mut level = 0;
    for (i, e) in errors.enumerate() {
        if e * scale > tolerance {
            break;
        }
        level = i as u8 + 1;
    }
    level
}

/// Meters one pixel spans at `distance` from a perspective camera (`fov_y` radians, `height`
/// pixels); `None` for an orthographic view.
pub fn meters_per_pixel(fov_y: f32, height: u32, distance: f32) -> Option<f32> {
    (fov_y > 0.0).then(|| 2.0 * (fov_y * 0.5).tan() * distance.max(1e-3) / height.max(1) as f32)
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::rock::rock;

    /// The levels of `mesh`, and the mesh with the vertices they made added to its own (what
    /// their indices point into).
    fn built(mesh: &Mesh) -> (Mesh, Vec<LodIndices>) {
        let lods = build(mesh);
        let mut all = mesh.clone();
        all.vertices.extend_from_slice(&lods.vertices);
        (all, lods.levels)
    }

    #[test]
    fn a_dense_stone_gets_a_chain_of_ever_coarser_levels() {
        let mesh = rock(Vec3::new(0.4, 0.25, 0.3), 7, 5);
        let full = mesh.indices.len() / 3;
        let (mesh, levels) = built(&mesh);
        assert!(levels.len() >= 3, "{} levels", levels.len());
        let mut last = full;
        let mut last_error = 0.0;
        for l in &levels {
            let n = l.indices.len() / 3;
            assert!(n < last, "{n} triangles after {last}");
            assert!(l.error >= last_error);
            assert!(
                l.indices
                    .iter()
                    .all(|&i| (i as usize) < mesh.vertices.len())
            );
            last = n;
            last_error = l.error;
        }
        let coarsest = levels.last().unwrap();
        assert!(
            coarsest.indices.len() / 3 <= full / 20,
            "{full} down to {}",
            coarsest.indices.len() / 3
        );
        // Still the stone: the coarsest level strays less than a quarter of its size, and the
        // first one less than a percent.
        assert!(coarsest.error < 0.1, "error {} m", coarsest.error);
        assert!(levels[0].error < 0.004, "error {} m", levels[0].error);
    }

    /// `mesh` with three vertices of its own for every triangle, each with the triangle's
    /// normal (flat shading, as most generated props are built).
    fn faceted(mesh: &Mesh) -> Mesh {
        let mut out = mesh.clone();
        out.vertices.clear();
        out.indices.clear();
        for tri in mesh.indices.as_chunks::<3>().0 {
            let [a, b, c] = tri.map(|i| Vec3::from(mesh.vertices[i as usize].position));
            let normal = (b - a).cross(c - a).normalize_or_zero();
            for &i in tri {
                let mut v = mesh.vertices[i as usize];
                v.normal = normal.to_array();
                // One texture coordinate per place, as a generated prop has.
                v.uv = [v.position[0], v.position[2]];
                out.indices.push(out.vertices.len() as u32);
                out.vertices.push(v);
            }
        }
        out
    }

    #[test]
    fn a_flat_shaded_stone_gets_levels_too_and_keeps_its_facets_normals() {
        let smooth = rock(Vec3::new(0.4, 0.25, 0.3), 7, 5);
        let mesh = faceted(&smooth);
        let full = mesh.indices.len() / 3;
        let (mesh, levels) = built(&mesh);
        assert!(levels.len() >= 3, "{} levels", levels.len());
        let coarsest = levels.last().unwrap();
        assert!(
            coarsest.indices.len() / 3 <= full / 20,
            "{full} down to {}",
            coarsest.indices.len() / 3
        );
        assert!(coarsest.error < 0.1, "error {} m", coarsest.error);
        // Every corner is drawn with a vertex of the mesh whose normal is near the
        // triangle's own, though the triangles are new.
        let first = &levels[0];
        let mut facing = 0.0;
        for tri in first.indices.as_chunks::<3>().0 {
            let [a, b, c] = tri.map(|i| Vec3::from(mesh.vertices[i as usize].position));
            let normal = (b - a).cross(c - a).normalize_or_zero();
            assert!(normal.length() > 0.5, "a triangle collapsed to nothing");
            for &i in tri {
                facing += normal.dot(Vec3::from(mesh.vertices[i as usize].normal));
            }
        }
        facing /= first.indices.len() as f32;
        assert!(facing > 0.9, "normals {facing} off the faces");
    }

    #[test]
    fn as_indexed_a_flat_shaded_stone_gets_no_levels() {
        // What the switch goes back to: every edge a border.
        let mesh = faceted(&rock(Vec3::new(0.4, 0.25, 0.3), 7, 5));
        assert!(build_as(&mesh, false).levels.is_empty());
        assert!(
            !build_as(&rock(Vec3::new(0.4, 0.25, 0.3), 7, 5), false)
                .levels
                .is_empty()
        );
    }

    /// An open tube along y, swaying `wave` to the sides: `rings` rings of `sides` vertices
    /// plus one more per ring where the texture wraps around (u 0 and u 1 at one position).
    fn tube(rings: u32, sides: u32, radius: f32, length: f32, wave: f32) -> Mesh {
        let mut mesh = Mesh::new(Vec::new(), Vec::new());
        for r in 0..rings {
            let y = length * r as f32 / (rings - 1) as f32;
            for s in 0..=sides {
                let a = std::f32::consts::TAU * (s % sides) as f32 / sides as f32;
                let n = Vec3::new(a.cos(), 0.0, a.sin());
                mesh.vertices.push(crate::mesh::Vertex::new(
                    n * radius + Vec3::new(wave * (y * 2.0).sin(), y, 0.0),
                    n,
                    Vec2::new(s as f32 / sides as f32, y),
                ));
            }
        }
        let row = sides + 1;
        for r in 0..rings - 1 {
            for s in 0..sides {
                let i = r * row + s;
                mesh.indices
                    .extend([i, i + row, i + 1, i + 1, i + row, i + row + 1]);
            }
        }
        mesh
    }

    fn area(mesh: &Mesh, indices: &[u32]) -> f32 {
        indices
            .as_chunks::<3>()
            .0
            .iter()
            .map(|t| {
                let [a, b, c] = t.map(|i| Vec3::from(mesh.vertices[i as usize].position));
                (b - a).cross(c - a).length() * 0.5
            })
            .sum()
    }

    #[test]
    fn a_tube_gets_levels_and_no_triangle_spans_its_seam() {
        let mesh = tube(48, 16, 0.5, 6.0, 0.0);
        let full = mesh.indices.len() / 3;
        let (mesh, levels) = built(&mesh);
        assert!(!levels.is_empty(), "no levels");
        let last = levels.last().unwrap();
        assert!(
            last.indices.len() / 3 <= full / 4,
            "{full} down to {}",
            last.indices.len() / 3
        );
        for l in &levels {
            for tri in l.indices.as_chunks::<3>().0 {
                let u = tri.map(|i| mesh.vertices[i as usize].uv[0]);
                let span =
                    u.iter().cloned().fold(0.0, f32::max) - u.iter().cloned().fold(1.0, f32::min);
                assert!(
                    span < 0.5,
                    "a triangle wraps around the texture: u {u:?}, vertices {:?} (ring, side), level of {} triangles",
                    tri.map(|i| (i / 17, i % 17)),
                    l.indices.len() / 3
                );
            }
            // Still a tube from end to end, as wide as it was.
            let ys = l
                .indices
                .iter()
                .map(|&i| mesh.vertices[i as usize].position[1]);
            assert_eq!(ys.clone().fold(f32::INFINITY, f32::min), 0.0);
            assert_eq!(ys.fold(0.0, f32::max), 6.0);
            assert!(area(&mesh, &l.indices) > 0.9 * area(&mesh, &mesh.indices));
        }
    }

    #[test]
    fn a_rope_gets_shorter_segments_but_stays_as_thick() {
        // 1 cm thick, 20 m long and hanging in waves: flattening it strays far less than
        // straightening it does, and it must not flatten.
        let mesh = tube(120, 6, 0.005, 20.0, 0.3);
        let full = mesh.indices.len() / 3;
        let (mesh, levels) = built(&mesh);
        assert!(!levels.is_empty(), "no levels");
        let last = levels.last().unwrap();
        // (Halving it took flattening it: its shading then agreed with its normals by 0.72
        // instead of 0.94.)
        assert!(
            last.indices.len() / 3 <= full * 3 / 5,
            "{full} down to {}",
            last.indices.len() / 3
        );
        let facing = |indices: &[u32]| {
            let (mut area, mut cos) = (0.0, 0.0);
            for t in indices.as_chunks::<3>().0 {
                let [a, b, c] = t.map(|i| mesh.vertices[i as usize]);
                let p = |v: crate::mesh::Vertex| Vec3::from(v.position);
                let cross = (p(b) - p(a)).cross(p(c) - p(a));
                let n = Vec3::from(a.normal) + Vec3::from(b.normal) + Vec3::from(c.normal);
                area += cross.length();
                cos += cross.length() * cross.normalize_or_zero().dot(n.normalize_or_zero());
            }
            cos / area
        };
        for l in &levels {
            assert!(
                facing(&l.indices) > facing(&mesh.indices) - 0.06,
                "the rope flattened: facing {} of {}",
                facing(&l.indices),
                facing(&mesh.indices)
            );
            assert!(
                area(&mesh, &l.indices) > 0.7 * area(&mesh, &mesh.indices),
                "the rope lost {:.0}% of its surface",
                100.0 - 100.0 * area(&mesh, &l.indices) / area(&mesh, &mesh.indices)
            );
        }
    }

    #[test]
    fn planks_lose_the_vertices_along_their_sides_and_nothing_else() {
        // A boardwalk in one mesh: flat strips of 8 quads each, 70 cm by 27 m, far apart.
        let mut mesh = Mesh::new(Vec::new(), Vec::new());
        for plank in 0..40u32 {
            let at = Vec3::new(plank as f32 * 8.0, (plank % 3) as f32, 0.0);
            let first = mesh.vertices.len() as u32;
            for row in 0..9 {
                for x in [0.0, 0.7] {
                    let p = at + Vec3::new(x, 0.0, row as f32 * 27.0 / 8.0);
                    mesh.vertices
                        .push(crate::mesh::Vertex::new(p, Vec3::Y, Vec2::new(x, p.z)));
                }
            }
            for row in 0..8 {
                let i = first + row * 2;
                mesh.indices.extend([i, i + 2, i + 1, i + 1, i + 2, i + 3]);
            }
        }
        let (mesh, levels) = built(&mesh);
        assert!(!levels.is_empty(), "no levels");
        // Two triangles a plank at the end, and every plank as large as it was.
        let last = levels.last().unwrap();
        assert_eq!(last.indices.len() / 3, 80);
        for l in &levels {
            let (kept, full) = (area(&mesh, &l.indices), area(&mesh, &mesh.indices));
            assert!(
                (kept / full - 1.0).abs() < 1e-3,
                "planks kept {kept} of {full} m²"
            );
        }
    }

    #[test]
    fn bent_blades_of_grass_dont_become_the_triangle_between_their_ends() {
        // A blade 5 mm wide and 57 cm long that bows 20 cm to the side: cutting its bend
        // short strays less than a hand's width, and makes it thirteen times its size.
        let spine = [
            (0.215, 0.0, 0.0),
            (0.191, 0.163, 0.022),
            (0.158, 0.323, 0.052),
            (0.097, 0.468, 0.106),
            (0.0, 0.571, 0.192),
        ];
        let mut mesh = Mesh::new(Vec::new(), Vec::new());
        for blade in 0..80u32 {
            let at = Vec3::new((blade % 10) as f32, 0.0, (blade / 10) as f32);
            let first = mesh.vertices.len() as u32;
            for (i, &(x, y, z)) in spine.iter().enumerate() {
                let p = at + Vec3::new(x, y, z);
                let v = 1.0 - i as f32 * 0.25;
                mesh.vertices
                    .push(crate::mesh::Vertex::new(p, Vec3::Z, Vec2::new(0.0, v)));
                if i + 1 < spine.len() {
                    mesh.vertices.push(crate::mesh::Vertex::new(
                        p + Vec3::new(0.004, 0.0, 0.004),
                        Vec3::Z,
                        Vec2::new(1.0, v),
                    ));
                }
            }
            let strip = [
                0, 1, 3, 0, 3, 2, 2, 3, 5, 2, 5, 4, 4, 5, 7, 4, 7, 6, 6, 7, 8,
            ];
            mesh.indices.extend(strip.map(|k| first + k));
        }
        assert!(mesh.indices.len() / 3 >= MIN_TRIANGLES);
        let full = area(&mesh, &mesh.indices);
        for l in built(&mesh).1 {
            let kept = area(&mesh, &l.indices) / full;
            assert!(
                (0.85..1.2).contains(&kept),
                "the blades are {kept} times their size"
            );
        }
    }

    #[test]
    fn leaf_cards_keep_their_corners() {
        // Cards with both faces as triangles of their own, back to back.
        let mut mesh = Mesh::new(Vec::new(), Vec::new());
        for i in 0..160u32 {
            let at = Vec3::new((i % 16) as f32, (i / 16) as f32, (i * 7 % 5) as f32);
            let first = mesh.vertices.len() as u32;
            for (x, y) in [(0.0, 0.0), (0.4, 0.0), (0.4, 0.3), (0.0, 0.3)] {
                mesh.vertices.push(crate::mesh::Vertex::new(
                    at + Vec3::new(x, y, 0.0),
                    Vec3::Z,
                    Vec2::new(x, y),
                ));
            }
            let quad = [0, 1, 2, 0, 2, 3, 0, 2, 1, 0, 3, 2];
            mesh.indices.extend(quad.map(|k| first + k));
        }
        assert!(mesh.indices.len() / 3 >= MIN_TRIANGLES);
        assert!(build(&mesh).levels.is_empty());
    }

    /// Boxes with every edge chamfered by `c`, each face with vertices of its own and its
    /// texture projected onto it in meters (as the showcase's houses build their timbers and
    /// stones), `n` of them in a row.
    fn chamfered_boxes(n: u32, h: Vec3, c: f32) -> Mesh {
        let mut mesh = Mesh::new(Vec::new(), Vec::new());
        let mut face = |pts: &[Vec3], facing: Vec3| {
            let f = facing.normalize();
            let t = (Vec3::X - f * f.x).normalize_or(Vec3::Z);
            let t = if t.length_squared() < 0.04 {
                Vec3::Z
            } else {
                t
            };
            let bt = f.cross(t);
            let first = mesh.vertices.len() as u32;
            for p in pts {
                mesh.vertices.push(crate::mesh::Vertex::new(
                    *p,
                    f,
                    Vec2::new(p.dot(t), p.dot(bt)),
                ));
            }
            let normal = (pts[1] - pts[0]).cross(pts[2] - pts[0]);
            for i in 1..pts.len() as u32 - 1 {
                if normal.dot(f) > 0.0 {
                    mesh.indices.extend([first, first + i, first + i + 1]);
                } else {
                    mesh.indices.extend([first, first + i + 1, first + i]);
                }
            }
        };
        for b in 0..n {
            let at = Vec3::new(b as f32 * h.x * 3.0, 0.0, 0.0);
            // Corner `s` of the face square to axis `i` (the others pulled in by the chamfer).
            let pt = |i: usize, s: [f32; 3]| {
                let mut p = at;
                for m in 0..3 {
                    p[m] += if m == i { h[m] } else { h[m] - c } * s[m];
                }
                p
            };
            let quad = [(-1.0, -1.0), (1.0, -1.0), (1.0, 1.0), (-1.0, 1.0)];
            for i in 0..3 {
                let (j, k) = ((i + 1) % 3, (i + 2) % 3);
                for si in [-1.0f32, 1.0] {
                    let pts: Vec<Vec3> = quad
                        .iter()
                        .map(|&(sj, sk)| {
                            let mut s = [0.0; 3];
                            (s[i], s[j], s[k]) = (si, sj, sk);
                            pt(i, s)
                        })
                        .collect();
                    let mut facing = Vec3::ZERO;
                    facing[i] = si;
                    face(&pts, facing);
                }
                for si in [-1.0f32, 1.0] {
                    for sj in [-1.0f32, 1.0] {
                        let s = |sk: f32| {
                            let mut s = [0.0; 3];
                            (s[i], s[j], s[k]) = (si, sj, sk);
                            s
                        };
                        let pts = [pt(i, s(-1.0)), pt(i, s(1.0)), pt(j, s(1.0)), pt(j, s(-1.0))];
                        let mut facing = Vec3::ZERO;
                        (facing[i], facing[j]) = (si, sj);
                        face(&pts, facing);
                    }
                }
            }
            for s0 in [-1.0f32, 1.0] {
                for s1 in [-1.0f32, 1.0] {
                    for s2 in [-1.0f32, 1.0] {
                        let s = [s0, s1, s2];
                        face(&[pt(0, s), pt(1, s), pt(2, s)], Vec3::from(s));
                    }
                }
            }
        }
        mesh
    }

    #[test]
    fn chamfered_boxes_lose_their_chamfers_and_keep_their_texture_in_place() {
        // Timbers of a house: 1 m by 20 by 16 cm, chamfered by 2 cm. Every edge is a texture
        // seam, so no chamfer could collapse into the face beside it while corners had to land
        // on a vertex the mesh has: such meshes got no levels.
        let c = 0.02;
        let mesh = chamfered_boxes(24, Vec3::new(0.5, 0.1, 0.08), c);
        let full = mesh.indices.len() / 3;
        assert_eq!(full, 24 * 44);
        let (mesh, levels) = built(&mesh);
        assert!(!levels.is_empty(), "no levels");
        // Half the chamfers go where they are under a pixel, then the rest: plain boxes.
        let (first, last) = (&levels[0], levels.last().unwrap());
        assert!(
            first.indices.len() / 3 <= full * 6 / 10 && first.error <= c,
            "{full} down to {} straying {} m",
            first.indices.len() / 3,
            first.error
        );
        assert_eq!(last.indices.len() / 3, 24 * 12);
        assert!(last.error <= 2.0 * c, "error {} m", last.error);
        // Texture in meters on every face, as it was: no triangle stretches it (a corner that
        // had to land on a vertex the mesh has would take another face's texture coordinate,
        // a tenth of a meter off). And the faces keep their normals, by area.
        for l in &levels {
            let (mut facing, mut total) = (0.0, 0.0);
            for tri in l.indices.as_chunks::<3>().0 {
                let v = tri.map(|i| mesh.vertices[i as usize]);
                for k in 0..3 {
                    let (a, b) = (v[k], v[(k + 1) % 3]);
                    let along = Vec3::from(a.position).distance(Vec3::from(b.position));
                    let in_texture = Vec2::from(a.uv).distance(Vec2::from(b.uv));
                    // (Within a quarter of a chamfer: a chamfer half gone leaves slivers.)
                    assert!(
                        (in_texture - along).abs() <= 0.15 * along + 0.25 * c,
                        "an edge of {along} m spans {in_texture} m of texture"
                    );
                }
                let [p0, p1, p2] = v.map(|v| Vec3::from(v.position));
                let cross = (p1 - p0).cross(p2 - p0);
                for v in v {
                    facing += cross.normalize_or_zero().dot(Vec3::from(v.normal)) * cross.length();
                    total += cross.length();
                }
            }
            assert!(
                facing / total > 0.97,
                "normals {} off the faces",
                facing / total
            );
        }
    }

    #[test]
    fn levels_are_picked_by_their_error_on_screen() {
        let errors = [0.001, 0.004, 0.02, 0.08];
        assert_eq!(pick(errors.into_iter(), 1.0, 0.0005), 0);
        assert_eq!(pick(errors.into_iter(), 1.0, 0.005), 2);
        assert_eq!(pick(errors.into_iter(), 2.0, 0.005), 1);
        assert_eq!(pick(errors.into_iter(), 1.0, 1.0), 4);
        let near = meters_per_pixel(60f32.to_radians(), 1440, 2.0).unwrap();
        let far = meters_per_pixel(60f32.to_radians(), 1440, 200.0).unwrap();
        assert!((far / near - 100.0).abs() < 1e-3);
        assert!(meters_per_pixel(0.0, 1440, 2.0).is_none());
    }
}
