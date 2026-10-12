//! Signed distance sculpting: build a shape from primitives blended with smooth unions, then
//! turn it into a triangle mesh with [`polygonize`] (surface nets, vertices projected onto the
//! surface, normals from the field's gradient, box-projected UVs in meters and a little
//! ambient occlusion baked into the vertex color).
//!
//! Distances are in meters; negative inside.

use engine_core::prelude::*;
use engine_render::{Mesh, Vertex};

pub fn sphere(p: Vec3, c: Vec3, r: f32) -> f32 {
    p.distance(c) - r
}

/// Ellipsoid with semi-axes `r` (a good, slightly inexact bound).
pub fn ellipsoid(p: Vec3, c: Vec3, r: Vec3) -> f32 {
    let q = (p - c) / r;
    let k0 = q.length();
    let k1 = ((p - c) / (r * r)).length();
    if k1 < 1e-9 {
        -r.min_element()
    } else {
        k0 * (k0 - 1.0) / k1
    }
}

/// Capsule from `a` to `b` with radius `r`.
pub fn capsule(p: Vec3, a: Vec3, b: Vec3, r: f32) -> f32 {
    let (pa, ba) = (p - a, b - a);
    let h = (pa.dot(ba) / ba.length_squared().max(1e-12)).clamp(0.0, 1.0);
    (pa - ba * h).length() - r
}

/// A capsule whose radius goes from `ra` at `a` to `rb` at `b` (limbs, horns, fingers).
pub fn round_cone(p: Vec3, a: Vec3, b: Vec3, ra: f32, rb: f32) -> f32 {
    let ba = b - a;
    let l2 = ba.length_squared().max(1e-12);
    let rr = ra - rb;
    let a2 = l2 - rr * rr;
    let il2 = 1.0 / l2;
    let pa = p - a;
    let y = pa.dot(ba);
    let z = y - l2;
    let x2 = (pa * l2 - ba * y).length_squared();
    let y2 = y * y * l2;
    let z2 = z * z * l2;
    let k = rr.signum() * rr * rr * x2;
    if z.signum() * a2 * z2 > k {
        return (x2 + z2).sqrt() * il2 - rb;
    }
    if y.signum() * a2 * y2 < k {
        return (x2 + y2).sqrt() * il2 - ra;
    }
    ((x2 * a2 * il2).sqrt() + y * rr) * il2 - ra
}

/// Box with half extents `half` and rounded edges of radius `r`, centered at `c`, rotated by
/// `rot`.
pub fn round_box(p: Vec3, c: Vec3, rot: Quat, half: Vec3, r: f32) -> f32 {
    let q = (rot.inverse() * (p - c)).abs() - (half - Vec3::splat(r));
    q.max(Vec3::ZERO).length() + q.max_element().min(0.0) - r
}

/// Torus around the local Y axis.
pub fn torus(p: Vec3, c: Vec3, rot: Quat, major: f32, minor: f32) -> f32 {
    let q = rot.inverse() * (p - c);
    Vec2::new(Vec2::new(q.x, q.z).length() - major, q.y).length() - minor
}

/// Cylinder along local Y with half height `h`, radius `r` and edge rounding `e`.
pub fn cylinder(p: Vec3, c: Vec3, rot: Quat, r: f32, h: f32, e: f32) -> f32 {
    let q = rot.inverse() * (p - c);
    let d = Vec2::new(Vec2::new(q.x, q.z).length() - r + e, q.y.abs() - h + e);
    d.max(Vec2::ZERO).length() + d.x.max(d.y).min(0.0) - e
}

/// Smooth union: blends within `k` meters.
pub fn smin(a: f32, b: f32, k: f32) -> f32 {
    if k <= 0.0 {
        return a.min(b);
    }
    let h = (0.5 + 0.5 * (b - a) / k).clamp(0.0, 1.0);
    b + (a - b) * h - k * h * (1.0 - h)
}

/// Smooth intersection.
pub fn smax(a: f32, b: f32, k: f32) -> f32 {
    -smin(-a, -b, k)
}

/// Smooth subtraction: carve `b` out of `a`.
pub fn ssub(a: f32, b: f32, k: f32) -> f32 {
    smax(a, -b, k)
}

/// Mirror across the YZ plane (model one side of a symmetric body).
pub fn mirror_x(p: Vec3) -> Vec3 {
    Vec3::new(p.x.abs(), p.y, p.z)
}

/// Options for [`polygonize`].
#[derive(Debug, Clone, Copy)]
pub struct Grid {
    pub min: Vec3,
    pub max: Vec3,
    /// Voxel size, meters. Detail finer than about twice this is lost.
    pub voxel: f32,
    /// Bake ambient occlusion into vertex colors (crevices darken).
    pub occlusion: bool,
}

