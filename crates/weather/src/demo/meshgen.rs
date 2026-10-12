//! Mesh building blocks for the station props: boxes with rounded edges, surfaces of
//! revolution (lathe), profiles swept along paths (tubes, rails, gutters), extruded outlines
//! and height fields. Units are meters; texture coordinates are in meters too (materials tile
//! them with `uv_scale`), with `v` growing downward on vertical faces so text reads upright.
//!
//! Curved surfaces get smooth normals; corners sharper than a crease angle stay hard.

use std::f32::consts::{FRAC_PI_4, TAU};

use engine_core::math::{Mat3, Mat4, Quat, Vec2, Vec3, Vec4};
use engine_render::{Mesh, Vertex};

/// Collects vertices and triangles; merges placed meshes into one.
#[derive(Default)]
pub struct Builder {
    pub mesh: Mesh,
}

impl Builder {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn vertex(&mut self, p: Vec3, n: Vec3, uv: Vec2) -> u32 {
        self.mesh.vertices.push(Vertex::new(p, n, uv));
        self.mesh.vertices.len() as u32 - 1
    }

    /// Triangle, counter-clockwise seen from the front.
    pub fn tri(&mut self, a: u32, b: u32, c: u32) {
        self.mesh.indices.extend_from_slice(&[a, b, c]);
    }

    /// Triangle wound so its front faces along `n`.
    pub fn tri_facing(&mut self, a: u32, b: u32, c: u32, n: Vec3) {
        let p = |i: u32| Vec3::from(self.mesh.vertices[i as usize].position);
        let g = (p(b) - p(a)).cross(p(c) - p(a));
        if g.dot(n) >= 0.0 {
            self.tri(a, b, c);
        } else {
            self.tri(a, c, b);
        }
    }

    /// Append `m` transformed by `t`.
    pub fn add(&mut self, m: &Mesh, t: Mat4) -> &mut Self {
        let mut m = m.clone();
        transform(&mut m, t);
        self.mesh.merge(&m);
        self
    }

    /// Append `m` transformed by `t`, with every vertex color multiplied by `color`.
    pub fn add_tinted(&mut self, m: &Mesh, t: Mat4, color: Vec4) -> &mut Self {
        let mut m = m.clone();
        transform(&mut m, t);
        for v in &mut m.vertices {
            v.color = (Vec4::from(v.color) * color).to_array();
        }
        self.mesh.merge(&m);
        self
    }

    pub fn finish(self) -> Mesh {
        self.mesh
    }
}

/// Transform positions and normals (handles mirroring by flipping the winding).
pub fn transform(m: &mut Mesh, t: Mat4) {
    let nm = Mat3::from_mat4(t).inverse().transpose();
    for v in &mut m.vertices {
        v.position = t.transform_point3(Vec3::from(v.position)).to_array();
        v.normal = (nm * Vec3::from(v.normal)).normalize_or(Vec3::Y).to_array();
    }
    if Mat3::from_mat4(t).determinant() < 0.0 {
        for tri in m.indices.as_chunks_mut::<3>().0 {
            tri.swap(1, 2);
        }
    }
}

/// A transform from translation, rotation and scale.
pub fn trs(t: Vec3, r: Quat, s: Vec3) -> Mat4 {
    Mat4::from_scale_rotation_translation(s, r, t)
}

/// A translation.
pub fn at(x: f32, y: f32, z: f32) -> Mat4 {
    Mat4::from_translation(Vec3::new(x, y, z))
}

/// Darken vertices near the ground (`y` below `height`) to `floor` brightness: cheap contact
/// occlusion where props meet the ground.
pub fn ground_occlusion(m: &mut Mesh, height: f32, floor: f32) {
    for v in &mut m.vertices {
        let t = (v.position[1] / height).clamp(0.0, 1.0);
        let k = floor + (1.0 - floor) * (t * t * (3.0 - 2.0 * t));
        for c in &mut v.color[..3] {
            *c *= k;
        }
    }
}

