//! Mesh building blocks for the proving ground's props.
//!
//! UVs are metric (1 UV unit = 1 m), so a material's `uv_scale` sets the texel density and
//! a texture keeps its scale across props of any size. Every shape can round or chamfer its
//! edges: real objects have no razor-sharp edges, and the highlight a bevel catches is much of
//! what makes a prop read as solid.
//!
//! Everything is built around the origin; [`MeshBuilder::placed`] moves a part into place.
//! Triangles face the way their vertex normals point, so shapes can list points in either
//! order.

use engine_core::math::{Mat3, Mat4, Vec2, Vec3, Vec4};
use engine_render::{Mesh, Vertex};

/// Linear RGBA from an sRGB hex color like `0xc9a227`.
pub fn srgb(hex: u32) -> Vec4 {
    let c = |shift: u32| {
        let s = ((hex >> shift) & 0xff) as f32 / 255.0;
        if s <= 0.04045 {
            s / 12.92
        } else {
            ((s + 0.055) / 1.055).powf(2.4)
        }
    };
    Vec4::new(c(16), c(8), c(0), 1.0)
}

/// Collects triangles with metric UVs and a current vertex color.
#[derive(Debug, Clone)]
pub struct MeshBuilder {
    pub mesh: Mesh,
    /// Linear RGBA given to new vertices (multiplied with the material's color).
    pub color: Vec4,
}

impl Default for MeshBuilder {
    fn default() -> Self {
        Self {
            mesh: Mesh::default(),
            color: Vec4::ONE,
        }
    }
}

impl MeshBuilder {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn build(self) -> Mesh {
        self.mesh
    }

    pub fn triangle_count(&self) -> usize {
        self.mesh.triangle_count()
    }

    /// Add a vertex with the current color; returns its index.
    pub fn vertex(&mut self, p: Vec3, n: Vec3, uv: Vec2) -> u32 {
        self.mesh
            .vertices
            .push(Vertex::new(p, n, uv).with_color(self.color));
        self.mesh.vertices.len() as u32 - 1
    }

    fn position(&self, i: u32) -> Vec3 {
        Vec3::from(self.mesh.vertices[i as usize].position)
    }

    fn normal(&self, i: u32) -> Vec3 {
        Vec3::from(self.mesh.vertices[i as usize].normal)
    }

    /// A triangle, wound so it faces the way its vertex normals point. Degenerate triangles
    /// (poles, collapsed rings) are dropped.
    pub fn tri(&mut self, a: u32, b: u32, c: u32) {
        let (pa, pb, pc) = (self.position(a), self.position(b), self.position(c));
        let face = (pb - pa).cross(pc - pa);
        if face.length_squared() < 1e-14 {
            return;
        }
        let n = self.normal(a) + self.normal(b) + self.normal(c);
        if face.dot(n) >= 0.0 {
            self.mesh.indices.extend_from_slice(&[a, b, c]);
        } else {
            self.mesh.indices.extend_from_slice(&[a, c, b]);
        }
    }

    /// A quad `a b c d` (around its edge, either direction), as two triangles.
    pub fn quad(&mut self, a: u32, b: u32, c: u32, d: u32) {
        // Split along the shorter diagonal: flatter, and never folds a convex quad.
        let ac = self.position(a).distance_squared(self.position(c));
        let bd = self.position(b).distance_squared(self.position(d));
        if ac <= bd {
            self.tri(a, b, c);
            self.tri(a, c, d);
        } else {
            self.tri(a, b, d);
            self.tri(b, c, d);
        }
    }

    /// A flat quad through four corners (around its edge), facing `normal`, with metric UVs
    /// projected along `u_axis` from `corners[0]`.
    pub fn flat_quad(&mut self, corners: [Vec3; 4], normal: Vec3, u_axis: Vec3) {
        let v_axis = normal.cross(u_axis).normalize_or(Vec3::Y);
        let uv = |p: Vec3| Vec2::new((p - corners[0]).dot(u_axis), (p - corners[0]).dot(v_axis));
        let i = corners.map(|p| self.vertex(p, normal, uv(p)));
        self.quad(i[0], i[1], i[2], i[3]);
    }

