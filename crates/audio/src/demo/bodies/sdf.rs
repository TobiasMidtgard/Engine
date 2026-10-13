//! Sculpted surfaces: a signed distance field built the way an animal is, from bones, muscles
//! and fat (ellipsoids and tapering capsules) joined by smooth unions, with grooves pressed in
//! and ridges raised along lines, then meshed by surface nets: one vertex in every grid cell
//! the surface passes through, pulled onto the surface and shaded with the field's own
//! gradient. A trunk with four legs and a neck comes out as one closed skin, creased where
//! its muscles meet.

use engine_core::math::{Quat, Vec2, Vec3, Vec4};
use engine_render::{Mesh, Vertex};

use super::rig::Xf;
use super::shape::Paint;

/// Smooth union of two distances, filleted over about `k` meters.
pub fn smin(a: f32, b: f32, k: f32) -> f32 {
    if k <= 0.0 {
        return a.min(b);
    }
    let h = (k - (a - b).abs()).max(0.0) / k;
    a.min(b) - h * h * k * 0.25
}

/// Smooth intersection; `smax(a, -b, k)` carves `b` out of `a`.
pub fn smax(a: f32, b: f32, k: f32) -> f32 {
    -smin(-a, -b, k)
}

/// Distance to an ellipsoid with radii `r` round the origin: exact on the surface and close
/// near it (Quilez's first-order bound).
pub fn ellipsoid(p: Vec3, r: Vec3) -> f32 {
    let k0 = (p / r).length();
    let k1 = (p / (r * r)).length();
    if k1 < 1e-12 {
        return -r.min_element();
    }
    k0 * (k0 - 1.0) / k1
}

/// A primitive solid.
#[derive(Clone, Copy, Debug)]
pub enum Shape {
    /// An ellipsoid: centre, its axes (local to world) and radii along them.
    Blob { c: Vec3, rot: Quat, r: Vec3 },
    /// A capsule from `a` to `b` whose elliptical section (radius toward `side`, radius
    /// across) tapers from `ra` to `rb`; the ends are rounded by the smaller radius.
    Limb {
        a: Vec3,
        b: Vec3,
        side: Vec3,
        ra: Vec2,
        rb: Vec2,
    },
    /// A tube along +Z on the midline through cross-sections `[z, centre y, half width,
    /// below, above]` (sorted by z, smoothly interpolated), closed by domes `caps` long. It
    /// stays where it is given (frames don't move it).
    Tube {
        keys: &'static [[f32; 5]],
        caps: [f32; 2],
    },
}

/// The cross-section of a tube at `z` (Catmull-Rom through its keys).
fn section(keys: &[[f32; 5]], z: f32) -> [f32; 5] {
    let n = keys.len();
    let i = keys[..n - 1]
        .iter()
        .rposition(|k| k[0] <= z)
        .unwrap_or(0)
        .min(n - 2);
    let t = ((z - keys[i][0]) / (keys[i + 1][0] - keys[i][0])).clamp(0.0, 1.0);
    let k = |j: isize| keys[j.clamp(0, n as isize - 1) as usize];
    let (a, b, c, d) = (
        k(i as isize - 1),
        k(i as isize),
        k(i as isize + 1),
        k(i as isize + 2),
    );
    let (t2, t3) = (t * t, t * t * t);
    let mut out = [0.0; 5];
    for (q, o) in out.iter_mut().enumerate() {
        let v = 0.5
            * (2.0 * b[q]
                + (-a[q] + c[q]) * t
                + (2.0 * a[q] - 5.0 * b[q] + 4.0 * c[q] - d[q]) * t2
                + (-a[q] + 3.0 * b[q] - 3.0 * c[q] + d[q]) * t3);
        *o = if q >= 2 {
            v.max(0.85 * b[q].min(c[q]))
        } else {
            v
        };
    }
    out
}

/// An ellipsoid with radii `r` along X, Y and Z.
pub fn blob(c: Vec3, r: Vec3) -> Shape {
    Shape::Blob {
        c,
        rot: Quat::IDENTITY,
        r,
    }
}

