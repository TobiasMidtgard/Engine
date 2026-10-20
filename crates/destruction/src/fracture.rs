//! Voronoi fracture of convex shapes.
//!
//! A shape is split by the bisector planes between random seed points: each seed keeps the part
//! of the shape that is closer to it than to any other seed, which gives the irregular convex
//! pieces real stone, concrete and glass break into. Seeds cluster around the impact point, so
//! pieces are small where the hit landed and large far from it.

use std::collections::HashMap;

use engine_core::math::noise::fbm3;
use engine_core::math::{Rng, Vec3};
use engine_render::Vertex;

use crate::material::DestructibleMaterial;
use crate::meshing::{MeshBuilder, mix, scale};

const EPS: f32 = 1e-5;

/// One flat face of a [`ConvexPoly`]: points counter-clockwise around `normal`.
#[derive(Debug, Clone, PartialEq)]
pub struct Face {
    pub points: Vec<Vec3>,
    pub normal: Vec3,
    /// A fracture surface (shows the broken interior color) rather than the original outside.
    pub interior: bool,
}

/// A convex polyhedron as a list of faces (local space, meters).
#[derive(Debug, Clone, PartialEq, Default)]
pub struct ConvexPoly {
    pub faces: Vec<Face>,
}

impl ConvexPoly {
    /// Box centered on the origin.
    pub fn cuboid(half: Vec3) -> Self {
        let h = half;
        let c = |x: f32, y: f32, z: f32| Vec3::new(x * h.x, y * h.y, z * h.z);
        let face = |n: Vec3, pts: [Vec3; 4]| Face {
            points: pts.to_vec(),
            normal: n,
            interior: false,
        };
        Self {
            faces: vec![
                face(
                    Vec3::X,
                    [
                        c(1., -1., 1.),
                        c(1., -1., -1.),
                        c(1., 1., -1.),
                        c(1., 1., 1.),
                    ],
                ),
                face(
                    Vec3::NEG_X,
                    [
                        c(-1., -1., -1.),
                        c(-1., -1., 1.),
                        c(-1., 1., 1.),
                        c(-1., 1., -1.),
                    ],
                ),
                face(
                    Vec3::Y,
                    [
                        c(-1., 1., 1.),
                        c(1., 1., 1.),
                        c(1., 1., -1.),
                        c(-1., 1., -1.),
                    ],
                ),
                face(
                    Vec3::NEG_Y,
                    [
                        c(-1., -1., -1.),
                        c(1., -1., -1.),
                        c(1., -1., 1.),
                        c(-1., -1., 1.),
                    ],
                ),
                face(
                    Vec3::Z,
                    [
                        c(-1., -1., 1.),
                        c(1., -1., 1.),
                        c(1., 1., 1.),
                        c(-1., 1., 1.),
                    ],
                ),
                face(
                    Vec3::NEG_Z,
                    [
                        c(1., -1., -1.),
                        c(-1., -1., -1.),
                        c(-1., 1., -1.),
                        c(1., 1., -1.),
                    ],
                ),
            ],
        }
    }

    /// Upright prism (a column) with `segments` sides, centered on the origin.
    pub fn prism(radius: f32, half_height: f32, segments: u32) -> Self {
        let n = segments.max(3);
        let ring: Vec<(f32, f32)> = (0..n)
            .map(|i| {
                let a = i as f32 / n as f32 * std::f32::consts::TAU;
                (a.cos() * radius, a.sin() * radius)
            })
            .collect();
        let mut faces = Vec::new();
        for i in 0..n as usize {
            let (x0, z0) = ring[i];
            let (x1, z1) = ring[(i + 1) % n as usize];
            let mid = Vec3::new(x0 + x1, 0.0, z0 + z1).normalize();
            let pts = vec![
                Vec3::new(x0, -half_height, z0),
                Vec3::new(x0, half_height, z0),
                Vec3::new(x1, half_height, z1),
                Vec3::new(x1, -half_height, z1),
            ];
            faces.push(oriented(pts, mid, false));
        }
        let top: Vec<Vec3> = ring
            .iter()
            .map(|&(x, z)| Vec3::new(x, half_height, z))
            .collect();
        let bottom: Vec<Vec3> = ring
            .iter()
            .map(|&(x, z)| Vec3::new(x, -half_height, z))
            .collect();
        faces.push(oriented(top, Vec3::Y, false));
        faces.push(oriented(bottom, Vec3::NEG_Y, false));
        Self { faces }
    }