    /// A grid of `rows × cols` points (row-major) from `point(row, col) -> (position, normal,
    /// uv)`, joined into quads.
    pub fn grid(
        &mut self,
        rows: usize,
        cols: usize,
        mut point: impl FnMut(usize, usize) -> (Vec3, Vec3, Vec2),
    ) {
        let base = self.mesh.vertices.len() as u32;
        for r in 0..rows {
            for c in 0..cols {
                let (p, n, uv) = point(r, c);
                self.vertex(p, n, uv);
            }
        }
        let at = |r: usize, c: usize| base + (r * cols + c) as u32;
        for r in 0..rows.saturating_sub(1) {
            for c in 0..cols.saturating_sub(1) {
                self.quad(at(r, c), at(r, c + 1), at(r + 1, c + 1), at(r + 1, c));
            }
        }
    }

    /// Build a part, then move what it added by `m` (rotation and translation; UVs stay in
    /// the part's own meters).
    pub fn placed(&mut self, m: Mat4, build: impl FnOnce(&mut Self)) {
        let start = self.mesh.vertices.len();
        build(self);
        let nm = Mat3::from_mat4(m).inverse().transpose();
        for v in &mut self.mesh.vertices[start..] {
            v.position = m.transform_point3(Vec3::from(v.position)).to_array();
            v.normal = (nm * Vec3::from(v.normal)).normalize_or(Vec3::Y).to_array();
        }
    }

    /// Build a part in another vertex color.
    pub fn colored(&mut self, color: Vec4, build: impl FnOnce(&mut Self)) {
        let old = std::mem::replace(&mut self.color, color);
        build(self);
        self.color = old;
    }

    /// Append a finished mesh moved by `m`.
    pub fn append(&mut self, mesh: &Mesh, m: Mat4) {
        self.placed(m, |b| b.mesh.merge(mesh));
    }

    /// A box with rounded edges and corners (`radius`, `segments` steps per quarter round).
    /// `radius` = the smallest half extent makes a capsule-like or spherical shape.
    ///
    /// UVs are projected per face in meters, with U along the face's longer side: boards and
    /// beams get their grain along their length.
    pub fn rounded_box(&mut self, half: Vec3, radius: f32, segments: u32) {
        let radius = radius.clamp(0.0, half.min_element());
        let segments = if radius > 1e-5 { segments.max(1) } else { 0 };
        let inner = (half - Vec3::splat(radius)).max(Vec3::ZERO);
        let samples = |axis: usize| -> Vec<f32> {
            let (h, c) = (half[axis], inner[axis]);
            let mut s = Vec::new();
            // Edge strips cover 45° of the round each (the other half is on the next face).
            for k in 0..=segments {
                let theta = std::f32::consts::FRAC_PI_4 * (1.0 - k as f32 / segments.max(1) as f32);
                s.push(-c - radius * theta.tan());
            }
            if c > 1e-6 {
                for k in 0..=segments {
                    let theta = std::f32::consts::FRAC_PI_4 * k as f32 / segments.max(1) as f32;
                    s.push(c + radius * theta.tan());
                }
            } else {
                for k in 1..=segments {
                    let theta = std::f32::consts::FRAC_PI_4 * k as f32 / segments.max(1) as f32;
                    s.push(radius * theta.tan());
                }
            }
            if segments == 0 {
                s = vec![-h, h];
            }
            s
        };
        for axis in 0..3 {
            let (i, j) = ((axis + 1) % 3, (axis + 2) % 3);
            // U along the face's longer side.
            let (ua, va) = if half[i] >= half[j] { (i, j) } else { (j, i) };
            let (us, vs) = (samples(ua), samples(va));
            for sign in [-1.0f32, 1.0] {
                self.grid(vs.len(), us.len(), |r, c| {
                    let mut q = Vec3::ZERO;
                    q[axis] = sign * half[axis];
                    q[ua] = us[c];
                    q[va] = vs[r];
                    let core = q.clamp(-inner, inner);
                    let mut n = (q - core).normalize_or(Vec3::ZERO);
                    if n == Vec3::ZERO {
                        n[axis] = sign;
                    }
                    let p = if radius > 0.0 { core + n * radius } else { q };
                    let mut face_n = n;
                    if radius == 0.0 {
                        face_n = Vec3::ZERO;
                        face_n[axis] = sign;
                    }
                    (p, face_n, Vec2::new(p[ua] * sign, p[va]))
                });
            }
        }
    }

    /// A sphere (a rounded box that is all round), `segments` steps per 45°.
    pub fn sphere(&mut self, radius: f32, segments: u32) {
        self.rounded_box(Vec3::splat(radius), radius, segments);
    }

