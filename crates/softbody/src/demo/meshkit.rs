//! Procedural mesh toolkit for the yard's props: rounded boxes, lathed (turned) profiles,
//! tubes along paths, chains, extruded and beveled profiles, hex nuts, plus deformers that
//! make hand-built things look less perfect (bow, twist, jitter) and vertex-color grime.
//!
//! Every builder returns an engine [`Mesh`] with smooth normals where the surface is smooth,
//! counter-clockwise front faces and texture coordinates in meters (except [`BoxUv::Wood`],
//! which maps straight into the wood atlas made by `texgen::wood`). Units: meters, Y up.

use std::f32::consts::{FRAC_PI_4, TAU};

use engine_core::math::{Mat4, Quat, Vec2, Vec3, Vec4};
use engine_render::{Mesh, Vertex};

use super::texgen::noise3;

/// Wood atlas layout (shared with `texgen::wood`): long grain fills `u < WOOD_GRAIN_U` and
/// repeats along v; end grain sits in the square `u >= WOOD_GRAIN_U, v < 1 - WOOD_GRAIN_U`.
pub const WOOD_GRAIN_U: f32 = 0.75;
/// Meters of wood across the long-grain region.
pub const WOOD_GRAIN_W_M: f32 = 0.36;
/// Meters of wood along the grain per texture repeat.
pub const WOOD_GRAIN_H_M: f32 = 2.4;
/// Meters across the end-grain square.
pub const WOOD_END_M: f32 = 0.3;

/// How a box's faces get texture coordinates.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum BoxUv {
    /// Meters, projected onto each face; `offset` shifts the pattern.
    Planar { offset: Vec2 },
    /// Wood atlas: grain runs along the box's longest axis, end faces get end grain.
    /// `offset.x` picks a strip across the grain (meters, 0..`WOOD_GRAIN_W_M` − width),
    /// `offset.y` slides along it; `flip` mirrors the strip.
    Wood { offset: Vec2, flip: bool },
}

impl BoxUv {
    /// Planar meter UVs with no offset.
    pub const PLANAR: BoxUv = BoxUv::Planar { offset: Vec2::ZERO };
}

/// Atlas coordinates for long grain: `across` meters across the board (0 at one edge),
/// `along` meters along the grain.
pub fn wood_uv(across: f32, along: f32, width: f32, offset: Vec2, flip: bool) -> Vec2 {
    let w = width.max(1e-4);
    let fit = (WOOD_GRAIN_W_M * 0.98 / w).min(1.0);
    let a = if flip { w - across } else { across } * fit;
    let x0 = offset
        .x
        .clamp(0.0, (WOOD_GRAIN_W_M * 0.99 - w * fit).max(0.0));
    Vec2::new(
        (x0 + a) / WOOD_GRAIN_W_M * WOOD_GRAIN_U,
        (offset.y + along) / WOOD_GRAIN_H_M,
    )
}

/// Atlas coordinates for end grain at `(a, b)` meters from the face center.
pub fn wood_end_uv(a: f32, b: f32) -> Vec2 {
    let s = 1.0 - WOOD_GRAIN_U;
    let lo = 0.004;
    Vec2::new(
        WOOD_GRAIN_U + ((0.5 + a / WOOD_END_M) * s).clamp(lo, s - lo),
        ((0.5 + b / WOOD_END_M) * s).clamp(lo, s - lo),
    )
}

fn push(m: &mut Mesh, p: Vec3, n: Vec3, uv: Vec2) -> u32 {
    m.vertices.push(Vertex::new(p, n, uv));
    m.vertices.len() as u32 - 1
}

/// Append a triangle, flipped if needed so its face normal agrees with `n`.
fn tri_facing(m: &mut Mesh, a: u32, b: u32, c: u32, n: Vec3) {
    let p = |i: u32| Vec3::from(m.vertices[i as usize].position);
    let g = (p(b) - p(a)).cross(p(c) - p(a));
    if g.dot(n) >= 0.0 {
        m.indices.extend_from_slice(&[a, b, c]);
    } else {
        m.indices.extend_from_slice(&[a, c, b]);
    }
}

/// Coordinates along one box axis: rounded part (angle-spaced so each face covers 45° of the
/// edge arc), then the flat part split into pieces no longer than `max_edge`.
fn box_axis(h: f32, r: f32, segs: u32, max_edge: f32) -> Vec<f32> {
    let inner = h - r;
    let mut c = Vec::new();
    if r > 0.0 {
        for k in (1..=segs).rev() {
            c.push(-inner - r * (k as f32 / segs as f32 * FRAC_PI_4).tan());
        }
    }
    if inner <= 1e-6 {
        c.push(0.0);
    } else {
        let n = ((2.0 * inner) / max_edge.max(1e-3)).ceil().max(1.0) as usize;
        for i in 0..=n {
            c.push(-inner + 2.0 * inner * i as f32 / n as f32);
        }
    }
    if r > 0.0 {
        for k in 1..=segs {
            c.push(inner + r * (k as f32 / segs as f32 * FRAC_PI_4).tan());
        }
    }
    c
}

