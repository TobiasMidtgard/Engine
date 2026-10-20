//! Detailed meshes for the proving ground's dynamic props: wooden shipping crates, 200 l steel
//! drums, rubber balls, the wrecking ball with its chain, and dominoes.
//!
//! Every function returns finished meshes centered on the physics body's origin and sized
//! from the collider's parameters, so the visible surface sits within a few millimeters of
//! the collider and contacts look right. One mesh per material; props made of two materials
//! return a small struct. Vertex colors carry per-part tint (multiplied with the material's
//! base color), so a material can stay a plain tileable texture set.
//!
//! UVs are metric (1 UV unit = 1 m, see [`super::kit`]). Wood grain runs along U on every
//! board and every board gets its own UV offset, so a tiling `crate_wood` texture never
//! repeats board to board.
//!
//! | Prop | Meshes → material | Triangles |
//! |---|---|---|
//! | [`crate_box`] | `wood` → crate wood, `metal` → painted or forged steel (brackets, bolts, nails) | ≤ 3 000 together |
//! | [`steel_drum`] | painted steel | ≈ 2 900 |
//! | [`ball`], [`playground_ball`] | rubber | 1 728 |
//! | [`wrecking_ball`] | forged steel | ≈ 3 700 |
//! | [`chain_segment`], [`chain_end_segment`] | forged steel | ≈ 2 000 per 0.8 m |
//! | [`domino`] | white, glossy (body and pip colors are vertex colors) | ≤ 1 800 for double six |

use std::f32::consts::{FRAC_PI_2, TAU};

use engine_core::math::{Mat4, Quat, Vec2, Vec3, Vec4};
use engine_render::Mesh;

use super::kit::{MeshBuilder, srgb};

// ---------------------------------------------------------------------------------------------
// Shared helpers
// ---------------------------------------------------------------------------------------------

/// Deterministic pseudo-random numbers in 0..1 (props look the same every run).
struct Rng(u32);

impl Rng {
    fn new(seed: u32) -> Self {
        // Scramble so neighboring seeds give unrelated sequences (and never 0).
        let mut s = seed.wrapping_mul(0x9e37_79b9) ^ 0x85eb_ca6b;
        s ^= s >> 15;
        s = s.wrapping_mul(0x2c1b_3c6d);
        s ^= s >> 12;
        Self(s | 1)
    }

    fn next(&mut self) -> f32 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 17;
        self.0 ^= self.0 << 5;
        (self.0 >> 8) as f32 / (1u32 << 24) as f32
    }

    fn range(&mut self, lo: f32, hi: f32) -> f32 {
        lo + (hi - lo) * self.next()
    }
}

fn signed_area(p: &[Vec2]) -> f32 {
    let n = p.len();
    (0..n).map(|i| p[i].perp_dot(p[(i + 1) % n])).sum::<f32>() * 0.5
}

/// Points on a circular arc around `center` from `a0` to `a1` degrees, both ends included.
fn arc(center: Vec2, radius: f32, a0: f32, a1: f32, steps: u32) -> Vec<Vec2> {
    (0..=steps)
        .map(|k| {
            let a = (a0 + (a1 - a0) * k as f32 / steps as f32).to_radians();
            center + Vec2::new(a.cos(), a.sin()) * radius
        })
        .collect()
}

/// The part of a convex polygon where `n · p <= d`.
fn clip(poly: &[Vec2], n: Vec2, d: f32) -> Vec<Vec2> {
    let mut out = Vec::new();
    for i in 0..poly.len() {
        let (a, b) = (poly[i], poly[(i + 1) % poly.len()]);
        let (da, db) = (a.dot(n) - d, b.dot(n) - d);
        if da <= 0.0 {
            out.push(a);
        }
        if (da <= 0.0) != (db <= 0.0) {
            out.push(a + (b - a) * (da / (da - db)));
        }
    }
    out
}

/// A local frame: columns `x`, `y`, `z` (right-handed) at `origin`.
fn frame(x: Vec3, y: Vec3, z: Vec3, origin: Vec3) -> Mat4 {
    Mat4::from_cols(
        x.extend(0.0),
        y.extend(0.0),
        z.extend(0.0),
        origin.extend(1.0),
    )
}

/// Lathe axis (+Y) turned onto +X.
fn y_to_x() -> Mat4 {
    Mat4::from_rotation_z(-FRAC_PI_2)
}

/// A board or plate lying on its back: `outline` (local XY, either winding, convex) raised from
/// z = 0 to `thickness`, its top edges and vertical corners rounded by `bevel`.
///
/// Each rounded edge is one chamfer whose vertex normals are those of the faces it joins, so
/// flat faces stay perfectly flat and every edge catches a soft highlight like a real eased
/// edge. There is no bottom face (the part lies against something). With `bevel` equal to
/// `thickness` the chamfer reaches the base and there are no side walls (sheet metal).
/// UVs: top in local meters plus `uv_offset` (grain along local X), walls along the
/// perimeter.
fn slab(b: &mut MeshBuilder, outline: &[Vec2], thickness: f32, bevel: f32, uv_offset: Vec2) {
    slab_shaded(b, outline, thickness, bevel, uv_offset, 1.0);
}

/// [`slab`] with its side walls' foot vertex-shaded by `foot` (a baked contact shadow).
fn slab_shaded(
    b: &mut MeshBuilder,
    outline: &[Vec2],
    thickness: f32,
    bevel: f32,
    uv_offset: Vec2,
    foot: f32,
) {
    let mut p = outline.to_vec();
    if signed_area(&p) < 0.0 {
        p.reverse();
    }
    let n = p.len();
    if n < 3 {
        return;
    }
    let bevel = bevel.clamp(0.0, thickness);
    let wall = thickness - bevel;
    let right = |d: Vec2| Vec2::new(d.y, -d.x);
    // Two ring points per corner (the vertical corner rounded) with their normals, and the
    // top face corner inset by the bevel.
    let mut ring: Vec<(Vec2, Vec2)> = Vec::with_capacity(2 * n);
    let mut top: Vec<Vec2> = Vec::with_capacity(n);
    for i in 0..n {
        let (prev, next) = (p[(i + n - 1) % n], p[(i + 1) % n]);
        let (din, dout) = ((p[i] - prev).normalize(), (next - p[i]).normalize());
        let c = bevel
            .min(0.4 * p[i].distance(prev))
            .min(0.4 * p[i].distance(next));
        let (nin, nout) = (right(din), right(dout));
        ring.push((p[i] - din * c, nin));
        ring.push((p[i] + dout * c, nout));
        let bis = (nin + nout).normalize_or(nin);
        top.push(p[i] - bis * (bevel / bis.dot(nin).max(0.3)));
    }
    let uv = |q: Vec2| q + uv_offset;
    let t: Vec<u32> = top
        .iter()
        .map(|&q| b.vertex(q.extend(thickness), Vec3::Z, uv(q)))
        .collect();
    for k in 1..n - 1 {
        b.tri(t[0], t[k], t[k + 1]);
    }
    // Chamfers: from the ring at the wall top up to the top face.
    let ring_v: Vec<u32> = ring
        .iter()
        .map(|&(q, nq)| b.vertex(q.extend(wall), nq.extend(0.0), uv(q)))
        .collect();
    for i in 0..n {
        let j = (i + 1) % n;
        b.tri(ring_v[2 * i], ring_v[2 * i + 1], t[i]);
        b.quad(ring_v[2 * i + 1], ring_v[2 * j], t[j], t[i]);
    }
    if wall > 1e-5 {
        let mut along = 0.0;
        for k in 0..2 * n {
            let (q0, n0) = ring[k];
            let (q1, n1) = ring[(k + 1) % (2 * n)];
            let len = q0.distance(q1);
            if len < 1e-6 {
                continue;
            }
            let o = uv_offset;
            let lit = b.color;
            b.color = (lit.truncate() * foot).extend(lit.w);
            let v00 = b.vertex(q0.extend(0.0), n0.extend(0.0), o + Vec2::new(along, 0.0));
            let v10 = b.vertex(
                q1.extend(0.0),
                n1.extend(0.0),
                o + Vec2::new(along + len, 0.0),
            );
            b.color = lit;
            let v11 = b.vertex(
                q1.extend(wall),
                n1.extend(0.0),
                o + Vec2::new(along + len, wall),
            );
            let v01 = b.vertex(q0.extend(wall), n0.extend(0.0), o + Vec2::new(along, wall));
            b.quad(v00, v10, v11, v01);
            along += len;
        }
    }
}

/// An infill plank: the top and both long rounded edges of a board running along local X
/// from `-half_len` to `half_len`, `width` wide, its top at z = 0 and its sides reaching down
/// to `-depth`. The ends are open (they hide under the frame boards).
fn plank(b: &mut MeshBuilder, half_len: f32, width: f32, depth: f32, bevel: f32, uv: Vec2) {
    let hw = width * 0.5;
    let bv = bevel.min(hw * 0.5).min(depth);
    let section = [
        (-hw, -depth, -Vec3::Y),
        (-hw, -bv, -Vec3::Y),
        (-hw + bv, 0.0, Vec3::Z),
        (hw - bv, 0.0, Vec3::Z),
        (hw, -bv, Vec3::Y),
        (hw, -depth, Vec3::Y),
    ];
    let mut v = [0.0f32; 6];
    for i in 1..6 {
        let (a, b2) = (section[i - 1], section[i]);
        v[i] = v[i - 1] + Vec2::new(a.0 - b2.0, a.1 - b2.1).length();
    }
    b.grid(6, 2, |r, c| {
        let (y, z, n) = section[r];
        let x = if c == 0 { -half_len } else { half_len };
        (Vec3::new(x, y, z), n, uv + Vec2::new(x, v[r] - hw - bv))
    });
}

