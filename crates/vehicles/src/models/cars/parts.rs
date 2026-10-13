//! Mesh builders shared by both cars: glass panes and decals laid onto a body field, lathed
//! parts (tires, rims, discs), lofts and tubes (roll cage, exhausts, wipers), boxes.

use engine_core::math::{Mat4, Quat, Vec2, Vec3, Vec4};
use engine_render::{Mesh, Vertex};

use super::mesher::Field;
use super::sdf::{gradient, raycast};

/// A grid laid onto the surface of `field`: `ray(s, t)` gives a ray start and direction for
/// each grid point (`s`, `t` in 0..1); the hit, pushed `offset` meters out along the normal,
/// becomes the vertex. `uv(s, t, hit)` picks the texture coordinate. Points whose ray misses
/// are dropped with their triangles.
pub fn draped_grid(
    field: Field,
    ns: usize,
    nt: usize,
    ray: impl Fn(f32, f32) -> (Vec3, Vec3),
    offset: f32,
    uv: impl Fn(f32, f32, Vec3) -> Vec2,
) -> Mesh {
    let mut verts: Vec<Option<Vertex>> = Vec::with_capacity((ns + 1) * (nt + 1));
    for j in 0..=nt {
        for i in 0..=ns {
            let (s, t) = (i as f32 / ns as f32, j as f32 / nt as f32);
            let (o, d) = ray(s, t);
            verts.push(raycast(field, o, d, 4.0).map(|hit| {
                let n = gradient(field, hit, 0.002).normalize_or(-d);
                Vertex::new(hit + n * offset, n, uv(s, t, hit))
            }));
        }
    }
    let mut mesh = Mesh::default();
    let mut remap = vec![u32::MAX; verts.len()];
    for (k, v) in verts.iter().enumerate() {
        if let Some(v) = v {
            remap[k] = mesh.vertices.len() as u32;
            mesh.vertices.push(*v);
        }
    }
    let id = |i: usize, j: usize| remap[j * (ns + 1) + i];
    for j in 0..nt {
        for i in 0..ns {
            let q = [id(i, j), id(i + 1, j), id(i + 1, j + 1), id(i, j + 1)];
            if q.contains(&u32::MAX) {
                continue;
            }
            mesh.indices
                .extend_from_slice(&[q[0], q[1], q[2], q[0], q[2], q[3]]);
        }
    }
    orient_like_normals(&mut mesh);
    mesh
}

/// Flip triangles whose winding disagrees with their vertex normals.
pub fn orient_like_normals(mesh: &mut Mesh) {
    for t in mesh.indices.as_chunks_mut::<3>().0.iter_mut() {
        let p = |i: u32| Vec3::from(mesh.vertices[i as usize].position);
        let n = |i: u32| Vec3::from(mesh.vertices[i as usize].normal);
        let f = (p(t[1]) - p(t[0])).cross(p(t[2]) - p(t[0]));
        if f.dot(n(t[0]) + n(t[1]) + n(t[2])) < 0.0 {
            t.swap(1, 2);
        }
    }
}

