//! Detail parts laid onto or into a body field: lenses and trim strips draped on the skin,
//! slabs (proud strips with side walls), tubes along the surface (light guides, wipers,
//! lettering), honeycomb grille inserts that follow the face they sit in, and small
//! sculpted parts (mirrors, seats) meshed from their own fields.

use engine_core::math::{Quat, Vec2, Vec3, Vec4};
use engine_render::{Mesh, Vertex};

use super::mesher::{self, Field};
use super::parts::{loft_rings, orient_like_normals};
use super::sdf::{gradient, raycast, sd_polygon};

/// Which way rays are cast onto the body: from the right side (−x) with points as (z, h), from
/// above (−y) with (x, z), from the front (+z) with (x, h), or from behind (−z) with (x, h).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Proj {
    Side,
    Top,
    Front,
    Back,
}

impl Proj {
    /// Ray origin and direction for plane point `q` (heights above a ground at −`ground`).
    pub fn ray(self, q: Vec2, ground: f32) -> (Vec3, Vec3) {
        match self {
            Proj::Side => (Vec3::new(1.5, q.y - ground, q.x), Vec3::NEG_X),
            Proj::Top => (Vec3::new(q.x, 1.6, q.y), Vec3::NEG_Y),
            Proj::Front => (Vec3::new(q.x, q.y - ground, -3.2), Vec3::Z),
            Proj::Back => (Vec3::new(q.x, q.y - ground, 3.2), Vec3::NEG_Z),
        }
    }
}

/// The surface point and normal under plane point `q`.
pub fn surface_point(f: Field, proj: Proj, q: Vec2, ground: f32) -> Option<(Vec3, Vec3)> {
    let (o, d) = proj.ray(q, ground);
    raycast(f, o, d, 4.0).map(|p| (p, gradient(f, p, 0.0015).normalize_or(-d)))
}

/// Resample a plane polyline every `step` meters and lay it on the surface.
pub fn surface_polyline(
    f: Field,
    proj: Proj,
    pts: &[Vec2],
    ground: f32,
    step: f32,
) -> Vec<(Vec3, Vec3)> {
    let mut out = Vec::new();
    for w in pts.windows(2) {
        let n = ((w[1] - w[0]).length() / step).ceil().max(1.0) as usize;
        for i in 0..n {
            let q = w[0].lerp(w[1], i as f32 / n as f32);
            out.extend(surface_point(f, proj, q, ground));
        }
    }
    if let Some(last) = pts.last() {
        out.extend(surface_point(f, proj, *last, ground));
    }
    out
}

/// A region between two plane curves: `lo(s)` and `hi(s)` for s in 0..1.
pub struct Strip<'a> {
    pub lo: &'a dyn Fn(f32) -> Vec2,
    pub hi: &'a dyn Fn(f32) -> Vec2,
}

impl Strip<'_> {
    pub fn at(&self, s: f32, t: f32) -> Vec2 {
        (self.lo)(s).lerp((self.hi)(s), t)
    }
}