/// A low dome on the XY plane facing +Z (a nail, rivet or bolt head): `radius` at the base,
/// `height` tall, its rim sunk a hair below z = 0 so it never floats. `rings` 1 is a cone
/// shaded round; 2 adds a shoulder.
fn dome(b: &mut MeshBuilder, radius: f32, height: f32, sides: u32, rings: u32) {
    let center = b.vertex(Vec3::Z * height, Vec3::Z, Vec2::ZERO);
    let mut prev: Option<Vec<u32>> = None;
    for k in 1..=rings {
        let t = k as f32 / rings as f32;
        let (r, z) = if k == rings {
            (radius, -0.04 * radius)
        } else {
            (radius * 0.68, height * 0.78)
        };
        // Steeper toward the rim than the true cap, so the edge reads round.
        let tilt = if k == rings { 1.15 } else { 0.45 } * t.max(0.5);
        let ring: Vec<u32> = (0..=sides)
            .map(|s| {
                let a = TAU * s as f32 / sides as f32;
                let (sn, cs) = a.sin_cos();
                let n = Vec3::new(cs * tilt.sin(), sn * tilt.sin(), tilt.cos());
                b.vertex(Vec3::new(cs * r, sn * r, z), n, Vec2::new(cs * r, sn * r))
            })
            .collect();
        match &prev {
            None => {
                for s in 0..sides as usize {
                    b.tri(center, ring[s], ring[s + 1]);
                }
            }
            Some(inner) => {
                for s in 0..sides as usize {
                    b.quad(inner[s], ring[s], ring[s + 1], inner[s + 1]);
                }
            }
        }
        prev = Some(ring);
    }
}

/// A short round boss along +Y (lathe): `radius`, from `-half` to `half`, rims rounded by
/// `bevel`.
fn boss(b: &mut MeshBuilder, radius: f32, half: f32, bevel: f32, sides: u32) {
    let profile = [
        Vec2::new(0.0, -half),
        Vec2::new(radius - bevel, -half),
        Vec2::new(radius - bevel * 0.3, -half + bevel * 0.3),
        Vec2::new(radius, -half + bevel),
        Vec2::new(radius, half - bevel),
        Vec2::new(radius - bevel * 0.3, half - bevel * 0.3),
        Vec2::new(radius - bevel, half),
        Vec2::new(0.0, half),
    ];
    b.lathe(&profile, sides, 50.0);
}

/// One face of a box, for laying parts on it. (`u`, `v`, `n`) is right-handed with `n`
/// pointing out; `d` is the face plane's distance from the center and `ext` the half extents
/// of the area this face owns along `u` and `v`.
#[derive(Clone, Copy)]
struct Face {
    n: Vec3,
    u: Vec3,
    v: Vec3,
    d: f32,
    ext: Vec2,
}

impl Face {
    /// Faces own the box edges in the order Y, X, Z: along every edge it shares with a face
    /// that owns it, a face gives up `inset`, so parts from neighboring faces butt against each
    /// other instead of overlapping.
    fn new(half: Vec3, axis: usize, sign: f32, inset: f32) -> Self {
        let (ui, vi) = match axis {
            0 => (2, 1),
            1 => (0, 2),
            _ => (0, 1),
        };
        let n = Vec3::AXES[axis] * sign;
        let v = Vec3::AXES[vi];
        let mut u = Vec3::AXES[ui];
        if u.cross(v).dot(n) < 0.0 {
            u = -u;
        }
        let eu = if axis == 2 {
            half[ui] - inset
        } else {
            half[ui]
        };
        let ev = if axis == 1 {
            half[vi]
        } else {
            half[vi] - inset
        };
        Self {
            n,
            u,
            v,
            d: half[axis],
            ext: Vec2::new(eu, ev),
        }
    }

    /// Local frame at face point `at`, `z` meters out from the face plane, local X along the
    /// face direction `dir` (unit).
    fn frame(&self, at: Vec2, dir: Vec2, z: f32) -> Mat4 {
        let x = self.u * dir.x + self.v * dir.y;
        let y = self.u * -dir.y + self.v * dir.x;
        let o = self.u * at.x + self.v * at.y + self.n * (self.d + z);
        frame(x, y, self.n, o)
    }

    /// Face coordinates of a point.
    fn coords(&self, p: Vec3) -> Vec2 {
        Vec2::new(p.dot(self.u), p.dot(self.v))
    }
}

// ---------------------------------------------------------------------------------------------
// Crates
// ---------------------------------------------------------------------------------------------

/// Thickness of a crate's steel corner brackets, meters.
pub const CRATE_BRACKET_THICKNESS: f32 = 0.003;
/// Height of the bolt heads on a crate's brackets, meters. The bolt tops are exactly on the
/// collider (crates rest on them); the brackets' faces sit this far inside it and the wood
/// another [`CRATE_BRACKET_THICKNESS`] further in.
pub const CRATE_BOLT_HEIGHT: f32 = 0.0025;

/// The two meshes of a crate.
#[derive(Debug, Clone)]
pub struct CrateMeshes {
    /// Boards (vertex-tinted per board) for a `crate_wood` material: grain along U.
    pub wood: Mesh,
    /// Corner brackets, their bolts and the nail heads, for a steel material (painted dark
    /// steel or forged steel). Nails are vertex-darkened.
    pub metal: Mesh,
}

/// Linear RGBA tint for one board: a little lighter or darker, warmer or cooler.
fn wood_tint(rng: &mut Rng, base: f32) -> Vec4 {
    let b = base * rng.range(0.84, 1.08);
    let warm = rng.range(-1.0, 1.0);
    Vec4::new(b * (1.0 + 0.05 * warm), b, b * (1.0 - 0.08 * warm), 1.0)
}

/// Builds a crate's woodwork: boards with their nails, and the tint and size rules they share.
struct Carpenter {
    wood: MeshBuilder,
    /// (face, position, height above the face plane) of every nail.
    nails: Vec<(Face, Vec2, f32)>,
    rng: Rng,
    /// Overall shade of this crate's wood.
    base: f32,
    /// Frame board width, m.
    w: f32,
    /// How far the frame stands proud of the planks, m.
    tf: f32,
    /// Detail shed to stay in budget: 1 nails braces once per end, 2 also gives long faces
    /// one brace instead of a stile and two.
    lean: u32,
}

impl Carpenter {
    const BEVEL: f32 = 0.004;
    /// Vertex shade at the foot of a board's sides: contact shadow where it meets the planks.
    const FOOT: f32 = 0.55;

    fn uv(&mut self) -> Vec2 {
        Vec2::new(self.rng.range(0.0, 3.0), self.rng.range(0.0, 3.0))
    }

    /// A board's outline (local XY, grain along X) at `m`, `thickness` tall: end grain darker,
    /// a contact shadow at its foot.
    fn board_slab(&mut self, m: Mat4, outline: &[Vec2], thickness: f32) {
        let tint = wood_tint(&mut self.rng, self.base);
        let uv = self.uv();
        self.wood.colored(tint, |b| {
            b.placed(m, |b| {
                let start = b.mesh.vertices.len();
                slab_shaded(b, outline, thickness, Self::BEVEL, uv, Self::FOOT);
                for v in &mut b.mesh.vertices[start..] {
                    if v.normal[0].abs() > 0.6 {
                        for c in &mut v.color[..3] {
                            *c *= 0.72;
                        }
                    }
                }
            })
        });
    }

    /// A frame board on face `f` from `lo` to `hi` (face coordinates), grain along its
    /// longer side, nailed at `±reach` from the face center along its length (once in the
    /// middle when that is too short).
    fn board(&mut self, f: &Face, lo: Vec2, hi: Vec2, reach: f32) {
        let size = hi - lo;
        let (dir, l, w) = if size.x >= size.y {
            (Vec2::X, size.x, size.y)
        } else {
            (Vec2::Y, size.y, size.x)
        };
        let (hl, hw) = (l * 0.5, w * 0.5);
        let outline = [
            Vec2::new(-hl, -hw),
            Vec2::new(hl, -hw),
            Vec2::new(hl, hw),
            Vec2::new(-hl, hw),
        ];
        let t = self.tf - self.rng.range(0.0, 0.0012);
        let mid = (lo + hi) * 0.5;
        self.board_slab(f.frame(mid, dir, -self.tf), &outline, t);
        let across = dir.perp();
        let ends: &[f32] = if reach > 0.05 { &[-1.0, 1.0] } else { &[0.0] };
        for &s in ends {
            let jitter = self.rng.range(-0.18, 0.18) * w;
            let at = dir * (s * reach) + across * (mid.dot(across) + jitter);
            self.nails.push((*f, at, t - self.tf));
        }
    }

