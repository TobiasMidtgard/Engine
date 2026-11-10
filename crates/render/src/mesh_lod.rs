//! Levels of detail for meshes on the instanced path (those too small for virtual geometry:
//! props, wall stones, debris). Each dense mesh gets a chain of simplified index lists over its
//! own vertices, built once on a worker thread, and every view picks the coarsest level whose
//! error stays under a pixel (the camera) or a texel (shadow maps). Objects smaller than that
//! are left out of the view altogether.

use engine_core::math::{Vec2, Vec3, Vec4};

use crate::mesh::Mesh;
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
/// The indices of a mesh's levels together, as a share of its own, when every level reaches
/// [`STEP`] (0.35 + 0.35² + ...), rounded up: the room the mesh pool holds for them.
pub const USUAL_INDEX_SHARE: f32 = 0.6;

/// One level of a mesh: its triangles as indices into the mesh's vertices, and how far its
/// surface strays from the full mesh (meters in mesh space).
#[derive(Debug, Clone)]
pub struct LodIndices {
    pub indices: Vec<u32>,
    pub error: f32,
}

/// A corner leaving a seam may only move to a place that has a vertex whose texture
/// coordinate is where the corner's triangle expects it, within this share of how far the
/// corner moves in texture space: so a triangle never ends up spanning the seam.
const UV_SLACK: f32 = 0.5;

/// Two triangles that meet at an edge fold back onto each other past this (cosine between
/// their normals): the two sides of a leaf card, not a surface going on.
const FOLD_COS: f32 = -0.9;

/// The mesh's vertices by place. Generated meshes repeat a vertex wherever the normal, texture
/// coordinate or color changes (a hard edge, flat shading, a seam), and as indexed every such
/// edge looks like a border, which a simplifier must not move: such meshes got no levels at
/// all. So vertices that sit at one position count as one place where the surface goes on
/// across them (an edge there has exactly one triangle on either side), the levels are
/// simplified over the places, and every corner of every triangle remembers which of its
/// place's vertices it is drawn with. Surfaces that only touch, or lie back to back, stay
/// apart as they are indexed.
struct Weld {
    /// The first vertex of each vertex's place.
    place: Vec<u32>,
    /// The next vertex of the same place (a ring through them).
    next: Vec<u32>,
    /// By place: its vertices differ in texture coordinate (a seam runs through it).
    seam: Vec<bool>,
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
        let mut seam = vec![false; n];
        for (i, &p) in place.iter().enumerate() {
            if p != i as u32 {
                next[i] = next[p as usize];
                next[p as usize] = i as u32;
                seam[p as usize] |= mesh.vertices[p as usize].uv != mesh.vertices[i].uv;
            }
        }
        Self { place, next, seam }
    }

    /// The vertex at place `at` for a corner drawn with `was` so far, in a triangle whose
    /// other two corners are drawn with `b` and `c`: of the vertices there whose texture
    /// coordinate is nearest to where the triangle's own mapping puts that place, the one
    /// closest to `was` in normal and color (the same side of a hard edge, the same face's
    /// tint). `None` for a corner leaving a seam for a place that has no vertex on the
    /// corner's side of it: it must not move there, or its triangle would span the seam.
    fn vertex(&self, mesh: &Mesh, at: u32, was: u32, b: u32, c: u32) -> Option<u32> {
        let v = |i: u32| &mesh.vertices[i as usize];
        let (p0, p1, p2) = (
            Vec3::from(v(was).position),
            Vec3::from(v(b).position),
            Vec3::from(v(c).position),
        );
        let uv = |i: u32| Vec2::from(v(i).uv);
        let (t0, t1, t2) = (uv(was), uv(b), uv(c));
        // The triangle's texture mapping, carried on to the new place.
        let (e1, e2, d) = (p1 - p0, p2 - p0, Vec3::from(v(at).position) - p0);
        let (d00, d01, d11) = (e1.dot(e1), e1.dot(e2), e2.dot(e2));
        let det = d00 * d11 - d01 * d01;
        let expected = if det > 1e-12 * d00 * d11 {
            let (r0, r1) = (d.dot(e1), d.dot(e2));
            let (a, b) = ((d11 * r0 - d01 * r1) / det, (d00 * r1 - d01 * r0) / det);
            t0 + (t1 - t0) * a + (t2 - t0) * b
        } else {
            t0
        };
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
        let mut nearest = f32::INFINITY;
        ring(&mut |i| nearest = nearest.min((uv(i) - expected).length_squared()));
        // (Off by a share of how far the corner travels in the texture: a move along the
        // seam lands on its vertex there, a move across it misses by the texture's width.)
        if self.seam[self.place[was as usize] as usize]
            && nearest > (expected - t0).length_squared() * UV_SLACK * UV_SLACK + 1e-10
        {
            return None;
        }
        let (normal, color) = (Vec3::from(v(was).normal), Vec4::from(v(was).color));
        let mut best: Option<(f32, u32)> = None;
        ring(&mut |i| {
            if (uv(i) - expected).length_squared() <= nearest * 1.001 + 1e-10 {
                let off = 1.0 - normal.dot(Vec3::from(v(i).normal))
                    + (Vec4::from(v(i).color) - color).length_squared();
                if best.is_none_or(|(o, _)| off < o) {
                    best = Some((off, i));
                }
            }
        });
        best.map(|(_, i)| i)
    }
}