/// A box centered on the origin with edges and corners rounded to `radius` (smooth normals,
/// `segs` rows per 45° of arc), flat parts split so no edge is longer than `max_edge`
/// (so deformers and vertex colors have vertices to work with).
pub fn rounded_box(size: Vec3, radius: f32, segs: u32, max_edge: f32, uv: BoxUv) -> Mesh {
    let h = size.abs() * 0.5;
    let r = radius.clamp(0.0, h.min_element() * 0.999);
    let segs = segs.max(1);
    let inner = h - Vec3::splat(r);
    let axes = [
        box_axis(h.x, r, segs, max_edge),
        box_axis(h.y, r, segs, max_edge),
        box_axis(h.z, r, segs, max_edge),
    ];
    let long = if h.x >= h.y && h.x >= h.z {
        0
    } else if h.y >= h.z {
        1
    } else {
        2
    };
    // (normal axis, sign, tangent 1 axis and sign, tangent 2 axis and sign); t1 × t2 = n.
    let faces: [(usize, f32, usize, f32, usize, f32); 6] = [
        (0, 1.0, 2, -1.0, 1, 1.0),
        (0, -1.0, 2, 1.0, 1, 1.0),
        (2, 1.0, 0, 1.0, 1, 1.0),
        (2, -1.0, 0, -1.0, 1, 1.0),
        (1, 1.0, 0, 1.0, 2, -1.0),
        (1, -1.0, 0, 1.0, 2, 1.0),
    ];
    let mut m = Mesh::default();
    for (a, sa, b, sb, c, sc) in faces {
        let base = m.vertices.len() as u32;
        let (cb, cc) = (&axes[b], &axes[c]);
        for &y in cc {
            for &x in cb {
                let mut p = Vec3::ZERO;
                p[a] = sa * h[a];
                p[b] = sb * x;
                p[c] = sc * y;
                let q = p.clamp(-inner, inner);
                let mut fnorm = Vec3::ZERO;
                fnorm[a] = sa;
                let n = (p - q).normalize_or(fnorm);
                let pos = q + n * r;
                let tex = match uv {
                    BoxUv::Planar { offset } => {
                        let mut t1 = Vec3::ZERO;
                        t1[b] = sb;
                        let mut t2 = Vec3::ZERO;
                        t2[c] = sc;
                        Vec2::new(pos.dot(t1), -pos.dot(t2)) + offset
                    }
                    BoxUv::Wood { offset, flip } => {
                        if a == long {
                            wood_end_uv(pos[b], pos[c])
                        } else {
                            let o = 3 - a - long;
                            wood_uv(pos[o] + h[o], pos[long] + h[long], 2.0 * h[o], offset, flip)
                        }
                    }
                };
                push(&mut m, pos, n, tex);
            }
        }
        let nx = cb.len() as u32;
        let ny = cc.len() as u32;
        for j in 0..ny - 1 {
            for i in 0..nx - 1 {
                let v00 = base + j * nx + i;
                let v10 = v00 + 1;
                let v01 = v00 + nx;
                let v11 = v01 + 1;
                m.indices.extend_from_slice(&[v00, v10, v11, v00, v11, v01]);
            }
        }
    }
    m
}

/// A plank or beam of `size` (centered) with `bevel`ed edges and wood UVs (see
/// [`BoxUv::Wood`]); long faces get enough segments to bend smoothly (see [`warp`]).
pub fn timber(size: Vec3, bevel: f32, offset: Vec2, flip: bool) -> Mesh {
    let segs = if size.min_element() < 0.07 { 1 } else { 2 };
    let max_edge = (size.max_element() / 8.0).max(0.12);
    rounded_box(size, bevel, segs, max_edge, BoxUv::Wood { offset, flip })
}

/// Texture mapping for [`lathe`]: `u_span` is the u at the end of one turn, `v_per_m` scales
/// the distance along the profile.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LatheUv {
    pub u_span: f32,
    pub v_per_m: f32,
    pub offset: Vec2,
}

impl LatheUv {
    /// Meters around a circle of radius `r` and meters along the profile.
    pub fn meters(r: f32) -> Self {
        Self {
            u_span: TAU * r,
            v_per_m: 1.0,
            offset: Vec2::ZERO,
        }
    }

    /// Wood atlas long grain along the profile, wrapped around a circle of radius `r`.
    pub fn wood(r: f32, offset: Vec2) -> Self {
        let circ = (TAU * r).min(WOOD_GRAIN_W_M * 0.95);
        Self {
            u_span: circ / WOOD_GRAIN_W_M * WOOD_GRAIN_U,
            v_per_m: 1.0 / WOOD_GRAIN_H_M,
            offset: Vec2::new(
                offset.x.clamp(0.0, WOOD_GRAIN_W_M - circ) / WOOD_GRAIN_W_M * WOOD_GRAIN_U,
                offset.y / WOOD_GRAIN_H_M,
            ),
        }
    }
}

