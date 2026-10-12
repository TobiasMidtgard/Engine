//! Geometry of the station's props, built from the [`meshgen`](super::meshgen) blocks in each
//! prop's local frame (meters, Y up, the prop's front facing +Z unless noted). Props with
//! several materials return one mesh per material.

use std::f32::consts::{FRAC_PI_2, PI, TAU};

use engine_core::math::{Mat4, Quat, Vec2, Vec3, Vec4};
use engine_render::Mesh;

use super::meshgen::{
    Builder, at, bezier, circle, extrude, lathe, rod, rounded_box, rounded_rect, sweep, transform,
    trs, tube,
};
use super::noise::Rng;

/// Rounded box with soft edges.
pub fn rb(size: [f32; 3], r: f32) -> Mesh {
    rounded_box(Vec3::from(size), r, 1, 0)
}

fn put(b: &mut Builder, m: &Mesh, pos: Vec3) {
    b.add(m, Mat4::from_translation(pos));
}

fn put_r(b: &mut Builder, m: &Mesh, pos: Vec3, rot: Quat) {
    b.add(m, Mat4::from_rotation_translation(rot, pos));
}

/// Shift texture coordinates (so repeated parts don't show the same patch of texture).
pub fn offset_uv(m: &mut Mesh, du: f32, dv: f32) {
    for v in &mut m.vertices {
        v.uv[0] += du;
        v.uv[1] += dv;
    }
}

/// Turn a box's planar UVs a quarter so wood grain runs up a vertical member.
pub fn grain_vertical(m: &mut Mesh) {
    for v in &mut m.vertices {
        v.uv = [v.uv[1], v.uv[0]];
    }
}

/// Hexagon nut (or bolt head) of across-corners radius `r`, height `h`, sitting on y = 0.
pub fn hex_nut(r: f32, h: f32) -> Mesh {
    lathe(
        &[(0.0, 0.0), (r, 0.0), (r, h * 0.85), (r * 0.85, h), (0.0, h)],
        6,
        30.0,
        true,
    )
}

/// A quad facing `right × up`, `w × h`, texture covering it once (u right, v down).
pub fn quad(center: Vec3, right: Vec3, up: Vec3, w: f32, h: f32) -> Mesh {
    let mut b = Builder::new();
    let n = right.cross(up).normalize();
    let (r, u) = (right * w * 0.5, up * h * 0.5);
    let a = b.vertex(center - r - u, n, Vec2::new(0.0, 1.0));
    let c = b.vertex(center + r - u, n, Vec2::new(1.0, 1.0));
    let d = b.vertex(center + r + u, n, Vec2::new(1.0, 0.0));
    let e = b.vertex(center - r + u, n, Vec2::new(0.0, 0.0));
    b.tri_facing(a, c, d, n);
    b.tri_facing(a, d, e, n);
    b.finish()
}

/// Loft rounded-rectangle sections along +X: `(x, half width in z, bottom y, top y, corner
/// radius)`. Smooth sides, flat end caps.
pub fn loft_x(sections: &[(f32, f32, f32, f32, f32)], corner_segs: u32) -> Mesh {
    let mut b = Builder::new();
    let rings: Vec<Vec<Vec3>> = sections
        .iter()
        .map(|&(x, hw, yb, yt, r)| {
            let yc = (yb + yt) * 0.5;
            rounded_rect(hw * 2.0, yt - yb, r, corner_segs)
                .into_iter()
                .map(|p| Vec3::new(x, yc + p.y, p.x))
                .collect()
        })
        .collect();
    let n = rings[0].len() as u32;
    let mut base = Vec::new();
    for ring in &rings {
        base.push(b.mesh.vertices.len() as u32);
        let mut v = 0.0;
        for (k, p) in ring.iter().enumerate() {
            if k > 0 {
                v += p.distance(ring[k - 1]);
            }
            b.vertex(*p, Vec3::Y, Vec2::new(p.x, v));
        }
    }
    for s in 0..rings.len() - 1 {
        let c0 = Vec3::new(sections[s].0, (sections[s].2 + sections[s].3) * 0.5, 0.0);
        for k in 0..n {
            let (a, bb) = (base[s] + k, base[s] + (k + 1) % n);
            let (c, d) = (base[s + 1] + (k + 1) % n, base[s + 1] + k);
            let out = (rings[s][k as usize] - c0).with_x(0.0);
            b.tri_facing(a, bb, c, out);
            b.tri_facing(a, c, d, out);
        }
    }
    b.mesh.compute_normals();
    for (s, sign) in [(0usize, -1.0f32), (rings.len() - 1, 1.0)] {
        let nrm = Vec3::X * sign;
        let ring = &rings[s];
        let c = ring.iter().copied().sum::<Vec3>() / ring.len() as f32;
        let center = b.vertex(c, nrm, Vec2::new(c.z, c.y));
        let first = b.mesh.vertices.len() as u32;
        for p in ring {
            b.vertex(*p, nrm, Vec2::new(p.z, -p.y));
        }
        for k in 0..n {
            b.tri_facing(center, first + k, first + (k + 1) % n, nrm);
        }
    }
    b.finish()
}

// ------------------------------------------------------------------------------- ground

/// Kerb stones around a rectangle `[x0, x1] × [z0, z1]` (outside it), `width` wide, `top` m
/// above the ground. Individual stones with joints, small height and angle errors.
pub fn kerbs(x0: f32, x1: f32, z0: f32, z1: f32, width: f32, top: f32, seed: u32) -> Mesh {
    let mut b = Builder::new();
    let mut rng = Rng::new(seed);
    let depth = top + 0.04;
    let run = |b: &mut Builder, from: Vec2, to: Vec2, rng: &mut Rng| {
        let len = from.distance(to);
        let count = (len / 1.0).round().max(1.0) as u32;
        let step = len / count as f32;
        let dir = (to - from) / len;
        let yaw = -dir.y.atan2(dir.x);
        for k in 0..count {
            let c = from + dir * (step * (k as f32 + 0.5));
            let mut stone = rounded_box(Vec3::new(step - 0.006, depth, width), 0.018, 2, 1 << 3);
            offset_uv(&mut stone, rng.next() * 3.0, rng.next() * 3.0);
            let tint = 0.9 + 0.14 * rng.next();
            let rot = Quat::from_rotation_y(yaw + rng.signed() * 0.004)
                * Quat::from_rotation_z(rng.signed() * 0.004);
            b.add_tinted(
                &stone,
                trs(
                    Vec3::new(c.x, top - depth * 0.5 + rng.signed() * 0.003, c.y),
                    rot,
                    Vec3::ONE,
                ),
                Vec4::new(tint, tint, tint * 0.99, 1.0),
            );
        }
    };
    let h = width * 0.5;
    run(
        &mut b,
        Vec2::new(x0 - width, z0 - h),
        Vec2::new(x1 + width, z0 - h),
        &mut rng,
    );
    run(
        &mut b,
        Vec2::new(x0 - width, z1 + h),
        Vec2::new(x1 + width, z1 + h),
        &mut rng,
    );
    run(
        &mut b,
        Vec2::new(x0 - h, z0),
        Vec2::new(x0 - h, z1),
        &mut rng,
    );
    run(
        &mut b,
        Vec2::new(x1 + h, z0),
        Vec2::new(x1 + h, z1),
        &mut rng,
    );
    b.finish()
}

/// Random-pattern stone paving over `[x0, x1] × [z0, z1]` with tops at `top`: rows 0.6 m
/// deep of slabs 0.3–0.9 m long, 10 mm joints, bevelled edges, color and texture varying per
/// slab, a little lipping between slabs.
pub fn pavers(x0: f32, x1: f32, z0: f32, z1: f32, top: f32, seed: u32) -> Mesh {
    let mut b = Builder::new();
    let mut rng = Rng::new(seed);
    let joint = 0.01;
    let rows = ((z1 - z0) / 0.6).round().max(1.0) as u32;
    let row_d = (z1 - z0) / rows as f32;
    let tints = [
        Vec4::new(1.0, 0.97, 0.92, 1.0),
        Vec4::new(0.9, 0.9, 0.9, 1.0),
        Vec4::new(1.02, 0.94, 0.9, 1.0),
        Vec4::new(0.84, 0.82, 0.78, 1.0),
        Vec4::new(1.05, 1.02, 0.96, 1.0),
    ];
    for r in 0..rows {
        let zc = z0 + row_d * (r as f32 + 0.5);
        let mut x = x0;
        while x < x1 - 0.01 {
            let mut len = [0.3, 0.6, 0.6, 0.9][(rng.next() * 4.0) as usize % 4];
            if x + len > x1 - 0.25 {
                len = x1 - x;
            }
            let thick = 0.05;
            let mut slab = rounded_box(
                Vec3::new(len - joint, thick, row_d - joint),
                0.007,
                1,
                1 << 3,
            );
            // Random texture patch and orientation for every slab.
            let (du, dv) = (rng.next() * 7.0, rng.next() * 7.0);
            let turn = rng.next() < 0.5;
            for v in &mut slab.vertices {
                let uv = if turn { [v.uv[1], v.uv[0]] } else { v.uv };
                v.uv = [uv[0] + du, uv[1] + dv];
            }
            let t = tints[(rng.next() * tints.len() as f32) as usize % tints.len()];
            let k = 0.94 + 0.1 * rng.next();
            let rot = Quat::from_rotation_x(rng.signed() * 0.003)
                * Quat::from_rotation_z(rng.signed() * 0.003)
                * Quat::from_rotation_y(rng.signed() * 0.004);
            b.add_tinted(
                &slab,
                trs(
                    Vec3::new(x + len * 0.5, top - thick * 0.5 + rng.signed() * 0.0015, zc),
                    rot,
                    Vec3::ONE,
                ),
                t * Vec4::new(k, k, k, 1.0),
            );
            x += len;
        }
    }
    b.finish()
}

/// Granite setts (small square cobbles) in a single row around `[x0, x1] × [z0, z1]`.
pub fn setts(x0: f32, x1: f32, z0: f32, z1: f32, size: f32, top: f32, seed: u32) -> Mesh {
    let mut b = Builder::new();
    let mut rng = Rng::new(seed);
    let base = rounded_box(
        Vec3::new(size - 0.012, 0.09, size - 0.012),
        0.014,
        1,
        1 << 3,
    );
    let row = |b: &mut Builder, from: Vec2, to: Vec2, rng: &mut Rng| {
        let len = from.distance(to);
        let count = (len / size).round().max(1.0) as u32;
        let step = len / count as f32;
        let dir = (to - from) / len;
        for k in 0..count {
            let c = from + dir * (step * (k as f32 + 0.5));
            let mut s = base.clone();
            offset_uv(&mut s, rng.next() * 5.0, rng.next() * 5.0);
            let k = 0.8 + 0.3 * rng.next();
            b.add_tinted(
                &s,
                trs(
                    Vec3::new(c.x, top - 0.045 + rng.signed() * 0.004, c.y),
                    Quat::from_rotation_y(rng.signed() * 0.08)
                        * Quat::from_rotation_x(rng.signed() * 0.02),
                    Vec3::new(step / size, 1.0, 1.0),
                ),
                Vec4::new(k, k, k * 1.02, 1.0),
            );
        }
    };
    let h = size * 0.5;
    row(
        &mut b,
        Vec2::new(x0 - size, z0 - h),
        Vec2::new(x1 + size, z0 - h),
        &mut rng,
    );
    row(
        &mut b,
        Vec2::new(x0 - size, z1 + h),
        Vec2::new(x1 + size, z1 + h),
        &mut rng,
    );
    row(
        &mut b,
        Vec2::new(x0 - h, z0),
        Vec2::new(x0 - h, z1),
        &mut rng,
    );
    row(
        &mut b,
        Vec2::new(x1 + h, z0),
        Vec2::new(x1 + h, z1),
        &mut rng,
    );
    b.finish()
}