    /// A diagonal brace filling the rectangle `center ± half` on face `f`, rising to the
    /// right unless `flip`.
    fn brace(&mut self, f: &Face, center: Vec2, half: Vec2, flip: bool) {
        let w = self.w;
        let diag = Vec2::new(half.x, if flip { -half.y } else { half.y }).normalize();
        let nd = diag.perp();
        let wb = w * 0.92;
        let rect = [
            Vec2::new(-half.x, -half.y),
            Vec2::new(half.x, -half.y),
            Vec2::new(half.x, half.y),
            Vec2::new(-half.x, half.y),
        ];
        let band = clip(&clip(&rect, nd, wb * 0.5), -nd, wb * 0.5);
        let local: Vec<Vec2> = band
            .iter()
            .map(|p| Vec2::new(p.dot(diag), p.dot(nd)))
            .collect();
        let tb = self.tf * 0.85;
        self.board_slab(f.frame(center, diag, -self.tf), &local, tb);
        // Every vertex of the clipped band lies on an end cut, so the nearest one on each
        // side is where the band stops being full width.
        for s in [-1.0f32, 1.0] {
            let reach = local
                .iter()
                .map(|p| p.x * s)
                .filter(|x| *x > 0.0)
                .fold(f32::MAX, f32::min);
            let across: &[f32] = if self.lean >= 1 { &[0.0] } else { &[-1.0, 1.0] };
            for &t in across {
                let at = center + diag * (s * (reach - w * 0.45)) + nd * (t * wb * 0.25);
                self.nails.push((*f, at, tb - self.tf));
            }
        }
    }

    /// Infill planks along u across the whole face, of slightly different widths with
    /// `gap` between them, their tops `tf` below the face plane and `depth` deep. Planks
    /// entirely under the frame are left out.
    fn planks(&mut self, f: &Face, depth: f32, gap: f32) {
        let (e, w) = (f.ext, self.w);
        let span = 2.0 * e.y;
        let count = ((span / w).round() as usize).max(3);
        let widths: Vec<f32> = (0..count).map(|_| self.rng.range(0.8, 1.2)).collect();
        let scale = (span - gap * (count - 1) as f32) / widths.iter().sum::<f32>();
        let mut lo = -e.y;
        for pw in widths.into_iter().map(|x| x * scale) {
            let hi = lo + pw;
            if lo < e.y - w && hi > -e.y + w {
                let tint = wood_tint(&mut self.rng, self.base * 0.93);
                let uv = self.uv();
                let lift = self.rng.range(-0.001, 0.0);
                let m = f.frame(Vec2::new(0.0, (lo + hi) * 0.5), Vec2::X, -self.tf + lift);
                self.wood.colored(tint, |b| {
                    b.placed(m, |b| plank(b, e.x - w * 0.5, pw, depth, 0.0025, uv))
                });
            }
            lo = hi + gap;
        }
    }
}

/// A wooden shipping crate filling the box collider with half extents `half` (meters).
///
/// Each face has a perimeter frame of boards standing 1.6–2 cm proud of infill planks of
/// uneven widths laid with 4 mm gaps, nail heads, and on the larger faces a diagonal brace
/// (on long faces a middle stile and two braces in a V); steel brackets wrap the eight
/// corners. Board widths scale from 8 cm (0.22 m half extents) to 12 cm (0.45 m), so small
/// and large crates both read right. `seed` varies per-board tint, UV offsets, plank widths,
/// brace directions and small irregularities. Outer extents equal `half` exactly: the bolt
/// heads on the brackets touch the collider, the brackets' faces are [`CRATE_BOLT_HEIGHT`]
/// inside it and the wood 5.5 mm (so stacked crates show a thin shadow line between them).
/// About 2 300–3 000 triangles in all, depending on size and shape: crates that would go
/// over 3 000 (small, tall ones) shed brace nails, then the extra braces on long faces.
pub fn crate_box(half: Vec3, seed: u32) -> CrateMeshes {
    let mut lean = 0;
    loop {
        let c = build_crate(half, seed, lean);
        if lean == 2 || c.wood.triangle_count() + c.metal.triangle_count() <= CRATE_TRIANGLES {
            return c;
        }
        lean += 1;
    }
}

/// The triangle budget [`crate_box`] keeps to.
const CRATE_TRIANGLES: usize = 3000;

fn build_crate(half: Vec3, seed: u32, lean: u32) -> CrateMeshes {
    let th = CRATE_BRACKET_THICKNESS;
    let plates = half - Vec3::splat(CRATE_BOLT_HEIGHT);
    let h = plates - Vec3::splat(th);
    let mut sorted = half.to_array();
    sorted.sort_by(f32::total_cmp);
    let k = ((sorted[1] - 0.22) / 0.23).clamp(0.0, 1.0);
    let mut rng = Rng::new(seed);
    let base = rng.range(0.92, 1.05);
    let mut c = Carpenter {
        wood: MeshBuilder::new(),
        nails: Vec::new(),
        rng,
        base,
        w: 0.08 + 0.04 * k,
        tf: 0.016 + 0.004 * k,
        lean,
    };
    let (w, tf) = (c.w, c.tf);
    let tp = 0.012; // plank thickness seen in the gaps
    let jg = 0.0015; // joint gap between butting boards
    let sb = 0.85 * w + 0.016; // bracket plate size

    // The dark inside, seen through the gaps between planks.
    c.wood.colored(Vec4::new(0.24, 0.2, 0.17, 1.0), |b| {
        b.rounded_box(h - Vec3::splat(tf + tp * 0.7), 0.0, 0)
    });
    let area = |axis: usize| match axis {
        0 => h.y * h.z,
        1 => h.x * h.z,
        _ => h.x * h.y,
    };
    let side_max = area(0).max(area(2));
    for axis in 0..3 {
        for sign in [-1.0f32, 1.0] {
            let f = Face::new(h, axis, sign, tf);
            let e = f.ext;
            c.planks(&f, tp, 0.004);

            // The frame: two boards over the full length, two between them.
            let boards: [(Vec2, Vec2); 4] = if e.x >= e.y {
                [
                    (Vec2::new(-e.x, e.y - w), Vec2::new(e.x, e.y)),
                    (Vec2::new(-e.x, -e.y), Vec2::new(e.x, -e.y + w)),
                    (
                        Vec2::new(e.x - w, -e.y + w + jg),
                        Vec2::new(e.x, e.y - w - jg),
                    ),
                    (
                        Vec2::new(-e.x, -e.y + w + jg),
                        Vec2::new(-e.x + w, e.y - w - jg),
                    ),
                ]
            } else {
                [
                    (Vec2::new(e.x - w, -e.y), Vec2::new(e.x, e.y)),
                    (Vec2::new(-e.x, -e.y), Vec2::new(-e.x + w, e.y)),
                    (
                        Vec2::new(-e.x + w + jg, e.y - w),
                        Vec2::new(e.x - w - jg, e.y),
                    ),
                    (
                        Vec2::new(-e.x + w + jg, -e.y),
                        Vec2::new(e.x - w - jg, -e.y + w),
                    ),
                ]
            };
            for (lo, hi) in boards {
                let size = hi - lo;
                let along = if size.x >= size.y { e.x } else { e.y };
                // Nails just inside the corner brackets.
                c.board(&f, lo, hi, along - sb - 0.03);
            }

            let brace = if axis == 1 {
                area(1) > 1.2 * side_max
            } else {
                area(axis) >= 0.6 * area(1).max(side_max)
            };
            if brace {
                let inner = Vec2::new(e.x - w - jg, e.y - w - jg);
                let flip = c.rng.next() < 0.5;
                if lean < 2 && inner.max_element() > 1.7 * inner.min_element() {
                    // A long face: a stile across the middle and a brace in each half.
                    let long = if inner.x >= inner.y { Vec2::X } else { Vec2::Y };
                    let short = long.perp().abs();
                    let reach = inner.dot(short);
                    let stile = short * reach + long * (w * 0.5);
                    c.board(&f, -stile, stile, reach - w * 0.6);
                    let sub = short * reach + long * ((inner.dot(long) - w * 0.5 - jg) * 0.5);
                    let off = long * ((inner.dot(long) + w * 0.5 + jg) * 0.5);
                    c.brace(&f, off, sub, flip);
                    c.brace(&f, -off, sub, !flip);
                } else {
                    c.brace(&f, Vec2::ZERO, inner, flip);
                }
            }
        }
    }

    // Steel corner brackets: three plates per corner, each with a bolt.
    let mut metal = MeshBuilder::new();
    for corner in 0..8 {
        let s = Vec3::new(
            if corner & 1 != 0 { 1.0 } else { -1.0 },
            if corner & 2 != 0 { 1.0 } else { -1.0 },
            if corner & 4 != 0 { 1.0 } else { -1.0 },
        );
        for axis in 0..3 {
            let f = Face::new(plates, axis, s[axis], th);
            let cs = f.coords(s);
            let (cu, cv) = (cs.x.signum(), cs.y.signum());
            let (eu, ev) = (f.ext.x, f.ext.y);
            // A square plate with its inner corner rounded off.
            let rc = sb * 0.55;
            let mut pts = vec![Vec2::new(eu, ev), Vec2::new(eu - sb, ev)];
            pts.extend(arc(
                Vec2::new(eu - sb + rc, ev - sb + rc),
                rc,
                180.0,
                270.0,
                3,
            ));
            pts.push(Vec2::new(eu, ev - sb));
            for p in &mut pts {
                *p *= Vec2::new(cu, cv);
            }
            metal.placed(f.frame(Vec2::ZERO, Vec2::X, -th), |b| {
                slab(b, &pts, th, th, Vec2::ZERO)
            });
            let bolt = Vec2::new(cu * (eu - 0.36 * sb), cv * (ev - 0.36 * sb));
            metal.placed(f.frame(bolt, Vec2::X, 0.0), |b| {
                dome(b, 0.0075, CRATE_BOLT_HEIGHT, 10, 1)
            });
        }
    }
    // Nail heads: duller than the brackets.
    for (f, at, z) in std::mem::take(&mut c.nails) {
        let k = 0.72 * c.rng.range(0.85, 1.1);
        metal.colored(Vec4::new(k, k * 0.97, k * 0.94, 1.0), |b| {
            b.placed(f.frame(at, Vec2::X, z), |b| dome(b, 0.0055, 0.0018, 8, 1))
        });
    }
    CrateMeshes {
        wood: c.wood.build(),
        metal: metal.build(),
    }
}

