//! Eyeballs: a sphere with the clear dome of the cornea in front, placed where the base mesh's
//! eye helpers sit. The shader draws the sclera, the iris seen through the cornea (refracted, so
//! it shifts with the view like a real eye's) and the cornea's wet reflections; the mesh only
//! carries the eye-local direction of each vertex.

use engine_core::math::Vec3;

/// Eyeball radius as a share of the eye helper's (the helper wraps the eye loosely).
pub const RADIUS_SCALE: f32 = 0.93;
/// Angle from the gaze axis where the cornea starts, radians (an 11.5 mm cornea on a 24 mm eye).
pub const CORNEA_ANGLE: f32 = 0.5;
/// How far the cornea's apex stands out of the sphere, as a share of the radius.
pub const CORNEA_BULGE: f32 = 0.085;

/// One eye vertex: model-space position and normal, and the eye-local direction (unit, +Z
/// along the gaze, +Y up) that the shader works in.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct EyeVertex {
    pub position: Vec3,
    pub normal: Vec3,
    pub local: Vec3,
}

/// Both eyes of a character, ready to upload.
#[derive(Debug, Clone, Default)]
pub struct EyeMesh {
    pub vertices: Vec<EyeVertex>,
    pub indices: Vec<u32>,
}

/// Local radius scale of the surface at angle `theta` from the gaze axis: 1 on the sclera, the
/// cornea's dome inside `CORNEA_ANGLE`.
pub fn bulge(theta: f32) -> f32 {
    if theta >= CORNEA_ANGLE {
        return 1.0;
    }
    let x = 1.0 - theta / CORNEA_ANGLE;
    1.0 + CORNEA_BULGE * x * x * (3.0 - 2.0 * x) * (0.6 + 0.4 * x)
}

/// Build both eyeballs at `centers` (model space) with `radius`, looking along +Z.
pub fn build(centers: [Vec3; 2], radius: f32) -> EyeMesh {
    const RINGS: u32 = 48;
    const SEGMENTS: u32 = 64;
    let mut mesh = EyeMesh::default();
    for center in centers {
        let base = mesh.vertices.len() as u32;
        // Rings from the front pole (theta 0) to the back (pi), denser near the cornea edge.
        for i in 0..=RINGS {
            let u = i as f32 / RINGS as f32;
            let theta = u * u * std::f32::consts::PI * 0.9 + u * std::f32::consts::PI * 0.1;
            for j in 0..=SEGMENTS {
                let phi = j as f32 / SEGMENTS as f32 * std::f32::consts::TAU;
                let local = Vec3::new(
                    theta.sin() * phi.cos(),
                    theta.sin() * phi.sin(),
                    theta.cos(),
                );
                let r = radius * bulge(theta);
                mesh.vertices.push(EyeVertex {
                    position: center + local * r,
                    normal: local,
                    local,
                });
            }
        }
        let row = SEGMENTS + 1;
        for i in 0..RINGS {
            for j in 0..SEGMENTS {
                let a = base + i * row + j;
                let b = a + row;
                mesh.indices.extend([a, b, a + 1, a + 1, b, b + 1]);
            }
        }
        // Normals of the dome from the surface itself.
        let start = base as usize;
        let normals = crate::mesh::vertex_normals(
            &mesh.vertices[start..]
                .iter()
                .map(|v| v.position)
                .collect::<Vec<_>>(),
            &mesh.indices[mesh.indices.len() - (RINGS * SEGMENTS * 6) as usize..]
                .iter()
                .map(|i| i - base)
                .collect::<Vec<_>>(),
        );
        for (v, n) in mesh.vertices[start..].iter_mut().zip(normals) {
            // Seams at the poles average poorly; fall back to the sphere normal there.
            v.normal = if n.dot(v.local) > 0.5 { n } else { v.local };
        }
    }
    mesh
}