/// Timber sleepers edging a sand pit `[x0, x1] × [z0, z1]` (outside it).
pub fn sleepers(x0: f32, x1: f32, z0: f32, z1: f32, w: f32, top: f32, seed: u32) -> Mesh {
    let mut b = Builder::new();
    let mut rng = Rng::new(seed);
    let hgt = top + 0.03;
    let piece = |b: &mut Builder, c: Vec3, len: f32, along_x: bool, rng: &mut Rng| {
        let mut m = rounded_box(Vec3::new(len - 0.008, hgt, w), 0.016, 2, 1 << 3);
        offset_uv(&mut m, rng.next() * 4.0, rng.next() * 4.0);
        let rot = if along_x {
            Quat::from_rotation_y(rng.signed() * 0.006)
        } else {
            Quat::from_rotation_y(FRAC_PI_2 + rng.signed() * 0.006)
        };
        let k = 0.85 + 0.2 * rng.next();
        b.add_tinted(
            &m,
            trs(
                c + Vec3::Y * (top - hgt * 0.5 + rng.signed() * 0.006),
                rot,
                Vec3::ONE,
            ),
            Vec4::new(k, k, k, 1.0),
        );
    };
    let lx = x1 - x0 + 2.0 * w;
    for (z, sign) in [(z0 - w * 0.5, 0.0), (z1 + w * 0.5, 1.0)] {
        let half = lx * (0.5 + 0.06 * (sign - 0.5));
        piece(
            &mut b,
            Vec3::new(x0 - w + half * 0.5, 0.0, z),
            half,
            true,
            &mut rng,
        );
        piece(
            &mut b,
            Vec3::new(x0 - w + half + (lx - half) * 0.5, 0.0, z),
            lx - half,
            true,
            &mut rng,
        );
    }
    let lz = z1 - z0;
    for x in [x0 - w * 0.5, x1 + w * 0.5] {
        piece(
            &mut b,
            Vec3::new(x, 0.0, z0 + lz * 0.25),
            lz * 0.5,
            false,
            &mut rng,
        );
        piece(
            &mut b,
            Vec3::new(x, 0.0, z0 + lz * 0.75),
            lz * 0.5,
            false,
            &mut rng,
        );
    }
    b.finish()
}

/// A ribbon of crack sealant following `points` (x, z) on a surface of height `y(x, z)`.
pub fn sealant(points: &[Vec2], width: f32, y: impl Fn(f32, f32) -> f32, seed: u32) -> Mesh {
    let mut b = Builder::new();
    let mut rng = Rng::new(seed);
    let mut v = 0.0;
    let mut prev: Option<(u32, u32)> = None;
    for (i, p) in points.iter().enumerate() {
        let dir = if i + 1 < points.len() {
            points[i + 1] - *p
        } else {
            *p - points[i - 1]
        }
        .normalize_or(Vec2::X);
        let side = Vec2::new(-dir.y, dir.x);
        let w = width * (0.6 + 0.8 * rng.next());
        if i > 0 {
            v += p.distance(points[i - 1]);
        }
        let l = *p + side * w * 0.5;
        let r = *p - side * w * 0.5;
        let a = b.vertex(Vec3::new(l.x, y(l.x, l.y), l.y), Vec3::Y, Vec2::new(0.0, v));
        let c = b.vertex(Vec3::new(r.x, y(r.x, r.y), r.y), Vec3::Y, Vec2::new(w, v));
        if let Some((pa, pc)) = prev {
            b.tri_facing(pa, pc, c, Vec3::Y);
            b.tri_facing(pa, c, a, Vec3::Y);
        }
        prev = Some((a, c));
    }
    b.finish()
}

/// A wandering crack path from `from` heading `dir`, `steps` segments of ~`step` m.
pub fn crack_path(from: Vec2, dir: f32, steps: u32, step: f32, seed: u32) -> Vec<Vec2> {
    let mut rng = Rng::new(seed);
    let mut p = from;
    let mut a = dir;
    let mut out = vec![p];
    for _ in 0..steps {
        a += rng.signed() * 0.5;
        a = dir + (a - dir).clamp(-0.7, 0.7);
        p += Vec2::new(a.cos(), a.sin()) * step * (0.6 + 0.8 * rng.next());
        out.push(p);
    }
    out
}

/// Round manhole cover (radius `r`) in its frame, top at y = 0. Returns (cover, frame).
pub fn manhole(r: f32) -> (Mesh, Mesh) {
    let mut cover = lathe(
        &[
            (r - 0.004, -0.02),
            (r - 0.004, -0.002),
            (r - 0.008, 0.0),
            (0.0, 0.0),
        ],
        48,
        30.0,
        false,
    );
    // Top face texture covers the disc once: u, v from x, z.
    for v in &mut cover.vertices {
        if v.normal[1] > 0.9 {
            v.uv = [
                v.position[0] / (2.0 * r) + 0.5,
                v.position[2] / (2.0 * r) + 0.5,
            ];
        }
    }
    let mut ring = lathe(
        &[
            (r + 0.06, -0.02),
            (r + 0.06, -0.002),
            (r + 0.055, 0.0015),
            (r + 0.004, 0.0015),
            (r + 0.002, -0.01),
        ],
        48,
        30.0,
        false,
    );
    // The frame samples a plain corner of the cover texture (outside the pattern).
    for v in &mut ring.vertices {
        v.uv = [
            0.03 + v.position[0].abs() * 0.05,
            0.03 + v.position[2].abs() * 0.05,
        ];
    }
    (cover, ring)
}

/// Gully grate: frame and bars (iron) and the dark gully below (second mesh), top at y = 0.
pub fn gully(w: f32, d: f32) -> (Mesh, Mesh) {
    let mut iron = Builder::new();
    let t = 0.04;
    for (c, s) in [
        (Vec3::new(0.0, -0.01, -d * 0.5 + t * 0.5), [w, 0.024, t]),
        (Vec3::new(0.0, -0.01, d * 0.5 - t * 0.5), [w, 0.024, t]),
        (
            Vec3::new(-w * 0.5 + t * 0.5, -0.01, 0.0),
            [t, 0.024, d - 2.0 * t],
        ),
        (
            Vec3::new(w * 0.5 - t * 0.5, -0.01, 0.0),
            [t, 0.024, d - 2.0 * t],
        ),
    ] {
        put(&mut iron, &rb(s, 0.004), c);
    }
    let bars = 9;
    let inner = w - 2.0 * t;
    for k in 0..bars {
        let x = -inner * 0.5 + inner * (k as f32 + 0.5) / bars as f32;
        put(
            &mut iron,
            &rb([0.016, 0.022, d - 2.0 * t], 0.003),
            Vec3::new(x, -0.011, 0.0),
        );
    }
    put(
        &mut iron,
        &rb([inner, 0.02, 0.02], 0.003),
        Vec3::new(0.0, -0.013, 0.0),
    );
    let hole = rounded_box(Vec3::new(w - 0.02, 0.004, d - 0.02), 0.0, 1, 0);
    let mut dark = Builder::new();
    put(&mut dark, &hole, Vec3::new(0.0, -0.019, 0.0));
    (iron.finish(), dark.finish())
}

// ------------------------------------------------------------------------------- shelter

/// Meshes of the bus-stop shelter, by material.
pub struct Shelter {
    /// Anthracite powder-coated steel: posts, roof frame, rails, brackets, bin.
    pub steel: Mesh,
    /// Galvanised: roof deck and seams, gutter, downpipe, fixings.
    pub galv: Mesh,
    /// Toughened glass panels (alpha blended).
    pub glass: Mesh,
    /// Hardwood bench slats.
    pub wood: Mesh,
    /// White ceiling (soffit).
    pub soffit: Mesh,
    /// LED strip in the ceiling (emissive at night).
    pub led: Mesh,
    /// Backlit poster faces of the advertising case (texture covers each face).
    pub poster: Mesh,
    /// Name plate on the front fascia (texture covers it).
    pub plate: Mesh,
    /// Black rubber and plastic: bin opening, glazing gaskets.
    pub black: Mesh,
}

/// Height of the shelter roof frame's underside at `z` (it slopes down to the back gutter).
pub fn shelter_roof_y(z: f32) -> f32 {
    2.42 + (z + 0.68) * (0.1 / 1.3)
}