// ---------------------------------------------------------------------------------------------
// Steel drum
// ---------------------------------------------------------------------------------------------

/// A 200 l steel drum standing along local Y, for a cylinder collider of `radius` and
/// `half_height` (meters; the proving ground's is 0.3 × 0.42).
///
/// Rolled chimes at both ends, two rolling hoops, shallow corrugation beads, a recessed lid
/// with an embossed ring and two bung caps (2" and ¾"). The chimes and hoops touch the
/// collider's radius, the chime rims its ends. One mesh for a painted-steel material;
/// UVs run around the drum (U, meters at the full radius) and along its profile (V).
/// 2 990 triangles.
pub fn steel_drum(radius: f32, half_height: f32) -> Mesh {
    let (r, h) = (radius, half_height);
    let ct = 0.013; // chime bead thickness
    let ch = 0.028; // chime height
    let rt = 0.0065; // chime shoulder radius
    let rb = r - 0.010; // body wall
    let ri = r - ct; // chime inner wall
    let head = 0.016; // the heads sit this far inside the chime rims
    let rc = ct * 0.5;

    // Profile, bottom center to top center: the bottom head, the rolled bottom chime, the
    // body with two rolling hoops and a corrugation bead, the top chime and the lid.
    let mut p = vec![Vec2::new(0.0, -h + head), Vec2::new(ri, -h + head)];
    p.extend(arc(Vec2::new(ri + rc, -h + rc), rc, 180.0, 360.0, 2));
    p.extend(arc(Vec2::new(r - rt, -h + ch - rt), rt, 0.0, 90.0, 1));
    p.push(Vec2::new(rb, -h + ch + 0.004));
    let body_start = p.len() - 1;
    let yh = h * 0.345;
    for (y, hoop) in [(-yh, true), (0.0, false), (yh, true)] {
        if hoop {
            p.extend([
                Vec2::new(rb, y - 0.024),
                Vec2::new(r - 0.0045, y - 0.0075),
                Vec2::new(r, y),
                Vec2::new(r - 0.0045, y + 0.0075),
                Vec2::new(rb, y + 0.024),
            ]);
        } else {
            p.extend([
                Vec2::new(rb, y - 0.012),
                Vec2::new(rb + 0.003, y),
                Vec2::new(rb, y + 0.012),
            ]);
        }
    }
    p.push(Vec2::new(rb, h - ch - 0.004));
    let body_end = p.len();
    p.extend(arc(Vec2::new(r - rt, h - ch + rt), rt, 270.0, 360.0, 1));
    p.extend(arc(Vec2::new(ri + rc, h - rc), rc, 0.0, 180.0, 2));
    // The lid, with an embossed stiffening ring.
    let yl = h - head;
    p.extend([
        Vec2::new(ri, yl),
        Vec2::new(0.5 * r, yl),
        Vec2::new(0.46 * r, yl + 0.006),
        Vec2::new(0.42 * r, yl),
        Vec2::new(0.0, yl),
    ]);

    let mut b = MeshBuilder::new();
    b.lathe(&p, 44, 50.0);

    // The body's welded side seam: a low ridge following the profile from chime to chime.
    let seam = &p[body_start..body_end];
    let (sa, ca) = 0.9f32.sin_cos();
    let radial = Vec3::new(ca, 0.0, -sa);
    let tangent = Vec3::new(-sa, 0.0, -ca);
    b.grid(seam.len(), 3, |row, col| {
        let q = seam[row];
        let d = seam[(row + 1).min(seam.len() - 1)] - seam[row.saturating_sub(1)];
        let n2 = Vec2::new(d.y, -d.x).normalize();
        let n = radial * n2.x + Vec3::Y * n2.y;
        let side = col as f32 - 1.0;
        let lift = if col == 1 { 0.0012 } else { -0.0003 };
        let pos = radial * q.x + Vec3::Y * q.y + n * lift + tangent * (side * 0.0022);
        let nrm = (n + tangent * (side * 0.7)).normalize();
        (pos, nrm, Vec2::new(side * 0.0022, q.y))
    });

    // Bung caps: a 2" and a ¾" one on opposite sides of the lid, each with a wrench bar.
    for (at, scale, sides) in [(r * 0.75, 1.0f32, 16u32), (-r * 0.72, 0.6, 10)] {
        let s = scale;
        let collar = [
            Vec2::new(0.040 * s, -0.002),
            Vec2::new(0.040 * s, 0.0035),
            Vec2::new(0.036 * s, 0.0072),
            Vec2::new(0.029 * s, 0.0072),
            Vec2::new(0.027 * s, 0.0105),
            Vec2::new(0.0, 0.0105),
        ];
        let base = Vec3::new(at, yl, 0.0);
        b.colored(Vec4::splat(0.92).with_w(1.0), |b| {
            b.placed(Mat4::from_translation(base), |b| {
                b.lathe(&collar, sides, 50.0)
            });
            let bar = [
                Vec2::new(-0.017 * s, -0.004 * s),
                Vec2::new(0.017 * s, -0.004 * s),
                Vec2::new(0.017 * s, 0.004 * s),
                Vec2::new(-0.017 * s, 0.004 * s),
            ];
            // Local Z (up off the plug) → +Y.
            let m = frame(Vec3::Z, Vec3::X, Vec3::Y, base + Vec3::Y * 0.0102);
            b.placed(m, |b| slab(b, &bar, 0.0045, 0.0015, Vec2::ZERO));
        });
    }
    b.build()
}

// ---------------------------------------------------------------------------------------------
// Balls
// ---------------------------------------------------------------------------------------------

/// A smooth ball of `radius` meters for a rubber material: an evenly tessellated cube sphere
/// (no poles) with analytic normals and metric UVs (one azimuthal island per octant, so seams
/// lie on three great circles). 1 728 triangles.
pub fn ball(radius: f32) -> Mesh {
    ball_mesh(radius, false)
}

/// A playground ball of `radius` meters: [`ball`] with shallow seam grooves along three great
/// circles (eight panels, like a classic rubber playground ball). The UV seams hide in the
/// grooves, which are vertex-darkened. The panels touch the collider's radius. 1 728
/// triangles.
pub fn playground_ball(radius: f32) -> Mesh {
    ball_mesh(radius, true)
}

#[derive(Clone, Copy)]
enum Sample {
    /// An angle on the cube face, radians.
    Angle(f32),
    /// An angular distance from the great circle through the face center, radians (solved
    /// per point, so groove rows follow the circle exactly).
    Groove(f32),
}

