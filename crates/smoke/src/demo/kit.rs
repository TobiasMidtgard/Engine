//! Mesh building blocks for the range props: beveled boxes, lathes, extrusions with filleted
//! profiles, swept tubes, parametric surfaces, and vertex shading (cavity, dirt, tint).
//!
//! Every builder returns a [`Mesh`] in meters with counter-clockwise front faces and texture
//! coordinates in meters (so one material tiles at the same density on every prop: set the
//! material's `uv_scale` to `1 / texture size in meters`).

use std::f32::consts::{FRAC_PI_4, TAU};

use engine_core::math::{Mat4, Quat, Vec2, Vec3};
use engine_render::{Mesh, Vertex};

/// A vertex with white color.
pub fn vtx(p: Vec3, n: Vec3, uv: Vec2) -> Vertex {
    Vertex::new(p, n, uv)
}

/// Translation and rotation as a matrix.
pub fn tr(t: Vec3, r: Quat) -> Mat4 {
    Mat4::from_rotation_translation(r, t)
}

/// Translation only.
pub fn at(t: Vec3) -> Mat4 {
    Mat4::from_translation(t)
}

/// The mesh, transformed.
pub fn placed(mut m: Mesh, t: Mat4) -> Mesh {
    m.transform(&t);
    m
}

/// Rotation that turns +Y into `dir`.
pub fn y_to(dir: Vec3) -> Quat {
    Quat::from_rotation_arc(Vec3::Y, dir.normalize_or(Vec3::Y))
}

/// Multiply every vertex color by `f(position, normal)` (RGB; alpha untouched).
pub fn shade(m: &mut Mesh, f: impl Fn(Vec3, Vec3) -> Vec3) {
    for v in &mut m.vertices {
        let c = f(Vec3::from(v.position), Vec3::from(v.normal));
        v.color[0] *= c.x;
        v.color[1] *= c.y;
        v.color[2] *= c.z;
    }
}

/// Multiply every vertex color by one color.
pub fn tint(mut m: Mesh, c: Vec3) -> Mesh {
    shade(&mut m, |_, _| c);
    m
}

/// Darken toward the ground (contact occlusion and splashed dirt): full strength at `ground`,
/// gone `height` m above it.
pub fn ground_grime(m: &mut Mesh, ground: f32, height: f32, dark: f32, dirt: Vec3) {
    shade(m, |p, n| {
        let t = 1.0 - ((p.y - ground) / height).clamp(0.0, 1.0);
        let t = t * t * (3.0 - 2.0 * t);
        // Up-facing surfaces near the ground catch more dirt than overhangs.
        let k = t * (0.75 + 0.25 * n.y.max(0.0));
        Vec3::ONE.lerp(dirt * dark, k)
    });
}

/// Shift texture coordinates (to decorrelate repeated props).
pub fn offset_uv(mut m: Mesh, o: Vec2) -> Mesh {
    for v in &mut m.vertices {
        v.uv[0] += o.x;
        v.uv[1] += o.y;
    }
    m
}

/// Box centered on the origin with rounded edges of `radius` (`segs` steps per quarter
/// round). Flat faces are split into cells no bigger than `cell` m so vertex shading has
/// something to work with. Texture u runs along each face's longer side.
pub fn rounded_box(size: Vec3, radius: f32, segs: u32, cell: f32) -> Mesh {
    let h = size * 0.5;
    let r = radius.clamp(0.0, h.min_element() * 0.98);
    let segs = if r > 0.0 { segs.max(1) } else { 0 };
    let inner = h - Vec3::splat(r);
    let ticks = |inner: f32| -> Vec<f32> {
        let mut t = Vec::new();
        for k in (1..=segs).rev() {
            let a = k as f32 / segs as f32 * FRAC_PI_4;
            t.push(-(inner + r * a.tan()));
        }
        if inner > 1e-5 {
            let n = ((2.0 * inner) / cell.max(1e-3)).ceil().max(1.0) as usize;
            for i in 0..=n {
                t.push(-inner + 2.0 * inner * i as f32 / n as f32);
            }
        } else {
            t.push(0.0);
        }
        for k in 1..=segs {
            let a = k as f32 / segs as f32 * FRAC_PI_4;
            t.push(inner + r * a.tan());
        }
        t
    };
    let axis_ticks = [ticks(inner.x), ticks(inner.y), ticks(inner.z)];
    let unit = [Vec3::X, Vec3::Y, Vec3::Z];
    let mut m = Mesh::default();
    for a in 0..3 {
        for s in [1.0f32, -1.0] {
            let n = unit[a] * s;
            let (b, c) = ((a + 1) % 3, (a + 2) % 3);
            // u along the longer in-plane side, v so that u × v = n.
            let (ua, va) = if size[b] >= size[c] { (b, c) } else { (c, b) };
            let vsign = if unit[ua].cross(unit[va]).dot(n) > 0.0 {
                1.0
            } else {
                -1.0
            };
            let (tu, tv) = (&axis_ticks[ua], &axis_ticks[va]);
            let base = m.vertices.len() as u32;
            let row = tu.len() as u32;
            for &v in tv {
                let v = v * vsign;
                for &u in tu {
                    let mut p = Vec3::ZERO;
                    p[a] = s * h[a];
                    p[ua] = u;
                    p[va] = v;
                    let q = p.clamp(-inner, inner);
                    let d = (p - q).normalize_or(n);
                    let pos = if r > 0.0 { q + d * r } else { p };
                    m.vertices
                        .push(vtx(pos, if r > 0.0 { d } else { n }, Vec2::new(u, v)));
                }
            }
            for j in 0..tv.len() as u32 - 1 {
                for i in 0..row - 1 {
                    let i0 = base + j * row + i;
                    let (i1, i2, i3) = (i0 + 1, i0 + row + 1, i0 + row);
                    m.indices.extend_from_slice(&[i0, i1, i2, i0, i2, i3]);
                }
            }
        }
    }
    m
}

