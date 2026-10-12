//! Mesh building blocks for the scenery: quads that fix their own winding, boxes with world
//! scale UVs, chamfered and rounded boxes, profiles swept along paths or turned on a lathe,
//! cylinders, spheres, tori and discs.
//! Every vertex carries the builder's current color (linear RGBA), which multiplies the
//! material's albedo: paint colors, per-panel tints and baked contact shadows live there.

use engine_core::math::{Mat4, Quat, Vec2, Vec3, Vec4};
use engine_render::{Mesh, Vertex};

/// Accumulates triangles in one [`Mesh`].
#[derive(Default, Clone)]
pub struct Builder {
    pub mesh: Mesh,
    /// Color given to new vertices (linear RGBA).
    pub color: Vec4,
}

/// A point on a swept path: position, the profile's x (lateral) and y (up) axes, and the
/// distance along the path (meters, used as a texture coordinate).
#[derive(Clone, Copy, Debug)]
pub struct Frame {
    pub pos: Vec3,
    pub side: Vec3,
    pub up: Vec3,
    pub s: f32,
}

impl Frame {
    /// A frame at `pos` heading along `dir` (horizontal), up = +Y, side = right of travel.
    pub fn flat(pos: Vec3, dir: Vec3, s: f32) -> Self {
        let d = Vec3::new(dir.x, 0.0, dir.z).normalize_or(Vec3::NEG_Z);
        Self {
            pos,
            side: d.cross(Vec3::Y).normalize(),
            up: Vec3::Y,
            s,
        }
    }

    /// The world position of profile point `p` (x along `side`, y along `up`).
    pub fn at(&self, p: Vec2) -> Vec3 {
        self.pos + self.side * p.x + self.up * p.y
    }
}

/// Frames along a polyline (horizontal headings), `s` accumulating from `s0`.
pub fn frames_along(points: &[Vec3], s0: f32) -> Vec<Frame> {
    let n = points.len();
    let mut out = Vec::with_capacity(n);
    let mut s = s0;
    for i in 0..n {
        let a = points[i.saturating_sub(1)];
        let b = points[(i + 1).min(n - 1)];
        if i > 0 {
            s += (points[i] - points[i - 1]).length();
        }
        out.push(Frame::flat(points[i], b - a, s));
    }
    out
}

/// Linear RGBA from an sRGB hex color.
pub fn hex(rgb: u32) -> Vec4 {
    let c = engine_render::material::srgb_hex_to_linear(rgb);
    Vec4::new(c[0], c[1], c[2], 1.0)
}

/// Gray (linear) with alpha 1.
pub fn gray(v: f32) -> Vec4 {
    Vec4::new(v, v, v, 1.0)
}

impl Builder {
    pub fn new() -> Self {
        Self {
            mesh: Mesh::default(),
            color: Vec4::ONE,
        }
    }

    /// Set the color for the next vertices.
    pub fn color(&mut self, c: Vec4) -> &mut Self {
        self.color = c;
        self
    }

    pub fn vertex(&mut self, p: Vec3, n: Vec3, uv: Vec2) -> u32 {
        let i = self.mesh.vertices.len() as u32;
        self.mesh
            .vertices
            .push(Vertex::new(p, n, uv).with_color(self.color));
        i
    }

    pub fn vertex_c(&mut self, p: Vec3, n: Vec3, uv: Vec2, c: Vec4) -> u32 {
        let i = self.mesh.vertices.len() as u32;
        self.mesh.vertices.push(Vertex::new(p, n, uv).with_color(c));
        i
    }

    /// A triangle of existing vertices, wound so its face agrees with their normals.
    pub fn tri_idx(&mut self, a: u32, b: u32, c: u32) {
        let v = &self.mesh.vertices;
        let pa = Vec3::from(v[a as usize].position);
        let pb = Vec3::from(v[b as usize].position);
        let pc = Vec3::from(v[c as usize].position);
        let n = Vec3::from(v[a as usize].normal)
            + Vec3::from(v[b as usize].normal)
            + Vec3::from(v[c as usize].normal);
        if (pb - pa).cross(pc - pa).dot(n) >= 0.0 {
            self.mesh.indices.extend_from_slice(&[a, b, c]);
        } else {
            self.mesh.indices.extend_from_slice(&[a, c, b]);
        }
    }