/// Bus-stop shelter, 4.4 m long and 1.5 m deep, open to +Z, centered on its floor.
pub fn shelter(seed: u32) -> Shelter {
    let mut rng = Rng::new(seed);
    let mut steel = Builder::new();
    let mut galv = Builder::new();
    let mut glass = Builder::new();
    let mut wood = Builder::new();
    let mut soffit = Builder::new();
    let mut led = Builder::new();
    let mut black = Builder::new();
    let back_z = -0.68;
    let front_z = 0.62;
    let back_x = [-2.1f32, -0.7, 0.7, 2.1];
    let nut = hex_nut(0.014, 0.012);
    let plate = rb([0.18, 0.012, 0.18], 0.003);
    // Posts with base plates and anchor nuts.
    for (x, z) in back_x
        .iter()
        .map(|&x| (x, back_z))
        .chain([(-2.1, front_z), (2.1, front_z)])
    {
        let h = shelter_roof_y(z) + 0.1;
        put(
            &mut steel,
            &rb([0.08, h, 0.08], 0.008),
            Vec3::new(x, h * 0.5, z),
        );
        put(&mut steel, &plate, Vec3::new(x, 0.006, z));
        for (dx, dz) in [(-0.06, -0.06), (0.06, -0.06), (-0.06, 0.06), (0.06, 0.06)] {
            put(&mut galv, &nut, Vec3::new(x + dx, 0.012, z + dz));
        }
    }
    // Roof: frame, deck with standing seams, soffit and LED strip, in a frame that slopes.
    let alpha = (0.1f32 / 1.3).atan();
    let roof = Mat4::from_rotation_translation(
        Quat::from_rotation_x(-alpha),
        Vec3::new(0.0, shelter_roof_y(0.0), 0.0),
    );
    let mut r_steel = Builder::new();
    let mut r_galv = Builder::new();
    let (rx, rz0, rz1) = (2.32f32, -0.98f32, 1.0f32);
    let fh = 0.16;
    put(
        &mut r_steel,
        &rb([rx * 2.0, fh, 0.05], 0.01),
        Vec3::new(0.0, fh * 0.5, rz1 - 0.025),
    );
    put(
        &mut r_steel,
        &rb([rx * 2.0, fh, 0.05], 0.01),
        Vec3::new(0.0, fh * 0.5, rz0 + 0.025),
    );
    for x in [-rx + 0.025, rx - 0.025] {
        put(
            &mut r_steel,
            &rb([0.05, fh, rz1 - rz0 - 0.1], 0.01),
            Vec3::new(x, fh * 0.5, (rz0 + rz1) * 0.5),
        );
    }
    // Cross beams under the deck over each post line.
    for x in [-0.7f32, 0.7] {
        put(
            &mut r_steel,
            &rb([0.06, 0.1, rz1 - rz0 - 0.1], 0.006),
            Vec3::new(x, 0.08, (rz0 + rz1) * 0.5),
        );
    }
    let deck_w = rx * 2.0 - 0.1;
    let deck_d = rz1 - rz0 - 0.1;
    put(
        &mut r_galv,
        &rb([deck_w, 0.02, deck_d], 0.004),
        Vec3::new(0.0, 0.135, 0.01),
    );
    let mut x = -deck_w * 0.5 + 0.15;
    while x < deck_w * 0.5 - 0.1 {
        put(
            &mut r_galv,
            &rb([0.022, 0.032, deck_d], 0.008),
            Vec3::new(x, 0.155, 0.01),
        );
        x += 0.3;
    }
    let mut r_soffit = Builder::new();
    put(
        &mut r_soffit,
        &rb([deck_w, 0.012, deck_d], 0.003),
        Vec3::new(0.0, 0.035, 0.01),
    );
    let mut r_led = Builder::new();
    put(
        &mut r_led,
        &rb([3.4, 0.01, 0.045], 0.004),
        Vec3::new(0.0, 0.026, -0.12),
    );
    steel.add(&r_steel.finish(), roof);
    galv.add(&r_galv.finish(), roof);
    soffit.add(&r_soffit.finish(), roof);
    led.add(&r_led.finish(), roof);
    // Name plate on the front fascia.
    let plate_mesh = {
        let q = quad(
            Vec3::new(0.0, 0.08, rz1 + 0.003),
            Vec3::X,
            Vec3::Y,
            1.9,
            0.12,
        );
        let mut m = q;
        transform(&mut m, roof);
        m
    };
    // Half-round gutter along the back with end caps, outlet and downpipe.
    let gz = -1.04f32;
    let gy = shelter_roof_y(-0.98) - 0.04;
    let mut gutter_profile = Vec::new();
    let (ro, ri) = (0.065f32, 0.06f32);
    for k in 0..=12 {
        let a = k as f32 / 12.0 * PI;
        gutter_profile.push(Vec2::new(-ro * a.sin(), -ro * a.cos()));
    }
    for k in (0..=12).rev() {
        let a = k as f32 / 12.0 * PI;
        gutter_profile.push(Vec2::new(-ri * a.sin(), -ri * a.cos()));
    }
    galv.add(
        &sweep(
            &gutter_profile,
            true,
            &[Vec3::new(-rx, gy, gz), Vec3::new(rx, gy, gz)],
            &[],
            Vec3::Y,
            40.0,
            false,
        ),
        Mat4::IDENTITY,
    );
    for x in [-rx, rx] {
        let cap = lathe(
            &[(0.0, -0.003), (ro, -0.003), (ro, 0.003), (0.0, 0.003)],
            16,
            30.0,
            false,
        );
        galv.add(
            &cap,
            trs(
                Vec3::new(x, gy, gz),
                Quat::from_rotation_z(FRAC_PI_2),
                Vec3::new(1.0, 1.0, 1.0),
            ),
        );
    }
    let px = 2.1;
    let pz = back_z - 0.04 - 0.045;
    let mut pipe = vec![Vec3::new(px, gy - 0.02, gz), Vec3::new(px, gy - 0.12, gz)];
    pipe.extend(bezier(
        Vec3::new(px, gy - 0.12, gz),
        Vec3::new(px, gy - 0.25, gz),
        Vec3::new(px, gy - 0.3, pz),
        Vec3::new(px, gy - 0.45, pz),
        8,
    ));
    pipe.push(Vec3::new(px, 0.3, pz));
    pipe.extend(bezier(
        Vec3::new(px, 0.3, pz),
        Vec3::new(px, 0.12, pz),
        Vec3::new(px, 0.09, pz - 0.05),
        Vec3::new(px, 0.08, pz - 0.18),
        8,
    ));
    pipe.dedup_by(|a, b| a.distance_squared(*b) < 1e-8);
    galv.add(&tube(&pipe, 0.034, 16, true), Mat4::IDENTITY);
    let clip = lathe(
        &[(0.037, 0.0), (0.04, 0.004), (0.04, 0.026), (0.037, 0.03)],
        16,
        40.0,
        false,
    );
    for y in [0.6, 1.4, 2.0] {
        put(&mut galv, &clip, Vec3::new(px, y, pz));
        put(
            &mut galv,
            &rb([0.02, 0.025, 0.05], 0.004),
            Vec3::new(px, y + 0.015, pz + 0.04),
        );
    }
    // Glass: back wall (three bays) and the left side, in rails with clamps.
    let gh = 2.18;
    let gy0 = 0.12;
    let panel = |glass: &mut Builder, steel: &mut Builder, c: Vec3, w: f32, along_x: bool| {
        let (size, rail) = if along_x {
            ([w, gh, 0.01], [w + 0.02, 0.045, 0.036])
        } else {
            ([0.01, gh, w], [0.036, 0.045, w + 0.02])
        };
        put(glass, &rounded_box(Vec3::from(size), 0.003, 1, 0), c);
        put(steel, &rb(rail, 0.006), Vec3::new(c.x, gy0 - 0.02, c.z));
        put(
            steel,
            &rb(rail, 0.006),
            Vec3::new(c.x, gy0 + gh + 0.02, c.z),
        );
    };
    for i in 0..3 {
        let x = (back_x[i] + back_x[i + 1]) * 0.5;
        panel(
            &mut glass,
            &mut steel,
            Vec3::new(x, gy0 + gh * 0.5, back_z),
            1.29,
            true,
        );
    }
    panel(
        &mut glass,
        &mut steel,
        Vec3::new(-2.1, gy0 + gh * 0.5, -0.03),
        1.2,
        false,
    );
    // Advertising case on the right side: frame and two backlit poster faces.
    let case_h = 2.06;
    put(
        &mut steel,
        &rounded_box(Vec3::new(0.11, case_h, 1.24), 0.02, 2, 0),
        Vec3::new(2.1, 0.1 + case_h * 0.5, -0.03),
    );
    let mut poster = Builder::new();
    let (pw, ph) = (1.06f32, 1.78f32);
    let pc = Vec3::new(2.1, 0.1 + case_h * 0.5 + 0.02, -0.03);
    poster.add(
        &quad(pc + Vec3::X * 0.0565, Vec3::NEG_Z, Vec3::Y, pw, ph),
        Mat4::IDENTITY,
    );
    poster.add(
        &quad(pc - Vec3::X * 0.0565, Vec3::Z, Vec3::Y, pw, ph),
        Mat4::IDENTITY,
    );
    for s in [-1.0f32, 1.0] {
        // Gasket around the poster window.
        for (c, size) in [
            (
                Vec3::new(0.0, ph * 0.5 + 0.01, 0.0),
                [0.006, 0.02, pw + 0.04],
            ),
            (
                Vec3::new(0.0, -ph * 0.5 - 0.01, 0.0),
                [0.006, 0.02, pw + 0.04],
            ),
            (Vec3::new(0.0, 0.0, pw * 0.5 + 0.01), [0.006, ph, 0.02]),
            (Vec3::new(0.0, 0.0, -pw * 0.5 - 0.01), [0.006, ph, 0.02]),
        ] {
            put(&mut black, &rb(size, 0.002), pc + Vec3::X * 0.056 * s + c);
        }
    }
    // Bench: brackets on the back posts, seat and back slats.
    let brk_x = [-2.02f32, -0.7, 0.7, 2.02];
    for &x in &brk_x {
        let z0 = back_z + 0.045;
        put(
            &mut steel,
            &rb([0.06, 0.78, 0.012], 0.003),
            Vec3::new(x, 0.52, z0),
        );
        put(
            &mut steel,
            &rb([0.014, 0.05, 0.38], 0.004),
            Vec3::new(x, 0.425, z0 + 0.19),
        );
        steel.add(
            &rod(
                Vec3::new(x, 0.18, z0 + 0.01),
                Vec3::new(x, 0.41, z0 + 0.3),
                0.009,
                8,
            ),
            Mat4::IDENTITY,
        );
        put(
            &mut steel,
            &rb([0.014, 0.34, 0.04], 0.004),
            Vec3::new(x, 0.74, z0 + 0.035),
        );
    }
    let slat_len = 4.12;
    for k in 0..4 {
        let mut s = rb([slat_len, 0.032, 0.078], 0.009);
        offset_uv(&mut s, rng.next() * 3.0, rng.next() * 3.0);
        put(
            &mut wood,
            &s,
            Vec3::new(0.0, 0.466, back_z + 0.1 + k as f32 * 0.09),
        );
    }
    for (k, y) in [0.68f32, 0.8].iter().enumerate() {
        let mut s = rb([slat_len, 0.078, 0.03], 0.009);
        offset_uv(&mut s, rng.next() * 3.0 + k as f32, rng.next() * 3.0);
        put_r(
            &mut wood,
            &s,
            Vec3::new(0.0, *y, back_z + 0.09 + (y - 0.68) * 0.15),
            Quat::from_rotation_x(-0.15),
        );
    }
    // Litter bin on the front left post.
    let bin = lathe(
        &[
            (0.0, 0.0),
            (0.15, 0.0),
            (0.16, 0.015),
            (0.16, 0.47),
            (0.168, 0.48),
            (0.168, 0.5),
            (0.16, 0.51),
            (0.15, 0.56),
            (0.1, 0.6),
            (0.0, 0.615),
        ],
        28,
        35.0,
        false,
    );
    let bin_c = Vec3::new(-2.1, 0.32, front_z + 0.04 + 0.175);
    put(&mut steel, &bin, bin_c);
    put(
        &mut steel,
        &rb([0.1, 0.32, 0.06], 0.01),
        Vec3::new(-2.1, 0.62, front_z + 0.07),
    );
    put_r(
        &mut black,
        &rb([0.2, 0.07, 0.04], 0.015),
        bin_c + Vec3::new(0.0, 0.52, 0.12),
        Quat::from_rotation_x(-0.5),
    );
    Shelter {
        steel: steel.finish(),
        galv: galv.finish(),
        glass: glass.finish(),
        wood: wood.finish(),
        soffit: soffit.finish(),
        led: led.finish(),
        poster: poster.finish(),
        plate: plate_mesh,
        black: black.finish(),
    }
}

// ------------------------------------------------------------------------------- lamp

/// Street lamp meshes by material. The head is at the end of the arm along +X.
pub struct Lamp {
    pub steel: Mesh,
    pub galv: Mesh,
    pub concrete: Mesh,
    pub lens: Mesh,
    /// Where the light sits, in the lamp's frame.
    pub light: Vec3,
}