/// Box centered on the origin with edges rounded to radius `r` (`segs` steps per 45°, so
/// `segs = 1` already gives a soft highlight). `skip` drops faces by bit: +X, −X, +Y, −Y, +Z,
/// −Z (bits 0–5), e.g. `1 << 3` for a box standing on the ground.
pub fn rounded_box(size: Vec3, r: f32, segs: u32, skip: u8) -> Mesh {
    let h = size * 0.5;
    let r = r.clamp(0.0, h.min_element() * 0.999);
    let inner = h - Vec3::splat(r);
    let coords = |half: f32| -> Vec<f32> {
        if r <= 0.0 || segs == 0 {
            return vec![-half, half];
        }
        let mut c = Vec::new();
        for k in (0..=segs).rev() {
            let t = (k as f32 / segs as f32 * FRAC_PI_4).tan();
            c.push(-(half - r) - r * t);
        }
        for k in 0..=segs {
            let t = (k as f32 / segs as f32 * FRAC_PI_4).tan();
            c.push((half - r) + r * t);
        }
        c
    };
    let faces: [(Vec3, Vec3, Vec3); 6] = [
        (Vec3::X, Vec3::NEG_Z, Vec3::Y),
        (Vec3::NEG_X, Vec3::Z, Vec3::Y),
        (Vec3::Y, Vec3::X, Vec3::NEG_Z),
        (Vec3::NEG_Y, Vec3::X, Vec3::Z),
        (Vec3::Z, Vec3::X, Vec3::Y),
        (Vec3::NEG_Z, Vec3::NEG_X, Vec3::Y),
    ];
    let mut b = Builder::new();
    for (f, (n, u, v)) in faces.into_iter().enumerate() {
        if skip & (1 << f) != 0 {
            continue;
        }
        let hn = n.abs().dot(h);
        let hu = u.abs().dot(h);
        let hv = v.abs().dot(h);
        let cu = coords(hu);
        let cv = coords(hv);
        let base = b.mesh.vertices.len() as u32;
        for &bv in &cv {
            for &au in &cu {
                let p = n * hn + u * au + v * bv;
                let c = p.clamp(-inner, inner);
                let d = p - c;
                let (pos, nor) = if r > 0.0 && d.length_squared() > 1e-12 {
                    let dn = d.normalize();
                    (c + dn * r, dn)
                } else {
                    (p, n)
                };
                b.vertex(pos, nor, Vec2::new(au + hu, hv - bv));
            }
        }
        let nu = cu.len() as u32;
        for j in 0..cv.len() as u32 - 1 {
            for i in 0..nu - 1 {
                let a = base + j * nu + i;
                b.tri(a, a + 1, a + nu + 1);
                b.tri(a, a + nu + 1, a + nu);
            }
        }
    }
    b.finish()
}

/// A profile point after crease handling: position, 2D normal, arc length, and whether a
/// strip connects it to the next one.
#[derive(Clone, Copy, Debug)]
struct RingPt {
    p: Vec2,
    n: Vec2,
    v: f32,
    connect: bool,
}