/// A grid laid on the surface over a strip, pushed out by `offset(s, t)`; with `walls`, side
/// walls drop from the grid's border back to the surface (a proud lens or trim strip).
/// UV: `uv(s, t)`. Vertex color white.
#[allow(clippy::too_many_arguments)]
pub fn draped_slab(
    f: Field,
    proj: Proj,
    ground: f32,
    strip: &Strip,
    ns: usize,
    nt: usize,
    offset: &dyn Fn(f32, f32) -> f32,
    walls: bool,
    uv: &dyn Fn(f32, f32) -> Vec2,
) -> Mesh {
    let mut pts: Vec<Option<(Vec3, Vec3)>> = Vec::with_capacity((ns + 1) * (nt + 1));
    for j in 0..=nt {
        for i in 0..=ns {
            let (s, t) = (i as f32 / ns as f32, j as f32 / nt as f32);
            pts.push(surface_point(f, proj, strip.at(s, t), ground));
        }
    }
    let id = |i: usize, j: usize| j * (ns + 1) + i;
    let mut top = Mesh::default();
    let mut remap = vec![u32::MAX; pts.len()];
    for j in 0..=nt {
        for i in 0..=ns {
            if let Some((p, n)) = pts[id(i, j)] {
                let (s, t) = (i as f32 / ns as f32, j as f32 / nt as f32);
                remap[id(i, j)] = top.vertices.len() as u32;
                top.vertices
                    .push(Vertex::new(p + n * offset(s, t), n, uv(s, t)));
            }
        }
    }
    for j in 0..nt {
        for i in 0..ns {
            let q = [
                remap[id(i, j)],
                remap[id(i + 1, j)],
                remap[id(i + 1, j + 1)],
                remap[id(i, j + 1)],
            ];
            if !q.contains(&u32::MAX) {
                top.indices
                    .extend_from_slice(&[q[0], q[1], q[2], q[0], q[2], q[3]]);
            }
        }
    }
    orient_like_normals(&mut top);
    if !walls {
        return top;
    }
    // Border loop (grid order around the edge).
    let mut border: Vec<(usize, usize)> = Vec::new();
    for i in 0..ns {
        border.push((i, 0));
    }
    for j in 0..nt {
        border.push((ns, j));
    }
    for i in (1..=ns).rev() {
        border.push((i, nt));
    }
    for j in (1..=nt).rev() {
        border.push((0, j));
    }
    let mut wall = Mesh::default();
    let center: Vec3 = top
        .vertices
        .iter()
        .map(|v| Vec3::from(v.position))
        .sum::<Vec3>()
        / top.vertices.len().max(1) as f32;
    for k in 0..border.len() {
        let (a, b) = (border[k], border[(k + 1) % border.len()]);
        let (Some((pa, na)), Some((pb, nb))) = (pts[id(a.0, a.1)], pts[id(b.0, b.1)]) else {
            continue;
        };
        let sa = (a.0 as f32 / ns as f32, a.1 as f32 / nt as f32);
        let sb = (b.0 as f32 / ns as f32, b.1 as f32 / nt as f32);
        let ta = pa + na * offset(sa.0, sa.1);
        let tb = pb + nb * offset(sb.0, sb.1);
        let ba = pa - na * 0.002;
        let bb = pb - nb * 0.002;
        // Outward wall normal: away from the strip center, across the surface.
        let along = (tb - ta).normalize_or(Vec3::X);
        let mut out = along.cross(na).normalize_or(Vec3::Y);
        if out.dot(ta - center) < 0.0 {
            out = -out;
        }
        let base = wall.vertices.len() as u32;
        let u = uv(sa.0, sa.1);
        for p in [ta, tb, bb, ba] {
            wall.vertices.push(Vertex::new(p, out, u));
        }
        wall.indices
            .extend_from_slice(&[base, base + 1, base + 2, base, base + 2, base + 3]);
    }
    orient_like_normals(&mut wall);
    top.merge(&wall);
    top
}

/// A round tube through surface points, each pushed out `offset` along its normal.
pub fn tube_on(pts: &[(Vec3, Vec3)], offset: f32, r: f32, sides: usize) -> Mesh {
    let line: Vec<Vec3> = pts.iter().map(|(p, n)| *p + *n * offset).collect();
    if line.len() < 2 {
        return Mesh::default();
    }
    super::parts::tube(&line, r, sides)
}

/// A flat ribbon through surface points (width `w`, pushed out `offset`), UV u along, v across.
pub fn ribbon_on(pts: &[(Vec3, Vec3)], offset: f32, w: f32) -> Mesh {
    let mut m = Mesh::default();
    let n = pts.len();
    if n < 2 {
        return m;
    }
    let mut len = 0.0;
    for k in 0..n {
        let (p, nr) = pts[k];
        let t = if k + 1 < n {
            pts[k + 1].0 - p
        } else {
            p - pts[k - 1].0
        }
        .normalize_or(Vec3::X);
        if k > 0 {
            len += (p - pts[k - 1].0).length();
        }
        let side = t.cross(nr).normalize_or(Vec3::Y) * (w * 0.5);
        let c = p + nr * offset;
        m.vertices
            .push(Vertex::new(c - side, nr, Vec2::new(len, 0.0)));
        m.vertices
            .push(Vertex::new(c + side, nr, Vec2::new(len, 1.0)));
    }
    for k in 0..n as u32 - 1 {
        let a = k * 2;
        m.indices
            .extend_from_slice(&[a, a + 1, a + 3, a, a + 3, a + 2]);
    }
    orient_like_normals(&mut m);
    m
}

