//! Mesh building blocks: rounded boxes, beams, lathed shapes, tubes and crystals. UVs are in
//! meters, so a material's `uv_scale` sets its texel density on any size of part.

use std::collections::HashMap;
use std::f32::consts::{FRAC_PI_4, TAU};

use engine_core::prelude::*;
use engine_render::Mesh;

use super::{fix_winding, moved, vtx};

/// Unit icosphere: vertices and triangles.
pub fn icosphere(subdiv: u32) -> (Vec<Vec3>, Vec<[u32; 3]>) {
    let t = (1.0 + 5.0f32.sqrt()) / 2.0;
    let mut v: Vec<Vec3> = [
        [-1.0, t, 0.0],
        [1.0, t, 0.0],
        [-1.0, -t, 0.0],
        [1.0, -t, 0.0],
        [0.0, -1.0, t],
        [0.0, 1.0, t],
        [0.0, -1.0, -t],
        [0.0, 1.0, -t],
        [t, 0.0, -1.0],
        [t, 0.0, 1.0],
        [-t, 0.0, -1.0],
        [-t, 0.0, 1.0],
    ]
    .iter()
    .map(|p| Vec3::from(*p).normalize())
    .collect();
    let mut f: Vec<[u32; 3]> = vec![
        [0, 11, 5],
        [0, 5, 1],
        [0, 1, 7],
        [0, 7, 10],
        [0, 10, 11],
        [1, 5, 9],
        [5, 11, 4],
        [11, 10, 2],
        [10, 7, 6],
        [7, 1, 8],
        [3, 9, 4],
        [3, 4, 2],
        [3, 2, 6],
        [3, 6, 8],
        [3, 8, 9],
        [4, 9, 5],
        [2, 4, 11],
        [6, 2, 10],
        [8, 6, 7],
        [9, 8, 1],
    ];
    for _ in 0..subdiv {
        let mut cache: HashMap<(u32, u32), u32> = HashMap::new();
        let mut mid = |a: u32, b: u32, v: &mut Vec<Vec3>| -> u32 {
            let key = (a.min(b), a.max(b));
            *cache.entry(key).or_insert_with(|| {
                v.push(((v[a as usize] + v[b as usize]) * 0.5).normalize());
                v.len() as u32 - 1
            })
        };
        let mut nf = Vec::with_capacity(f.len() * 4);
        for [a, b, c] in f {
            let ab = mid(a, b, &mut v);
            let bc = mid(b, c, &mut v);
            let ca = mid(c, a, &mut v);
            nf.extend_from_slice(&[[a, ab, ca], [b, bc, ab], [c, ca, bc], [ab, bc, ca]]);
        }
        f = nf;
    }
    (v, f)
}

/// Give every triangle its own vertices with UVs projected along its dominant axis (metres),
/// keeping the smooth normals.
pub fn box_uv_unshared(m: &Mesh) -> Mesh {
    let mut out = Mesh::default();
    for tri in m.indices.as_chunks::<3>().0 {
        let p: Vec<Vec3> = tri
            .iter()
            .map(|&i| Vec3::from(m.vertices[i as usize].position))
            .collect();
        let g = (p[1] - p[0]).cross(p[2] - p[0]).abs();
        for (k, &i) in tri.iter().enumerate() {
            let mut v = m.vertices[i as usize];
            let q = p[k];
            let uv = if g.x >= g.y && g.x >= g.z {
                Vec2::new(q.z, q.y)
            } else if g.y >= g.z {
                Vec2::new(q.x, q.z)
            } else {
                Vec2::new(q.x, q.y)
            };
            v.uv = uv.to_array();
            out.vertices.push(v);
        }
    }
    out.indices = (0..out.vertices.len() as u32).collect();
    out
}