/// Revolve a profile of `(radius, y)` points around +Y. List the profile from the bottom up
/// with the outside on the right (start and end at radius 0 to close it). Repeat a point to
/// put a hard edge there; everywhere else the normals are smooth.
pub fn lathe(profile: &[Vec2], segments: u32, uv: LatheUv) -> Mesh {
    let segments = segments.max(3);
    let n = profile.len();
    let seg_normal = |p: Vec2, q: Vec2| {
        let d = q - p;
        Vec2::new(d.y, -d.x).normalize_or(Vec2::X)
    };
    let mut normals = Vec::with_capacity(n);
    let mut arc = Vec::with_capacity(n);
    let mut s = 0.0;
    for i in 0..n {
        if i > 0 {
            s += profile[i].distance(profile[i - 1]);
        }
        arc.push(s);
        let prev =
            (i > 0 && profile[i] != profile[i - 1]).then(|| seg_normal(profile[i - 1], profile[i]));
        let next = (i + 1 < n && profile[i + 1] != profile[i])
            .then(|| seg_normal(profile[i], profile[i + 1]));
        normals.push(match (prev, next) {
            (Some(a), Some(b)) => (a + b).normalize_or(a),
            (Some(a), None) | (None, Some(a)) => a,
            (None, None) => Vec2::X,
        });
    }
    let mut m = Mesh::default();
    let row = segments + 1;
    for i in 0..n {
        let (p, nn) = (profile[i], normals[i]);
        for k in 0..=segments {
            let t = k as f32 / segments as f32;
            let (sin, cos) = (t * TAU).sin_cos();
            let dir = Vec3::new(cos, 0.0, -sin);
            push(
                &mut m,
                dir * p.x + Vec3::Y * p.y,
                (dir * nn.x + Vec3::Y * nn.y).normalize_or(Vec3::Y),
                Vec2::new(t * uv.u_span, -arc[i] * uv.v_per_m) + uv.offset,
            );
        }
    }
    for i in 0..n.saturating_sub(1) as u32 {
        if profile[i as usize] == profile[i as usize + 1] {
            continue;
        }
        for k in 0..segments {
            let a = i * row + k;
            let b = a + 1;
            let c = a + row;
            let d = c + 1;
            m.indices.extend_from_slice(&[a, b, c, c, b, d]);
        }
    }
    m
}

/// A sphere (lathed, so UVs follow [`LatheUv`]).
pub fn sphere(radius: f32, segments: u32) -> Mesh {
    let rings = (segments / 2).max(3);
    let profile: Vec<Vec2> = (0..=rings)
        .map(|i| {
            let a = -std::f32::consts::FRAC_PI_2 + std::f32::consts::PI * i as f32 / rings as f32;
            Vec2::new(a.cos().max(0.0) * radius, a.sin() * radius)
        })
        .collect();
    lathe(&profile, segments, LatheUv::meters(radius))
}

/// Sweep a circle along `path` (parallel-transported frame). `radius(t)` gives the radius at
/// `t` = 0..1 along the path. `closed` joins the ends into a loop; `caps` closes open ends.
/// UVs: u = 0..1 around, v = meters along / `v_per_m`⁻¹.
pub fn tube(
    path: &[Vec3],
    radius: impl Fn(f32) -> f32,
    sides: u32,
    closed: bool,
    caps: bool,
    v_per_m: f32,
) -> Mesh {
    let mut m = Mesh::default();
    let n = path.len();
    if n < 2 {
        return m;
    }
    let sides = sides.max(3);
    let tangent = |i: usize| -> Vec3 {
        let (a, b) = if closed {
            (path[(i + n - 1) % n], path[(i + 1) % n])
        } else {
            (path[i.saturating_sub(1)], path[(i + 1).min(n - 1)])
        };
        (b - a).normalize_or(Vec3::Y)
    };
    let mut len = vec![0.0f32; n + 1];
    for i in 1..=n {
        let seg = if i == n {
            if closed {
                path[0].distance(path[n - 1])
            } else {
                0.0
            }
        } else {
            path[i].distance(path[i - 1])
        };
        len[i] = len[i - 1] + seg;
    }
    let total = if closed { len[n] } else { len[n - 1] }.max(1e-6);
    let count = if closed { n + 1 } else { n };
    let t0 = tangent(0);
    let mut normal = t0.any_orthonormal_vector();
    if t0.dot(Vec3::Y).abs() < 0.9 {
        normal = t0.cross(Vec3::Y).cross(t0).normalize_or(normal);
    }
    let mut prev_t = t0;
    let row = sides + 1;
    for (i, &along) in len.iter().enumerate().take(count) {
        let pi = i % n;
        let t = tangent(pi);
        normal = (Quat::from_rotation_arc(prev_t, t) * normal).normalize_or(normal);
        prev_t = t;
        let binormal = t.cross(normal);
        let f = along / total;
        let r = radius(f);
        let dr = (radius((f + 0.01).min(1.0)) - radius((f - 0.01).max(0.0))) / (0.02 * total);
        for k in 0..=sides {
            let a = k as f32 / sides as f32 * TAU;
            let dir = normal * a.cos() + binormal * a.sin();
            push(
                &mut m,
                path[pi] + dir * r,
                (dir - t * dr).normalize_or(dir),
                Vec2::new(k as f32 / sides as f32, -len[i] * v_per_m),
            );
        }
    }
    for i in 0..(count as u32 - 1) {
        for k in 0..sides {
            let a = i * row + k;
            let b = a + 1;
            let c = a + row;
            let d = c + 1;
            m.indices.extend_from_slice(&[a, b, c, c, b, d]);
        }
    }
    if caps && !closed {
        for (i, sign) in [(0usize, -1.0f32), (n - 1, 1.0)] {
            let t = tangent(i) * sign;
            let ring = if i == 0 { 0 } else { (count as u32 - 1) * row };
            let r = radius(if i == 0 { 0.0 } else { 1.0 });
            let center = push(&mut m, path[i], t, Vec2::splat(0.5));
            let first = m.vertices.len() as u32;
            for k in 0..=sides {
                let v = m.vertices[(ring + k) as usize];
                let p = Vec3::from(v.position);
                let d = (p - path[i]) / r.max(1e-6);
                push(&mut m, p, t, Vec2::new(0.5 + d.x * 0.5, 0.5 + d.z * 0.5));
            }
            for k in 0..sides {
                tri_facing(&mut m, center, first + k, first + k + 1, t);
            }
        }
    }
    m
}