    /// Two triangles over four existing vertices in ring order.
    pub fn quad_idx(&mut self, a: u32, b: u32, c: u32, d: u32) {
        self.tri_idx(a, b, c);
        self.tri_idx(a, c, d);
    }

    /// A flat quad (corners in ring order) facing `n` (winding fixed to match).
    pub fn quad_n(&mut self, p: [Vec3; 4], uv: [Vec2; 4], n: Vec3) {
        let i: Vec<u32> = (0..4).map(|k| self.vertex(p[k], n, uv[k])).collect();
        self.quad_idx(i[0], i[1], i[2], i[3]);
    }

    /// A flat quad with its normal from the corners (ring order, counter-clockwise seen from
    /// the front).
    pub fn quad(&mut self, p: [Vec3; 4], uv: [Vec2; 4]) {
        let n = (p[1] - p[0])
            .cross(p[2] - p[0])
            .normalize_or((p[2] - p[0]).cross(p[3] - p[0]).normalize_or(Vec3::Y));
        self.quad_n(p, uv, n);
    }

    /// A flat quad with UVs from world position projected on the quad's own axes (meters).
    pub fn quad_world(&mut self, p: [Vec3; 4]) {
        let n = (p[1] - p[0]).cross(p[2] - p[0]).normalize_or(Vec3::Y);
        let (u, v) = basis(n);
        let uv = p.map(|q| Vec2::new(q.dot(u), q.dot(v)));
        self.quad_n(p, uv, n);
    }

    /// A flat triangle (counter-clockwise from the front) with world-projected UVs.
    pub fn tri_world(&mut self, p: [Vec3; 3]) {
        let n = (p[1] - p[0]).cross(p[2] - p[0]).normalize_or(Vec3::Y);
        let (u, v) = basis(n);
        let a = self.vertex(p[0], n, Vec2::new(p[0].dot(u), p[0].dot(v)));
        let b = self.vertex(p[1], n, Vec2::new(p[1].dot(u), p[1].dot(v)));
        let c = self.vertex(p[2], n, Vec2::new(p[2].dot(u), p[2].dot(v)));
        self.mesh.indices.extend_from_slice(&[a, b, c]);
    }

    /// An axis-aligned box (rotated by `rot` about its center), UVs in meters per face.
    pub fn cuboid(&mut self, center: Vec3, half: Vec3, rot: Quat) {
        self.chamfer_box(center, half, 0.0, rot);
    }

    /// A box with chamfered edges `bevel` meters wide (0 = sharp). Faces, edge strips and
    /// corner triangles each get their own flat normal, so the edges catch the light.
    pub fn chamfer_box(&mut self, center: Vec3, half: Vec3, bevel: f32, rot: Quat) {
        let b = bevel.min(half.min_element() * 0.95).max(0.0);
        let inner = half - Vec3::splat(b);
        let p = |s: Vec3, push: Vec3| center + rot * (inner * s + push * b);
        let axes = [Vec3::X, Vec3::Y, Vec3::Z];
        // Faces.
        for (ai, a) in axes.iter().enumerate() {
            for sign in [-1.0f32, 1.0] {
                let n = *a * sign;
                let u = axes[(ai + 1) % 3];
                let v = axes[(ai + 2) % 3];
                let corners = [(-1.0, -1.0), (1.0, -1.0), (1.0, 1.0), (-1.0, 1.0)]
                    .map(|(x, y)| p(n + u * x + v * y, n));
                let wn = rot * n;
                let (bu, bv) = basis(wn);
                let uv = corners.map(|q| Vec2::new(q.dot(bu), q.dot(bv)));
                self.quad_n(corners, uv, wn);
            }
        }
        if b <= 0.0 {
            return;
        }
        // Edge strips: for each pair of axes, four edges.
        for (ai, a) in axes.iter().enumerate() {
            let (b1, b2) = (axes[(ai + 1) % 3], axes[(ai + 2) % 3]);
            for s1 in [-1.0f32, 1.0] {
                for s2 in [-1.0f32, 1.0] {
                    let base = b1 * s1 + b2 * s2;
                    let q = [
                        p(base - *a, b1 * s1),
                        p(base + *a, b1 * s1),
                        p(base + *a, b2 * s2),
                        p(base - *a, b2 * s2),
                    ];
                    let n = rot * (b1 * s1 + b2 * s2).normalize();
                    let (bu, bv) = basis(n);
                    let uv = q.map(|x| Vec2::new(x.dot(bu), x.dot(bv)));
                    self.quad_n(q, uv, n);
                }
            }
        }
        // Corner triangles.
        for sx in [-1.0f32, 1.0] {
            for sy in [-1.0f32, 1.0] {
                for sz in [-1.0f32, 1.0] {
                    let c = Vec3::new(sx, sy, sz);
                    let t = [p(c, Vec3::X * sx), p(c, Vec3::Y * sy), p(c, Vec3::Z * sz)];
                    let n = rot * c.normalize();
                    let i: Vec<u32> = t.iter().map(|&x| self.vertex(x, n, Vec2::ZERO)).collect();
                    self.tri_idx(i[0], i[1], i[2]);
                }
            }
        }
    }