/// A surface of revolution about the X axis (wheels): `profile` is a list of
/// (axial x, radius) points; `segments` around. Smooth normals; `u` runs around (0..1), `v`
/// along the profile by length (0..1). Hard edges where `sharp[k]` is set duplicate vertex k.
pub fn lathe_x(profile: &[Vec2], segments: usize, sharp: &[usize]) -> Mesh {
    // Split the profile at sharp points into smooth runs.
    let mut runs: Vec<Vec<Vec2>> = vec![vec![]];
    for (k, p) in profile.iter().enumerate() {
        runs.last_mut().unwrap().push(*p);
        if sharp.contains(&k) && k + 1 < profile.len() {
            runs.push(vec![*p]);
        }
    }
    let total: f32 = profile
        .windows(2)
        .map(|w| (w[1] - w[0]).length())
        .sum::<f32>()
        .max(1e-6);
    let mut mesh = Mesh::default();
    let mut acc = 0.0;
    for run in runs {
        if run.len() < 2 {
            continue;
        }
        // Profile normals (2D, pointing away from the axis side the profile winds toward).
        let n2: Vec<Vec2> = (0..run.len())
            .map(|k| {
                let a = run[k.saturating_sub(1)];
                let b = run[(k + 1).min(run.len() - 1)];
                let d = (b - a).normalize_or(Vec2::X);
                Vec2::new(d.y, -d.x)
            })
            .collect();
        let base = mesh.vertices.len() as u32;
        let mut lens = vec![acc];
        for w in run.windows(2) {
            acc += (w[1] - w[0]).length();
            lens.push(acc);
        }
        for (k, p) in run.iter().enumerate() {
            for s in 0..=segments {
                let a = s as f32 / segments as f32 * std::f32::consts::TAU;
                let (sa, ca) = a.sin_cos();
                let pos = Vec3::new(p.x, p.y * ca, p.y * sa);
                let nrm = Vec3::new(n2[k].x, n2[k].y * ca, n2[k].y * sa);
                mesh.vertices.push(Vertex::new(
                    pos,
                    nrm,
                    Vec2::new(s as f32 / segments as f32, lens[k] / total),
                ));
            }
        }
        let row = (segments + 1) as u32;
        for k in 0..run.len() as u32 - 1 {
            for s in 0..segments as u32 {
                let a = base + k * row + s;
                let b = a + 1;
                let c = a + row + 1;
                let d = a + row;
                mesh.indices.extend_from_slice(&[a, b, c, a, c, d]);
            }
        }
    }
    orient_like_normals(&mut mesh);
    mesh
}

/// A loft through closed cross-sections (each a ring of points, equal counts).
pub fn loft_rings(rings: &[Vec<Vec3>], cap: bool) -> Mesh {
    let m = rings[0].len();
    let mut mesh = Mesh::default();
    for (i, r) in rings.iter().enumerate() {
        for (j, p) in r.iter().enumerate() {
            mesh.vertices.push(Vertex::new(
                *p,
                Vec3::Y,
                Vec2::new(j as f32 / m as f32, i as f32 / (rings.len() - 1) as f32),
            ));
        }
    }
    for i in 0..rings.len() - 1 {
        for j in 0..m {
            let a = (i * m + j) as u32;
            let b = (i * m + (j + 1) % m) as u32;
            let c = ((i + 1) * m + (j + 1) % m) as u32;
            let d = ((i + 1) * m + j) as u32;
            mesh.indices.extend_from_slice(&[a, b, c, a, c, d]);
        }
    }
    if cap {
        for (ri, flip) in [(0, true), (rings.len() - 1, false)] {
            let c: Vec3 = rings[ri].iter().copied().sum::<Vec3>() / m as f32;
            let base = mesh.vertices.len() as u32;
            mesh.vertices
                .push(Vertex::new(c, Vec3::Y, Vec2::splat(0.5)));
            let first = (ri * m) as u32;
            for j in 0..m as u32 {
                let (a, b) = (first + j, first + (j + 1) % m as u32);
                if flip {
                    mesh.indices.extend_from_slice(&[base, b, a]);
                } else {
                    mesh.indices.extend_from_slice(&[base, a, b]);
                }
            }
        }
    }
    fix_winding_outward(&mut mesh);
    mesh.compute_normals();
    mesh
}

/// Make a closed mesh face outward (signed volume positive).
pub fn fix_winding_outward(mesh: &mut Mesh) {
    let mut vol = 0.0;
    for t in mesh.indices.as_chunks::<3>().0.iter() {
        let p = |i: u32| Vec3::from(mesh.vertices[i as usize].position);
        vol += p(t[0]).dot(p(t[1]).cross(p(t[2])));
    }
    if vol < 0.0 {
        for t in mesh.indices.as_chunks_mut::<3>().0.iter_mut() {
            t.swap(1, 2);
        }
    }
}