impl Grid {
    pub fn new(min: Vec3, max: Vec3, voxel: f32) -> Self {
        Self {
            min,
            max,
            voxel,
            occlusion: true,
        }
    }
}

/// Mesh the zero surface of `f` inside the grid (surface nets).
pub fn polygonize(f: &dyn Fn(Vec3) -> f32, grid: Grid) -> Mesh {
    let v = grid.voxel.max(1e-4);
    let size = ((grid.max - grid.min) / v).ceil();
    let (nx, ny, nz) = (
        size.x as usize + 2,
        size.y as usize + 2,
        size.z as usize + 2,
    );
    let origin = grid.min - Vec3::splat(v * 0.5);
    let at = |x: usize, y: usize, z: usize| origin + Vec3::new(x as f32, y as f32, z as f32) * v;
    let idx = |x: usize, y: usize, z: usize| (z * ny + y) * nx + x;
    let mut field = vec![0.0f32; nx * ny * nz];
    for z in 0..nz {
        for y in 0..ny {
            for x in 0..nx {
                field[idx(x, y, z)] = f(at(x, y, z));
            }
        }
    }
    // One vertex per cell the surface passes through.
    let (cx, cy, cz) = (nx - 1, ny - 1, nz - 1);
    let cidx = |x: usize, y: usize, z: usize| (z * cy + y) * cx + x;
    let mut cell_vertex = vec![u32::MAX; cx * cy * cz];
    let mut positions: Vec<Vec3> = Vec::new();
    const CORNERS: [(usize, usize, usize); 8] = [
        (0, 0, 0),
        (1, 0, 0),
        (0, 1, 0),
        (1, 1, 0),
        (0, 0, 1),
        (1, 0, 1),
        (0, 1, 1),
        (1, 1, 1),
    ];
    const EDGES: [(usize, usize); 12] = [
        (0, 1),
        (2, 3),
        (4, 5),
        (6, 7),
        (0, 2),
        (1, 3),
        (4, 6),
        (5, 7),
        (0, 4),
        (1, 5),
        (2, 6),
        (3, 7),
    ];
    for z in 0..cz {
        for y in 0..cy {
            for x in 0..cx {
                let d: [f32; 8] = CORNERS.map(|(a, b, c)| field[idx(x + a, y + b, z + c)]);
                let inside = d.iter().filter(|&&s| s < 0.0).count();
                if inside == 0 || inside == 8 {
                    continue;
                }
                let mut sum = Vec3::ZERO;
                let mut n = 0.0;
                for (a, b) in EDGES {
                    if (d[a] < 0.0) != (d[b] < 0.0) {
                        let t = d[a] / (d[a] - d[b]);
                        let pa = Vec3::new(
                            CORNERS[a].0 as f32,
                            CORNERS[a].1 as f32,
                            CORNERS[a].2 as f32,
                        );
                        let pb = Vec3::new(
                            CORNERS[b].0 as f32,
                            CORNERS[b].1 as f32,
                            CORNERS[b].2 as f32,
                        );
                        sum += pa + (pb - pa) * t;
                        n += 1.0;
                    }
                }
                cell_vertex[cidx(x, y, z)] = positions.len() as u32;
                positions.push(at(x, y, z) + sum / n * v);
            }
        }
    }
    // Quads across every grid edge with a sign change.
    let mut indices = Vec::new();
    // `out` is the direction the surface faces across the edge (from inside to outside).
    let mut quad = |a: u32, b: u32, c: u32, d: u32, out: Vec3| {
        if [a, b, c, d].contains(&u32::MAX) {
            return;
        }
        let [pa, pb, pc] = [a, b, c].map(|i| positions[i as usize]);
        if (pb - pa).cross(pc - pa).dot(out) >= 0.0 {
            indices.extend_from_slice(&[a, b, c, a, c, d]);
        } else {
            indices.extend_from_slice(&[a, c, b, a, d, c]);
        }
    };
    for z in 1..cz {
        for y in 1..cy {
            for x in 0..cx {
                // Edge along +X from (x, y, z).
                let (s0, s1) = (field[idx(x, y, z)], field[idx(x + 1, y, z)]);
                if (s0 < 0.0) != (s1 < 0.0) {
                    quad(
                        cell_vertex[cidx(x, y - 1, z - 1)],
                        cell_vertex[cidx(x, y, z - 1)],
                        cell_vertex[cidx(x, y, z)],
                        cell_vertex[cidx(x, y - 1, z)],
                        if s0 < 0.0 { Vec3::X } else { Vec3::NEG_X },
                    );
                }
            }
        }
    }
    for z in 1..cz {
        for y in 0..cy {
            for x in 1..cx {
                let (s0, s1) = (field[idx(x, y, z)], field[idx(x, y + 1, z)]);
                if (s0 < 0.0) != (s1 < 0.0) {
                    quad(
                        cell_vertex[cidx(x - 1, y, z - 1)],
                        cell_vertex[cidx(x - 1, y, z)],
                        cell_vertex[cidx(x, y, z)],
                        cell_vertex[cidx(x, y, z - 1)],
                        if s0 < 0.0 { Vec3::Y } else { Vec3::NEG_Y },
                    );
                }
            }
        }
    }
    for z in 0..cz {
        for y in 1..cy {
            for x in 1..cx {
                let (s0, s1) = (field[idx(x, y, z)], field[idx(x, y, z + 1)]);
                if (s0 < 0.0) != (s1 < 0.0) {
                    quad(
                        cell_vertex[cidx(x - 1, y - 1, z)],
                        cell_vertex[cidx(x, y - 1, z)],
                        cell_vertex[cidx(x, y, z)],
                        cell_vertex[cidx(x - 1, y, z)],
                        if s0 < 0.0 { Vec3::Z } else { Vec3::NEG_Z },
                    );
                }
            }
        }
    }
    let e = v * 0.25;
    let gradient = |p: Vec3| {
        Vec3::new(
            f(p + Vec3::X * e) - f(p - Vec3::X * e),
            f(p + Vec3::Y * e) - f(p - Vec3::Y * e),
            f(p + Vec3::Z * e) - f(p - Vec3::Z * e),
        )
        .normalize_or(Vec3::Y)
    };
    let vertices = positions
        .iter()
        .map(|&p0| {
            // Pull the vertex onto the surface (two Newton steps).
            let mut p = p0;
            for _ in 0..2 {
                let d = f(p);
                if d.abs() > v * 2.0 {
                    break;
                }
                p -= gradient(p) * d;
            }
            let n = gradient(p);
            let a = n.abs();
            let uv = if a.x >= a.y && a.x >= a.z {
                Vec2::new(p.z * n.x.signum(), -p.y)
            } else if a.y >= a.z {
                Vec2::new(p.x, p.z * n.y.signum())
            } else {
                Vec2::new(-p.x * n.z.signum(), -p.y)
            };
            let mut vertex = Vertex::new(p, n, uv);
            if grid.occlusion {
                let mut occ = 0.0;
                let mut w = 0.5;
                for k in 1..=4 {
                    let dist = v * 1.5 * k as f32;
                    occ += w * (dist - f(p + n * dist)).max(0.0) / dist;
                    w *= 0.5;
                }
                let ao = (1.0 - occ * 1.2).clamp(0.35, 1.0);
                vertex = vertex.with_color(Vec4::new(ao, ao, ao, 1.0));
            }
            vertex
        })
        .collect();
    Mesh::new(vertices, indices)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_sphere_meshes_closed_and_round() {
        let mesh = polygonize(
            &|p| sphere(p, Vec3::ZERO, 0.5),
            Grid::new(Vec3::splat(-0.6), Vec3::splat(0.6), 0.04),
        );
        assert!(mesh.triangle_count() > 1000);
        for v in &mesh.vertices {
            let p = Vec3::from(v.position);
            assert!((p.length() - 0.5).abs() < 0.004, "{p}");
            // Normals point out.
            assert!(Vec3::from(v.normal).dot(p.normalize()) > 0.95);
        }
        // Counter-clockwise front faces: every triangle's winding agrees with its normal.
        for t in mesh.indices.chunks(3) {
            let [a, b, c] = [0, 1, 2].map(|i| Vec3::from(mesh.vertices[t[i] as usize].position));
            let face = (b - a).cross(c - a);
            assert!(face.dot(a + b + c) >= 0.0);
        }
    }

    #[test]
    fn blends_and_shapes_have_sensible_distances() {
        let a = Vec3::new(0.0, 0.0, 0.0);
        let b = Vec3::new(0.0, 1.0, 0.0);
        assert!((round_cone(Vec3::new(0.3, 0.0, 0.0), a, b, 0.2, 0.1) - 0.1).abs() < 1e-3);
        assert!((capsule(Vec3::new(0.0, 0.5, 0.5), a, b, 0.2) - 0.3).abs() < 1e-4);
        let r = round_box(
            Vec3::new(2.0, 0.0, 0.0),
            Vec3::ZERO,
            Quat::IDENTITY,
            Vec3::ONE,
            0.1,
        );
        assert!((r - 1.0).abs() < 1e-4);
        assert!(smin(0.1, 0.1, 0.2) < 0.1);
        assert!(
            ellipsoid(
                Vec3::new(0.0, 0.0, 0.0),
                Vec3::ZERO,
                Vec3::new(1.0, 0.5, 0.3)
            ) < 0.0
        );
    }
}