/// Surface of revolution around +Y. `profile` is (radius, height) from bottom to top along
/// the outside (its normal points away from the axis on the right-hand side). Corners sharper
/// than `crease_deg` stay hard. `faceted` gives flat sides (hex nuts, octagons).
pub fn lathe(profile: &[Vec2], segments: u32, crease_deg: f32, faceted: bool) -> Mesh {
    lathe_arc(profile, segments, crease_deg, faceted, 0.0, TAU)
}

/// [`lathe`] over part of a turn, from angle `a0` to `a1` (radians, about +Y, 0 = +X).
pub fn lathe_arc(
    profile: &[Vec2],
    segments: u32,
    crease_deg: f32,
    faceted: bool,
    a0: f32,
    a1: f32,
) -> Mesh {
    let mut m = Mesh::default();
    let profile = dedupe_open(profile);
    if profile.len() < 2 {
        return m;
    }
    let segments = segments.max(3);
    let strips = profile_strips(&profile, false, crease_deg);
    let r_ref = profile.iter().fold(0.0f32, |a, p| a.max(p.x)).max(1e-3);
    let ring = |a: f32| (a.cos(), -a.sin());
    for s in &strips {
        let base = m.vertices.len() as u32;
        if faceted {
            for k in 0..segments {
                let ang0 = a0 + (a1 - a0) * k as f32 / segments as f32;
                let ang1 = a0 + (a1 - a0) * (k + 1) as f32 / segments as f32;
                let mid = 0.5 * (ang0 + ang1);
                let (cm, sm) = ring(mid);
                let b = m.vertices.len() as u32;
                for &(p, n, v) in s {
                    for ang in [ang0, ang1] {
                        let (c, sn) = ring(ang);
                        let pos = Vec3::new(p.x * c, p.y, p.x * sn);
                        // Flat side: the 2D normal turned to the facet's middle, scaled so
                        // the facet plane stays flat.
                        let half = 0.5 * (ang1 - ang0);
                        let nn = Vec3::new(
                            n.x * cm / half.cos().max(1e-3),
                            n.y,
                            n.x * sm / half.cos().max(1e-3),
                        )
                        .normalize_or(Vec3::Y);
                        m.vertices.push(vtx(pos, nn, Vec2::new(ang * r_ref, v)));
                    }
                }
                for j in 0..s.len() as u32 - 1 {
                    let i0 = b + j * 2;
                    let (i1, i2, i3) = (i0 + 1, i0 + 3, i0 + 2);
                    m.indices.extend_from_slice(&[i0, i1, i2, i0, i2, i3]);
                }
            }
        } else {
            for k in 0..=segments {
                let ang = a0 + (a1 - a0) * k as f32 / segments as f32;
                let (c, sn) = ring(ang);
                for &(p, n, v) in s {
                    let pos = Vec3::new(p.x * c, p.y, p.x * sn);
                    let nn = Vec3::new(n.x * c, n.y, n.x * sn).normalize_or(Vec3::Y);
                    m.vertices.push(vtx(pos, nn, Vec2::new(ang * r_ref, v)));
                }
            }
            let row = s.len() as u32;
            for k in 0..segments {
                for j in 0..row - 1 {
                    let i0 = base + k * row + j;
                    let (i1, i2, i3) = (i0 + row, i0 + row + 1, i0 + 1);
                    m.indices.extend_from_slice(&[i0, i1, i2, i0, i2, i3]);
                }
            }
        }
    }
    m
}

/// [`lathe`] of a closed (r, y) loop, counter-clockwise: a shell such as a rolled drum.
pub fn lathe_loop(profile: &[Vec2], segments: u32, crease_deg: f32) -> Mesh {
    let mut pts = dedupe(profile);
    if area(&pts) < 0.0 {
        pts.reverse();
    }
    let strips = closed_strips(&pts, crease_deg);
    lathe_strips(&strips, segments, &pts)
}