/// Split a 2D polyline into ring points: smooth normals where consecutive segments bend less
/// than the crease angle, duplicated points with each side's normal where they bend more.
/// The outward normal of a segment is its direction turned clockwise, so profiles run with
/// the visible side on their right (CCW outlines face outward).
fn rings(pts: &[Vec2], closed: bool, crease_deg: f32) -> Vec<RingPt> {
    let mut pts: Vec<Vec2> = pts.to_vec();
    pts.dedup_by(|a, b| a.distance_squared(*b) < 1e-14);
    if closed && pts.len() > 2 && pts[0].distance_squared(*pts.last().unwrap()) < 1e-14 {
        pts.pop();
    }
    let n = pts.len();
    let seg_count = if closed { n } else { n - 1 };
    let seg_normal = |i: usize| {
        let a = pts[i % n];
        let b = pts[(i + 1) % n];
        let t = (b - a).normalize_or_zero();
        Vec2::new(t.y, -t.x)
    };
    let crease = crease_deg.to_radians().cos();
    let mut out = Vec::new();
    let mut v = 0.0;
    let count = if closed { n + 1 } else { n };
    for k in 0..count {
        let i = k % n;
        if k > 0 {
            v += pts[i].distance(pts[(k - 1) % n]);
        }
        let prev = if k > 0 {
            Some(seg_normal(k - 1))
        } else if closed {
            Some(seg_normal(seg_count - 1))
        } else {
            None
        };
        let next = if k < seg_count {
            Some(seg_normal(k))
        } else if closed {
            Some(seg_normal(0))
        } else {
            None
        };
        match (prev, next) {
            (Some(a), Some(b)) if a.dot(b) >= crease => out.push(RingPt {
                p: pts[i],
                n: (a + b).normalize_or(b),
                v,
                connect: true,
            }),
            (Some(a), Some(b)) => {
                out.push(RingPt {
                    p: pts[i],
                    n: a,
                    v,
                    connect: false,
                });
                out.push(RingPt {
                    p: pts[i],
                    n: b,
                    v,
                    connect: true,
                });
            }
            (Some(a), None) => out.push(RingPt {
                p: pts[i],
                n: a,
                v,
                connect: false,
            }),
            (None, Some(b)) => out.push(RingPt {
                p: pts[i],
                n: b,
                v,
                connect: true,
            }),
            (None, None) => {}
        }
    }
    if let Some(last) = out.last_mut() {
        last.connect = false;
    }
    out
}

/// Surface of revolution around +Y. `profile` holds (radius, height) points, listed so the
/// visible side is on the right when walking along them (bottom to top for an outer wall).
/// Bends sharper than `crease_deg` stay hard. `segments` around; `faceted` gives flat sides
/// (hex nuts, octagonal posts).
pub fn lathe(profile: &[(f32, f32)], segments: u32, crease_deg: f32, faceted: bool) -> Mesh {
    let pts: Vec<Vec2> = profile.iter().map(|&(r, y)| Vec2::new(r, y)).collect();
    let ring = rings(&pts, false, crease_deg);
    let max_r = pts.iter().fold(0.0f32, |m, p| m.max(p.x));
    let u_len = TAU * max_r.max(0.01);
    let mut b = Builder::new();
    let segs = segments.max(3);
    let angle = |s: u32| s as f32 / segs as f32 * TAU;
    if !faceted {
        let row = segs + 1;
        let base = b.mesh.vertices.len() as u32;
        for rp in &ring {
            for s in 0..=segs {
                let (sin, cos) = angle(s).sin_cos();
                b.vertex(
                    Vec3::new(rp.p.x * cos, rp.p.y, -rp.p.x * sin),
                    Vec3::new(rp.n.x * cos, rp.n.y, -rp.n.x * sin),
                    Vec2::new(s as f32 / segs as f32 * u_len, rp.v),
                );
            }
        }
        for (k, rp) in ring.iter().enumerate() {
            if !rp.connect {
                continue;
            }
            let k = k as u32;
            for s in 0..segs {
                let a = base + k * row + s;
                b.tri(a, a + 1, a + row + 1);
                b.tri(a, a + row + 1, a + row);
            }
        }
    } else {
        for s in 0..segs {
            let (s0, c0) = angle(s).sin_cos();
            let (s1, c1) = angle(s + 1).sin_cos();
            let (sm, cm) = ((angle(s) + angle(s + 1)) * 0.5).sin_cos();
            let base = b.mesh.vertices.len() as u32;
            for rp in &ring {
                let n = Vec3::new(rp.n.x * cm, rp.n.y, -rp.n.x * sm).normalize_or(Vec3::Y);
                b.vertex(
                    Vec3::new(rp.p.x * c0, rp.p.y, -rp.p.x * s0),
                    n,
                    Vec2::new(s as f32 / segs as f32 * u_len, rp.v),
                );
                b.vertex(
                    Vec3::new(rp.p.x * c1, rp.p.y, -rp.p.x * s1),
                    n,
                    Vec2::new((s + 1) as f32 / segs as f32 * u_len, rp.v),
                );
            }
            for (k, rp) in ring.iter().enumerate() {
                if !rp.connect {
                    continue;
                }
                let a = base + k as u32 * 2;
                b.tri(a, a + 1, a + 3);
                b.tri(a, a + 3, a + 2);
            }
        }
    }
    b.finish()
}