fn ball_mesh(radius: f32, grooves: bool) -> Mesh {
    let gw = (0.016 * radius).max(0.004); // groove half width, m
    let gd = (0.010 * radius).max(0.002); // groove depth, m
    let dw = gw / radius;
    let deg = |d: f32| Sample::Angle(d.to_radians());
    let samples: Vec<Sample> = if grooves {
        vec![
            Sample::Groove(0.0),
            Sample::Groove(0.55 * dw),
            Sample::Groove(dw),
            deg(11.25),
            deg(22.5),
            deg(33.75),
            deg(45.0),
        ]
    } else {
        (0..=6).map(|k| deg(7.5 * k as f32)).collect()
    };
    let profile = |t: f32| {
        if t >= 1.0 { 0.0 } else { (1.0 - t * t).powi(2) }
    };
    let groove = |d: Vec3| -> f32 {
        if !grooves {
            return 0.0;
        }
        [d.x, d.y, d.z]
            .iter()
            .map(|c| profile(c.abs().min(1.0).asin() / dw))
            .fold(0.0, f32::max)
    };
    let surface = |d: Vec3| d * (radius - gd * groove(d));
    let normal = |d: Vec3| -> Vec3 {
        let e1 = d.any_orthonormal_vector();
        let e2 = d.cross(e1);
        let eps = 2e-4;
        let at = |o: Vec3| surface((d + o).normalize());
        let n = (at(e1 * eps) - at(-e1 * eps)).cross(at(e2 * eps) - at(-e2 * eps));
        let n = n.normalize_or(d);
        if n.dot(d) < 0.0 { -n } else { n }
    };

    let mut b = MeshBuilder::new();
    let m = samples.len();
    for axis in 0..3 {
        for sign in [-1.0f32, 1.0] {
            let n = Vec3::AXES[axis] * sign;
            let u = Vec3::AXES[(axis + 1) % 3];
            let v = Vec3::AXES[(axis + 2) % 3];
            for qa in [-1.0f32, 1.0] {
                for qb in [-1.0f32, 1.0] {
                    // UVs: an azimuthal map around the octant's center.
                    let oc = (n + u * qa + v * qb).normalize();
                    let t1 = Vec3::Y.cross(oc).normalize();
                    let t2 = oc.cross(t1);
                    b.grid(m, m, |r, c| {
                        let init = |s: Sample| match s {
                            Sample::Angle(a) | Sample::Groove(a) => a,
                        };
                        let (mut a, mut e) = (init(samples[c]), init(samples[r]));
                        for _ in 0..4 {
                            if let Sample::Groove(dist) = samples[c] {
                                a = (dist.tan() / e.cos()).atan();
                            }
                            if let Sample::Groove(dist) = samples[r] {
                                e = (dist.tan() / a.cos()).atan();
                            }
                        }
                        let d = (n + u * (qa * a.tan()) + v * (qb * e.tan())).normalize();
                        let theta = d.dot(oc).clamp(-1.0, 1.0).acos();
                        let proj = Vec2::new(d.dot(t1), d.dot(t2));
                        let uv = proj.normalize_or(Vec2::X) * (radius * theta);
                        (surface(d), normal(d), uv)
                    });
                }
            }
        }
    }
    if grooves {
        for vtx in &mut b.mesh.vertices {
            let g = groove(Vec3::from(vtx.position).normalize());
            let k = 1.0 - 0.45 * g;
            vtx.color = [k, k, k, 1.0];
        }
    }
    b.build()
}

// ---------------------------------------------------------------------------------------------
// Chain
// ---------------------------------------------------------------------------------------------

/// The proving ground's chain: segment length (meters, joint to joint) …
pub const CHAIN_SEGMENT_LENGTH: f32 = 0.8;
/// … and wire radius (meters). Links are 0.14 m wide, the capsule colliders' diameter.
pub const CHAIN_WIRE: f32 = 0.02;

/// How many links a chain segment of `length` holds with wire of radius `wire`, and their
/// pitch (inner link length, meters). The count is even, so link orientation keeps
/// alternating from one segment to the next.
pub fn chain_layout(length: f32, wire: f32) -> (usize, f32) {
    let n = ((length / (wire * 6.6) * 0.5).round() as usize).max(1) * 2;
    (n, length / n as f32)
}

/// A rigid chain segment along local Y, `length` meters joint to joint (centered), made of
/// heavy welded links of wire radius `wire`.
///
/// Links alternate 90° and the first and last ones end exactly at the segment's ends, so
/// consecutive segments ball-jointed there interlock and read as one continuous chain. For
/// the proving ground's capsules (r 0.07, half-height 0.33, 0.8 m apart) use
/// [`CHAIN_SEGMENT_LENGTH`] and [`CHAIN_WIRE`]: six links 0.14 m wide. For a forged-steel
/// material; links are slightly vertex-tinted one from another. 432 triangles per link.
pub fn chain_segment(length: f32, wire: f32) -> Mesh {
    chain_links(length, wire, 0)
}

/// [`chain_segment`] without its two bottom links: for the segment the wrecking ball hangs
/// from, whose shackle takes their place (see [`wrecking_ball`]).
pub fn chain_end_segment(length: f32, wire: f32) -> Mesh {
    chain_links(length, wire, 2)
}

fn chain_links(length: f32, wire: f32, skip: usize) -> Mesh {
    let (n, pitch) = chain_layout(length, wire);
    let mut b = MeshBuilder::new();
    let mut rng = Rng::new(n as u32 * 7919 + (wire * 1e4) as u32);
    for i in 0..n {
        let tint = rng.range(0.82, 1.0);
        let warm = rng.range(-0.03, 0.05);
        if i < skip {
            continue;
        }
        let y = -length * 0.5 + pitch * (i as f32 + 0.5);
        let turn = if i % 2 == 1 {
            Quat::from_rotation_y(FRAC_PI_2)
        } else {
            Quat::IDENTITY
        };
        let m = Mat4::from_rotation_translation(turn, Vec3::Y * y);
        let c = Vec4::new(tint * (1.0 + warm), tint, tint * (1.0 - warm), 1.0);
        b.colored(c, |b| b.placed(m, |b| link(b, pitch, wire)));
    }
    b.build()
}

/// One chain link in the XY plane, long axis Y: inner length `pitch`, wire radius `wire`,
/// inner width 3 × `wire`.
fn link(b: &mut MeshBuilder, pitch: f32, wire: f32) {
    let bend = 2.5 * wire; // center-line bend radius
    let straight = (pitch * 0.5 + wire - bend).max(0.0);
    let mut path: Vec<Vec3> = arc(Vec2::new(0.0, straight), bend, 0.0, 180.0, 8)
        .iter()
        .map(|p| p.extend(0.0))
        .collect();
    path.extend(
        arc(Vec2::new(0.0, -straight), bend, 180.0, 360.0, 8)
            .iter()
            .map(|p| p.extend(0.0)),
    );
    b.tube(&path, wire, 12, true, false);
}

// ---------------------------------------------------------------------------------------------
// Wrecking ball
// ---------------------------------------------------------------------------------------------

/// A steel wrecking ball of `radius` meters (the proving ground's is 0.8) with a welded
/// lifting lug and a shackle on top (local +Y), and a weld bead around its equator.
///
/// The lug, shackle and the equator bead (4.5 mm proud) are render-only extras on the sphere
/// collider. The shackle is sized for the proving-ground chain ([`CHAIN_SEGMENT_LENGTH`],
/// [`CHAIN_WIRE`]) joined at the sphere's top: hang the ball from a
/// [`chain_end_segment`], whose lowest link threads the shackle's bow. For a forged-steel
/// material; the weld's heat-tinted band is vertex-colored. 3 916 triangles.
pub fn wrecking_ball(radius: f32) -> Mesh {
    let r = radius;
    let mut b = MeshBuilder::new();

    // The ball: two hemispheres and the weld bead between them.
    let polar = |rad: f32, deg: f32| {
        let a = deg.to_radians();
        Vec2::new((rad * a.cos()).max(0.0), rad * a.sin())
    };
    let toe = (0.013 / r).to_degrees();
    let steps = 11;
    let mut prof: Vec<Vec2> = (0..=steps)
        .map(|k| polar(r, -90.0 + (90.0 - toe) * k as f32 / steps as f32))
        .collect();
    prof.extend([
        polar(r + 0.0028, -toe * 0.45),
        polar(r + 0.0045, 0.0),
        polar(r + 0.0028, toe * 0.45),
    ]);
    prof.extend((0..=steps).map(|k| polar(r, toe + (90.0 - toe) * k as f32 / steps as f32)));
    let start = b.mesh.vertices.len();
    b.lathe(&prof, 44, 60.0);
    // Heat tint along the weld: darker and a little blue, fading out over a few centimeters.
    for v in &mut b.mesh.vertices[start..] {
        let y = v.position[1].abs();
        let k = (y / 0.05).clamp(0.0, 1.0);
        let k = k * k * (3.0 - 2.0 * k);
        let bead = if y < 0.006 { 0.82 } else { 1.0 };
        v.color = [
            (0.7 + 0.3 * k) * bead,
            (0.71 + 0.29 * k) * bead,
            (0.78 + 0.22 * k) * bead,
            1.0,
        ];
    }

    // Lug and shackle, built with the pin along X and then turned so the pin runs along Z:
    // the bow then threads the first link of a `chain_end_segment` (its plane is XY).
    let (_, pitch) = chain_layout(CHAIN_SEGMENT_LENGTH, CHAIN_WIRE);
    b.placed(Mat4::from_rotation_y(FRAC_PI_2), |b| {
        lug_and_shackle(b, r, 2.0 * pitch)
    });
    b.build()
}