fn lathe_strips(strips: &[Vec<(Vec2, Vec2, f32)>], segments: u32, pts: &[Vec2]) -> Mesh {
    let mut m = Mesh::default();
    let r_ref = pts.iter().fold(0.0f32, |a, p| a.max(p.x)).max(1e-3);
    for s in strips {
        let base = m.vertices.len() as u32;
        for k in 0..=segments {
            let ang = k as f32 / segments as f32 * TAU;
            let (c, sn) = (ang.cos(), -ang.sin());
            for &(p, n, v) in s {
                let pos = Vec3::new(p.x * c, p.y, p.x * sn);
                let nn = Vec3::new(n.x * c, n.y, n.x * sn).normalize_or(Vec3::Y);
                m.vertices.push(vtx(pos, nn, Vec2::new(ang * r_ref, v)));
            }
        }
        let row = s.len() as u32;
        for k in 0..segments {
            for j in 0..row - 1 {
                let i0 = base + k * row + j;
                let (i1, i2, i3) = (i0 + row, i0 + row + 1, i0 + 1);
                m.indices.extend_from_slice(&[i0, i1, i2, i0, i2, i3]);
            }
        }
    }
    m
}

/// Split a 2D polyline into runs of smoothly shaded points: (point, outward normal, length
/// along the line). The normal is on the right of the direction of travel.
fn profile_strips(points: &[Vec2], closed: bool, crease_deg: f32) -> Vec<Vec<(Vec2, Vec2, f32)>> {
    let n = points.len();
    let seg_count = if closed { n } else { n - 1 };
    let seg_normal = |i: usize| {
        let a = points[i % n];
        let b = points[(i + 1) % n];
        let t = b - a;
        Vec2::new(t.y, -t.x).normalize_or(Vec2::X)
    };
    let cos_crease = crease_deg.to_radians().cos();
    let mut strips: Vec<Vec<(Vec2, Vec2, f32)>> = Vec::new();
    let mut cur: Vec<(Vec2, Vec2, f32)> = Vec::new();
    let mut length = 0.0;
    for i in 0..seg_count {
        let a = points[i % n];
        let b = points[(i + 1) % n];
        let ns = seg_normal(i);
        // Start of this segment.
        let prev = if i > 0 || closed {
            Some(seg_normal((i + seg_count - 1) % seg_count))
        } else {
            None
        };
        let start_n = match prev {
            Some(p) if p.dot(ns) >= cos_crease => (p + ns).normalize_or(ns),
            _ => ns,
        };
        let smooth_start = matches!(prev, Some(p) if p.dot(ns) >= cos_crease);
        if !smooth_start || cur.is_empty() {
            if cur.len() >= 2 {
                strips.push(std::mem::take(&mut cur));
            }
            cur.clear();
            cur.push((a, start_n, length));
        }
        length += (b - a).length();
        let next = if i + 1 < seg_count || closed {
            Some(seg_normal((i + 1) % seg_count))
        } else {
            None
        };
        let end_n = match next {
            Some(q) if q.dot(ns) >= cos_crease => (q + ns).normalize_or(ns),
            _ => ns,
        };
        cur.push((b, end_n, length));
        let smooth_end = matches!(next, Some(q) if q.dot(ns) >= cos_crease);
        if !smooth_end {
            strips.push(std::mem::take(&mut cur));
        }
    }
    if cur.len() >= 2 {
        strips.push(cur);
    }
    strips
}

/// [`profile_strips`] for a closed loop: one seamless run when it has no hard corners,
/// otherwise runs that start and end at its hard corners.
fn closed_strips(points: &[Vec2], crease_deg: f32) -> Vec<Vec<(Vec2, Vec2, f32)>> {
    let n = points.len();
    let seg_normal = |i: usize| {
        let t = points[(i + 1) % n] - points[i % n];
        Vec2::new(t.y, -t.x).normalize_or(Vec2::X)
    };
    let cos_crease = crease_deg.to_radians().cos();
    let crease = (0..n).find(|&i| seg_normal((i + n - 1) % n).dot(seg_normal(i)) < cos_crease);
    match crease {
        None => {
            let mut strip = Vec::with_capacity(n + 1);
            let mut length = 0.0;
            for i in 0..=n {
                let k = i % n;
                if i > 0 {
                    length += (points[k] - points[i - 1]).length();
                }
                let nn = (seg_normal((k + n - 1) % n) + seg_normal(k)).normalize_or(Vec2::X);
                strip.push((points[k], nn, length));
            }
            vec![strip]
        }
        Some(c) => {
            let mut ring: Vec<Vec2> = (0..=n).map(|i| points[(c + i) % n]).collect();
            ring.dedup_by(|a, b| (*a - *b).length_squared() < 1e-14);
            profile_strips(&ring, false, crease_deg)
        }
    }
}