/// Box with rounded edges (radius `r`), centred on the origin. UVs are in metres with V along
/// axis `grain` (0 = x, 1 = y, 2 = z), so wood grain follows the long side.
pub fn bevel_box(size: Vec3, r: f32, grain: usize) -> Mesh {
    let h = size * 0.5;
    let r = r.min(h.min_element() * 0.9).max(0.0005);
    const SEGS: usize = 3;
    let samples = |half: f32| -> Vec<f32> {
        let mut s = Vec::new();
        let inner = half - r;
        for k in (1..=SEGS).rev() {
            s.push(-(inner + r * (k as f32 / SEGS as f32 * FRAC_PI_4).tan()));
        }
        let n = ((2.0 * inner / 0.35).ceil() as usize).max(1);
        for k in 0..=n {
            s.push(-inner + 2.0 * inner * k as f32 / n as f32);
        }
        for k in 1..=SEGS {
            s.push(inner + r * (k as f32 / SEGS as f32 * FRAC_PI_4).tan());
        }
        s
    };
    let inner = h - Vec3::splat(r);
    let mut m = Mesh::default();
    for a in 0..3 {
        let (b, c) = ((a + 1) % 3, (a + 2) % 3);
        let (sb, sc) = (samples(h[b]), samples(h[c]));
        for sign in [-1.0f32, 1.0] {
            let base = m.vertices.len() as u32;
            for &u in &sb {
                for &v in &sc {
                    let mut p = Vec3::ZERO;
                    p[a] = sign * h[a];
                    p[b] = u;
                    p[c] = v;
                    let q = p.clamp(-inner, inner);
                    let n = (p - q).normalize_or(Vec3::Y);
                    let pos = q + n * r;
                    let uv = if grain == b {
                        Vec2::new(v, u)
                    } else {
                        Vec2::new(u, v)
                    };
                    m.vertices.push(vtx(pos, n, uv, Vec4::ONE));
                }
            }
            let nc = sc.len() as u32;
            for k in 0..sb.len() as u32 - 1 {
                for l in 0..nc - 1 {
                    let i00 = base + k * nc + l;
                    let i10 = i00 + nc;
                    m.indices
                        .extend_from_slice(&[i00, i10, i10 + 1, i00, i10 + 1, i00 + 1]);
                }
            }
        }
    }
    fix_winding(&mut m);
    m
}

/// A beam between two points (rounded box, grain along its length).
pub fn beam(a: Vec3, b: Vec3, w: f32, d: f32, r: f32) -> Mesh {
    let len = a.distance(b);
    let m = bevel_box(Vec3::new(w, len, d), r, 1);
    let rot = Quat::from_rotation_arc(Vec3::Y, (b - a) / len);
    moved(m, Mat4::from_rotation_translation(rot, (a + b) * 0.5))
}

/// Surface of revolution around +Y. `profile` holds (radius, y) from bottom to top. UVs: u
/// around the circumference in metres at radius `u_radius`, v = y in metres.
pub fn lathe(profile: &[(f32, f32)], segments: u32, u_radius: f32) -> Mesh {
    let mut m = Mesh::default();
    let n = profile.len();
    let row = segments + 1;
    for (k, &(r, y)) in profile.iter().enumerate() {
        let prev = profile[k.saturating_sub(1)];
        let next = profile[(k + 1).min(n - 1)];
        let t = Vec2::new(next.0 - prev.0, next.1 - prev.1);
        let n2 = Vec2::new(t.y, -t.x).normalize_or(Vec2::X);
        for s in 0..=segments {
            let a = s as f32 / segments as f32 * TAU;
            let (sin, cos) = a.sin_cos();
            let p = Vec3::new(cos * r, y, -sin * r);
            let nn = Vec3::new(cos * n2.x, n2.y, -sin * n2.x);
            m.vertices
                .push(vtx(p, nn, Vec2::new(a * u_radius, y), Vec4::ONE));
        }
    }
    for k in 0..n as u32 - 1 {
        for s in 0..segments {
            let i = k * row + s;
            m.indices
                .extend_from_slice(&[i, i + 1, i + row, i + 1, i + row + 1, i + row]);
        }
    }
    fix_winding(&mut m);
    m
}

/// A tube between two points.
pub fn tube(a: Vec3, b: Vec3, radius: f32, segments: u32) -> Mesh {
    let len = a.distance(b);
    let m = lathe(
        &[(0.0, 0.0), (radius, 0.0), (radius, len), (0.0, len)],
        segments,
        radius,
    );
    let rot = Quat::from_rotation_arc(Vec3::Y, (b - a) / len);
    moved(m, Mat4::from_rotation_translation(rot, a))
}

/// A crystal: a six-sided prism with a pointed tip, faceted.
pub fn crystal(len: f32, r: f32) -> Mesh {
    let mut m = lathe(
        &[
            (0.0, -0.02),
            (r * 0.8, 0.0),
            (r, len * 0.15),
            (r * 0.92, len * 0.72),
            (0.0, len),
        ],
        6,
        r,
    );
    m.compute_flat_normals();
    m
}