/// The wrecking ball's welded lifting lug and its D-shackle on a ball of radius `r`, the pin
/// along X; the inside of the bow's top is `reach` above the ball.
fn lug_and_shackle(b: &mut MeshBuilder, r: f32, reach: f32) {
    // The lug: a pad eye plate in the YZ plane, welded on.
    let hr = 0.034; // hole radius
    let ro = 0.068; // outer radius around the hole
    let tl = 0.06; // plate thickness
    let hc = r + 0.078; // hole center height
    let yb = r - 0.03; // plate bottom (inside the ball)
    let wb = 0.105; // half width at the bottom
    let mut outline: Vec<Vec2> = arc(Vec2::new(0.0, hc), ro, 0.0, 180.0, 12);
    let (lt, lb) = (Vec2::new(-ro, hc), Vec2::new(-wb, yb));
    let (rt, rb) = (Vec2::new(ro, hc), Vec2::new(wb, yb));
    outline.extend((1..=3).map(|k| lt.lerp(lb, k as f32 / 3.0)));
    outline.extend((1..3).map(|k| lb.lerp(rb, k as f32 / 3.0)));
    outline.extend((0..3).map(|k| rb.lerp(rt, k as f32 / 3.0)));
    // Lug local: x across, y up, z through → z → X, x → -Z.
    b.placed(Mat4::from_rotation_y(FRAC_PI_2), |b| {
        eye_plate(b, &outline, Vec2::new(0.0, hc), hr, tl, 0.006)
    });

    // Fillet weld along the line where the lug meets the ball.
    let foot = ro + (wb - ro) * ((hc - r) / (hc - yb)); // lug half width there
    let (fx, fz, fr) = (tl * 0.5, foot, 0.012);
    let mut path = Vec::new();
    for (cx, cz, a0) in [
        (fx - fr, fz - fr, 0.0f32),
        (-(fx - fr), fz - fr, 90.0),
        (-(fx - fr), -(fz - fr), 180.0),
        (fx - fr, -(fz - fr), 270.0),
    ] {
        for p in arc(Vec2::new(cx, cz), fr, a0, a0 + 90.0, 3) {
            path.push(Vec3::new(p.x, 0.0, p.y));
        }
        // Points along the long side that follows, so the bead hugs the ball's curve.
        if a0 == 90.0 || a0 == 270.0 {
            let x = if a0 == 90.0 { -fx } else { fx };
            let from = if a0 == 90.0 { fz - fr } else { -(fz - fr) };
            for k in 1..4 {
                path.push(Vec3::new(x, 0.0, from * (1.0 - 2.0 * k as f32 / 4.0)));
            }
        }
    }
    for p in &mut path {
        p.y = (r * r - p.x * p.x - p.z * p.z).max(0.0).sqrt();
    }
    b.colored(Vec4::new(0.7, 0.7, 0.76, 1.0), |b| {
        b.tube(&path, 0.013, 6, true, false)
    });

    // A bolt-type D-shackle: the pin bears on the top of the lug's hole.
    let rs = 0.026; // bow wire radius
    let xl = 0.064; // the legs' center lines, either side of the lug
    let r_pin = 0.03;
    let y_pin = hc + (hr - r_pin);
    let yc = r + reach + rs - xl; // center of the bow's round top
    let mut bow = vec![
        Vec3::new(-xl, y_pin, 0.0),
        Vec3::new(-xl, (y_pin + yc) * 0.5, 0.0),
    ];
    bow.extend(
        arc(Vec2::new(0.0, yc), xl, 180.0, 0.0, 10)
            .iter()
            .map(|p| p.extend(0.0)),
    );
    bow.extend([
        Vec3::new(xl, (y_pin + yc) * 0.5, 0.0),
        Vec3::new(xl, y_pin, 0.0),
    ]);
    // The forged bow a shade darker than the machined pin.
    let ear_half = 0.024;
    b.colored(Vec4::new(0.94, 0.94, 0.94, 1.0), |b| {
        b.tube(&bow, rs, 12, false, false);
        for side in [-1.0f32, 1.0] {
            let m = Mat4::from_translation(Vec3::new(side * xl, y_pin, 0.0)) * y_to_x();
            b.placed(m, |b| boss(b, 0.056, ear_half, 0.008, 18));
        }
    });
    // The pin: a collar head on one side, a nut on the other.
    let x0 = -(xl + ear_half + 0.002);
    let x1 = xl + ear_half + 0.036;
    let pin = [
        Vec2::new(0.0, x0),
        Vec2::new(r_pin - 0.003, x0),
        Vec2::new(r_pin, x0 + 0.003),
        Vec2::new(r_pin, x1 - 0.004),
        Vec2::new(r_pin - 0.004, x1),
        Vec2::new(0.0, x1),
    ];
    b.colored(Vec4::ONE, |b| {
        b.placed(Mat4::from_translation(Vec3::Y * y_pin) * y_to_x(), |b| {
            b.lathe(&pin, 14, 50.0)
        });
        let head_x = -(xl + ear_half) - 0.0105;
        let head = Mat4::from_translation(Vec3::new(head_x, y_pin, 0.0)) * y_to_x();
        b.placed(head, |b| boss(b, 0.05, 0.0105, 0.004, 16));
        let hex: Vec<Vec2> = (0..6)
            .map(|k| {
                let a = (30.0 + 60.0 * k as f32).to_radians();
                Vec2::new(a.cos(), a.sin()) * 0.05
            })
            .collect();
        let nut = Mat4::from_translation(Vec3::new(xl + ear_half + 0.0135, y_pin, 0.0))
            * Mat4::from_rotation_y(FRAC_PI_2);
        b.placed(nut, |b| b.extrude(&hex, 0.025, 0.004, 30.0));
    });
}

/// A plate with a round hole (a pad eye): `outline` (local XY, counter-clockwise, star-shaped
/// around the hole center `hc`, starting anywhere) extruded `thickness` along Z (centered),
/// a hole of radius `hr`, all edges rounded by `bevel`.
fn eye_plate(b: &mut MeshBuilder, outline: &[Vec2], hc: Vec2, hr: f32, thickness: f32, bevel: f32) {
    let n = outline.len();
    let normals: Vec<Vec2> = (0..n)
        .map(|i| {
            let d = (outline[(i + 1) % n] - outline[(i + n - 1) % n]).normalize();
            Vec2::new(d.y, -d.x)
        })
        .collect();
    let dirs: Vec<Vec2> = outline.iter().map(|p| (*p - hc).normalize()).collect();
    let zf = thickness * 0.5;
    let zw = zf - bevel;
    // Per ring: (position, normal) for every outline index.
    let outer_w = |i: usize, z: f32| (outline[i].extend(z), normals[i].extend(0.0));
    let outer_c = |i: usize, z: f32| {
        let p = outline[i] - normals[i] * bevel;
        (p.extend(z), Vec3::Z * z.signum())
    };
    let hole_w = |i: usize, z: f32| ((hc + dirs[i] * hr).extend(z), (-dirs[i]).extend(0.0));
    let hole_c = |i: usize, z: f32| {
        let p = hc + dirs[i] * (hr + bevel);
        (p.extend(z), Vec3::Z * z.signum())
    };
    type Ring<'a> = &'a dyn Fn(usize, f32) -> (Vec3, Vec3);
    let strips: [(Ring, f32, Ring, f32); 8] = [
        (&outer_w, -zw, &outer_w, zw),
        (&outer_w, zw, &outer_c, zf),
        (&outer_c, zf, &hole_c, zf),
        (&hole_c, zf, &hole_w, zw),
        (&hole_w, zw, &hole_w, -zw),
        (&hole_w, -zw, &hole_c, -zf),
        (&hole_c, -zf, &outer_c, -zf),
        (&outer_c, -zf, &outer_w, -zw),
    ];
    let mut along = vec![0.0f32; n + 1];
    for i in 0..n {
        along[i + 1] = along[i] + outline[i].distance(outline[(i + 1) % n]);
    }
    for (ra, za, rb, zb) in strips {
        let flat = za == zb;
        b.grid(2, n + 1, |row, col| {
            let i = col % n;
            let (p, nn) = if row == 0 { ra(i, za) } else { rb(i, zb) };
            let uv = if flat {
                p.truncate()
            } else {
                Vec2::new(along[col], p.z)
            };
            (p, nn, uv)
        });
    }
}

// ---------------------------------------------------------------------------------------------
// Dominoes
// ---------------------------------------------------------------------------------------------

/// Pip positions on a 3 × 3 grid (column, row from the bottom) for 0..=9 pips.
fn pip_layout(count: u8) -> &'static [(i8, i8)] {
    match count {
        0 => &[],
        1 => &[(0, 0)],
        2 => &[(-1, 1), (1, -1)],
        3 => &[(-1, 1), (0, 0), (1, -1)],
        4 => &[(-1, -1), (-1, 1), (1, -1), (1, 1)],
        5 => &[(-1, -1), (-1, 1), (0, 0), (1, -1), (1, 1)],
        6 => &[(-1, -1), (-1, 0), (-1, 1), (1, -1), (1, 0), (1, 1)],
        7 => &[(-1, -1), (-1, 0), (-1, 1), (0, 0), (1, -1), (1, 0), (1, 1)],
        8 => &[
            (-1, -1),
            (-1, 0),
            (-1, 1),
            (0, -1),
            (0, 1),
            (1, -1),
            (1, 0),
            (1, 1),
        ],
        _ => &[
            (-1, -1),
            (-1, 0),
            (-1, 1),
            (0, -1),
            (0, 0),
            (0, 1),
            (1, -1),
            (1, 0),
            (1, 1),
        ],
    }
}