/// A torus in the XZ plane (around +Y).
pub fn torus(major: f32, minor: f32, segments: u32, sides: u32) -> Mesh {
    let path: Vec<Vec3> = (0..segments.max(3))
        .map(|i| {
            let a = i as f32 / segments.max(3) as f32 * TAU;
            Vec3::new(a.cos() * major, 0.0, -a.sin() * major)
        })
        .collect();
    tube(&path, |_| minor, sides, true, false, 1.0)
}

/// One chain link (a stadium-shaped loop of wire) in the XY plane, long along Y, centered.
/// `length` and `width` are outside dimensions.
pub fn chain_link(length: f32, width: f32, wire: f32) -> Mesh {
    let r = (width * 0.5 - wire).max(wire * 0.5);
    let straight = (length * 0.5 - wire - r).max(0.0);
    let mut path = Vec::new();
    let arc = 8;
    for (cy, a0) in [(straight, 0.0f32), (-straight, std::f32::consts::PI)] {
        for k in 0..=arc {
            let a = a0 + std::f32::consts::PI * k as f32 / arc as f32;
            path.push(Vec3::new(a.cos() * r, cy + a.sin() * r, 0.0));
        }
    }
    tube(&path, |_| wire, 8, true, false, 1.0)
}

/// A chain hanging along `points` (a polyline), links alternating 90°.
pub fn chain(points: &[Vec3], link_len: f32, wire: f32) -> Mesh {
    let mut m = Mesh::default();
    let pitch = link_len - 2.0 * wire;
    let link = chain_link(link_len, link_len * 0.62, wire);
    let mut k = 0;
    for w in points.windows(2) {
        let (a, b) = (w[0], w[1]);
        let d = b - a;
        let l = d.length();
        if l < 1e-5 {
            continue;
        }
        let dir = d / l;
        let count = (l / pitch).floor().max(1.0) as usize;
        for i in 0..count {
            let c = a + dir * (pitch * (i as f32 + 0.5));
            let q = Quat::from_rotation_arc(Vec3::Y, dir)
                * Quat::from_rotation_y(if k % 2 == 0 { 0.0 } else { TAU * 0.25 });
            m.merge(&xf(&link, Mat4::from_rotation_translation(q, c)));
            k += 1;
        }
    }
    m
}

/// Ear-clip a simple polygon (counter-clockwise) into triangles (indices into `poly`).
pub fn triangulate(poly: &[Vec2]) -> Vec<[usize; 3]> {
    let mut idx: Vec<usize> = (0..poly.len()).collect();
    let mut out = Vec::new();
    let cross = |a: Vec2, b: Vec2, c: Vec2| (b - a).perp_dot(c - a);
    let inside = |p: Vec2, a: Vec2, b: Vec2, c: Vec2| {
        cross(a, b, p) >= -1e-9 && cross(b, c, p) >= -1e-9 && cross(c, a, p) >= -1e-9
    };
    let mut guard = 0;
    while idx.len() > 3 && guard < 10_000 {
        guard += 1;
        let n = idx.len();
        let mut clipped = false;
        for i in 0..n {
            let (ia, ib, ic) = (idx[(i + n - 1) % n], idx[i], idx[(i + 1) % n]);
            let (a, b, c) = (poly[ia], poly[ib], poly[ic]);
            if cross(a, b, c) <= 1e-9 {
                continue;
            }
            let blocked = idx
                .iter()
                .any(|&j| j != ia && j != ib && j != ic && inside(poly[j], a, b, c));
            if !blocked {
                out.push([ia, ib, ic]);
                idx.remove(i);
                clipped = true;
                break;
            }
        }
        if !clipped {
            break;
        }
    }
    if idx.len() == 3 {
        out.push([idx[0], idx[1], idx[2]]);
    }
    out
}