/// Drop consecutive points closer than 0.01 mm (open polylines).
pub fn dedupe_open(points: &[Vec2]) -> Vec<Vec2> {
    let mut out: Vec<Vec2> = Vec::with_capacity(points.len());
    for &p in points {
        if out.last().is_none_or(|q| (*q - p).length_squared() > 1e-10) {
            out.push(p);
        }
    }
    out
}

/// Drop consecutive points closer than 0.01 mm (and a last point equal to the first).
pub fn dedupe(points: &[Vec2]) -> Vec<Vec2> {
    let mut out: Vec<Vec2> = Vec::with_capacity(points.len());
    for &p in points {
        if out.last().is_none_or(|q| (*q - p).length_squared() > 1e-10) {
            out.push(p);
        }
    }
    while out.len() > 2 && (out[0] - out[out.len() - 1]).length_squared() <= 1e-10 {
        out.pop();
    }
    out
}

/// Round every corner of a closed polygon with an arc of `radius` (shrunk where the edges are
/// too short), `segs` steps per arc.
pub fn fillet(points: &[Vec2], radius: f32, segs: u32) -> Vec<Vec2> {
    fillet_each(points, &vec![radius; points.len()], segs)
}

/// [`fillet`] with a radius per corner (0 keeps the corner sharp).
pub fn fillet_each(points: &[Vec2], radii: &[f32], segs: u32) -> Vec<Vec2> {
    let n = points.len();
    let mut out = Vec::with_capacity(n * (segs as usize + 1));
    for i in 0..n {
        let p = points[i];
        let a = points[(i + n - 1) % n];
        let b = points[(i + 1) % n];
        let r = radii.get(i).copied().unwrap_or(0.0);
        let (da, db) = ((a - p).normalize_or(Vec2::X), (b - p).normalize_or(Vec2::X));
        let cos = da.dot(db).clamp(-1.0, 1.0);
        let half = cos.acos() * 0.5;
        if r <= 0.0 || half < 1e-3 || (std::f32::consts::FRAC_PI_2 - half) < 1e-3 {
            out.push(p);
            continue;
        }
        let mut d = r / half.tan();
        let lim = 0.49 * (a - p).length().min((b - p).length());
        let r = if d > lim {
            d = lim;
            lim * half.tan()
        } else {
            r
        };
        let ta = p + da * d;
        let tb = p + db * d;
        let bis = (da + db).normalize_or(Vec2::X);
        let center = p + bis * (r / half.sin());
        let s0 = ta - center;
        let s1 = tb - center;
        let ang0 = s0.y.atan2(s0.x);
        let mut sweep = s1.y.atan2(s1.x) - ang0;
        while sweep > std::f32::consts::PI {
            sweep -= TAU;
        }
        while sweep < -std::f32::consts::PI {
            sweep += TAU;
        }
        for k in 0..=segs {
            let t = ang0 + sweep * k as f32 / segs as f32;
            out.push(center + Vec2::new(t.cos(), t.sin()) * r);
        }
    }
    dedupe(&out)
}

/// Signed area of a polygon (positive when counter-clockwise).
pub fn area(points: &[Vec2]) -> f32 {
    let n = points.len();
    let mut a = 0.0;
    for i in 0..n {
        let (p, q) = (points[i], points[(i + 1) % n]);
        a += p.x * q.y - q.x * p.y;
    }
    a * 0.5
}

/// Triangulate a simple counter-clockwise polygon (ear clipping). Indices into `points`.
pub fn triangulate(points: &[Vec2]) -> Vec<u32> {
    let n = points.len();
    let mut out = Vec::with_capacity(n.saturating_sub(2) * 3);
    if n < 3 {
        return out;
    }
    let mut idx: Vec<usize> = (0..n).collect();
    if area(points) < 0.0 {
        idx.reverse();
    }
    let cross = |o: Vec2, a: Vec2, b: Vec2| (a - o).perp_dot(b - o);
    let mut guard = 0;
    while idx.len() > 3 && guard < n * n {
        guard += 1;
        let m = idx.len();
        let mut clipped = false;
        for i in 0..m {
            let (ia, ib, ic) = (idx[(i + m - 1) % m], idx[i], idx[(i + 1) % m]);
            let (a, b, c) = (points[ia], points[ib], points[ic]);
            if cross(a, b, c) <= 1e-12 {
                continue;
            }
            let inside = idx.iter().any(|&j| {
                if j == ia || j == ib || j == ic {
                    return false;
                }
                let p = points[j];
                cross(a, b, p) >= 0.0 && cross(b, c, p) >= 0.0 && cross(c, a, p) >= 0.0
            });
            if inside {
                continue;
            }
            out.extend_from_slice(&[ia as u32, ib as u32, ic as u32]);
            idx.remove(i);
            clipped = true;
            break;
        }
        if !clipped {
            // Degenerate leftovers: fan them.
            break;
        }
    }
    for k in 1..idx.len().saturating_sub(1) {
        out.extend_from_slice(&[idx[0] as u32, idx[k] as u32, idx[k + 1] as u32]);
    }
    out
}