/// A 5.8 m street lamp: concrete collar, base plate with anchor nuts, cast base with a door,
/// tapered pole curving into an arm, LED luminaire with a flat lens.
pub fn street_lamp() -> Lamp {
    let mut steel = Builder::new();
    let mut galv = Builder::new();
    let mut concrete = Builder::new();
    concrete.add(
        &lathe(
            &[
                (0.0, -0.03),
                (0.24, -0.03),
                (0.24, 0.02),
                (0.22, 0.04),
                (0.0, 0.04),
            ],
            32,
            30.0,
            false,
        ),
        Mat4::IDENTITY,
    );
    put(
        &mut galv,
        &rb([0.3, 0.02, 0.3], 0.004),
        Vec3::new(0.0, 0.05, 0.0),
    );
    let nut = hex_nut(0.019, 0.016);
    let washer = lathe(
        &[(0.0, 0.0), (0.024, 0.0), (0.024, 0.004), (0.0, 0.004)],
        16,
        30.0,
        false,
    );
    let stud = lathe(
        &[(0.0, 0.0), (0.009, 0.0), (0.009, 0.05), (0.0, 0.052)],
        10,
        30.0,
        false,
    );
    for (x, z) in [(-0.11, -0.11), (0.11, -0.11), (-0.11, 0.11), (0.11, 0.11)] {
        put(&mut galv, &washer, Vec3::new(x, 0.06, z));
        put(&mut galv, &nut, Vec3::new(x, 0.064, z));
        put(&mut galv, &stud, Vec3::new(x, 0.06, z));
    }
    let top = 5.25;
    steel.add(
        &lathe(
            &[
                (0.0, 0.06),
                (0.12, 0.06),
                (0.12, 0.08),
                (0.105, 0.13),
                (0.092, 0.3),
                (0.088, 0.78),
                (0.096, 0.8),
                (0.096, 0.83),
                (0.078, 0.86),
                (0.072, 1.0),
                (0.046, top),
            ],
            28,
            35.0,
            false,
        ),
        Mat4::IDENTITY,
    );
    // Access door with two screws.
    put(
        &mut steel,
        &rb([0.075, 0.28, 0.012], 0.004),
        Vec3::new(0.0, 0.47, 0.087),
    );
    for y in [0.36, 0.58] {
        put_r(
            &mut galv,
            &lathe(
                &[(0.0, 0.0), (0.008, 0.0), (0.007, 0.003), (0.0, 0.004)],
                10,
                30.0,
                false,
            ),
            Vec3::new(0.0, y, 0.093),
            Quat::from_rotation_x(FRAC_PI_2),
        );
    }
    // Swan-neck arm.
    let arm = bezier(
        Vec3::new(0.0, top - 0.01, 0.0),
        Vec3::new(0.0, top + 0.45, 0.0),
        Vec3::new(0.15, top + 0.62, 0.0),
        Vec3::new(0.66, top + 0.64, 0.0),
        16,
    );
    let scale: Vec<f32> = (0..arm.len())
        .map(|i| 1.0 - 0.3 * i as f32 / (arm.len() - 1) as f32)
        .collect();
    steel.add(
        &sweep(&circle(0.046, 20), true, &arm, &scale, Vec3::Z, 70.0, false),
        Mat4::IDENTITY,
    );
    // Luminaire: lofted housing tilted up 5°, cooling fins on top, flat lens below.
    let head_at = Vec3::new(0.6, top + 0.64, 0.0);
    let tilt = Quat::from_rotation_z(0.09);
    let head = loft_x(
        &[
            (0.0, 0.05, -0.035, 0.035, 0.03),
            (0.07, 0.12, -0.048, 0.05, 0.04),
            (0.3, 0.155, -0.05, 0.05, 0.045),
            (0.52, 0.145, -0.046, 0.032, 0.035),
            (0.64, 0.105, -0.04, 0.012, 0.03),
            (0.65, 0.095, -0.036, 0.008, 0.026),
        ],
        5,
    );
    let place = Mat4::from_rotation_translation(tilt, head_at);
    steel.add(&head, place);
    for k in 0..5 {
        let z = -0.08 + k as f32 * 0.04;
        steel.add(
            &rb([0.3, 0.018, 0.006], 0.003),
            place * Mat4::from_translation(Vec3::new(0.3, 0.055, z)),
        );
    }
    let mut lens = Builder::new();
    lens.add(
        &rounded_box(Vec3::new(0.4, 0.008, 0.22), 0.004, 1, 0),
        place * Mat4::from_translation(Vec3::new(0.33, -0.051, 0.0)),
    );
    let light = place.transform_point3(Vec3::new(0.33, -0.12, 0.0));
    Lamp {
        steel: steel.finish(),
        galv: galv.finish(),
        concrete: concrete.finish(),
        lens: lens.finish(),
        light,
    }
}

// ------------------------------------------------------------------------------- bollard

/// Steel bollard (0.9 m) with two retro-reflective bands: (body, bands).
pub fn bollard() -> (Mesh, Mesh) {
    let body = lathe(
        &[
            (0.0, -0.02),
            (0.09, -0.02),
            (0.09, 0.012),
            (0.07, 0.025),
            (0.061, 0.035),
            (0.061, 0.84),
            (0.056, 0.874),
            (0.04, 0.893),
            (0.0, 0.9),
        ],
        28,
        35.0,
        false,
    );
    let mut bands = Builder::new();
    for y in [0.66f32, 0.75] {
        bands.add(
            &lathe(
                &[
                    (0.0615, y),
                    (0.0625, y + 0.002),
                    (0.0625, y + 0.048),
                    (0.0615, y + 0.05),
                ],
                28,
                50.0,
                false,
            ),
            Mat4::IDENTITY,
        );
    }
    (body, bands.finish())
}

// ------------------------------------------------------------------------------- plinth

/// Pedestal for the chrome ball (cast stone): base, shaft and cap. The ball sits at
/// `PLINTH_BALL_Y` above the ground. Returns (stone, metal cup).
pub fn plinth() -> (Mesh, Mesh) {
    let mut stone = Builder::new();
    put(
        &mut stone,
        &rounded_box(Vec3::new(0.62, 0.14, 0.62), 0.02, 2, 1 << 3),
        Vec3::new(0.0, 0.07, 0.0),
    );
    put(
        &mut stone,
        &rounded_box(Vec3::new(0.52, 0.06, 0.52), 0.015, 2, 0),
        Vec3::new(0.0, 0.17, 0.0),
    );
    put(
        &mut stone,
        &rounded_box(Vec3::new(0.4, 0.58, 0.4), 0.012, 2, 0),
        Vec3::new(0.0, 0.49, 0.0),
    );
    put(
        &mut stone,
        &rounded_box(Vec3::new(0.52, 0.06, 0.52), 0.015, 2, 0),
        Vec3::new(0.0, 0.81, 0.0),
    );
    put(
        &mut stone,
        &rounded_box(Vec3::new(0.46, 0.04, 0.46), 0.012, 2, 0),
        Vec3::new(0.0, 0.86, 0.0),
    );
    let cup = lathe(
        &[
            (0.0, 0.88),
            (0.12, 0.88),
            (0.13, 0.9),
            (0.125, 0.925),
            (0.115, 0.93),
            (0.1, 0.915),
            (0.0, 0.9),
        ],
        40,
        40.0,
        false,
    );
    (stone.finish(), cup)
}

/// Height of the chrome ball's center above the ground for a ball of radius `r`.
pub fn plinth_ball_y(r: f32) -> f32 {
    0.93 + (r * r - 0.11f32 * 0.11).max(0.0).sqrt() - 0.004
}

// ------------------------------------------------------------------------------- mast

/// Guyed lattice mast meshes and where its instruments go.
pub struct Mast {
    pub galv: Mesh,
    pub concrete: Mesh,
    /// White-painted logger box.
    pub white: Mesh,
    pub solar: Mesh,
    /// Red obstruction light lens.
    pub beacon: Mesh,
    /// Where the anemometer rotor, the wind vane rotor and the beacon go (mast frame).
    pub anemometer_at: Vec3,
    pub vane_at: Vec3,
    pub beacon_at: Vec3,
}