/// Offset a counter-clockwise polygon inward by `d` (mitered corners).
pub fn inset(poly: &[Vec2], d: f32) -> Vec<Vec2> {
    let n = poly.len();
    (0..n)
        .map(|i| {
            let (p0, p1, p2) = (poly[(i + n - 1) % n], poly[i], poly[(i + 1) % n]);
            let e0 = (p1 - p0).normalize_or(Vec2::X);
            let e1 = (p2 - p1).normalize_or(Vec2::X);
            let n0 = Vec2::new(e0.y, -e0.x);
            let n1 = Vec2::new(e1.y, -e1.x);
            let m = (n0 + n1) / (1.0 + n0.dot(n1)).max(0.2);
            p1 - m * d
        })
        .collect()
}

/// Extrude a counter-clockwise polygon in the XY plane along Z (centered, `depth` thick),
/// with its front and back edges beveled by `bevel`. Corners sharper than `smooth_deg` get a
/// hard edge; gentler ones (arcs) are smooth. UVs: planar meters.
pub fn extrude(poly: &[Vec2], depth: f32, bevel: f32, smooth_deg: f32) -> Mesh {
    let n = poly.len();
    let mut m = Mesh::default();
    if n < 3 {
        return m;
    }
    let hz = depth * 0.5;
    let b = bevel.clamp(0.0, hz * 0.9);
    let outer = poly.to_vec();
    let inner = if b > 0.0 {
        inset(poly, b)
    } else {
        outer.clone()
    };
    let edge_n = |p: &[Vec2], i: usize| {
        let e = (p[(i + 1) % n] - p[i]).normalize_or(Vec2::X);
        Vec2::new(e.y, -e.x)
    };
    let cos_limit = smooth_deg.to_radians().cos();
    // Vertex normal for the end of edge `i` at vertex `v`.
    let vert_n = |i: usize, v: usize| {
        let own = edge_n(&outer, i);
        let other = if v == i {
            edge_n(&outer, (i + n - 1) % n)
        } else {
            edge_n(&outer, (i + 1) % n)
        };
        if own.dot(other) >= cos_limit {
            (own + other).normalize_or(own)
        } else {
            own
        }
    };
    // Rings from back to front: (polygon, z, normal z-tilt).
    let rings: Vec<(&[Vec2], f32, f32)> = if b > 0.0 {
        vec![
            (&inner, -hz, -1.0),
            (&outer, -hz + b, 0.0),
            (&outer, hz - b, 0.0),
            (&inner, hz, 1.0),
        ]
    } else {
        vec![(&outer, -hz, 0.0), (&outer, hz, 0.0)]
    };
    let mut perim = vec![0.0f32; n + 1];
    for i in 0..n {
        perim[i + 1] = perim[i] + outer[(i + 1) % n].distance(outer[i]);
    }
    for i in 0..n {
        let j = (i + 1) % n;
        let base = m.vertices.len() as u32;
        for (ring, z, tilt) in &rings {
            for (v, u) in [(i, perim[i]), (j, perim[i + 1])] {
                let nn = vert_n(i, v);
                let normal = if *tilt == 0.0 {
                    Vec3::new(nn.x, nn.y, 0.0)
                } else {
                    Vec3::new(nn.x, nn.y, *tilt).normalize()
                };
                push(
                    &mut m,
                    Vec3::new(ring[v].x, ring[v].y, *z),
                    normal,
                    Vec2::new(u, -z),
                );
            }
        }
        for r in 0..rings.len() as u32 - 1 {
            let a = base + r * 2;
            // a: ring r at i, a+1: ring r at j, a+2: ring r+1 at i, a+3: ring r+1 at j.
            let side = Vec3::new(edge_n(&outer, i).x, edge_n(&outer, i).y, 0.0);
            tri_facing(&mut m, a, a + 1, a + 3, side);
            tri_facing(&mut m, a, a + 3, a + 2, side);
        }
    }
    let tris = triangulate(&inner);
    for (z, nz) in [(hz, Vec3::Z), (-hz, Vec3::NEG_Z)] {
        let base = m.vertices.len() as u32;
        for p in &inner {
            push(&mut m, Vec3::new(p.x, p.y, z), nz, Vec2::new(p.x, -p.y));
        }
        for t in &tris {
            tri_facing(
                &mut m,
                base + t[0] as u32,
                base + t[1] as u32,
                base + t[2] as u32,
                nz,
            );
        }
    }
    m
}