/// Extrude a closed counter-clockwise polygon in the XY plane along +Z from `z0` to `z1`.
/// Corners sharper than `crease_deg` stay hard; `bevel` (m) chamfers both ends with a
/// smooth-shaded 45° roll-off. Caps are added at both ends.
pub fn extrude(profile: &[Vec2], z0: f32, z1: f32, crease_deg: f32, bevel: f32) -> Mesh {
    extrude_caps(profile, z0, z1, crease_deg, bevel, true, true)
}

/// [`extrude`] choosing which ends get caps (no bevel).
pub fn extrude_caps(
    profile: &[Vec2],
    z0: f32,
    z1: f32,
    crease_deg: f32,
    bevel: f32,
    cap0: bool,
    cap1: bool,
) -> Mesh {
    extrude_full(profile, z0, z1, crease_deg, bevel, bevel, cap0, cap1)
}

/// [`extrude`] with a separate chamfer at each end (0 = square end).
pub fn extrude_ends(profile: &[Vec2], z0: f32, z1: f32, crease_deg: f32, b0: f32, b1: f32) -> Mesh {
    extrude_full(profile, z0, z1, crease_deg, b0, b1, true, true)
}

#[allow(clippy::too_many_arguments)]
fn extrude_full(
    profile: &[Vec2],
    z0: f32,
    z1: f32,
    crease_deg: f32,
    b0: f32,
    b1: f32,
    cap0: bool,
    cap1: bool,
) -> Mesh {
    let mut pts = dedupe(profile);
    if area(&pts) < 0.0 {
        pts.reverse();
    }
    let n = pts.len();
    let mut m = Mesh::default();
    if n < 3 {
        return m;
    }
    // Miter directions for insetting the ends.
    let miter: Vec<Vec2> = (0..n)
        .map(|i| {
            let a = pts[(i + n - 1) % n];
            let p = pts[i];
            let b = pts[(i + 1) % n];
            let n0 = Vec2::new((p - a).y, -(p - a).x).normalize_or(Vec2::X);
            let n1 = Vec2::new((b - p).y, -(b - p).x).normalize_or(Vec2::X);
            let mid = (n0 + n1).normalize_or(n0);
            mid / mid.dot(n0).max(0.35)
        })
        .collect();
    let b0 = b0.clamp(0.0, 0.45 * (z1 - z0));
    let b1 = b1.clamp(0.0, 0.45 * (z1 - z0));
    let strips = closed_strips(&pts, crease_deg);
    // Rows along z: (z, inset, normal z-tilt).
    let mut rows: Vec<(f32, f32, f32)> = Vec::new();
    if b0 > 0.0 {
        rows.push((z0, b0, -1.0));
    }
    rows.push((z0 + b0, 0.0, 0.0));
    rows.push((z1 - b1, 0.0, 0.0));
    if b1 > 0.0 {
        rows.push((z1, b1, 1.0));
    }
    // Map a profile point back to its index to find its miter.
    let index_of = |p: Vec2| -> usize {
        pts.iter()
            .position(|q| (*q - p).length_squared() < 1e-12)
            .unwrap_or(0)
    };
    for s in &strips {
        let base = m.vertices.len() as u32;
        for &(z, inset, tilt) in &rows {
            for &(p, nrm, u) in s {
                let mi = miter[index_of(p)];
                let pos = p - mi * inset;
                let nn = (Vec3::new(nrm.x, nrm.y, 0.0) + Vec3::Z * tilt * 0.8).normalize();
                m.vertices
                    .push(vtx(Vec3::new(pos.x, pos.y, z), nn, Vec2::new(u, z)));
            }
        }
        let row = s.len() as u32;
        for r in 0..rows.len() as u32 - 1 {
            for j in 0..row - 1 {
                let i0 = base + r * row + j;
                let (i1, i2, i3) = (i0 + 1, i0 + 1 + row, i0 + row);
                m.indices.extend_from_slice(&[i0, i1, i2, i0, i2, i3]);
            }
        }
    }
    let tris = triangulate(&pts);
    for (z, sign, on, b) in [(z0, -1.0f32, cap0, b0), (z1, 1.0, cap1, b1)] {
        if !on {
            continue;
        }
        let base = m.vertices.len() as u32;
        for (i, p) in pts.iter().enumerate() {
            let q = *p - miter[i] * b;
            m.vertices.push(vtx(
                Vec3::new(q.x, q.y, z),
                Vec3::Z * sign,
                Vec2::new(q.x, q.y),
            ));
        }
        for t in tris.chunks(3) {
            if sign > 0.0 {
                m.indices
                    .extend_from_slice(&[base + t[0], base + t[1], base + t[2]]);
            } else {
                m.indices
                    .extend_from_slice(&[base + t[0], base + t[2], base + t[1]]);
            }
        }
    }
    m
}