    /// Every distinct corner.
    pub fn vertices(&self) -> Vec<Vec3> {
        let mut out: Vec<Vec3> = Vec::new();
        for f in &self.faces {
            for &p in &f.points {
                if !out.iter().any(|q| q.distance_squared(p) < 1e-10) {
                    out.push(p);
                }
            }
        }
        out
    }

    /// Stretched by `s` along each axis (all components positive).
    pub fn scaled(&self, s: Vec3) -> ConvexPoly {
        ConvexPoly {
            faces: self
                .faces
                .iter()
                .map(|f| Face {
                    points: f.points.iter().map(|&p| p * s).collect(),
                    normal: (f.normal / s).normalize_or(f.normal),
                    interior: f.interior,
                })
                .collect(),
        }
    }

    /// Axis-aligned bounds `(min, max)`.
    pub fn bounds(&self) -> (Vec3, Vec3) {
        let mut lo = Vec3::splat(f32::INFINITY);
        let mut hi = Vec3::splat(f32::NEG_INFINITY);
        for f in &self.faces {
            for &p in &f.points {
                lo = lo.min(p);
                hi = hi.max(p);
            }
        }
        (lo, hi)
    }

    /// Volume (m³) and center of mass.
    pub fn volume_centroid(&self) -> (f32, Vec3) {
        let mut vol = 0.0;
        let mut c = Vec3::ZERO;
        for f in &self.faces {
            for i in 1..f.points.len().saturating_sub(1) {
                let (a, b, d) = (f.points[0], f.points[i], f.points[i + 1]);
                let v = a.dot(b.cross(d)) / 6.0;
                vol += v;
                c += (a + b + d) * (v / 4.0);
            }
        }
        if vol.abs() < 1e-12 {
            let pts = self.vertices();
            let n = pts.len().max(1) as f32;
            return (0.0, pts.into_iter().sum::<Vec3>() / n);
        }
        (vol, c / vol)
    }

    /// Whether `p` is inside (or on) every face.
    pub fn contains(&self, p: Vec3) -> bool {
        self.faces
            .iter()
            .all(|f| f.normal.dot(p - f.points[0]) <= 1e-4)
    }

    /// Keep the part where `normal · p <= offset`. The cut face is marked interior.
    pub fn clip(&self, normal: Vec3, offset: f32) -> Option<ConvexPoly> {
        let dist = |p: Vec3| normal.dot(p) - offset;
        let mut all_inside = true;
        let mut any_inside = false;
        for f in &self.faces {
            for &p in &f.points {
                let d = dist(p);
                if d > EPS {
                    all_inside = false;
                } else {
                    any_inside = true;
                }
            }
        }
        if all_inside {
            return Some(self.clone());
        }
        if !any_inside {
            return None;
        }
        let mut faces = Vec::with_capacity(self.faces.len() + 1);
        let mut cut: Vec<Vec3> = Vec::new();
        for f in &self.faces {
            let mut out = Vec::with_capacity(f.points.len() + 2);
            let n = f.points.len();
            for i in 0..n {
                let a = f.points[i];
                let b = f.points[(i + 1) % n];
                let (da, db) = (dist(a), dist(b));
                let a_in = da <= EPS;
                let b_in = db <= EPS;
                if a_in {
                    out.push(a);
                    if da.abs() <= EPS {
                        cut.push(a);
                    }
                }
                if a_in != b_in && (da - db).abs() > 1e-12 {
                    let t = da / (da - db);
                    let p = a + (b - a) * t;
                    out.push(p);
                    cut.push(p);
                }
            }
            dedup_ring(&mut out);
            if out.len() >= 3 {
                faces.push(Face {
                    points: out,
                    normal: f.normal,
                    interior: f.interior,
                });
            }
        }
        let mut cap: Vec<Vec3> = Vec::new();
        for p in cut {
            if !cap.iter().any(|q| q.distance_squared(p) < 1e-10) {
                cap.push(p);
            }
        }
        if cap.len() >= 3 {
            faces.push(oriented(cap, normal, true));
        }
        if faces.len() < 4 {
            return None;
        }
        Some(ConvexPoly { faces })
    }