/// A hex nut (or bolt head) with chamfered top, standing on y = 0, flat shaded.
pub fn hex_nut(across_flats: f32, height: f32) -> Mesh {
    let r = across_flats * 0.5 / (std::f32::consts::PI / 6.0).cos();
    let c = height * 0.25;
    let profile = [
        Vec2::new(0.0, 0.0),
        Vec2::new(r * 0.92, 0.0),
        Vec2::new(r, c * 0.5),
        Vec2::new(r, height - c),
        Vec2::new(r * 0.8, height),
        Vec2::new(0.0, height),
    ];
    let mut m = lathe(&profile, 6, LatheUv::meters(r));
    m.transform(&Mat4::from_rotation_y(TAU / 12.0));
    m.compute_flat_normals();
    m
}

/// A round-headed rivet or carriage bolt head (dome) standing on y = 0.
pub fn dome(radius: f32, height: f32, segments: u32) -> Mesh {
    let rings = 4;
    // The rim point appears twice (bottom disc, then the dome): a hard edge there.
    let mut profile = vec![Vec2::ZERO, Vec2::new(radius, 0.0)];
    for i in 0..=rings {
        let a = i as f32 / rings as f32 * std::f32::consts::FRAC_PI_2;
        profile.push(Vec2::new(a.cos() * radius, a.sin() * height));
    }
    lathe(&profile, segments, LatheUv::meters(radius))
}

/// A bolt with a washer and hex nut: the washer sits on y = 0, the threaded end pokes up.
pub fn bolt(size: f32) -> Mesh {
    let mut m = lathe(
        &[
            Vec2::new(0.0, 0.0),
            Vec2::new(size * 1.1, 0.0),
            Vec2::new(size * 1.1, size * 0.12),
            Vec2::new(size * 1.05, size * 0.16),
            Vec2::new(0.0, size * 0.16),
        ],
        12,
        LatheUv::meters(size),
    );
    m.merge(&xf(
        &hex_nut(size * 1.5, size * 0.8),
        Mat4::from_translation(Vec3::Y * size * 0.15),
    ));
    m.merge(&lathe(
        &[
            Vec2::new(0.0, size * 0.9),
            Vec2::new(size * 0.45, size * 0.9),
            Vec2::new(size * 0.45, size * 1.25),
            Vec2::new(size * 0.35, size * 1.35),
            Vec2::new(0.0, size * 1.35),
        ],
        8,
        LatheUv::meters(size),
    ));
    m
}

/// Round every corner of a counter-clockwise polygon with an arc of radius `radius(i)`
/// (`segs` segments; limited so arcs never overlap), so extrusions catch light on their edges.
pub fn round_corners(poly: &[Vec2], radius: impl Fn(usize) -> f32, segs: u32) -> Vec<Vec2> {
    let n = poly.len();
    let mut out = Vec::new();
    for i in 0..n {
        let (p0, p1, p2) = (poly[(i + n - 1) % n], poly[i], poly[(i + 1) % n]);
        let (l0, l1) = (p0.distance(p1), p2.distance(p1));
        let (e0, e1) = ((p0 - p1) / l0.max(1e-9), (p2 - p1) / l1.max(1e-9));
        let half = e0.dot(e1).clamp(-1.0, 1.0).acos() * 0.5;
        let r = radius(i);
        if r <= 0.0 || half < 1e-3 || half > std::f32::consts::FRAC_PI_2 - 1e-3 {
            out.push(p1);
            continue;
        }
        let t = (r / half.tan()).min(0.45 * l0.min(l1));
        let r = t * half.tan();
        let bis = (e0 + e1).normalize();
        let center = p1 + bis * (r / half.sin());
        let a = p1 + e0 * t - center;
        let b = p1 + e1 * t - center;
        let (a0, mut a1) = (a.y.atan2(a.x), b.y.atan2(b.x));
        let mut d = a1 - a0;
        if d > std::f32::consts::PI {
            d -= TAU;
        } else if d < -std::f32::consts::PI {
            d += TAU;
        }
        a1 = a0 + d;
        for k in 0..=segs {
            let ang = a0 + (a1 - a0) * k as f32 / segs as f32;
            out.push(center + Vec2::new(ang.cos(), ang.sin()) * r);
        }
    }
    out
}