/// Triangular lattice mast `height` m tall with three guy wires to anchors `guy_r` m out (in
/// the directions of `guy_dirs`, radians from +Z toward +X), a crossarm with the anemometer
/// (+X) and wind vane (−X) mounts, a lightning rod, a logger box and a solar panel facing −Z.
pub fn mast(height: f32, guy_r: f32, guy_dirs: [f32; 3]) -> Mast {
    let mut galv = Builder::new();
    let mut concrete = Builder::new();
    let mut white = Builder::new();
    concrete.add(
        &rounded_box(Vec3::new(0.7, 0.16, 0.7), 0.03, 2, 1 << 3),
        at(0.0, 0.04, 0.0),
    );
    put(
        &mut galv,
        &rb([0.42, 0.016, 0.42], 0.004),
        Vec3::new(0.0, 0.128, 0.0),
    );
    let nut = hex_nut(0.018, 0.016);
    for (x, z) in [(-0.17, -0.17), (0.17, -0.17), (-0.17, 0.17), (0.17, 0.17)] {
        put(&mut galv, &nut, Vec3::new(x, 0.136, z));
    }
    let rad = 0.32 / 3f32.sqrt();
    let legs: Vec<Vec2> = (0..3)
        .map(|k| {
            let a = k as f32 * TAU / 3.0 + FRAC_PI_2;
            Vec2::new(a.cos(), a.sin()) * rad
        })
        .collect();
    let y0 = 0.136;
    let y1 = height;
    for l in &legs {
        galv.add(
            &rod(Vec3::new(l.x, y0, l.y), Vec3::new(l.x, y1, l.y), 0.016, 10),
            Mat4::IDENTITY,
        );
    }
    let bay = 0.4;
    let bays = ((y1 - y0 - 0.1) / bay) as u32;
    for i in 0..3 {
        let (a, b) = (legs[i], legs[(i + 1) % 3]);
        for k in 0..bays {
            let ya = y0 + 0.05 + k as f32 * bay;
            let yb = ya + bay;
            let (p, q) = if k % 2 == 0 { (a, b) } else { (b, a) };
            galv.add(
                &rod(Vec3::new(p.x, ya, p.y), Vec3::new(q.x, yb, q.y), 0.0075, 6),
                Mat4::IDENTITY,
            );
            if k % 2 == 0 {
                galv.add(
                    &rod(Vec3::new(a.x, ya, a.y), Vec3::new(b.x, ya, b.y), 0.0075, 6),
                    Mat4::IDENTITY,
                );
            }
        }
    }
    // Top plate, stub pole, crossarm, instrument mounts, lightning rod.
    put(
        &mut galv,
        &rb([0.36, 0.014, 0.36], 0.004),
        Vec3::new(0.0, y1, 0.0),
    );
    galv.add(
        &rod(
            Vec3::new(0.0, y1, 0.0),
            Vec3::new(0.0, y1 + 0.55, 0.0),
            0.03,
            14,
        ),
        Mat4::IDENTITY,
    );
    let arm_y = y1 + 0.42;
    galv.add(
        &rod(
            Vec3::new(-0.62, arm_y, 0.0),
            Vec3::new(0.62, arm_y, 0.0),
            0.02,
            12,
        ),
        Mat4::IDENTITY,
    );
    put(
        &mut galv,
        &rb([0.09, 0.09, 0.08], 0.01),
        Vec3::new(0.0, arm_y, 0.0),
    );
    for x in [-0.6f32, 0.6] {
        galv.add(
            &rod(
                Vec3::new(x, arm_y - 0.03, 0.0),
                Vec3::new(x, arm_y + 0.2, 0.0),
                0.014,
                10,
            ),
            Mat4::IDENTITY,
        );
    }
    // Anemometer body (static) on +X, vane bearing on −X.
    let body = lathe(
        &[
            (0.0, 0.0),
            (0.03, 0.0),
            (0.032, 0.01),
            (0.032, 0.15),
            (0.026, 0.165),
            (0.012, 0.175),
            (0.0, 0.178),
        ],
        20,
        35.0,
        false,
    );
    put(&mut galv, &body, Vec3::new(0.6, arm_y + 0.2, 0.0));
    let bearing = lathe(
        &[
            (0.0, 0.0),
            (0.024, 0.0),
            (0.026, 0.01),
            (0.026, 0.09),
            (0.02, 0.1),
            (0.0, 0.1),
        ],
        18,
        35.0,
        false,
    );
    put(&mut galv, &bearing, Vec3::new(-0.6, arm_y + 0.2, 0.0));
    galv.add(
        &rod(
            Vec3::new(0.0, y1 + 0.55, 0.0),
            Vec3::new(0.0, y1 + 1.25, 0.0),
            0.007,
            8,
        ),
        Mat4::IDENTITY,
    );
    let tip = lathe(&[(0.0, 0.0), (0.009, 0.0), (0.0, 0.05)], 8, 30.0, false);
    put(&mut galv, &tip, Vec3::new(0.0, y1 + 1.25, 0.0));
    // Guy ring and wires to anchors with turnbuckles.
    let guy_y = height * 0.72;
    put(
        &mut galv,
        &rb([0.42, 0.03, 0.42], 0.01),
        Vec3::new(0.0, guy_y, 0.0),
    );
    let tb = lathe(
        &[
            (0.0, 0.0),
            (0.008, 0.0),
            (0.014, 0.03),
            (0.014, 0.13),
            (0.008, 0.16),
            (0.0, 0.16),
        ],
        8,
        35.0,
        false,
    );
    for a in guy_dirs {
        let d = Vec3::new(a.sin(), 0.0, a.cos());
        let anchor = d * guy_r + Vec3::Y * 0.12;
        let top = d * 0.2 + Vec3::Y * guy_y;
        concrete.add(
            &rounded_box(Vec3::new(0.3, 0.16, 0.3), 0.02, 2, 1 << 3),
            Mat4::from_translation(d * guy_r + Vec3::Y * 0.04),
        );
        let dir = (top - anchor).normalize();
        let tb_end = anchor + dir * 0.32;
        galv.add(&rod(anchor, anchor + dir * 0.15, 0.007, 8), Mat4::IDENTITY);
        galv.add(
            &tb,
            Mat4::from_rotation_translation(
                Quat::from_rotation_arc(Vec3::Y, dir),
                anchor + dir * 0.15,
            ),
        );
        galv.add(&rod(tb_end, top, 0.0045, 6), Mat4::IDENTITY);
    }
    // Logger box with a hinged lid line and cable gland, solar panel above facing south (−Z).
    let face = Vec3::new(0.0, 0.0, -rad * 0.5 - 0.02);
    white.add(
        &rounded_box(Vec3::new(0.34, 0.42, 0.16), 0.018, 2, 0),
        Mat4::from_translation(face + Vec3::new(0.0, 1.45, -0.08)),
    );
    put(
        &mut galv,
        &rb([0.08, 0.02, 0.02], 0.005),
        face + Vec3::new(0.12, 1.45, -0.165),
    );
    galv.add(
        &rod(
            face + Vec3::new(-0.08, 1.24, -0.08),
            face + Vec3::new(-0.08, 1.18, -0.08),
            0.012,
            10,
        ),
        Mat4::IDENTITY,
    );
    for y in [1.3f32, 1.6] {
        put(
            &mut galv,
            &rb([0.38, 0.03, 0.03], 0.005),
            face + Vec3::new(0.0, y, 0.0),
        );
    }
    let tilt = Quat::from_rotation_x(0.75);
    let pc = face + Vec3::new(0.0, 2.25, -0.2);
    let mut solar = Builder::new();
    solar.add(
        &quad(Vec3::new(0.0, 0.0, -0.016), Vec3::NEG_X, Vec3::Y, 0.5, 0.98),
        Mat4::from_rotation_translation(tilt, pc),
    );
    // Panel frame and back.
    for (c, s) in [
        (Vec3::new(0.0, 0.5, 0.0), [0.54, 0.03, 0.035]),
        (Vec3::new(0.0, -0.5, 0.0), [0.54, 0.03, 0.035]),
        (Vec3::new(0.26, 0.0, 0.0), [0.03, 1.0, 0.035]),
        (Vec3::new(-0.26, 0.0, 0.0), [0.03, 1.0, 0.035]),
    ] {
        galv.add(
            &rb(s, 0.004),
            Mat4::from_rotation_translation(tilt, pc) * Mat4::from_translation(c),
        );
    }
    white.add(
        &rb([0.5, 0.98, 0.01], 0.002),
        Mat4::from_rotation_translation(tilt, pc)
            * Mat4::from_translation(Vec3::new(0.0, 0.0, 0.004)),
    );
    galv.add(
        &rod(
            face + Vec3::new(0.0, 1.9, 0.0),
            pc + tilt * Vec3::new(0.0, -0.2, 0.02),
            0.014,
            8,
        ),
        Mat4::IDENTITY,
    );
    let beacon_at = Vec3::new(0.0, y1 + 0.58, 0.0);
    let beacon = lathe(
        &[
            (0.0, 0.0),
            (0.035, 0.0),
            (0.035, 0.03),
            (0.03, 0.07),
            (0.015, 0.09),
            (0.0, 0.092),
        ],
        16,
        30.0,
        false,
    );
    put(
        &mut galv,
        &lathe(
            &[(0.0, 0.0), (0.04, 0.0), (0.04, 0.02), (0.0, 0.02)],
            16,
            30.0,
            false,
        ),
        Vec3::new(0.0, y1 + 0.55, 0.0),
    );
    let mut b_lens = Builder::new();
    put(&mut b_lens, &beacon, beacon_at);
    Mast {
        galv: galv.finish(),
        concrete: concrete.finish(),
        white: white.finish(),
        solar: solar.finish(),
        beacon: b_lens.finish(),
        anemometer_at: Vec3::new(0.6, arm_y + 0.2 + 0.178, 0.0),
        vane_at: Vec3::new(-0.6, arm_y + 0.2 + 0.1, 0.0),
        beacon_at,
    }
}

/// Cup anemometer rotor around its hub (spins about +Y): (metal hub and arms, black cups).
/// Arm radius `CUP_RADIUS`; cups open backwards for a counter-clockwise spin seen from above.
pub fn anemometer_rotor() -> (Mesh, Mesh) {
    let mut metal = Builder::new();
    let mut cups = Builder::new();
    metal.add(
        &lathe(
            &[
                (0.0, 0.0),
                (0.02, 0.0),
                (0.022, 0.01),
                (0.022, 0.035),
                (0.016, 0.05),
                (0.0, 0.056),
            ],
            18,
            35.0,
            false,
        ),
        Mat4::IDENTITY,
    );
    let (r_out, r_in) = (0.052f32, 0.049f32);
    let mut prof = Vec::new();
    // Cup opening toward +Y: inner surface from the bottom up, rim, outer surface down.
    for k in 0..=10 {
        let a = k as f32 / 10.0 * FRAC_PI_2;
        prof.push((r_in * a.sin(), -r_in * a.cos()));
    }
    prof.push((r_out + 0.002, 0.002));
    for k in (0..=10).rev() {
        let a = k as f32 / 10.0 * FRAC_PI_2;
        prof.push((r_out * a.sin(), -r_out * a.cos()));
    }
    prof.reverse();
    let cup = lathe(&prof, 24, 50.0, false);
    for k in 0..3 {
        let a = k as f32 * TAU / 3.0;
        let d = Vec3::new(a.cos(), 0.0, -a.sin());
        metal.add(
            &rod(
                d * 0.015 + Vec3::Y * 0.025,
                d * (CUP_RADIUS - 0.045) + Vec3::Y * 0.025,
                0.005,
                8,
            ),
            Mat4::IDENTITY,
        );
        let open = d.cross(Vec3::Y).normalize();
        cups.add(
            &cup,
            Mat4::from_rotation_translation(
                Quat::from_rotation_arc(Vec3::Y, open),
                d * CUP_RADIUS + Vec3::Y * 0.025,
            ),
        );
    }
    (metal.finish(), cups.finish())
}

/// Radius of the anemometer cup centers, meters.
pub const CUP_RADIUS: f32 = 0.16;

/// Wind vane rotor around its pivot (yaws about +Y): tail fin downwind (+Z), counterweight
/// nose upwind (−Z). Returns (aluminium, brass nose).
pub fn vane_rotor() -> (Mesh, Mesh) {
    let mut alu = Builder::new();
    alu.add(
        &lathe(
            &[
                (0.0, 0.0),
                (0.017, 0.0),
                (0.017, 0.07),
                (0.012, 0.08),
                (0.0, 0.08),
            ],
            16,
            35.0,
            false,
        ),
        Mat4::IDENTITY,
    );
    alu.add(
        &rod(
            Vec3::new(0.0, 0.05, -0.3),
            Vec3::new(0.0, 0.05, 0.45),
            0.007,
            8,
        ),
        Mat4::IDENTITY,
    );
    let fin = extrude(
        &[
            Vec2::new(0.2, -0.02),
            Vec2::new(0.47, -0.05),
            Vec2::new(0.47, 0.14),
            Vec2::new(0.24, 0.05),
        ],
        0.004,
        30.0,
    );
    // Outline drawn in (z, y): rotate so the extrusion's x becomes z.
    alu.add(
        &fin,
        Mat4::from_rotation_translation(
            Quat::from_rotation_y(-FRAC_PI_2),
            Vec3::new(0.0, 0.05, 0.0),
        ),
    );
    let nose = lathe(
        &[
            (0.0, 0.0),
            (0.02, 0.012),
            (0.024, 0.05),
            (0.02, 0.09),
            (0.008, 0.1),
            (0.0, 0.1),
        ],
        16,
        35.0,
        false,
    );
    let mut brass = Builder::new();
    brass.add(
        &nose,
        Mat4::from_rotation_translation(
            Quat::from_rotation_x(FRAC_PI_2),
            Vec3::new(0.0, 0.05, -0.38),
        ),
    );
    (alu.finish(), brass.finish())
}

// ------------------------------------------------------------------------------- windsock

/// Windsock pole (galvanised) with a tilting hinge, `height` m to the swivel.
pub fn windsock_pole(height: f32) -> Mesh {
    let mut g = Builder::new();
    put(
        &mut g,
        &rb([0.3, 0.015, 0.3], 0.004),
        Vec3::new(0.0, 0.0075, 0.0),
    );
    for x in [-0.05f32, 0.05] {
        put(
            &mut g,
            &rb([0.012, 0.22, 0.12], 0.003),
            Vec3::new(x, 0.12, 0.0),
        );
    }
    g.add(
        &rod(
            Vec3::new(-0.07, 0.17, 0.0),
            Vec3::new(0.07, 0.17, 0.0),
            0.012,
            8,
        ),
        Mat4::IDENTITY,
    );
    let nut = hex_nut(0.018, 0.014);
    for (x, z) in [(-0.12, -0.12), (0.12, -0.12), (-0.12, 0.12), (0.12, 0.12)] {
        put(&mut g, &nut, Vec3::new(x, 0.015, z));
    }
    g.add(
        &lathe(
            &[
                (0.0, 0.08),
                (0.04, 0.08),
                (0.04, height - 0.02),
                (0.03, height),
                (0.0, height),
            ],
            18,
            35.0,
            false,
        ),
        Mat4::IDENTITY,
    );
    g.finish()
}

/// Swivel frame of the windsock (rotates with it): bearing, throat ring and a second ring
/// joined by rods, holding the first fabric band open along +Z.
pub fn windsock_frame(throat: f32) -> Mesh {
    let mut g = Builder::new();
    g.add(
        &rod(
            Vec3::new(0.0, -throat - 0.2, 0.0),
            Vec3::new(0.0, -throat, 0.0),
            0.02,
            12,
        ),
        Mat4::IDENTITY,
    );
    let ring = |r: f32| -> Mesh {
        let pts: Vec<Vec3> = (0..=40)
            .map(|k| {
                let a = k as f32 / 40.0 * TAU;
                Vec3::new(r * a.cos(), r * a.sin(), 0.0)
            })
            .collect();
        tube(&pts, 0.007, 8, false)
    };
    g.add(&ring(throat), at(0.0, 0.0, 0.0));
    g.add(&ring(throat * 0.93), at(0.0, 0.0, 0.38));
    for k in 0..4 {
        let a = k as f32 * FRAC_PI_2 + FRAC_PI_2 * 0.5;
        let (c, s) = (a.cos(), a.sin());
        g.add(
            &rod(
                Vec3::new(c * throat, s * throat, 0.0),
                Vec3::new(c * throat * 0.93, s * throat * 0.93, 0.38),
                0.005,
                6,
            ),
            Mat4::IDENTITY,
        );
    }
    g.finish()
}