/// A round tube along a polyline (radius `r`, `sides` around), with smooth bends.
pub fn tube(points: &[Vec3], r: f32, sides: usize) -> Mesh {
    let n = points.len();
    let mut rings = Vec::with_capacity(n);
    let mut prev_side = None::<Vec3>;
    for k in 0..n {
        let dir = if k == 0 {
            points[1] - points[0]
        } else if k == n - 1 {
            points[n - 1] - points[n - 2]
        } else {
            (points[k + 1] - points[k]).normalize_or_zero()
                + (points[k] - points[k - 1]).normalize_or_zero()
        }
        .normalize_or(Vec3::Z);
        let side = match prev_side {
            Some(s) => (s - dir * s.dot(dir)).normalize_or(dir.any_orthonormal_vector()),
            None => dir.any_orthonormal_vector(),
        };
        prev_side = Some(side);
        let up = dir.cross(side);
        rings.push(
            (0..sides)
                .map(|j| {
                    let a = j as f32 / sides as f32 * std::f32::consts::TAU;
                    points[k] + (side * a.cos() + up * a.sin()) * r
                })
                .collect::<Vec<_>>(),
        );
    }
    loft_rings(&rings, true)
}

/// A box with bevelled edges (`bevel` meters), centered at the origin, smooth-ish shading.
pub fn bevel_box(size: Vec3, bevel: f32) -> Mesh {
    let h = size * 0.5;
    let b = bevel.min(h.min_element() * 0.9);
    let inner = h - Vec3::splat(b);
    // Rings of a rounded rectangle in XY, at z levels forming the bevel.
    let ring = |z: f32, grow: f32| -> Vec<Vec3> {
        let mut pts = Vec::new();
        let corners = [
            (inner.x, inner.y, 0.0f32),
            (-inner.x, inner.y, 90.0),
            (-inner.x, -inner.y, 180.0),
            (inner.x, -inner.y, 270.0),
        ];
        for (cx, cy, a0) in corners {
            for s in 0..3 {
                let a = (a0 + s as f32 * 45.0).to_radians();
                pts.push(Vec3::new(cx + a.cos() * grow, cy + a.sin() * grow, z));
            }
        }
        pts
    };
    let rings = vec![
        ring(-h.z, 0.0),
        ring(-h.z + b * 0.3, b * 0.7),
        ring(-inner.z, b),
        ring(inner.z, b),
        ring(h.z - b * 0.3, b * 0.7),
        ring(h.z, 0.0),
    ];
    loft_rings(&rings, true)
}

/// Apply `m` to a mesh (positions and normals).
pub fn transformed(mut mesh: Mesh, m: Mat4) -> Mesh {
    mesh.transform(&m);
    mesh
}

/// Translate + rotate + scale shorthand.
pub fn trs(t: Vec3, r: Quat, s: Vec3) -> Mat4 {
    Mat4::from_scale_rotation_translation(s, r, t)
}

/// Paint every vertex of `mesh` with `color` (linear RGBA) — handy for merging parts that
/// share a material but differ in shade.
pub fn tint(mut mesh: Mesh, color: Vec4) -> Mesh {
    for v in &mut mesh.vertices {
        v.color = color.to_array();
    }
    mesh
}

/// Merge meshes.
pub fn merge(parts: impl IntoIterator<Item = Mesh>) -> Mesh {
    let mut out = Mesh::default();
    for p in parts {
        out.merge(&p);
    }
    out
}

/// Mirror a mesh across x = 0 (keeps it facing outward).
pub fn mirror_x(mesh: &Mesh) -> Mesh {
    let mut m = mesh.clone();
    for v in &mut m.vertices {
        v.position[0] = -v.position[0];
        v.normal[0] = -v.normal[0];
    }
    for t in m.indices.as_chunks_mut::<3>().0.iter_mut() {
        t.swap(1, 2);
    }
    m
}