/// An ellipsoid whose axes are turned by `rot`.
pub fn blob_turned(c: Vec3, r: Vec3, rot: Quat) -> Shape {
    Shape::Blob { c, rot, r }
}

/// An ellipsoid reaching from `a` to `b`, `w` thick toward `side` and `h` across both.
pub fn blob_along(a: Vec3, b: Vec3, w: f32, h: f32, side: Vec3) -> Shape {
    let z = (b - a).normalize_or(Vec3::Z);
    let x = (side - z * z.dot(side)).normalize_or(Vec3::X);
    let y = z.cross(x);
    Shape::Blob {
        c: (a + b) * 0.5,
        rot: Quat::from_mat3(&engine_core::math::Mat3::from_cols(x, y, z)),
        r: Vec3::new(w, h, a.distance(b) * 0.5),
    }
}

/// A tapering capsule (see [`Shape::Limb`]).
pub fn limb(a: Vec3, b: Vec3, ra: Vec2, rb: Vec2, side: Vec3) -> Shape {
    Shape::Limb { a, b, side, ra, rb }
}

impl Shape {
    pub fn d(&self, p: Vec3) -> f32 {
        match *self {
            Shape::Blob { c, rot, r } => ellipsoid(rot.inverse() * (p - c), r),
            Shape::Limb { a, b, side, ra, rb } => {
                let ab = b - a;
                let len = ab.length().max(1e-6);
                let axis = ab / len;
                let x = (side - axis * axis.dot(side)).normalize_or(Vec3::X);
                let y = axis.cross(x);
                let q = p - a;
                let along = q.dot(axis);
                let t = along.clamp(0.0, len);
                let r = ra.lerp(rb, t / len);
                let local = Vec3::new(q.dot(x), q.dot(y), along - t);
                ellipsoid(local, Vec3::new(r.x, r.y, r.x.min(r.y)))
            }
            Shape::Tube { keys, caps } => {
                let zc = p.z.clamp(keys[0][0], keys[keys.len() - 1][0]);
                let [_, cy, w, below, above] = section(keys, zc);
                let (dy, dz) = (p.y - cy, p.z - zc);
                let ry = if dy < 0.0 { below } else { above };
                let rz = if dz < 0.0 { caps[0] } else { caps[1] };
                ellipsoid(Vec3::new(p.x, dy, dz), Vec3::new(w, ry, rz.max(1e-3)))
            }
        }
    }

    /// A sphere holding the shape: centre and radius.
    fn bounds(&self) -> (Vec3, f32) {
        match *self {
            Shape::Blob { c, r, .. } => (c, r.max_element()),
            Shape::Limb { a, b, ra, rb, .. } => (
                (a + b) * 0.5,
                a.distance(b) * 0.5 + ra.max_element().max(rb.max_element()),
            ),
            Shape::Tube { keys, caps } => {
                let (z0, z1) = (keys[0][0] - caps[0], keys[keys.len() - 1][0] + caps[1]);
                let y = keys.iter().map(|k| k[1]).sum::<f32>() / keys.len() as f32;
                let r = keys
                    .iter()
                    .map(|k| k[2].max(k[3]).max(k[4]) + (k[1] - y).abs())
                    .fold(0.0, f32::max);
                (Vec3::new(0.0, y, 0.5 * (z0 + z1)), 0.5 * (z1 - z0) + r)
            }
        }
    }

    /// The shape moved from a bone's frame into the frame `xf` is given in.
    pub fn placed(self, xf: Xf) -> Shape {
        match self {
            Shape::Blob { c, rot, r } => Shape::Blob {
                c: xf.point(c),
                rot: xf.r * rot,
                r,
            },
            Shape::Limb { a, b, side, ra, rb } => Shape::Limb {
                a: xf.point(a),
                b: xf.point(b),
                side: xf.r * side,
                ra,
                rb,
            },
            tube @ Shape::Tube { .. } => tube,
        }
    }