/// One fabric band of the windsock: an open, gently pleated cone from radius `r0` at z = 0 to
/// `r1` at z = `len` (draw double sided).
pub fn sock_band(r0: f32, r1: f32, len: f32, seed: u32) -> Mesh {
    let mut b = Builder::new();
    let sides = 28;
    let rows = 6;
    let mut rng = Rng::new(seed);
    let phase = rng.next() * TAU;
    for j in 0..=rows {
        let t = j as f32 / rows as f32;
        let z = t * len;
        let r = r0 + (r1 - r0) * t;
        for s in 0..=sides {
            let a = s as f32 / sides as f32 * TAU;
            let pleat =
                1.0 + 0.035 * (a * 6.0 + phase + t * 2.0).sin() + 0.02 * (a * 11.0 + t * 5.0).sin();
            let p = Vec3::new(r * pleat * a.cos(), r * pleat * a.sin(), z);
            b.vertex(p, Vec3::new(a.cos(), a.sin(), 0.0), Vec2::new(a * r, z));
        }
    }
    let row = sides + 1;
    for j in 0..rows {
        for s in 0..sides {
            let a = j * row + s;
            let out = Vec3::from(b.mesh.vertices[a as usize].normal);
            b.tri_facing(a, a + 1, a + row + 1, out);
            b.tri_facing(a, a + row + 1, a + row, out);
        }
    }
    b.mesh.compute_normals();
    b.finish()
}

// ------------------------------------------------------------------------------- screen

/// Stevenson screen: a white louvred hut on legs, door to +Z, double roof.
pub fn stevenson_screen() -> Mesh {
    let mut w = Builder::new();
    let (bw, bd, bh) = (0.78f32, 0.54f32, 0.6f32);
    let y0 = 1.0f32;
    // Legs with braces.
    for (x, z) in [(-0.3, -0.2), (0.3, -0.2), (-0.3, 0.2), (0.3, 0.2)] {
        put(
            &mut w,
            &rounded_box(Vec3::new(0.06, y0 + 0.05, 0.06), 0.008, 1, 1 << 3),
            Vec3::new(x, (y0 + 0.05) * 0.5 - 0.02, z),
        );
    }
    for z in [-0.2f32, 0.2] {
        put(
            &mut w,
            &rb([0.6, 0.06, 0.025], 0.006),
            Vec3::new(0.0, 0.4, z + 0.042 * z.signum()),
        );
    }
    for x in [-0.3f32, 0.3] {
        put(
            &mut w,
            &rb([0.025, 0.06, 0.4], 0.006),
            Vec3::new(x + 0.042 * x.signum(), 0.55, 0.0),
        );
    }
    // Floor boards with gaps.
    for k in 0..4 {
        let z = -bd * 0.5 + 0.07 + k as f32 * (bd - 0.14) / 3.0;
        put(
            &mut w,
            &rb([bw - 0.04, 0.018, 0.1], 0.004),
            Vec3::new(0.0, y0 + 0.009, z),
        );
    }
    // Corner posts, top and bottom rails.
    for (x, z) in [(-1.0, -1.0), (1.0, -1.0), (-1.0, 1.0), (1.0, 1.0)] {
        put(
            &mut w,
            &rb([0.045, bh, 0.045], 0.006),
            Vec3::new(
                x * (bw * 0.5 - 0.0225),
                y0 + bh * 0.5,
                z * (bd * 0.5 - 0.0225),
            ),
        );
    }
    for (y, h) in [(y0 + 0.025, 0.05), (y0 + bh - 0.025, 0.05)] {
        for z in [-1.0f32, 1.0] {
            put(
                &mut w,
                &rb([bw, h, 0.03], 0.006),
                Vec3::new(0.0, y, z * (bd * 0.5 - 0.015)),
            );
        }
        for x in [-1.0f32, 1.0] {
            put(
                &mut w,
                &rb([0.03, h, bd], 0.006),
                Vec3::new(x * (bw * 0.5 - 0.015), y, 0.0),
            );
        }
    }
    // Double louvres on all four sides.
    let slat_count = 11;
    let (ly0, ly1) = (y0 + 0.07, y0 + bh - 0.07);
    let pitch = (ly1 - ly0) / slat_count as f32;
    let louvres = |w: &mut Builder, center: Vec3, len: f32, normal: Vec3| {
        let along = Vec3::Y.cross(normal).normalize();
        let base = rounded_box(Vec3::new(len, 0.008, 0.062), 0.003, 1, 0);
        for k in 0..slat_count {
            let y = ly0 + pitch * (k as f32 + 0.5);
            for (depth, tilt) in [(0.0f32, 0.75f32), (-0.032, -0.75)] {
                let c = center + normal * depth + Vec3::Y * (y - center.y);
                let frame = Mat4::from_cols(
                    along.extend(0.0),
                    Vec3::Y.extend(0.0),
                    normal.extend(0.0),
                    c.extend(1.0),
                );
                w.add(&base, frame * Mat4::from_rotation_x(tilt));
            }
        }
    };
    let yc = y0 + bh * 0.5;
    louvres(
        &mut w,
        Vec3::new(0.0, yc, bd * 0.5 - 0.016),
        bw - 0.09,
        Vec3::Z,
    );
    louvres(
        &mut w,
        Vec3::new(0.0, yc, -bd * 0.5 + 0.016),
        bw - 0.09,
        Vec3::NEG_Z,
    );
    louvres(
        &mut w,
        Vec3::new(bw * 0.5 - 0.016, yc, 0.0),
        bd - 0.09,
        Vec3::X,
    );
    louvres(
        &mut w,
        Vec3::new(-bw * 0.5 + 0.016, yc, 0.0),
        bd - 0.09,
        Vec3::NEG_X,
    );
    // Door stiles, hinges and latch on the front.
    for x in [-1.0f32, 1.0] {
        put(
            &mut w,
            &rb([0.035, bh - 0.1, 0.02], 0.005),
            Vec3::new(x * (bw * 0.5 - 0.06), yc, bd * 0.5 + 0.004),
        );
    }
    put(
        &mut w,
        &rb([bw - 0.1, 0.035, 0.02], 0.005),
        Vec3::new(0.0, yc, bd * 0.5 + 0.004),
    );
    // Roofs: inner flat roof, then a sloping upper roof on spacers (air gap).
    put(
        &mut w,
        &rb([bw + 0.06, 0.025, bd + 0.06], 0.006),
        Vec3::new(0.0, y0 + bh + 0.0125, 0.0),
    );
    for (x, z) in [(-0.3, -0.18), (0.3, -0.18), (-0.3, 0.18), (0.3, 0.18)] {
        put(
            &mut w,
            &rb([0.05, 0.05 + z * 0.12, 0.05], 0.005),
            Vec3::new(x, y0 + bh + 0.05 + z * 0.06, z),
        );
    }
    put_r(
        &mut w,
        &rb([bw + 0.16, 0.022, bd + 0.2], 0.006),
        Vec3::new(0.0, y0 + bh + 0.085, 0.0),
        Quat::from_rotation_x(0.12),
    );
    w.finish()
}

/// Brass hinges and the door latch of the Stevenson screen.
pub fn screen_hardware() -> Mesh {
    let mut m = Builder::new();
    let (bw, bd) = (0.78f32, 0.54f32);
    for y in [1.12f32, 1.48] {
        m.add(
            &rod(
                Vec3::new(-bw * 0.5 + 0.02, y - 0.03, bd * 0.5 + 0.016),
                Vec3::new(-bw * 0.5 + 0.02, y + 0.03, bd * 0.5 + 0.016),
                0.006,
                8,
            ),
            Mat4::IDENTITY,
        );
        put(
            &mut m,
            &rb([0.06, 0.04, 0.003], 0.001),
            Vec3::new(-bw * 0.5 + 0.05, y, bd * 0.5 + 0.0155),
        );
    }
    m.add(
        &lathe(
            &[
                (0.0, 0.0),
                (0.006, 0.0),
                (0.006, 0.02),
                (0.012, 0.025),
                (0.012, 0.035),
                (0.0, 0.037),
            ],
            12,
            35.0,
            false,
        ),
        Mat4::from_rotation_translation(
            Quat::from_rotation_x(FRAC_PI_2),
            Vec3::new(bw * 0.5 - 0.06, 1.3, bd * 0.5 + 0.014),
        ),
    );
    m.finish()
}

// ------------------------------------------------------------------------------- gauges

/// Rain gauge on a post: (galvanised body and post, glass sight tube, brass fittings,
/// scale plate). The sight tube's water column starts at `RAIN_TUBE_Y0`.
pub fn rain_gauge() -> (Mesh, Mesh, Mesh, Mesh) {
    let mut galv = Builder::new();
    galv.add(
        &lathe(
            &[
                (0.0, 0.0),
                (0.035, 0.0),
                (0.035, 0.03),
                (0.03, 0.04),
                (0.03, 0.7),
                (0.0, 0.71),
            ],
            16,
            35.0,
            false,
        ),
        Mat4::IDENTITY,
    );
    galv.add(
        &lathe(
            &[
                (0.0, 0.66),
                (0.074, 0.66),
                (0.08, 0.672),
                (0.08, 1.11),
                (0.084, 1.12),
                (0.084, 1.145),
                (0.08, 1.155),
                (0.08, 1.245),
                (0.0805, 1.25),
                (0.0785, 1.246),
                (0.0785, 1.2),
                (0.01, 1.13),
                (0.006, 1.1),
                (0.0, 1.1),
            ],
            40,
            35.0,
            false,
        ),
        Mat4::IDENTITY,
    );
    // Clamp band around the gauge and the post.
    galv.add(
        &lathe(
            &[(0.082, 0.8), (0.085, 0.803), (0.085, 0.837), (0.082, 0.84)],
            40,
            40.0,
            false,
        ),
        Mat4::IDENTITY,
    );
    let tube_z = 0.105;
    let (y0, y1) = (RAIN_TUBE_Y0 - 0.01, RAIN_TUBE_Y0 + RAIN_TUBE_LEN + 0.01);
    let mut glass = Builder::new();
    glass.add(
        &lathe(
            &[(0.0, y0), (0.013, y0), (0.013, y1), (0.0, y1)],
            18,
            35.0,
            false,
        ),
        at(0.0, 0.0, tube_z),
    );
    let mut brass = Builder::new();
    for y in [y0 - 0.02, y1] {
        brass.add(
            &lathe(
                &[(0.0, 0.0), (0.016, 0.0), (0.016, 0.02), (0.0, 0.02)],
                16,
                30.0,
                false,
            ),
            at(0.0, y, tube_z),
        );
    }
    brass.add(
        &rod(
            Vec3::new(0.0, y0 - 0.01, tube_z),
            Vec3::new(0.0, y0 - 0.01, 0.075),
            0.006,
            8,
        ),
        Mat4::IDENTITY,
    );
    brass.add(
        &rod(
            Vec3::new(0.0, y1 + 0.01, tube_z),
            Vec3::new(0.0, y1 + 0.01, 0.075),
            0.006,
            8,
        ),
        Mat4::IDENTITY,
    );
    let plate = quad(
        Vec3::new(0.034, (y0 + y1) * 0.5, tube_z - 0.004),
        Vec3::X,
        Vec3::Y,
        0.036,
        y1 - y0,
    );
    (galv.finish(), glass.finish(), brass.finish(), plate)
}