/// A rounded box as [`MeshBuilder::rounded_box`] builds it with one segment per 45°, but with
/// extra sample lines in the flat area of each axis (so faces share every edge vertex).
struct RoundBox {
    half: Vec3,
    radius: f32,
    inner: Vec3,
}

impl RoundBox {
    fn new(half: Vec3, radius: f32) -> Self {
        let radius = radius.clamp(0.0, half.min_element());
        Self {
            half,
            radius,
            inner: half - Vec3::splat(radius),
        }
    }

    fn samples(&self, axis: usize, extra: &[f32]) -> Vec<f32> {
        let c = self.inner[axis];
        let mut s = vec![-self.half[axis], -c, c, self.half[axis]];
        s.extend(extra.iter().copied().filter(|x| x.abs() < c - 1e-5));
        s.sort_by(f32::total_cmp);
        s.dedup_by(|a, b| (*a - *b).abs() < 1e-6);
        s
    }

    /// Face axes `(ua, va)` with U along the face's longer side.
    fn face_axes(&self, axis: usize) -> (usize, usize) {
        let (i, j) = ((axis + 1) % 3, (axis + 2) % 3);
        if self.half[i] >= self.half[j] {
            (i, j)
        } else {
            (j, i)
        }
    }

    fn point(&self, axis: usize, sign: f32, su: f32, sv: f32) -> (Vec3, Vec3, Vec2) {
        let (ua, va) = self.face_axes(axis);
        let mut q = Vec3::ZERO;
        q[axis] = sign * self.half[axis];
        q[ua] = su;
        q[va] = sv;
        let core = q.clamp(-self.inner, self.inner);
        let mut n = (q - core).normalize_or(Vec3::ZERO);
        if n == Vec3::ZERO {
            n[axis] = sign;
        }
        let p = core + n * self.radius;
        (p, n, Vec2::new(p[ua] * sign, p[va]))
    }
}

/// A big domino for a box collider with half extents `half` (the proving ground's is
/// 0.25 × 0.5 × 0.06: 1 m tall, its thin side along local Z, faces ±Z).
///
/// Rounded edges, an engraved center line with a brass spinner, and recessed pips on the
/// front (+Z) face: `pips.0` on the top half (+Y), `pips.1` on the bottom (0..=9 each). An
/// `ivory` domino has black pips, otherwise the body is black with white pips. Colors are
/// vertex colors: use a white (or near-white), fairly glossy material. About 860 + 66
/// triangles per pip (1 656 for double six, under 2 000 for any pips).
pub fn domino(half: Vec3, pips: (u8, u8), ivory: bool) -> Mesh {
    let (body, dot) = if ivory {
        (srgb(0xeee6d0), srgb(0x18181a))
    } else {
        (srgb(0x1b1b1f), srgb(0xf1eee6))
    };
    let rbox = RoundBox::new(half, (0.33 * half.z).min(0.2 * half.x));
    let q = 0.54 * half.x; // pip spacing
    let rp = 0.168 * half.x; // pip radius
    let dp = 0.17 * half.z; // pip depth
    let gw = 0.012 * half.y; // center line half width
    let gd = 0.067 * half.z; // center line depth
    let (g0, g1) = (0.81 * half.x, 0.848 * half.x); // the line ends (full depth to zero)
    let yc = half.y * 0.5;
    let cells = |c: f32| [c - 1.5 * q, c - 0.5 * q, c + 0.5 * q, c + 1.5 * q];
    let mut xs: Vec<f32> = cells(0.0).to_vec();
    xs.extend([-g1, g1]);
    let mut ys: Vec<f32> = cells(yc).to_vec();
    ys.extend(cells(-yc));
    ys.extend([-gw, -0.5 * gw, 0.5 * gw, gw]);
    let extra: [&[f32]; 3] = [&xs, &ys, &[]];

    let mut b = MeshBuilder::new();
    b.color = body;
    for axis in 0..3 {
        let (ua, va) = rbox.face_axes(axis);
        let us = rbox.samples(ua, extra[ua]);
        let vs = rbox.samples(va, extra[va]);
        for sign in [-1.0f32, 1.0] {
            if axis == 2 && sign > 0.0 {
                continue; // the front, below
            }
            b.grid(vs.len(), us.len(), |r, c| {
                rbox.point(axis, sign, us[c], vs[r])
            });
        }
    }

    // The front face: the same grid, with the center line pressed in and pips in their cells.
    let (ua, va) = rbox.face_axes(2);
    let us = rbox.samples(ua, extra[ua]);
    let vs = rbox.samples(va, extra[va]);
    let zf = half.z;
    let flat = |p: Vec3| p.x.abs() <= rbox.inner.x + 1e-5 && p.y.abs() <= rbox.inner.y + 1e-5;
    let line = |x: f32, y: f32| -> (f32, Vec3) {
        let t = y.abs() / gw;
        if t >= 1.0 || x.abs() >= g1 {
            return (0.0, Vec3::Z);
        }
        let (g, dg) = if x.abs() <= g0 {
            (1.0, 0.0)
        } else {
            ((g1 - x.abs()) / (g1 - g0), -x.signum() / (g1 - g0))
        };
        let f = (1.0 - t * t).powi(2);
        let df = -4.0 * t * (1.0 - t * t) * y.signum() / gw;
        let (dzdx, dzdy) = (-gd * f * dg, -gd * g * df);
        (gd * f * g, Vec3::new(-dzdx, -dzdy, 1.0).normalize())
    };
    let mut idx = vec![vec![0u32; us.len()]; vs.len()];
    for (r, row) in idx.iter_mut().enumerate() {
        for (c, slot) in row.iter_mut().enumerate() {
            let (mut p, mut n, uv) = rbox.point(2, 1.0, us[c], vs[r]);
            let mut color = body;
            if flat(p) {
                let (depth, ln) = line(p.x, p.y);
                if depth > 0.0 {
                    p.z -= depth;
                    n = ln;
                    color = body.lerp(dot, (depth / (gd * 0.55)).min(1.0));
                }
            }
            b.color = color;
            *slot = b.vertex(p, n, uv);
        }
    }
    let mut holes: Vec<Vec2> = Vec::new();
    for (count, cy) in [(pips.0, yc), (pips.1, -yc)] {
        for &(i, j) in pip_layout(count) {
            holes.push(Vec2::new(i as f32 * q, cy + j as f32 * q));
        }
    }
    // Rounder pips while they fit the budget (up to 7|7), 12-sided past that.
    let sides = if holes.len() <= 14 { 16 } else { 12 };
    for r in 0..vs.len() - 1 {
        for c in 0..us.len() - 1 {
            // Rows run along X (va), columns along Y (ua).
            let center = Vec2::new((vs[r] + vs[r + 1]) * 0.5, (us[c] + us[c + 1]) * 0.5);
            let corners = [idx[r][c], idx[r + 1][c], idx[r + 1][c + 1], idx[r][c + 1]];
            match holes.iter().find(|h| h.distance(center) < 1e-4) {
                Some(&h) => {
                    b.color = body;
                    pip(&mut b, corners, h, (rp, dp, zf), sides, body, dot);
                }
                None => b.quad(corners[0], corners[1], corners[2], corners[3]),
            }
        }
    }
    // The spinner: a brass dome where the line crosses the middle.
    let floor = zf - gd * 0.5625;
    b.colored(srgb(0xb8923e), |b| {
        b.placed(Mat4::from_translation(Vec3::new(0.0, 0.0, floor)), |b| {
            dome(b, 0.064 * half.x, gd * 0.5625 + 0.0025, 12, 2)
        })
    });
    b.build()
}

/// A recessed pip in a square front-face cell, `sides` (a multiple of 4) around: the
/// dimple is painted `dot` right up to its rim, so the pip's edge is crisp. `corners` are
/// the cell's vertices at (-x, -y), (+x, -y), (+x, +y), (-x, +y) around `center`; `shape` is
/// (pip radius, depth, face plane z).
fn pip(
    b: &mut MeshBuilder,
    corners: [u32; 4],
    center: Vec2,
    shape: (f32, f32, f32),
    sides: usize,
    body: Vec4,
    dot: Vec4,
) {
    let (rp, dp, zf) = shape;
    let uv = |p: Vec3| Vec2::new(p.y, p.x);
    // The rim on the face, joined to the cell's corners.
    b.color = body;
    let rim_ring = |b: &mut MeshBuilder| -> Vec<u32> {
        (0..sides)
            .map(|k| {
                let a = TAU * k as f32 / sides as f32;
                let p = (center + Vec2::new(a.cos(), a.sin()) * rp).extend(zf);
                b.vertex(p, Vec3::Z, uv(p))
            })
            .collect()
    };
    let rim = rim_ring(b);
    // Corner for each quadrant: +x+y, -x+y, -x-y, +x-y.
    let quadrant = [corners[2], corners[3], corners[0], corners[1]];
    for k in 0..sides {
        let mid = (k as f32 + 0.5) / sides as f32 * 4.0;
        b.tri(quadrant[mid as usize % 4], rim[k], rim[(k + 1) % sides]);
    }
    let q4 = sides / 4;
    for (side, k) in [(0usize, 0usize), (1, q4), (2, 2 * q4), (3, 3 * q4)] {
        // Side between quadrant corner `side - 1` and `side`, at rim point `k`.
        let a = quadrant[(side + 3) % 4];
        let c = quadrant[side];
        b.tri(a, c, rim[k]);
    }
    // The dimple: a spherical cap, its lip rounded by the rim's upright normals.
    let rho = (rp * rp + dp * dp) / (2.0 * dp);
    let sphere = center.extend(zf - dp + rho);
    let ring = |b: &mut MeshBuilder, frac: f32, color: Vec4| -> Vec<u32> {
        b.color = color;
        (0..sides)
            .map(|k| {
                let a = TAU * k as f32 / sides as f32;
                let rr = rp * frac;
                let z = sphere.z - (rho * rho - rr * rr).sqrt();
                let p = (center + Vec2::new(a.cos(), a.sin()) * rr).extend(z);
                b.vertex(p, (sphere - p).normalize(), uv(p))
            })
            .collect()
    };
    let lip = ring(b, 0.74, dot);
    b.color = dot;
    let rim = rim_ring(b);
    let bottom = center.extend(zf - dp);
    let c = b.vertex(bottom, Vec3::Z, uv(bottom));
    for k in 0..sides {
        let j = (k + 1) % sides;
        b.quad(rim[k], rim[j], lip[j], lip[k]);
        b.tri(lip[k], lip[j], c);
    }
    b.color = body;
}

