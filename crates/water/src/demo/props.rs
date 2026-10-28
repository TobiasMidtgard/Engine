//! Detailed procedural models for the test area's props: boulders, boats, the jetty, buoys,
//! logs, the culvert and the people. Each returns meshes in the prop's own frame (meters,
//! y up), split into parts by [`Surface`], with UVs in meters for tiled textures (wood grain
//! runs along v) and linear vertex colors that tint the material (paint colors, baked
//! cavity darkening, algae, moss).
//!
//! Every part is a closed surface wound counterclockwise seen from outside, so it draws
//! correctly with back-face culling, unless [`Part::double_sided`] says otherwise.

use std::f32::consts::{FRAC_PI_2, PI, TAU};

use engine_core::math::{Mat4, Quat, Rng, Vec2, Vec3, Vec4, hash_u64, noise};
use engine_render::{Mesh, Vertex};

use super::materials::Surface;

/// One piece of a prop drawn with one material.
#[derive(Clone)]
pub struct Part {
    pub mesh: Mesh,
    pub surface: Surface,
    /// Draw without back-face culling (thin open sheets). Copy it to the material.
    pub double_sided: bool,
}

impl Part {
    /// A closed, single-sided part.
    pub fn new(mesh: Mesh, surface: Surface) -> Self {
        Self {
            mesh,
            surface,
            double_sided: false,
        }
    }
}

// ---------------------------------------------------------------------------------------------
// Geometry toolkit
// ---------------------------------------------------------------------------------------------

/// Collects geometry per surface while a prop is built.
#[derive(Default)]
struct Kit {
    parts: Vec<(Surface, Mesh)>,
}

impl Kit {
    /// The mesh that collects `surface`'s geometry.
    fn mesh(&mut self, surface: Surface) -> &mut Mesh {
        if let Some(i) = self.parts.iter().position(|(s, _)| *s == surface) {
            return &mut self.parts[i].1;
        }
        self.parts.push((surface, Mesh::default()));
        &mut self.parts.last_mut().expect("just pushed").1
    }

    /// Append `mesh` to `surface`'s part.
    fn add(&mut self, surface: Surface, mesh: &Mesh) {
        self.mesh(surface).merge(mesh);
    }

    fn finish(self) -> Vec<Part> {
        self.parts
            .into_iter()
            .filter(|(_, m)| !m.indices.is_empty())
            .map(|(s, m)| Part::new(m, s))
            .collect()
    }
}

/// Linear RGB from an sRGB hex color.
fn srgb(hex: u32) -> Vec3 {
    let c = |shift: u32| {
        let v = ((hex >> shift) & 0xff) as f32 / 255.0;
        if v <= 0.04045 {
            v / 12.92
        } else {
            ((v + 0.055) / 1.055).powf(2.4)
        }
    };
    Vec3::new(c(16), c(8), c(0))
}

fn rgba(c: Vec3) -> Vec4 {
    c.extend(1.0)
}