    /// A box with every edge and corner rounded to `radius`, `k` segments per 45° of round
    /// (smooth normals, UVs in meters per face). Each face grid runs its flat span and then
    /// half of each neighboring round, sampled at even angles.
    pub fn rounded_box(&mut self, center: Vec3, half: Vec3, radius: f32, k: u32, rot: Quat) {
        let r = radius.min(half.min_element()).max(1e-4);
        let inner = half - Vec3::splat(r);
        let k = k.max(1);
        let coords = |ih: f32| -> Vec<f32> {
            let band = |i: u32| r * (i as f32 / k as f32 * std::f32::consts::FRAC_PI_4).tan();
            let mut v: Vec<f32> = (0..=k).rev().map(|i| -ih - band(i)).collect();
            v.extend((0..=k).map(|i| ih + band(i)));
            v
        };
        let axes = [Vec3::X, Vec3::Y, Vec3::Z];
        for ai in 0..3 {
            let (u, v) = (axes[(ai + 1) % 3], axes[(ai + 2) % 3]);
            let (cu, cv) = (coords(inner.dot(u)), coords(inner.dot(v)));
            for sign in [-1.0f32, 1.0] {
                let face = axes[ai] * sign * half.dot(axes[ai]);
                let base = self.mesh.vertices.len() as u32;
                for &a in &cu {
                    for &b in &cv {
                        let g = face + u * a + v * b;
                        let core = g.clamp(-inner, inner);
                        let n = (g - core).normalize_or(axes[ai] * sign);
                        let q = core + n * r;
                        let uv = Vec2::new(q.dot(u), q.dot(v));
                        self.vertex(center + rot * q, rot * n, uv);
                    }
                }
                let row = cv.len() as u32;
                for i in 0..cu.len() as u32 - 1 {
                    for j in 0..row - 1 {
                        let q = base + i * row + j;
                        self.quad_idx(q, q + row, q + row + 1, q + 1);
                    }
                }
            }
        }
    }

    /// A surface of revolution around `axis` through `c`: `profile` holds (radius, offset
    /// along the axis) points. Faces point to the left of the profile's direction of travel
    /// in the (radius, offset) plane, so a profile walked from +offset to −offset over the
    /// rim faces outward. `sharp` marks creases, as for [`Builder::sweep`].
    pub fn lathe(&mut self, c: Vec3, axis: Vec3, profile: &[Vec2], sharp: &[bool], seg: u32) {
        let axis = axis.normalize();
        let (x, y) = basis(axis);
        let r = profile.iter().fold(0.0f32, |m, p| m.max(p.x));
        let frames: Vec<Frame> = (0..=seg)
            .map(|i| {
                let t = i as f32 / seg as f32 * std::f32::consts::TAU;
                Frame {
                    pos: c,
                    side: x * t.cos() + y * t.sin(),
                    up: axis,
                    s: t * r,
                }
            })
            .collect();
        self.sweep(profile, sharp, &frames, false);
    }