    /// Mesh with flat faces: outside faces in `exterior`, fracture faces in `interior`.
    /// Vertices are offset by `-center`.
    pub fn to_mesh(&self, center: Vec3, exterior: [f32; 3], interior: [f32; 3]) -> MeshBuilder {
        let mut b = MeshBuilder::new();
        for (k, f) in self.faces.iter().enumerate() {
            let shade =
                0.94 + 0.12 * (((k as u32).wrapping_mul(2654435761) >> 16) & 0xff) as f32 / 255.0;
            let color = if f.interior {
                scale(interior, shade)
            } else {
                scale(mix(exterior, interior, 0.03), shade)
            };
            let pts: Vec<Vec3> = f.points.iter().map(|&p| p - center).collect();
            b.polygon(&pts, f.normal, color);
        }
        b
    }
}

/// How a broken piece is drawn: its outside faces flat, its fracture faces rough.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BreakLook {
    /// Vertex color of the original outside.
    pub exterior: [f32; 3],
    /// Vertex color of the fracture faces.
    pub interior: [f32; 3],
    /// Height of the bumps on fracture faces, as a fraction of the piece's size (0 = flat,
    /// like glass; ~0.05 for stone).
    pub roughness: f32,
    /// Draw the outside with the fracture faces' material too (a monolithic stone block, whose
    /// usual surface texture has mortar joints that don't belong on one block).
    pub monolithic: bool,
}

impl BreakLook {
    /// How `material` breaks: stone, concrete and rock into rough faces, wood into nearly flat
    /// ones (its splinters are drawn separately), glass and metal clean. A stone piece is one
    /// dressed block, drawn all in its joint-free broken-stone material.
    pub fn of(material: DestructibleMaterial, exterior: [f32; 3], interior: [f32; 3]) -> Self {
        use DestructibleMaterial as M;
        let roughness = match material {
            M::Rock => 0.06,
            M::Stone | M::Concrete | M::ReinforcedConcrete | M::Dirt => 0.05,
            M::Brick => 0.045,
            M::Plaster | M::Ice => 0.03,
            M::Wood => 0.012,
            M::Glass | M::Metal => 0.0,
        };
        Self {
            exterior,
            interior,
            roughness,
            monolithic: material == M::Stone,
        }
    }
}

impl ConvexPoly {
    /// Meshes for a broken piece, with vertices offset by `-center`: the outside faces (flat;
    /// `None` when there are none, or when `look.monolithic` moves them into the second
    /// mesh), and the fracture faces broken up into small triangles pushed in and out by
    /// noise, tapering to nothing at their edges so neighboring faces still meet. Fracture
    /// faces get planar UVs (meters), so a texture runs across the bumps without seams.
    pub fn broken_meshes(
        &self,
        center: Vec3,
        look: &BreakLook,
        seed: u64,
    ) -> (Option<MeshBuilder>, MeshBuilder) {
        let (lo, hi) = self.bounds();
        let size = (hi - lo).max_element().max(0.01);
        let thin = (hi - lo).min_element().max(0.005);
        let rough = (look.roughness * size).min(thin * 0.15);
        let mut outside = MeshBuilder::new();
        let mut inside = MeshBuilder::new();
        for (k, f) in self.faces.iter().enumerate() {
            let shade =
                0.94 + 0.12 * (((k as u32).wrapping_mul(2654435761) >> 16) & 0xff) as f32 / 255.0;
            let pts: Vec<Vec3> = f.points.iter().map(|&p| p - center).collect();
            if !f.interior {
                let color = scale(mix(look.exterior, look.interior, 0.03), shade);
                if look.monolithic {
                    planar_polygon(&mut inside, &pts, f.normal, color);
                } else {
                    outside.polygon(&pts, f.normal, color);
                }
                continue;
            }
            let color = scale(look.interior, shade);
            if rough <= 1e-4 {
                planar_polygon(&mut inside, &pts, f.normal, color);
            } else {
                rough_face(&mut inside, &pts, f.normal, color, rough, size, seed);
            }
        }
        let outside = (!outside.is_empty()).then_some(outside);
        (outside, inside)
    }
}

/// A flat face with planar UVs (meters) along the face's own axes.
fn planar_polygon(b: &mut MeshBuilder, pts: &[Vec3], normal: Vec3, color: [f32; 3]) {
    if pts.len() < 3 {
        return;
    }
    let (t, bt) = face_axes(normal);
    let base = b.vertices.len() as u32;
    for &p in pts {
        b.vertices.push(Vertex {
            position: p.to_array(),
            normal: normal.to_array(),
            uv: [p.dot(t), -p.dot(bt)],
            color: [color[0], color[1], color[2], 1.0],
        });
    }
    for i in 1..pts.len() as u32 - 1 {
        b.triangle(base, base + i, base + i + 1);
    }
}