    /// The mirror image across x = 0.
    pub fn mirrored(self) -> Shape {
        let m = |v: Vec3| Vec3::new(-v.x, v.y, v.z);
        match self {
            Shape::Blob { c, rot, r } => {
                // Mirror the axes and flip X to keep the frame right-handed (the ellipsoid is
                // symmetric about its own axes, so the flip changes nothing else).
                let (x, y, z) = (m(rot * Vec3::X), m(rot * Vec3::Y), m(rot * Vec3::Z));
                Shape::Blob {
                    c: m(c),
                    rot: Quat::from_mat3(&engine_core::math::Mat3::from_cols(-x, y, z)),
                    r,
                }
            }
            Shape::Limb { a, b, side, ra, rb } => Shape::Limb {
                a: m(a),
                b: m(b),
                side: m(side),
                ra,
                rb,
            },
            // Already symmetric.
            tube @ Shape::Tube { .. } => tube,
        }
    }
}

#[derive(Clone, Copy, Debug)]
enum Op {
    Add(Shape, f32),
    Carve(Shape, f32),
    /// Push the surface in by `depth` (out where negative) along a line through the first
    /// `n` of `pts`, over `width`.
    Press {
        pts: [Vec3; GROOVE],
        n: usize,
        width: f32,
        depth: f32,
    },
}

/// The most points a groove's line has.
const GROOVE: usize = 16;

#[derive(Clone, Copy, Debug)]
struct Step {
    op: Op,
    /// Beyond this sphere the step changes nothing.
    centre: Vec3,
    reach: f32,
}

/// A body sculpted step by step. Shapes are given in the current frame (see
/// [`Sculpt::frame`]), so a muscle can be placed on the bone it moves with.
#[derive(Clone, Debug, Default)]
pub struct Sculpt {
    steps: Vec<Step>,
    frame: Option<Xf>,
}

impl Sculpt {
    pub fn new() -> Self {
        Self::default()
    }

    /// Give the following shapes and lines in `xf`'s frame.
    pub fn frame(&mut self, xf: Xf) -> &mut Self {
        self.frame = Some(xf);
        self
    }

    fn place(&self, s: Shape) -> Shape {
        self.frame.map_or(s, |xf| s.placed(xf))
    }

    fn point(&self, p: Vec3) -> Vec3 {
        self.frame.map_or(p, |xf| xf.point(p))
    }

    fn push(&mut self, op: Op) {
        let (centre, reach) = match op {
            Op::Add(s, k) | Op::Carve(s, k) => {
                let (c, r) = s.bounds();
                (c, r + k)
            }
            Op::Press { pts, n, width, .. } => {
                let c = pts[..n].iter().sum::<Vec3>() / n as f32;
                let r = pts[..n].iter().map(|p| p.distance(c)).fold(0.0, f32::max);
                (c, r + 3.0 * width)
            }
        };
        self.steps.push(Step { op, centre, reach });
    }

    /// Join `s` on, filleted over `k` meters.
    pub fn add(&mut self, s: Shape, k: f32) -> &mut Self {
        let s = self.place(s);
        self.push(Op::Add(s, k));
        self
    }

    /// `s` and its mirror image across x = 0.
    pub fn pair(&mut self, s: Shape, k: f32) -> &mut Self {
        let s = self.place(s);
        self.push(Op::Add(s, k));
        self.push(Op::Add(s.mirrored(), k));
        self
    }

    /// Cut `s` away, the edge rounded over `k` meters.
    pub fn carve(&mut self, s: Shape, k: f32) -> &mut Self {
        let s = self.place(s);
        self.push(Op::Carve(s, k));
        self
    }

    pub fn carve_pair(&mut self, s: Shape, k: f32) -> &mut Self {
        let s = self.place(s);
        self.push(Op::Carve(s, k));
        self.push(Op::Carve(s.mirrored(), k));
        self
    }

    /// Press a groove `depth` deep and about `width` wide into the surface along `a`→`b`
    /// (a ridge where `depth` is negative), the line laid onto the surface as it is so far.
    pub fn press(&mut self, a: Vec3, b: Vec3, width: f32, depth: f32) -> &mut Self {
        let (a, b) = (self.point(a), self.point(b));
        self.groove(a, b, width, depth);
        self
    }

    pub fn press_pair(&mut self, a: Vec3, b: Vec3, width: f32, depth: f32) -> &mut Self {
        let (a, b) = (self.point(a), self.point(b));
        let m = |v: Vec3| Vec3::new(-v.x, v.y, v.z);
        self.groove(a, b, width, depth);
        self.groove(m(a), m(b), width, depth);
        self
    }