    /// A box between two points (a beam, post or brace) with a square section `w` × `h`
    /// (`h` along the beam's "up", which is +Y unless the beam is vertical).
    pub fn beam(&mut self, a: Vec3, b: Vec3, w: f32, h: f32, bevel: f32) {
        let d = b - a;
        let len = d.length();
        if len < 1e-5 {
            return;
        }
        let dir = d / len;
        let rot = Quat::from_rotation_arc(Vec3::Z, dir);
        // Keep the section's "up" as close to world up as possible.
        let up_now = rot * Vec3::Y;
        let want = (Vec3::Y - dir * Vec3::Y.dot(dir)).normalize_or(rot * Vec3::Y);
        let twist = Quat::from_rotation_arc(up_now, want);
        let rot = if (twist * dir - dir).length() < 1e-3 {
            twist * rot
        } else {
            rot
        };
        self.chamfer_box(
            (a + b) * 0.5,
            Vec3::new(w * 0.5, h * 0.5, len * 0.5),
            bevel,
            rot,
        );
    }

    /// A cylinder (or cone/frustum) from `a` (radius `ra`) to `b` (radius `rb`) with `seg`
    /// sides, optional caps. UVs: u = arc length (m), v = distance along (m).
    pub fn cylinder(&mut self, a: Vec3, b: Vec3, ra: f32, rb: f32, seg: u32, caps: bool) {
        let d = b - a;
        let len = d.length();
        if len < 1e-6 {
            return;
        }
        let axis = d / len;
        let (x, y) = basis(axis);
        let slope = (ra - rb) / len;
        let base = self.mesh.vertices.len() as u32;
        let r_avg = (ra + rb) * 0.5;
        for i in 0..=seg {
            let t = i as f32 / seg as f32 * std::f32::consts::TAU;
            let radial = x * t.cos() + y * t.sin();
            let n = (radial + axis * slope).normalize();
            let u = t * r_avg;
            self.vertex(a + radial * ra, n, Vec2::new(u, 0.0));
            self.vertex(b + radial * rb, n, Vec2::new(u, len));
        }
        for i in 0..seg {
            let k = base + i * 2;
            self.quad_idx(k, k + 2, k + 3, k + 1);
        }
        if caps {
            for (c, r, n) in [(a, ra, -axis), (b, rb, axis)] {
                if r <= 0.0 {
                    continue;
                }
                let center = self.vertex(c, n, Vec2::ZERO);
                let first = self.mesh.vertices.len() as u32;
                for i in 0..=seg {
                    let t = i as f32 / seg as f32 * std::f32::consts::TAU;
                    let radial = x * t.cos() + y * t.sin();
                    self.vertex(c + radial * r, n, Vec2::new(t.cos() * r, t.sin() * r));
                }
                for i in 0..seg {
                    self.tri_idx(center, first + i, first + i + 1);
                }
            }
        }
    }

    /// A UV sphere (or a part of it: `lat` 0..1 from the top pole down).
    pub fn sphere(&mut self, c: Vec3, r: f32, seg: u32, lat: f32) {
        let rings = (seg / 2).max(2);
        let base = self.mesh.vertices.len() as u32;
        for j in 0..=rings {
            let phi = j as f32 / rings as f32 * std::f32::consts::PI * lat;
            for i in 0..=seg {
                let t = i as f32 / seg as f32 * std::f32::consts::TAU;
                let n = Vec3::new(phi.sin() * t.cos(), phi.cos(), phi.sin() * t.sin());
                self.vertex(c + n * r, n, Vec2::new(t * r, phi * r));
            }
        }
        let row = seg + 1;
        for j in 0..rings {
            for i in 0..seg {
                let k = base + j * row + i;
                self.quad_idx(k, k + 1, k + row + 1, k + row);
            }
        }
    }