/// Tangent axes of a face, chosen the same way for every face with that normal.
fn face_axes(n: Vec3) -> (Vec3, Vec3) {
    let up = if n.y.abs() < 0.9 { Vec3::Y } else { Vec3::Z };
    let t = up.cross(n).normalize_or(n.any_orthonormal_vector());
    (t, n.cross(t))
}

/// A fracture face as rough broken stone (see [`ConvexPoly::broken_meshes`]).
fn rough_face(
    b: &mut MeshBuilder,
    pts: &[Vec3],
    normal: Vec3,
    color: [f32; 3],
    rough: f32,
    size: f32,
    seed: u64,
) {
    let n = pts.len();
    if n < 3 {
        return;
    }
    let c = pts.iter().copied().sum::<Vec3>() / n as f32;
    let longest = (0..n)
        .map(|i| {
            (pts[(i + 1) % n] - pts[i])
                .length()
                .max((pts[i] - c).length())
        })
        .fold(0.0f32, f32::max);
    // About 5 cm triangles, at most 8 segments a side.
    let segs = ((longest / 0.05).ceil() as usize).clamp(2, 8);
    let band = (rough * 3.0).max(1e-3);
    // Distance from a point on the face to its edge (the face is convex).
    let edge_dist = |p: Vec3| {
        (0..n)
            .map(|i| {
                let (a, e) = (pts[i], pts[(i + 1) % n]);
                let ab = e - a;
                let t = ((p - a).dot(ab) / ab.length_squared().max(1e-12)).clamp(0.0, 1.0);
                (p - (a + ab * t)).length()
            })
            .fold(f32::MAX, f32::min)
    };
    let freq = 1.0 / (0.3 * size).max(0.05);
    let bump = |p: Vec3| {
        let q = p.as_dvec3() * freq as f64;
        let h = fbm3(q.x, q.y, q.z, seed, 3, 2.0, 0.5);
        let taper = (edge_dist(p) / band).min(1.0);
        p + normal * (rough * h * taper * (3.0 - 2.0 * taper) * taper)
    };
    // Fan of triangles from the centroid, each split into a grid of `segs` per side; shared
    // points are welded by position so the bumps shade smoothly.
    let mut index: HashMap<[i32; 3], u32> = HashMap::new();
    let mut verts: Vec<Vec3> = Vec::new();
    let mut flat: Vec<Vec3> = Vec::new();
    let mut tris: Vec<[u32; 3]> = Vec::new();
    let mut id = |p: Vec3, verts: &mut Vec<Vec3>, flat: &mut Vec<Vec3>| {
        let key = [
            (p.x * 1e4).round() as i32,
            (p.y * 1e4).round() as i32,
            (p.z * 1e4).round() as i32,
        ];
        *index.entry(key).or_insert_with(|| {
            verts.push(bump(p));
            flat.push(p);
            (verts.len() - 1) as u32
        })
    };
    for i in 0..n {
        let (a, e) = (pts[i], pts[(i + 1) % n]);
        let at = |u: usize, v: usize| {
            let (fu, fv) = (u as f32 / segs as f32, v as f32 / segs as f32);
            c + (a - c) * fu + (e - c) * fv
        };
        for u in 0..segs {
            for v in 0..segs - u {
                let p0 = id(at(u, v), &mut verts, &mut flat);
                let p1 = id(at(u + 1, v), &mut verts, &mut flat);
                let p2 = id(at(u, v + 1), &mut verts, &mut flat);
                tris.push([p0, p1, p2]);
                if u + v + 1 < segs {
                    let p3 = id(at(u + 1, v + 1), &mut verts, &mut flat);
                    tris.push([p1, p3, p2]);
                }
            }
        }
    }
    let mut normals = vec![Vec3::ZERO; verts.len()];
    for t in &mut tris {
        let [i, j, k] = *t;
        let mut w =
            (verts[j as usize] - verts[i as usize]).cross(verts[k as usize] - verts[i as usize]);
        if w.dot(normal) < 0.0 {
            t.swap(1, 2);
            w = -w;
        }
        for v in *t {
            normals[v as usize] += w;
        }
    }
    let (tx, ty) = face_axes(normal);
    let base = b.vertices.len() as u32;
    for (k, p) in verts.iter().enumerate() {
        let nn = normals[k].normalize_or(normal);
        let uvp = flat[k];
        // Darker in the hollows, a touch lighter on the knobs.
        let depth = (*p - uvp).dot(normal) / rough.max(1e-4);
        let tone = 1.0 + 0.08 * depth.clamp(-1.0, 1.0);
        b.vertices.push(Vertex {
            position: p.to_array(),
            normal: nn.to_array(),
            uv: [uvp.dot(tx), -uvp.dot(ty)],
            color: [color[0] * tone, color[1] * tone, color[2] * tone, 1.0],
        });
    }
    for t in tris {
        b.triangle(base + t[0], base + t[1], base + t[2]);
    }
}