/// Bottom of the rain gauge's water column, meters above the ground.
pub const RAIN_TUBE_Y0: f32 = 0.74;
/// Length of the sight tube's scale (full at the gauge's capacity), meters.
pub const RAIN_TUBE_LEN: f32 = 0.34;

/// Graduated snow stake, 6 cm square and 2 m tall (texture covers each face).
pub fn snow_stake() -> Mesh {
    let mut m = rounded_box(Vec3::new(0.06, 2.0, 0.06), 0.004, 1, 1 << 3);
    for v in &mut m.vertices {
        if v.normal[1].abs() < 0.9 {
            v.uv = [v.uv[0] / 0.06, v.uv[1] / 2.0];
        } else {
            v.uv = [0.5, 0.01];
        }
    }
    transform(&mut m, at(0.0, 1.0, 0.0));
    m
}

/// Snow piled around the stake for a depth of 1 m (scaled by the real depth).
pub fn snow_mound() -> Mesh {
    // A drift collar about 0.3 m across: flat on top at the snow depth, slumping irregularly
    // to the ground, with a little scour right at the stake.
    let mut m = lathe(
        &[
            (0.34, 0.0),
            (0.3, 0.2),
            (0.25, 0.6),
            (0.19, 0.9),
            (0.12, 1.0),
            (0.05, 0.98),
            (0.034, 0.93),
            (0.0, 0.93),
        ],
        36,
        70.0,
        false,
    );
    for v in &mut m.vertices {
        let (x, z) = (v.position[0], v.position[2]);
        let r = (x * x + z * z).sqrt();
        if r > 0.04 {
            let a = z.atan2(x);
            let wobble = 1.0
                + 0.22 * super::noise::fbm(a.cos() * 1.6 + 3.0, a.sin() * 1.6, (0, 0), 3, 77)
                + 0.1 * (a * 3.0).sin();
            let k = 1.0 + (wobble - 1.0) * (r / 0.34).min(1.0);
            v.position[0] *= k;
            v.position[2] *= k;
        }
    }
    m
}

/// The thermometer board assembly: (galvanised posts, board with the printed scale (texture
/// covers it), glass tube and bulb, red bulb, LED display housing, display face).
pub struct Thermometer {
    pub galv: Mesh,
    pub board: Mesh,
    pub glass: Mesh,
    pub bulb: Mesh,
    pub housing: Mesh,
    pub face: Mesh,
}

/// Board size and where its scale sits (meters, board-local: x across, y up from the ground).
pub const THERMO_BOARD: (f32, f32, f32) = (0.6, 2.0, 0.55);
/// Height of the −30 °C mark above the ground; the scale runs `THERMO_SCALE_LEN` m to +50 °C.
pub const THERMO_SCALE_Y0: f32 = 0.86;
pub const THERMO_SCALE_LEN: f32 = 1.5;
/// Height of the digital display's center above the ground.
pub const DISPLAY_Y: f32 = 2.86;

pub fn thermometer() -> Thermometer {
    let (bw, bh, by0) = THERMO_BOARD;
    let mut galv = Builder::new();
    for x in [-0.36f32, 0.36] {
        galv.add(
            &rounded_box(Vec3::new(0.06, 3.15, 0.06), 0.006, 1, 1 << 3),
            at(x, 3.15 * 0.5 - 0.02, -0.04),
        );
        galv.add(
            &rounded_box(Vec3::new(0.2, 0.012, 0.2), 0.003, 1, 0),
            at(x, 0.006, -0.04),
        );
        let nut = hex_nut(0.014, 0.012);
        for (dx, dz) in [(-0.07, -0.07), (0.07, -0.07), (-0.07, 0.07), (0.07, 0.07)] {
            galv.add(&nut, at(x + dx, 0.012, -0.04 + dz));
        }
        for y in [by0 + 0.2, by0 + bh - 0.2, DISPLAY_Y] {
            galv.add(&rb([0.12, 0.05, 0.02], 0.004), at(x, y, -0.0));
        }
    }
    // Board: rounded outline extruded 2 cm; front cap texture covers it (uv in meters → the
    // material scales by 1/size).
    let mut board = extrude(&rounded_rect(bw, bh, 0.035, 4), 0.02, 40.0);
    transform(&mut board, at(0.0, by0 + bh * 0.5, 0.02));
    let mut glass = Builder::new();
    let tz = 0.055;
    let top = THERMO_SCALE_Y0 + THERMO_SCALE_LEN + 0.05;
    glass.add(
        &lathe(
            &[
                (0.0, THERMO_SCALE_Y0 - 0.12),
                (0.02, THERMO_SCALE_Y0 - 0.12),
                (0.02, top - 0.02),
                (0.012, top),
                (0.0, top + 0.004),
            ],
            20,
            35.0,
            false,
        ),
        at(0.0, 0.0, tz),
    );
    let mut bulb = Builder::new();
    bulb.add(
        &lathe(&sphere_profile(0.032, 12), 24, 60.0, false),
        at(0.0, THERMO_SCALE_Y0 - 0.15, tz),
    );
    glass.add(
        &lathe(&sphere_profile(0.038, 12), 24, 60.0, false),
        at(0.0, THERMO_SCALE_Y0 - 0.15, tz),
    );
    for y in [
        THERMO_SCALE_Y0 + 0.2,
        THERMO_SCALE_Y0 + 0.75,
        THERMO_SCALE_Y0 + 1.3,
    ] {
        galv.add(&rb([0.07, 0.018, 0.035], 0.005), at(0.0, y, tz - 0.012));
    }
    let mut housing = Builder::new();
    housing.add(
        &rounded_box(Vec3::new(0.86, 0.4, 0.12), 0.025, 2, 0),
        at(0.0, DISPLAY_Y, 0.0),
    );
    housing.add(
        &rb([0.9, 0.03, 0.16], 0.008),
        at(0.0, DISPLAY_Y + 0.215, 0.01),
    );
    let face = quad(
        Vec3::new(0.0, DISPLAY_Y, 0.0605),
        Vec3::X,
        Vec3::Y,
        0.78,
        0.32,
    );
    Thermometer {
        galv: galv.finish(),
        board,
        glass: glass.finish(),
        bulb: bulb.finish(),
        housing: housing.finish(),
        face,
    }
}

/// Profile of a sphere of radius `r` around (0, 0) for [`lathe`].
pub fn sphere_profile(r: f32, steps: u32) -> Vec<(f32, f32)> {
    (0..=steps)
        .map(|k| {
            let a = -FRAC_PI_2 + k as f32 / steps as f32 * PI;
            (r * a.cos().max(0.0), r * a.sin())
        })
        .collect()
}

/// One LED segment of a seven-segment digit: a bar with pointed ends, `len` long along X,
/// `thick` across, in the XY plane facing +Z.
pub fn led_segment(len: f32, thick: f32) -> Mesh {
    let (h, t) = (len * 0.5, thick * 0.5);
    extrude(
        &[
            Vec2::new(-h, 0.0),
            Vec2::new(-h + t, -t),
            Vec2::new(h - t, -t),
            Vec2::new(h, 0.0),
            Vec2::new(h - t, t),
            Vec2::new(-h + t, t),
        ],
        0.004,
        30.0,
    )
}

// ------------------------------------------------------------------------------- fence, sign

/// Post-and-rail timber fence around `[x0, x1] × [z0, z1]` with a gate gap on the +X side
/// between `gate.0` and `gate.1` (z). Returns (timber, galvanised fittings).
pub fn fence(x0: f32, x1: f32, z0: f32, z1: f32, gate: (f32, f32), seed: u32) -> (Mesh, Mesh) {
    let mut t = Builder::new();
    let mut g = Builder::new();
    let mut rng = Rng::new(seed);
    let post_h = 1.12;
    let cap = lathe(&[(0.0, 0.0), (0.07, 0.0), (0.0, 0.05)], 4, 30.0, true);
    let post = |t: &mut Builder, x: f32, z: f32, rng: &mut Rng| {
        let mut p = rounded_box(Vec3::new(0.09, post_h, 0.09), 0.01, 1, 1 << 3);
        grain_vertical(&mut p);
        offset_uv(&mut p, rng.next() * 3.0, rng.next() * 3.0);
        t.add(
            &p,
            trs(
                Vec3::new(x, post_h * 0.5 - 0.03, z),
                Quat::from_rotation_y(rng.signed() * 0.03),
                Vec3::ONE,
            ),
        );
        t.add(
            &cap,
            trs(
                Vec3::new(x, post_h - 0.03, z),
                Quat::from_rotation_y(FRAC_PI_2 * 0.5),
                Vec3::ONE,
            ),
        );
    };
    let run = |t: &mut Builder, a: Vec2, b: Vec2, rng: &mut Rng, outward: Vec2| {
        let len = a.distance(b);
        let n = (len / 1.8).ceil().max(1.0) as u32;
        for k in 0..=n {
            let p = a + (b - a) * (k as f32 / n as f32);
            post(t, p.x, p.y, rng);
        }
        let dir = (b - a) / len;
        let yaw = -dir.y.atan2(dir.x);
        for y in [0.42f32, 0.92] {
            let mut r = rounded_box(Vec3::new(len + 0.06, 0.09, 0.035), 0.008, 1, 0);
            offset_uv(&mut r, rng.next() * 3.0, rng.next() * 3.0);
            let c = (a + b) * 0.5 + outward * 0.062;
            t.add(
                &r,
                trs(
                    Vec3::new(c.x, y + rng.signed() * 0.01, c.y),
                    Quat::from_rotation_y(yaw),
                    Vec3::ONE,
                ),
            );
        }
    };
    run(
        &mut t,
        Vec2::new(x0, z1),
        Vec2::new(x1, z1),
        &mut rng,
        Vec2::Y,
    );
    run(
        &mut t,
        Vec2::new(x0, z0),
        Vec2::new(x1, z0),
        &mut rng,
        Vec2::NEG_Y,
    );
    run(
        &mut t,
        Vec2::new(x0, z0),
        Vec2::new(x0, z1),
        &mut rng,
        Vec2::NEG_X,
    );
    run(
        &mut t,
        Vec2::new(x1, z0),
        Vec2::new(x1, gate.0),
        &mut rng,
        Vec2::X,
    );
    run(
        &mut t,
        Vec2::new(x1, gate.1),
        Vec2::new(x1, z1),
        &mut rng,
        Vec2::X,
    );
    // Gate: frame with a diagonal brace, standing a little open.
    let gw = gate.1 - gate.0 - 0.1;
    let mut gate_m = Builder::new();
    for y in [0.25f32, 0.95] {
        gate_m.add(
            &rounded_box(Vec3::new(0.03, 0.08, gw), 0.008, 1, 0),
            at(0.0, y, gw * 0.5),
        );
    }
    for z in [0.04f32, gw - 0.04] {
        let mut stile = rounded_box(Vec3::new(0.03, 0.8, 0.07), 0.008, 1, 0);
        grain_vertical(&mut stile);
        gate_m.add(&stile, at(0.0, 0.6, z));
    }
    let diag = Vec3::new(0.0, 0.7, gw - 0.1);
    let mut brace = rounded_box(Vec3::new(0.025, diag.length(), 0.06), 0.006, 1, 0);
    grain_vertical(&mut brace);
    gate_m.add(
        &brace,
        Mat4::from_rotation_translation(
            Quat::from_rotation_arc(Vec3::Y, diag.normalize()),
            Vec3::new(0.0, 0.6, gw * 0.5),
        ),
    );
    t.add(
        &gate_m.finish(),
        Mat4::from_rotation_translation(
            Quat::from_rotation_y(-0.35),
            Vec3::new(x1 + 0.03, 0.0, gate.0 + 0.05),
        ),
    );
    for y in [0.3f32, 0.9] {
        g.add(
            &rb([0.03, 0.03, 0.12], 0.004),
            at(x1 + 0.04, y, gate.0 + 0.05),
        );
    }
    g.add(
        &rb([0.04, 0.02, 0.1], 0.004),
        at(x1 + 0.06, 0.95, gate.1 - 0.05),
    );
    (t.finish(), g.finish())
}