    /// A torus around `axis` through `c`.
    pub fn torus(&mut self, c: Vec3, axis: Vec3, radius: f32, tube: f32, seg: u32, sides: u32) {
        let (x, y) = basis(axis);
        let base = self.mesh.vertices.len() as u32;
        for i in 0..=seg {
            let a = i as f32 / seg as f32 * std::f32::consts::TAU;
            let radial = x * a.cos() + y * a.sin();
            for j in 0..=sides {
                let b = j as f32 / sides as f32 * std::f32::consts::TAU;
                let n = radial * b.cos() + axis * b.sin();
                self.vertex(
                    c + radial * radius + n * tube,
                    n,
                    Vec2::new(a * radius, b * tube),
                );
            }
        }
        let row = sides + 1;
        for i in 0..seg {
            for j in 0..sides {
                let k = base + i * row + j;
                self.quad_idx(k, k + row, k + row + 1, k + 1);
            }
        }
    }

    /// A flat ring (or disc when `r0` is 0) facing `n` around `c`, UVs = world-ish meters.
    pub fn annulus(&mut self, c: Vec3, n: Vec3, r0: f32, r1: f32, seg: u32) {
        let (x, y) = basis(n);
        let base = self.mesh.vertices.len() as u32;
        for i in 0..=seg {
            let t = i as f32 / seg as f32 * std::f32::consts::TAU;
            let radial = x * t.cos() + y * t.sin();
            let p0 = c + radial * r0;
            let p1 = c + radial * r1;
            self.vertex(p0, n, Vec2::new(p0.dot(x), p0.dot(y)));
            self.vertex(p1, n, Vec2::new(p1.dot(x), p1.dot(y)));
        }
        for i in 0..seg {
            let k = base + i * 2;
            self.quad_idx(k, k + 2, k + 3, k + 1);
        }
    }

    /// Sweep a 2D `profile` (x = lateral along the frame's side, y = up) along `frames`.
    /// `sharp` marks profile points where the surface creases (their neighbors get separate
    /// normals). UVs: u = distance along the path, v = distance along the profile (meters),
    /// swapped when `swap_uv`. Faces point to the left of the profile's direction of travel
    /// (counter-clockwise profiles face outward).
    pub fn sweep(&mut self, profile: &[Vec2], sharp: &[bool], frames: &[Frame], swap_uv: bool) {
        if profile.len() < 2 || frames.len() < 2 {
            return;
        }
        let segs = profile.len() - 1;
        // Per-segment 2D normals (left of travel).
        let seg_n: Vec<Vec2> = (0..segs)
            .map(|i| {
                let d = (profile[i + 1] - profile[i]).normalize_or(Vec2::X);
                Vec2::new(-d.y, d.x)
            })
            .collect();
        let mut vlen = vec![0.0f32; profile.len()];
        for i in 1..profile.len() {
            vlen[i] = vlen[i - 1] + (profile[i] - profile[i - 1]).length();
        }
        for i in 0..segs {
            // Normals at both ends of this segment.
            let end_n = |k: usize, other: Option<usize>| -> Vec2 {
                match other {
                    Some(o) if !sharp.get(k).copied().unwrap_or(false) => {
                        (seg_n[i] + seg_n[o]).normalize_or(seg_n[i])
                    }
                    _ => seg_n[i],
                }
            };
            let n0 = end_n(i, i.checked_sub(1));
            let n1 = end_n(i + 1, (i + 1 < segs).then_some(i + 1));
            let base = self.mesh.vertices.len() as u32;
            for f in frames {
                for (k, n2) in [(i, n0), (i + 1, n1)] {
                    let p = f.at(profile[k]);
                    let n = (f.side * n2.x + f.up * n2.y).normalize_or(f.up);
                    let uv = if swap_uv {
                        Vec2::new(vlen[k], f.s)
                    } else {
                        Vec2::new(f.s, vlen[k])
                    };
                    self.vertex(p, n, uv);
                }
            }
            for fi in 0..frames.len() as u32 - 1 {
                let k = base + fi * 2;
                self.quad_idx(k, k + 2, k + 3, k + 1);
            }
        }
    }