    fn groove(&mut self, a: Vec3, b: Vec3, width: f32, depth: f32) {
        let n = ((a.distance(b) / width).ceil() as usize + 1).clamp(2, GROOVE);
        let mut pts = [Vec3::ZERO; GROOVE];
        for (i, p) in pts[..n].iter_mut().enumerate() {
            *p = self.snap(a.lerp(b, i as f32 / (n - 1) as f32));
        }
        self.push(Op::Press {
            pts,
            n,
            width,
            depth,
        });
    }

    /// `p` moved onto the surface along the field's gradient.
    pub fn snap(&self, mut p: Vec3) -> Vec3 {
        for _ in 0..8 {
            let d = self.d(p);
            let g = gradient(&|q| self.d(q), p, 1e-3);
            p -= g * d / g.length_squared().max(1e-6);
        }
        p
    }

    /// The field at `p`: negative inside.
    pub fn d(&self, p: Vec3) -> f32 {
        let mut d = f32::MAX;
        for s in &self.steps {
            let near = (p - s.centre).length() - s.reach;
            match s.op {
                Op::Add(shape, k) => {
                    if near > d {
                        continue;
                    }
                    d = smin(d, shape.d(p), k);
                }
                Op::Carve(shape, k) => {
                    if near > 0.0 {
                        continue;
                    }
                    d = smax(d, -shape.d(p), k);
                }
                Op::Press {
                    pts,
                    n,
                    width,
                    depth,
                } => {
                    if near > 0.0 {
                        continue;
                    }
                    let mut r2 = f32::MAX;
                    for w in pts[..n].windows(2) {
                        let ab = w[1] - w[0];
                        let t =
                            ((p - w[0]).dot(ab) / ab.length_squared().max(1e-12)).clamp(0.0, 1.0);
                        r2 = r2.min((p - (w[0] + ab * t)).length_squared());
                    }
                    d += depth * (-r2 / (width * width)).exp();
                }
            }
        }
        d
    }

    /// Where a ray from `from` (inside) along `dir` first leaves the surface.
    pub fn exit(&self, from: Vec3, dir: Vec3) -> Vec3 {
        exit(&|p| self.d(p), from, dir)
    }
}

/// The field's gradient by central differences `e` apart.
pub fn gradient(f: &dyn Fn(Vec3) -> f32, p: Vec3, e: f32) -> Vec3 {
    let dx = f(p + Vec3::X * e) - f(p - Vec3::X * e);
    let dy = f(p + Vec3::Y * e) - f(p - Vec3::Y * e);
    let dz = f(p + Vec3::Z * e) - f(p - Vec3::Z * e);
    Vec3::new(dx, dy, dz) / (2.0 * e)
}

/// Where a ray from `from` (inside `f`) along `dir` first leaves the surface.
pub fn exit(f: &dyn Fn(Vec3) -> f32, from: Vec3, dir: Vec3) -> Vec3 {
    let dir = dir.normalize_or(Vec3::Y);
    let (mut lo, mut t) = (0.0f32, 0.0f32);
    for _ in 0..4000 {
        let d = f(from + dir * t);
        if d > 0.0 {
            break;
        }
        lo = t;
        t += (-d * 0.6).max(0.0015);
    }
    let mut hi = t;
    for _ in 0..30 {
        let mid = 0.5 * (lo + hi);
        if f(from + dir * mid) > 0.0 {
            hi = mid;
        } else {
            lo = mid;
        }
    }
    from + dir * hi
}

/// Run `f` over `0..n` on every core.
pub(super) fn par_map<T: Send>(n: usize, f: impl Fn(usize) -> T + Sync) -> Vec<T> {
    let threads = std::thread::available_parallelism().map_or(1, |t| t.get().min(8));
    let chunk = n.div_ceil(threads).max(256);
    let f = &f;
    std::thread::scope(|s| {
        let jobs: Vec<_> = (0..n)
            .step_by(chunk)
            .map(|start| {
                s.spawn(move || (start..(start + chunk).min(n)).map(f).collect::<Vec<T>>())
            })
            .collect();
        jobs.into_iter()
            .flat_map(|j| j.join().expect("surface nets worker"))
            .collect()
    })
}