fn smooth(e0: f32, e1: f32, x: f32) -> f32 {
    let t = ((x - e0) / (e1 - e0)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// Polynomial smooth maximum (rounds the intersection of two distance fields by about `k`).
fn smax(a: f32, b: f32, k: f32) -> f32 {
    let h = (k - (a - b).abs()).max(0.0) / k.max(1e-9);
    a.max(b) + h * h * k * 0.25
}

fn fbm3(p: Vec3, seed: u64, octaves: u32) -> f32 {
    noise::fbm3(p.x as f64, p.y as f64, p.z as f64, seed, octaves, 2.0, 0.5)
}

/// One point of a parametric surface: position, UV in meters, linear RGBA color.
#[derive(Clone, Copy)]
struct Sample {
    p: Vec3,
    uv: Vec2,
    c: Vec4,
}

impl Sample {
    fn new(p: Vec3, uv: Vec2, c: Vec4) -> Self {
        Self { p, uv, c }
    }
}

/// `n + 1` evenly spaced values from `a` to `b`.
fn steps(a: f32, b: f32, n: usize) -> Vec<f32> {
    (0..=n).map(|i| a + (b - a) * i as f32 / n as f32).collect()
}

/// Tessellate the surface `f(a, b)` over the ascending parameter values `a` × `b`.
///
/// Normals are ∂f/∂a × ∂f/∂b (reversed with `flip`) from central differences of `f` itself,
/// so a smooth surface shades smoothly however coarsely it is tessellated; split a surface into
/// separate calls at creases. `wrap` marks `f` as periodic in a and/or b, with the first and
/// last values the same point (the seam is duplicated so UVs stay continuous).
fn surface(
    m: &mut Mesh,
    a: &[f32],
    b: &[f32],
    wrap: (bool, bool),
    flip: bool,
    f: impl Fn(f32, f32) -> Sample,
) {
    let (na, nb) = (a.len(), b.len());
    if na < 2 || nb < 2 {
        return;
    }
    let (amin, amax) = (a[0], a[na - 1]);
    let (bmin, bmax) = (b[0], b[nb - 1]);
    let ea = (amax - amin).abs() * 1e-3;
    let eb = (bmax - bmin).abs() * 1e-3;
    let diffs = |x: f32, y: f32| -> (Vec3, Vec3) {
        let (a0, a1) = if wrap.0 {
            (x - ea, x + ea)
        } else {
            ((x - ea).max(amin), (x + ea).min(amax))
        };
        let (b0, b1) = if wrap.1 {
            (y - eb, y + eb)
        } else {
            ((y - eb).max(bmin), (y + eb).min(bmax))
        };
        (f(a1, y).p - f(a0, y).p, f(x, b1).p - f(x, b0).p)
    };
    let base = m.vertices.len() as u32;
    for &y in b {
        for &x in a {
            let s = f(x, y);
            let (du, dv) = diffs(x, y);
            let mut n = du.cross(dv);
            if n.length() <= 1e-5 * du.length() * dv.length() || n.length() < 1e-16 {
                // A pole (a row or column collapses to a point): average the normals just
                // next to it.
                n = Vec3::ZERO;
                if du.length() <= dv.length() {
                    let y2 = if y - bmin < bmax - y {
                        y + 0.02 * (bmax - bmin)
                    } else {
                        y - 0.02 * (bmax - bmin)
                    };
                    for &x2 in &a[..na - wrap.0 as usize] {
                        let (du, dv) = diffs(x2, y2);
                        n += du.cross(dv).normalize_or_zero();
                    }
                } else {
                    let x2 = if x - amin < amax - x {
                        x + 0.02 * (amax - amin)
                    } else {
                        x - 0.02 * (amax - amin)
                    };
                    for &y2 in &b[..nb - wrap.1 as usize] {
                        let (du, dv) = diffs(x2, y2);
                        n += du.cross(dv).normalize_or_zero();
                    }
                }
            }
            let n = n.normalize_or(Vec3::Y) * if flip { -1.0 } else { 1.0 };
            m.vertices.push(Vertex {
                position: s.p.to_array(),
                normal: n.to_array(),
                uv: s.uv.to_array(),
                color: s.c.to_array(),
            });
        }
    }
    let idx = |i: usize, j: usize| base + (j * na + i) as u32;
    for j in 0..nb - 1 {
        for i in 0..na - 1 {
            let (q00, q10, q11, q01) = (idx(i, j), idx(i + 1, j), idx(i + 1, j + 1), idx(i, j + 1));
            let p = |k: u32| Vec3::from(m.vertices[k as usize].position);
            let tris = if p(q00).distance_squared(p(q11)) <= p(q10).distance_squared(p(q01)) {
                [[q00, q10, q11], [q00, q11, q01]]
            } else {
                [[q00, q10, q01], [q10, q11, q01]]
            };
            for [t0, t1, t2] in tris {
                if (p(t1) - p(t0)).cross(p(t2) - p(t0)).length_squared() < 1e-18 {
                    continue;
                }
                if flip {
                    m.indices.extend_from_slice(&[t0, t2, t1]);
                } else {
                    m.indices.extend_from_slice(&[t0, t1, t2]);
                }
            }
        }
    }
}

/// Linear interpolation along a polyline at a fractional index `t` (clamped).
fn poly_at<T>(pts: &[T], t: f32) -> T
where
    T: Copy + std::ops::Mul<f32, Output = T> + std::ops::Add<Output = T>,
{
    let n = pts.len();
    let t = t.clamp(0.0, (n - 1) as f32);
    let i = (t.floor() as usize).min(n.saturating_sub(2));
    let f = t - i as f32;
    pts[i] * (1.0 - f) + pts[(i + 1).min(n - 1)] * f
}

/// Cumulative lengths along a polyline.
fn arc_lengths<T: Copy>(pts: &[T], dist: impl Fn(T, T) -> f32) -> Vec<f32> {
    let mut out = Vec::with_capacity(pts.len());
    let mut s = 0.0;
    for (i, p) in pts.iter().enumerate() {
        if i > 0 {
            s += dist(pts[i - 1], *p);
        }
        out.push(s);
    }
    out
}

/// Revolve `prof` ((radius, y) points, bottom to top along the outside) about +y with `segs`
/// segments. Shades smooth: split the profile at creases. UVs: u = angle × `uv_r` meters,
/// v = distance along the profile from `v0`. `color` gets the position.
fn lathe(
    m: &mut Mesh,
    prof: &[Vec2],
    segs: usize,
    uv_r: f32,
    v0: f32,
    color: &dyn Fn(Vec3) -> Vec4,
) {
    let len = arc_lengths(prof, |a, b| a.distance(b));
    let t: Vec<f32> = (0..prof.len()).map(|i| i as f32).collect();
    surface(
        m,
        &steps(0.0, 1.0, segs),
        &t,
        (true, false),
        false,
        |a, t| {
            let q = poly_at(prof, t);
            let th = a * TAU;
            let p = Vec3::new(q.x * th.cos(), q.y, -q.x * th.sin());
            Sample::new(
                p,
                Vec2::new(a * TAU * uv_r, v0 + poly_at(&len, t)),
                color(p),
            )
        },
    );
}

/// Rotation-minimizing frames (double reflection) along a polyline: a unit normal per point,
/// perpendicular to the tangent. `closed` spreads the loop's leftover twist along it so the
/// frame comes back to where it started.
fn rmf(pts: &[Vec3], up: Vec3, closed: bool) -> Vec<(Vec3, Vec3)> {
    let n = pts.len();
    let tangent = |i: usize| -> Vec3 {
        let (a, b) = if closed {
            (pts[(i + n - 1) % n], pts[(i + 1) % n])
        } else {
            (pts[i.saturating_sub(1)], pts[(i + 1).min(n - 1)])
        };
        (b - a).normalize_or(Vec3::X)
    };
    let t0 = tangent(0);
    let mut r = up - t0 * up.dot(t0);
    if r.length_squared() < 1e-8 {
        r = t0.any_orthonormal_vector();
    }
    let mut out = vec![(t0, r.normalize())];
    let count = if closed { n + 1 } else { n };
    for k in 1..count {
        let i = k % n;
        let (ti, ri) = out[k - 1];
        let v1 = pts[i] - pts[k - 1];
        let c1 = v1.dot(v1);
        let (rl, tl) = if c1 < 1e-16 {
            (ri, ti)
        } else {
            (
                ri - v1 * (2.0 / c1 * v1.dot(ri)),
                ti - v1 * (2.0 / c1 * v1.dot(ti)),
            )
        };
        let t = tangent(i);
        let v2 = t - tl;
        let c2 = v2.dot(v2);
        let r = if c2 < 1e-16 {
            rl
        } else {
            rl - v2 * (2.0 / c2 * v2.dot(rl))
        };
        out.push((t, (r - t * r.dot(t)).normalize_or(ri)));
    }
    if closed {
        // Rotate the frames progressively so the last matches the first.
        let (t, r_end) = out[n];
        let r0 = out[0].1;
        let angle = r_end.cross(r0).dot(t).atan2(r_end.dot(r0));
        out.truncate(n);
        for (k, (t, r)) in out.iter_mut().enumerate() {
            let a = angle * k as f32 / n as f32;
            *r = Quat::from_axis_angle(*t, a) * *r;
        }
    }
    out
}

/// Sweep a circle of radius `radius(s)` (s = 0..1 along the path) along `pts` with `segs`
/// sides. A `closed` path loops back to its start; an open one gets flat end caps.
fn tube(
    m: &mut Mesh,
    pts: &[Vec3],
    segs: usize,
    closed: bool,
    radius: &dyn Fn(f32) -> f32,
    color: Vec4,
) {
    let n = pts.len();
    let frames = rmf(pts, Vec3::Y, closed);
    let mut path: Vec<Vec3> = pts.to_vec();
    let mut fr = frames.clone();
    if closed {
        path.push(pts[0]);
        fr.push(frames[0]);
    }
    let len = arc_lengths(&path, |a, b| a.distance(b));
    let total = len[len.len() - 1].max(1e-6);
    let r_ref = radius(0.5);
    let b: Vec<f32> = (0..path.len()).map(|i| i as f32).collect();
    let at = |t: f32| -> (Vec3, Vec3, Vec3, f32) {
        let tt = if closed { t.rem_euclid(n as f32) } else { t };
        let p = poly_at(&path, tt);
        let tan = poly_at(&fr.iter().map(|f| f.0).collect::<Vec<_>>(), tt).normalize_or(Vec3::X);
        let nor = poly_at(&fr.iter().map(|f| f.1).collect::<Vec<_>>(), tt);
        let nor = (nor - tan * nor.dot(tan)).normalize_or(Vec3::Y);
        (p, tan, nor, poly_at(&len, tt))
    };
    surface(
        m,
        &steps(0.0, 1.0, segs),
        &b,
        (true, closed),
        false,
        |a, t| {
            let (p, tan, nor, s) = at(t);
            let bin = tan.cross(nor);
            let th = a * TAU;
            let r = radius(s / total);
            let q = p + (nor * th.cos() + bin * th.sin()) * r;
            Sample::new(q, Vec2::new(a * TAU * r_ref, s), color)
        },
    );
    if !closed {
        for (end, sign) in [(0usize, -1.0f32), (n - 1, 1.0)] {
            let (p, tan, nor, _) = at(end as f32);
            let r = radius(end as f32 / (n - 1) as f32);
            let bin = tan.cross(nor);
            let ring: Vec<Vec3> = (0..segs)
                .map(|k| {
                    let th = k as f32 / segs as f32 * TAU * sign;
                    p + (nor * th.cos() + bin * th.sin()) * r
                })
                .collect();
            if r > 1e-5 {
                flat_polygon(m, &ring, tan * sign, color);
            }
        }
    }
}

/// Points of a circular arc in the plane spanned by `u` and `v` around `center`.
fn arc(center: Vec3, u: Vec3, v: Vec3, radius: f32, a0: f32, a1: f32, n: usize) -> Vec<Vec3> {
    (0..=n)
        .map(|i| {
            let a = a0 + (a1 - a0) * i as f32 / n as f32;
            center + (u * a.cos() + v * a.sin()) * radius
        })
        .collect()
}

/// Two unit axes spanning the plane with `normal`, with `t × b = normal`: `b` points up on
/// vertical faces and `t` along x on horizontal ones.
fn face_axes(normal: Vec3) -> (Vec3, Vec3) {
    let n = normal.normalize();
    if n.y.abs() > 0.7 {
        let t = (Vec3::X - n * n.x).normalize();
        (t, n.cross(t))
    } else {
        let b = (Vec3::Y - n * n.y).normalize();
        (b.cross(n), b)
    }
}

/// Ear-clipping triangulation of a simple polygon (either winding); the triangles come out
/// counterclockwise.
fn triangulate(poly: &[Vec2]) -> Vec<[usize; 3]> {
    let n = poly.len();
    let area: f32 = (0..n)
        .map(|i| poly[i].perp_dot(poly[(i + 1) % n]))
        .sum::<f32>();
    let mut idx: Vec<usize> = if area >= 0.0 {
        (0..n).collect()
    } else {
        (0..n).rev().collect()
    };
    let mut out = Vec::with_capacity(n.saturating_sub(2));
    let mut guard = 0;
    while idx.len() > 3 && guard < 4 * n * n {
        guard += 1;
        let k = idx.len();
        let mut clipped = false;
        for i in 0..k {
            let (ia, ib, ic) = (idx[(i + k - 1) % k], idx[i], idx[(i + 1) % k]);
            let (a, b, c) = (poly[ia], poly[ib], poly[ic]);
            if (b - a).perp_dot(c - b) <= 1e-12 {
                continue;
            }
            let inside = idx.iter().any(|&j| {
                if j == ia || j == ib || j == ic {
                    return false;
                }
                let p = poly[j];
                (b - a).perp_dot(p - a) > 0.0
                    && (c - b).perp_dot(p - b) > 0.0
                    && (a - c).perp_dot(p - c) > 0.0
            });
            if inside {
                continue;
            }
            out.push([ia, ib, ic]);
            idx.remove(i);
            clipped = true;
            break;
        }
        if !clipped {
            // Degenerate leftovers: fan them.
            break;
        }
    }
    for i in 1..idx.len().saturating_sub(1) {
        out.push([idx[0], idx[i], idx[i + 1]]);
    }
    out
}

/// A flat polygon (planar points in order, either winding) facing `normal`, with planar UVs
/// in meters.
fn flat_polygon(m: &mut Mesh, pts: &[Vec3], normal: Vec3, color: Vec4) {
    let n = normal.normalize();
    let (t, b) = face_axes(n);
    let flat: Vec<Vec2> = pts.iter().map(|p| Vec2::new(p.dot(t), p.dot(b))).collect();
    let base = m.vertices.len() as u32;
    for (p, q) in pts.iter().zip(&flat) {
        m.vertices
            .push(Vertex::new(*p, n, Vec2::new(q.x, -q.y)).with_color(color));
    }
    for [a, b, c] in triangulate(&flat) {
        m.indices
            .extend_from_slice(&[base + a as u32, base + b as u32, base + c as u32]);
    }
}

/// Emit a flat convex polygon (points in any order) facing `normal`, with planar UVs.
fn convex_face(m: &mut Mesh, pts: &[Vec3], normal: Vec3, color: Vec4) {
    let center = pts.iter().copied().sum::<Vec3>() / pts.len() as f32;
    let (t, b) = face_axes(normal);
    let mut sorted: Vec<Vec3> = pts.to_vec();
    sorted.sort_by(|p, q| {
        let ap = (*p - center).dot(b).atan2((*p - center).dot(t));
        let aq = (*q - center).dot(b).atan2((*q - center).dot(t));
        ap.total_cmp(&aq)
    });
    flat_polygon(m, &sorted, normal, color);
}

/// Extrude a 2D outline (in the xy plane) from z = `z0` to `z1` and move it by `xf`: flat
/// caps and one flat quad per edge.
fn prism(m: &mut Mesh, outline: &[Vec2], z0: f32, z1: f32, xf: Mat4, color: Vec4) {
    let mut local = Mesh::default();
    let n = outline.len();
    let area: f32 = (0..n)
        .map(|i| outline[i].perp_dot(outline[(i + 1) % n]))
        .sum();
    let ccw: Vec<Vec2> = if area >= 0.0 {
        outline.to_vec()
    } else {
        outline.iter().rev().copied().collect()
    };
    let mut u = 0.0;
    for i in 0..n {
        let (a, b) = (ccw[i], ccw[(i + 1) % n]);
        let e = b - a;
        let len = e.length();
        if len < 1e-7 {
            continue;
        }
        let nrm = Vec3::new(e.y, -e.x, 0.0) / len;
        let base = local.vertices.len() as u32;
        for (p, uv) in [
            (a.extend(z0), Vec2::new(u, z0)),
            (b.extend(z0), Vec2::new(u + len, z0)),
            (b.extend(z1), Vec2::new(u + len, z1)),
            (a.extend(z1), Vec2::new(u, z1)),
        ] {
            local
                .vertices
                .push(Vertex::new(p, nrm, uv).with_color(color));
        }
        local
            .indices
            .extend_from_slice(&[base, base + 1, base + 2, base, base + 2, base + 3]);
        u += len;
    }
    let top: Vec<Vec3> = ccw.iter().map(|p| p.extend(z1)).collect();
    flat_polygon(&mut local, &top, Vec3::Z, color);
    let bottom: Vec<Vec3> = ccw.iter().rev().map(|p| p.extend(z0)).collect();
    flat_polygon(&mut local, &bottom, Vec3::NEG_Z, color);
    local.transform(&xf);
    m.merge(&local);
}

/// A box of half-extents `half` with chamfered edges (`c` meters), moved by `xf`, with
/// per-face planar UVs in meters (before the move).
fn chamfer_box(m: &mut Mesh, half: Vec3, c: f32, xf: Mat4, color: Vec4) {
    let mut local = Mesh::default();
    let c = c.min(half.min_element() * 0.99);
    let corner = |s: [f32; 3], axis: usize| -> Vec3 {
        let mut p = Vec3::new(
            s[0] * (half.x - c),
            s[1] * (half.y - c),
            s[2] * (half.z - c),
        );
        p[axis] = s[axis] * half[axis];
        p
    };
    let signs = [-1.0f32, 1.0];
    for axis in 0..3 {
        for s in signs {
            let mut n = Vec3::ZERO;
            n[axis] = s;
            let mut pts = Vec::new();
            for a in signs {
                for b in signs {
                    let mut sg = [0.0; 3];
                    sg[axis] = s;
                    sg[(axis + 1) % 3] = a;
                    sg[(axis + 2) % 3] = b;
                    pts.push(corner(sg, axis));
                }
            }
            convex_face(&mut local, &pts, n, color);
        }
    }
    if c > 0.0 {
        for (a1, a2) in [(0usize, 1usize), (1, 2), (2, 0)] {
            let a3 = 3 - a1 - a2;
            for s1 in signs {
                for s2 in signs {
                    let mut n = Vec3::ZERO;
                    n[a1] = s1;
                    n[a2] = s2;
                    let mut pts = Vec::new();
                    for s3 in signs {
                        let mut sg = [0.0; 3];
                        sg[a1] = s1;
                        sg[a2] = s2;
                        sg[a3] = s3;
                        pts.push(corner(sg, a1));
                        pts.push(corner(sg, a2));
                    }
                    convex_face(&mut local, &pts, n.normalize(), color);
                }
            }
        }
        for sx in signs {
            for sy in signs {
                for sz in signs {
                    let pts: Vec<Vec3> = (0..3).map(|axis| corner([sx, sy, sz], axis)).collect();
                    convex_face(&mut local, &pts, Vec3::new(sx, sy, sz).normalize(), color);
                }
            }
        }
    }
    local.transform(&xf);
    m.merge(&local);
}

fn moved(mut mesh: Mesh, xf: Mat4) -> Mesh {
    mesh.transform(&xf);
    mesh
}

// ---------------------------------------------------------------------------------------------
// Boulder
// ---------------------------------------------------------------------------------------------

/// A plane that cuts the boulder: `n · p ≤ d`, rounded by `k`, its face displaced by noise.
struct Cut {
    n: Vec3,
    d: f32,
    k: f32,
    rough: f32,
    fracture: bool,
}

struct Rock {
    size: f32,
    axes: Vec3,
    /// Superellipsoid exponent of the base block (2 = ellipsoid, higher = boxier).
    power: f32,
    cuts: Vec<Cut>,
    /// A quartz vein: plane normal, offset and half-width (meters).
    vein: Option<(Vec3, f32, f32)>,
    seed: u64,
}

impl Rock {
    fn new(seed: u64, size: f32, edge: f32) -> Self {
        let mut rng = Rng::new(hash_u64(seed ^ 0x6b0b_1de5));
        let axes = Vec3::new(
            0.5 * size * rng.range(0.98, 1.12),
            size * rng.range(0.32, 0.44),
            0.5 * size * rng.range(0.7, 1.0),
        );
        let support = |n: Vec3| (n * axes).length();
        let mut cuts = Vec::new();
        // Weathered joint planes: big, flat facets with rounded edges. Steep ones trim the
        // sides; the few that face up only shave the top, so the rock stays chunky.
        for _ in 0..rng.range_i32(4, 8) {
            let mut n = rng.unit_vector();
            n.y = n.y.clamp(-0.2, 0.8);
            let n = n.normalize();
            let depth = if n.y > 0.45 {
                rng.range(0.8, 0.92)
            } else {
                rng.range(0.66, 0.88)
            };
            cuts.push(Cut {
                n,
                d: support(n) * depth,
                k: (size * rng.range(0.06, 0.12)).max(edge * 1.6),
                rough: rng.range(0.004, 0.01),
                fracture: false,
            });
        }
        // A fresh fracture face: flat, crisp-edged, granular.
        for _ in 0..rng.range_i32(1, 3) {
            let a = rng.range(0.0, TAU);
            let n = Vec3::new(a.cos(), rng.range(-0.1, 0.45), a.sin()).normalize();
            cuts.push(Cut {
                n,
                d: support(n) * rng.range(0.62, 0.8),
                k: edge * 1.4,
                rough: 0.005,
                fracture: true,
            });
        }
        let vein = rng.chance(0.6).then(|| {
            let n = rng.unit_vector();
            (
                n,
                rng.range(-0.2, 0.2) * size,
                size * rng.range(0.006, 0.014),
            )
        });
        let mut rock = Rock {
            size,
            axes,
            power: rng.range(2.3, 3.4),
            cuts,
            vein,
            seed,
        };
        // Small chips off the upper edges and corners.
        for _ in 0..rng.range_i32(4, 9) {
            let mut d = rng.unit_vector();
            d.y = d.y.abs() * 0.8 + 0.1;
            let q = rock.hit(Vec3::ZERO, d.normalize());
            let n = (rock.gradient(q) + rng.unit_vector() * 0.35).normalize();
            let d = q.dot(n) - size * rng.range(0.015, 0.035);
            // Only nick the surface: never a plane that reaches toward the middle.
            if d < q.length() * 0.6 {
                continue;
            }
            rock.cuts.push(Cut {
                n,
                d,
                k: edge * 1.3,
                rough: 0.002,
                fracture: true,
            });
        }
        // The flat bed it rests on.
        rock.cuts.push(Cut {
            n: Vec3::NEG_Y,
            d: 0.15 * size,
            k: size * 0.05,
            rough: 0.01,
            fracture: false,
        });
        rock
    }

    /// How much of the quartz vein is at `p` (0..1).
    fn vein_at(&self, p: Vec3) -> f32 {
        let Some((n, d, w)) = self.vein else {
            return 0.0;
        };
        let wobble = fbm3(p * (4.0 / self.size), self.seed ^ 0x1e, 2) * w * 1.5;
        1.0 - smooth(w * 0.5, w, (p.dot(n) - d + wobble).abs())
    }

    /// Signed distance (approximately), meters: negative inside.
    fn sdf(&self, p: Vec3) -> f32 {
        let s = self.size;
        let q = p / s;
        let broad = fbm3(q * 2.1, self.seed, 3);
        let fine = fbm3(q * 7.5 + Vec3::splat(3.7), self.seed ^ 0x55, 3);
        // A rounded block ("woolsack" weathering), its shape warped by noise.
        let warp = Vec3::new(
            fbm3(q * 1.3 + Vec3::splat(9.1), self.seed ^ 0x21, 2),
            fbm3(q * 1.3 + Vec3::splat(5.3), self.seed ^ 0x22, 2),
            fbm3(q * 1.3 + Vec3::splat(1.7), self.seed ^ 0x23, 2),
        );
        let w = (p + warp * (0.07 * s)) / self.axes;
        let e = self.power;
        let k = (w.x.abs().powf(e) + w.y.abs().powf(e) + w.z.abs().powf(e)).powf(1.0 / e);
        let mut f = (k - 1.0) * self.axes.min_element() * 1.2;
        f -= s * 0.04 * broad;
        for c in &self.cuts {
            let rough = if c.fracture {
                0.6 - fine.abs() * 1.6
            } else {
                broad
            };
            f = smax(f, p.dot(c.n) - c.d - s * c.rough * rough, c.k);
        }
        // Mid-scale relief: shallow dents, bulges and a few sharper ridges.
        let ridge = 1.0 - fbm3(q * 4.2 + Vec3::splat(8.0), self.seed ^ 0x66, 2).abs() * 2.0;
        f - s * 0.012 * fine - s * 0.006 * ridge.max(0.0).powi(3)
    }

    fn gradient(&self, p: Vec3) -> Vec3 {
        let e = self.size * 2e-3;
        Vec3::new(
            self.sdf(p + Vec3::X * e) - self.sdf(p - Vec3::X * e),
            self.sdf(p + Vec3::Y * e) - self.sdf(p - Vec3::Y * e),
            self.sdf(p + Vec3::Z * e) - self.sdf(p - Vec3::Z * e),
        )
        .normalize_or(Vec3::Y)
    }

    /// Where the ray from `from` along `dir` leaves the rock (bisection).
    fn hit(&self, from: Vec3, dir: Vec3) -> Vec3 {
        let (mut lo, mut hi) = (0.0f32, self.size * 1.2);
        for _ in 0..18 {
            let mid = 0.5 * (lo + hi);
            if self.sdf(from + dir * mid) < 0.0 {
                lo = mid;
            } else {
                hi = mid;
            }
        }
        from + dir * (0.5 * (lo + hi))
    }

    /// Ambient occlusion from the distance field: 1 open, 0 deep in a crevice.
    fn occlusion(&self, p: Vec3, n: Vec3) -> f32 {
        let mut occ = 0.0;
        let mut w = 1.0;
        for i in 1..=4 {
            let d = self.size * 0.02 * i as f32;
            occ += w * (d - self.sdf(p + n * d)).max(0.0) / d;
            w *= 0.65;
        }
        (1.0 - occ * 1.1).clamp(0.0, 1.0)
    }

    fn color(&self, p: Vec3, n: Vec3, tint: Vec3, curvature: f32) -> Vec4 {
        let s = self.size;
        let ao = self.occlusion(p, n);
        let mut c = tint * (0.3 + 0.7 * ao);
        // Mottled weathering, and fresh-looking, slightly lighter edges where it is worn.
        let mottle = fbm3(p * 3.0, self.seed ^ 0x77, 3);
        c *= 0.88 + 0.2 * mottle + 0.25 * curvature.clamp(0.0, 0.4);
        // The quartz vein.
        c = c.lerp(Vec3::new(0.93, 0.91, 0.86), self.vein_at(p) * 0.85);
        // Soil and water staining toward the bed.
        let stain = 1.0 - smooth(-0.15 * s, 0.03 * s, p.y);
        c = c.lerp(c * Vec3::new(0.6, 0.52, 0.42), stain * 0.85);
        // Moss on the tops and in sheltered spots, lichen spots on the open faces, a little
        // iron staining.
        let up = n.y;
        let patch = fbm3(p * 2.2 + Vec3::splat(11.0), self.seed ^ 0x99, 4);
        let moss = smooth(0.3, 0.8, up) * smooth(-0.1, 0.2, patch) * (0.5 + 0.5 * ao);
        c = c.lerp(Vec3::new(0.26, 0.36, 0.1), moss.clamp(0.0, 0.9));
        let spots = fbm3(p * 9.0, self.seed ^ 0x3c, 2);
        let lichen = smooth(0.22, 0.36, spots) * smooth(-0.4, 0.3, up) * (1.0 - moss);
        c = c.lerp(Vec3::new(0.85, 0.87, 0.72), lichen * 0.65);
        let orange = smooth(
            0.3,
            0.4,
            fbm3(p * 11.0 + Vec3::splat(2.0), self.seed ^ 0x4d, 2),
        );
        c = c.lerp(
            Vec3::new(0.85, 0.5, 0.18),
            orange * 0.5 * smooth(0.0, 0.5, up),
        );
        let rust = smooth(
            0.3,
            0.45,
            fbm3(p * 5.0 + Vec3::splat(4.0), self.seed ^ 0xaa, 2),
        );
        c = c.lerp(c * Vec3::new(1.12, 0.88, 0.6), rust * 0.45 * (1.0 - moss));
        rgba(c.min(Vec3::ONE))
    }
}

/// A granite boulder about `size` meters across, widest around y = 0, with a flat bed down to
/// about y = -0.15 × `size` (half-bury it). Weathered joint planes, a fresh fracture face,
/// chipped edges, a quartz vein and rounded erosion, vertex-colored with crevice darkening,
/// soil stains at the bed, moss on top and lichen. At most 4 000 triangles below 1.5 m,
/// 10 000 above; every seed gives a different rock.
pub fn boulder(seed: u64, size: f32) -> Vec<Part> {
    let n: i32 = if size < 1.5 { 18 } else { 28 };
    // Typical distance between vertices: edges are never rounded tighter than the mesh can
    // follow.
    let edge = size * 2.7 / (4 * n) as f32;
    let rock = Rock::new(seed, size, edge);
    let mut rng = Rng::new(hash_u64(seed ^ 0x7155));
    let tint = Vec3::new(0.8, 0.79, 0.76).lerp(Vec3::new(0.86, 0.75, 0.69), rng.f32())
        * rng.range(0.85, 1.0);
    let center = Vec3::new(0.0, 0.02 * size, 0.0);
    // Rays from near the center through a squat ellipsoid like the rock's, so vertices spread
    // evenly over the surface rather than crowding onto the hidden bed.
    let reference = |d: Vec3| -> Vec3 {
        let y = if d.y < 0.0 {
            d.y * 0.17 * size
        } else {
            d.y * rock.axes.y
        };
        Vec3::new(d.x * rock.axes.x, y, d.z * rock.axes.z)
    };
    // A cube-sphere with the cube's edges welded: unique vertices keyed by lattice point.
    let faces = [
        (Vec3::X, Vec3::Y, Vec3::Z),
        (Vec3::NEG_X, Vec3::Z, Vec3::Y),
        (Vec3::Y, Vec3::Z, Vec3::X),
        (Vec3::NEG_Y, Vec3::X, Vec3::Z),
        (Vec3::Z, Vec3::X, Vec3::Y),
        (Vec3::NEG_Z, Vec3::Y, Vec3::X),
    ];
    let mut keys = std::collections::HashMap::new();
    let mut pos: Vec<Vec3> = Vec::new();
    let mut face_ids: Vec<Vec<usize>> = Vec::new();
    for (e, t1, t2) in faces {
        let mut ids = Vec::new();
        for j in 0..=n {
            for i in 0..=n {
                let (u, v) = (2 * i - n, 2 * j - n);
                let c = e * n as f32 + t1 * u as f32 + t2 * v as f32;
                let key = (c.x.round() as i32, c.y.round() as i32, c.z.round() as i32);
                let id = *keys.entry(key).or_insert_with(|| {
                    let q = |k: i32| (k as f32 / n as f32 * PI * 0.25).tan();
                    let d = (e + t1 * q(u) + t2 * q(v)).normalize();
                    pos.push(rock.hit(center, (reference(d) - center).normalize()));
                    pos.len() - 1
                });
                ids.push(id);
            }
        }
        face_ids.push(ids);
    }
    let mut neighbors: Vec<Vec<usize>> = vec![Vec::new(); pos.len()];
    let row = (n + 1) as usize;
    for ids in &face_ids {
        for j in 0..=n as usize {
            for i in 0..=n as usize {
                let a = ids[j * row + i];
                for (di, dj) in [(1usize, 0usize), (0, 1)] {
                    if i + di <= n as usize && j + dj <= n as usize {
                        let b = ids[(j + dj) * row + i + di];
                        if a != b && !neighbors[a].contains(&b) {
                            neighbors[a].push(b);
                            neighbors[b].push(a);
                        }
                    }
                }
            }
        }
    }
    // Pull vertices toward the edges (where the normal turns fastest) so creases are traced
    // by the mesh instead of zigzagging across it, then snap them back onto the surface.
    let mut nrm: Vec<Vec3> = pos.iter().map(|p| rock.gradient(*p)).collect();
    let curvature = |nrm: &[Vec3]| -> Vec<f32> {
        (0..nrm.len())
            .map(|i| {
                let k = &neighbors[i];
                k.iter().map(|&j| 1.0 - nrm[i].dot(nrm[j])).sum::<f32>() / k.len().max(1) as f32
            })
            .collect()
    };
    for _ in 0..3 {
        let curv = curvature(&nrm);
        let next: Vec<Vec3> = (0..pos.len())
            .map(|i| {
                let (mut sum, mut wsum, mut plain) = (Vec3::ZERO, 0.0, Vec3::ZERO);
                for &j in &neighbors[i] {
                    let w = 1.0 + 12.0 * curv[j].min(0.15);
                    sum += pos[j] * w;
                    wsum += w;
                    plain += pos[j];
                }
                let plain = plain / neighbors[i].len() as f32;
                // Blend the edge-seeking target with an even spread so flat faces between
                // edges keep well-shaped triangles.
                let target = (sum / wsum) * 0.6 + plain * 0.4;
                let mut p = pos[i] + (target - pos[i]) * 0.5;
                p -= nrm[i] * (p - pos[i]).dot(nrm[i]);
                for _ in 0..2 {
                    p -= nrm[i] * rock.sdf(p);
                }
                p
            })
            .collect();
        pos = next;
        nrm = pos.iter().map(|p| rock.gradient(*p)).collect();
    }
    let curv = curvature(&nrm);
    let colors: Vec<Vec4> = (0..pos.len())
        .map(|i| rock.color(pos[i], nrm[i], tint, curv[i] * 8.0))
        .collect();
    // Stand the rock centered over the origin (its bed stays where it is).
    let (lo, hi) = pos.iter().fold((Vec3::MAX, Vec3::MIN), |(lo, hi), p| {
        (lo.min(*p), hi.max(*p))
    });
    let shift = Vec3::new(lo.x + hi.x, 0.0, lo.z + hi.z) * 0.5;
    for p in &mut pos {
        *p -= shift;
    }
    // Triangles, split along the diagonal that follows the surface's fold.
    let mut tris: Vec<[usize; 3]> = Vec::new();
    for ids in &face_ids {
        for j in 0..n as usize {
            for i in 0..n as usize {
                let (a, b) = (ids[j * row + i], ids[j * row + i + 1]);
                let (c, d) = (ids[(j + 1) * row + i + 1], ids[(j + 1) * row + i]);
                if nrm[a].dot(nrm[c]) >= nrm[b].dot(nrm[d])
                    || pos[a].distance(pos[c]) < pos[b].distance(pos[d]) * 0.8
                {
                    tris.extend_from_slice(&[[a, b, c], [a, c, d]]);
                } else {
                    tris.extend_from_slice(&[[a, b, d], [b, c, d]]);
                }
            }
        }
    }
    // Box-project the UVs by each triangle's own facing (six charts, seams duplicated), so
    // steep facets never stretch the texture more than 1.7×.
    let mut m = Mesh::default();
    let mut chart_ids: Vec<Vec<u32>> = vec![vec![u32::MAX; pos.len()]; 6];
    for t in tris {
        let fnrm = (pos[t[1]] - pos[t[0]]).cross(pos[t[2]] - pos[t[0]]);
        let axis = fnrm.abs().max_position();
        let chart = axis * 2 + (fnrm[axis] < 0.0) as usize;
        // `faces` is ordered +x, -x, +y, -y, +z, -z: the same order as the charts.
        let (_, t1, t2) = faces[chart];
        for &id in &t {
            if chart_ids[chart][id] == u32::MAX {
                chart_ids[chart][id] = m.vertices.len() as u32;
                let p = pos[id];
                m.vertices.push(
                    Vertex::new(p, nrm[id], Vec2::new(p.dot(t1), p.dot(t2))).with_color(colors[id]),
                );
            }
            m.indices.push(chart_ids[chart][id]);
        }
    }
    vec![Part::new(m, Surface::Rock)]
}

// ---------------------------------------------------------------------------------------------
// Rowboat
// ---------------------------------------------------------------------------------------------

/// Planking thickness of the rowboat, meters.
const ROW_T: f32 = 0.015;
/// Number of strakes (planks) per side.
const ROW_STRAKES: usize = 6;

/// The rowboat's molded hull surface (inside of the planking): `u` runs from the transom
/// (0) to the stem (1), `s` from the keel (0) to the sheer (1). Starboard side (z ≥ 0).
fn row_hull(u: f32, s: f32) -> Vec3 {
    let u = u.clamp(0.0, 1.0);
    let s = s.clamp(0.0, 1.0);
    // Half-breadth at the sheer: widest a little aft of midships, a 0.8 m transom, fine bow.
    let (umax, bmax, b0) = (0.45, 0.635, 0.4);
    let b = if u <= umax {
        bmax - (bmax - b0) * (1.0 - u / umax).powi(2)
    } else {
        bmax * ((u - umax) / (1.0 - umax) * FRAC_PI_2).cos()
    };
    // Keel (rabbet) line and sheer line heights.
    let keel = if u < 0.5 {
        -0.27 + 0.09 * (1.0 - u / 0.5).powi(2)
    } else {
        -0.27 + 0.05 * ((u - 0.5) / 0.5).powi(2)
    };
    let sheer = if u < 0.42 {
        0.29 + 0.07 * ((0.42 - u) / 0.42).powi(2)
    } else {
        0.29 + 0.17 * ((u - 0.42) / 0.58).powi(2)
    };
    // Section shape: a little deadrise, a firm bilge and flared topsides, turning into a
    // V toward the bow and collapsing onto the stem's curve at u = 1.
    let dead = 0.12 * (1.0 - u.powi(4));
    let y_frac = (1.0 - (s * FRAC_PI_2).cos()) * (1.0 - dead) + dead * s;
    let round = (s * FRAC_PI_2 * 0.86).sin() / (FRAC_PI_2 * 0.86).sin();
    let vee = s.powf(1.15);
    let z_frac = round + (vee - round) * smooth(0.5, 1.0, u);
    // The transom rakes aft at the top (and is a plane); the stem curves up from the forefoot.
    let x_aft = -1.70 - 0.10 * y_frac;
    let x_fwd = 1.45 + 0.35 * (s * FRAC_PI_2).sin();
    Vec3::new(
        x_aft + (x_fwd - x_aft) * u,
        keel + (sheer - keel) * y_frac,
        b * z_frac,
    )
}

/// Outward unit normal of the starboard molded surface.
fn row_normal(u: f32, s: f32) -> Vec3 {
    let e = 1e-3;
    let du = row_hull(u + e, s) - row_hull(u - e, s);
    let ds = row_hull(u, s + e) - row_hull(u, s - e);
    let n = du.cross(ds);
    if n.length_squared() > 1e-14 {
        n.normalize()
    } else {
        row_normal((u - 0.01).max(0.0), (s + 0.01).min(1.0))
    }
}

/// Length of the section from keel to sheer at `u`, meters.
fn row_girth(u: f32) -> f32 {
    (0..24)
        .map(|i| row_hull(u, (i + 1) as f32 / 24.0).distance(row_hull(u, i as f32 / 24.0)))
        .sum()
}

/// `u` of the station through `x` at height fraction `s` (stations are not plane: the stem
/// and transom are raked).
fn row_u_at(x: f32, s: f32) -> f32 {
    let (a, b) = (row_hull(0.0, s).x, row_hull(1.0, s).x);
    ((x - a) / (b - a)).clamp(0.0, 1.0)
}

/// Half-breadth of the molded surface at `x` and height `y` (meters), and the `s` there.
fn row_half_breadth(x: f32, y: f32) -> (f32, f32) {
    let (mut lo, mut hi) = (0.0f32, 1.0f32);
    for _ in 0..30 {
        let mid = 0.5 * (lo + hi);
        if row_hull(row_u_at(x, mid), mid).y < y {
            lo = mid;
        } else {
            hi = mid;
        }
    }
    let s = 0.5 * (lo + hi);
    (row_hull(row_u_at(x, s), s).z, s)
}

/// Emit a surface on one side of the boat: `side` = 1 starboard, -1 port (mirrored in z,
/// which also flips the winding).
fn sided(
    m: &mut Mesh,
    a: &[f32],
    b: &[f32],
    wrap: (bool, bool),
    flip: bool,
    side: f32,
    f: impl Fn(f32, f32) -> Sample,
) {
    surface(m, a, b, wrap, flip ^ (side < 0.0), |x, y| {
        let mut s = f(x, y);
        s.p.z *= side;
        s
    });
}

/// Paint for the outside of strake `k` and its tint at `p`: antifouling red below the
/// waterline, white topsides, a green sheer strake.
fn row_paint(k: usize) -> Surface {
    match k {
        0..=2 => Surface::PaintedWoodRed,
        5 => Surface::PaintedWoodGreen,
        _ => Surface::PaintedWood,
    }
}

/// A rounded rectangle traced counterclockwise by `a` in 0..1 (half extents `h`, corner
/// radius `r`), starting at the middle of the +x side.
fn rounded_rect(a: f32, h: Vec2, r: f32) -> Vec2 {
    let r = r.min(h.x).min(h.y);
    let (sx, sy) = (h.x - r, h.y - r);
    let quarter = r * FRAC_PI_2;
    let perim = 4.0 * (sx + sy) + 4.0 * quarter;
    let mut t = a.rem_euclid(1.0) * perim;
    // Segments: half right side, corner, top, corner, left, corner, bottom, corner, half right.
    let segs: [(f32, u8); 9] = [
        (sy, 0),
        (quarter, 1),
        (2.0 * sx, 2),
        (quarter, 3),
        (2.0 * sy, 4),
        (quarter, 5),
        (2.0 * sx, 6),
        (quarter, 7),
        (sy, 8),
    ];
    for (len, kind) in segs {
        if t <= len || kind == 8 {
            let f = if len > 0.0 { (t / len).min(1.0) } else { 0.0 };
            let corner = |cx: f32, cy: f32, a0: f32| {
                let ang = a0 + f * FRAC_PI_2;
                Vec2::new(cx + r * ang.cos(), cy + r * ang.sin())
            };
            return match kind {
                0 => Vec2::new(h.x, f * sy),
                1 => corner(sx, sy, 0.0),
                2 => Vec2::new(sx - 2.0 * sx * f, h.y),
                3 => corner(-sx, sy, FRAC_PI_2),
                4 => Vec2::new(-h.x, sy - 2.0 * sy * f),
                5 => corner(-sx, -sy, PI),
                6 => Vec2::new(-sx + 2.0 * sx * f, -h.y),
                7 => corner(sx, -sy, 1.5 * PI),
                _ => Vec2::new(h.x, -sy + sy * f),
            };
        }
        t -= len;
    }
    Vec2::new(h.x, 0.0)
}

/// Sweep a rounded-rectangle section (half extents `h`, corner radius `r`) along a path of
/// frames `(point, tangent, x axis)`; `y = tangent × x`. Flat end caps.
fn sweep_rect(
    m: &mut Mesh,
    frames: &[(Vec3, Vec3, Vec3)],
    h: Vec2,
    r: f32,
    around: usize,
    color: &dyn Fn(Vec3) -> Vec4,
) {
    let n = frames.len();
    if n < 2 {
        return;
    }
    let len = arc_lengths(frames, |a, b| a.0.distance(b.0));
    let pts: Vec<Vec3> = frames.iter().map(|f| f.0).collect();
    let tans: Vec<Vec3> = frames.iter().map(|f| f.1).collect();
    let xs: Vec<Vec3> = frames.iter().map(|f| f.2).collect();
    let at = |t: f32, q: Vec2| -> Vec3 {
        let tan = poly_at(&tans, t).normalize_or(Vec3::X);
        let x = poly_at(&xs, t);
        let x = (x - tan * x.dot(tan)).normalize_or(Vec3::Y);
        let y = tan.cross(x);
        poly_at(&pts, t) + x * q.x + y * q.y
    };
    let perim = 4.0 * (h.x + h.y);
    let b: Vec<f32> = (0..n).map(|i| i as f32).collect();
    surface(
        m,
        &steps(0.0, 1.0, around),
        &b,
        (true, false),
        false,
        |a, t| {
            let q = rounded_rect(a, h, r);
            let p = at(t, q);
            Sample::new(p, Vec2::new(a * perim, poly_at(&len, t)), color(p))
        },
    );
    for (t, sign) in [(0.0f32, -1.0f32), ((n - 1) as f32, 1.0)] {
        let ring: Vec<Vec3> = (0..around)
            .map(|k| at(t, rounded_rect(k as f32 / around as f32, h, r)))
            .collect();
        let tan = poly_at(&tans, t).normalize_or(Vec3::X) * sign;
        let c = color(poly_at(&pts, t));
        flat_polygon(m, &ring, tan, c);
    }
}

/// Frames along a path for [`sweep_rect`]: tangents from neighbors, the section's x axis
/// from `x_axis(i)` made perpendicular to the tangent.
fn path_frames(pts: &[Vec3], x_axis: impl Fn(usize) -> Vec3) -> Vec<(Vec3, Vec3, Vec3)> {
    let n = pts.len();
    (0..n)
        .map(|i| {
            let t = (pts[(i + 1).min(n - 1)] - pts[i.saturating_sub(1)]).normalize_or(Vec3::X);
            let x = x_axis(i);
            (pts[i], t, (x - t * x.dot(t)).normalize_or(Vec3::Y))
        })
        .collect()
}

/// A lofted oar along +x from the handle end at the origin: `length` overall, blade at +x,
/// blade face in the xz plane.
fn oar(m: &mut Mesh, length: f32, color: Vec4) {
    let blade = 0.55;
    let profile = |t: f32| -> (f32, f32) {
        // (half width, half thickness) at distance t from the handle end.
        let x = t * length;
        let grip = 0.016 * (1.0 - (1.0 - (x / 0.012).min(1.0)).powi(2)).sqrt();
        if x < 0.13 {
            (grip, grip)
        } else if x < 0.17 {
            let f = smooth(0.13, 0.17, x);
            let r = 0.016 + (0.025 - 0.016) * f;
            (r, r)
        } else if x < length - blade {
            let f = (x - 0.17) / (length - blade - 0.17);
            let r = 0.025 - 0.006 * f;
            (r, r)
        } else {
            let f = (x - (length - blade)) / blade;
            let w = 0.019 + (0.07 - 0.019) * smooth(0.0, 0.35, f);
            let th = 0.019 + (0.006 - 0.019) * smooth(0.0, 0.3, f);
            // Rounded tip.
            let tip = (1.0 - ((f - 0.86) / 0.14).max(0.0).powi(2)).max(0.0).sqrt();
            (w * tip, th * tip.sqrt())
        }
    };
    let mut b = steps(0.0, 1.0, 46);
    b.extend_from_slice(&[0.0005, 0.002, 0.005, 0.998, 0.9995]);
    b.sort_by(f32::total_cmp);
    surface(m, &steps(0.0, 1.0, 12), &b, (true, false), false, |a, t| {
        let (w, th) = profile(t);
        let ang = a * TAU;
        let p = Vec3::new(t * length, th * ang.sin(), -w * ang.cos());
        Sample::new(p, Vec2::new(a * 0.12, t * length), color)
    });
}

/// A clinker-built (lapstrake) wooden rowboat along +x: 3.6 m long (transom at x = -1.8,
/// stem head at +1.8), 1.35 m beam, keel at y ≈ -0.31, sheer at y ≈ +0.30 midships rising to
/// +0.46 at the stem.
///
/// Six overlapping 15 mm strakes a side with visible lands, painted by strake (antifouling
/// red bottom, white topsides, green sheer strake) and varnished inside; keel and stem,
/// a varnished transom with knees, steam-bent ribs, keelson, risers, two thwarts (x = -0.7,
/// +0.5, top at y ≈ 0.13), a stern seat, floorboards, gunwale rails, a breasthook, two bronze
/// oarlocks, two oars lying on the thwarts and a painter ring on the stem.
pub fn rowboat() -> Vec<Part> {
    let mut kit = Kit::default();
    let white = rgba(Vec3::new(0.93, 0.92, 0.88));
    // Stations from the transom to the stem, closer together toward both ends.
    let us: Vec<f32> = (0..=40)
        .map(|i| {
            let t = i as f32 / 40.0;
            0.6 * t + 0.4 * (0.5 - 0.5 * (t * PI).cos())
        })
        .collect();
    let girth: Vec<f32> = us.iter().map(|&u| row_girth(u)).collect();
    let girth_at = |u: f32| -> f32 {
        let i = us.partition_point(|&x| x < u).clamp(1, us.len() - 1);
        let f = ((u - us[i - 1]) / (us[i] - us[i - 1]).max(1e-6)).clamp(0.0, 1.0);
        girth[i - 1] + (girth[i] - girth[i - 1]) * f
    };
    let at = |u: f32, s: f32, o: f32| row_hull(u, s) + row_normal(u, s) * o;

    // --- Planking ---------------------------------------------------------------------------
    for side in [1.0f32, -1.0] {
        for k in 0..ROW_STRAKES {
            let s_lo = k as f32 / ROW_STRAKES as f32;
            let s_hi = (k + 1) as f32 / ROW_STRAKES as f32;
            let land = move |u: f32| if k == 0 { 0.0 } else { 0.022 / girth_at(u) };
            let bev_s = move |u: f32| 0.004 / girth_at(u);
            let bev_o = 0.004;
            let bottom = move |u: f32| s_lo - land(u);
            let io = move |u: f32, s: f32| ROW_T * (s_hi - s) / (s_hi - bottom(u));
            let paint = row_paint(k);
            let outside = move |p: Vec3, s: f32, u: f32| -> Vec4 {
                let base = if paint == Surface::PaintedWood {
                    white
                } else {
                    Vec4::ONE
                };
                // Shadowed just under the land above, grime along the waterline.
                let under = if k + 1 < ROW_STRAKES {
                    let lap = s_hi - 0.022 / girth_at(u);
                    1.0 - 0.45 * smooth(lap - 0.035 / girth_at(u), lap, s)
                } else {
                    1.0
                };
                let wl = 1.0 - smooth(0.0, 0.06, (p.y + 0.09).abs());
                let grime = Vec4::new(0.78, 0.8, 0.66, 1.0).lerp(Vec4::ONE, 1.0 - wl * 0.6);
                base * grime * under
            };
            let stations = &us;
            // Outer face.
            let mesh = kit.mesh(paint);
            sided(
                mesh,
                &steps(0.0, 1.0, 4),
                stations,
                (false, false),
                true,
                side,
                |a, u| {
                    let s = bottom(u) + bev_s(u) + (s_hi - bev_s(u) - bottom(u) - bev_s(u)) * a;
                    let p = at(u, s, io(u, s) + ROW_T);
                    Sample::new(p, Vec2::new(s * girth_at(u), p.x), outside(p, s, u))
                },
            );
            // Lower edge (the visible land) and its eased corner.
            sided(
                mesh,
                &steps(0.0, 1.0, 1),
                stations,
                (false, false),
                true,
                side,
                |a, u| {
                    let s = bottom(u);
                    let o = io(u, s) + (ROW_T - bev_o) * a;
                    let p = at(u, s, o);
                    Sample::new(
                        p,
                        Vec2::new(s * girth_at(u) - o, p.x),
                        outside(p, s, u) * 0.6,
                    )
                },
            );
            sided(
                mesh,
                &steps(0.0, 1.0, 1),
                stations,
                (false, false),
                true,
                side,
                |a, u| {
                    let s = bottom(u) + bev_s(u) * a;
                    let o = io(u, s) + ROW_T - bev_o * (1.0 - a);
                    let p = at(u, s, o);
                    Sample::new(p, Vec2::new(s * girth_at(u), p.x), outside(p, s, u))
                },
            );
            // Upper eased corner (only the sheer strake's shows; the rest are lapped over).
            sided(
                mesh,
                &steps(0.0, 1.0, 1),
                stations,
                (false, false),
                true,
                side,
                |a, u| {
                    let s = s_hi - bev_s(u) * (1.0 - a);
                    let o = io(u, s) + ROW_T - bev_o * a;
                    let p = at(u, s, o);
                    Sample::new(p, Vec2::new(s * girth_at(u), p.x), outside(p, s, u))
                },
            );
            // Inside: varnished, darker in the bilge and in the corner under each land.
            let mesh = kit.mesh(Surface::Varnish);
            sided(
                mesh,
                &steps(0.0, 1.0, 1),
                stations,
                (false, false),
                true,
                side,
                |a, u| {
                    let s = s_hi;
                    let o = io(u, s) + (ROW_T - bev_o) * (1.0 - a);
                    let p = at(u, s, o);
                    Sample::new(p, Vec2::new(s * girth_at(u) + o, p.x), Vec4::ONE)
                },
            );
            sided(
                mesh,
                &steps(0.0, 1.0, 2),
                stations,
                (false, false),
                false,
                side,
                |a, u| {
                    let s = bottom(u) + (s_hi - bottom(u)) * a;
                    let p = at(u, s, io(u, s));
                    let corner = if k > 0 {
                        1.0 - 0.4 * (1.0 - smooth(s_lo, s_lo + 0.05, s))
                    } else {
                        1.0
                    };
                    let bilge = 1.0 - 0.35 * (1.0 - smooth(-0.25, -0.1, p.y));
                    let c = Vec3::splat(corner * bilge) * Vec3::new(1.0, 0.97, 0.93);
                    Sample::new(p, Vec2::new(s * girth_at(u), p.x), rgba(c))
                },
            );
            // Plank ends at the transom (covered by the transom's edge, but seen from inside).
            let u0 = us[0];
            let ring = [
                at(u0, bottom(u0), io(u0, bottom(u0))),
                at(u0, s_hi, 0.0),
                at(u0, s_hi, ROW_T),
                at(u0, bottom(u0), io(u0, bottom(u0)) + ROW_T),
            ];
            let ring: Vec<Vec3> = ring
                .iter()
                .map(|p| Vec3::new(p.x, p.y, p.z * side))
                .collect();
            flat_polygon(kit.mesh(paint), &ring, Vec3::NEG_X, Vec4::ONE);
        }
    }

    // --- Keel and stem ----------------------------------------------------------------------
    let mut spine: Vec<Vec3> = (0..=40).map(|i| row_hull(i as f32 / 40.0, 0.0)).collect();
    spine.extend((1..=24).map(|i| row_hull(1.0, i as f32 / 24.0)));
    let head = *spine.last().expect("stem");
    spine.push(head + Vec3::new(0.008, 0.045, 0.0));
    let n = spine.len();
    let spine_frames: Vec<(Vec3, Vec3, Vec3)> = (0..n)
        .map(|i| {
            let t = (spine[(i + 1).min(n - 1)] - spine[i.saturating_sub(1)]).normalize();
            let out = t.cross(Vec3::Z).normalize();
            let molded = 0.045;
            (spine[i] + out * (molded * 0.5 - 0.004), t, out)
        })
        .collect();
    let split = spine_frames
        .iter()
        .position(|f| f.0.x > 1.2 && f.0.y > -0.06)
        .unwrap_or(n - 2);
    let keel_color = |_: Vec3| Vec4::ONE;
    sweep_rect(
        kit.mesh(Surface::PaintedWoodRed),
        &spine_frames[..=split],
        Vec2::new(0.0225, 0.028),
        0.006,
        16,
        &keel_color,
    );
    sweep_rect(
        kit.mesh(Surface::Varnish),
        &spine_frames[split..],
        Vec2::new(0.0225, 0.032),
        0.006,
        16,
        &keel_color,
    );
    // A bronze stem band on the stem's leading edge.
    let band: Vec<(Vec3, Vec3, Vec3)> = spine_frames[split.saturating_sub(4)..]
        .iter()
        .map(|&(p, t, x)| (p + x * 0.024, t, x))
        .collect();
    let bronze = rgba(srgb(0x9c6b35));
    sweep_rect(
        kit.mesh(Surface::PaintedMetal),
        &band,
        Vec2::new(0.003, 0.009),
        0.0025,
        10,
        &|_| bronze,
    );

    // --- Transom ------------------------------------------------------------------------------
    // The plane through the planking's ends, its outward (aft, slightly down) normal and an
    // in-plane up axis.
    let p_lo = row_hull(0.0, 0.0);
    let p_hi = row_hull(0.0, 1.0);
    let up_axis = Vec3::new(p_hi.x - p_lo.x, p_hi.y - p_lo.y, 0.0).normalize();
    let aft = Vec3::Z.cross(up_axis).normalize();
    let aft = if aft.x > 0.0 { -aft } else { aft };
    let mut outline: Vec<Vec2> = Vec::new();
    let to2 = |p: Vec3| Vec2::new(p.z, (p - p_lo).dot(up_axis));
    for i in 0..=20 {
        let s = 1.0 - i as f32 / 20.0;
        outline.push(to2(at(0.0, s, ROW_T * 2.1)) * Vec2::new(-1.0, 1.0));
    }
    for i in 1..=20 {
        let s = i as f32 / 20.0;
        outline.push(to2(at(0.0, s, ROW_T * 2.1)));
    }
    // A crowned top edge, a little above the sheer.
    let top_l = *outline.last().expect("outline");
    for i in 1..12 {
        let f = i as f32 / 12.0;
        let z = top_l.x * (1.0 - 2.0 * f);
        outline.push(Vec2::new(
            z,
            top_l.y + 0.02 + 0.03 * (1.0 - (z / top_l.x).powi(2)),
        ));
    }
    let plane = Mat4::from_cols(
        Vec3::Z.extend(0.0),
        up_axis.extend(0.0),
        aft.extend(0.0),
        (p_lo - aft * 0.002).extend(1.0),
    );
    let transom_c = rgba(Vec3::new(1.0, 0.95, 0.9));
    prism(
        kit.mesh(Surface::Varnish),
        &outline,
        0.0,
        0.026,
        plane,
        transom_c,
    );

    // Knees: a sternpost knee on the centerline and quarter knees in the transom's top corners.
    let knee = |arm_a: f32, arm_b: f32, w: f32| -> Vec<Vec2> {
        let mut pts = vec![Vec2::ZERO, Vec2::new(arm_a, 0.0), Vec2::new(arm_a, w)];
        for i in 1..12 {
            let t = i as f32 / 12.0 * FRAC_PI_2;
            pts.push(Vec2::new(
                arm_a - (arm_a - w) * t.sin(),
                arm_b - (arm_b - w) * t.cos(),
            ));
        }
        pts.extend_from_slice(&[Vec2::new(w, arm_b), Vec2::new(0.0, arm_b)]);
        pts
    };
    let knee_c = rgba(Vec3::new(0.95, 0.9, 0.85));
    {
        // On the centerline: one arm forward along the keelson, one up the transom.
        let foot = row_hull(0.0, 0.0);
        let corner = Vec3::new(foot.x + 0.035, foot.y + 0.03, 0.0);
        prism(
            kit.mesh(Surface::Varnish),
            &knee(0.34, 0.3, 0.06),
            -0.022,
            0.022,
            at_(corner),
            knee_c,
        );
    }
    for side in [1.0f32, -1.0] {
        // Horizontal, under the gunwale: one arm along the transom (z), one along the side.
        let y = row_hull(0.0, 1.0).y - 0.045;
        let (hb, _) = row_half_breadth(-1.62, y);
        let corner = Vec3::new(-1.74, y, (hb - 0.02) * side);
        let xf = Mat4::from_cols(
            Vec3::new(0.0, 0.0, -side).extend(0.0),
            Vec3::X.extend(0.0),
            Vec3::new(0.0, side, 0.0).extend(0.0) * -1.0,
            corner.extend(1.0),
        );
        let mut out = Mesh::default();
        prism(&mut out, &knee(0.22, 0.24, 0.05), -0.017, 0.017, xf, knee_c);
        kit.add(Surface::Varnish, &out);
    }

    // --- Keelson, ribs and risers -------------------------------------------------------------
    let hog: Vec<Vec3> = (0..=30)
        .map(|i| {
            let u = 0.03 + 0.86 * i as f32 / 30.0;
            row_hull(u, 0.0) + Vec3::new(0.0, 0.016, 0.0)
        })
        .collect();
    let hog_frames = path_frames(&hog, |_| Vec3::Z);
    let wood = |p: Vec3| rgba(Vec3::splat(1.0 - 0.3 * (1.0 - smooth(-0.26, -0.15, p.y))));
    sweep_rect(
        kit.mesh(Surface::Varnish),
        &hog_frames,
        Vec2::new(0.045, 0.014),
        0.005,
        16,
        &wood,
    );
    let mut x = -1.48;
    while x < 1.42 {
        for side in [1.0f32, -1.0] {
            let pts: Vec<Vec3> = (0..=12)
                .map(|i| {
                    let s = 0.04 + 0.95 * i as f32 / 12.0;
                    let u = row_u_at(x, s);
                    let p = row_hull(u, s) - row_normal(u, s) * 0.011;
                    Vec3::new(p.x, p.y, p.z * side)
                })
                .collect();
            let frames = path_frames(&pts, |i| {
                let s = 0.04 + 0.95 * i as f32 / 12.0;
                let n = row_normal(row_u_at(x, s), s);
                Vec3::new(n.x, n.y, n.z * side)
            });
            sweep_rect(
                kit.mesh(Surface::Varnish),
                &frames,
                Vec2::new(0.011, 0.016),
                0.004,
                8,
                &wood,
            );
        }
        x += if x < -1.0 || x > 0.9 { 0.24 } else { 0.27 };
    }
    // Risers carry the thwarts.
    let riser_y = 0.093;
    for side in [1.0f32, -1.0] {
        let pts: Vec<Vec3> = (0..=24)
            .map(|i| {
                let x = -1.55 + 2.75 * i as f32 / 24.0;
                let (hb, _) = row_half_breadth(x, riser_y);
                Vec3::new(x, riser_y, (hb - 0.033) * side)
            })
            .collect();
        let frames = path_frames(&pts, |_| Vec3::Y);
        sweep_rect(
            kit.mesh(Surface::Varnish),
            &frames,
            Vec2::new(0.022, 0.01),
            0.004,
            12,
            &|_| Vec4::ONE,
        );
    }

    // --- Thwarts, stern seat and floorboards --------------------------------------------------
    let seat_c = rgba(Vec3::new(1.0, 0.97, 0.94));
    let board = |kit: &mut Kit, x0: f32, x1: f32, y: f32, thick: f32| {
        let w0 = row_half_breadth(x0, y).0 - 0.03;
        let w1 = row_half_breadth(x1, y).0 - 0.03;
        let c = 0.006;
        let outline = [
            Vec2::new(x0, -w0 + c),
            Vec2::new(x0 + c, -w0),
            Vec2::new(x1 - c, -w1),
            Vec2::new(x1, -w1 + c),
            Vec2::new(x1, w1 - c),
            Vec2::new(x1 - c, w1),
            Vec2::new(x0 + c, w0),
            Vec2::new(x0, w0 - c),
        ];
        // The board lies in the xz plane; local z is up. Grain runs across the boat (v).
        let xf = Mat4::from_cols(Vec4::X, Vec4::NEG_Z, Vec4::Y, Vec4::W);
        let mut out = Mesh::default();
        prism(&mut out, &outline, y - thick, y, xf, seat_c);
        box_uv(&mut out, 2);
        kit.add(Surface::Varnish, &out);
    };
    let thwart_top = 0.133;
    board(&mut kit, -0.82, -0.58, thwart_top, 0.024);
    board(&mut kit, 0.38, 0.62, thwart_top, 0.024);
    board(&mut kit, -1.62, -1.47, thwart_top, 0.022);
    board(&mut kit, -1.45, -1.3, thwart_top, 0.022);
    // Floorboards: slats along the bottom on bearers shaped to the hull.
    let floor_y = -0.16;
    for xb in [-0.95f32, -0.25, 0.45] {
        let y0 = row_hull(row_u_at(xb, 0.0), 0.0).y + 0.03;
        let y1 = floor_y - 0.018;
        let mut outline = Vec::new();
        for i in 0..=6 {
            let y = y1 + (y0 - y1) * i as f32 / 6.0;
            outline.push(Vec2::new(row_half_breadth(xb, y).0 - 0.026, y));
        }
        for i in (0..=6).rev() {
            let y = y1 + (y0 - y1) * i as f32 / 6.0;
            outline.push(Vec2::new(-(row_half_breadth(xb, y).0 - 0.026), y));
        }
        let xf = Mat4::from_cols(
            Vec4::Z,
            Vec4::Y,
            Vec4::NEG_X,
            Vec3::new(xb, 0.0, 0.0).extend(1.0),
        );
        let mut out = Mesh::default();
        prism(&mut out, &outline, -0.018, 0.018, xf, knee_c);
        box_uv(&mut out, 2);
        kit.add(Surface::Varnish, &out);
    }
    for k in 0..5 {
        let z = (k as f32 - 2.0) * 0.11;
        let mut ends = [0.0f32; 2];
        for (e, (start, step)) in [(-1.6f32, 0.01f32), (1.2, -0.01)].into_iter().enumerate() {
            let mut x = start;
            while row_half_breadth(x, floor_y - 0.012).0 < z.abs() + 0.06 && x.abs() < 1.6 {
                x += step;
            }
            ends[e] = x;
        }
        let (x0, x1) = (ends[0].max(-1.32), ends[1].min(0.95));
        if x1 - x0 < 0.3 {
            continue;
        }
        let xf = at_(Vec3::new((x0 + x1) * 0.5, floor_y - 0.009, z));
        let mut out = Mesh::default();
        chamfer_box(
            &mut out,
            Vec3::new((x1 - x0) * 0.5, 0.009, 0.042),
            0.004,
            xf,
            seat_c,
        );
        box_uv(&mut out, 0);
        kit.add(Surface::Varnish, &out);
    }

    // --- Gunwales and breasthook -------------------------------------------------------------
    for side in [1.0f32, -1.0] {
        for (offset, drop, h) in [
            (ROW_T + 0.011, 0.014, Vec2::new(0.011, 0.016)),
            (-0.012, 0.016, Vec2::new(0.012, 0.017)),
        ] {
            let pts: Vec<Vec3> = (0..=32)
                .map(|i| {
                    let u = 0.004 + 0.976 * i as f32 / 32.0;
                    let s = 1.0 - drop / girth_at(u);
                    let p = at(u, s, offset);
                    Vec3::new(p.x, p.y, p.z * side)
                })
                .collect();
            let frames = path_frames(&pts, |i| {
                let u = 0.004 + 0.976 * i as f32 / 32.0;
                let n = row_normal(u, 1.0);
                Vec3::new(n.x, n.y, n.z * side)
            });
            sweep_rect(kit.mesh(Surface::Varnish), &frames, h, 0.005, 10, &|_| {
                Vec4::ONE
            });
        }
    }
    {
        let y = row_hull(1.0, 1.0).y - 0.035;
        let mut outline = Vec::new();
        for i in 0..=8 {
            let x = 1.42 + 0.33 * i as f32 / 8.0;
            let (hb, _) = row_half_breadth(x, y);
            outline.push(Vec2::new(x, -(hb - 0.012).max(0.0)));
        }
        for i in (0..=8).rev() {
            let x = 1.42 + 0.33 * i as f32 / 8.0;
            let (hb, _) = row_half_breadth(x, y);
            outline.push(Vec2::new(x, (hb - 0.012).max(0.0)));
        }
        outline.dedup_by(|a, b| a.distance(*b) < 1e-4);
        let xf = Mat4::from_cols(Vec4::X, Vec4::NEG_Z, Vec4::Y, Vec4::W);
        prism(
            kit.mesh(Surface::Varnish),
            &outline,
            y - 0.028,
            y,
            xf,
            knee_c,
        );
    }

    // --- Oarlocks -----------------------------------------------------------------------------
    for side in [1.0f32, -1.0] {
        let xo = 0.24;
        let u = row_u_at(xo, 1.0);
        let top = row_hull(u, 1.0);
        let base = Vec3::new(xo, top.y + 0.004, (top.z + 0.004) * side);
        let mut m = Mesh::default();
        chamfer_box(
            &mut m,
            Vec3::new(0.05, 0.005, 0.022),
            0.003,
            at_(base),
            bronze,
        );
        tube(
            &mut m,
            &[base + Vec3::Y * 0.004, base + Vec3::Y * 0.028],
            10,
            false,
            &|_| 0.008,
            bronze,
        );
        let horn = arc(
            base + Vec3::Y * 0.066,
            Vec3::X,
            Vec3::NEG_Y,
            0.036,
            -0.15 * PI,
            1.15 * PI,
            20,
        );
        tube(
            &mut m,
            &horn,
            10,
            false,
            &|s| 0.0075 + 0.003 * (2.0 * s - 1.0).powi(8),
            bronze,
        );
        let collar = [base + Vec3::Y * 0.026, base + Vec3::Y * 0.034];
        tube(&mut m, &collar, 10, false, &|_| 0.011, bronze);
        kit.add(Surface::PaintedMetal, &m);
    }

    // --- Oars ---------------------------------------------------------------------------------
    for (z, yaw, x0) in [(0.2f32, 0.035f32, -1.25f32), (-0.24, -0.02, -1.18)] {
        let mut m = Mesh::default();
        oar(&mut m, 2.15, Vec4::ONE);
        let xf = Mat4::from_translation(Vec3::new(x0, thwart_top + 0.026, z))
            * Mat4::from_rotation_y(yaw)
            * Mat4::from_rotation_x(if z > 0.0 { 0.25 } else { -0.4 });
        kit.add(Surface::Varnish, &moved(m, xf));
        // A leather sleeve where the oar works in the oarlock.
        let mut sleeve = Mesh::default();
        tube(
            &mut sleeve,
            &[Vec3::new(0.62, 0.0, 0.0), Vec3::new(0.82, 0.0, 0.0)],
            14,
            false,
            &|_| 0.0262,
            rgba(srgb(0x6b4a32)),
        );
        kit.add(Surface::Rubber, &moved(sleeve, xf));
    }

    // --- Painter ring on the stem -------------------------------------------------------------
    {
        let eye = row_hull(1.0, 0.82) + Vec3::new(0.045, 0.0, 0.0);
        let ring = arc(
            eye + Vec3::new(0.02, -0.022, 0.0),
            Vec3::X,
            Vec3::Y,
            0.022,
            0.0,
            TAU,
            24,
        );
        let mut m = Mesh::default();
        tube(&mut m, &ring[..24], 8, true, &|_| 0.0045, bronze);
        tube(
            &mut m,
            &[eye - Vec3::X * 0.01, eye + Vec3::X * 0.012],
            8,
            false,
            &|_| 0.008,
            bronze,
        );
        kit.add(Surface::PaintedMetal, &m);
    }
    kit.finish()
}

fn at_(p: Vec3) -> Mat4 {
    Mat4::from_translation(p)
}

/// Planar UVs in meters by each vertex's facing, with the wood grain (v) along axis `grain`
/// (0 = x, 1 = y, 2 = z) wherever the face allows.
fn box_uv(m: &mut Mesh, grain: usize) {
    for v in &mut m.vertices {
        let n = Vec3::from(v.normal).abs();
        let d = n.max_position();
        let p = Vec3::from(v.position);
        let (a, b) = ((d + 1) % 3, (d + 2) % 3);
        // v follows the grain when the face contains that axis.
        v.uv = if grain == a {
            [p[b], p[a]]
        } else {
            [p[a], p[b]]
        };
    }
}

// ---------------------------------------------------------------------------------------------
// Motorboat
// ---------------------------------------------------------------------------------------------

/// The runabout's three hull lines at station `u` (0 = transom, 1 = bow tip), all at the same
/// x: keel height, chine (half-breadth, height) and sheer (half-breadth, height).
struct Lines {
    x: f32,
    keel: f32,
    chine: Vec2,
    sheer: Vec2,
}

fn mb_lines(u: f32) -> Lines {
    let u = u.clamp(0.0, 1.0);
    let x = -2.6 + 5.2 * u;
    let keel = if u <= 0.45 {
        -0.45
    } else {
        -0.45 + 0.9 * ((u - 0.45) / 0.55).powf(2.3)
    };
    let sheer_y = 0.36 + 0.09 * u * u;
    let sheer_z = if u <= 0.4 {
        1.0 - 0.08 * (1.0 - u / 0.4).powi(2)
    } else {
        ((u - 0.4) / 0.6 * FRAC_PI_2).cos()
    };
    let chine_z = if u <= 0.35 {
        0.86 - 0.02 * (1.0 - u / 0.35).powi(2)
    } else {
        0.86 * ((u - 0.35) / 0.65 * FRAC_PI_2).cos().max(0.0).powf(1.15)
    };
    let deadrise = (19.0 + 11.0 * smooth(0.3, 0.7, u) + 25.0 * smooth(0.7, 1.0, u)).to_radians();
    let chine_y = (keel + chine_z * deadrise.tan()).min(sheer_y);
    Lines {
        x,
        keel,
        chine: Vec2::new(chine_z, chine_y),
        sheer: Vec2::new(sheer_z, sheer_y),
    }
}

/// Deck height at plan position (x, z): the sheer height plus a crown.
fn mb_deck_y(x: f32, z: f32) -> f32 {
    let u = ((x + 2.6) / 5.2).clamp(0.0, 1.0);
    let l = mb_lines(u);
    let f = (z / l.sheer.x.max(1e-3)).clamp(-1.0, 1.0);
    l.sheer.y + 0.035 * (1.0 - f * f)
}

/// Where the ray from `c` along `d` (plan, x/z) crosses the closed polygon `poly`.
fn ray_polygon(c: Vec2, d: Vec2, poly: &[Vec2]) -> Vec2 {
    let mut best = f32::MAX;
    for i in 0..poly.len() {
        let (a, b) = (poly[i], poly[(i + 1) % poly.len()]);
        let e = b - a;
        let den = d.perp_dot(e);
        if den.abs() < 1e-9 {
            continue;
        }
        let t = (a - c).perp_dot(e) / den;
        let s = (a - c).perp_dot(d) / den;
        if t > 0.0 && (-1e-4..=1.0 + 1e-4).contains(&s) && t < best {
            best = t;
        }
    }
    c + d * if best == f32::MAX { 0.0 } else { best }
}

/// A box of half extents `half` with edges rounded by `r` (cushions, molded parts), moved
/// by `xf`. `n` subdivisions per face.
fn rounded_box(m: &mut Mesh, half: Vec3, r: f32, n: usize, xf: Mat4, color: Vec4) {
    let r = r.min(half.min_element() * 0.999);
    let inner = half - Vec3::splat(r);
    let faces = [
        (Vec3::X, Vec3::Y, Vec3::Z),
        (Vec3::NEG_X, Vec3::Z, Vec3::Y),
        (Vec3::Y, Vec3::Z, Vec3::X),
        (Vec3::NEG_Y, Vec3::X, Vec3::Z),
        (Vec3::Z, Vec3::X, Vec3::Y),
        (Vec3::NEG_Z, Vec3::Y, Vec3::X),
    ];
    let mut local = Mesh::default();
    // Parameter values crowd toward the edges where the rounding is.
    let vals: Vec<f32> = (0..=n)
        .map(|i| {
            let t = i as f32 / n as f32 * 2.0 - 1.0;
            t.signum() * t.abs().powf(0.6)
        })
        .collect();
    for (e, t1, t2) in faces {
        surface(&mut local, &vals, &vals, (false, false), false, |a, b| {
            let p = (e + t1 * a + t2 * b) * half;
            let q = p.clamp(-inner, inner);
            let d = (p - q).normalize_or(e);
            let pos = q + d * r;
            Sample::new(pos, Vec2::new(pos.dot(t1), pos.dot(t2)), color)
        });
    }
    local.transform(&xf);
    m.merge(&local);
}

/// Reverse a mesh's winding and normals if it encloses negative volume (a safety net for
/// parts whose parameterization's handedness is hard to see).
fn orient_outward(m: &mut Mesh) {
    let c = m
        .vertices
        .iter()
        .map(|v| Vec3::from(v.position))
        .sum::<Vec3>()
        / m.vertices.len().max(1) as f32;
    let vol: f32 = m
        .indices
        .chunks(3)
        .map(|t| {
            let p = |i: u32| Vec3::from(m.vertices[i as usize].position) - c;
            p(t[0]).dot(p(t[1]).cross(p(t[2])))
        })
        .sum();
    if vol < 0.0 {
        for t in m.indices.chunks_mut(3) {
            t.swap(1, 2);
        }
        for v in &mut m.vertices {
            v.normal = (-Vec3::from(v.normal)).to_array();
        }
    }
}

/// A cast cleat along x, `scale` × the 0.25 m dock size, base at y = 0.
fn cleat_mesh(scale: f32, color: Vec4) -> Mesh {
    let len = 0.25;
    let half = len * 0.5;
    let leg = |x: f32| -> f32 {
        // 1 inside the legs (|x| in 0.018..0.062), 0 under the bar and the horns.
        let ax = x.abs();
        smooth(0.012, 0.022, ax) * (1.0 - smooth(0.056, 0.068, ax))
    };
    let section = |x: f32, a: f32| -> Vec3 {
        let ax = x.abs() / half;
        let top = 0.056 + 0.006 * ax.powi(2);
        let bottom = 0.034 * (1.0 - leg(x)) + 0.002 * ax;
        // Horn tips round off.
        let tip = (1.0 - ((ax - 0.82) / 0.18).max(0.0).powi(2))
            .max(0.0)
            .sqrt();
        let half_h = (top - bottom) * 0.5 * tip.max(0.05);
        let cy = bottom + (top - bottom) * 0.5 + (1.0 - tip) * (top - bottom) * 0.3;
        let th = a * TAU;
        let (c, s) = (th.cos(), th.sin());
        // A boxy superellipse section.
        let k = 0.55;
        let zc = c.signum() * c.abs().powf(k);
        let yc = s.signum() * s.abs().powf(k);
        let y = cy + yc * half_h;
        let w = (0.016 + 0.004 * (1.0 - ax)) * tip;
        let flare = 1.0 + 0.9 * leg(x) * (1.0 - smooth(0.0, 0.022, y));
        Vec3::new(x, y, zc * w * flare)
    };
    let mut xs: Vec<f32> = (0..=56).map(|i| -half + len * i as f32 / 56.0).collect();
    xs.extend_from_slice(&[-half + 0.001, half - 0.001, -half + 0.004, half - 0.004]);
    xs.sort_by(f32::total_cmp);
    xs.dedup();
    let mut m = Mesh::default();
    surface(
        &mut m,
        &steps(0.0, 1.0, 20),
        &xs,
        (true, false),
        false,
        |a, x| {
            let p = section(x, a);
            Sample::new(p * scale, Vec2::new(p.z * scale, p.x * scale), color)
        },
    );
    orient_outward(&mut m);
    m
}

/// An open fiberglass runabout along +x: 5.2 m long (transom at x = -2.6, bow at +2.6),
/// 2.0 m beam, keel at y = -0.45, deck edge from y = 0.36 aft to 0.45 at the bow.
///
/// Deep-V planing hull with hard chines and two spray rails a side, flared bow, rub rail,
/// a navy and red stripe, cambered deck with a cockpit (non-slip floor at y = -0.05),
/// wraparound windshield, helm console with wheel and gauges, two bucket seats, a stern
/// bench, cleats, and an outboard motor hung on the transom (leg at x ≈ -2.75, propeller
/// at x ≈ -2.98, cowling back to x ≈ -3.1, its skeg down to y ≈ -0.72).
pub fn motorboat() -> Vec<Part> {
    let mut kit = Kit::default();
    let us: Vec<f32> = (0..=72)
        .map(|i| {
            let t = i as f32 / 72.0;
            // Closer stations toward the bow where the hull turns fastest.
            1.0 - (1.0 - t).powf(1.35)
        })
        .collect();
    let white = Vec3::new(0.94, 0.94, 0.92);
    let navy = srgb(0x1b2c4a);
    let red = srgb(0xa3221c);
    // Section points from keel to chine flat (starboard, z/y) at station u.
    let rail = |u: f32| 0.026 * (1.0 - smooth(0.72, 0.9, u));
    let bottom_pts = |u: f32| -> [Vec2; 9] {
        let l = mb_lines(u);
        let k = Vec2::new(0.0, l.keel);
        let c = l.chine;
        let slope = if c.x > 1e-4 { (c.y - k.y) / c.x } else { 0.0 };
        let on_bottom = |z: f32| Vec2::new(z, k.y + z * slope);
        let w = rail(u) * (c.x / 0.86).min(1.0);
        let r1 = c.x * 0.34;
        let r2 = c.x * 0.67;
        let flat = 0.045 * (c.x / 0.86).min(1.0);
        [
            k,
            on_bottom(r1),
            Vec2::new(r1 + w, on_bottom(r1).y),
            on_bottom(r1 + w),
            on_bottom(r2),
            Vec2::new(r2 + w, on_bottom(r2).y),
            on_bottom(r2 + w),
            c,
            Vec2::new(c.x + flat, c.y - flat * 0.18),
        ]
    };
    // Topsides: chine flat edge (t = 0) up to the deck edge (t = 1), flared and a little full.
    let topside = |u: f32, t: f32| -> Vec2 {
        let l = mb_lines(u);
        let p0 = bottom_pts(u)[8];
        let p1 = l.sheer;
        let mid = Vec2::new(
            p0.x + (p1.x - p0.x) * 0.75 + 0.02,
            p0.y + (p1.y - p0.y) * 0.45,
        );
        let a = p0.lerp(mid, t);
        let b = mid.lerp(p1, t);
        a.lerp(b, t)
    };
    let hull_color = |p: Vec3, t: f32| -> Vec4 {
        // t: 0 at the chine flat, 1 at the deck edge (topsides); -1 below.
        let mut c = white;
        if t >= 0.0 {
            let a = 1.0 - t;
            if (0.07..0.3).contains(&a) {
                c = navy;
            } else if (0.335..0.36).contains(&a) {
                c = red;
            }
        }
        let wl = 1.0 - smooth(0.0, 0.07, (p.y + 0.2).abs());
        c *= 1.0 - 0.18 * wl;
        rgba(c)
    };
    // Stripe edges get doubled rows so they stay crisp.
    let mut ts: Vec<f32> = steps(0.0, 1.0, 10);
    for a in [0.07f32, 0.3, 0.335, 0.36] {
        ts.push(1.0 - a - 0.0015);
        ts.push(1.0 - a + 0.0015);
    }
    ts.sort_by(f32::total_cmp);
    for side in [1.0f32, -1.0] {
        let mesh = kit.mesh(Surface::Gelcoat);
        for seg in 0..8 {
            sided(
                mesh,
                &[0.0, 1.0],
                &us,
                (false, false),
                true,
                side,
                |a, u| {
                    let pts = bottom_pts(u);
                    let q = pts[seg].lerp(pts[seg + 1], a);
                    let p = Vec3::new(mb_lines(u).x, q.y, q.x);
                    let girth = seg as f32 * 0.1 + a * 0.1;
                    Sample::new(p, Vec2::new(girth + q.x, p.x), hull_color(p, -1.0))
                },
            );
        }
        sided(mesh, &ts, &us, (false, false), true, side, |t, u| {
            let q = topside(u, t);
            let p = Vec3::new(mb_lines(u).x, q.y, q.x);
            Sample::new(p, Vec2::new(1.0 + p.y, p.x), hull_color(p, t))
        });
    }
    // Transom: the section outline both sides, closed along the crowned deck edge.
    {
        let mut outline: Vec<Vec3> = Vec::new();
        let pts = bottom_pts(0.0);
        let mut half: Vec<Vec2> = pts.to_vec();
        for i in 1..=10 {
            half.push(topside(0.0, i as f32 / 10.0));
        }
        for q in half.iter().rev() {
            outline.push(Vec3::new(-2.6, q.y, -q.x));
        }
        for q in half.iter().skip(1) {
            outline.push(Vec3::new(-2.6, q.y, q.x));
        }
        let zs = mb_lines(0.0).sheer.x;
        for i in 1..12 {
            let z = zs * (1.0 - 2.0 * i as f32 / 12.0);
            outline.push(Vec3::new(-2.6, mb_deck_y(-2.6, z), z));
        }
        outline.dedup_by(|a, b| a.distance(*b) < 1e-5);
        let mut m = Mesh::default();
        flat_polygon(&mut m, &outline, Vec3::NEG_X, rgba(white));
        kit.add(Surface::Gelcoat, &m);
    }

    // --- Deck, cockpit liner and floor ------------------------------------------------------
    let sheer_plan: Vec<Vec2> = {
        let mut v: Vec<Vec2> = us
            .iter()
            .map(|&u| {
                let l = mb_lines(u);
                Vec2::new(l.x, l.sheer.x)
            })
            .collect();
        let back: Vec<Vec2> = v
            .iter()
            .rev()
            .skip(1)
            .map(|p| Vec2::new(p.x, -p.y))
            .collect();
        v.extend(back);
        v.dedup_by(|a, b| a.distance(*b) < 1e-5);
        v
    };
    let (cx0, cx1) = (-2.22f32, 0.5f32);
    let cockpit_half = |x: f32| -> f32 {
        let u = ((x + 2.6) / 5.2).clamp(0.0, 1.0);
        mb_lines(u).sheer.x - 0.2
    };
    let cockpit_plan: Vec<Vec2> = {
        let mut v = Vec::new();
        let n = 24;
        let (rf, ra) = (0.32f32, 0.14f32);
        // Starboard side aft to fore, around the front corners, back down port.
        for i in 0..=n {
            let x = cx0 + ra + (cx1 - rf - cx0 - ra) * i as f32 / n as f32;
            v.push(Vec2::new(x, cockpit_half(x)));
        }
        for i in 1..=8 {
            let a = i as f32 / 8.0 * FRAC_PI_2;
            let w = cockpit_half(cx1 - rf);
            v.push(Vec2::new(cx1 - rf + rf * a.sin(), w - rf + rf * a.cos()));
        }
        for i in 1..8 {
            let a = i as f32 / 8.0 * FRAC_PI_2;
            let w = cockpit_half(cx1 - rf);
            v.push(Vec2::new(cx1 - rf + rf * a.cos(), -(w - rf) - rf * a.sin()));
        }
        for i in 0..=n {
            let x = cx1 - rf - (cx1 - rf - cx0 - ra) * i as f32 / n as f32;
            v.push(Vec2::new(x, -cockpit_half(x)));
        }
        for i in 1..8 {
            let a = i as f32 / 8.0 * FRAC_PI_2;
            let w = cockpit_half(cx0 + ra);
            v.push(Vec2::new(cx0 + ra - ra * a.sin(), -(w - ra) - ra * a.cos()));
        }
        for i in 0..8 {
            let a = i as f32 / 8.0 * FRAC_PI_2;
            let w = cockpit_half(cx0 + ra);
            v.push(Vec2::new(cx0 + ra - ra * a.cos(), (w - ra) + ra * a.sin()));
        }
        v
    };
    let center = Vec2::new(-0.85, 0.0);
    let fillet = 0.03;
    let floor_y = -0.05;
    let edge = |th: f32, poly: &[Vec2]| ray_polygon(center, Vec2::new(th.cos(), th.sin()), poly);
    // Plan direction pointing out of the cockpit, perpendicular to its edge.
    let out_dir = |th: f32| -> Vec2 {
        let e = 1e-3;
        let a = edge(th - e, &cockpit_plan);
        let b = edge(th + e, &cockpit_plan);
        let t = (b - a).normalize_or(Vec2::X);
        Vec2::new(t.y, -t.x)
    };
    let around = steps(0.0, 1.0, 160);
    let deck_mesh = kit.mesh(Surface::Gelcoat);
    surface(
        deck_mesh,
        &around,
        &steps(0.0, 1.0, 7),
        (true, false),
        false,
        |a, k| {
            let th = a * TAU;
            let inner = edge(th, &cockpit_plan) + out_dir(th) * fillet;
            let outer = edge(th, &sheer_plan);
            let q = inner.lerp(outer, k);
            let y = if k >= 1.0 {
                mb_lines(((q.x + 2.6) / 5.2).clamp(0.0, 1.0)).sheer.y
            } else {
                mb_deck_y(q.x, q.y)
            };
            let p = Vec3::new(q.x, y, q.y);
            Sample::new(p, Vec2::new(p.x, p.z), rgba(white))
        },
    );
    // The rounded lip of the cockpit opening, then the liner down to the floor.
    surface(
        deck_mesh,
        &around,
        &steps(0.0, 1.0, 4),
        (true, false),
        true,
        |a, f| {
            let th = a * TAU;
            let e = edge(th, &cockpit_plan);
            let phi = f * FRAC_PI_2;
            let y0 = mb_deck_y(e.x, e.y);
            let q = e + out_dir(th) * (fillet * (1.0 - phi.sin()));
            let p = Vec3::new(q.x, y0 - fillet * (1.0 - phi.cos()), q.y);
            Sample::new(p, Vec2::new(a * 8.0, p.y), rgba(white))
        },
    );
    surface(
        deck_mesh,
        &around,
        &steps(0.0, 1.0, 3),
        (true, false),
        true,
        |a, f| {
            let th = a * TAU;
            let e = edge(th, &cockpit_plan);
            let y0 = mb_deck_y(e.x, e.y) - fillet;
            // The liner draws in a little toward the floor.
            let q = e - out_dir(th) * (0.04 * f * f);
            let y = y0 + (floor_y - y0) * f;
            let shade = 1.0 - 0.3 * f;
            let p = Vec3::new(q.x, y, q.y);
            Sample::new(p, Vec2::new(a * 8.0, p.y), rgba(white * shade))
        },
    );
    {
        let ring: Vec<Vec3> = (0..160)
            .map(|i| {
                let th = i as f32 / 160.0 * TAU;
                let q = edge(th, &cockpit_plan) - out_dir(th) * 0.04;
                Vec3::new(q.x, floor_y, q.y)
            })
            .collect();
        let mut m = Mesh::default();
        flat_polygon(&mut m, &ring, Vec3::Y, rgba(Vec3::splat(0.85)));
        kit.add(Surface::NonSlip, &m);
    }

    // --- Rub rail -------------------------------------------------------------------------------
    for side in [1.0f32, -1.0] {
        let pts: Vec<Vec3> = us
            .iter()
            .filter(|&&u| u < 0.985)
            .map(|&u| {
                let l = mb_lines(u);
                Vec3::new(l.x, l.sheer.y - 0.012, (l.sheer.x + 0.008) * side)
            })
            .collect();
        let frames = path_frames(&pts, |i| {
            let u = us[i];
            let l = mb_lines(u);
            let l2 = mb_lines(u + 1e-3);
            let tangent = Vec3::new(l2.x - l.x, 0.0, (l2.sheer.x - l.sheer.x) * side);
            tangent.cross(Vec3::Y).normalize() * side
        });
        sweep_rect(
            kit.mesh(Surface::Rubber),
            &frames,
            Vec2::new(0.014, 0.022),
            0.012,
            12,
            &|_| rgba(Vec3::splat(0.7)),
        );
        let insert: Vec<(Vec3, Vec3, Vec3)> = frames
            .iter()
            .map(|&(p, t, x)| (p + x * 0.013, t, x))
            .collect();
        sweep_rect(
            kit.mesh(Surface::PaintedMetal),
            &insert,
            Vec2::new(0.003, 0.007),
            0.0025,
            8,
            &|_| rgba(Vec3::splat(0.85)),
        );
    }
    // A stainless stem plate and bow eye at the tip.
    {
        let tip = mb_lines(1.0);
        let mut m = Mesh::default();
        let path: Vec<Vec3> = (0..=10)
            .map(|i| {
                let u = 0.93 + 0.07 * i as f32 / 10.0;
                let l = mb_lines(u);
                Vec3::new(l.x + 0.006, l.keel + 0.004, 0.0)
            })
            .collect();
        tube(
            &mut m,
            &path,
            10,
            false,
            &|_| 0.014,
            rgba(Vec3::splat(0.85)),
        );
        let eye_at = Vec3::new(tip.x - 0.13, mb_lines(0.975).keel - 0.0, 0.0);
        let ring = arc(
            eye_at + Vec3::new(0.035, -0.02, 0.0),
            Vec3::X,
            Vec3::Y,
            0.022,
            0.0,
            TAU,
            20,
        );
        tube(
            &mut m,
            &ring[..20],
            8,
            true,
            &|_| 0.005,
            rgba(Vec3::splat(0.85)),
        );
        kit.add(Surface::PaintedMetal, &m);
    }

    // --- Windshield ---------------------------------------------------------------------------
    {
        let base = |w: f32| -> Vec3 {
            // w in -1..1 across: the front curve, wrapping back along the side decks.
            let z = w * 0.84;
            let x = 0.62 - 0.42 * w.abs().powf(2.6);
            Vec3::new(x, mb_deck_y(x, z) + 0.004, z)
        };
        let lean = |w: f32| -> Vec3 {
            // Raked back and drawn in at the top.
            Vec3::new(
                -0.24 + 0.1 * w.abs().powi(2),
                0.34 - 0.02 * w.abs(),
                -0.1 * w,
            )
        };
        let glass = |w: f32, h: f32, off: f32| -> Vec3 {
            let b = base(w);
            let e = 1e-3;
            let tangent = (base(w + e) - base(w - e)).normalize();
            let up = lean(w).normalize();
            let n = tangent.cross(up).normalize();
            b + lean(w) * h + n * off
        };
        let ws = steps(-1.0, 1.0, 40);
        let hs = steps(0.0, 1.0, 4);
        let tint = Vec4::new(0.6, 0.68, 0.7, 1.0);
        let mut g = Mesh::default();
        for (off, flip) in [(0.003f32, false), (-0.003, true)] {
            surface(&mut g, &ws, &hs, (false, false), flip, |w, h| {
                let p = glass(w, h, off);
                Sample::new(p, Vec2::new(w, h), tint)
            });
        }
        for (w, flip) in [(-1.0f32, false), (1.0, true)] {
            surface(
                &mut g,
                &[-0.003, 0.003],
                &hs,
                (false, false),
                flip,
                |o, h| Sample::new(glass(w, h, o), Vec2::new(o, h), tint),
            );
        }
        for (h, flip) in [(1.0f32, true), (0.0, false)] {
            surface(
                &mut g,
                &ws,
                &[-0.003, 0.003],
                (false, false),
                flip,
                |w, o| Sample::new(glass(w, h, o), Vec2::new(w, o), tint),
            );
        }
        orient_outward(&mut g);
        kit.add(Surface::Glass, &g);
        // Frame: a top rail, end posts and a rubber gasket along the base.
        let mut f = Mesh::default();
        let top: Vec<Vec3> = ws.iter().map(|&w| glass(w, 1.0, 0.0)).collect();
        tube(&mut f, &top, 10, false, &|_| 0.011, rgba(Vec3::splat(0.82)));
        for w in [-1.0f32, 1.0] {
            let post: Vec<Vec3> = hs.iter().map(|&h| glass(w, h, 0.0)).collect();
            tube(
                &mut f,
                &post,
                10,
                false,
                &|_| 0.012,
                rgba(Vec3::splat(0.82)),
            );
        }
        kit.add(Surface::PaintedMetal, &f);
        let mut gasket = Mesh::default();
        let bottom: Vec<Vec3> = ws.iter().map(|&w| glass(w, 0.0, 0.0)).collect();
        tube(
            &mut gasket,
            &bottom,
            8,
            false,
            &|_| 0.012,
            rgba(Vec3::splat(0.6)),
        );
        kit.add(Surface::Rubber, &gasket);
    }

    // --- Helm console, wheel and gauges -----------------------------------------------------------
    {
        let (z0, z1) = (0.2f32, 0.66f32);
        let profile = [
            Vec2::new(-0.3, floor_y),
            Vec2::new(0.3, floor_y),
            Vec2::new(0.3, 0.36),
            Vec2::new(0.12, 0.56),
            Vec2::new(-0.04, 0.6),
            Vec2::new(-0.14, 0.56),
            Vec2::new(-0.3, 0.3),
        ];
        let mut m = Mesh::default();
        prism(&mut m, &profile, z0, z1, Mat4::IDENTITY, rgba(white));
        kit.add(Surface::Gelcoat, &m);
        // The dash: a dark panel on the sloped face, two gauges and the wheel.
        let d0 = Vec3::new(-0.14, 0.56, 0.0);
        let d1 = Vec3::new(-0.3, 0.3, 0.0);
        let along = (d0 - d1).normalize();
        // Facing the driver: aft and up.
        let face_n = Vec3::new(-along.y, along.x, 0.0);
        let mid = (d0 + d1) * 0.5 + Vec3::new(0.0, 0.0, (z0 + z1) * 0.5);
        let panel = Mat4::from_cols(
            Vec3::Z.extend(0.0),
            along.extend(0.0),
            Vec3::Z.cross(along).extend(0.0),
            (mid + face_n * 0.004).extend(1.0),
        );
        let mut dash = Mesh::default();
        chamfer_box(
            &mut dash,
            Vec3::new(0.2, 0.12, 0.006),
            0.004,
            panel,
            rgba(Vec3::splat(0.12)),
        );
        let mut gauge_glass = Mesh::default();
        for (dz, dy) in [(-0.11f32, 0.06f32), (0.11, 0.06)] {
            let c = mid + Vec3::Z * dz + along * dy + face_n * 0.012;
            let ring = arc(c, Vec3::Z, along, 0.042, 0.0, TAU, 24);
            tube(
                &mut dash,
                &ring[..24],
                8,
                true,
                &|_| 0.006,
                rgba(Vec3::splat(0.85)),
            );
            let disc: Vec<Vec3> = arc(c, Vec3::Z, along, 0.038, 0.0, TAU, 24)[..24].to_vec();
            flat_polygon(
                &mut gauge_glass,
                &disc,
                face_n,
                Vec4::new(0.2, 0.22, 0.24, 1.0),
            );
        }
        kit.add(Surface::PaintedMetal, &dash);
        kit.add(Surface::Glass, &gauge_glass);
        // Steering wheel: rim, three spokes, hub and column.
        let hub = mid - along * 0.06 + face_n * 0.12;
        let tilt_u = Vec3::Z;
        let tilt_v = along;
        let rim = arc(hub, tilt_u, tilt_v, 0.17, 0.0, TAU, 40);
        let mut wheel = Mesh::default();
        tube(
            &mut wheel,
            &rim[..40],
            12,
            true,
            &|_| 0.014,
            rgba(Vec3::splat(0.9)),
        );
        kit.add(Surface::Rubber, &wheel);
        let mut metal = Mesh::default();
        for k in 0..3 {
            let a = FRAC_PI_2 + k as f32 * TAU / 3.0;
            let dir = tilt_u * a.cos() + tilt_v * a.sin();
            tube(
                &mut metal,
                &[hub + dir * 0.03, hub + dir * 0.165 - face_n * 0.01],
                8,
                false,
                &|_| 0.008,
                rgba(Vec3::splat(0.85)),
            );
        }
        tube(
            &mut metal,
            &[hub - face_n * 0.13, hub + face_n * 0.025],
            12,
            false,
            &|s| 0.022 + 0.012 * s,
            rgba(Vec3::splat(0.3)),
        );
        // Throttle lever on the inboard side.
        let pivot = Vec3::new(-0.05, 0.42, z0 - 0.02);
        tube(
            &mut metal,
            &[pivot, pivot + Vec3::new(0.02, 0.0, -0.04)],
            10,
            false,
            &|_| 0.03,
            rgba(Vec3::splat(0.15)),
        );
        tube(
            &mut metal,
            &[
                pivot + Vec3::new(0.0, 0.0, -0.03),
                pivot + Vec3::new(-0.08, 0.16, -0.05),
            ],
            8,
            false,
            &|_| 0.008,
            rgba(Vec3::splat(0.85)),
        );
        kit.add(Surface::PaintedMetal, &metal);
        let mut knob = Mesh::default();
        rounded_box(
            &mut knob,
            Vec3::new(0.025, 0.02, 0.05),
            0.018,
            4,
            at_(pivot + Vec3::new(-0.085, 0.17, -0.06)),
            rgba(Vec3::splat(0.2)),
        );
        kit.add(Surface::Rubber, &knob);
    }

    // --- Seats ----------------------------------------------------------------------------------
    let vinyl = rgba(Vec3::new(0.93, 0.92, 0.88));
    let piping = rgba(navy);
    for z in [0.43f32, -0.43] {
        let x = -0.78;
        let mut metal = Mesh::default();
        tube(
            &mut metal,
            &[Vec3::new(x, floor_y, z), Vec3::new(x, 0.2, z)],
            14,
            false,
            &|_| 0.035,
            rgba(Vec3::splat(0.75)),
        );
        chamfer_box(
            &mut metal,
            Vec3::new(0.12, 0.006, 0.12),
            0.004,
            at_(Vec3::new(x, floor_y + 0.006, z)),
            rgba(Vec3::splat(0.75)),
        );
        kit.add(Surface::PaintedMetal, &metal);
        let mut seat = Mesh::default();
        rounded_box(
            &mut seat,
            Vec3::new(0.24, 0.035, 0.23),
            0.03,
            6,
            at_(Vec3::new(x, 0.24, z)),
            rgba(Vec3::splat(0.35)),
        );
        rounded_box(
            &mut seat,
            Vec3::new(0.25, 0.055, 0.24),
            0.05,
            6,
            at_(Vec3::new(x + 0.01, 0.31, z)),
            vinyl,
        );
        let back =
            Mat4::from_translation(Vec3::new(x - 0.25, 0.56, z)) * Mat4::from_rotation_z(0.22);
        rounded_box(
            &mut seat,
            Vec3::new(0.055, 0.26, 0.24),
            0.05,
            6,
            back,
            vinyl,
        );
        // Side bolsters in the accent color.
        for s in [-1.0f32, 1.0] {
            let bol = Mat4::from_translation(Vec3::new(x - 0.2, 0.56, z + s * 0.22))
                * Mat4::from_rotation_z(0.22);
            rounded_box(
                &mut seat,
                Vec3::new(0.07, 0.24, 0.035),
                0.03,
                4,
                bol,
                piping,
            );
        }
        kit.add(Surface::Fabric, &seat);
    }
    {
        // Stern bench across the back of the cockpit.
        let x = -2.0;
        let w = cockpit_half(x) - 0.05;
        let mut seat = Mesh::default();
        rounded_box(
            &mut seat,
            Vec3::new(0.2, 0.13, w),
            0.04,
            6,
            at_(Vec3::new(x, floor_y + 0.13, 0.0)),
            rgba(white * 0.97),
        );
        kit.add(Surface::Gelcoat, &seat);
        let mut cushion = Mesh::default();
        rounded_box(
            &mut cushion,
            Vec3::new(0.21, 0.05, w - 0.02),
            0.045,
            6,
            at_(Vec3::new(x, floor_y + 0.31, 0.0)),
            vinyl,
        );
        let back =
            Mat4::from_translation(Vec3::new(x - 0.17, 0.5, 0.0)) * Mat4::from_rotation_z(0.12);
        rounded_box(
            &mut cushion,
            Vec3::new(0.05, 0.17, w - 0.04),
            0.045,
            6,
            back,
            vinyl,
        );
        kit.add(Surface::Fabric, &cushion);
    }

    // --- Cleats -----------------------------------------------------------------------------------
    {
        let steel = rgba(Vec3::splat(0.85));
        let mut m = Mesh::default();
        let bow = cleat_mesh(0.65, steel);
        let x = 2.2;
        m.merge(&moved(
            bow,
            at_(Vec3::new(x, mb_deck_y(x, 0.0) - 0.002, 0.0)),
        ));
        for z in [0.72f32, -0.72] {
            let x = -2.42;
            let c = cleat_mesh(0.55, steel);
            m.merge(&moved(c, at_(Vec3::new(x, mb_deck_y(x, z) - 0.004, z))));
        }
        kit.add(Surface::PaintedMetal, &m);
    }

    // --- Outboard motor -----------------------------------------------------------------------------
    {
        let graphite = rgba(Vec3::splat(0.09));
        let silver = rgba(Vec3::splat(0.6));
        let mut body = Mesh::default();
        // Transom clamp bracket and swivel bracket.
        chamfer_box(
            &mut body,
            Vec3::new(0.07, 0.11, 0.13),
            0.012,
            at_(Vec3::new(-2.63, 0.3, 0.0)),
            graphite,
        );
        chamfer_box(
            &mut body,
            Vec3::new(0.06, 0.13, 0.08),
            0.015,
            at_(Vec3::new(-2.74, 0.36, 0.0)),
            graphite,
        );
        for z in [-0.09f32, 0.09] {
            tube(
                &mut body,
                &[Vec3::new(-2.6, 0.33, z), Vec3::new(-2.52, 0.33, z)],
                10,
                false,
                &|_| 0.012,
                silver,
            );
        }
        // Cowling: a rounded-box hood over a slightly narrower lower pan (the seam between
        // them shows as a crisp shadow line), then the chaps narrowing into the midsection.
        let (x0, x1) = (-3.1f32, -2.56f32);
        let pod = |t: f32, a: f32, bottom: f32, top: &dyn Fn(f32) -> f32, half_w: f32, k: f32| {
            let x = x0 + (x1 - x0) * t;
            // Quarter-round ends so the loft closes smoothly front and back.
            let d = (t.min(1.0 - t) / 0.16).min(1.0);
            let e = (1.0 - (1.0 - d).powi(2)).max(0.0).sqrt();
            let w = half_w * (0.92 + 0.08 * t) * e;
            let tt = top(t);
            let half_h = (tt - bottom) * 0.5 * (0.25 + 0.75 * e);
            let cy = (tt + bottom) * 0.5;
            let th = a * TAU;
            let (c, s) = (th.cos(), th.sin());
            Vec3::new(
                x,
                cy + s.signum() * s.abs().powf(k) * half_h,
                c.signum() * c.abs().powf(k) * w,
            )
        };
        let hood_top = |t: f32| 0.86 + 0.1 * smooth(0.0, 0.55, t) - 0.03 * smooth(0.7, 1.0, t);
        let pan_top = |_: f32| 0.615;
        let mut cowl = Mesh::default();
        surface(
            &mut cowl,
            &steps(0.0, 1.0, 36),
            &steps(0.0, 1.0, 30),
            (true, false),
            false,
            |a, t| {
                let p = pod(t, a, 0.6, &hood_top, 0.205, 0.38);
                let band = smooth(0.635, 0.64, p.y) * (1.0 - smooth(0.668, 0.673, p.y));
                let c = graphite.lerp(rgba(Vec3::splat(0.72)), band);
                Sample::new(p, Vec2::new(p.x, p.y + p.z), c)
            },
        );
        orient_outward(&mut cowl);
        body.merge(&cowl);
        let mut pan = Mesh::default();
        surface(
            &mut pan,
            &steps(0.0, 1.0, 28),
            &steps(0.0, 1.0, 22),
            (true, false),
            false,
            |a, t| {
                let p = pod(t, a, 0.44, &pan_top, 0.19, 0.42);
                Sample::new(p, Vec2::new(p.x, p.y + p.z), rgba(Vec3::splat(0.14)))
            },
        );
        orient_outward(&mut pan);
        body.merge(&pan);
        // Chaps and midsection: a streamlined column down to the gearcase.
        let foil = |x_le: f32, chord: f32, thick: f32, a: f32| -> (f32, f32) {
            // Teardrop section: leading edge at +x.
            let th = a * TAU;
            let c = th.cos();
            let x = x_le - chord * (1.0 - c) * 0.5;
            let t = (1.0 - c) * 0.5;
            let half = thick * 0.5 * 2.6 * (t.max(0.0).sqrt() * (1.0 - t)).max(0.0);
            (x, half * th.sin().signum())
        };
        let mut leg = Mesh::default();
        let mut ys = steps(0.0, 1.0, 16);
        ys.extend_from_slice(&[0.04, 0.08, 0.12]);
        ys.sort_by(f32::total_cmp);
        surface(
            &mut leg,
            &steps(0.0, 1.0, 22),
            &ys,
            (true, false),
            false,
            |a, t| {
                let y = 0.46 - 0.94 * t;
                let flare = 1.0 - smooth(0.0, 0.16, t);
                let chord = 0.24 + 0.2 * flare;
                let thick = 0.085 + 0.22 * flare;
                let (x, z) = foil(-2.62 - 0.02 * (1.0 - flare), chord, thick, a);
                Sample::new(Vec3::new(x, y, z), Vec2::new(x, y), graphite)
            },
        );
        orient_outward(&mut leg);
        body.merge(&leg);
        // Gearcase torpedo with the propeller shaft at its tail.
        let mut gear = Mesh::default();
        let (gx0, gx1, gy) = (-2.64f32, -2.94f32, -0.53f32);
        surface(
            &mut gear,
            &steps(0.0, 1.0, 16),
            &steps(0.0, 1.0, 16),
            (true, false),
            false,
            |a, t| {
                let x = gx0 + (gx1 - gx0) * t;
                let r = 0.058 * (1.0 - (1.0 - t).powi(4)).max(0.0).sqrt() * (1.0 - 0.25 * t);
                let th = a * TAU;
                Sample::new(
                    Vec3::new(x, gy + r * th.cos(), r * th.sin()),
                    Vec2::new(x, a * 0.3),
                    graphite,
                )
            },
        );
        orient_outward(&mut gear);
        body.merge(&gear);
        // Skeg below the gearcase.
        let mut skeg = Mesh::default();
        surface(
            &mut skeg,
            &steps(0.0, 1.0, 12),
            &steps(0.0, 1.0, 6),
            (true, false),
            false,
            |a, t| {
                let y = gy - 0.02 - 0.17 * t;
                let chord = 0.2 - 0.08 * t;
                let (x, z) = foil(-2.7 - 0.06 * t, chord, 0.024, a);
                Sample::new(Vec3::new(x, y, z), Vec2::new(x, y), graphite)
            },
        );
        orient_outward(&mut skeg);
        body.merge(&skeg);
        // Cavitation plate.
        let plate: Vec<Vec2> = (0..24)
            .map(|i| {
                let q = rounded_rect(i as f32 / 24.0, Vec2::new(0.16, 0.12), 0.06);
                Vec2::new(-2.84 + q.x, q.y * (0.75 + 0.25 * (q.x + 0.16) / 0.32))
            })
            .collect();
        let xf = Mat4::from_cols(Vec4::X, Vec4::NEG_Z, Vec4::Y, Vec4::W);
        prism(&mut body, &plate, -0.415, -0.4, xf, graphite);
        kit.add(Surface::PaintedMetal, &body);
        // Propeller: hub and three twisted, raked blades.
        let mut prop = Mesh::default();
        let axis_y = gy;
        let px = -2.96;
        surface(
            &mut prop,
            &steps(0.0, 1.0, 16),
            &steps(0.0, 1.0, 8),
            (true, false),
            false,
            |a, t| {
                let x = px + 0.03 - 0.13 * t;
                let r = 0.042 * (1.0 - 0.45 * t * t) * (1.0 - smooth(0.85, 1.0, t) * 0.6);
                let th = a * TAU;
                Sample::new(
                    Vec3::new(x, axis_y + r * th.cos(), r * th.sin()),
                    Vec2::new(x, a * 0.25),
                    silver,
                )
            },
        );
        let (r_hub, r_tip, pitch) = (0.036f32, 0.165f32, 0.33f32);
        for k in 0..3 {
            let phi0 = k as f32 * TAU / 3.0;
            let mut blade = Mesh::default();
            surface(
                &mut blade,
                &steps(0.0, 1.0, 16),
                &steps(0.0, 1.0, 10),
                (true, false),
                false,
                |a, t| {
                    let r = r_hub + (r_tip - r_hub) * t;
                    let th = a * TAU;
                    let c = th.cos();
                    // Angular half-width: wide through the middle, rounded at the tip.
                    let span = 0.62 * (1.0 - t * t).max(0.0).sqrt() * (0.75 + 0.5 * t * (1.0 - t));
                    let psi = phi0 + c * span + 0.25 * t;
                    let thick = 0.009 * (1.0 - 0.7 * t) * th.sin();
                    let x = px - 0.04 - pitch / TAU * c * span - 0.12 * (r - r_hub) + thick;
                    Sample::new(
                        Vec3::new(x, axis_y + r * psi.cos(), r * psi.sin()),
                        Vec2::new(c, r),
                        silver,
                    )
                },
            );
            orient_outward(&mut blade);
            prop.merge(&blade);
        }
        kit.add(Surface::PaintedMetal, &prop);
    }
    kit.finish()
}

// ---------------------------------------------------------------------------------------------
// Jetty: deck, posts, cleat and ladder
// ---------------------------------------------------------------------------------------------

/// Board width of the jetty deck, meters, and the stringers' positions along the boards (z).
const BOARD_W: f32 = 0.16;
const STRINGERS_Z: [f32; 3] = [-0.92, 0.0, 0.92];

/// `u` per meter around something round so its circumference holds a whole number of
/// texture tiles (no seam).
fn wrap_scale(surface: Surface, circumference: f32) -> f32 {
    let tile = super::materials::tile_size(surface)[0].max(1e-3);
    let tiles = (circumference / tile).round().max(1.0);
    tiles * tile / circumference
}

/// One 0.55 m slot of the jetty deck: x across the slot (centered), z along the boards
/// (2.0 m, centered), board tops at y = +0.035.
///
/// Three 0.16 m boards with 2.3 cm gaps, 4.5 cm thick, eased edges, a little cup, twist,
/// bow and height jitter per seed; below them the slot's share of three 7.5 × 20 cm
/// stringers running along x at z = -0.92, 0 and +0.92 (bottoms at y = -0.21). The plank
/// texture's nail rows line up over the stringers. Give consecutive slots consecutive seeds
/// and the stringers' grain runs on from one slot to the next. About 1 200 triangles.
pub fn deck_section(seed: u64) -> Vec<Part> {
    let mut rng = Rng::new(hash_u64(seed ^ 0xdec4));
    let mut m = Mesh::default();
    let pitch = 0.55 / 3.0;
    let h = Vec2::new(BOARD_W * 0.5, 0.0225);
    let r = 0.006;
    // Texture v: nail rows (every 0.667 m) over the stringers.
    let v_scale = 0.6667 / STRINGERS_Z[2];
    for b in 0..3 {
        let xc = (b as f32 - 1.0) * pitch + rng.range(-0.003, 0.003);
        let cup = rng.range(-0.0025, 0.0025);
        let twist = rng.range(-0.004, 0.004);
        let bow = rng.range(-0.004, 0.004);
        let lift = rng.range(-0.0015, 0.0015);
        let yaw = rng.range(-0.004, 0.004);
        let ends = [
            -1.0 + rng.range(-0.012, 0.004),
            1.0 + rng.range(-0.004, 0.012),
        ];
        // A whole number of virtual boards over in u so neighbors differ.
        let u0 = (rng.range_i32(0, 3) + b * 4) as f32 * (0.5 / 3.0);
        let tone = rng.range(0.85, 1.05);
        let pos = move |q: Vec2, z: f32| -> Vec3 {
            let f = q.x / h.x;
            let zz = z / 1.0;
            let dy = cup * f * f + twist * f * zz + bow * (1.0 - zz * zz) + lift;
            Vec3::new(xc + q.x + yaw * z, 0.0125 + q.y + dy, z)
        };
        let perim = 4.0 * (h.x + h.y);
        let zs = steps(ends[0], ends[1], 10);
        let color = move |p: Vec3| {
            // Darker toward the ends and the edges where water soaks in.
            let end = 1.0 - smooth(0.85, 1.0, p.z.abs()) * 0.25;
            rgba(Vec3::splat(tone * end))
        };
        surface(
            &mut m,
            &steps(0.0, 1.0, 16),
            &zs,
            (true, false),
            false,
            |a, z| {
                let q = rounded_rect(a, h, r);
                let p = pos(q, z);
                // u runs across the board's top centered on a virtual board.
                let u = u0 - (a * perim - (h.y - r) - r * FRAC_PI_2 - (h.x - r));
                Sample::new(p, Vec2::new(u, z * v_scale + 0.6667), color(p))
            },
        );
        for (z, sign) in [(ends[0], -1.0f32), (ends[1], 1.0)] {
            let ring: Vec<Vec3> = (0..16)
                .map(|k| pos(rounded_rect(k as f32 / 16.0, h, r), z))
                .collect();
            flat_polygon(
                &mut m,
                &ring,
                Vec3::Z * sign,
                color(Vec3::new(0.0, 0.0, z)) * 0.8,
            );
        }
    }
    // Stringers along x under the boards.
    for z in STRINGERS_Z {
        let frames = path_frames(
            &[Vec3::new(-0.275, -0.11, z), Vec3::new(0.275, -0.11, z)],
            |_| Vec3::Z,
        );
        let mut s = Mesh::default();
        sweep_rect(&mut s, &frames, Vec2::new(0.0375, 0.1), 0.008, 12, &|p| {
            rgba(Vec3::splat(0.5 + 0.2 * smooth(-0.21, -0.03, p.y)))
        });
        box_uv(&mut s, 0);
        // Slots laid out in seed order continue each other's grain.
        let run = (seed % 64) as f32 * 0.55;
        for v in &mut s.vertices {
            if v.normal[0].abs() < 0.7 {
                v.uv[1] += run;
            }
        }
        m.merge(&s);
    }
    vec![Part::new(m, Surface::Planks)]
}

/// A round timber pile of radius 0.11 m from y = 0 to `height`, with a chamfered, sawn top
/// (end grain), a slight irregular taper and two cracks. Darkened, with green algae from the
/// bottom up to `waterline` (meters above its base) and a wet band just above. At most
/// 2 000 triangles.
pub fn post(height: f32, waterline: f32, seed: u64) -> Vec<Part> {
    let mut rng = Rng::new(hash_u64(seed ^ 0x9057));
    let r0 = 0.11;
    let chamfer = 0.016;
    // Two cracks: (angle, half width in radians, bottom, top, depth).
    let cracks: Vec<(f32, f32, f32, f32, f32)> = (0..2)
        .map(|_| {
            let a = rng.range(0.0, TAU);
            let len = rng.range(0.4, 1.2).min(height * 0.6);
            let top = rng.range(height * 0.5, height - 0.02).max(len);
            (
                a,
                rng.range(0.03, 0.05),
                top - len,
                top,
                rng.range(0.008, 0.014),
            )
        })
        .collect();
    let lean = Vec2::new(rng.range(-0.01, 0.01), rng.range(-0.01, 0.01));
    let noise_seed = rng.next_u64();
    let crack_list = cracks.clone();
    let radius = move |th: f32, y: f32| -> f32 {
        let t = y / height.max(0.1);
        let mut r = r0 * (1.03 - 0.06 * t);
        let (c, s) = (th.cos() as f64, th.sin() as f64);
        r *= 1.0 + 0.035 * noise::fbm3(c * 1.2, s * 1.2, y as f64 * 0.8, noise_seed, 3, 2.0, 0.5);
        for &(a, w, y0, y1, d) in &crack_list {
            let dth = ((th - a + PI).rem_euclid(TAU) - PI).abs();
            let along = smooth(y0, y0 + 0.08, y) * (1.0 - smooth(y1 - 0.08, y1, y));
            r -= d * along * (1.0 - smooth(0.0, w, dth));
        }
        // Chamfer at the top.
        r - (y - (height - chamfer)).max(0.0)
    };
    // Angles: an even ring plus extra samples across each crack.
    let mut ths: Vec<f32> = (0..=18).map(|i| i as f32 / 18.0).collect();
    for &(a, w, ..) in &cracks {
        for f in [-1.0f32, -0.5, 0.0, 0.5, 1.0] {
            ths.push((a + f * w).rem_euclid(TAU) / TAU);
        }
    }
    ths.sort_by(f32::total_cmp);
    ths.dedup_by(|a, b| (*a - *b).abs() < 0.004);
    // Heights: denser around the waterline bands and the chamfer.
    let mut ys: Vec<f32> = Vec::new();
    let rows = ((height / 0.2).ceil() as usize).clamp(4, 22);
    ys.extend(steps(0.0, height - chamfer, rows));
    for y in [
        waterline - 0.12,
        waterline - 0.03,
        waterline,
        waterline + 0.04,
        waterline + 0.14,
        waterline + 0.3,
        height - chamfer * 0.5,
        height,
    ] {
        if y > 0.0 && y <= height {
            ys.push(y);
        }
    }
    for &(_, _, y0, y1, _) in &cracks {
        ys.extend_from_slice(&[y0 + 0.04, y1 - 0.04]);
    }
    ys.retain(|y| (0.0..=height).contains(y));
    ys.sort_by(f32::total_cmp);
    ys.dedup_by(|a, b| (*a - *b).abs() < 0.01);
    let us = wrap_scale(Surface::Post, TAU * r0);
    let color_seed = rng.next_u64();
    let color = move |p: Vec3, th: f32| -> Vec4 {
        let y = p.y;
        let n = noise::fbm3(
            th.cos() as f64 * 2.0,
            th.sin() as f64 * 2.0,
            y as f64 * 3.0,
            color_seed,
            3,
            2.0,
            0.5,
        );
        // Weathered, slightly silvered wood above; wet and dark just over the water; green
        // and brown growth below it, brightest at the waterline.
        let mut c = Vec3::splat(1.15 + 0.15 * n);
        let wet = 1.0 - smooth(waterline, waterline + 0.28, y);
        c = c.lerp(Vec3::splat(0.55), wet);
        let under = 1.0 - smooth(waterline - 0.02, waterline + 0.03, y);
        let slime = Vec3::new(1.3, 2.0, 0.9) * (0.75 + 0.35 * n);
        let deep = Vec3::new(0.9, 1.1, 0.65) * (0.8 + 0.3 * n);
        let band = 1.0 - smooth(0.0, 0.25, (waterline - 0.06 - y).abs());
        c = c.lerp(deep.lerp(slime, band), under);
        rgba(c)
    };
    let mut m = Mesh::default();
    surface(&mut m, &ths, &ys, (true, false), false, |a, y| {
        let th = a * TAU;
        let r = radius(th, y);
        let off = lean * (y / height.max(0.1));
        let p = Vec3::new(r * th.cos() + off.x, y, -r * th.sin() + off.y);
        Sample::new(p, Vec2::new(a * TAU * r0 * us, y), color(p, th))
    });
    // The bottom (buried or in the mud) and the sawn top.
    let ring = |y: f32| -> Vec<Vec3> {
        ths[..ths.len() - 1]
            .iter()
            .map(|&a| {
                let th = a * TAU;
                let r = radius(th, y);
                let off = lean * (y / height.max(0.1));
                Vec3::new(r * th.cos() + off.x, y, -r * th.sin() + off.y)
            })
            .collect()
    };
    let bottom = ring(0.0);
    flat_polygon(&mut m, &bottom, Vec3::NEG_Y, rgba(Vec3::splat(0.6)));
    let mut top = Mesh::default();
    let ring_top = ring(height);
    let center = ring_top.iter().copied().sum::<Vec3>() / ring_top.len() as f32;
    // Weathered end grain: grayed and darkened.
    flat_polygon(
        &mut top,
        &ring_top,
        Vec3::Y,
        rgba(Vec3::new(0.6, 0.68, 0.82)),
    );
    for v in &mut top.vertices {
        v.uv = [v.position[0] - center.x, v.position[2] - center.z];
    }
    vec![
        Part::new(m, Surface::Post),
        Part::new(top, Surface::EndGrain),
    ]
}

/// A cast dock cleat along x, 0.25 m horn to horn and 0.06 m tall, base at y = 0 (painted
/// black iron).
pub fn cleat() -> Vec<Part> {
    vec![Part::new(
        cleat_mesh(1.0, rgba(Vec3::splat(0.12))),
        Surface::PaintedMetal,
    )]
}

/// A galvanized swim ladder hung on the end of the jetty. Frame: the deck top at y = 0, the
/// jetty's end face at x = 0 with open water toward +x. The two 42 mm rails (z = ±0.25)
/// stand at x = +0.06 from y = -`height` up to 0.55 above the deck, then bend back over it
/// and down to flanges on the deck at x = -0.36. Treads every 0.28 m, standoffs with rubber
/// bumpers against the jetty face.
pub fn ladder(height: f32) -> Vec<Part> {
    let mut metal = Mesh::default();
    let mut rubber = Mesh::default();
    let zinc = rgba(Vec3::splat(0.95));
    let (xr, top, bend) = (0.06f32, 0.55f32, 0.21f32);
    for z in [-0.25f32, 0.25] {
        let mut path: Vec<Vec3> = vec![Vec3::new(xr, -height, z)];
        path.extend(
            steps(-height + 0.1, top, 8)
                .into_iter()
                .map(|y| Vec3::new(xr, y, z)),
        );
        let arc_pts = arc(
            Vec3::new(xr - bend, top, z),
            Vec3::X,
            Vec3::Y,
            bend,
            0.0,
            PI,
            16,
        );
        path.extend(arc_pts.into_iter().skip(1));
        path.extend(
            steps(top - 0.1, 0.012, 4)
                .into_iter()
                .map(|y| Vec3::new(xr - 2.0 * bend, y, z)),
        );
        tube(&mut metal, &path, 14, false, &|_| 0.021, zinc);
        // Deck flange with bolt heads.
        let flange = Vec3::new(xr - 2.0 * bend, 0.0, z);
        tube(
            &mut metal,
            &[flange, flange + Vec3::Y * 0.012],
            20,
            false,
            &|_| 0.055,
            zinc,
        );
        for k in 0..4 {
            let a = k as f32 * FRAC_PI_2 + 0.785;
            let b = flange + Vec3::new(a.cos() * 0.04, 0.012, a.sin() * 0.04);
            tube(
                &mut metal,
                &[b, b + Vec3::Y * 0.006],
                6,
                false,
                &|_| 0.008,
                zinc,
            );
        }
        // Standoffs to the jetty face, with rubber bumpers.
        for y in [-0.12f32, -height + 0.45] {
            let p = Vec3::new(xr, y, z);
            tube(
                &mut metal,
                &[p, Vec3::new(0.018, y, z)],
                10,
                false,
                &|_| 0.014,
                zinc,
            );
            tube(
                &mut rubber,
                &[Vec3::new(0.018, y, z), Vec3::new(0.0, y, z)],
                14,
                false,
                &|_| 0.03,
                rgba(Vec3::splat(0.8)),
            );
        }
    }
    // Treads: flat bars with a raised nosing, welded between the rails.
    let mut y = -height + 0.3;
    while y < -0.12 {
        chamfer_box(
            &mut metal,
            Vec3::new(0.045, 0.01, 0.235),
            0.003,
            at_(Vec3::new(xr - 0.01, y, 0.0)),
            rgba(Vec3::splat(0.85)),
        );
        tube(
            &mut metal,
            &[
                Vec3::new(xr + 0.035, y + 0.011, -0.232),
                Vec3::new(xr + 0.035, y + 0.011, 0.232),
            ],
            8,
            false,
            &|_| 0.006,
            zinc,
        );
        y += 0.28;
    }
    vec![
        Part::new(metal, Surface::Galvanized),
        Part::new(rubber, Surface::Rubber),
    ]
}

// ---------------------------------------------------------------------------------------------
// Buoy
// ---------------------------------------------------------------------------------------------

/// A mooring buoy about 0.32 m in radius centered on the origin: a molded lathe body with
/// its parting-line seam band, a 0.5 m marker spar on top (to y ≈ 0.8) with a galvanized
/// lifting eye (to 0.9), and a bottom pad eye with a shackle and two chain links (down to
/// y ≈ -0.55). Red when `seed` is even, white when odd; algae below the waterline. About
/// 4 700 triangles.
pub fn buoy(seed: u64) -> Vec<Part> {
    let red = srgb(0xc22a1d);
    let white = Vec3::new(0.92, 0.92, 0.9);
    let (body, accent) = if seed.is_multiple_of(2) {
        (red, white)
    } else {
        (white, red)
    };
    let grime_seed = hash_u64(seed ^ 0xb0e);
    let tint = move |p: Vec3, base: Vec3| -> Vec4 {
        let n = fbm3(p * 9.0, grime_seed, 3);
        // A ragged waterline a little below the seam: olive growth turning brown deeper
        // down, a bright slime line at the water and a brown scum line just above it.
        let wl = -0.08 + 0.02 * fbm3(Vec3::new(p.x * 4.0, 0.0, p.z * 4.0), grime_seed ^ 3, 2);
        let below = 1.0 - smooth(wl - 0.03, wl + 0.004, p.y);
        let deep = smooth(wl - 0.05, wl - 0.25, p.y);
        let algae =
            Vec3::new(0.3, 0.34, 0.18).lerp(Vec3::new(0.17, 0.14, 0.08), deep) * (0.8 + 0.4 * n);
        let slime = 1.0 - smooth(0.0, 0.016, (p.y - (wl - 0.012)).abs());
        let scum = 1.0 - smooth(0.0, 0.012, (p.y - (wl + 0.012)).abs());
        let mut c = base.lerp(algae, below * 0.92);
        c = c.lerp(Vec3::new(0.3, 0.45, 0.16), slime * 0.6);
        c = c.lerp(base * Vec3::new(0.7, 0.6, 0.48), scum * 0.5);
        // Scuffs, chalking, and streaks running down from the spar.
        let streak = fbm3(
            Vec3::new(p.x * 30.0, p.y * 3.0, p.z * 30.0),
            grime_seed ^ 9,
            2,
        );
        c *= (0.93 + 0.1 * n) * (1.0 - 0.14 * smooth(0.25, 0.6, streak) * smooth(-0.05, 0.25, p.y));
        rgba(c)
    };
    let mut plastic = Mesh::default();
    // Lower body from the bottom pad to the seam band, the band itself (creased), then the
    // upper body to the neck that holds the spar.
    let lower: Vec<Vec2> = {
        let mut v = vec![Vec2::new(0.0, -0.305), Vec2::new(0.055, -0.305)];
        for i in 1..=12 {
            let a = -FRAC_PI_2 + i as f32 / 12.0 * FRAC_PI_2;
            let r = 0.32 * a.cos().powf(0.9);
            let y = 0.31 * a.sin() - 0.012 * (1.0 - a.cos());
            // Only the curve above the flat bottom pad.
            if y > -0.3 && r > 0.06 {
                v.push(Vec2::new(r, y));
            }
        }
        v.last_mut().expect("profile").y = -0.012;
        v
    };
    lathe(&mut plastic, &lower, 36, 0.32, 0.0, &|p| tint(p, body));
    let band = [
        Vec2::new(0.32, -0.012),
        Vec2::new(0.336, -0.009),
        Vec2::new(0.338, 0.0),
        Vec2::new(0.336, 0.009),
        Vec2::new(0.32, 0.012),
    ];
    lathe(&mut plastic, &band, 36, 0.32, 0.0, &|p| tint(p, body) * 0.9);
    let upper: Vec<Vec2> = {
        let mut v = Vec::new();
        for i in 0..=12 {
            let a = i as f32 / 12.0 * FRAC_PI_2;
            let r = 0.32 * a.cos().powf(0.9);
            // Up to the neck that holds the spar.
            if r > 0.085 {
                v.push(Vec2::new(r, 0.012 + 0.3 * a.sin()));
            }
        }
        v.push(Vec2::new(0.072, 0.302));
        v.push(Vec2::new(0.068, 0.33));
        v.push(Vec2::new(0.0, 0.33));
        v
    };
    lathe(&mut plastic, &upper, 36, 0.32, 0.0, &|p| tint(p, body));
    // The spar: tapered, banded, with a white reflective collar near the top.
    let spar: Vec<Vec2> = vec![
        Vec2::new(0.0, 0.33),
        Vec2::new(0.055, 0.33),
        Vec2::new(0.05, 0.4),
        Vec2::new(0.045, 0.55),
        Vec2::new(0.04, 0.7),
        Vec2::new(0.036, 0.78),
        Vec2::new(0.034, 0.8),
        Vec2::new(0.0, 0.805),
    ];
    let mut ys: Vec<Vec2> = Vec::new();
    for w in spar.windows(2) {
        for k in 0..4 {
            ys.push(w[0].lerp(w[1], k as f32 / 4.0));
        }
    }
    ys.push(*spar.last().expect("spar"));
    lathe(&mut plastic, &ys, 20, 0.05, 0.33, &|p| {
        let c = if (0.66..0.71).contains(&p.y) {
            Vec3::splat(0.97)
        } else if ((p.y - 0.33) / 0.11) as i32 % 2 == 0 {
            accent
        } else {
            body
        };
        rgba(c)
    });
    // Metal: the lifting eye on top, the pad eye, shackle and chain below.
    let zinc = rgba(Vec3::splat(0.75));
    let mut metal = Mesh::default();
    tube(
        &mut metal,
        &[Vec3::new(0.0, 0.8, 0.0), Vec3::new(0.0, 0.83, 0.0)],
        10,
        false,
        &|_| 0.012,
        zinc,
    );
    let eye = arc(
        Vec3::new(0.0, 0.862, 0.0),
        Vec3::X,
        Vec3::Y,
        0.032,
        0.0,
        TAU,
        24,
    );
    tube(&mut metal, &eye[..24], 8, true, &|_| 0.0075, zinc);
    tube(
        &mut metal,
        &[Vec3::new(0.0, -0.3, 0.0), Vec3::new(0.0, -0.32, 0.0)],
        20,
        false,
        &|_| 0.07,
        zinc,
    );
    let mut pad = vec![Vec3::new(-0.022, -0.312, 0.0)];
    pad.extend(arc(
        Vec3::new(0.0, -0.345, 0.0),
        Vec3::X,
        Vec3::Y,
        0.022,
        PI,
        TAU,
        12,
    ));
    pad.push(Vec3::new(0.022, -0.312, 0.0));
    tube(&mut metal, &pad, 8, false, &|_| 0.008, zinc);
    // Shackle: a bow through the pad eye and its pin.
    let bow = arc(
        Vec3::new(0.0, -0.37, 0.0),
        Vec3::Z,
        Vec3::Y,
        0.028,
        0.0,
        -PI,
        14,
    );
    let mut shackle: Vec<Vec3> = vec![Vec3::new(0.0, -0.33, 0.028)];
    shackle.extend(bow);
    shackle.push(Vec3::new(0.0, -0.33, -0.028));
    tube(&mut metal, &shackle, 8, false, &|_| 0.008, zinc);
    tube(
        &mut metal,
        &[Vec3::new(0.0, -0.33, -0.042), Vec3::new(0.0, -0.33, 0.042)],
        8,
        false,
        &|_| 0.009,
        zinc,
    );
    // Two chain links hanging from the shackle.
    for (k, plane) in [(0usize, Vec3::X), (1, Vec3::Z)] {
        let c = Vec3::new(0.0, -0.43 - k as f32 * 0.07, 0.0);
        let link: Vec<Vec3> = (0..20)
            .map(|i| {
                let a = i as f32 / 20.0 * TAU;
                let y = a.sin() * 0.045;
                let side = a.cos() * 0.022;
                c + Vec3::Y * y + plane * side
            })
            .collect();
        tube(&mut metal, &link, 8, true, &|_| 0.007, zinc);
    }
    vec![
        Part::new(plastic, Surface::Plastic),
        Part::new(metal, Surface::Galvanized),
    ]
}

// ---------------------------------------------------------------------------------------------
// Log
// ---------------------------------------------------------------------------------------------

/// Drop the triangles none of whose corners has a color alpha above `min`, then the vertices
/// left unused, and make every color opaque again (alpha carries a build-time mask).
fn keep_by_alpha(m: &mut Mesh, min: f32) {
    let alpha = |i: u32| m.vertices[i as usize].color[3];
    let kept: Vec<u32> = m
        .indices
        .chunks(3)
        .filter(|t| t.iter().any(|&i| alpha(i) > min))
        .flatten()
        .copied()
        .collect();
    let mut remap = vec![u32::MAX; m.vertices.len()];
    let mut vertices = Vec::new();
    let mut indices = Vec::with_capacity(kept.len());
    for i in kept {
        if remap[i as usize] == u32::MAX {
            remap[i as usize] = vertices.len() as u32;
            let mut v = m.vertices[i as usize];
            v.color[3] = 1.0;
            vertices.push(v);
        }
        indices.push(remap[i as usize]);
    }
    m.vertices = vertices;
    m.indices = indices;
}

/// Give every triangle corners of its own carrying its face normal: hard facets for broken,
/// splintered surfaces whose smooth normals would disagree with their folds.
fn flat_shade(m: &mut Mesh) {
    let mut vertices = Vec::with_capacity(m.indices.len());
    for t in m.indices.chunks(3) {
        let [a, b, c] = [0, 1, 2].map(|k| m.vertices[t[k] as usize]);
        let p = |v: &Vertex| Vec3::from(v.position);
        let n = (p(&b) - p(&a)).cross(p(&c) - p(&a)).normalize_or_zero();
        if n == Vec3::ZERO {
            continue;
        }
        for mut v in [a, b, c] {
            v.normal = n.to_array();
            vertices.push(v);
        }
    }
    m.indices = (0..vertices.len() as u32).collect();
    m.vertices = vertices;
}

/// A floating log along x, centered: `length` long, `radius` thick at the middle (as used:
/// 1.7 m × 0.17 m). Furrowed bark with lichen on the plates and moss on top has partly
/// fallen away, mostly from the upper side and the ends, baring silver-gray weathered wood
/// with drying checks; the bark's broken edges show its thickness. A slight bend, taper,
/// ovality and lumps; two or three broken branch stubs; a sawn end at -x (end grain with
/// growth rings centered on the pith) and a broken, splintered end at +x. Darker and green
/// where it floats in the water (below y ≈ -0.03). At most 6 000 triangles.
pub fn log(length: f32, radius: f32, seed: u64) -> Vec<Part> {
    let mut rng = Rng::new(hash_u64(seed ^ 0x106));
    let half = length * 0.5;
    let bend = Vec2::new(rng.range(-0.05, 0.05), rng.range(-0.03, 0.03));
    let taper = rng.range(0.1, 0.2);
    let furrows = (TAU * radius / 0.11).round().max(5.0);
    let bark_t = (radius * 0.1).max(0.008);
    let (s1, s2, s3, s4) = (
        rng.next_u64(),
        rng.next_u64(),
        rng.next_u64(),
        rng.next_u64(),
    );
    let cut_tilt = Vec2::new(rng.range(-0.08, 0.08), rng.range(-0.08, 0.08));
    let oval = rng.range(0.0, PI);
    let center = move |x: f32| -> Vec3 {
        let t = x / half;
        Vec3::new(x, bend.x * (1.0 - t * t), bend.y * (1.0 - t * t))
    };
    // The radial direction at angle `th` (0 = up).
    let radial = |th: f32| Vec3::new(0.0, th.cos(), -th.sin());
    // Outside of the bark: taper, a flare toward the butt (-x), ovality and lumps.
    let outer = move |th: f32, x: f32| -> f32 {
        let t = (x + half) / length;
        let lumps = fbm3(Vec3::new(th.cos() * 0.9, th.sin() * 0.9, x * 1.3), s3, 3);
        radius
            * (1.0 + taper * (0.5 - t))
            * (1.0 + 0.06 * (1.0 - smooth(0.0, 0.12, t)))
            * (1.0 + 0.035 * (2.0 * (th - oval)).cos() + 0.04 * lumps)
    };
    // Where the bark remains (1) or has fallen away (0): patches, mostly gone from the top
    // and toward the ends.
    let cover = move |th: f32, x: f32| -> f32 {
        let n = fbm3(Vec3::new(th.cos() * 1.4, th.sin() * 1.4, x * 1.5), s4, 3);
        let ends = smooth(0.6, 1.0, (x / half).abs());
        smooth(
            -0.05,
            0.05,
            n + 0.3 - 0.36 * th.cos().max(-0.3) - 0.35 * ends,
        )
    };
    // Bark relief: furrows wandering along the log, plates broken across.
    let bark = move |th: f32, x: f32| -> (f32, f32) {
        let (c, s) = (th.cos(), th.sin());
        let warp = fbm3(Vec3::new(c * 0.8, s * 0.8, x * 1.6), s1, 3) * 2.2;
        let ridge = ((th * furrows + warp) * 0.5).sin().abs();
        let plate = smooth(0.1, 0.6, ridge);
        let breaks = fbm3(Vec3::new(c * 3.0, s * 3.0, x * 7.0), s2, 2);
        let depth = bark_t * 0.85 * (1.0 - plate) + bark_t * 0.3 * smooth(0.2, 0.5, breaks);
        (depth, plate)
    };
    // Drying checks in the bare wood: (angle, from x, to x, depth).
    let checks: Vec<(f32, f32, f32, f32)> = (0..3)
        .map(|_| {
            let a = rng.range(0.0, TAU);
            let x0 = rng.range(-half, half * 0.3);
            let len = rng.range(0.3, 0.9) * half;
            (a, x0, (x0 + len).min(half), rng.range(0.004, 0.008))
        })
        .collect();
    let wood_base = move |th: f32, x: f32| outer(th, x) - bark_t * 1.3;
    let wood_r = |th: f32, x: f32| -> f32 {
        let mut r = wood_base(th, x);
        for &(a, x0, x1, d) in &checks {
            let dth = ((th - a + PI).rem_euclid(TAU) - PI).abs();
            let along = smooth(x0, x0 + 0.1, x) * (1.0 - smooth(x1 - 0.1, x1, x));
            r -= d * along * (1.0 - smooth(0.0, 0.035, dth)) * (1.0 - cover(th, x));
        }
        r
    };
    // The bark's outside: just under the bare wood where it has gone, rising over a steep
    // broken edge to its full thickness. (radius, cover, plate, depth)
    let bark_r = move |th: f32, x: f32| -> (f32, f32, f32, f32) {
        let e = cover(th, x);
        let (depth, plate) = bark(th, x);
        let base = wood_base(th, x) - 0.0015;
        let full = outer(th, x) - depth;
        (base + (full - base) * e, e, plate, depth)
    };
    // The sawn end a slightly tilted plane, the broken end jagged.
    let splinter = move |th: f32, rho: f32| -> f32 {
        let (c, s) = (th.cos() as f64, th.sin() as f64);
        let spikes = noise::ridged2(c * 3.0 + rho as f64 * 2.0, s * 3.0, s2 ^ 0x5, 3);
        let tear = (th - 1.3).cos() * 0.5 + 0.5;
        radius * (0.15 + 0.5 * spikes.powi(3) * (0.4 + 0.6 * tear)) * (1.0 - 0.5 * rho)
    };
    let end_x = move |th: f32, sawn: bool| -> f32 {
        if sawn {
            -half + radius * (cut_tilt.x * th.cos() + cut_tilt.y * th.sin())
        } else {
            half - radius * 0.6 + splinter(th, 1.0)
        }
    };
    // Wet and green where it floats, a little moss along the top.
    let moss_seed = rng.next_u64();
    let water = move |p: Vec3, c: Vec3| -> Vec3 {
        let up = (p.y - center(p.x).y) / radius;
        let n = fbm3(p * 5.0, moss_seed ^ 7, 2);
        let wet = 1.0 - smooth(-0.35, 0.05 + 0.1 * n, up);
        let c = c.lerp(c * Vec3::new(0.5, 0.58, 0.42), wet);
        c * (1.0 - 0.25 * smooth(-0.3, -0.8, up))
    };
    let bark_color = move |p: Vec3, depth: f32, plate: f32, e: f32| -> Vec4 {
        let n = fbm3(p * 6.0, moss_seed, 3);
        // Gray-brown weathered plates, dark furrows.
        let mut c =
            Vec3::new(0.86, 0.86, 0.92) * (0.6 + 0.45 * plate) - Vec3::splat(5.0 * depth.min(0.04));
        let lichen = smooth(0.2, 0.35, fbm3(p * 14.0, moss_seed ^ 1, 2)) * plate;
        c = c.lerp(Vec3::new(1.7, 1.8, 1.55), lichen * 0.35);
        let up = (p.y - center(p.x).y) / radius;
        let moss = smooth(0.2, 0.7, up) * smooth(-0.05, 0.25, n);
        c = c.lerp(Vec3::new(0.9, 1.5, 0.5), moss * 0.75);
        // The broken edge shows the reddish inner bark.
        c = c.lerp(
            Vec3::new(1.7, 1.1, 0.75),
            (1.0 - smooth(0.25, 0.75, e)) * 0.8,
        );
        let mut out = rgba(water(p, c));
        out.w = e;
        out
    };
    let wood_seed = rng.next_u64();
    let wood_color = move |p: Vec3, e: f32| -> Vec4 {
        let n = fbm3(Vec3::new(p.x * 0.8, p.y * 6.0, p.z * 6.0), wood_seed, 3);
        // Silvered on top, browner where the bark came off recently (near its edges).
        let mut c = Vec3::new(1.12, 1.14, 1.18) * (0.92 + 0.12 * n);
        c = c.lerp(Vec3::new(0.95, 0.82, 0.7), smooth(0.0, 0.5, e) * 0.7);
        rgba(water(p, c))
    };

    let mut ths: Vec<f32> = steps(0.0, 1.0, 38);
    for &(a, ..) in &checks {
        for f in [-1.0f32, 0.0, 1.0] {
            let t = (a + f * 0.035).rem_euclid(TAU) / TAU;
            if t > 0.002 && t < 0.998 {
                ths.push(t);
            }
        }
    }
    ths.sort_by(f32::total_cmp);
    ths.dedup_by(|a, b| (*a - *b).abs() < 0.004);
    let us_wood = wrap_scale(Surface::Post, TAU * radius);
    let us_bark = wrap_scale(Surface::Bark, TAU * radius);
    let mut along = steps(0.0, 1.0, 22);
    along.extend_from_slice(&[0.006, 0.994]);
    along.sort_by(f32::total_cmp);
    let x_at = move |th: f32, t: f32| {
        let (x0, x1) = (end_x(th, true), end_x(th, false));
        x0 + (x1 - x0) * t
    };

    // (Angle, then along x, about x: the surface faces outward with `flip`.)
    let mut wood = Mesh::default();
    surface(&mut wood, &ths, &along, (true, false), true, |a, t| {
        let th = a * TAU;
        let x = x_at(th, t);
        let p = center(x) + radial(th) * wood_r(th, x);
        Sample::new(
            p,
            Vec2::new(a * TAU * radius * us_wood, x),
            wood_color(p, cover(th, x)),
        )
    });
    let mut barkm = Mesh::default();
    surface(&mut barkm, &ths, &along, (true, false), true, |a, t| {
        let th = a * TAU;
        let x = x_at(th, t);
        let (r, e, plate, depth) = bark_r(th, x);
        let p = center(x) + radial(th) * r;
        Sample::new(
            p,
            Vec2::new(a * TAU * radius * us_bark, x),
            bark_color(p, depth, plate, e),
        )
    });
    // Bark rims at both ends, from the bark's outside in to the wood.
    for (sawn, flip) in [(true, false), (false, true)] {
        surface(
            &mut barkm,
            &ths,
            &[0.0, 1.0],
            (true, false),
            flip,
            |a, f| {
                let th = a * TAU;
                let x = end_x(th, sawn);
                let (rb, e, ..) = bark_r(th, x);
                let r = rb + (wood_r(th, x) - rb) * f;
                let p = center(x) + radial(th) * r;
                let mut c = rgba(water(p, Vec3::new(1.5, 1.0, 0.72)));
                c.w = e;
                Sample::new(p, Vec2::new(a * TAU * radius * us_bark, r), c)
            },
        );
    }
    let mut grain = Mesh::default();
    // Sawn end: end grain, grayed by the weather toward the rim.
    let c0 = center(-half);
    surface(
        &mut grain,
        &ths,
        &steps(0.0, 1.0, 4),
        (true, false),
        false,
        |a, f| {
            let th = a * TAU;
            let x = end_x(th, true);
            let r = wood_r(th, x) * (1.0 - f);
            let p = center(x) + radial(th) * r;
            let q = p - c0;
            let c = Vec3::new(0.95, 0.92, 0.9).lerp(Vec3::new(1.2, 1.25, 1.35), 1.0 - f) * 0.85;
            Sample::new(p, Vec2::new(q.z, q.y), rgba(water(p, c)))
        },
    );
    // Broken end: splinters standing out of the break, lighter fresh wood, faceted.
    let c1 = center(half);
    let mut broken = Mesh::default();
    surface(
        &mut broken,
        &ths,
        &steps(0.0, 1.0, 6),
        (true, false),
        true,
        |a, f| {
            let th = a * TAU;
            let rho = 1.0 - f;
            let x = half - radius * 0.6 + splinter(th, rho);
            let r = wood_r(th, x) * rho;
            let p = center(x) + radial(th) * r;
            let q = p - c1;
            let fresh = 0.9 + 0.6 * (splinter(th, rho) / radius).min(0.6);
            Sample::new(p, Vec2::new(q.z, q.y), rgba(water(p, Vec3::splat(fresh))))
        },
    );
    flat_shade(&mut broken);
    grain.merge(&broken);

    // Branch stubs: a sleeve of bark flaring into the trunk, then gray wood snapped off in a
    // splintered point.
    let stubs = rng.range_i32(2, 4);
    for _ in 0..stubs {
        let x = rng.range(-0.3, 0.35) * length;
        let th = rng.range(0.0, TAU);
        let out = radial(th);
        let dir = (out + Vec3::X * rng.range(0.3, 0.9)).normalize();
        let br = radius * rng.range(0.2, 0.3);
        let r_here = outer(th, x);
        let root = center(x) + out * r_here * 0.5;
        let reach = r_here * 0.5 / dir.dot(out).max(0.3);
        let sleeve = reach + br * 0.8;
        let len = rng.range(0.025, 0.07);
        let fx = dir.any_orthonormal_vector();
        let fy = dir.cross(fx);
        let tip_seed = rng.next_u64();
        let ring = steps(0.0, 1.0, 12);
        surface(
            &mut barkm,
            &ring,
            &steps(0.0, 1.0, 5),
            (true, false),
            false,
            |a, f| {
                let t2 = a * TAU;
                let s = f * sleeve;
                let flare = (-(s - reach).max(0.0) / (br * 0.7)).exp();
                let r = br * (1.0 + 0.4 * flare) * (1.0 - 0.1 * smooth(0.7, 1.0, f)) + 0.002;
                let p = root + dir * s + (fx * t2.cos() + fy * t2.sin()) * r;
                let c = bark_color(p, 0.0, 0.7, 1.0);
                Sample::new(p, Vec2::new(a * TAU * br, s), c)
            },
        );
        let s0 = sleeve - 0.015;
        surface(
            &mut wood,
            &ring,
            &steps(0.0, 1.0, 5),
            (true, false),
            false,
            |a, f| {
                let t2 = a * TAU;
                let jag = noise::fbm2(
                    t2.cos() as f64 * 2.0,
                    t2.sin() as f64 * 2.0,
                    tip_seed,
                    2,
                    2.0,
                    0.5,
                );
                let reach_tip = len * (0.75 + 0.6 * jag);
                let s = s0 + f * (reach_tip + 0.015);
                // Snapped off blunt, the splinters ragged around the break.
                let r = br * 0.88 * (1.0 - f.powi(3)).max(0.0).sqrt();
                let p = root + dir * s + (fx * t2.cos() + fy * t2.sin()) * r;
                Sample::new(p, Vec2::new(a * TAU * br, s), wood_color(p, 0.3))
            },
        );
    }
    keep_by_alpha(&mut barkm, 0.01);
    vec![
        Part::new(barkm, Surface::Bark),
        Part::new(wood, Surface::Post),
        Part::new(grain, Surface::EndGrain),
    ]
}

// ---------------------------------------------------------------------------------------------
// Culvert
// ---------------------------------------------------------------------------------------------

/// Corrugation pitch and half depth of the culvert pipe (2⅔" × ½" profile), meters.
const CORR_PITCH: f32 = 0.068;
const CORR_AMP: f32 = 0.0065;

/// A corrugated galvanized steel culvert in a cast concrete headwall. The pipe's axis runs
/// along x at y = 0: the outlet at x = 0 opens toward +x and the pipe runs back into the bank
/// to x = -2.0; inner radius 0.38 (invert at y = -0.38). Real corrugations inside and out,
/// darkening into the pipe. The headwall stands from x = -0.35 to -0.05, 3.2 m wide (z) and
/// 1.0 m above the invert (top at y = 0.62, footing down to y = -0.63), chamfered, with two
/// wingwalls flaring 35° downstream (reaching x ≈ 1.0, z = ±2.34). About 25 000 triangles.
pub fn culvert() -> Vec<Part> {
    let mut steel = Mesh::default();
    let r_in = |x: f32| 0.38 + CORR_AMP + CORR_AMP * (x / CORR_PITCH * TAU).sin();
    let wall = 0.003;
    let around = 44;
    let us = wrap_scale(Surface::Galvanized, TAU * 0.39);
    let station = |n: usize, x0: f32, x1: f32| -> Vec<f32> { steps(x0, x1, n) };
    let mut xs = station(74, -2.0, -1.0);
    xs.extend(station(118, -1.0, 0.0).into_iter().skip(1));
    let inside_color = |p: Vec3| -> Vec4 {
        // Little sky reaches into the pipe: dimmer from just inside the mouth, nearly
        // black at the back.
        let depth = smooth(0.0, 1.3, -p.x);
        let invert = smooth(-0.2, -0.36, p.y);
        let mut c = Vec3::splat((1.0 - 0.94 * depth) * (1.0 - 0.3 * smooth(0.0, 0.15, -p.x)));
        // Rust and silt along the invert, a darker wet line.
        c = c.lerp(c * Vec3::new(0.62, 0.42, 0.25), invert * 0.85);
        rgba(c)
    };
    // Inside, seen from the outlet (normals toward the axis).
    surface(
        &mut steel,
        &steps(0.0, 1.0, around),
        &xs,
        (true, false),
        false,
        |a, x| {
            let th = a * TAU;
            let r = r_in(x);
            let p = Vec3::new(x, r * th.sin(), r * th.cos());
            Sample::new(p, Vec2::new(a * TAU * 0.39 * us, x), inside_color(p))
        },
    );
    // Outside: corrugated where it shows in front of the headwall, plain where buried.
    let xo = station(52, -0.4, 0.0);
    surface(
        &mut steel,
        &steps(0.0, 1.0, around),
        &xo,
        (true, false),
        true,
        |a, x| {
            let th = a * TAU;
            let r = r_in(x) + wall;
            let p = Vec3::new(x, r * th.sin(), r * th.cos());
            Sample::new(p, Vec2::new(a * TAU * 0.39 * us, x), rgba(Vec3::splat(0.9)))
        },
    );
    surface(
        &mut steel,
        &steps(0.0, 1.0, around),
        &[-2.0, -0.4],
        (true, false),
        true,
        |a, x| {
            let th = a * TAU;
            let r = r_in(-0.4) + wall;
            let p = Vec3::new(x, r * th.sin(), r * th.cos());
            Sample::new(p, Vec2::new(a * TAU * 0.39 * us, x), rgba(Vec3::splat(0.5)))
        },
    );
    // The cut ends: a thin bright rim at the outlet, a closed back.
    for (x, flip) in [(0.0f32, false), (-2.0, true)] {
        surface(
            &mut steel,
            &steps(0.0, 1.0, around),
            &[0.0, 1.0],
            (true, false),
            flip,
            |a, f| {
                let th = a * TAU;
                let r0 = r_in(x);
                let r1 = if x < -1.0 {
                    r_in(-0.4) + wall
                } else {
                    r0 + wall
                };
                let r = r0 + (r1 - r0) * f;
                Sample::new(
                    Vec3::new(x, r * th.sin(), r * th.cos()),
                    Vec2::new(a, f),
                    Vec4::ONE,
                )
            },
        );
    }

    // Silt along the invert, thinning out toward the mouth.
    let mut silt = Mesh::default();
    let silt_seed = 0x5117;
    let silt_y = move |x: f32, z: f32| -> f32 {
        let ripple = noise::fbm2(x as f64 * 3.0, z as f64 * 6.0, silt_seed, 3, 2.0, 0.5);
        -0.31 - 0.09 * smooth(-0.7, -0.08, x) + 0.006 * ripple
    };
    surface(
        &mut silt,
        &steps(-1.0, 1.0, 10),
        &steps(-2.0, -0.02, 30),
        (false, false),
        false,
        |a, x| {
            let y = silt_y(x, 0.0);
            let w = (0.383f32 * 0.383 - y * y).max(1e-4).sqrt();
            let z = a * w;
            let y = silt_y(x, z) - 0.01 * a * a;
            let n = noise::fbm2(x as f64 * 7.0, z as f64 * 7.0, silt_seed + 1, 2, 2.0, 0.5);
            let c =
                Vec3::new(0.3, 0.25, 0.17) * (0.85 + 0.3 * n) * (1.0 - 0.8 * smooth(0.1, 1.5, -x));
            Sample::new(Vec3::new(x, y, z), Vec2::new(x, z), rgba(c))
        },
    );

    // Headwall: a chamfered block with a bore for the pipe.
    let mut concrete = Mesh::default();
    let (x_back, x_front) = (-0.35f32, -0.05f32);
    let (y_bot, y_top, hz) = (-0.63f32, 0.62f32, 1.6f32);
    let ch = 0.03;
    let bore = 0.4;
    let stain_seed = 0xc0;
    let wall_color = move |p: Vec3| -> Vec4 {
        let n = noise::fbm2(p.z as f64 * 1.5, p.y as f64 * 0.4, stain_seed, 3, 2.0, 0.5);
        let streak = noise::fbm2(
            p.z as f64 * 9.0,
            p.y as f64 * 0.6,
            stain_seed + 1,
            2,
            2.0,
            0.5,
        );
        let mut c = Vec3::splat(0.95 + 0.1 * n);
        // Rain streaks running down from the top, darker toward the ground.
        c *= 1.0 - 0.25 * smooth(0.1, 0.5, streak) * smooth(-0.2, 0.55, p.y);
        // Wet, algae-stained base where the water runs.
        let wet = 1.0 - smooth(-0.42, -0.2, p.y);
        c = c.lerp(Vec3::new(0.5, 0.56, 0.42), wet * 0.8);
        // Efflorescence below the pipe.
        let below = (1.0 - smooth(0.2, 0.55, p.z.abs())) * smooth(-0.6, -0.42, p.y) * (1.0 - wet);
        c = c.lerp(Vec3::splat(1.2), below * 0.3);
        // Moss along the top edge.
        let moss = smooth(0.5, 0.6, p.y) * smooth(0.0, 0.3, n);
        c = c.lerp(Vec3::new(0.55, 0.7, 0.35), moss * 0.7);
        rgba(c)
    };
    let mut block = Mesh::default();
    chamfer_box(
        &mut block,
        Vec3::new((x_front - x_back) * 0.5, (y_top - y_bot) * 0.5, hz),
        ch,
        at_(Vec3::new(
            (x_front + x_back) * 0.5,
            (y_top + y_bot) * 0.5,
            0.0,
        )),
        Vec4::ONE,
    );
    // Drop the flat front and back faces: they get the bore cut through them.
    let keep: Vec<u32> = block
        .indices
        .chunks(3)
        .filter(|t| {
            let n = Vec3::from(block.vertices[t[0] as usize].normal);
            n.x.abs() < 0.99
        })
        .flatten()
        .copied()
        .collect();
    block.indices = keep;
    concrete.merge(&block);
    // Front and back faces: rings from the bore out to the chamfer's inner rectangle.
    let rect = [
        Vec2::new(-(hz - ch), y_bot + ch),
        Vec2::new(hz - ch, y_bot + ch),
        Vec2::new(hz - ch, y_top - ch),
        Vec2::new(-(hz - ch), y_top - ch),
    ];
    let mut angles: Vec<f32> = steps(0.0, 1.0, 64);
    for c in rect {
        angles.push(c.y.atan2(c.x).rem_euclid(TAU) / TAU);
    }
    angles.sort_by(f32::total_cmp);
    angles.dedup_by(|a, b| (*a - *b).abs() < 1e-4);
    for (x, flip) in [(x_front, false), (x_back, true)] {
        surface(
            &mut concrete,
            &angles,
            &steps(0.0, 1.0, 6),
            (true, false),
            flip,
            |a, f| {
                let th = a * TAU;
                let d = Vec2::new(th.cos(), th.sin());
                let outer = ray_polygon(Vec2::ZERO, d, &rect);
                let q = (d * bore).lerp(outer, f);
                let p = Vec3::new(x, q.y, q.x);
                Sample::new(p, Vec2::new(p.z, -p.y), wall_color(p))
            },
        );
    }
    // The bore through the wall.
    surface(
        &mut concrete,
        &steps(0.0, 1.0, 48),
        &[x_back, x_front],
        (true, false),
        false,
        |a, x| {
            let th = a * TAU;
            let p = Vec3::new(x, bore * th.sin(), bore * th.cos());
            Sample::new(p, Vec2::new(a * TAU * bore, x), rgba(Vec3::splat(0.6)))
        },
    );
    for v in &mut concrete.vertices {
        let p = Vec3::from(v.position);
        let n = Vec3::from(v.normal);
        if v.color == [1.0; 4] {
            v.color = wall_color(p).to_array();
            // Concrete: form boards along u, drips down along v.
            v.uv = if n.y.abs() > 0.7 {
                [p.z, p.x]
            } else if n.x.abs() > 0.7 {
                [p.z, -p.y]
            } else {
                [p.x, -p.y]
            };
        }
    }
    // Wingwalls flaring downstream, their tops sloping down to the ground.
    for side in [1.0f32, -1.0] {
        let ang = 35f32.to_radians();
        let dir = Vec3::new(ang.cos(), 0.0, side * ang.sin());
        let start = Vec3::new(x_front - 0.12, 0.0, side * (hz - 0.13));
        let len = 1.35;
        let thick = 0.24;
        let normal = Vec3::Y.cross(dir) * -side;
        let top = |s: f32| y_top - 0.05 - (y_top - 0.05 - (-0.1)) * (s / len);
        let section = |s: f32, k: usize| -> Vec3 {
            // Counterclockwise seen from the far end: bottom out, up, chamfers, top, down.
            let t = top(s);
            let w = thick * 0.5;
            let c = 0.025;
            let q = [
                Vec2::new(-w, y_bot),
                Vec2::new(w, y_bot),
                Vec2::new(w, t - c),
                Vec2::new(w - c, t),
                Vec2::new(-w + c, t),
                Vec2::new(-w, t - c),
            ][k % 6];
            start + dir * s + normal * q.x + Vec3::Y * (q.y - start.y)
        };
        let mut wing = Mesh::default();
        for k in 0..6 {
            surface(
                &mut wing,
                &[0.0, 1.0],
                &[0.0, len],
                (false, false),
                false,
                |a, s| {
                    let p = section(s, k).lerp(section(s, k + 1), a);
                    Sample::new(p, Vec2::new(s, -p.y), Vec4::ONE)
                },
            );
        }
        for (s, n) in [(0.0f32, -dir), (len, dir)] {
            let ring: Vec<Vec3> = (0..6).map(|k| section(s, k)).collect();
            flat_polygon(&mut wing, &ring, n, Vec4::ONE);
        }
        orient_outward(&mut wing);
        for v in &mut wing.vertices {
            let p = Vec3::from(v.position);
            v.color = wall_color(p).to_array();
            let n = Vec3::from(v.normal);
            let along = (p - start).dot(dir);
            let across = (p - start).dot(normal);
            v.uv = if n.y.abs() > 0.5 {
                // The sloping coping: boards across, drips along.
                [across, along]
            } else if n.dot(normal).abs() > 0.7 {
                [along, -p.y]
            } else {
                [across, -p.y]
            };
        }
        concrete.merge(&wing);
    }
    // A mortar collar where the pipe leaves the wall.
    let collar = arc(
        Vec3::new(x_front + 0.004, 0.0, 0.0),
        Vec3::Z,
        Vec3::Y,
        0.403,
        0.0,
        TAU,
        48,
    );
    tube(
        &mut concrete,
        &collar[..48],
        6,
        true,
        &|_| 0.013,
        rgba(Vec3::splat(0.85)),
    );
    vec![
        Part::new(steel, Surface::Galvanized),
        Part::new(concrete, Surface::Concrete),
        Part::new(silt, Surface::Ground),
    ]
}

// ---------------------------------------------------------------------------------------------
// People
// ---------------------------------------------------------------------------------------------

/// Cubic Hermite interpolation through `rows` of (t, values) with Catmull-Rom tangents,
/// clamped beyond the first and last rows.
fn table<const N: usize>(rows: &[(f32, [f32; N])], t: f32) -> [f32; N] {
    let n = rows.len();
    if n == 1 || t <= rows[0].0 {
        return rows[0].1;
    }
    if t >= rows[n - 1].0 {
        return rows[n - 1].1;
    }
    let i = rows.iter().rposition(|r| r.0 <= t).unwrap_or(0).min(n - 2);
    let (t0, t1) = (rows[i].0, rows[i + 1].0);
    let h = t1 - t0;
    let f = (t - t0) / h;
    let (f2, f3) = (f * f, f * f * f);
    let mut out = [0.0; N];
    for (k, o) in out.iter_mut().enumerate() {
        let slope = |a: usize, b: usize| (rows[b].1[k] - rows[a].1[k]) / (rows[b].0 - rows[a].0);
        let m1 = if i > 0 {
            slope(i - 1, i + 1)
        } else {
            slope(i, i + 1)
        } * h;
        let m2 = if i + 2 < n {
            slope(i, i + 2)
        } else {
            slope(i, i + 1)
        } * h;
        *o = (2.0 * f3 - 3.0 * f2 + 1.0) * rows[i].1[k]
            + (f3 - 2.0 * f2 + f) * m1
            + (-2.0 * f3 + 3.0 * f2) * rows[i + 1].1[k]
            + (f3 - f2) * m2;
    }
    out
}

/// Distance from the center to an off-center superellipse at angle `phi` (0 = front,
/// π/2 = the + side): radii `r` = [front, back, + side, − side].
fn quad_radius(phi: f32, r: [f32; 4], power: f32) -> f32 {
    let (c, s) = (phi.cos(), phi.sin());
    let rx = if c >= 0.0 { r[0] } else { r[1] }.max(1e-5);
    let ry = if s >= 0.0 { r[2] } else { r[3] }.max(1e-5);
    ((c.abs() / rx).powf(power) + (s.abs() / ry).powf(power)).powf(-1.0 / power)
}

/// 1 in the middle of `start..end`, rounding off to 0 like a quarter circle over the first
/// `len0` and the last `len1` (closes a loft with domed ends).
fn dome(t: f32, start: f32, end: f32, len0: f32, len1: f32) -> f32 {
    let q = |x: f32| {
        let x = x.clamp(0.0, 1.0);
        (1.0 - (1.0 - x) * (1.0 - x)).sqrt()
    };
    q((t - start) / len0) * q((end - t) / len1)
}

/// exp(−(x/s)²).
fn bell(x: f32, s: f32) -> f32 {
    (-(x / s) * (x / s)).exp()
}

/// A limb's centerline through its joints, the bends rounded over `round` meters, and per bone
/// the direction its cross-section's front faces.
struct Chain {
    joints: Vec<Vec3>,
    fronts: Vec<Vec3>,
    round: f32,
}

impl Chain {
    fn bones(&self) -> usize {
        self.joints.len() - 1
    }

    fn seg(&self, i: usize) -> (Vec3, f32) {
        let d = self.joints[i + 1] - self.joints[i];
        (d.normalize_or(Vec3::NEG_Y), d.length())
    }

    /// The rounded stretch around inner joint `k`, in bone units.
    fn window(&self, k: usize) -> (f32, f32) {
        let e0 = (self.round / self.seg(k - 1).1.max(1e-4)).min(0.45);
        let e1 = (self.round / self.seg(k).1.max(1e-4)).min(0.45);
        (k as f32 - e0, k as f32 + e1)
    }

    /// The centerline at `t` in bone units (joint k at t = k; the first and last bones run on
    /// straight beyond the ends).
    fn center(&self, t: f32) -> Vec3 {
        let n = self.bones();
        let i = (t.floor().max(0.0) as usize).min(n - 1);
        let mut p = self.joints[i].lerp(self.joints[i + 1], t - i as f32);
        for k in 1..n {
            let (a, b) = self.window(k);
            if t > a && t < b {
                let w = (t - a) / (b - a);
                let p0 = self.joints[k].lerp(self.joints[k - 1], k as f32 - a);
                let p1 = self.joints[k].lerp(self.joints[k + 1], b - k as f32);
                p = p0 * ((1.0 - w) * (1.0 - w))
                    + self.joints[k] * (2.0 * w * (1.0 - w))
                    + p1 * (w * w);
            }
        }
        p
    }

    /// Center, unit tangent and unit front at `t`.
    fn frame(&self, t: f32) -> (Vec3, Vec3, Vec3) {
        let e = 1e-3;
        let tan = (self.center(t + e) - self.center(t - e)).normalize_or(self.seg(0).0);
        let n = self.bones();
        let mut front = self.fronts[(t.floor().max(0.0) as usize).min(n - 1)];
        for k in 1..n {
            let (a, b) = self.window(k);
            if t > a && t < b {
                front = self.fronts[k - 1].lerp(self.fronts[k], smooth(a, b, t));
            }
        }
        let front = (front - tan * front.dot(tan)).normalize_or(tan.any_orthonormal_vector());
        (self.center(t), tan, front)
    }

    /// Distance along the bones to `t` (for UVs).
    fn length_to(&self, t: f32) -> f32 {
        let i = (t.floor().max(0.0) as usize).min(self.bones() - 1);
        (0..i).map(|k| self.seg(k).1).sum::<f32>() + (t - i as f32) * self.seg(i).1
    }
}

/// Loft a body segment along `chain` over the stations `ts` with `around` sides:
/// `radius(t, phi)` from the centerline, `phi` from the front toward tangent × front.
fn loft(
    m: &mut Mesh,
    chain: &Chain,
    ts: &[f32],
    around: usize,
    radius: &dyn Fn(f32, f32) -> f32,
    color: &dyn Fn(Vec3, f32, f32) -> Vec4,
) {
    let uv_r = radius(ts[ts.len() / 2], 0.0).max(0.01);
    surface(
        m,
        &steps(0.0, 1.0, around),
        ts,
        (true, false),
        false,
        |a, t| {
            let (c, tan, front) = chain.frame(t);
            let phi = a * TAU;
            let p = c + (front * phi.cos() + tan.cross(front) * phi.sin()) * radius(t, phi);
            Sample::new(
                p,
                Vec2::new(a * TAU * uv_r, chain.length_to(t)),
                color(p, t, phi),
            )
        },
    );
}

/// Joint positions of the standing body frame: x forward, y up, z to the person's right,
/// soles at y = 0. Shoulder and hip joints are for the right side (z mirrors).
const SHOULDER: Vec3 = Vec3::new(-0.02, 1.43, 0.178);
const HIP: Vec3 = Vec3::new(0.0, 0.905, 0.088);
const UPPER_ARM: f32 = 0.3;
const FOREARM: f32 = 0.255;
const THIGH: f32 = 0.425;
const SHIN: f32 = 0.41;
/// The head's own frame sits here (eye height, between the ears) and turns about the neck.
const HEAD_AT: Vec3 = Vec3::new(0.008, 1.638, 0.0);
const NECK_PIVOT: Vec3 = Vec3::new(-0.01, 1.53, 0.0);

/// Torso rows: (y, [front, back, half width, center x]).
const TORSO: [(f32, [f32; 4]); 15] = [
    (0.8, [0.05, 0.065, 0.1, -0.01]),
    (0.83, [0.062, 0.085, 0.13, -0.01]),
    (0.87, [0.075, 0.104, 0.158, -0.01]),
    (0.93, [0.084, 0.106, 0.168, -0.005]),
    (1.0, [0.088, 0.088, 0.152, 0.0]),
    (1.06, [0.088, 0.077, 0.142, 0.004]),
    (1.13, [0.092, 0.082, 0.147, 0.004]),
    (1.21, [0.1, 0.09, 0.157, 0.0]),
    (1.29, [0.108, 0.096, 0.163, -0.004]),
    (1.36, [0.102, 0.098, 0.165, -0.008]),
    (1.41, [0.085, 0.09, 0.158, -0.012]),
    (1.455, [0.06, 0.072, 0.105, -0.016]),
    (1.49, [0.05, 0.056, 0.064, -0.014]),
    (1.54, [0.048, 0.05, 0.056, -0.008]),
    (1.645, [0.044, 0.048, 0.052, 0.002]),
];

/// The torso's radius at height `y` and angle `th` (0 = front, π/2 = left), with the
/// muscles' relief scaled by `relief` (1 for skin, less for cloth), and its center x.
fn torso_radius(y: f32, th: f32, relief: f32) -> (f32, f32) {
    let [f, b, w, cx] = table(&TORSO, y);
    let r = quad_radius(th, [f, b, w, w], 2.4);
    // Angle from the front, 0..π.
    let a = ((th + PI).rem_euclid(TAU) - PI).abs();
    let lower = |c: f32, below: f32, above: f32| {
        if y < c {
            bell(y - c, below)
        } else {
            bell(y - c, above)
        }
    };
    let bump = 0.013 * bell(a - 0.62, 0.4) * lower(1.3, 0.025, 0.06)
        + 0.009 * bell(a - 2.45, 0.3) * bell(y - 1.33, 0.06)
        + 0.006 * bell(a - 1.75, 0.3) * bell(y - 1.24, 0.07)
        + 0.016 * bell(a - 2.6, 0.35) * bell(y - 0.89, 0.045)
        - 0.02 * bell(a - PI, 0.09) * (1.0 - smooth(0.9, 0.98, y))
        - 0.007 * bell(a - PI, 0.1) * smooth(0.95, 1.05, y) * (1.0 - smooth(1.42, 1.48, y))
        - 0.004 * bell(a, 0.09) * smooth(1.0, 1.05, y) * (1.0 - smooth(1.36, 1.4, y))
        + 0.004 * bell(a - 0.2, 0.15) * bell(y - 1.11, 0.07)
        + 0.005 * bell(a - 0.9, 0.35) * bell(y - 1.445, 0.012);
    (r + bump * relief, cx)
}

/// Arm rows over the chain (shoulder joint t = 0, elbow 1, wrist 2):
/// [front, back, lateral, medial].
const ARM: [(f32, [f32; 4]); 11] = [
    (0.0, [0.047, 0.05, 0.052, 0.035]),
    (0.2, [0.046, 0.049, 0.05, 0.037]),
    (0.42, [0.044, 0.046, 0.042, 0.038]),
    (0.65, [0.042, 0.042, 0.037, 0.037]),
    (0.88, [0.035, 0.037, 0.036, 0.038]),
    (1.0, [0.034, 0.036, 0.036, 0.038]),
    (1.15, [0.039, 0.036, 0.043, 0.039]),
    (1.35, [0.037, 0.034, 0.041, 0.038]),
    (1.65, [0.029, 0.026, 0.033, 0.031]),
    (1.9, [0.021, 0.019, 0.029, 0.027]),
    (2.0, [0.02, 0.018, 0.028, 0.026]),
];

/// Leg rows over the chain (hip joint t = 0, knee 1, ankle 2).
const LEG: [(f32, [f32; 4]); 13] = [
    (0.0, [0.075, 0.085, 0.086, 0.062]),
    (0.15, [0.079, 0.08, 0.08, 0.07]),
    (0.35, [0.075, 0.072, 0.07, 0.066]),
    (0.6, [0.066, 0.062, 0.06, 0.06]),
    (0.8, [0.058, 0.053, 0.05, 0.058]),
    (0.95, [0.053, 0.05, 0.048, 0.053]),
    (1.0, [0.051, 0.048, 0.046, 0.05]),
    (1.08, [0.045, 0.05, 0.045, 0.048]),
    (1.25, [0.04, 0.06, 0.046, 0.05]),
    (1.45, [0.037, 0.054, 0.042, 0.045]),
    (1.7, [0.03, 0.04, 0.033, 0.034]),
    (1.9, [0.027, 0.03, 0.029, 0.03]),
    (2.0, [0.028, 0.032, 0.032, 0.033]),
];

/// A limb's radius from its table at `t` and `phi` for the limb on `side` (+1 right).
fn limb_radius(rows: &[(f32, [f32; 4])], t: f32, phi: f32, side: f32) -> f32 {
    let [f, b, lat, med] = table(rows, t);
    let (pos, neg) = if side > 0.0 { (lat, med) } else { (med, lat) };
    quad_radius(phi, [f, b, pos, neg], 2.0)
}

/// Which way a limb's two bones face: the first bone's front turns toward the bend (forward
/// for an elbow, backward for a knee: `knee`), the second follows; `hint` for a straight
/// limb.
fn bend_fronts(d1: Vec3, d2: Vec3, hint: Vec3, knee: bool) -> (Vec3, Vec3) {
    let perp = |v: Vec3, d: Vec3| v - d * v.dot(d);
    let w = smooth(0.03, 0.25, d1.cross(d2).length());
    let s = if knee { -1.0 } else { 1.0 };
    let h1 = perp(hint, d1).normalize_or(d1.any_orthonormal_vector());
    let b1 = (perp(d2, d1) * s).normalize_or(h1);
    let a1 = h1.lerp(b1, w).normalize_or(b1);
    let h2 = perp(a1, d2).normalize_or(h1);
    let b2 = (perp(-d1, d2) * s).normalize_or(h2);
    (a1, h2.lerp(b2, w).normalize_or(b2))
}

/// How a person stands: directions in the body frame, per side [left, right].
struct Pose {
    /// Upper arm and forearm.
    arms: [[Vec3; 2]; 2],
    /// Hand: toward the fingertips and out of the palm; finger curl and thumb spread.
    hands: [(Vec3, Vec3, f32, f32); 2],
    /// Thigh and shin.
    legs: [[Vec3; 2]; 2],
    /// Foot: toward the toes, up out of the instep, and the toes' bend up at the ball.
    feet: [(Vec3, Vec3, f32); 2],
    /// The head's turn and nod about the neck.
    head: Quat,
}

const SIDES: [f32; 2] = [-1.0, 1.0];

impl Pose {
    fn arm(&self, k: usize) -> Chain {
        let s = SHOULDER * Vec3::new(1.0, 1.0, SIDES[k]);
        let [d1, d2] = self.arms[k].map(Vec3::normalize);
        let e = s + d1 * UPPER_ARM;
        let w = e + d2 * FOREARM;
        let (f1, f2) = bend_fronts(d1, d2, Vec3::X, false);
        Chain {
            joints: vec![s, e, w],
            fronts: vec![f1, f2],
            round: 0.045,
        }
    }

    fn leg(&self, k: usize) -> Chain {
        let h = HIP * Vec3::new(1.0, 1.0, SIDES[k]);
        let [d1, d2] = self.legs[k].map(Vec3::normalize);
        let kn = h + d1 * THIGH;
        let a = kn + d2 * SHIN;
        let (f1, f2) = bend_fronts(d1, d2, Vec3::X, true);
        Chain {
            joints: vec![h, kn, a],
            fronts: vec![f1, f2],
            round: 0.06,
        }
    }

    /// Body frame from the head's own frame.
    fn head_xf(&self) -> Mat4 {
        Mat4::from_translation(NECK_PIVOT)
            * Mat4::from_quat(self.head)
            * Mat4::from_translation(HEAD_AT - NECK_PIVOT)
    }
}

/// Skin tint: a little blotchy, redder over knuckles, knees and elbows.
fn skin_tint(p: Vec3, tone: Vec3, flush: f32) -> Vec4 {
    let n = fbm3(p * 9.0, 0x5c1, 2);
    rgba(tone * (0.96 + 0.08 * n)).lerp(rgba(tone * Vec3::new(1.07, 0.93, 0.9)), flush)
}

/// Skin of the arm on `side` from station `t0` (inside a sleeve, or the shoulder cap at
/// -0.18) to the wrist.
fn arm_skin(m: &mut Mesh, pose: &Pose, k: usize, t0: f32, tone: Vec3) {
    let chain = pose.arm(k);
    let side = SIDES[k];
    let ts = steps(t0, 2.07, ((2.07 - t0) * 12.0).round() as usize);
    let cap0 = if t0 < 0.0 { -t0 } else { 0.06 };
    loft(
        m,
        &chain,
        &ts,
        16,
        &|t, phi| limb_radius(&ARM, t, phi, side) * dome(t, t0, 2.07, cap0, 0.07),
        &|p, t, _| skin_tint(p, tone, 0.6 * bell(t - 1.0, 0.07)),
    );
}

/// Skin of the leg on `side` from station `t0` to the ankle.
fn leg_skin(m: &mut Mesh, pose: &Pose, k: usize, t0: f32, tone: Vec3) {
    let chain = pose.leg(k);
    let side = SIDES[k];
    let ts = steps(t0, 2.05, ((2.05 - t0) * 12.5).round() as usize);
    let cap0 = if t0 < 0.0 { -t0 } else { 0.06 };
    loft(
        m,
        &chain,
        &ts,
        18,
        &|t, phi| {
            // The kneecap and the ankle bones.
            let knee = 0.007 * bell(t - 1.0, 0.05) * phi.cos().max(0.0).powi(4);
            let ankle = 0.005 * bell(t - 1.97, 0.05) * phi.sin().abs().powi(6);
            (limb_radius(&LEG, t, phi, side) + knee + ankle) * dome(t, t0, 2.05, cap0, 0.05)
        },
        &|p, t, _| skin_tint(p, tone, 0.5 * bell(t - 1.0, 0.06)),
    );
}

/// A hand at `wrist`: one piece of palm and fingers (grooved between the fingers, the tips
/// scalloped, nails lighter) curled toward the palm by `curl` radians, and a thumb opened by
/// `spread` (0 = along the index finger). `dir` points to the fingertips, `palm` out of the
/// palm; `side` is +1 for the right hand.
#[allow(clippy::too_many_arguments)]
fn hand(
    m: &mut Mesh,
    wrist: Vec3,
    dir: Vec3,
    palm: Vec3,
    side: f32,
    curl: f32,
    spread: f32,
    tone: Vec3,
) {
    let f0 = dir.normalize();
    let n0 = (palm - f0 * palm.dot(f0)).normalize();
    let s0 = f0.cross(n0);
    // The centerline curls toward the palm past the knuckles.
    let (s_min, s_max, steps_n) = (-0.03f32, 0.2f32, 46usize);
    let ds = (s_max - s_min) / steps_n as f32;
    let mut track = Vec::with_capacity(steps_n + 1);
    let mut p = wrist + f0 * s_min;
    for i in 0..=steps_n {
        let s = s_min + ds * i as f32;
        let th = curl * ((s - 0.086) / 0.075).clamp(0.0, 1.4);
        let fd = f0 * th.cos() + n0 * th.sin();
        track.push((p, fd, n0 * th.cos() - f0 * th.sin()));
        p += fd * ds;
    }
    let at = |s: f32| -> (Vec3, Vec3) {
        let x = ((s - s_min) / ds).clamp(0.0, steps_n as f32);
        let i = (x.floor() as usize).min(steps_n - 1);
        let f = x - i as f32;
        (
            track[i].0.lerp(track[i + 1].0, f),
            track[i].2.lerp(track[i + 1].2, f).normalize(),
        )
    };
    // u runs across the hand, +1 at the index finger's edge.
    let palm_len = |u: f32| {
        table(
            &[
                (-1.0, [0.084]),
                (-0.75, [0.086]),
                (-0.25, [0.094]),
                (0.25, [0.097]),
                (0.75, [0.095]),
                (1.0, [0.094]),
            ],
            u,
        )[0]
    };
    let finger = |u: f32| {
        table(
            &[
                (-1.0, [0.054]),
                (-0.75, [0.06]),
                (-0.25, [0.078]),
                (0.25, [0.086]),
                (0.75, [0.078]),
                (1.0, [0.072]),
            ],
            u,
        )[0]
    };
    let groove = |u: f32| {
        [-0.5f32, 0.0, 0.5]
            .iter()
            .map(|c| bell(u - c, 0.1))
            .sum::<f32>()
    };
    let width = |s: f32| {
        table(
            &[
                (-0.03, [0.022]),
                (0.0, [0.027]),
                (0.03, [0.036]),
                (0.065, [0.042]),
                (0.095, [0.043]),
                (0.13, [0.04]),
                (0.18, [0.036]),
            ],
            s,
        )[0]
    };
    let thick = |s: f32| {
        table(
            &[
                (-0.03, [0.016]),
                (0.0, [0.018]),
                (0.03, [0.0165]),
                (0.07, [0.015]),
                (0.095, [0.0125]),
                (0.13, [0.0095]),
                (0.18, [0.0085]),
            ],
            s,
        )[0]
    };
    let s_start = -0.025;
    let bk = 0.5;
    let mut bs = steps(0.0, 1.0, 15);
    bs.extend_from_slice(&[0.02, 0.975]);
    bs.sort_by(f32::total_cmp);
    surface(m, &steps(0.0, 1.0, 14), &bs, (true, false), true, |a, b| {
        let phi = a * TAU;
        let (c, sn) = (phi.cos(), phi.sin());
        let cs = c.signum() * c.abs().powf(0.6);
        let ss = sn.signum() * sn.abs().powf(0.6);
        let u = cs * side;
        let pl = palm_len(u);
        let total = pl + finger(u) * (1.0 - 0.3 * groove(u));
        let s = if b < bk {
            s_start + (pl - s_start) * b / bk
        } else {
            pl + (total - pl) * (b - bk) / (1.0 - bk)
        };
        let (center, nd) = at(s);
        let mut h = thick(s);
        if ss > 0.0 {
            // The pad at the base of the thumb.
            h += 0.006 * smooth(0.1, 0.8, u) * bell(s - 0.03, 0.025);
        }
        h *= 1.0 - 0.45 * groove(u) * smooth(pl - 0.01, pl + 0.015, s);
        let tip = ((s - (total - 0.012)) / 0.012).clamp(0.0, 1.0);
        h *= (1.0 - tip * tip).sqrt();
        let k = dome(s, s_start, 10.0, 0.02, 1.0);
        let p = center + s0 * (width(s) * cs * k) + nd * (h * ss * k);
        // Nails on the back of the fingertips, the palm a little paler.
        let nail = smooth(total - 0.016, total - 0.011, s) * smooth(-0.3, -0.7, ss);
        let mut col = skin_tint(p, tone, 0.35 * bell(s - pl, 0.012) * smooth(0.0, -0.5, ss));
        col = col.lerp(
            rgba(tone * Vec3::new(1.12, 1.06, 1.02)),
            smooth(0.3, 0.8, ss) * 0.6,
        );
        col = col.lerp(rgba(Vec3::new(1.35, 1.15, 1.12)), nail);
        Sample::new(p, Vec2::new(a * 0.18, s), col)
    });
    // Thumb.
    let t_side = s0 * side;
    let base = wrist + f0 * 0.022 + t_side * 0.02 + n0 * 0.006;
    let d1 = (f0 * 0.75 + t_side * (0.4 + 0.15 * spread) + n0 * (0.15 + 0.4 * spread)).normalize();
    let mcp = base + d1 * 0.046;
    let d2 = (f0 * 0.9 + t_side * 0.12 * spread + n0 * (0.1 + 0.5 * spread)).normalize();
    let chain = Chain {
        joints: vec![base, mcp, mcp + d2 * 0.048],
        fronts: vec![n0, n0],
        round: 0.012,
    };
    loft(
        m,
        &chain,
        &steps(-0.2, 2.2, 10),
        8,
        &|t, _| {
            table(
                &[
                    (0.0, [0.0155]),
                    (1.0, [0.0115]),
                    (1.6, [0.0105]),
                    (2.0, [0.0092]),
                ],
                t,
            )[0] * dome(t, -0.2, 2.2, 0.25, 0.3)
        },
        &|p, _, _| skin_tint(p, tone, 0.0),
    );
}

/// A bare foot below `ankle`: heel, arch, the ball and five toes (scalloped, grooved between,
/// nails lighter), bent up at the ball by `toe_bend` radians. `fwd` toward the toes, `up` out
/// of the instep; `side` +1 for the right foot.
fn foot(m: &mut Mesh, ankle: Vec3, fwd: Vec3, up: Vec3, side: f32, toe_bend: f32, tone: Vec3) {
    let f0 = fwd.normalize();
    let u0 = (up - f0 * up.dot(f0)).normalize();
    let s0 = f0.cross(u0);
    let ball = 0.14;
    // Sole line: straight to the ball, then the toes bend up.
    let (s_min, s_max, steps_n) = (-0.08f32, 0.22f32, 40usize);
    let ds = (s_max - s_min) / steps_n as f32;
    let mut track = Vec::with_capacity(steps_n + 1);
    let mut p = ankle + f0 * s_min;
    for i in 0..=steps_n {
        let s = s_min + ds * i as f32;
        let th = toe_bend * smooth(ball - 0.012, ball + 0.02, s);
        let fd = f0 * th.cos() + u0 * th.sin();
        track.push((p, u0 * th.cos() - f0 * th.sin()));
        p += fd * ds;
    }
    let at = |s: f32| -> (Vec3, Vec3) {
        let x = ((s - s_min) / ds).clamp(0.0, steps_n as f32);
        let i = (x.floor() as usize).min(steps_n - 1);
        let f = x - i as f32;
        (
            track[i].0.lerp(track[i + 1].0, f),
            track[i].1.lerp(track[i + 1].1, f).normalize(),
        )
    };
    // ul runs across the foot, +1 at the little toe's edge.
    let toe_end = |ul: f32| {
        table(
            &[
                (-1.0, [0.2]),
                (-0.75, [0.206]),
                (-0.35, [0.199]),
                (0.05, [0.19]),
                (0.42, [0.178]),
                (0.78, [0.163]),
                (1.0, [0.152]),
            ],
            ul,
        )[0]
    };
    let groove = |ul: f32| {
        1.6 * bell(ul + 0.55, 0.08)
            + [-0.15f32, 0.24, 0.6]
                .iter()
                .map(|c| bell(ul - c, 0.07))
                .sum::<f32>()
    };
    let top = |s: f32| {
        table(
            &[
                (-0.08, [-0.035]),
                (-0.06, [0.0]),
                (-0.03, [0.03]),
                (0.02, [0.022]),
                (0.05, [-0.008]),
                (0.09, [-0.034]),
                (0.13, [-0.048]),
                (0.16, [-0.052]),
                (0.21, [-0.06]),
            ],
            s,
        )[0]
    };
    let width = |s: f32| {
        table(
            &[
                (-0.08, [0.02]),
                (-0.055, [0.03]),
                (0.0, [0.033]),
                (0.05, [0.037]),
                (0.1, [0.045]),
                (0.14, [0.048]),
                (0.17, [0.046]),
                (0.21, [0.04]),
            ],
            s,
        )[0]
    };
    let sole = -0.072;
    let s_start = -0.075;
    let mut bs = steps(0.0, 1.0, 16);
    bs.push(0.975);
    bs.sort_by(f32::total_cmp);
    surface(m, &steps(0.0, 1.0, 14), &bs, (true, false), true, |a, b| {
        let phi = a * TAU;
        let (c, sn) = (phi.cos(), phi.sin());
        let cs = c.signum() * c.abs().powf(0.75);
        let ul = cs * side;
        let end = toe_end(ul) - 0.012 * groove(ul).min(1.0);
        let s = s_start + (end - s_start) * b;
        let (center, ud) = at(s);
        // Arch under the inner side.
        let bot = sole + 0.014 * smooth(0.0, -0.7, ul) * bell(s - 0.06, 0.045);
        let mut hi = top(s);
        hi -= 0.008 * groove(ul).min(1.0) * smooth(ball, ball + 0.02, s);
        let mid = (hi + bot) * 0.5;
        let mut half = (hi - bot) * 0.5;
        let tip = ((s - (end - 0.013)) / 0.013).clamp(0.0, 1.0);
        half *= (1.0 - tip * tip).sqrt();
        let v = if sn >= 0.0 {
            sn.powf(0.75)
        } else {
            -(-sn).powf(0.3)
        };
        let k = dome(s, s_start, 10.0, 0.03, 1.0);
        let p = center + s0 * (width(s) * cs * k) + ud * (mid + half * v * k);
        let nail = smooth(end - 0.016, end - 0.01, s) * smooth(0.3, 0.7, v);
        let mut col = skin_tint(p, tone, 0.3 * smooth(ball + 0.01, end, s));
        col = col.lerp(
            rgba(tone * Vec3::new(1.1, 1.02, 0.98)),
            smooth(-0.3, -0.8, v) * 0.7,
        );
        col = col.lerp(rgba(Vec3::new(1.3, 1.12, 1.08)), nail);
        Sample::new(p, Vec2::new(a * 0.2, s), col)
    });
}

/// The eyes' centers in the head's frame (right eye; the left mirrors z) and their radius.
const EYE: Vec3 = Vec3::new(0.077, 0.004, 0.032);
const EYE_R: f32 = 0.0118;

/// The head's skin as a distance field in its own frame (x forward, y up, eyes at y ≈ 0,
/// ears at z = ±0.077). The eyeballs are separate meshes seen through the lids' slits.
fn head_sdf(p: Vec3) -> f32 {
    let q = Vec3::new(p.x, p.y, p.z.abs());
    let ell = |c: Vec3, r: Vec3| {
        let d = q - c;
        let k0 = (d / r).length();
        let k1 = (d / (r * r)).length().max(1e-9);
        k0 * (k0 - 1.0) / k1
    };
    let cap = |a: Vec3, b: Vec3, r: f32| {
        let (pa, ba) = (q - a, b - a);
        let h = (pa.dot(ba) / ba.dot(ba)).clamp(0.0, 1.0);
        (pa - ba * h).length() - r
    };
    let smin = |a: f32, b: f32, k: f32| -smax(-a, -b, k);
    // Cranium, the face, the jaw and chin, cheekbones and brow.
    let mut d = ell(Vec3::new(-0.008, 0.022, 0.0), Vec3::new(0.1, 0.104, 0.077));
    d = smin(
        d,
        ell(Vec3::new(0.034, -0.03, 0.0), Vec3::new(0.061, 0.066, 0.064)),
        0.045,
    );
    d = smin(
        d,
        ell(
            Vec3::new(0.05, -0.045, 0.038),
            Vec3::new(0.026, 0.026, 0.02),
        ),
        0.015,
    );
    // Cheekbone arches running back to the ears.
    d = smin(
        d,
        cap(
            Vec3::new(0.055, -0.006, 0.052),
            Vec3::new(0.0, -0.012, 0.066),
            0.009,
        ),
        0.02,
    );
    d = smin(
        d,
        cap(
            Vec3::new(-0.012, -0.066, 0.056),
            Vec3::new(0.066, -0.101, 0.017),
            0.013,
        ),
        0.028,
    );
    d = smin(
        d,
        ell(
            Vec3::new(0.077, -0.097, 0.0),
            Vec3::new(0.023, 0.019, 0.027),
        ),
        0.022,
    );
    d = smin(
        d,
        ell(
            Vec3::new(0.058, -0.008, 0.05),
            Vec3::new(0.02, 0.014, 0.022),
        ),
        0.016,
    );
    d = smin(
        d,
        cap(
            Vec3::new(0.085, 0.025, 0.0),
            Vec3::new(0.079, 0.027, 0.045),
            0.011,
        ),
        0.012,
    );
    // Eye sockets, and the lids over the eyes open in an almond slit.
    d = smax(
        d,
        0.019 - (q - Vec3::new(0.1, 0.004, 0.032)).length(),
        0.008,
    );
    let e = q - EYE;
    let lids = ell(EYE, Vec3::new(0.0138, 0.0138, 0.0148));
    let open = 0.0034 * (1.0 - (e.z / 0.0128).powi(2)) - (e.y + 0.0016).abs();
    d = smin(d, smax(lids, open, 0.0015), 0.003);
    // Nose: bridge, tip, wings and nostrils.
    d = smin(
        d,
        cap(
            Vec3::new(0.092, 0.01, 0.0),
            Vec3::new(0.11, -0.029, 0.0),
            0.0065,
        ),
        0.01,
    );
    d = smin(
        d,
        (q - Vec3::new(0.108, -0.032, 0.0)).length() - 0.0085,
        0.007,
    );
    d = smin(
        d,
        (q - Vec3::new(0.099, -0.037, 0.0105)).length() - 0.0065,
        0.005,
    );
    d = smax(
        d,
        0.0035 - (q - Vec3::new(0.104, -0.045, 0.0065)).length(),
        0.003,
    );
    // Lips and the line between them.
    d = smin(
        d,
        ell(
            Vec3::new(0.088, -0.0585, 0.0),
            Vec3::new(0.0075, 0.0055, 0.024),
        ),
        0.006,
    );
    d = smin(
        d,
        ell(
            Vec3::new(0.085, -0.0705, 0.0),
            Vec3::new(0.0078, 0.0065, 0.02),
        ),
        0.006,
    );
    d = smax(
        d,
        -ell(
            Vec3::new(0.097, -0.0642, 0.0),
            Vec3::new(0.012, 0.0014, 0.021),
        ),
        0.002,
    );
    d
}

/// Where the head's surface is along `dir` from a point inside it.
fn head_hit(dir: Vec3) -> Vec3 {
    let o = Vec3::new(0.0, -0.01, 0.0);
    let mut t0 = 0.0;
    let mut t1 = 0.003;
    while head_sdf(o + dir * t1) < 0.0 && t1 < 0.25 {
        t0 = t1;
        t1 += 0.003;
    }
    for _ in 0..12 {
        let t = (t0 + t1) * 0.5;
        if head_sdf(o + dir * t) < 0.0 {
            t0 = t;
        } else {
            t1 = t;
        }
    }
    o + dir * ((t0 + t1) * 0.5)
}

/// Ambient occlusion of the head's skin at `p`: how much the distance field closes in
/// along its normal (dark in the sockets, nostrils, mouth corners and under the jaw).
fn head_ao(p: Vec3) -> f32 {
    let e = 4e-4;
    let n = Vec3::new(
        head_sdf(p + Vec3::X * e) - head_sdf(p - Vec3::X * e),
        head_sdf(p + Vec3::Y * e) - head_sdf(p - Vec3::Y * e),
        head_sdf(p + Vec3::Z * e) - head_sdf(p - Vec3::Z * e),
    )
    .normalize_or(Vec3::Y);
    let mut occ = 0.0;
    for (i, h) in [0.003f32, 0.007, 0.013].into_iter().enumerate() {
        occ += (h - head_sdf(p + n * h)).max(0.0) / h * [0.5, 0.3, 0.2][i];
    }
    (1.0 - 0.9 * occ).clamp(0.35, 1.0)
}

/// The unit direction for polar angle `th` from the crown and azimuth `ph` (0 = the face,
/// π/2 = the right ear).
fn head_dir(th: f32, ph: f32) -> Vec3 {
    Vec3::new(th.sin() * ph.cos(), th.cos(), th.sin() * ph.sin())
}

/// Polar angles for the head: finest over the brows and eyes, fine over the face, coarse over
/// the crown (under hair or a cap from `fine_from`) and under the chin.
fn head_rows(fine_from: f32) -> Vec<f32> {
    let mut v = steps(0.0, fine_from, 3);
    for (a, b, n) in [
        (fine_from, 1.1, 3),
        (1.1, 1.34, 6),
        (1.34, 1.47, 9),
        (1.47, 1.62, 4),
        (1.62, 2.2, 9),
        (2.2, PI, 5),
    ] {
        v.extend(steps(a, b, n).into_iter().skip(1));
    }
    v
}

/// Azimuths, from the back of the head around: fine over the face.
fn head_cols() -> Vec<f32> {
    let mut v = steps(-PI, -1.15, 8);
    v.extend(steps(-1.15, 1.15, 26).into_iter().skip(1));
    v.extend(steps(1.15, PI, 8).into_iter().skip(1));
    v
}

/// The face: lips, brows, nostrils, a flush over the cheeks and nose, lash lines, stubble.
fn face_tint(p: Vec3, tone: Vec3, stubble: f32, brow: Vec3) -> Vec4 {
    let q = Vec3::new(p.x, p.y, p.z.abs());
    let front = smooth(0.06, 0.085, p.x);
    let mut c = tone * (0.97 + 0.06 * fbm3(p * 12.0, 0xface, 2)) * head_ao(p);
    let lips = bell(p.y + 0.064, 0.008) * bell(q.z, 0.02).sqrt() * smooth(0.08, 0.09, p.x);
    c = c.lerp(c * Vec3::new(1.12, 0.74, 0.74), lips * 0.85);
    let cheek = bell((q - Vec3::new(0.08, -0.03, 0.045)).length(), 0.022);
    let nose = bell((q - Vec3::new(0.11, -0.03, 0.0)).length(), 0.012);
    c = c.lerp(
        c * Vec3::new(1.08, 0.92, 0.9),
        (cheek * 0.5 + nose * 0.4) * front,
    );
    // Stubble over the jaw, chin and upper lip.
    let beard = smooth(-0.03, -0.07, p.y) * smooth(-0.03, 0.04, p.x) * (1.0 - lips);
    c = c.lerp(c * Vec3::new(0.85, 0.84, 0.88), beard * stubble);
    // Brows: arched bands over the eyes.
    let arch = 0.029 - 0.01 * ((q.z - 0.033) / 0.025).powi(2);
    let brows = smooth(0.0065, 0.002, (p.y - arch).abs())
        * smooth(0.008, 0.016, q.z)
        * smooth(0.058, 0.044, q.z)
        * smooth(0.07, 0.085, p.x);
    c = c.lerp(brow, brows * 0.92);
    // Lash lines along the lids' edges.
    let e = q - EYE;
    let rim = (0.0034 * (1.0 - (e.z / 0.0128).powi(2)) - (e.y + 0.0016).abs()).abs();
    let lash = bell(rim, 0.0015) * smooth(0.0175, 0.012, e.length());
    c *= 1.0 - 0.55 * lash;
    rgba(c)
}

/// How the face looks: skin tone, stubble (0..1), brow color, and the hair's color showing
/// through the scalp above `hairline(azimuth)` (polar angle) when `hair` is set.
struct Face<'a> {
    tone: Vec3,
    stubble: f32,
    brow: Vec3,
    hair: Option<(&'a dyn Fn(f32) -> f32, Vec3)>,
}

/// The head's skin in the body frame (crown rows coarse up to `fine_from` radians).
fn head_skin(m: &mut Mesh, pose: &Pose, face: &Face, fine_from: f32) {
    let mut h = Mesh::default();
    surface(
        &mut h,
        &head_cols(),
        &head_rows(fine_from),
        (true, false),
        false,
        |ph, th| {
            let p = head_hit(head_dir(th, ph));
            let mut c = face_tint(p, face.tone, face.stubble, face.brow);
            if let Some((edge, hair)) = face.hair {
                let roots = smooth(edge(ph) + 0.05, edge(ph) - 0.02, th);
                c = c.lerp(rgba(hair * 0.9), roots * 0.8);
            }
            Sample::new(p, Vec2::new(ph * 0.09, th * 0.1), c)
        },
    );
    h.transform(&pose.head_xf());
    m.merge(&h);
}

/// The eyeballs (white, an iris of `iris` color, the pupil) gazing along `gaze` in the head's
/// frame, into `m` in the body frame.
fn eyes(m: &mut Mesh, pose: &Pose, gaze: Vec3, iris: Vec3) {
    let g = gaze.normalize();
    let u = g.any_orthonormal_vector();
    let v = u.cross(g);
    let rows = [0.0, 0.16, 0.2, 0.42, 0.48, 0.54, 0.9, 1.4, 2.2, PI];
    let mut e = Mesh::default();
    for side in [-1.0f32, 1.0] {
        let c = EYE * Vec3::new(1.0, 1.0, side);
        surface(
            &mut e,
            &steps(0.0, 1.0, 12),
            &rows,
            (true, false),
            false,
            |a, th| {
                let ph = a * TAU;
                let p = c + (g * th.cos() + (u * ph.cos() + v * ph.sin()) * th.sin()) * EYE_R;
                let col = if th < 0.18 {
                    Vec3::splat(0.03)
                } else if th < 0.48 {
                    iris * (0.8 + 0.4 * smooth(0.2, 0.45, th))
                } else {
                    Vec3::new(1.25, 1.22, 1.15) * (1.0 - 0.3 * smooth(0.9, 2.0, th))
                };
                Sample::new(p, Vec2::new(a * 0.07, th * EYE_R), rgba(col))
            },
        );
    }
    e.transform(&pose.head_xf());
    m.merge(&e);
}

/// An ear on `side` (+1 right) in the head's frame: rim, hollow and lobe.
fn ear(m: &mut Mesh, pose: &Pose, side: f32, tone: Vec3) {
    let up = Vec3::new(-0.26, 0.97, 0.0).normalize();
    let back = Vec3::new(-0.97, -0.26, 0.0);
    let out = Vec3::new(0.0, 0.0, side);
    let base = head_hit(Vec3::new(-0.1, -0.05, side).normalize()) - out * 0.002;
    let (a, b) = (0.031, 0.018);
    let mut e = Mesh::default();
    for lateral in [true, false] {
        let flip = lateral == (side > 0.0);
        surface(
            &mut e,
            &steps(0.0, 1.0, 14),
            &steps(0.0, 1.0, 5),
            (true, false),
            flip,
            |u, rho| {
                let ph = u * TAU;
                let (c, s) = (ph.cos(), ph.sin());
                let h = if lateral {
                    let rim = 0.4 + 0.6 * smooth(0.5, 0.85, rho);
                    let lobe = 0.35 * smooth(-0.5, -0.9, c);
                    0.011 * (1.0 - rho.powi(4)).max(0.0).sqrt() * (rim + lobe).min(1.0)
                        + 0.006 * s.max(0.0) * rho
                } else {
                    -0.006 * (1.0 - rho * rho).max(0.0).sqrt()
                };
                let p = base + up * (a * rho * c) + back * (b * rho * s) + out * h + back * 0.006;
                let hollow = if lateral {
                    1.0 - 0.25 * bell(rho - 0.35, 0.25)
                } else {
                    1.0
                };
                Sample::new(
                    p,
                    Vec2::new(ph * 0.03, rho * 0.03),
                    skin_tint(p, tone * hollow, 0.6),
                )
            },
        );
    }
    orient_outward(&mut e);
    e.transform(&pose.head_xf());
    m.merge(&e);
}

/// A shell over the scalp in the body frame (hair or a swim cap): from the crown down to the
/// edge at polar angle `edge(azimuth)`, `thick(t)` above the skin (t = 0 crown .. 1 edge),
/// tucked into the skin at the edge.
fn scalp_shell(
    m: &mut Mesh,
    pose: &Pose,
    edge: &dyn Fn(f32) -> f32,
    thick: &dyn Fn(f32, Vec3) -> f32,
    color: &dyn Fn(Vec3, f32) -> Vec4,
) {
    let mut s = Mesh::default();
    let mut ts = steps(0.0, 1.0, 12);
    ts.extend_from_slice(&[0.96, 0.985]);
    ts.sort_by(f32::total_cmp);
    surface(&mut s, &head_cols(), &ts, (true, false), false, |ph, t| {
        let th = edge(ph) * t;
        let dir = head_dir(th, ph);
        let skin = head_hit(dir);
        let tuck = smooth(0.9, 1.0, t);
        let p = skin + dir * (thick(t, skin) * (1.0 - tuck) - 0.0015 * tuck);
        Sample::new(p, Vec2::new(ph * 0.1, th * 0.1), color(p, t))
    });
    s.transform(&pose.head_xf());
    m.merge(&s);
}

/// The torso's skin from `y0` to the top of the neck (inside the head), domed at both ends.
fn torso_skin(m: &mut Mesh, y0: f32, tone: Vec3) {
    let y1 = 1.645;
    let rows = ((y1 - y0) / 0.024).round() as usize;
    surface(
        m,
        &steps(0.0, 1.0, 32),
        &steps(y0, y1, rows),
        (true, false),
        false,
        |a, y| {
            let th = a * TAU;
            let (r, cx) = torso_radius(y, th, 1.0);
            let r = r * dome(y, y0, y1, 0.03, 0.03);
            let p = Vec3::new(cx + r * th.cos(), y, -r * th.sin());
            Sample::new(p, Vec2::new(a * TAU * 0.15, y), skin_tint(p, tone, 0.0))
        },
    );
}

/// A cloth shell around the torso from `y0` to `y1(th)`, `offset(y, th)` off the skin with
/// the muscles' relief scaled by `relief`.
fn torso_shell(
    m: &mut Mesh,
    y0: f32,
    y1: &dyn Fn(f32) -> f32,
    rows: usize,
    relief: f32,
    offset: &dyn Fn(f32, f32) -> f32,
    color: &dyn Fn(Vec3, f32) -> Vec4,
) {
    surface(
        m,
        &steps(0.0, 1.0, 32),
        &steps(0.0, 1.0, rows),
        (true, false),
        false,
        |a, t| {
            let th = a * TAU;
            let y = y0 + (y1(th) - y0) * t;
            let (r, cx) = torso_radius(y, th, relief);
            let r = (r + offset(y, th)) * dome(y, 0.795, 10.0, 0.02, 1.0);
            let p = Vec3::new(cx + r * th.cos(), y, -r * th.sin());
            Sample::new(p, Vec2::new(a * TAU * 0.16, y), color(p, th))
        },
    );
}

/// A rolled hem: a thin closed tube through `pts`.
fn hem(m: &mut Mesh, pts: &[Vec3], radius: f32, color: Vec4) {
    tube(m, pts, 6, true, &|_| radius, color);
}

/// The points around a loft's ring at station `t`, `offset` outside `radius`.
fn ring_at(chain: &Chain, t: f32, n: usize, radius: &dyn Fn(f32, f32) -> f32) -> Vec<Vec3> {
    (0..n)
        .map(|i| {
            let phi = i as f32 / n as f32 * TAU;
            let (c, tan, front) = chain.frame(t);
            c + (front * phi.cos() + tan.cross(front) * phi.sin()) * radius(t, phi)
        })
        .collect()
}

/// Move the parts so their lowest point is at y = 0 (`ground`) or their bounds are centered
/// on the origin.
fn settle(kit: &mut Kit, xf: Mat4, ground: bool) {
    let mut lo = Vec3::splat(f32::MAX);
    let mut hi = Vec3::splat(f32::MIN);
    for (_, m) in &mut kit.parts {
        m.transform(&xf);
        for v in &m.vertices {
            lo = lo.min(Vec3::from(v.position));
            hi = hi.max(Vec3::from(v.position));
        }
    }
    let shift = if ground {
        Vec3::new(-(lo.x + hi.x) * 0.5, -lo.y, -(lo.z + hi.z) * 0.5)
    } else {
        -(lo + hi) * 0.5
    };
    for (_, m) in &mut kit.parts {
        m.transform(&Mat4::from_translation(shift));
    }
}

/// A swimmer doing the front crawl, about 1.76 m tall: horizontal along x with the head
/// toward +x, face down, rolled 35° onto the right side as the left arm recovers high over
/// the water and the head turns left to breathe; the right arm reaches ahead, the legs flutter
/// with pointed toes. Jammers, a silicone cap and goggles. Centered on the origin (about
/// 2.3 m long with the arm reaching ahead). About 14 000 triangles.
pub fn swimmer() -> Vec<Part> {
    // Body frame: x = the belly side (down in the water), y = toward the head, z = right.
    let pose = Pose {
        arms: [
            // Left: recovering, elbow high and out to the side, the forearm hanging forward.
            [Vec3::new(-0.5, 0.25, -0.84), Vec3::new(0.55, 0.62, 0.25)],
            // Right: reaching ahead, nearly straight.
            [Vec3::new(0.14, 1.0, 0.08), Vec3::new(0.24, 1.0, 0.03)],
        ],
        hands: [
            (
                Vec3::new(0.65, 0.65, 0.3),
                Vec3::new(-0.3, -0.8, 0.5),
                0.25,
                0.2,
            ),
            (
                Vec3::new(0.33, 1.0, 0.0),
                Vec3::new(1.0, -0.33, 0.0),
                0.12,
                0.05,
            ),
        ],
        legs: [
            [Vec3::new(-0.1, -1.0, -0.03), Vec3::new(-0.13, -1.0, -0.03)],
            [Vec3::new(0.12, -1.0, 0.03), Vec3::new(-0.3, -1.0, 0.02)],
        ],
        feet: [
            (
                Vec3::new(0.25, -1.0, -0.06),
                Vec3::new(1.0, 0.25, 0.0),
                -0.15,
            ),
            (
                Vec3::new(0.08, -1.0, 0.03),
                Vec3::new(1.0, 0.08, 0.0),
                -0.15,
            ),
        ],
        head: Quat::from_rotation_y(0.85) * Quat::from_rotation_z(0.12),
    };
    let tone = Vec3::new(1.0, 0.97, 0.95);
    let mut kit = Kit::default();
    let skin = kit.mesh(Surface::Skin);
    torso_skin(skin, 0.95, tone);
    for (k, side) in SIDES.into_iter().enumerate() {
        arm_skin(skin, &pose, k, -0.1, tone);
        leg_skin(skin, &pose, k, 0.45, tone);
        let arm = pose.arm(k);
        let (hd, hp, curl, spread) = pose.hands[k];
        hand(skin, arm.joints[2], hd, hp, side, curl, spread, tone);
        let leg = pose.leg(k);
        let (fd, fu, bend) = pose.feet[k];
        foot(skin, leg.joints[2], fd, fu, side, bend, tone);
    }
    let face = Face {
        tone,
        stubble: 0.0,
        brow: Vec3::new(0.35, 0.28, 0.22),
        hair: None,
    };
    head_skin(skin, &pose, &face, 0.95);
    let mut eyeballs = Mesh::default();
    eyes(
        &mut eyeballs,
        &pose,
        Vec3::new(1.0, 0.15, 0.0),
        Vec3::new(0.12, 0.22, 0.3),
    );
    kit.add(Surface::Plastic, &eyeballs);

    // Jammers: navy, a pale stripe down each side, a darker waistband.
    let navy = Vec3::new(0.06, 0.1, 0.32);
    let fabric = kit.mesh(Surface::Fabric);
    torso_shell(
        fabric,
        0.795,
        &|_| 1.0,
        9,
        0.6,
        &|y, _| 0.004 + 0.0015 * smooth(0.965, 0.975, y) - 0.007 * smooth(0.99, 1.0, y),
        &|p, _| rgba(navy * (1.0 - 0.3 * smooth(0.965, 0.975, p.y))),
    );
    for (k, side) in SIDES.into_iter().enumerate() {
        let chain = pose.leg(k);
        let r = move |t: f32, phi: f32| {
            (limb_radius(&LEG, t, phi, side) + 0.004 - 0.007 * smooth(0.6, 0.64, t))
                * dome(t, -0.15, 2.0, 0.15, 1.0)
        };
        loft(
            fabric,
            &chain,
            &steps(-0.15, 0.64, 10),
            20,
            &r,
            &|_, _, phi| {
                let stripe = smooth(0.9, 0.96, phi.sin() * side);
                rgba(navy.lerp(Vec3::new(1.2, 1.25, 1.3), stripe))
            },
        );
    }
    // Swim cap: yellow silicone pulled down over the ears.
    let cap_edge = |ph: f32| {
        table(
            &[
                (0.0, [0.98]),
                (0.9, [1.25]),
                (1.45, [1.82]),
                (2.2, [1.98]),
                (PI, [2.06]),
            ],
            ph.abs(),
        )[0]
    };
    scalp_shell(
        fabric,
        &pose,
        &cap_edge,
        &|t, _| 0.004 + 0.003 * (1.0 - t),
        &|p, t| {
            let n = fbm3(p * 30.0, 0xcab, 2);
            rgba(
                Vec3::new(1.45, 1.05, 0.08)
                    * (0.94 + 0.06 * n)
                    * (1.0 - 0.1 * smooth(0.85, 1.0, t)),
            )
        },
    );
    // Goggles: blue frames, tinted lenses, a black strap round the back of the cap.
    let mut frame = Mesh::default();
    let mut lens = Mesh::default();
    let mut strap = Mesh::default();
    let blue = rgba(Vec3::new(0.15, 0.35, 1.2));
    for side in [-1.0f32, 1.0] {
        let c = Vec3::new(0.1, 0.006, 0.033 * side);
        let axis = Vec3::new(1.0, 0.0, 0.25 * side).normalize();
        // Toward the temple, then up.
        let across = Vec3::Y.cross(axis) * -side;
        // The frame's outline pressed onto the face (`grow` widens it).
        let outline = |a: f32, grow: f32| {
            let q = c + Vec3::Y * ((0.0135 + grow) * a.sin()) + across * ((0.019 + grow) * a.cos());
            let d = (q - Vec3::new(0.0, -0.01, 0.0)).normalize();
            head_hit(d) + d * 0.0045
        };
        let rim: Vec<Vec3> = (0..20)
            .map(|i| outline(i as f32 / 20.0 * TAU, 0.0))
            .collect();
        tube(&mut frame, &rim, 6, true, &|_| 0.0035, blue);
        // The lens: a shallow dome in the frame.
        let mid = rim.iter().copied().sum::<Vec3>() / rim.len() as f32 + axis * 0.004;
        let mut l = Mesh::default();
        surface(
            &mut l,
            &steps(0.0, 1.0, 16),
            &steps(0.0, 1.0, 4),
            (true, false),
            false,
            |a, rho| {
                let q = outline(a * TAU, -0.0015);
                let p = mid.lerp(q, rho) + axis * (0.005 * (1.0 - rho * rho));
                Sample::new(p, Vec2::new(a, rho), rgba(Vec3::new(0.6, 0.8, 1.4)))
            },
        );
        orient_lens(&mut l, axis);
        lens.merge(&l);
        // Strap from the temple round the back.
        let mut pts = vec![outline(0.0, 0.0) + across * 0.004];
        for i in 0..=16 {
            let ph = side * (0.95 + (PI - 0.95) * i as f32 / 16.0);
            let th = 1.6 - 0.25 * (i as f32 / 16.0);
            let d = head_dir(th, ph);
            pts.push(head_hit(d) + d * 0.0085);
        }
        let frames = path_frames(&pts, |i| {
            let p = pts[i.min(pts.len() - 1)];
            (p - Vec3::new(0.0, -0.01, 0.0)).normalize()
        });
        sweep_rect(
            &mut strap,
            &frames,
            Vec2::new(0.0012, 0.007),
            0.0008,
            8,
            &|_| rgba(Vec3::splat(0.6)),
        );
    }
    let bridge = [
        Vec3::new(0.103, 0.004, -0.014),
        Vec3::new(0.108, 0.0, 0.0),
        Vec3::new(0.103, 0.004, 0.014),
    ];
    tube(&mut frame, &bridge, 6, false, &|_| 0.0025, blue);
    for (s, mut m) in [
        (Surface::Plastic, frame),
        (Surface::Glass, lens),
        (Surface::Rubber, strap),
    ] {
        m.transform(&pose.head_xf());
        kit.add(s, &m);
    }

    // Into the water: the head toward +x, the belly down, rolled onto the right side.
    let xf = Mat4::from_rotation_x(35f32.to_radians()) * Mat4::from_rotation_z(-FRAC_PI_2);
    settle(&mut kit, xf, false);
    kit.finish()
}

/// Make a lens dome face along `axis` (its convex side) whichever way it was wound.
fn orient_lens(m: &mut Mesh, axis: Vec3) {
    let facing: f32 = m
        .vertices
        .iter()
        .map(|v| Vec3::from(v.normal).dot(axis))
        .sum();
    if facing < 0.0 {
        for v in &mut m.vertices {
            v.normal = (-Vec3::from(v.normal)).to_array();
        }
        for t in m.indices.chunks_mut(3) {
            t.swap(1, 2);
        }
    }
}

/// A man wading, about 1.76 m tall, soles at y = 0, facing +x mid-stride: the right foot
/// forward on its heel, the left pushing off its toes, arms held out from the sides for
/// balance with the hands above the water, looking down ahead. Red T-shirt, teal board shorts,
/// short dark hair. About 15 000 triangles.
pub fn wader() -> Vec<Part> {
    let rz = |deg: f32| Quat::from_rotation_z(deg.to_radians());
    let rx = |deg: f32| Quat::from_rotation_x(deg.to_radians());
    let down = Vec3::NEG_Y;
    // Legs: right forward (hip 22°, knee 10°), left back (hip -12°, knee 30°).
    let r_thigh = rz(22.0) * rx(-3.0);
    let r_shin = r_thigh * rz(-10.0);
    let l_thigh = rz(-12.0) * rx(3.0);
    let l_shin = l_thigh * rz(-30.0);
    // The back foot pitches heel-up until its ball is as low as the front heel.
    let leg_end = |q1: Quat, q2: Quat| q1 * down * THIGH + q2 * down * SHIN;
    let front_heel = leg_end(r_thigh, r_shin).y - 0.075;
    let back_ankle = leg_end(l_thigh, l_shin).y;
    let pitch = (0..=60)
        .map(|d| d as f32)
        .min_by(|a, b| {
            let ball = |deg: f32| back_ankle + (rz(-deg) * Vec3::new(0.14, -0.072, 0.0)).y;
            (ball(*a) - front_heel)
                .abs()
                .total_cmp(&(ball(*b) - front_heel).abs())
        })
        .unwrap_or(30.0);
    let toe_out = |side: f32| Quat::from_rotation_y(-0.1 * side);
    let pose = Pose {
        arms: [
            [
                rz(28.0) * rx(22.0) * down,
                rz(28.0) * rx(22.0) * rz(62.0) * down,
            ],
            [
                rz(6.0) * rx(-26.0) * down,
                rz(6.0) * rx(-26.0) * rz(55.0) * down,
            ],
        ],
        hands: [
            (
                rz(28.0) * rx(22.0) * rz(75.0) * down,
                Vec3::new(-0.2, -1.0, 0.35),
                0.55,
                0.6,
            ),
            (
                rz(6.0) * rx(-26.0) * rz(68.0) * down,
                Vec3::new(-0.1, -1.0, -0.35),
                0.5,
                0.6,
            ),
        ],
        legs: [
            [l_thigh * down, l_shin * down],
            [r_thigh * down, r_shin * down],
        ],
        feet: [
            (
                toe_out(-1.0) * rz(-pitch) * Vec3::X,
                toe_out(-1.0) * rz(-pitch) * Vec3::Y,
                pitch.to_radians() * 0.75,
            ),
            (
                toe_out(1.0) * rz(6.0) * Vec3::X,
                toe_out(1.0) * rz(6.0) * Vec3::Y,
                0.0,
            ),
        ],
        head: Quat::from_rotation_y(-0.12) * Quat::from_rotation_z(-0.14),
    };
    let tone = Vec3::new(1.04, 0.98, 0.95);
    let mut kit = Kit::default();
    let skin = kit.mesh(Surface::Skin);
    // Only what the clothes leave bare: neck and head, forearms and hands, shins and feet.
    torso_skin(skin, 1.38, tone);
    for (k, side) in SIDES.into_iter().enumerate() {
        arm_skin(skin, &pose, k, 0.3, tone);
        leg_skin(skin, &pose, k, 0.72, tone);
        let arm = pose.arm(k);
        let (hd, hp, curl, spread) = pose.hands[k];
        hand(skin, arm.joints[2], hd, hp, side, curl, spread, tone);
        let leg = pose.leg(k);
        let (fd, fu, bend) = pose.feet[k];
        foot(skin, leg.joints[2], fd, fu, side, bend, tone);
        ear(skin, &pose, side, tone);
    }
    let brown = Vec3::new(0.16, 0.1, 0.065);
    let hairline = |ph: f32| {
        let rows = [
            (0.0, [0.92]),
            (0.55, [1.02]),
            (1.05, [1.5]),
            (1.3, [1.62]),
            (1.5, [1.3]),
            (1.9, [1.34]),
            (2.5, [1.98]),
            (PI, [2.08]),
        ];
        table(&rows, ph.abs())[0]
            + 0.07 * fbm3(Vec3::new(ph.cos() * 5.0, 0.0, ph.sin() * 5.0), 0x4a0, 3)
    };
    let face = Face {
        tone,
        stubble: 0.3,
        brow: brown * 0.7,
        hair: Some((&hairline, brown)),
    };
    head_skin(skin, &pose, &face, 0.85);
    let mut eyeballs = Mesh::default();
    eyes(
        &mut eyeballs,
        &pose,
        Vec3::new(1.0, -0.12, 0.0),
        Vec3::new(0.3, 0.2, 0.1),
    );
    kit.add(Surface::Plastic, &eyeballs);

    let fabric = kit.mesh(Surface::Fabric);
    // Short dark-brown hair, thicker on top, with combed streaks.
    // A ragged hairline, a fuller top swept back, clumps and combed streaks.
    scalp_shell(
        fabric,
        &pose,
        &hairline,
        &|t, p| {
            let clumps = fbm3(Vec3::new(p.x * 40.0, p.y * 25.0, p.z * 40.0), 0x4a1, 2);
            (0.012 - 0.008 * t + 0.004 * smooth(0.0, 0.05, p.x) * (1.0 - t)) * (1.0 + 0.35 * clumps)
        },
        &|p, t| {
            let streak = fbm3(Vec3::new(p.x * 20.0, p.y * 160.0, p.z * 160.0), 0x4a2, 2);
            rgba(brown * (0.7 + 0.6 * streak) * (1.0 + 0.35 * smooth(0.75, 1.0, t)))
        },
    );
    // T-shirt: a crew neck, loose over the belly, short sleeves, rolled hems.
    let red = Vec3::new(1.05, 0.2, 0.17);
    let shirt_col = move |p: Vec3, wrinkle: f32| {
        rgba(red * (0.9 + 0.12 * wrinkle) * (0.95 + 0.05 * fbm3(p * 20.0, 0x7ee, 2)))
    };
    let neck_y = |th: f32| 1.478 - 0.032 * th.cos().max(0.0).powi(2);
    let shirt_off = |y: f32, th: f32| {
        let wr = fbm3(Vec3::new(th.cos() * 2.0, y * 9.0, th.sin() * 2.0), 0x5a1, 2);
        let n = neck_y(th);
        // Out over the shoulders to drape onto the sleeves.
        let a = ((th + PI).rem_euclid(TAU) - PI).abs();
        let shoulder = 0.04 * bell(a - FRAC_PI_2, 0.45) * bell(y - 1.405, 0.035);
        let tuck = smooth(n - 0.03, n - 0.006, y);
        (0.01 + 0.014 * smooth(1.2, 0.95, y) + 0.004 * wr + shoulder) * (1.0 - tuck) - 0.008 * tuck
    };
    torso_shell(fabric, 0.9, &neck_y, 18, 0.35, &shirt_off, &|p, th| {
        shirt_col(
            p,
            fbm3(
                Vec3::new(th.cos() * 2.0, p.y * 9.0, th.sin() * 2.0),
                0x5a1,
                2,
            ),
        )
    });
    let hem_ring: Vec<Vec3> = (0..36)
        .map(|i| {
            let th = i as f32 / 36.0 * TAU;
            let (r, cx) = torso_radius(0.9, th, 0.35);
            let r = r + shirt_off(0.9, th) - 0.002;
            Vec3::new(cx + r * th.cos(), 0.9, -r * th.sin())
        })
        .collect();
    hem(fabric, &hem_ring, 0.004, shirt_col(Vec3::ZERO, -0.3));
    let collar: Vec<Vec3> = (0..36)
        .map(|i| {
            let th = i as f32 / 36.0 * TAU;
            let y = neck_y(th) - 0.012;
            let (r, cx) = torso_radius(y, th, 1.0);
            Vec3::new(cx + (r + 0.003) * th.cos(), y, -(r + 0.003) * th.sin())
        })
        .collect();
    hem(fabric, &collar, 0.0055, shirt_col(Vec3::ZERO, -0.2));
    for (k, side) in SIDES.into_iter().enumerate() {
        let chain = pose.arm(k);
        let r = move |t: f32, phi: f32| {
            (limb_radius(&ARM, t.min(0.3), phi, side) + 0.006 + 0.012 * smooth(0.05, 0.42, t))
                * dome(t, -0.02, 2.0, 0.12, 1.0)
        };
        loft(
            fabric,
            &chain,
            &steps(-0.02, 0.42, 7),
            16,
            &r,
            &|p, _, _| shirt_col(p, 0.0),
        );
        hem(
            fabric,
            &ring_at(&chain, 0.415, 16, &r),
            0.004,
            shirt_col(Vec3::ZERO, -0.3),
        );
    }
    // Board shorts to just above the knee: teal, a pale band down each side.
    let teal = Vec3::new(0.1, 0.42, 0.5);
    torso_shell(
        fabric,
        0.795,
        &|_| 1.02,
        10,
        0.4,
        &|y, _| 0.016 + 0.004 * smooth(0.98, 1.0, y),
        &|p, th| {
            let band = smooth(0.975, 0.99, th.sin().abs()) * smooth(0.9, 0.85, p.y);
            rgba(teal.lerp(Vec3::new(0.85, 0.85, 0.75), band))
        },
    );
    for (k, side) in SIDES.into_iter().enumerate() {
        let chain = pose.leg(k);
        let r = move |t: f32, phi: f32| {
            let fold = 0.004 * (phi * 3.0 + t * 4.0 + side).sin();
            (limb_radius(&LEG, t.min(0.2), phi, side) + 0.02 + 0.014 * smooth(0.2, 0.85, t) + fold)
                * dome(t, -0.15, 2.0, 0.15, 1.0)
        };
        let col = move |_: Vec3, _: f32, phi: f32| {
            rgba(teal.lerp(
                Vec3::new(0.85, 0.85, 0.75),
                smooth(0.975, 0.99, phi.sin() * side),
            ))
        };
        loft(fabric, &chain, &steps(-0.15, 0.86, 10), 18, &r, &col);
        hem(
            fabric,
            &ring_at(&chain, 0.855, 18, &r),
            0.005,
            rgba(teal * 0.8),
        );
    }
    settle(&mut kit, Mat4::IDENTITY, true);
    kit.finish()
}

/// A deep concrete pool sunk into the ground, the paving's top at y = `paving`: water
/// `half` meters each way from the center, walls `wall` thick, a floor `depth` below the
/// ground, and `apron` meters of paving round it.
pub fn pool_liner(half: f32, wall: f32, apron: f32, depth: f32, paving: f32) -> Vec<Part> {
    let mut m = Mesh::default();
    let pale = Vec4::new(0.86, 0.85, 0.81, 1.0);
    let c = 0.012;
    let outer = half + wall;
    let slab = 0.2;
    // The floor, its top `depth` down.
    chamfer_box(
        &mut m,
        Vec3::new(outer, slab * 0.5, outer),
        c,
        Mat4::from_translation(Vec3::new(0.0, -depth - slab * 0.5, 0.0)),
        pale,
    );
    // Walls from under the floor to the paving's top.
    let (lo, hi) = (-depth - slab, paving);
    let mid = (lo + hi) * 0.5;
    let h = (hi - lo) * 0.5;
    for s in [-1.0f32, 1.0] {
        chamfer_box(
            &mut m,
            Vec3::new(wall * 0.5, h, outer),
            c,
            Mat4::from_translation(Vec3::new(s * (half + wall * 0.5), mid, 0.0)),
            pale,
        );
        chamfer_box(
            &mut m,
            Vec3::new(half, h, wall * 0.5),
            c,
            Mat4::from_translation(Vec3::new(0.0, mid, s * (half + wall * 0.5))),
            pale,
        );
    }
    // Paving slabs round the top, a shade darker and weathered.
    let paver = Vec4::new(0.72, 0.71, 0.67, 1.0);
    let t = 0.12;
    for s in [-1.0f32, 1.0] {
        chamfer_box(
            &mut m,
            Vec3::new(apron * 0.5, t * 0.5, outer + apron),
            c,
            Mat4::from_translation(Vec3::new(s * (outer + apron * 0.5), paving - t * 0.5, 0.0)),
            paver,
        );
        chamfer_box(
            &mut m,
            Vec3::new(outer, t * 0.5, apron * 0.5),
            c,
            Mat4::from_translation(Vec3::new(0.0, paving - t * 0.5, s * (outer + apron * 0.5))),
            paver,
        );
    }
    vec![Part::new(m, Surface::Concrete)]
}

/// A cube of edge `size` centered on the origin with chamfered edges and per-face UVs in
/// meters (the density test blocks).
pub fn block(size: f32) -> Mesh {
    let mut m = Mesh::default();
    let c = (size * 0.03).min(0.015);
    chamfer_box(
        &mut m,
        Vec3::splat(size * 0.5),
        c,
        Mat4::IDENTITY,
        Vec4::ONE,
    );
    m
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Finite data, unit normals, whole triangles with indices in range.
    fn check_mesh(name: &str, m: &Mesh) {
        assert!(!m.indices.is_empty(), "{name}: empty mesh");
        assert_eq!(m.indices.len() % 3, 0, "{name}: partial triangle");
        assert!(
            m.indices.iter().all(|&i| (i as usize) < m.vertices.len()),
            "{name}: index out of range"
        );
        for v in &m.vertices {
            let finite = v
                .position
                .iter()
                .chain(&v.normal)
                .chain(&v.uv)
                .chain(&v.color);
            assert!(
                finite.copied().all(f32::is_finite),
                "{name}: non-finite vertex {v:?}"
            );
            let n = Vec3::from(v.normal).length();
            assert!((n - 1.0).abs() < 1e-3, "{name}: normal of length {n}");
        }
    }

    fn triangles(parts: &[Part]) -> usize {
        parts.iter().map(|p| p.mesh.triangle_count()).sum()
    }

    fn bounds(parts: &[Part]) -> (Vec3, Vec3) {
        let points = parts
            .iter()
            .flat_map(|p| &p.mesh.vertices)
            .map(|v| Vec3::from(v.position));
        points.fold((Vec3::MAX, Vec3::MIN), |(lo, hi), p| (lo.min(p), hi.max(p)))
    }

    /// Signed volume of the parts together, about their vertex centroid (open parts that
    /// close each other off count together).
    fn volume(parts: &[Part]) -> f32 {
        let points: Vec<Vec3> = parts
            .iter()
            .flat_map(|p| &p.mesh.vertices)
            .map(|v| Vec3::from(v.position))
            .collect();
        let c = points.iter().sum::<Vec3>() / points.len() as f32;
        let mut v = 0.0;
        for m in parts.iter().map(|p| &p.mesh) {
            for t in m.indices.chunks(3) {
                let p = |k: usize| Vec3::from(m.vertices[t[k] as usize].position) - c;
                v += p(0).dot(p(1).cross(p(2))) / 6.0;
            }
        }
        v
    }

    /// The share of a mesh's (non-degenerate) triangles whose winding agrees with their
    /// vertex normals.
    fn winding_agreement(m: &Mesh) -> f32 {
        let (mut good, mut all) = (0, 0);
        for t in m.indices.chunks(3) {
            let v = |k: usize| &m.vertices[t[k] as usize];
            let p = |k: usize| Vec3::from(v(k).position);
            let face = (p(1) - p(0)).cross(p(2) - p(0));
            if face.length_squared() < 1e-14 {
                continue;
            }
            let n: Vec3 = (0..3).map(|k| Vec3::from(v(k).normal)).sum();
            all += 1;
            good += (face.dot(n) > 0.0) as usize;
        }
        good as f32 / all.max(1) as f32
    }

    /// Every part is sound and wound to face along its normals, the whole within its budget
    /// and the expected box (to 10 % of the box's extent plus 3 cm).
    fn check_prop(name: &str, parts: &[Part], budget: usize, lo: [f32; 3], hi: [f32; 3]) {
        assert!(!parts.is_empty(), "{name}: no parts");
        for p in parts {
            let part = format!("{name} {:?}", p.surface);
            check_mesh(&part, &p.mesh);
            let w = winding_agreement(&p.mesh);
            assert!(
                w >= 0.98,
                "{part}: only {:.1} % of the triangles face outward",
                w * 100.0
            );
        }
        let tris = triangles(parts);
        assert!(tris <= budget, "{name}: {tris} triangles, budget {budget}");
        let (got_lo, got_hi) = bounds(parts);
        let (lo, hi) = (Vec3::from(lo), Vec3::from(hi));
        let tolerance = (hi - lo) * 0.1 + Vec3::splat(0.03);
        let off = (got_lo - lo).abs().max((got_hi - hi).abs());
        assert!(
            off.cmple(tolerance).all(),
            "{name}: bounds {got_lo:.3} .. {got_hi:.3}, expected {lo} .. {hi}"
        );
    }

    fn closed(name: &str, parts: &[Part]) -> f32 {
        let v = volume(parts);
        assert!(v > 0.0, "{name}: signed volume {v} (inside out or open)");
        v
    }

    fn same(a: &[Part], b: &[Part]) -> bool {
        a.len() == b.len()
            && a.iter().zip(b).all(|(a, b)| {
                a.mesh.indices == b.mesh.indices
                    && a.mesh.vertices.len() == b.mesh.vertices.len()
                    && a.mesh.vertices.iter().zip(&b.mesh.vertices).all(|(u, v)| {
                        u.position == v.position && u.normal == v.normal && u.color == v.color
                    })
            })
    }

    #[test]
    fn boulders_are_about_their_size_bedded_and_closed() {
        for (seed, size) in [(1u64, 0.8f32), (2, 1.3), (3, 2.0), (4, 1.0)] {
            let name = format!("boulder {seed} {size}");
            let parts = boulder(seed, size);
            let budget = if size < 1.5 { 4_000 } else { 10_000 };
            let h = size * 0.5;
            // Its depth (z) is 70 to 100 % of its length (x).
            let d = size * 0.425;
            check_prop(
                &name,
                &parts,
                budget,
                [-h, -0.15 * size, -d],
                [h, 0.35 * size, d],
            );
            let (lo, hi) = bounds(&parts);
            let across = (hi - lo).x.max((hi - lo).z);
            assert!(
                (0.85..1.15).contains(&(across / size)),
                "{name}: {across} m across"
            );
            assert!(
                (lo.y + 0.15 * size).abs() < 0.03 * size,
                "{name}: bed at {}",
                lo.y
            );
            let v = closed(&name, &parts);
            assert!(v > 0.1 * size.powi(3), "{name}: volume {v}");
        }
    }

    #[test]
    fn boats_fit_their_frames() {
        let row = rowboat();
        check_prop(
            "rowboat",
            &row,
            30_000,
            [-1.8, -0.31, -0.675],
            [1.8, 0.46, 0.675],
        );
        closed("rowboat", &row);
        // The outboard's cowling and skeg stand out behind and below the 5.2 m hull.
        let motor = motorboat();
        check_prop(
            "motorboat",
            &motor,
            50_000,
            [-3.1, -0.72, -1.0],
            [2.6, 0.96, 1.0],
        );
        closed("motorboat", &motor);
    }

    #[test]
    fn jetty_parts_fit_their_frames() {
        for seed in 0..3 {
            let deck = deck_section(seed);
            check_prop(
                "deck_section",
                &deck,
                2_000,
                [-0.275, -0.21, -1.0],
                [0.275, 0.035, 1.0],
            );
            closed("deck_section", &deck);
            let pile = post(1.35, 0.55, seed);
            check_prop(
                "post",
                &pile,
                2_000,
                [-0.11, 0.0, -0.11],
                [0.11, 1.35, 0.11],
            );
            closed("post", &pile);
        }
        let c = cleat();
        check_prop(
            "cleat",
            &c,
            4_000,
            [-0.125, 0.0, -0.03],
            [0.125, 0.06, 0.03],
        );
        closed("cleat", &c);
        let l = ladder(1.1);
        check_prop("ladder", &l, 4_000, [-0.41, -1.1, -0.3], [0.1, 0.78, 0.3]);
        closed("ladder", &l);
    }

    #[test]
    fn floating_things_fit_their_frames() {
        for seed in 0..2 {
            let b = buoy(seed);
            check_prop("buoy", &b, 6_000, [-0.34, -0.55, -0.34], [0.34, 0.9, 0.34]);
            closed("buoy", &b);
        }
        for seed in 0..6 {
            let l = log(1.7, 0.17, seed);
            check_prop("log", &l, 6_000, [-0.85, -0.2, -0.2], [0.85, 0.2, 0.2]);
            closed("log", &l);
        }
    }

    #[test]
    fn the_culvert_fits_its_frame() {
        let c = culvert();
        check_prop(
            "culvert",
            &c,
            25_000,
            [-2.0, -0.63, -2.34],
            [1.0, 0.62, 2.34],
        );
        let concrete: Vec<Part> = c
            .into_iter()
            .filter(|p| p.surface == Surface::Concrete)
            .collect();
        closed("culvert headwall", &concrete);
    }

    #[test]
    fn people_are_life_size_and_closed() {
        let s = swimmer();
        check_prop(
            "swimmer",
            &s,
            15_000,
            [-1.13, -0.32, -0.27],
            [1.13, 0.32, 0.27],
        );
        let w = wader();
        check_prop("wader", &w, 15_000, [-0.49, 0.0, -0.41], [0.49, 1.77, 0.41]);
        for (name, parts) in [("swimmer", s), ("wader", w)] {
            assert!(
                parts.iter().any(|p| p.surface == Surface::Fabric),
                "{name}: no clothes"
            );
            let skin: Vec<Part> = parts
                .into_iter()
                .filter(|p| p.surface == Surface::Skin)
                .collect();
            let v = closed(name, &skin);
            // The skin under the clothes is left out; what remains is a few liters.
            assert!(v > 0.005, "{name}: skin volume {v}");
        }
    }

    #[test]
    fn the_block_is_closed_and_outward() {
        let m = block(0.5);
        let parts = [Part::new(m, Surface::Cork)];
        check_prop("block", &parts, 100, [-0.25; 3], [0.25; 3]);
        let v = volume(&parts);
        assert!((v - 0.125).abs() < 0.005, "volume {v}");
    }

    #[test]
    fn seeds_vary_the_props_and_repeat_exactly() {
        type Build = dyn Fn(u64) -> Vec<Part>;
        let pairs: [(&str, &Build); 5] = [
            ("boulder", &|s| boulder(s, 0.7)),
            ("deck_section", &deck_section),
            ("post", &|s| post(1.35, 0.55, s)),
            ("buoy", &buoy),
            ("log", &|s| log(1.7, 0.17, s)),
        ];
        for (name, build) in pairs {
            assert!(same(&build(3), &build(3)), "{name}: one seed, two meshes");
            assert!(
                !same(&build(3), &build(4)),
                "{name}: seeds 3 and 4 are alike"
            );
        }
    }
}