/// Honeycomb mesh filling `outline` (plane polygon in the projection's coordinates) on the
/// surface, set `inset` meters into it (along the ray): hexagonal cells of circumradius `r`,
/// webs `web` wide and `depth` deep. Every corner follows the face (depth sampled on a grid and
/// interpolated), so neighboring cells line up into one continuous mesh. Cells reach past the
/// outline so the edge hides in the pocket walls; cells that would show through the skin
/// where the face turns away are dropped.
#[allow(clippy::too_many_arguments)]
pub fn honeycomb(
    f: Field,
    proj: Proj,
    ground: f32,
    outline: &[Vec2],
    r: f32,
    web: f32,
    depth: f32,
    inset: f32,
) -> Mesh {
    let (lo, hi) = outline.iter().fold(
        (Vec2::splat(f32::MAX), Vec2::splat(f32::MIN)),
        |(a, b), p| (a.min(*p), b.max(*p)),
    );
    let dx = r * 3.0_f32.sqrt();
    let dy = r * 1.5;
    let mut m = Mesh::default();
    let dir = proj.ray(Vec2::ZERO, ground).1;
    // Where the face is along the ray at plane point q (cached per cell corner).
    let depth_at = |q: Vec2| -> Option<f32> {
        let (o, d) = proj.ray(q, ground);
        raycast(f, o, d, 4.0).map(|p| (p - o).dot(d))
    };
    let place = |q: Vec2, t: f32| -> Vec3 {
        let (o, d) = proj.ray(q, ground);
        o + d * t
    };
    // Face depth on a grid one cell radius apart, bilinearly interpolated.
    let g0 = lo - Vec2::splat(3.0 * r);
    let gx = ((hi.x - lo.x) / r).ceil() as usize + 7;
    let gy = ((hi.y - lo.y) / r).ceil() as usize + 7;
    let samples: Vec<Option<f32>> = (0..gx * gy)
        .map(|i| depth_at(g0 + Vec2::new((i % gx) as f32, (i / gx) as f32) * r))
        .collect();
    let face = |q: Vec2| -> Option<f32> {
        let f = (q - g0) / r;
        let (i, j) = (f.x.floor(), f.y.floor());
        if i < 0.0 || j < 0.0 || i as usize + 1 >= gx || j as usize + 1 >= gy {
            return None;
        }
        let (i, j, fx, fy) = (i as usize, j as usize, f.x - i, f.y - j);
        let s = |a: usize, b: usize| samples[b * gx + a];
        let (a, b, c, d) = (s(i, j)?, s(i + 1, j)?, s(i, j + 1)?, s(i + 1, j + 1)?);
        let lo = a + (b - a) * fx;
        let hi = c + (d - c) * fx;
        Some(lo + (hi - lo) * fy)
    };
    // How far inside the face a front corner must lie (less than `inset` where the face is
    // oblique to the ray).
    let keep_inside = -0.4 * inset + 0.001;
    let rows = ((hi.y - lo.y) / dy).ceil() as i32 + 2;
    let cols = ((hi.x - lo.x) / dx).ceil() as i32 + 2;
    for j in -1..=rows {
        for i in -1..=cols {
            let c = Vec2::new(
                lo.x + i as f32 * dx + if j % 2 != 0 { dx * 0.5 } else { 0.0 },
                lo.y + j as f32 * dy,
            );
            if sd_polygon(c, outline) > r * 0.6 {
                continue;
            }
            let corner = |k: usize, rr: f32| {
                let a = (k as f32 / 6.0 + 1.0 / 12.0) * std::f32::consts::TAU;
                c + Vec2::new(a.cos(), a.sin()) * rr
            };
            // Depth of each corner: outer ring 0..6, inner ring 6..12.
            let mut t = [0.0f32; 12];
            let mut ok = true;
            for (k, tk) in t.iter_mut().enumerate() {
                let q = corner(k % 6, if k < 6 { r } else { r - web });
                match face(q) {
                    Some(d) => *tk = d + inset,
                    None => ok = false,
                }
            }
            if !ok || (0..6).any(|k| f(place(corner(k, r), t[k])) > keep_inside) {
                continue;
            }
            let base = m.vertices.len() as u32;
            let back = -dir;
            let front = Vec4::new(0.7, 0.7, 0.7, 1.0);
            // Front web: outer ring and inner ring.
            for (k, &tk) in t.iter().enumerate() {
                let q = corner(k % 6, if k < 6 { r } else { r - web });
                m.vertices
                    .push(Vertex::new(place(q, tk), back, Vec2::ZERO).with_color(front));
            }
            for k in 0..6u32 {
                let k1 = (k + 1) % 6;
                let (a, b, cc, d) = (base + k, base + k1, base + 6 + k1, base + 6 + k);
                m.indices.extend_from_slice(&[a, b, cc, a, cc, d]);
            }
            // Cell walls into the depth.
            for k in 0..6usize {
                let k1 = (k + 1) % 6;
                let (p0, p1) = (corner(k, r - web), corner(k1, r - web));
                let (t0, t1) = (t[6 + k], t[6 + k1]);
                let mid = (p0 + p1) * 0.5;
                // Walls face the cell center (seen through the opening).
                let nrm = (place(c, t0) - place(mid, t0)).normalize_or(back);
                let wb = m.vertices.len() as u32;
                for (p, tt, shade) in [
                    (p0, t0, 0.7),
                    (p1, t1, 0.7),
                    (p1, t1 + depth, 0.2),
                    (p0, t0 + depth, 0.2),
                ] {
                    m.vertices.push(
                        Vertex::new(place(p, tt), nrm, Vec2::ZERO)
                            .with_color(Vec4::new(shade, shade, shade, 1.0)),
                    );
                }
                m.indices
                    .extend_from_slice(&[wb, wb + 1, wb + 2, wb, wb + 2, wb + 3]);
            }
        }
    }
    orient_like_normals(&mut m);
    m
}