/// Frames (tangent, normal, binormal) along a path, rotation-minimizing, starting from `up`.
fn frames(path: &[Vec3], up: Vec3) -> Vec<(Vec3, Vec3, Vec3)> {
    let n = path.len();
    let mut out = Vec::with_capacity(n);
    let mut normal = Vec3::ZERO;
    for i in 0..n {
        let t = if i == 0 {
            path[1] - path[0]
        } else if i == n - 1 {
            path[n - 1] - path[n - 2]
        } else {
            (path[i + 1] - path[i]).normalize_or_zero()
                + (path[i] - path[i - 1]).normalize_or_zero()
        }
        .normalize_or(Vec3::Z);
        if i == 0 {
            normal = up - t * up.dot(t);
            if normal.length_squared() < 1e-8 {
                let axis = if t.x.abs() < 0.9 { Vec3::X } else { Vec3::Z };
                normal = axis - t * t.dot(axis);
            }
        } else {
            normal -= t * normal.dot(t);
        }
        normal = normal.normalize_or(Vec3::Y);
        out.push((t, normal, t.cross(normal)));
    }
    out
}

/// Sweep a 2D `profile` (x along the path's normal, which starts near `up`, y along the
/// binormal; counter-clockwise when closed) along `path`. Bends of the profile sharper than
/// `crease_deg` stay hard. `caps` closes both ends of a closed convex profile.
/// `scale` (one per path point, or empty) scales the profile along the path (tapers).
pub fn sweep(
    profile: &[Vec2],
    closed: bool,
    path: &[Vec3],
    scale: &[f32],
    up: Vec3,
    crease_deg: f32,
    caps: bool,
) -> Mesh {
    let ring = rings(profile, closed, crease_deg);
    let fr = frames(path, up);
    let mut b = Builder::new();
    let row = ring.len() as u32;
    let mut along = 0.0;
    let s_at = |i: usize| scale.get(i).copied().unwrap_or(1.0);
    for (i, (p, (_, nn, bn))) in path.iter().zip(&fr).enumerate() {
        if i > 0 {
            along += p.distance(path[i - 1]);
        }
        let s = s_at(i);
        for rp in &ring {
            b.vertex(
                *p + (*nn * rp.p.x + *bn * rp.p.y) * s,
                (*nn * rp.n.x + *bn * rp.n.y).normalize_or(Vec3::Y),
                Vec2::new(rp.v * s, along),
            );
        }
    }
    for i in 0..path.len() as u32 - 1 {
        for (k, rp) in ring.iter().enumerate() {
            if !rp.connect {
                continue;
            }
            let a = i * row + k as u32;
            b.tri(a, a + 1, a + row + 1);
            b.tri(a, a + row + 1, a + row);
        }
    }
    if caps && closed {
        for (end, sign) in [(0usize, -1.0f32), (path.len() - 1, 1.0)] {
            let (t, nn, bn) = fr[end];
            let n = t * sign;
            let s = s_at(end);
            let c = profile.iter().copied().sum::<Vec2>() / profile.len() as f32;
            let center = b.vertex(
                path[end] + (nn * c.x + bn * c.y) * s,
                n,
                Vec2::new(c.x, -c.y),
            );
            let first = b.mesh.vertices.len() as u32;
            for q in profile {
                b.vertex(
                    path[end] + (nn * q.x + bn * q.y) * s,
                    n,
                    Vec2::new(q.x * s, -q.y * s),
                );
            }
            let m = profile.len() as u32;
            for k in 0..m {
                b.tri_facing(center, first + k, first + (k + 1) % m, n);
            }
        }
    }
    b.finish()
}