/// A closed surface mesh of `f(p) = 0`: positions, unit normals (the field's gradient) and
/// triangles wound outward.
#[derive(Clone, Debug, Default)]
pub struct Nets {
    pub pts: Vec<Vec3>,
    pub normals: Vec<Vec3>,
    pub tris: Vec<[u32; 3]>,
}

impl Nets {
    /// Mesh `f` inside the box `lo..hi` (which must hold the whole surface) with cubes `cell`
    /// meters across. The field is sampled coarsely first and finely only near the surface.
    pub fn new(f: &(dyn Fn(Vec3) -> f32 + Sync), lo: Vec3, hi: Vec3, cell: f32) -> Self {
        let size = ((hi - lo) / cell).ceil();
        let (nx, ny, nz) = (
            size.x as usize + 1,
            size.y as usize + 1,
            size.z as usize + 1,
        );
        let node =
            |i: usize, j: usize, k: usize| lo + Vec3::new(i as f32, j as f32, k as f32) * cell;
        let idx = |i: usize, j: usize, k: usize| (k * ny + j) * nx + i;

        // Coarse pass every `C` nodes; blocks the surface may pass through are sampled finely.
        const C: usize = 4;
        let (cx, cy, cz) = (
            (nx - 1).div_ceil(C) + 1,
            (ny - 1).div_ceil(C) + 1,
            (nz - 1).div_ceil(C) + 1,
        );
        let coarse = par_map(cx * cy * cz, |n| {
            let (i, j, k) = (n % cx, (n / cx) % cy, n / (cx * cy));
            f(node(i * C, j * C, k * C))
        });
        let band = cell * C as f32 * 1.6;
        let mut v = vec![f32::NAN; nx * ny * nz];
        let mut exact = vec![false; nx * ny * nz];
        for bk in 0..cz - 1 {
            for bj in 0..cy - 1 {
                for bi in 0..cx - 1 {
                    let c: Vec<f32> = (0..8)
                        .map(|q| {
                            coarse[((bk + (q >> 2)) * cy + bj + ((q >> 1) & 1)) * cx + bi + (q & 1)]
                        })
                        .collect();
                    let near = c.iter().any(|x| x.abs() < band)
                        || c.iter().any(|&x| (x < 0.0) != (c[0] < 0.0));
                    for k in bk * C..=(bk * C + C).min(nz - 1) {
                        for j in bj * C..=(bj * C + C).min(ny - 1) {
                            for i in bi * C..=(bi * C + C).min(nx - 1) {
                                let n = idx(i, j, k);
                                if near {
                                    exact[n] = true;
                                } else if v[n].is_nan() {
                                    v[n] = c[0];
                                }
                            }
                        }
                    }
                }
            }
        }
        let todo: Vec<usize> = (0..v.len()).filter(|&n| exact[n]).collect();
        let vals = par_map(todo.len(), |t| {
            let n = todo[t];
            f(node(n % nx, (n / nx) % ny, n / (nx * ny)))
        });
        for (&n, x) in todo.iter().zip(vals) {
            v[n] = x;
        }

        // A vertex in every cell the surface crosses: the mean of its edges' crossings.
        const EDGES: [(usize, usize); 12] = [
            (0, 1),
            (2, 3),
            (4, 5),
            (6, 7),
            (0, 2),
            (1, 3),
            (4, 6),
            (5, 7),
            (0, 4),
            (1, 5),
            (2, 6),
            (3, 7),
        ];
        let cid = |i: usize, j: usize, k: usize| (k * (ny - 1) + j) * (nx - 1) + i;
        let mut vid = vec![u32::MAX; (nx - 1) * (ny - 1) * (nz - 1)];
        let mut pts = Vec::new();
        for k in 0..nz - 1 {
            for j in 0..ny - 1 {
                for i in 0..nx - 1 {
                    let corner = |q: usize| (i + (q & 1), j + ((q >> 1) & 1), k + (q >> 2));
                    let val = |q: usize| {
                        let (a, b, c) = corner(q);
                        v[idx(a, b, c)]
                    };
                    let inside = val(0) < 0.0;
                    if (1..8).all(|q| (val(q) < 0.0) == inside) {
                        continue;
                    }
                    let mut sum = Vec3::ZERO;
                    let mut count = 0.0;
                    for (a, b) in EDGES {
                        let (va, vb) = (val(a), val(b));
                        if (va < 0.0) != (vb < 0.0) {
                            let t = va / (va - vb);
                            let (pa, pb) = (corner(a), corner(b));
                            sum += node(pa.0, pa.1, pa.2).lerp(node(pb.0, pb.1, pb.2), t);
                            count += 1.0;
                        }
                    }
                    vid[cid(i, j, k)] = pts.len() as u32;
                    pts.push(sum / count);
                }
            }
        }

        // Pull each vertex onto the surface (a few Newton steps, never leaving its cell's
        // neighbourhood), and take the normal from the field.
        let moved = par_map(pts.len(), |n| {
            let start = pts[n];
            let mut p = start;
            for _ in 0..4 {
                let d = f(p);
                let g = gradient(f, p, cell * 0.1);
                p -= (g * (d / g.length_squared().max(1e-8))).clamp_length_max(cell * 0.5);
                p = start + (p - start).clamp_length_max(cell);
            }
            let g = gradient(f, p, cell * 0.25);
            (p, g.normalize_or(Vec3::Y))
        });
        let (pts, normals): (Vec<Vec3>, Vec<Vec3>) = moved.into_iter().unzip();

        // A quad across every edge the surface crosses, joining the four cells round it.
        let mut tris = Vec::new();
        let mut quad = |cells: [(usize, usize, usize); 4], flip: bool| {
            let q = cells.map(|(i, j, k)| vid[cid(i, j, k)]);
            if q.contains(&u32::MAX) {
                return;
            }
            let [a, b, c, d] = if flip { [q[3], q[2], q[1], q[0]] } else { q };
            let p = |i: u32| pts[i as usize];
            if p(a).distance_squared(p(c)) <= p(b).distance_squared(p(d)) {
                tris.push([a, b, c]);
                tris.push([a, c, d]);
            } else {
                tris.push([a, b, d]);
                tris.push([b, c, d]);
            }
        };
        for k in 1..nz - 1 {
            for j in 1..ny - 1 {
                for i in 1..nx - 1 {
                    let inside = v[idx(i, j, k)] < 0.0;
                    if (v[idx(i + 1, j, k)] < 0.0) != inside {
                        quad(
                            [(i, j - 1, k - 1), (i, j, k - 1), (i, j, k), (i, j - 1, k)],
                            !inside,
                        );
                    }
                    if (v[idx(i, j + 1, k)] < 0.0) != inside {
                        quad(
                            [(i - 1, j, k - 1), (i - 1, j, k), (i, j, k), (i, j, k - 1)],
                            !inside,
                        );
                    }
                    if (v[idx(i, j, k + 1)] < 0.0) != inside {
                        quad(
                            [(i - 1, j - 1, k), (i, j - 1, k), (i, j, k), (i - 1, j, k)],
                            !inside,
                        );
                    }
                }
            }
        }
        // Wind every triangle with the field's outward gradient.
        for t in &mut tris {
            let [a, b, c] = t.map(|i| pts[i as usize]);
            let n: Vec3 = t.iter().map(|&i| normals[i as usize]).sum();
            if (b - a).cross(c - a).dot(n) < 0.0 {
                t.swap(1, 2);
            }
        }
        Self { pts, normals, tris }
    }