/// The levels of `mesh`, finest first (empty for a mesh too small or too ragged to simplify).
/// `ENGINE_MESH_LOD_WELD=0` in the environment builds them as before 2026-11 (over the
/// vertices as indexed, borders fixed), to compare.
pub fn build(mesh: &Mesh) -> Vec<LodIndices> {
    static WELD: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    let weld = *WELD.get_or_init(|| std::env::var("ENGINE_MESH_LOD_WELD").is_ok_and(|v| v == "0"));
    build_as(mesh, !weld)
}

fn build_as(mesh: &Mesh, welded: bool) -> Vec<LodIndices> {
    let corners = mesh.indices.as_chunks::<3>().0;
    if corners.len() < MIN_TRIANGLES {
        return Vec::new();
    }
    if !welded {
        return build_as_indexed(mesh);
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
    let mut staged: Vec<u32> = Vec::new();
    while levels.len() < MAX_LEVELS && current.len() > FLOOR_TRIANGLES * 2 {
        let target = ((current.len() as f32 * STEP) as usize).max(FLOOR_TRIANGLES);
        // A collapse happens only if every corner it moves finds a vertex to be drawn with
        // at its new place.
        let mut moved = |moves: &[(u32, u8)], to: u32| {
            staged.clear();
            for &(t, k) in moves {
                let d = drawn[t as usize];
                let k = k as usize;
                match weld.vertex(mesh, to, d[k], d[(k + 1) % 3], d[(k + 2) % 3]) {
                    Some(v) => staged.push(v),
                    None => return false,
                }
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
            },
        );
        if out.triangles.is_empty() || out.triangles.len() as f32 > current.len() as f32 * MIN_GAIN
        {
            break;
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
    levels
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

    #[test]
    fn a_dense_stone_gets_a_chain_of_ever_coarser_levels() {
        let mesh = rock(Vec3::new(0.4, 0.25, 0.3), 7, 5);
        let full = mesh.indices.len() / 3;
        let levels = build(&mesh);
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
        let levels = build(&mesh);
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
        assert!(build_as(&mesh, false).is_empty());
        assert!(!build_as(&rock(Vec3::new(0.4, 0.25, 0.3), 7, 5), false).is_empty());
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
        let levels = build(&mesh);
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
        let levels = build(&mesh);
        assert!(!levels.is_empty(), "no levels");
        let last = levels.last().unwrap();
        assert!(
            last.indices.len() / 3 <= full / 2,
            "{full} down to {}",
            last.indices.len() / 3
        );
        for l in &levels {
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
        let levels = build(&mesh);
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
        for l in build(&mesh) {
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
        assert!(build(&mesh).is_empty());
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