/// Frames (tangent, normal, binormal) along a path by parallel transport.
fn frames(path: &[Vec3], closed: bool) -> Vec<(Vec3, Vec3, Vec3)> {
    let n = path.len();
    let tangent = |i: usize| -> Vec3 {
        let (a, b) = if closed {
            (path[(i + n - 1) % n], path[(i + 1) % n])
        } else {
            (path[i.saturating_sub(1)], path[(i + 1).min(n - 1)])
        };
        (b - a).normalize_or(Vec3::Z)
    };
    let t0 = tangent(0);
    let helper = if t0.y.abs() < 0.9 { Vec3::Y } else { Vec3::X };
    let mut nrm = t0.cross(helper).normalize();
    let mut out = Vec::with_capacity(n);
    let mut prev_t = t0;
    for i in 0..n {
        let t = tangent(i);
        let q = Quat::from_rotation_arc(prev_t, t);
        nrm = (q * nrm).normalize();
        nrm = (nrm - t * nrm.dot(t)).normalize_or(nrm);
        out.push((t, nrm, t.cross(nrm)));
        prev_t = t;
    }
    out
}

/// Sweep a closed counter-clockwise cross-section (x along the path's normal, y along its
/// binormal) along `path`. Open paths get flat end caps. `crease_deg` keeps the section's sharp
/// corners hard.
pub fn sweep(path: &[Vec3], section: &[Vec2], closed: bool, crease_deg: f32) -> Mesh {
    let mut m = Mesh::default();
    if path.len() < 2 || section.len() < 3 {
        return m;
    }
    let fr = frames(path, closed);
    let mut sec = dedupe(section);
    if area(&sec) < 0.0 {
        sec.reverse();
    }
    let strips = closed_strips(&sec, crease_deg);
    let count = if closed { path.len() + 1 } else { path.len() };
    let mut lengths = vec![0.0f32; count];
    for i in 1..count {
        lengths[i] = lengths[i - 1] + (path[i % path.len()] - path[i - 1]).length();
    }
    for s in &strips {
        let base = m.vertices.len() as u32;
        for i in 0..count {
            let (_, nn, bn) = fr[i % path.len()];
            let p = path[i % path.len()];
            for &(q, n2, v) in s {
                let pos = p + nn * q.x + bn * q.y;
                let nor = (nn * n2.x + bn * n2.y).normalize_or(nn);
                m.vertices.push(vtx(pos, nor, Vec2::new(lengths[i], v)));
            }
        }
        let row = s.len() as u32;
        for i in 0..count as u32 - 1 {
            for j in 0..row - 1 {
                // Section runs counter-clockwise around the path tangent.
                let i0 = base + i * row + j;
                let (i1, i2, i3) = (i0 + 1, i0 + row + 1, i0 + row);
                m.indices.extend_from_slice(&[i0, i1, i2, i0, i2, i3]);
            }
        }
    }
    if !closed {
        let tris = triangulate(&sec);
        for (end, sign) in [(0usize, -1.0f32), (path.len() - 1, 1.0)] {
            let (t, nn, bn) = fr[end];
            let p = path[end];
            let base = m.vertices.len() as u32;
            for q in &sec {
                m.vertices.push(vtx(p + nn * q.x + bn * q.y, t * sign, *q));
            }
            for tri in tris.chunks(3) {
                if sign > 0.0 {
                    m.indices
                        .extend_from_slice(&[base + tri[0], base + tri[1], base + tri[2]]);
                } else {
                    m.indices
                        .extend_from_slice(&[base + tri[0], base + tri[2], base + tri[1]]);
                }
            }
        }
    }
    m
}

/// A circle of `sides` points with radius `r` (counter-clockwise).
pub fn circle(r: f32, sides: u32) -> Vec<Vec2> {
    (0..sides)
        .map(|i| {
            let a = i as f32 / sides as f32 * TAU;
            Vec2::new(a.cos(), a.sin()) * r
        })
        .collect()
}

/// A round tube of radius `r` along `path`.
pub fn tube(path: &[Vec3], r: f32, sides: u32, closed: bool) -> Mesh {
    sweep(path, &circle(r, sides), closed, 89.0)
}

/// A ring (torus) of radius `radius` in the XZ plane, tube radius `r`.
pub fn ring(radius: f32, r: f32, segments: u32, sides: u32) -> Mesh {
    let path: Vec<Vec3> = (0..segments)
        .map(|i| {
            let a = i as f32 / segments as f32 * TAU;
            Vec3::new(a.cos() * radius, 0.0, a.sin() * radius)
        })
        .collect();
    tube(&path, r, sides, true)
}