    /// Revolve a profile of `(radius, height)` points around +Y with `sides` steps.
    ///
    /// Walk the profile from the bottom of the axis to the top with the outside on your right
    /// (bottom center → rim → wall → top center). Corners turning more than `crease_deg` get
    /// a hard edge; gentler turns are smooth. U runs around at the largest radius in meters,
    /// V along the profile.
    pub fn lathe(&mut self, profile: &[Vec2], sides: u32, crease_deg: f32) {
        let ring = profile_normals(profile, crease_deg);
        let sides = sides.max(3) as usize;
        let r_max = profile.iter().map(|p| p.x).fold(0.0f32, f32::max);
        let around = std::f32::consts::TAU * r_max;
        // Each run of points without a crease is one smooth grid.
        for run in ring {
            self.grid(run.len(), sides + 1, |r, c| {
                let (p, n, v) = run[r];
                let a = c as f32 / sides as f32 * std::f32::consts::TAU;
                let (s, co) = a.sin_cos();
                (
                    Vec3::new(p.x * co, p.y, -p.x * s),
                    Vec3::new(n.x * co, n.y, -n.x * s),
                    Vec2::new(c as f32 / sides as f32 * around, v),
                )
            });
        }
    }

    /// A cylinder along Y with rounded rims (`bevel` meters, 0 for sharp).
    pub fn cylinder(&mut self, radius: f32, half_height: f32, sides: u32, bevel: f32) {
        let b = bevel.clamp(0.0, radius.min(half_height) * 0.99);
        let mut profile = vec![Vec2::new(0.0, -half_height)];
        profile.extend(round_corner(
            Vec2::new(radius, -half_height),
            Vec2::new(-1.0, 0.0),
            Vec2::new(0.0, 1.0),
            b,
            3,
        ));
        profile.extend(round_corner(
            Vec2::new(radius, half_height),
            Vec2::new(0.0, -1.0),
            Vec2::new(-1.0, 0.0),
            b,
            3,
        ));
        profile.push(Vec2::new(0.0, half_height));
        self.lathe(&profile, sides, 40.0);
    }

    /// A tube of `radius` along a path (parallel-transported frames, so it never twists).
    /// Open tubes get flat end caps when `caps` is set.
    pub fn tube(&mut self, path: &[Vec3], radius: f32, sides: u32, closed: bool, caps: bool) {
        self.tube_with(path, &vec![radius; path.len()], sides, closed, caps);
    }

    /// [`tube`](Self::tube) with a radius per path point.
    pub fn tube_with(
        &mut self,
        path: &[Vec3],
        radii: &[f32],
        sides: u32,
        closed: bool,
        caps: bool,
    ) {
        let n = path.len();
        if n < 2 {
            return;
        }
        let sides = sides.max(3) as usize;
        let tangent = |i: usize| -> Vec3 {
            let prev = if i > 0 {
                path[i] - path[i - 1]
            } else if closed {
                path[0] - path[n - 1]
            } else {
                Vec3::ZERO
            };
            let next = if i + 1 < n {
                path[i + 1] - path[i]
            } else if closed {
                path[0] - path[n - 1]
            } else {
                Vec3::ZERO
            };
            (prev.normalize_or(Vec3::ZERO) + next.normalize_or(Vec3::ZERO)).normalize_or(Vec3::Y)
        };
        let t0 = tangent(0);
        let mut normal = t0.any_orthonormal_vector();
        let mut frames = Vec::with_capacity(n + 1);
        let mut along = 0.0;
        let mut prev_t = t0;
        let count = if closed { n + 1 } else { n };
        for k in 0..count {
            let i = k % n;
            let t = tangent(i);
            // Parallel transport: rotate the normal by the change in tangent.
            let q = engine_core::math::Quat::from_rotation_arc(prev_t, t);
            normal = (q * normal).reject_from(t).normalize_or(normal);
            prev_t = t;
            if k > 0 {
                along += path[i].distance(path[(k - 1) % n]);
            }
            frames.push((path[i], t, normal, t.cross(normal), radii[i], along));
        }
        let r_max = radii.iter().copied().fold(0.0f32, f32::max);
        let around = std::f32::consts::TAU * r_max;
        self.grid(frames.len(), sides + 1, |r, c| {
            let (p, _, nn, bn, rad, v) = frames[r];
            let a = c as f32 / sides as f32 * std::f32::consts::TAU;
            let dir = nn * a.cos() + bn * a.sin();
            (
                p + dir * rad,
                dir,
                Vec2::new(c as f32 / sides as f32 * around, v),
            )
        });
        if caps && !closed {
            for (i, sign) in [(0usize, -1.0f32), (n - 1, 1.0)] {
                let (p, t, nn, bn, rad, _) = frames[i];
                let face = t * sign;
                let center = self.vertex(p, face, Vec2::ZERO);
                let rim: Vec<u32> = (0..=sides)
                    .map(|c| {
                        let a = c as f32 / sides as f32 * std::f32::consts::TAU;
                        let off = (nn * a.cos() + bn * a.sin()) * rad;
                        self.vertex(p + off, face, Vec2::new(off.dot(nn), off.dot(bn)))
                    })
                    .collect();
                for w in rim.windows(2) {
                    self.tri(center, w[0], w[1]);
                }
            }
        }
    }