    /// Close the ends of a swept profile with flat caps (convex profiles only).
    pub fn sweep_caps(&mut self, profile: &[Vec2], first: &Frame, last: &Frame) {
        for (f, outward) in [(first, -1.0f32), (last, 1.0)] {
            let n = f.side.cross(f.up).normalize() * -outward;
            let pts: Vec<Vec3> = profile.iter().map(|p| f.at(*p)).collect();
            let (bu, bv) = basis(n);
            let idx: Vec<u32> = pts
                .iter()
                .map(|p| self.vertex(*p, n, Vec2::new(p.dot(bu), p.dot(bv))))
                .collect();
            for k in 1..idx.len() - 1 {
                self.tri_idx(idx[0], idx[k], idx[k + 1]);
            }
        }
    }

    /// Append a mesh transformed by `m`.
    pub fn add_mesh(&mut self, other: &Mesh, m: &Mat4) {
        let mut copy = other.clone();
        copy.transform(m);
        self.mesh.merge(&copy);
    }

    pub fn triangles(&self) -> usize {
        self.mesh.triangle_count()
    }

    pub fn finish(self) -> Mesh {
        self.mesh
    }
}

/// Two unit vectors perpendicular to `n` (and to each other).
pub fn basis(n: Vec3) -> (Vec3, Vec3) {
    let n = n.normalize_or(Vec3::Y);
    let helper = if n.y.abs() > 0.9 { Vec3::X } else { Vec3::Y };
    let u = helper.cross(n).normalize();
    let v = n.cross(u);
    (u, v)
}

/// Darken vertices near the ground (local y between `y0` and `y0 + fade`) by up to `dark`
/// (0..1): a cheap contact shadow for walls, posts and props.
pub fn ground_occlusion(mesh: &mut Mesh, y0: f32, fade: f32, dark: f32) {
    for v in &mut mesh.vertices {
        let t = ((v.position[1] - y0) / fade).clamp(0.0, 1.0);
        let k = 1.0 - dark * (1.0 - t) * (1.0 - t);
        for c in 0..3 {
            v.color[c] *= k;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn boxes_face_outward() {
        let mut b = Builder::new();
        b.chamfer_box(Vec3::ZERO, Vec3::new(1.0, 0.5, 2.0), 0.1, Quat::IDENTITY);
        let m = b.finish();
        let mut vol = 0.0;
        for t in m.indices.chunks(3) {
            let p = |i: u32| Vec3::from(m.vertices[i as usize].position);
            vol += p(t[0]).dot(p(t[1]).cross(p(t[2]))) / 6.0;
        }
        // Full box 8 m³ minus chamfers.
        assert!(vol > 7.5 && vol < 8.0, "{vol}");
        let mut c = Builder::new();
        c.cylinder(Vec3::ZERO, Vec3::Y * 2.0, 0.5, 0.5, 16, true);
        let m = c.finish();
        let mut vol = 0.0;
        for t in m.indices.chunks(3) {
            let p = |i: u32| Vec3::from(m.vertices[i as usize].position);
            vol += p(t[0]).dot(p(t[1]).cross(p(t[2]))) / 6.0;
        }
        assert!(vol > 1.4 && vol < 1.6, "{vol}");
        // Rounded box: 8 m³ minus the rounds.
        let mut r = Builder::new();
        r.rounded_box(Vec3::ZERO, Vec3::new(1.0, 0.5, 2.0), 0.2, 3, Quat::IDENTITY);
        let vol = volume(&r.finish());
        assert!(vol > 7.6 && vol < 7.95, "{vol}");
        // Lathe: a closed profile walked from +offset to −offset over the rim (a cylinder).
        let mut l = Builder::new();
        let prof = [
            Vec2::new(0.0, 0.5),
            Vec2::new(1.0, 0.5),
            Vec2::new(1.0, -0.5),
            Vec2::new(0.0, -0.5),
        ];
        l.lathe(Vec3::ZERO, Vec3::Y, &prof, &[false, true, true, false], 32);
        let vol = volume(&l.finish());
        assert!(vol > 3.0 && vol < 3.15, "{vol}");
    }

    /// Signed volume enclosed by a mesh (positive when its faces point outward).
    fn volume(m: &Mesh) -> f32 {
        m.indices
            .chunks(3)
            .map(|t| {
                let p = |i: u32| Vec3::from(m.vertices[i as usize].position);
                p(t[0]).dot(p(t[1]).cross(p(t[2]))) / 6.0
            })
            .sum()
    }
}