/// A rod of radius `r` from `a` to `b` with flat ends.
pub fn rod(a: Vec3, b: Vec3, r: f32, sides: u32) -> Mesh {
    let len = (b - a).length();
    let prof = [
        Vec2::new(0.0, 0.0),
        Vec2::new(r, 0.0),
        Vec2::new(r, len),
        Vec2::new(0.0, len),
    ];
    placed(lathe(&prof, sides, 50.0, false), tr(a, y_to(b - a)))
}

/// A parametric surface: `f(u, v)` for u, v in 0..1 on an `nu` × `nv` grid gives (position,
/// texture coordinate). Wrapped directions repeat the first row (for the texture seam). Normals
/// are smoothed across seams and poles.
pub fn surface(nu: u32, nv: u32, wrap_u: bool, f: impl Fn(f32, f32) -> (Vec3, Vec2)) -> Mesh {
    let mut m = Mesh::default();
    for j in 0..=nv {
        for i in 0..=nu {
            let (u, v) = (i as f32 / nu as f32, j as f32 / nv as f32);
            let (p, uv) = f(if wrap_u && i == nu { 1.0 } else { u }, v);
            m.vertices.push(vtx(p, Vec3::Y, uv));
        }
    }
    let row = nu + 1;
    for j in 0..nv {
        for i in 0..nu {
            let i0 = j * row + i;
            let (i1, i2, i3) = (i0 + 1, i0 + row + 1, i0 + row);
            m.indices.extend_from_slice(&[i0, i1, i2, i0, i2, i3]);
        }
    }
    welded_normals(&mut m);
    m
}

/// Smooth normals that ignore duplicated vertices (texture seams, poles): every vertex at
/// the same position gets the same normal.
pub fn welded_normals(m: &mut Mesh) {
    use std::collections::HashMap;
    let key = |p: [f32; 3]| {
        (
            (p[0] * 1e4).round() as i64,
            (p[1] * 1e4).round() as i64,
            (p[2] * 1e4).round() as i64,
        )
    };
    let mut acc: HashMap<(i64, i64, i64), Vec3> = HashMap::new();
    for t in m.indices.chunks(3) {
        let [a, b, c] = [t[0] as usize, t[1] as usize, t[2] as usize];
        let pa = Vec3::from(m.vertices[a].position);
        let pb = Vec3::from(m.vertices[b].position);
        let pc = Vec3::from(m.vertices[c].position);
        let n = (pb - pa).cross(pc - pa);
        for i in [a, b, c] {
            *acc.entry(key(m.vertices[i].position)).or_insert(Vec3::ZERO) += n;
        }
    }
    for v in &mut m.vertices {
        if let Some(n) = acc.get(&key(v.position)) {
            v.normal = n.normalize_or(Vec3::Y).to_array();
        }
    }
}

/// Turn a closed mesh's triangles to face outward (by the sign of its enclosed volume).
pub fn orient_outward(m: &mut Mesh) {
    let mut vol = 0.0;
    for t in m.indices.chunks(3) {
        let a = Vec3::from(m.vertices[t[0] as usize].position);
        let b = Vec3::from(m.vertices[t[1] as usize].position);
        let c = Vec3::from(m.vertices[t[2] as usize].position);
        vol += a.dot(b.cross(c));
    }
    if vol < 0.0 {
        for t in m.indices.chunks_mut(3) {
            t.swap(1, 2);
        }
    }
}

/// A hex bolt head with a washer and a stub of thread, standing on +Y from the origin.
pub fn hex_bolt(size: f32) -> Mesh {
    let s = size;
    let mut m = lathe(
        &[
            Vec2::new(0.0, 0.0),
            Vec2::new(s * 1.05, 0.0),
            Vec2::new(s * 1.05, s * 0.18),
            Vec2::new(s * 0.98, s * 0.22),
            Vec2::new(0.0, s * 0.22),
        ],
        14,
        40.0,
        false,
    );
    let hex = lathe(
        &[
            Vec2::new(0.0, s * 0.22),
            Vec2::new(s * 0.86, s * 0.22),
            Vec2::new(s * 0.86, s * 0.78),
            Vec2::new(s * 0.7, s * 0.86),
            Vec2::new(0.0, s * 0.86),
        ],
        6,
        30.0,
        true,
    );
    m.merge(&hex);
    let thread = lathe(
        &[
            Vec2::new(0.0, s * 0.86),
            Vec2::new(s * 0.45, s * 0.86),
            Vec2::new(s * 0.45, s * 1.3),
            Vec2::new(s * 0.35, s * 1.38),
            Vec2::new(0.0, s * 1.38),
        ],
        10,
        40.0,
        false,
    );
    m.merge(&thread);
    m
}