/// Sign on two posts: (galvanised posts and fixings, board edge in the sign's color, front
/// face (texture covers it)). Board `w × h` with its lower edge `y0` above the ground.
pub fn sign(w: f32, h: f32, y0: f32) -> (Mesh, Mesh, Mesh) {
    let mut g = Builder::new();
    for x in [-w * 0.32, w * 0.32] {
        g.add(
            &rounded_box(Vec3::new(0.05, y0 + h + 0.05, 0.05), 0.005, 1, 1 << 3),
            at(x, (y0 + h + 0.05) * 0.5 - 0.03, -0.04),
        );
        g.add(
            &lathe(
                &[(0.0, 0.05), (0.03, 0.05), (0.025, 0.06), (0.0, 0.065)],
                4,
                30.0,
                true,
            ),
            trs(
                Vec3::new(x, y0 + h - 0.03, -0.04),
                Quat::from_rotation_y(FRAC_PI_2 * 0.5),
                Vec3::ONE,
            ),
        );
        for y in [y0 + 0.1, y0 + h - 0.1] {
            g.add(&rb([0.09, 0.04, 0.03], 0.004), at(x, y, -0.005));
        }
    }
    let mut edge = extrude(&rounded_rect(w, h, 0.03, 4), 0.012, 40.0);
    transform(&mut edge, at(0.0, y0 + h * 0.5, 0.0));
    let face = quad(
        Vec3::new(0.0, y0 + h * 0.5, 0.0062),
        Vec3::X,
        Vec3::Y,
        w - 0.01,
        h - 0.01,
    );
    (g.finish(), edge, face)
}

/// Bus stop flag on a 3 m pole: the plate (both faces printed) sticks out toward +X in the
/// XY plane, and a timetable case faces +Z on the pole.
pub struct BusFlag {
    /// Galvanised pole, collar, clamps and brackets.
    pub galv: Mesh,
    /// Plate rim and timetable case (powder-coated).
    pub steel: Mesh,
    /// Front and back faces of the plate (the texture covers each once).
    pub face: Mesh,
    /// Timetable sheet.
    pub sheet: Mesh,
    /// Glazing over the sheet.
    pub glass: Mesh,
}

pub fn bus_flag() -> BusFlag {
    let (fw, fh) = super::signs::FLAG_SIZE;
    let (tw, th) = super::signs::TIMETABLE_SIZE;
    let mut galv = Builder::new();
    let mut steel = Builder::new();
    let (pole_h, r) = (3.0f32, 0.038f32);
    galv.add(
        &lathe(
            &[
                (r, -0.05),
                (r, pole_h - 0.012),
                (r * 0.85, pole_h - 0.002),
                (r * 0.5, pole_h + 0.004),
                (0.0, pole_h + 0.006),
            ],
            20,
            40.0,
            false,
        ),
        Mat4::IDENTITY,
    );
    galv.add(
        &lathe(
            &[
                (r + 0.014, -0.01),
                (r + 0.014, 0.04),
                (r + 0.004, 0.055),
                (r, 0.056),
            ],
            20,
            40.0,
            false,
        ),
        Mat4::IDENTITY,
    );
    let band = |b: &mut Builder, y: f32| {
        b.add(
            &lathe(
                &[
                    (r + 0.005, y - 0.02),
                    (r + 0.006, y - 0.018),
                    (r + 0.006, y + 0.018),
                    (r + 0.005, y + 0.02),
                ],
                20,
                40.0,
                false,
            ),
            Mat4::IDENTITY,
        );
    };
    // Plate beside the pole, its top just under the cap.
    let (cx, cy) = (r + 0.025 + fw * 0.5, pole_h - 0.06 - fh * 0.5);
    let mut rim = extrude(&rounded_rect(fw, fh, 0.02, 4), 0.01, 40.0);
    transform(&mut rim, at(cx, cy, 0.0));
    steel.add(&rim, Mat4::IDENTITY);
    let mut face = Builder::new();
    face.add(
        &quad(
            Vec3::new(cx, cy, 0.0052),
            Vec3::X,
            Vec3::Y,
            fw - 0.012,
            fh - 0.012,
        ),
        Mat4::IDENTITY,
    );
    face.add(
        &quad(
            Vec3::new(cx, cy, -0.0052),
            Vec3::NEG_X,
            Vec3::Y,
            fw - 0.012,
            fh - 0.012,
        ),
        Mat4::IDENTITY,
    );
    for y in [cy + fh * 0.5 - 0.07, cy - fh * 0.5 + 0.07] {
        band(&mut galv, y);
        put(
            &mut galv,
            &rb([0.05, 0.035, 0.022], 0.004),
            Vec3::new(r + 0.02, y, 0.0),
        );
    }
    // Timetable case on brackets, facing +Z.
    let (case_y, case_z) = (1.55f32, r + 0.03);
    for y in [case_y - th * 0.35, case_y + th * 0.35] {
        band(&mut galv, y);
        put(
            &mut galv,
            &rb([0.05, 0.03, 0.03], 0.004),
            Vec3::new(0.0, y, r + 0.008),
        );
    }
    put(
        &mut steel,
        &rb([tw + 0.05, th + 0.05, 0.04], 0.01),
        Vec3::new(0.0, case_y, case_z + 0.02),
    );
    let front = case_z + 0.0405;
    let sheet = quad(Vec3::new(0.0, case_y, front), Vec3::X, Vec3::Y, tw, th);
    let glass = quad(
        Vec3::new(0.0, case_y, front + 0.004),
        Vec3::X,
        Vec3::Y,
        tw + 0.02,
        th + 0.02,
    );
    BusFlag {
        galv: galv.finish(),
        steel: steel.finish(),
        face: face.finish(),
        sheet,
        glass,
    }
}

/// Something small scattered on the ground: where, turned how, how big (relative), which
/// atlas cell and a brightness tint.
#[derive(Clone, Copy, Debug)]
pub struct Scatter {
    pub pos: Vec3,
    pub yaw: f32,
    pub size: f32,
    pub variant: u32,
    pub tint: f32,
}

/// Fallen leaves lying on the ground, curled a little (one 3 × 3 patch each), using the 2 × 2
/// leaf atlas. `size` is the leaf length in meters.
pub fn leaf_litter(leaves: &[Scatter]) -> Mesh {
    let mut b = Builder::new();
    for l in leaves {
        let (cu, cv) = (
            (l.variant % 2) as f32 * 0.5,
            ((l.variant / 2) % 2) as f32 * 0.5,
        );
        let rot = Quat::from_rotation_y(l.yaw);
        let s = l.size;
        // Curl: edges up, the tip lifting; the slope gives the normal.
        let curl = |x: f32, z: f32| (0.1 * x * x + 0.06 * (z + 1.0) * (z + 1.0) * 0.25) * s;
        let base = b.mesh.vertices.len() as u32;
        for j in 0..3 {
            for i in 0..3 {
                let (x, z) = (i as f32 - 1.0, j as f32 - 1.0);
                let p = l.pos + rot * Vec3::new(x * 0.5 * s, curl(x, z), z * 0.5 * s);
                let n = rot * Vec3::new(-0.2 * x, 1.0, -0.06 * (z + 1.0)).normalize();
                let uv = Vec2::new(cu + (x + 1.0) * 0.25, cv + (1.0 - z) * 0.25);
                let k = b.vertex(p, n, uv);
                b.mesh.vertices[k as usize].color = [l.tint, l.tint, l.tint, 1.0];
            }
        }
        for j in 0..2u32 {
            for i in 0..2u32 {
                let a = base + j * 3 + i;
                b.tri_facing(a, a + 1, a + 4, Vec3::Y);
                b.tri_facing(a, a + 4, a + 3, Vec3::Y);
            }
        }
    }
    b.finish()
}

/// Grass tufts: three crossed cards each (both sides as separate faces with upward normals,
/// so they light like the ground they grow from), using the two-clump grass atlas. `size`
/// scales a 0.2 m card.
pub fn tufts(items: &[Scatter]) -> Mesh {
    let mut b = Builder::new();
    for t in items {
        let (u0, u1) = if t.variant % 2 == 0 {
            (0.0, 0.5)
        } else {
            (0.5, 1.0)
        };
        let (w, h) = (0.2 * t.size, 0.2 * t.size);
        for k in 0..3 {
            let rot = Quat::from_rotation_y(t.yaw + k as f32 * PI / 3.0);
            let right = rot * Vec3::X;
            let normal = rot * Vec3::Z;
            let up_n = (Vec3::Y + normal * 0.3).normalize();
            let down_n = (Vec3::Y - normal * 0.3).normalize();
            let p =
                |x: f32, y: f32, spread: f32| t.pos + right * x * w * 0.5 * spread + Vec3::Y * y;
            let dark = 0.5 * t.tint;
            for (n, facing) in [(up_n, normal), (down_n, -normal)] {
                let a = b.vertex(p(-1.0, -0.01, 1.0), n, Vec2::new(u0, 1.0));
                let c = b.vertex(p(1.0, -0.01, 1.0), n, Vec2::new(u1, 1.0));
                let d = b.vertex(p(1.0, h, 1.25), n, Vec2::new(u1, 0.0));
                let e = b.vertex(p(-1.0, h, 1.25), n, Vec2::new(u0, 0.0));
                for (i, shade) in [(a, dark), (c, dark), (d, t.tint), (e, t.tint)] {
                    b.mesh.vertices[i as usize].color = [shade, shade, shade, 1.0];
                }
                b.tri_facing(a, c, d, facing);
                b.tri_facing(a, d, e, facing);
            }
        }
    }
    b.finish()
}

#[cfg(test)]
mod tests {
    use super::super::meshgen::check_winding;
    use super::*;

    #[test]
    fn props_build_and_face_outward() {
        let s = shelter(1);
        let lamp = street_lamp();
        let m = mast(7.0, 2.2, [0.0, 2.1, 4.2]);
        let (rotor, cups) = anemometer_rotor();
        let (vane, nose) = vane_rotor();
        let t = thermometer();
        for (name, mesh) in [
            ("shelter steel", &s.steel),
            ("shelter glass", &s.glass),
            ("shelter wood", &s.wood),
            ("lamp", &lamp.steel),
            ("lamp lens", &lamp.lens),
            ("mast", &m.galv),
            ("rotor", &rotor),
            ("cups", &cups),
            ("vane", &vane),
            ("nose", &nose),
            ("thermometer", &t.galv),
            ("board", &t.board),
            ("screen", &stevenson_screen()),
            ("pavers", &pavers(0.0, 2.0, 0.0, 1.2, 0.015, 3)),
            ("kerbs", &kerbs(-1.0, 1.0, -1.0, 1.0, 0.15, 0.06, 2)),
        ] {
            assert!(!mesh.indices.is_empty(), "{name} is empty");
            let bad = check_winding(mesh);
            assert!(
                bad * 50 <= mesh.indices.len() / 3,
                "{name}: {bad} of {} triangles face inward",
                mesh.indices.len() / 3
            );
        }
        // The shelter's roof is 2.4–2.6 m up and the lamp's light ~5.8 m.
        assert!((2.4..2.6).contains(&shelter_roof_y(0.0)));
        assert!((5.5..6.2).contains(&lamp.light.y), "{}", lamp.light);
    }
}