    /// A chain link: a round wire bent into a stadium in the XY plane, long axis Y.
    /// `length` and `width` are measured along the wire's center line.
    pub fn chain_link(&mut self, length: f32, width: f32, wire: f32, sides: u32, segments: u32) {
        let r = width * 0.5;
        let straight = (length - width).max(0.0) * 0.5;
        let segments = segments.max(2) as usize;
        let mut path = Vec::new();
        for (cy, start) in [(straight, 0.0f32), (-straight, std::f32::consts::PI)] {
            for k in 0..=segments {
                let a = start + std::f32::consts::PI * k as f32 / segments as f32;
                path.push(Vec3::new(r * a.cos(), cy + r * a.sin(), 0.0));
            }
        }
        self.tube(&path, wire, sides, true, false);
    }

    /// A prism: the polygon `outline` (XY, either winding, may be concave) extruded along Z
    /// from `-depth / 2` to `depth / 2`, with its cap edges chamfered by `bevel`. Outline
    /// corners turning less than `crease_deg` are smooth (for curved outlines).
    pub fn extrude(&mut self, outline: &[Vec2], depth: f32, bevel: f32, crease_deg: f32) {
        let mut pts: Vec<Vec2> = outline.to_vec();
        if signed_area(&pts) < 0.0 {
            pts.reverse();
        }
        let n = pts.len();
        if n < 3 {
            return;
        }
        let hz = depth * 0.5;
        let bevel = bevel.clamp(0.0, hz * 0.99);
        let inset = inset_polygon(&pts, bevel);
        // Side walls (between the chamfers), smooth across gentle corners.
        // Outward edge normals scaled by edge length (length-weighted smoothing).
        let edge_n = |i: usize| -> Vec2 {
            let d = pts[(i + 1) % n] - pts[i];
            Vec2::new(d.y, -d.x)
        };
        let cos_crease = crease_deg.to_radians().cos();
        let mut along = 0.0;
        for i in 0..n {
            let j = (i + 1) % n;
            let en = edge_n(i);
            let smooth_with = |other: usize| -> Vec2 {
                let o = edge_n(other);
                if en.normalize_or(Vec2::X).dot(o.normalize_or(Vec2::X)) >= cos_crease {
                    (en + o).normalize_or(Vec2::X)
                } else {
                    en.normalize_or(Vec2::X)
                }
            };
            let na = smooth_with((i + n - 1) % n);
            let nb = smooth_with(j);
            let len = pts[i].distance(pts[j]);
            let zs = [-hz + bevel, hz - bevel];
            let a0 = self.vertex(
                pts[i].extend(zs[0]),
                na.extend(0.0),
                Vec2::new(along, zs[0]),
            );
            let b0 = self.vertex(
                pts[j].extend(zs[0]),
                nb.extend(0.0),
                Vec2::new(along + len, zs[0]),
            );
            let b1 = self.vertex(
                pts[j].extend(zs[1]),
                nb.extend(0.0),
                Vec2::new(along + len, zs[1]),
            );
            let a1 = self.vertex(
                pts[i].extend(zs[1]),
                na.extend(0.0),
                Vec2::new(along, zs[1]),
            );
            self.quad(a0, b0, b1, a1);
            // Chamfers to the inset caps.
            if bevel > 0.0 {
                for (z_side, z_cap, s) in [(zs[0], -hz, -1.0f32), (zs[1], hz, 1.0)] {
                    let cn = |n2: Vec2| (n2.extend(0.0) + Vec3::Z * s).normalize();
                    let p = self.vertex(pts[i].extend(z_side), cn(na), Vec2::new(along, z_side));
                    let q = self.vertex(
                        pts[j].extend(z_side),
                        cn(nb),
                        Vec2::new(along + len, z_side),
                    );
                    let qi = self.vertex(
                        inset[j].extend(z_cap),
                        cn(nb),
                        Vec2::new(along + len, z_cap + s * bevel),
                    );
                    let pi = self.vertex(
                        inset[i].extend(z_cap),
                        cn(na),
                        Vec2::new(along, z_cap + s * bevel),
                    );
                    self.quad(p, q, qi, pi);
                }
            }
            along += len;
        }
        // Caps.
        let tris = triangulate(&inset);
        for (z, nz) in [(-hz, -1.0f32), (hz, 1.0)] {
            let base = self.mesh.vertices.len() as u32;
            for p in &inset {
                self.vertex(p.extend(z), Vec3::Z * nz, *p);
            }
            for t in &tris {
                self.tri(base + t[0], base + t[1], base + t[2]);
            }
        }
    }
}