/// A dome-headed rivet of head radius `r` on +Y.
pub fn rivet(r: f32) -> Mesh {
    let mut p = vec![Vec2::new(0.0, -0.002), Vec2::new(r, -0.002)];
    p.extend((0..=5).map(|i| {
        let a = i as f32 / 5.0 * std::f32::consts::FRAC_PI_2;
        Vec2::new(r * a.cos(), r * 0.55 * a.sin())
    }));
    lathe(&p, 10, 50.0, false)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn check(m: &Mesh, name: &str) {
        assert!(!m.indices.is_empty(), "{name}: empty");
        let mut bad = 0;
        let mut total = 0;
        for t in m.indices.chunks(3) {
            let p: Vec<Vec3> = t
                .iter()
                .map(|&i| Vec3::from(m.vertices[i as usize].position))
                .collect();
            let n: Vec3 = t
                .iter()
                .map(|&i| Vec3::from(m.vertices[i as usize].normal))
                .sum();
            let g = (p[1] - p[0]).cross(p[2] - p[0]);
            if g.length_squared() > 1e-14 {
                total += 1;
                if g.dot(n) < 0.0 {
                    bad += 1;
                }
            }
        }
        assert!(
            bad * 50 <= total,
            "{name}: {bad} of {total} triangles wound clockwise"
        );
        for v in &m.vertices {
            assert!(
                Vec3::from(v.position).is_finite() && Vec3::from(v.normal).is_finite(),
                "{name}: NaN"
            );
        }
    }

    #[test]
    fn builders_wind_counter_clockwise() {
        check(&rounded_box(Vec3::new(1.0, 0.5, 2.0), 0.05, 3, 0.25), "box");
        check(
            &rounded_box(Vec3::new(1.0, 0.5, 2.0), 0.0, 3, 0.5),
            "sharp box",
        );
        let prof = [
            Vec2::new(0.0, 0.0),
            Vec2::new(0.5, 0.0),
            Vec2::new(0.5, 1.0),
            Vec2::new(0.3, 1.2),
            Vec2::new(0.0, 1.2),
        ];
        check(&lathe(&prof, 16, 30.0, false), "lathe");
        check(&lathe(&prof, 6, 30.0, true), "faceted lathe");
        let poly = fillet(
            &[
                Vec2::new(-0.3, 0.0),
                Vec2::new(0.3, 0.0),
                Vec2::new(0.1, 0.4),
                Vec2::new(0.05, 0.8),
                Vec2::new(-0.05, 0.8),
                Vec2::new(-0.1, 0.4),
            ],
            0.02,
            3,
        );
        // A bevel narrower than the fillets (a wider one would fold the rounded corners).
        check(&extrude(&poly, 0.0, 3.0, 40.0, 0.008), "extrude");
        check(
            &extrude(&poly, 0.0, 3.0, 40.0, 0.0),
            "extrude without bevel",
        );
        let path: Vec<Vec3> = (0..20)
            .map(|i| Vec3::new(i as f32 * 0.1, (i as f32 * 0.3).sin(), 0.0))
            .collect();
        check(&tube(&path, 0.05, 8, false), "tube");
        check(&ring(1.0, 0.01, 32, 6), "ring");
        check(&rod(Vec3::ZERO, Vec3::new(1.0, 1.0, 0.0), 0.1, 8), "rod");
        check(&hex_bolt(0.02), "bolt");
        let shell = [
            Vec2::new(1.0, -0.2),
            Vec2::new(1.02, -0.2),
            Vec2::new(1.02, 0.2),
            Vec2::new(1.0, 0.2),
        ];
        check(&lathe_loop(&shell, 32, 40.0), "lathe loop");
        let tri = [
            Vec2::new(0.02, 0.0),
            Vec2::new(-0.01, 0.01),
            Vec2::new(-0.01, -0.01),
        ];
        check(&sweep(&path, &tri, false, 30.0), "sweep");
        check(&rivet(0.01), "rivet");
        let s = surface(16, 8, true, |u, v| {
            let (a, b) = (u * TAU, v * std::f32::consts::PI);
            (
                Vec3::new(b.sin() * a.cos(), b.cos(), b.sin() * a.sin()),
                Vec2::new(u, v),
            )
        });
        check(&s, "surface");
    }

    #[test]
    fn concave_polygons_triangulate_fully() {
        let l = [
            Vec2::new(0.0, 0.0),
            Vec2::new(2.0, 0.0),
            Vec2::new(2.0, 1.0),
            Vec2::new(1.0, 1.0),
            Vec2::new(1.0, 2.0),
            Vec2::new(0.0, 2.0),
        ];
        let t = triangulate(&l);
        assert_eq!(t.len(), 12);
        let covered: f32 = t
            .chunks(3)
            .map(|c| {
                let (a, b, d) = (l[c[0] as usize], l[c[1] as usize], l[c[2] as usize]);
                (b - a).perp_dot(d - a) * 0.5
            })
            .sum();
        assert!((covered - 3.0).abs() < 1e-4, "{covered}");
    }
}