/// Order `points` (all on one plane) counter-clockwise around `normal`.
fn oriented(mut points: Vec<Vec3>, normal: Vec3, interior: bool) -> Face {
    let n = points.len() as f32;
    let c = points.iter().copied().sum::<Vec3>() / n;
    let u = (points[0] - c).normalize_or(normal.any_orthonormal_vector());
    let v = normal.cross(u);
    points.sort_by(|a, b| {
        let pa = *a - c;
        let pb = *b - c;
        let aa = pa.dot(v).atan2(pa.dot(u));
        let ab = pb.dot(v).atan2(pb.dot(u));
        aa.total_cmp(&ab)
    });
    Face {
        points,
        normal,
        interior,
    }
}

fn dedup_ring(points: &mut Vec<Vec3>) {
    points.dedup_by(|a, b| a.distance_squared(*b) < 1e-12);
    while points.len() > 1 && points[0].distance_squared(points[points.len() - 1]) < 1e-12 {
        points.pop();
    }
}

/// Split `shape` into one convex piece per seed (pieces with no volume are dropped).
pub fn voronoi(shape: &ConvexPoly, seeds: &[Vec3]) -> Vec<ConvexPoly> {
    let mut pieces = Vec::with_capacity(seeds.len());
    let mut order: Vec<usize> = Vec::with_capacity(seeds.len());
    for (i, &si) in seeds.iter().enumerate() {
        // Nearest neighbors first: they cut the most, so later planes are often no-ops.
        order.clear();
        order.extend((0..seeds.len()).filter(|&j| j != i));
        order.sort_by(|&a, &b| {
            seeds[a]
                .distance_squared(si)
                .total_cmp(&seeds[b].distance_squared(si))
        });
        let mut cell = Some(shape.clone());
        for &j in &order {
            let sj = seeds[j];
            let d = sj - si;
            if d.length_squared() < 1e-10 {
                continue;
            }
            let n = d.normalize();
            let offset = n.dot((si + sj) * 0.5);
            cell = cell.and_then(|c| c.clip(n, offset));
            if cell.is_none() {
                break;
            }
        }
        if let Some(c) = cell {
            let (vol, _) = c.volume_centroid();
            if vol > 1e-7 {
                pieces.push(c);
            }
        }
    }
    pieces
}