/// Where the lids cross each eye, for shading the eye under them: per eye (left, then right)
/// its upper and lower lid edge as `y = a + b x + c x²` over the eye-local x (unit radius, head
/// frame), from the lid margins (`margins[eye][upper, lower]`, model space). An eye without
/// enough margin points gets a typical almond opening.
pub fn lid_curves(centers: [Vec3; 2], margins: &[[Vec<Vec3>; 2]; 2]) -> [[f32; 4]; 4] {
    const TYPICAL: [[f32; 4]; 2] = [[0.42, 0.0, -0.35, 0.0], [-0.38, 0.0, 0.3, 0.0]];
    let mut out = [[0.0; 4]; 4];
    for (eye, lids) in margins.iter().enumerate() {
        for (k, lid) in lids.iter().enumerate() {
            let pts: Vec<Vec3> = lid
                .iter()
                .map(|m| (*m - centers[eye]).normalize_or_zero())
                .filter(|d| d.z > 0.2)
                .collect();
            out[eye * 2 + k] = fit_quadratic(&pts).unwrap_or(TYPICAL[k]);
        }
    }
    out
}

/// Least-squares `y = a + b x + c x²` through the points' (x, y).
fn fit_quadratic(pts: &[Vec3]) -> Option<[f32; 4]> {
    if pts.len() < 3 {
        return None;
    }
    // Normal equations, in f64.
    let mut m = [[0.0f64; 4]; 3];
    for p in pts {
        let (x, y) = (p.x as f64, p.y as f64);
        let row = [1.0, x, x * x];
        for i in 0..3 {
            for j in 0..3 {
                m[i][j] += row[i] * row[j];
            }
            m[i][3] += row[i] * y;
        }
    }
    // Gauss-Jordan elimination with partial pivoting.
    for col in 0..3 {
        let pivot = (col..3).max_by(|a, b| m[*a][col].abs().total_cmp(&m[*b][col].abs()))?;
        m.swap(col, pivot);
        if m[col][col].abs() < 1e-12 {
            return None;
        }
        for r in 0..3 {
            if r != col {
                let f = m[r][col] / m[col][col];
                let pivot_row = m[col];
                for (cell, p) in m[r].iter_mut().zip(pivot_row).skip(col) {
                    *cell -= f * p;
                }
            }
        }
    }
    Some([
        (m[0][3] / m[0][0]) as f32,
        (m[1][3] / m[1][1]) as f32,
        (m[2][3] / m[2][2]) as f32,
        0.0,
    ])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lids_are_fitted_as_curves_across_the_eye() {
        // An upper lid along y = 0.4 - 0.3 x² on a unit eye at the origin, a lower one missing.
        let upper: Vec<Vec3> = (-4..=4)
            .map(|i| {
                let x = i as f32 * 0.15;
                let y = 0.4 - 0.3 * x * x;
                Vec3::new(x, y, (1.0 - x * x - y * y).max(0.0).sqrt()) * 0.012
            })
            .collect();
        let c = lid_curves(
            [Vec3::ZERO; 2],
            &[[upper.clone(), Vec::new()], [upper, Vec::new()]],
        );
        assert!(
            (c[0][0] - 0.4).abs() < 1e-3 && c[0][1].abs() < 1e-3,
            "{:?}",
            c[0]
        );
        assert!((c[0][2] + 0.3).abs() < 1e-3, "{:?}", c[0]);
        assert!(
            c[1][0] < 0.0 && c[1][2] > 0.0,
            "a typical lower lid: {:?}",
            c[1]
        );
    }

    #[test]
    fn eyes_face_forward_with_a_cornea() {
        let m = build(
            [Vec3::new(0.03, 0.0, 0.0), Vec3::new(-0.03, 0.0, 0.0)],
            0.012,
        );
        let front = m
            .vertices
            .iter()
            .filter(|v| v.local.z > 0.999)
            .map(|v| v.position.z)
            .fold(f32::MIN, f32::max);
        assert!(
            (front - 0.012 * (1.0 + CORNEA_BULGE)).abs() < 1e-4,
            "{front}"
        );
        // Outward normals, counter-clockwise triangles seen from outside.
        for t in m.indices.as_chunks::<3>().0.iter().take(500) {
            let [a, b, c] = [t[0], t[1], t[2]].map(|i| m.vertices[i as usize].position);
            let n = (b - a).cross(c - a);
            let center = if a.x > 0.0 {
                Vec3::new(0.03, 0.0, 0.0)
            } else {
                Vec3::new(-0.03, 0.0, 0.0)
            };
            if n.length() > 1e-12 {
                assert!(n.dot(a - center) > 0.0);
            }
        }
        assert_eq!(bulge(1.0), 1.0);
    }
}