/// A Catmull-Rom curve through `points`, `steps` samples per span.
pub fn smooth_path(points: &[Vec3], steps: u32) -> Vec<Vec3> {
    let n = points.len();
    if n < 3 {
        return points.to_vec();
    }
    let mut out = Vec::new();
    for i in 0..n - 1 {
        let p0 = points[i.saturating_sub(1)];
        let (p1, p2) = (points[i], points[i + 1]);
        let p3 = points[(i + 2).min(n - 1)];
        for s in 0..steps {
            let t = s as f32 / steps as f32;
            let (t2, t3) = (t * t, t * t * t);
            out.push(
                0.5 * (2.0 * p1
                    + (p2 - p0) * t
                    + (2.0 * p0 - 5.0 * p1 + 4.0 * p2 - p3) * t2
                    + (3.0 * p1 - p0 - 3.0 * p2 + p3) * t3),
            );
        }
    }
    out.push(points[n - 1]);
    out
}

/// Mirror a mesh across the YZ plane (x → −x), keeping faces front-facing.
pub fn mirror_x(mut mesh: Mesh) -> Mesh {
    for v in &mut mesh.vertices {
        v.position[0] = -v.position[0];
        v.normal[0] = -v.normal[0];
    }
    for t in mesh.indices.as_chunks_mut::<3>().0 {
        t.swap(1, 2);
    }
    mesh
}

/// A copy of `mesh` transformed by `t`.
pub fn xf(mesh: &Mesh, t: Mat4) -> Mesh {
    let mut m = mesh.clone();
    m.transform(&t);
    m
}

/// Place a mesh: rotation then translation.
pub fn place(mesh: Mesh, at: Vec3, rot: Quat) -> Mesh {
    xf(&mesh, Mat4::from_rotation_translation(rot, at))
}

/// Multiply every vertex color by `c`.
pub fn tint(mut mesh: Mesh, c: Vec4) -> Mesh {
    for v in &mut mesh.vertices {
        v.color = (Vec4::from(v.color) * c).to_array();
    }
    mesh
}

/// Darken vertex colors toward `dark` below `y1` (fully at `y0`): dirt and damp where a
/// post meets the ground.
pub fn grime(mut mesh: Mesh, y0: f32, y1: f32, dark: f32) -> Mesh {
    for v in &mut mesh.vertices {
        let k = engine_core::math::smoothstep(y0, y1, v.position[1]);
        let f = dark + (1.0 - dark) * k;
        v.color[0] *= f;
        v.color[1] *= f;
        v.color[2] *= f;
    }
    mesh
}

/// Displace vertices by smooth noise (same position, same offset, so seams stay closed).
pub fn jitter(mut mesh: Mesh, amount: f32, freq: f32, seed: u32) -> Mesh {
    for v in &mut mesh.vertices {
        let p = Vec3::from(v.position) * freq;
        let d = Vec3::new(
            noise3(p, seed),
            noise3(p + Vec3::splat(17.3), seed),
            noise3(p + Vec3::splat(41.7), seed),
        );
        v.position = (Vec3::from(v.position) + d * amount).to_array();
    }
    mesh
}

/// Bow a mesh along `axis` (its length `len`, centered): points move along `dir` by up to
/// `amount` in the middle. Twist rotates cross-sections about `axis` by up to `twist`
/// radians at the ends (opposite signs). Normals follow approximately.
pub fn warp(mut mesh: Mesh, axis: Vec3, len: f32, dir: Vec3, amount: f32, twist: f32) -> Mesh {
    let half = (len * 0.5).max(1e-4);
    for v in &mut mesh.vertices {
        let p = Vec3::from(v.position);
        let s = (p.dot(axis) / half).clamp(-1.0, 1.0);
        let q = Quat::from_axis_angle(axis, twist * s);
        let moved = q * p + dir * (amount * (1.0 - s * s));
        v.position = moved.to_array();
        v.normal = (q * Vec3::from(v.normal)).to_array();
    }
    mesh
}

/// Recompute smooth normals, sharing them between vertices at the same position (UV seams
/// stay invisible).
pub fn smooth_normals(mut mesh: Mesh) -> Mesh {
    use std::collections::HashMap;
    let key = |p: [f32; 3]| p.map(|c| (c * 1e5).round() as i64);
    let mut sums: HashMap<[i64; 3], Vec3> = HashMap::new();
    for tri in mesh.indices.as_chunks::<3>().0 {
        let [a, b, c] =
            [tri[0], tri[1], tri[2]].map(|i| Vec3::from(mesh.vertices[i as usize].position));
        let n = (b - a).cross(c - a);
        for i in tri {
            *sums
                .entry(key(mesh.vertices[*i as usize].position))
                .or_default() += n;
        }
    }
    for v in &mut mesh.vertices {
        if let Some(n) = sums.get(&key(v.position)) {
            v.normal = n.normalize_or(Vec3::Y).to_array();
        }
    }
    mesh
}

