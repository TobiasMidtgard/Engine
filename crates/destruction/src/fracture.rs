//! Voronoi fracture of convex shapes.
//!
//! A shape is split by the bisector planes between random seed points: each seed keeps the part
//! of the shape that is closer to it than to any other seed, which gives the irregular convex
//! pieces real stone, concrete and glass break into. Seeds cluster around the impact point, so
//! pieces are small where the hit landed and large far from it.

use engine_core::math::{Rng, Vec3};

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
    fn prism_is_closed() {
        let p = ConvexPoly::prism(0.5, 1.0, 12);
        let (v, _) = p.volume_centroid();
        let expected = 0.5 * 12.0 * 0.25 * (std::f32::consts::TAU / 12.0).sin() * 2.0;
        assert!((v - expected).abs() < 1e-3, "{v} vs {expected}");
    }
}