/// Seed points inside `shape`: about half near `impact` (within `spread` meters), the rest
/// spread through the whole shape.
pub fn impact_seeds(
    shape: &ConvexPoly,
    impact: Vec3,
    count: usize,
    spread: f32,
    rng: &mut Rng,
) -> Vec<Vec3> {
    let (lo, hi) = shape.bounds();
    let mut seeds = Vec::with_capacity(count);
    let mut tries = 0;
    while seeds.len() < count && tries < count * 40 {
        tries += 1;
        let near = seeds.len() % 2 == 0;
        let p = if near {
            impact + rng.unit_vector() * spread * rng.f32().powf(0.7)
        } else {
            Vec3::new(
                rng.range(lo.x, hi.x),
                rng.range(lo.y, hi.y),
                rng.range(lo.z, hi.z),
            )
        };
        if shape.contains(p) {
            seeds.push(p);
        }
    }
    if seeds.len() < 2 {
        let (_, c) = shape.volume_centroid();
        seeds = vec![c + (hi - lo) * 0.2, c - (hi - lo) * 0.2];
    }
    seeds
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cuboid_volume_and_center() {
        let b = ConvexPoly::cuboid(Vec3::new(1.0, 0.5, 2.0));
        let (v, c) = b.volume_centroid();
        assert!((v - 8.0).abs() < 1e-4, "{v}");
        assert!(c.length() < 1e-4);
        assert_eq!(b.vertices().len(), 8);
        // Faces point outward.
        for f in &b.faces {
            let n = (f.points[1] - f.points[0]).cross(f.points[2] - f.points[0]);
            assert!(n.dot(f.normal) > 0.0, "face {:?} winds inward", f.normal);
        }
    }

    #[test]
    fn clipping_halves_a_box() {
        let b = ConvexPoly::cuboid(Vec3::ONE);
        let half = b.clip(Vec3::X, 0.0).unwrap();
        let (v, c) = half.volume_centroid();
        assert!((v - 4.0).abs() < 1e-4, "{v}");
        assert!((c.x + 0.5).abs() < 1e-4, "{c}");
        assert!(half.faces.iter().any(|f| f.interior));
        assert!(b.clip(Vec3::X, -2.0).is_none());
        assert_eq!(b.clip(Vec3::X, 2.0).unwrap().faces.len(), 6);
    }

    #[test]
    fn voronoi_pieces_fill_the_shape() {
        let b = ConvexPoly::cuboid(Vec3::new(1.0, 2.0, 0.5));
        let mut rng = Rng::new(7);
        let seeds = impact_seeds(&b, Vec3::new(0.8, 1.5, 0.4), 12, 0.6, &mut rng);
        let pieces = voronoi(&b, &seeds);
        assert!(pieces.len() >= 8, "{} pieces", pieces.len());
        let total: f32 = pieces.iter().map(|p| p.volume_centroid().0).sum();
        assert!(
            (total - 8.0).abs() < 1e-2,
            "pieces add up to {total} m³, not 8"
        );
        for p in &pieces {
            let m = p.to_mesh(p.volume_centroid().1, [1.0; 3], [0.5; 3]);
            assert!(!m.is_empty());
        }
    }

    #[test]
    fn broken_pieces_have_rough_fracture_faces_that_meet_at_the_edges() {
        let b = ConvexPoly::cuboid(Vec3::new(0.4, 0.3, 0.25));
        let mut rng = Rng::new(3);
        let seeds = impact_seeds(&b, Vec3::new(0.3, 0.2, 0.2), 6, 0.6, &mut rng);
        let look = BreakLook {
            exterior: [1.0; 3],
            interior: [0.8; 3],
            roughness: 0.05,
            monolithic: false,
        };
        for p in voronoi(&b, &seeds) {
            let c = p.volume_centroid().1;
            let (outside, inside) = p.broken_meshes(c, &look, 9);
            let flat = p.to_mesh(c, [1.0; 3], [0.8; 3]);
            let interior = p.faces.iter().filter(|f| f.interior).count();
            if interior > 0 {
                // Many more triangles than the flat faces, and bumps off the planes.
                assert!(inside.indices.len() > 4 * flat.indices.len() / p.faces.len().max(1));
                let off = p.faces.iter().filter(|f| f.interior).any(|f| {
                    let d0 = (f.points[0] - c).dot(f.normal);
                    inside.vertices.iter().any(|v| {
                        let q = Vec3::from(v.position);
                        (q.dot(f.normal) - d0).abs() > 1e-3 && (q.dot(f.normal) - d0).abs() < 0.05
                    })
                });
                assert!(off, "fracture faces stayed flat");
            }
            // Every corner of the piece is still a vertex of one of the meshes: edges meet.
            for v in p.vertices() {
                let q = v - c;
                let found = inside
                    .vertices
                    .iter()
                    .chain(outside.iter().flat_map(|o| o.vertices.iter()))
                    .any(|w| Vec3::from(w.position).distance(q) < 1e-4);
                assert!(found, "corner {q} lost");
            }
            // Monolithic: everything in the second mesh.
            let mono = BreakLook {
                monolithic: true,
                ..look
            };
            let (o, i) = p.broken_meshes(c, &mono, 9);
            assert!(o.is_none() && !i.is_empty());
        }
    }

    #[test]
    fn prism_is_closed() {
        let p = ConvexPoly::prism(0.5, 1.0, 12);
        let (v, _) = p.volume_centroid();
        let expected = 0.5 * 12.0 * 0.25 * (std::f32::consts::TAU / 12.0).sin() * 2.0;
        assert!((v - expected).abs() < 1e-3, "{v} vs {expected}");
    }
}