/// Join meshes into one.
pub fn join(parts: impl IntoIterator<Item = Mesh>) -> Mesh {
    let mut m = Mesh::default();
    for p in parts {
        m.merge(&p);
    }
    m
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Finite positions, unit normals, valid indices, and faces wound to agree with normals.
    pub fn check(name: &str, m: &Mesh) {
        assert!(!m.indices.is_empty(), "{name}: empty");
        assert_eq!(m.indices.len() % 3, 0, "{name}");
        for v in &m.vertices {
            let p = Vec3::from(v.position);
            assert!(p.is_finite(), "{name}: position {p}");
            let n = Vec3::from(v.normal);
            assert!((n.length() - 1.0).abs() < 1e-3, "{name}: normal {n}");
            assert!(Vec2::from(v.uv).is_finite(), "{name}: uv");
        }
        let count = m.vertices.len() as u32;
        assert!(
            m.indices.iter().all(|&i| i < count),
            "{name}: index out of range"
        );
        let mut bad = 0;
        let mut total = 0;
        for t in m.indices.as_chunks::<3>().0 {
            let p = |i: u32| Vec3::from(m.vertices[i as usize].position);
            let nn: Vec3 = t
                .iter()
                .map(|&i| Vec3::from(m.vertices[i as usize].normal))
                .sum();
            let g = (p(t[1]) - p(t[0])).cross(p(t[2]) - p(t[0]));
            if g.length_squared() > 1e-14 {
                total += 1;
                if g.dot(nn) < 0.0 {
                    bad += 1;
                }
            }
        }
        assert!(
            bad * 50 <= total,
            "{name}: {bad}/{total} triangles face inward"
        );
    }

    #[test]
    fn builders_make_valid_meshes() {
        let r = rounded_box(Vec3::new(1.0, 0.3, 0.2), 0.03, 3, 0.25, BoxUv::PLANAR);
        check("rounded box", &r);
        let b = r.aabb();
        assert!(b.max.abs_diff_eq(Vec3::new(0.5, 0.15, 0.1), 1e-4), "{b:?}");
        check(
            "timber",
            &timber(Vec3::new(0.2, 3.0, 0.1), 0.01, Vec2::new(0.1, 0.3), true),
        );
        check(
            "sharp box",
            &rounded_box(Vec3::ONE, 0.0, 1, 1.0, BoxUv::PLANAR),
        );
        let post = lathe(
            &[
                Vec2::new(0.0, 0.0),
                Vec2::new(0.1, 0.0),
                Vec2::new(0.1, 0.0),
                Vec2::new(0.1, 1.0),
                Vec2::new(0.06, 1.1),
                Vec2::new(0.0, 1.15),
            ],
            16,
            LatheUv::wood(0.1, Vec2::ZERO),
        );
        check("lathe", &post);
        check("sphere", &sphere(0.5, 16));
        let helix: Vec<Vec3> = (0..40)
            .map(|i| {
                let a = i as f32 * 0.3;
                Vec3::new(a.cos(), i as f32 * 0.05, a.sin())
            })
            .collect();
        check(
            "tube",
            &tube(&helix, |t| 0.05 + 0.02 * t, 8, false, true, 1.0),
        );
        check("torus", &torus(0.5, 0.1, 24, 8));
        check("link", &chain_link(0.1, 0.06, 0.01));
        check(
            "chain",
            &chain(&[Vec3::ZERO, Vec3::new(0.0, -1.0, 0.2)], 0.1, 0.01),
        );
        let ibeam = [
            Vec2::new(-0.1, -0.15),
            Vec2::new(0.1, -0.15),
            Vec2::new(0.1, -0.13),
            Vec2::new(0.01, -0.13),
            Vec2::new(0.01, 0.13),
            Vec2::new(0.1, 0.13),
            Vec2::new(0.1, 0.15),
            Vec2::new(-0.1, 0.15),
            Vec2::new(-0.1, 0.13),
            Vec2::new(-0.01, 0.13),
            Vec2::new(-0.01, -0.13),
            Vec2::new(-0.1, -0.13),
        ];
        assert_eq!(triangulate(&ibeam).len(), 10);
        check("extrude", &extrude(&ibeam, 2.0, 0.004, 30.0));
        check("nut", &hex_nut(0.03, 0.02));
        check("dome", &dome(0.01, 0.006, 8));
        check("bolt", &bolt(0.02));
        let bent = warp(
            timber(Vec3::new(2.0, 0.05, 0.2), 0.01, Vec2::ZERO, false),
            Vec3::X,
            2.0,
            Vec3::Y,
            0.02,
            0.02,
        );
        check("warped", &jitter(grime(bent, 0.0, 0.5, 0.6), 0.003, 3.0, 7));
    }

    #[test]
    fn wood_uvs_stay_inside_their_regions() {
        for across in [0.0, 0.1, 0.3] {
            let uv = wood_uv(across, 5.0, 0.3, Vec2::new(0.2, 0.0), false);
            assert!(uv.x >= 0.0 && uv.x <= WOOD_GRAIN_U, "{uv}");
        }
        let e = wood_end_uv(1.0, -1.0);
        assert!(e.x > WOOD_GRAIN_U && e.x < 1.0 && e.y > 0.0 && e.y < 1.0 - WOOD_GRAIN_U);
    }
}