/// Points rounding the corner at `corner` between an incoming edge running along `-from`
/// and an outgoing edge along `to`, with `radius` and `steps` steps (just the corner when
/// `radius` is 0).
pub fn round_corner(corner: Vec2, from: Vec2, to: Vec2, radius: f32, steps: u32) -> Vec<Vec2> {
    if radius <= 0.0 {
        return vec![corner];
    }
    let (a, b) = (corner + from * radius, corner + to * radius);
    // Quadratic Bézier through the corner: close to a circular arc for right angles.
    let steps = steps.max(1);
    (0..=steps)
        .map(|k| {
            let t = k as f32 / steps as f32;
            a * (1.0 - t) * (1.0 - t) + corner * 2.0 * t * (1.0 - t) + b * t * t
        })
        .collect()
}

/// Per-run lathe points `(point, normal, v)`: a new run starts at each crease.
fn profile_normals(profile: &[Vec2], crease_deg: f32) -> Vec<Vec<(Vec2, Vec2, f32)>> {
    let n = profile.len();
    if n < 2 {
        return Vec::new();
    }
    // Length-weighted normals: a long flat face stays flat right up to a small bevel.
    let seg_n: Vec<Vec2> = profile
        .windows(2)
        .map(|w| {
            let d = w[1] - w[0];
            Vec2::new(d.y, -d.x)
        })
        .collect();
    let cos_crease = crease_deg.to_radians().cos();
    let mut runs = vec![Vec::new()];
    let mut v = 0.0;
    for i in 0..n {
        if i > 0 {
            v += profile[i].distance(profile[i - 1]);
        }
        let before = if i > 0 { Some(seg_n[i - 1]) } else { None };
        let after = seg_n.get(i).copied();
        match (before, after) {
            (Some(a), Some(b))
                if a != Vec2::ZERO
                    && b != Vec2::ZERO
                    && a.normalize().dot(b.normalize()) < cos_crease =>
            {
                runs.last_mut()
                    .unwrap()
                    .push((profile[i], a.normalize(), v));
                runs.push(vec![(profile[i], b.normalize(), v)]);
            }
            _ => {
                let s = before.unwrap_or(Vec2::ZERO) + after.unwrap_or(Vec2::ZERO);
                runs.last_mut()
                    .unwrap()
                    .push((profile[i], s.normalize_or(Vec2::Y), v));
            }
        }
    }
    runs.retain(|r| r.len() >= 2);
    runs
}

fn signed_area(p: &[Vec2]) -> f32 {
    let n = p.len();
    (0..n).map(|i| p[i].perp_dot(p[(i + 1) % n])).sum::<f32>() * 0.5
}

/// Move every edge of a counter-clockwise polygon inward by `d` (mitered corners).
fn inset_polygon(p: &[Vec2], d: f32) -> Vec<Vec2> {
    if d <= 0.0 {
        return p.to_vec();
    }
    let n = p.len();
    (0..n)
        .map(|i| {
            let prev = (p[i] - p[(i + n - 1) % n]).normalize_or(Vec2::X);
            let next = (p[(i + 1) % n] - p[i]).normalize_or(Vec2::X);
            // Inward normals of a CCW polygon are the left perpendiculars.
            let (na, nb) = (prev.perp(), next.perp());
            let bis = (na + nb).normalize_or(na);
            let cos_half = bis.dot(na).max(0.2);
            p[i] + bis * (d / cos_half)
        })
        .collect()
}