/// Extrude a closed outline in the XY plane (counter-clockwise, convex for the caps) along Z
/// from `-depth/2` to `depth/2`. Cap texture coordinates run from the outline's top-left
/// corner in meters (u right, v down), so a texture scaled by `1/size` covers the face.
pub fn extrude(outline: &[Vec2], depth: f32, crease_deg: f32) -> Mesh {
    let ring = rings(outline, true, crease_deg);
    let mut b = Builder::new();
    let hz = depth * 0.5;
    let row = ring.len() as u32;
    for z in [-hz, hz] {
        for rp in &ring {
            b.vertex(
                Vec3::new(rp.p.x, rp.p.y, z),
                Vec3::new(rp.n.x, rp.n.y, 0.0),
                Vec2::new(rp.v, z + hz),
            );
        }
    }
    for (k, rp) in ring.iter().enumerate() {
        if !rp.connect {
            continue;
        }
        let a = k as u32;
        // Outline CCW seen from +Z, outward normals on the right: front faces outward.
        b.tri_facing(a, a + 1, a + row + 1, Vec3::new(rp.n.x, rp.n.y, 0.0));
        b.tri_facing(a, a + row + 1, a + row, Vec3::new(rp.n.x, rp.n.y, 0.0));
    }
    let min = outline.iter().fold(Vec2::splat(f32::MAX), |m, p| m.min(*p));
    let max = outline.iter().fold(Vec2::splat(f32::MIN), |m, p| m.max(*p));
    let c = outline.iter().copied().sum::<Vec2>() / outline.len() as f32;
    for (z, n) in [(hz, Vec3::Z), (-hz, Vec3::NEG_Z)] {
        let uv = |p: Vec2| {
            if n.z > 0.0 {
                Vec2::new(p.x - min.x, max.y - p.y)
            } else {
                Vec2::new(max.x - p.x, max.y - p.y)
            }
        };
        let center = b.vertex(Vec3::new(c.x, c.y, z), n, uv(c));
        let first = b.mesh.vertices.len() as u32;
        for p in outline {
            b.vertex(Vec3::new(p.x, p.y, z), n, uv(*p));
        }
        let m = outline.len() as u32;
        for k in 0..m {
            b.tri_facing(center, first + k, first + (k + 1) % m, n);
        }
    }
    b.finish()
}

/// Counter-clockwise rounded rectangle centered on the origin.
pub fn rounded_rect(w: f32, h: f32, r: f32, segs: u32) -> Vec<Vec2> {
    let r = r.min(w * 0.5).min(h * 0.5);
    let (hw, hh) = (w * 0.5 - r, h * 0.5 - r);
    let mut out = Vec::new();
    for (cx, cy, a0) in [
        (hw, -hh, -90.0f32),
        (hw, hh, 0.0),
        (-hw, hh, 90.0),
        (-hw, -hh, 180.0),
    ] {
        for k in 0..=segs {
            let a = (a0 + 90.0 * k as f32 / segs.max(1) as f32).to_radians();
            out.push(Vec2::new(cx + r * a.cos(), cy + r * a.sin()));
        }
    }
    out
}

/// Counter-clockwise circle.
pub fn circle(r: f32, n: u32) -> Vec<Vec2> {
    (0..n)
        .map(|k| {
            let a = k as f32 / n as f32 * TAU;
            Vec2::new(r * a.cos(), r * a.sin())
        })
        .collect()
}

/// Tube of radius `r` along `path`.
pub fn tube(path: &[Vec3], r: f32, sides: u32, caps: bool) -> Mesh {
    sweep(&circle(r, sides), true, path, &[], Vec3::Y, 70.0, caps)
}

/// Straight tube from `a` to `b`.
pub fn rod(a: Vec3, b: Vec3, r: f32, sides: u32) -> Mesh {
    let up = if (b - a).normalize_or(Vec3::Y).y.abs() > 0.9 {
        Vec3::X
    } else {
        Vec3::Y
    };
    sweep(&circle(r, sides), true, &[a, b], &[], up, 70.0, true)
}