/// A crushed rock's corners, about 1 m across (each rock scales them): 11 points on a lumpy
/// ellipsoid, whose convex hull is both the rock's collider and its mesh. `seed` picks one.
pub fn rock_points(seed: u32) -> Vec<Vec3> {
    let mut state = seed.wrapping_mul(0x9e37_79b9) ^ 0x51ed_270b;
    let mut next = move || {
        state ^= state << 13;
        state ^= state >> 17;
        state ^= state << 5;
        (state >> 8) as f32 / (1u32 << 24) as f32
    };
    // A golden-angle spiral spreads the corners evenly; jitter and a lumpy radius break it up.
    const N: usize = 11;
    let squash = Vec3::new(1.0, 0.6 + 0.25 * next(), 0.75 + 0.2 * next());
    (0..N)
        .map(|i| {
            let y = 1.0 - 2.0 * (i as f32 + 0.5) / N as f32;
            let r = (1.0 - y * y).sqrt();
            let a = i as f32 * 2.399_963 + next() * 0.6;
            let dir = Vec3::new(r * a.cos(), y + (next() - 0.5) * 0.4, r * a.sin()).normalize();
            dir * squash * 0.5 * (0.8 + 0.35 * next())
        })
        .collect()
}

/// The convex hull of `points` as a faceted rock: flat facets (fresh fractures), each with
/// its own planar UVs in meters of the unscaled rock, tinted `tint` (sRGB hex: rocks from
/// one quarry differ a little in shade).
pub fn rock(points: &[Vec3], tint: u32) -> Mesh {
    let (verts, tris) = rapier3d::parry::transformation::convex_hull(points);
    let center = verts.iter().copied().sum::<Vec3>() / verts.len().max(1) as f32;
    let mut b = MeshBuilder::new();
    b.color = srgb(tint);
    for [i, j, k] in tris {
        let (p0, p1, p2) = (verts[i as usize], verts[j as usize], verts[k as usize]);
        let mut n = (p1 - p0).cross(p2 - p0).normalize_or_zero();
        if n == Vec3::ZERO {
            continue;
        }
        if n.dot(p0 - center) < 0.0 {
            n = -n;
        }
        // Projected along the facet's normal, so neighboring facets' grain doesn't line up.
        let u = n.any_orthonormal_vector();
        let v = n.cross(u);
        let uv = |p: Vec3| Vec2::new(p.dot(u), p.dot(v));
        let a = b.vertex(p0, n, uv(p0));
        let c = b.vertex(p1, n, uv(p1));
        let d = b.vertex(p2, n, uv(p2));
        b.tri(a, c, d);
    }
    b.build()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tris(m: &Mesh) -> usize {
        m.triangle_count()
    }

    /// Every rock is a closed, outward-facing hull of its collider's points, about 1 m across.
    #[test]
    fn rocks_are_their_collider_hulls_facing_out() {
        for seed in 0..8 {
            let points = rock_points(seed);
            let m = rock(&points, 0xffffff);
            assert!((12..=40).contains(&tris(&m)), "{} triangles", tris(&m));
            let center = points.iter().copied().sum::<Vec3>() / points.len() as f32;
            let p = |i: u32| Vec3::from(m.vertices[i as usize].position);
            for t in m.indices.chunks(3) {
                let (a, b, c) = (p(t[0]), p(t[1]), p(t[2]));
                let n = (b - a).cross(c - a);
                assert!(n.dot(a - center) > 0.0, "rock {seed} has an inward facet");
                assert!(points.iter().any(|q| q.distance(a) < 1e-5));
            }
            let size = points.iter().fold(Vec3::ZERO, |acc, q| acc.max(q.abs())) * 2.0;
            assert!(
                size.max_element() < 1.2 && size.min_element() > 0.5,
                "{size}"
            );
        }
    }

    #[test]
    fn props_fit_their_colliders_and_budgets() {
        for half in [
            Vec3::splat(0.22),
            Vec3::splat(0.45),
            Vec3::new(0.45, 0.2, 0.3),
        ] {
            let c = crate_box(half, 7);
            let mut both = c.wood.clone();
            both.merge(&c.metal);
            let bb = both.aabb();
            assert!((bb.max - half).abs().max_element() < 1e-3, "{half}: {bb:?}");
            assert!((bb.min + half).abs().max_element() < 1e-3, "{half}: {bb:?}");
            assert!(tris(&c.wood) + tris(&c.metal) <= 3000);
        }
        let d = steel_drum(0.3, 0.42);
        let bb = d.aabb();
        assert!((bb.max.x - 0.3).abs() < 2e-3 && (bb.max.y - 0.42).abs() < 1e-3);
        assert!(tris(&d) <= 3000, "drum {}", tris(&d));
        for m in [ball(0.3), playground_ball(0.3)] {
            assert!(tris(&m) <= 2000);
            let r = m
                .vertices
                .iter()
                .map(|v| Vec3::from(v.position).length())
                .fold(0.0f32, f32::max);
            assert!((r - 0.3).abs() < 1e-4);
        }
        let w = wrecking_ball(0.8);
        assert!(tris(&w) <= 4000, "wrecking ball {}", tris(&w));
        for top in 0..=9 {
            for bottom in 0..=9 {
                let dm = domino(Vec3::new(0.25, 0.5, 0.06), (top, bottom), true);
                assert!(tris(&dm) <= 2000, "domino {top}|{bottom}: {}", tris(&dm));
            }
        }
        let dm = domino(Vec3::new(0.25, 0.5, 0.06), (6, 6), true);
        // Only the spinner stands a little proud of the front face.
        assert!(
            (dm.aabb().max - Vec3::new(0.25, 0.5, 0.06))
                .abs()
                .max_element()
                < 3e-3
        );
    }

    #[test]
    fn every_proving_ground_crate_fits() {
        // The pile's random crates span these ranges; the others are cubes.
        let (mut worst, mut leaner) = (0, 0);
        for x in [0.22, 0.25, 0.3, 0.35, 0.4, 0.45] {
            for y in [0.2, 0.25, 0.3, 0.35, 0.45] {
                for z in [0.22, 0.25, 0.3, 0.35, 0.4, 0.45] {
                    let half = Vec3::new(x, y, z);
                    let seed = (x * 100.0 + y * 10.0 + z) as u32;
                    let c = crate_box(half, seed);
                    let n = tris(&c.wood) + tris(&c.metal);
                    worst = worst.max(n);
                    let full = build_crate(half, seed, 0);
                    if tris(&full.wood) + tris(&full.metal) > CRATE_TRIANGLES {
                        leaner += 1;
                    }
                    assert!(n <= CRATE_TRIANGLES, "{half}: {n} triangles");
                    let mut both = c.wood.clone();
                    both.merge(&c.metal);
                    let bb = both.aabb();
                    assert!((bb.max - half).abs().max_element() < 1e-3, "{half}: {bb:?}");
                }
            }
        }
        println!("worst crate: {worst} triangles; {leaner} of 180 shed detail");
    }

    #[test]
    fn chain_segments_interlock() {
        let (n, pitch) = chain_layout(CHAIN_SEGMENT_LENGTH, CHAIN_WIRE);
        assert_eq!(n % 2, 0);
        let m = chain_segment(CHAIN_SEGMENT_LENGTH, CHAIN_WIRE);
        let bb = m.aabb();
        // The end links' wire reaches one wire diameter past the joints, into the next links.
        assert!((bb.max.y - (0.4 + 2.0 * CHAIN_WIRE)).abs() < 2e-3, "{bb:?}");
        for x in [bb.max.x, bb.max.z, -bb.min.x, -bb.min.z] {
            assert!((x - 0.07).abs() < 1e-3, "{bb:?}");
        }
        assert!(pitch > 4.0 * CHAIN_WIRE);
    }
}