/// Ear-clipping triangulation of a simple counter-clockwise polygon.
fn triangulate(p: &[Vec2]) -> Vec<[u32; 3]> {
    let mut idx: Vec<usize> = (0..p.len()).collect();
    let mut out = Vec::new();
    // Points on the candidate ear's boundary block it too (they would touch its diagonal).
    let inside = |a: Vec2, b: Vec2, c: Vec2, q: Vec2| {
        let d1 = (b - a).perp_dot(q - a);
        let d2 = (c - b).perp_dot(q - b);
        let d3 = (a - c).perp_dot(q - c);
        d1 >= 0.0 && d2 >= 0.0 && d3 >= 0.0
    };
    while idx.len() > 3 {
        let m = idx.len();
        let mut clipped = false;
        for k in 0..m {
            let (ia, ib, ic) = (idx[(k + m - 1) % m], idx[k], idx[(k + 1) % m]);
            let (a, b, c) = (p[ia], p[ib], p[ic]);
            let turn = (b - a).perp_dot(c - b);
            if turn.abs() <= 1e-12 {
                // Collinear: drop the middle point, no triangle.
                idx.remove(k);
                clipped = true;
                break;
            }
            if turn < 0.0 {
                continue;
            }
            let blocked = idx.iter().any(|&o| {
                let q = p[o];
                q != a && q != b && q != c && inside(a, b, c, q)
            });
            if blocked {
                continue;
            }
            out.push([ia as u32, ib as u32, ic as u32]);
            idx.remove(k);
            clipped = true;
            break;
        }
        if !clipped {
            break;
        }
    }
    if idx.len() == 3 {
        out.push([idx[0] as u32, idx[1] as u32, idx[2] as u32]);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn closed(mesh: &Mesh) -> bool {
        // Every triangle faces away from the centroid of a convex shape.
        let c = mesh
            .vertices
            .iter()
            .map(|v| Vec3::from(v.position))
            .sum::<Vec3>()
            / mesh.vertices.len() as f32;
        mesh.indices.as_chunks::<3>().0.iter().all(|t| {
            let p = t.map(|i| Vec3::from(mesh.vertices[i as usize].position));
            let n = (p[1] - p[0]).cross(p[2] - p[0]);
            n.dot((p[0] + p[1] + p[2]) / 3.0 - c) > -1e-6
        })
    }

    #[test]
    fn shapes_face_outward_and_stay_in_bounds() {
        let mut b = MeshBuilder::new();
        b.rounded_box(Vec3::new(0.5, 0.3, 0.2), 0.05, 2);
        let m = b.build();
        assert!(closed(&m));
        let bb = m.aabb();
        assert!(
            (bb.max - Vec3::new(0.5, 0.3, 0.2))
                .abs()
                .max_element()
                < 1e-5
        );

        let mut b = MeshBuilder::new();
        b.sphere(0.4, 3);
        let m = b.build();
        assert!(closed(&m));
        assert!(
            m.vertices
                .iter()
                .all(|v| (Vec3::from(v.position).length() - 0.4).abs() < 1e-5)
        );

        let mut b = MeshBuilder::new();
        b.cylinder(0.3, 0.42, 24, 0.02);
        let m = b.build();
        assert!(closed(&m));
        let bb = m.aabb();
        assert!((bb.max.y - 0.42).abs() < 1e-5 && (bb.max.x - 0.3).abs() < 1e-3);

        let mut b = MeshBuilder::new();
        b.extrude(
            &[
                Vec2::new(-1.0, -1.0),
                Vec2::new(1.0, -1.0),
                Vec2::new(1.0, 1.0),
                Vec2::new(-1.0, 1.0),
            ],
            0.5,
            0.05,
            30.0,
        );
        let m = b.build();
        assert!(closed(&m));
        assert_eq!(m.aabb().max.z, 0.25);
    }

    #[test]
    fn concave_outlines_triangulate() {
        // An I-beam section.
        let (w, h, t, f) = (0.3f32, 0.5f32, 0.02f32, 0.03f32);
        let outline = [
            Vec2::new(-w, -h),
            Vec2::new(w, -h),
            Vec2::new(w, -h + f),
            Vec2::new(t, -h + f),
            Vec2::new(t, h - f),
            Vec2::new(w, h - f),
            Vec2::new(w, h),
            Vec2::new(-w, h),
            Vec2::new(-w, h - f),
            Vec2::new(-t, h - f),
            Vec2::new(-t, -h + f),
            Vec2::new(-w, -h + f),
        ];
        let tris = triangulate(&outline);
        let area: f32 = tris
            .iter()
            .map(|t| {
                let [a, b, c] = t.map(|i| outline[i as usize]);
                (b - a).perp_dot(c - a) * 0.5
            })
            .sum();
        assert!((area - signed_area(&outline)).abs() < 1e-5);
    }
}