    /// Split into meshes: each triangle goes to piece `which(its centre)` (or is dropped), and
    /// each piece's vertices get texture coordinates `uv(p, n)` and colours from `paint`.
    pub fn split(
        &self,
        pieces: usize,
        which: impl Fn(Vec3) -> Option<usize>,
        uv: impl Fn(Vec3, Vec3) -> Vec2,
        paint: impl Fn(&Paint) -> Vec4,
    ) -> Vec<Mesh> {
        let mut out = vec![Mesh::default(); pieces];
        let mut map = vec![vec![u32::MAX; self.pts.len()]; pieces];
        for t in &self.tris {
            let centre = t.iter().map(|&i| self.pts[i as usize]).sum::<Vec3>() / 3.0;
            let Some(k) = which(centre) else {
                continue;
            };
            for &i in t {
                let i = i as usize;
                if map[k][i] == u32::MAX {
                    map[k][i] = out[k].vertices.len() as u32;
                    let (p, n) = (self.pts[i], self.normals[i]);
                    let color = paint(&Paint { p, n, s: 0.0 });
                    out[k]
                        .vertices
                        .push(Vertex::new(p, n, uv(p, n)).with_color(color));
                }
                out[k].indices.push(map[k][i]);
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_sphere_meshes_closed_and_round() {
        let f = |p: Vec3| p.length() - 0.3;
        let nets = Nets::new(&f, Vec3::splat(-0.4), Vec3::splat(0.4), 0.02);
        assert!(nets.tris.len() > 1000);
        for (p, n) in nets.pts.iter().zip(&nets.normals) {
            assert!((p.length() - 0.3).abs() < 1e-3, "{p} off the surface");
            assert!(n.dot(p.normalize()) > 0.99);
        }
        // Every edge is shared by exactly two triangles (closed and manifold), and every
        // triangle faces outward.
        let mut edges = std::collections::HashMap::new();
        for t in &nets.tris {
            for e in 0..3 {
                let (a, b) = (t[e], t[(e + 1) % 3]);
                *edges.entry((a.min(b), a.max(b))).or_insert(0) += 1;
            }
            let [a, b, c] = t.map(|i| nets.pts[i as usize]);
            assert!((b - a).cross(c - a).dot(a + b + c) > 0.0);
        }
        assert!(edges.values().all(|&n| n == 2));
    }

    #[test]
    fn smooth_unions_fill_the_joint_and_grooves_press_in() {
        let mut s = Sculpt::new();
        s.add(blob(Vec3::ZERO, Vec3::splat(0.1)), 0.0)
            .pair(blob(Vec3::new(0.15, 0.0, 0.0), Vec3::splat(0.08)), 0.05);
        // In the crease between the balls the fillet fills in what a hard union leaves.
        let p = Vec3::new(0.1, 0.07, 0.0);
        let hard = (p.length() - 0.1).min(p.distance(Vec3::new(0.15, 0.0, 0.0)) - 0.08);
        assert!(s.d(p) < hard - 0.003);
        assert!(s.d(Vec3::new(-0.15, 0.0, 0.0)) < -0.07, "mirrored");
        let top = s.exit(Vec3::ZERO, Vec3::Y);
        assert!((top.y - 0.1).abs() < 0.01, "{top}");
        // A groove 2 cm deep over 1 cm sinks the surface under its line by about 9 mm, and
        // the line is laid onto the ball, so it sinks it round the curve too.
        let slope = Vec3::new(0.0, 1.0, 0.6).normalize();
        let before = s.exit(Vec3::ZERO, slope).length();
        s.press(top - Vec3::Z * 0.1, top + Vec3::Z * 0.1, 0.01, 0.02);
        assert!(s.exit(Vec3::ZERO, Vec3::Y).y < top.y - 0.007);
        assert!(s.exit(Vec3::ZERO, slope).length() < before - 0.007);
        let limb = limb(
            Vec3::ZERO,
            Vec3::Y,
            Vec2::new(0.2, 0.1),
            Vec2::new(0.1, 0.05),
            Vec3::X,
        );
        assert!(limb.d(Vec3::new(0.2, 0.0, 0.0)).abs() < 1e-4);
        assert!(limb.d(Vec3::new(0.0, 1.0, 0.05)).abs() < 1e-4);
        let m = blob_along(Vec3::ZERO, Vec3::new(0.0, 0.0, 1.0), 0.1, 0.2, Vec3::X).mirrored();
        assert!(m.d(Vec3::new(0.0, 0.0, 1.0)).abs() < 1e-4);
        assert!(m.d(Vec3::new(-0.1, 0.0, 0.5)).abs() < 1e-4);
    }
}