/// Cubic Bézier sampled at `n + 1` points.
pub fn bezier(p0: Vec3, p1: Vec3, p2: Vec3, p3: Vec3, n: u32) -> Vec<Vec3> {
    (0..=n)
        .map(|k| {
            let t = k as f32 / n as f32;
            let u = 1.0 - t;
            p0 * (u * u * u) + p1 * (3.0 * u * u * t) + p2 * (3.0 * u * t * t) + p3 * (t * t * t)
        })
        .collect()
}

/// Height field over `[x0, x1] × [z0, z1]` with `nx × nz` quads. `f(x, z)` returns the height
/// and a vertex color. Normals come from the triangles (smooth).
pub fn heightfield(
    (x0, z0): (f32, f32),
    (x1, z1): (f32, f32),
    (nx, nz): (u32, u32),
    f: impl Fn(f32, f32) -> (f32, Vec4),
) -> Mesh {
    let mut m = Mesh::default();
    for j in 0..=nz {
        for i in 0..=nx {
            let x = x0 + (x1 - x0) * i as f32 / nx as f32;
            let z = z0 + (z1 - z0) * j as f32 / nz as f32;
            let (y, c) = f(x, z);
            m.vertices
                .push(Vertex::new(Vec3::new(x, y, z), Vec3::Y, Vec2::new(x, z)).with_color(c));
        }
    }
    let row = nx + 1;
    for j in 0..nz {
        for i in 0..nx {
            let a = j * row + i;
            m.indices
                .extend_from_slice(&[a, a + row, a + 1, a + 1, a + row, a + row + 1]);
        }
    }
    m.compute_normals();
    m
}

#[cfg(test)]
pub(crate) fn check_winding(m: &Mesh) -> usize {
    let mut bad = 0;
    for tri in m.indices.as_chunks::<3>().0 {
        let p: Vec<Vec3> = tri
            .iter()
            .map(|&i| Vec3::from(m.vertices[i as usize].position))
            .collect();
        let n: Vec3 = tri
            .iter()
            .map(|&i| Vec3::from(m.vertices[i as usize].normal))
            .sum();
        let g = (p[1] - p[0]).cross(p[2] - p[0]);
        if g.length_squared() > 1e-14 && g.dot(n) < 0.0 {
            bad += 1;
        }
    }
    bad
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn primitives_face_outward() {
        let shapes = [
            ("box", rounded_box(Vec3::new(1.0, 0.5, 0.3), 0.05, 2, 0)),
            ("sharp box", rounded_box(Vec3::ONE, 0.0, 2, 1 << 3)),
            (
                "lathe",
                lathe(
                    &[(0.0, 0.0), (0.3, 0.0), (0.3, 1.0), (0.1, 1.2), (0.0, 1.2)],
                    16,
                    30.0,
                    false,
                ),
            ),
            (
                "nut",
                lathe(
                    &[(0.0, 0.0), (0.02, 0.0), (0.02, 0.01), (0.0, 0.01)],
                    6,
                    30.0,
                    true,
                ),
            ),
            (
                "tube",
                tube(
                    &bezier(
                        Vec3::ZERO,
                        Vec3::Y,
                        Vec3::new(0.5, 1.5, 0.0),
                        Vec3::new(1.0, 1.5, 0.3),
                        12,
                    ),
                    0.05,
                    10,
                    true,
                ),
            ),
            (
                "extrude",
                extrude(&rounded_rect(1.0, 0.6, 0.1, 3), 0.05, 40.0),
            ),
            (
                "field",
                heightfield((0.0, 0.0), (1.0, 1.0), (4, 4), |x, z| {
                    (0.1 * (x * 3.0).sin() * z, Vec4::ONE)
                }),
            ),
        ];
        for (name, m) in shapes {
            assert!(!m.indices.is_empty(), "{name}");
            assert_eq!(check_winding(&m), 0, "{name} has inward triangles");
        }
    }

    #[test]
    fn rounded_box_keeps_its_size() {
        let b = rounded_box(Vec3::new(2.0, 1.0, 0.5), 0.1, 2, 0).aabb();
        assert!(b.max.abs_diff_eq(Vec3::new(1.0, 0.5, 0.25), 1e-5), "{b:?}");
    }
}