/// Mesh a small closed part from its own field (cell `h`), simplified, with vertex colors
/// from `color(p, n)`. `mirror_x` for parts symmetric about x = 0 in their own frame.
pub fn sculpt(
    f: Field,
    min: Vec3,
    max: Vec3,
    h: f32,
    mirror_x: bool,
    tol: f32,
    color: &dyn Fn(Vec3, Vec3) -> Vec4,
) -> Mesh {
    let raw = mesher::surface_nets(f, min, max, h, mirror_x, 1.2);
    let raw = mesher::refine(&raw, f, 1, 30.0, 0.0004);
    let raw = mesher::decimate(&raw, tol, 22.0, 0.2, mirror_x, |_| 1.0);
    let mut m = Mesh::default();
    for (p, n) in raw.positions.iter().zip(&raw.normals) {
        m.vertices
            .push(Vertex::new(*p, *n, Vec2::new(p.z, -p.y)).with_color(color(*p, *n)));
    }
    m.indices = raw.indices;
    m
}

/// Points around a circle (radius `r`) in the plane spanned by `u`, `v` at `c`.
pub fn circle(c: Vec3, u: Vec3, v: Vec3, r: f32, n: usize) -> Vec<Vec3> {
    (0..n)
        .map(|k| {
            let a = k as f32 / n as f32 * std::f32::consts::TAU;
            c + (u * a.cos() + v * a.sin()) * r
        })
        .collect()
}

/// A ring-shaped loft (closed tube) around a circle: torus of ring radius `r`, tube `t`.
pub fn torus(c: Vec3, axis: Vec3, r: f32, t: f32, n: usize, m: usize) -> Mesh {
    let rot = Quat::from_rotation_arc(Vec3::X, axis.normalize());
    let mut rings = Vec::new();
    for k in 0..n {
        let a = k as f32 / n as f32 * std::f32::consts::TAU;
        let radial = rot * Vec3::new(0.0, a.cos(), a.sin());
        let ring = circle(c + radial * r, radial, rot * Vec3::X, t, m);
        rings.push(ring);
    }
    rings.push(rings[0].clone());
    let mut mesh = loft_rings(&rings, false);
    // Smooth normals from the ring geometry (the loft's seam would show otherwise).
    for v in &mut mesh.vertices {
        let p = Vec3::from(v.position) - c;
        let ax = rot * Vec3::X;
        let radial = (p - ax * p.dot(ax)).normalize_or(Vec3::Y);
        v.normal = (p - radial * r).normalize_or(radial).to_array();
    }
    orient_like_normals(&mut mesh);
    mesh
}
