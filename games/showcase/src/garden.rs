//! Kitchen gardens behind the village's houses: a wattle fence round a plot of bare earth,
//! raised beds edged with boards, and rows of what a cottage garden grows (colewort, leeks,
//! onions, lettuce, beans on poles, a few marigolds). Medieval and early-modern gardens were
//! always enclosed, often by a wattle fence of branches woven between stakes, and grew mostly
//! vegetables (leeks, onions, peas, beans, colewort) with a few herbs and flowers
//! (https://thegardenstrust.org/history-hub/medieval-gardens/).
//!
//! Each garden is four meshes built for its own spot (they follow the ground): the earth, the
//! beds, the fence and the plants. Everything is from code, vertex colored like the rest of the
//! village's props.

use crate::landscape::Landscape;
use crate::meshgen::Builder;
use engine::core::math::Rng;
use engine::prelude::*;
use std::f32::consts::TAU;

/// `--gardens off`: the village without its kitchen gardens (for before-and-after pictures).
#[derive(Resource, Clone, Copy)]
pub struct NoGardens;

/// A garden plot: its middle, the axis along the house's back wall and the one pointing away
/// from the house (the gate is on the house side), half its size along those, and a seed.
#[derive(Clone, Copy, Debug)]
pub struct Garden {
    pub center: DVec2,
    pub out: DVec2,
    pub half: DVec2,
    pub seed: u64,
}

impl Garden {
    /// Local +X: along the house's back wall (so that local +Z is [`Garden::out`] under a turn
    /// about Y, as the houses' transforms do).
    pub fn along(&self) -> DVec2 {
        DVec2::new(self.out.y, -self.out.x)
    }

    /// The turn about Y that takes local axes to the garden's.
    pub fn yaw(&self) -> f32 {
        self.out.x.atan2(self.out.y) as f32
    }

    /// World (x, z) of a local point.
    pub fn world(&self, x: f64, z: f64) -> DVec2 {
        self.center + self.along() * x + self.out * z
    }

    /// Distance from `p` to the plot, fence included (0 inside).
    pub fn distance(&self, p: DVec2) -> f64 {
        let d = p - self.center;
        let q = DVec2::new(d.dot(self.along()).abs(), d.dot(self.out).abs())
            - (self.half + DVec2::splat(FENCE_OUT));
        q.max(DVec2::ZERO).length()
    }

    /// Corners of the plot with the fence, grown by `grow` (m).
    pub fn corners(&self, grow: f64) -> [DVec2; 4] {
        let h = self.half + DVec2::splat(FENCE_OUT + grow);
        [
            self.world(h.x, h.y),
            self.world(-h.x, h.y),
            self.world(-h.x, -h.y),
            self.world(h.x, -h.y),
        ]
    }
}

/// How far the fence stands outside the plot's earth, m.
const FENCE_OUT: f64 = 0.15;
/// Width of the gate in the fence on the house side, m.
const GATE: f64 = 1.0;

/// A garden's meshes, in its local frame (origin at the plot's middle on the ground).
pub struct GardenMeshes {
    pub earth: Mesh,
    pub beds: Mesh,
    pub fence: Mesh,
    pub plants: Mesh,
}

/// Height of the ground at a local point, relative to the garden's origin.
struct Ground<'a> {
    g: &'a Garden,
    land: &'a Landscape,
    base: f64,
}

impl Ground<'_> {
    fn y(&self, x: f32, z: f32) -> f32 {
        let p = self.g.world(x as f64, z as f64);
        (self.land.height(p.x, p.y) - self.base) as f32
    }
}

/// Ground height at the garden's middle (its entity's origin).
pub fn base_height(g: &Garden, land: &Landscape) -> f64 {
    land.height(g.center.x, g.center.y)
}

/// Build the garden's meshes for where it stands.
pub fn meshes(g: &Garden, land: &Landscape) -> GardenMeshes {
    let ground = Ground {
        g,
        land,
        base: base_height(g, land),
    };
    let mut rng = Rng::new(g.seed);
    let (hx, hz) = (g.half.x as f32, g.half.y as f32);
    let mut earth = Builder::default();
    sheet(&mut earth, &ground, [-hx, hx], [-hz, hz], 0.025, |_, _| 0.0);
    let mut beds = Builder::default();
    let mut plants = Builder::default();
    // Beds run away from the house with paths between them and a cross path by the gate.
    let (bed_w, path) = (1.2f32, 0.5f32);
    let n = (((2.0 * hx - path) / (bed_w + path)).floor() as i32).max(1);
    let used = n as f32 * bed_w + (n - 1) as f32 * path;
    let (z0, z1) = (-hz + 0.8, hz - 0.35);
    for i in 0..n {
        let cx = -used * 0.5 + bed_w * 0.5 + i as f32 * (bed_w + path);
        let (x0, x1) = (cx - bed_w * 0.5, cx + bed_w * 0.5);
        bed(&mut beds, &mut earth, &ground, [x0, x1], [z0, z1]);
        let crop = CROPS[rng.range_i32(0, CROPS.len() as i32) as usize];
        sow(&mut plants, &ground, &mut rng, crop, [x0, x1], [z0, z1]);
    }
    let mut fence = Builder::default();
    wattle(
        &mut fence,
        &ground,
        &mut rng,
        hx + FENCE_OUT as f32,
        hz + FENCE_OUT as f32,
    );
    GardenMeshes {
        earth: earth.finish(),
        beds: beds.finish(),
        fence: fence.finish(),
        plants: plants.finish(),
    }
}

/// A grid over [x0, x1] × [z0, z1] following the ground, `lift` above it plus `bump(u, v)`
/// (u, v in 0..1 across the grid), with texture coordinates in meters.
fn sheet(
    b: &mut Builder,
    ground: &Ground,
    xs: [f32; 2],
    zs: [f32; 2],
    lift: f32,
    bump: impl Fn(f32, f32) -> f32,
) {
    let cell = 0.3;
    let nx = ((xs[1] - xs[0]) / cell).ceil().max(1.0) as u32;
    let nz = ((zs[1] - zs[0]) / cell).ceil().max(1.0) as u32;
    let at = |i: u32, j: u32| {
        let (u, v) = (i as f32 / nx as f32, j as f32 / nz as f32);
        let (x, z) = (xs[0] + (xs[1] - xs[0]) * u, zs[0] + (zs[1] - zs[0]) * v);
        Vec3::new(x, ground.y(x, z) + lift + bump(u, v), z)
    };
    let base = b.mesh.vertices.len() as u32;
    for j in 0..=nz {
        for i in 0..=nx {
            let p = at(i, j);
            // Normal from the neighbors (the ground's slope and the bump).
            let dx = at((i + 1).min(nx), j) - at(i.saturating_sub(1), j);
            let dz = at(i, (j + 1).min(nz)) - at(i, j.saturating_sub(1));
            let n = dz.cross(dx).normalize_or(Vec3::Y);
            b.mesh
                .vertices
                .push(engine::render::Vertex::new(p, n, Vec2::new(p.x, p.z)).with_color(Vec4::ONE));
        }
    }
    let row = nx + 1;
    for j in 0..nz {
        for i in 0..nx {
            let a = base + j * row + i;
            b.mesh
                .indices
                .extend_from_slice(&[a, a + row, a + 1, a + 1, a + row, a + row + 1]);
        }
    }
}

/// A raised bed: boards round [x0, x1] × [z0, z1] and a crowned top of earth inside them.
fn bed(beds: &mut Builder, earth: &mut Builder, ground: &Ground, xs: [f32; 2], zs: [f32; 2]) {
    let (t, top) = (0.04f32, 0.2f32);
    let board = Vec3::new(0.95, 0.9, 0.82);
    let corners = [
        Vec2::new(xs[0], zs[0]),
        Vec2::new(xs[1], zs[0]),
        Vec2::new(xs[1], zs[1]),
        Vec2::new(xs[0], zs[1]),
    ];
    for k in 0..4 {
        let (a, c) = (corners[k], corners[(k + 1) % 4]);
        board_strip(beds, ground, a, c, t, top, board);
    }
    sheet(
        earth,
        ground,
        [xs[0] + t, xs[1] - t],
        [zs[0] + t, zs[1] - t],
        top - 0.05,
        |u, v| 0.05 * (1.0 - (2.0 * u - 1.0).powi(2)) * (1.0 - (2.0 * v - 1.0).powi(8)),
    );
}

/// A board standing on edge from `a` to `b` (outer face to the left of a→b), `t` thick, its
/// top `top` above the ground, following the ground in short pieces.
fn board_strip(b: &mut Builder, ground: &Ground, a: Vec2, c: Vec2, t: f32, top: f32, col: Vec3) {
    let len = a.distance(c);
    let dir = (c - a) / len;
    let inward = Vec2::new(-dir.y, dir.x);
    let pieces = (len / 0.6).ceil().max(1.0) as u32;
    let p3 = |q: Vec2, y: f32| Vec3::new(q.x, y, q.y);
    for k in 0..pieces {
        let (q0, q1) = (
            a + dir * (len * k as f32 / pieces as f32),
            a + dir * (len * (k + 1) as f32 / pieces as f32),
        );
        let (g0, g1) = (ground.y(q0.x, q0.y), ground.y(q1.x, q1.y));
        let (i0, i1) = (q0 + inward * t, q1 + inward * t);
        // Outer face, top, inner face (down into the ground a little), each wound to face
        // out of the board.
        b.quad(
            [
                p3(q1, g1 - 0.05),
                p3(q0, g0 - 0.05),
                p3(q0, g0 + top),
                p3(q1, g1 + top),
            ],
            col,
        );
        b.quad(
            [
                p3(i0, g0 + top),
                p3(i1, g1 + top),
                p3(q1, g1 + top),
                p3(q0, g0 + top),
            ],
            col * 1.05,
        );
        b.quad(
            [
                p3(i0, g0 - 0.05),
                p3(i1, g1 - 0.05),
                p3(i1, g1 + top),
                p3(i0, g0 + top),
            ],
            col * 0.8,
        );
    }
}

#[derive(Clone, Copy, PartialEq, Debug)]
enum Crop {
    Colewort,
    Leeks,
    Onions,
    Lettuce,
    Beans,
    Marigolds,
}

/// What the beds grow, picked per bed (the common ones listed more than once).
const CROPS: [Crop; 9] = [
    Crop::Colewort,
    Crop::Colewort,
    Crop::Leeks,
    Crop::Onions,
    Crop::Lettuce,
    Crop::Lettuce,
    Crop::Beans,
    Crop::Beans,
    Crop::Marigolds,
];

/// Plant a bed's rows.
fn sow(b: &mut Builder, ground: &Ground, rng: &mut Rng, crop: Crop, xs: [f32; 2], zs: [f32; 2]) {
    let (rows, spacing): (&[f32], f32) = match crop {
        Crop::Colewort => (&[-0.28, 0.28], 0.5),
        Crop::Leeks => (&[-0.39, -0.13, 0.13, 0.39], 0.2),
        Crop::Onions => (&[-0.4, -0.2, 0.0, 0.2, 0.4], 0.14),
        Crop::Lettuce => (&[-0.35, 0.0, 0.35], 0.32),
        Crop::Beans => (&[0.0], 1.25),
        Crop::Marigolds => (&[-0.3, 0.3], 0.4),
    };
    let cx = (xs[0] + xs[1]) * 0.5;
    let len = zs[1] - zs[0] - 0.2;
    let count = (len / spacing).floor().max(1.0) as u32;
    let start = zs[0] + 0.1 + (len - (count - 1) as f32 * spacing) * 0.5;
    // The bed's earth top (see `bed`), crown included.
    let soil = |x: f32, z: f32| {
        let u = (x - xs[0]) / (xs[1] - xs[0]);
        ground.y(x, z) + 0.15 + 0.05 * (1.0 - (2.0 * u - 1.0).powi(2))
    };
    for &dx in rows {
        for k in 0..count {
            // A few gaps where something was eaten or already picked.
            if crop != Crop::Beans && rng.chance(0.07) {
                continue;
            }
            let x = cx + dx + rng.range(-0.03, 0.03);
            let z = start + k as f32 * spacing + rng.range(-0.03, 0.03);
            let at = Vec3::new(x, soil(x, z) - 0.01, z);
            let size = rng.range(0.8, 1.15);
            match crop {
                Crop::Colewort => colewort(b, rng, at, size),
                Crop::Leeks => leek(b, rng, at, size),
                Crop::Onions => onion(b, rng, at, size),
                Crop::Lettuce => lettuce(b, rng, at, size),
                Crop::Beans => beans(b, rng, at, size),
                Crop::Marigolds => marigold(b, rng, at, size),
            }
        }
    }
}

/// Both sides of a leaf: a strip from `base` along `dir` (horizontal heading), rising by
/// `lift` and curling down at the tip by `droop`, `width` at its widest. Colored `col`, paler
/// at the midrib.
#[allow(clippy::too_many_arguments)]
fn leaf(
    b: &mut Builder,
    base: Vec3,
    heading: f32,
    length: f32,
    width: f32,
    lift: f32,
    droop: f32,
    col: Vec3,
) {
    let dir = Vec3::new(heading.cos(), 0.0, heading.sin());
    let side = Vec3::new(-dir.z, 0.0, dir.x);
    let segs = 3;
    let point = |t: f32| base + dir * (length * t) + Vec3::Y * (lift * t - droop * t * t);
    let half = |t: f32| width * 0.5 * (std::f32::consts::PI * t.powf(0.8)).sin().max(0.08);
    for s in 0..segs {
        let (t0, t1) = (s as f32 / segs as f32, (s + 1) as f32 / segs as f32);
        let (p0, p1) = (point(t0), point(t1));
        let (w0, w1) = (half(t0), half(t1));
        // Cupped: the edges a little higher than the midrib.
        let cup = Vec3::Y * 0.25;
        let quad = [
            p0 - side * w0 + cup * w0,
            p1 - side * w1 + cup * w1,
            p1 + side * w1 + cup * w1,
            p0 + side * w0 + cup * w0,
        ];
        let c = col * (0.9 + 0.2 * t1);
        b.quad(quad, c);
        b.quad([quad[3], quad[2], quad[1], quad[0]], c * 0.85);
    }
}

/// Colewort (kale): a rosette of broad blue-green leaves round a short stem.
fn colewort(b: &mut Builder, rng: &mut Rng, at: Vec3, size: f32) {
    let col = Vec3::new(0.06, 0.11, 0.075);
    let turn = rng.range(0.0, TAU);
    for ring in 0..2 {
        let leaves = 6 + ring;
        for i in 0..leaves {
            let h = turn + (i as f32 + ring as f32 * 0.5) / leaves as f32 * TAU;
            let (len, lift) = if ring == 0 { (0.3, 0.16) } else { (0.2, 0.22) };
            leaf(
                b,
                at + Vec3::Y * (0.05 + 0.04 * ring as f32) * size,
                h + rng.range(-0.15, 0.15),
                len * size,
                0.17 * size,
                lift * size,
                0.12 * size,
                col * rng.range(0.85, 1.15),
            );
        }
    }
}

/// Lettuce: a low, bright green ball of frilled leaves.
fn lettuce(b: &mut Builder, rng: &mut Rng, at: Vec3, size: f32) {
    let col = Vec3::new(0.12, 0.25, 0.04);
    let turn = rng.range(0.0, TAU);
    for ring in 0..3 {
        let leaves = 7 - ring;
        for i in 0..leaves {
            let h = turn + (i as f32 + ring as f32 * 0.4) / leaves as f32 * TAU;
            let r = ring as f32;
            leaf(
                b,
                at + Vec3::Y * 0.02,
                h,
                (0.15 - 0.03 * r) * size,
                (0.13 - 0.02 * r) * size,
                (0.06 + 0.07 * r) * size,
                0.03 * size,
                col * (1.0 + 0.15 * r),
            );
        }
    }
}

/// A leek: a pale shank and a fan of flat, blue-green blades.
fn leek(b: &mut Builder, rng: &mut Rng, at: Vec3, size: f32) {
    let shank = 0.14 * size;
    b.tube(
        at,
        at + Vec3::Y * shank,
        0.018 * size,
        0.015 * size,
        5,
        Vec3::new(0.45, 0.48, 0.38),
        Vec3::new(0.2, 0.28, 0.18),
        true,
    );
    let fan = rng.range(0.0, TAU);
    for i in 0..5 {
        let h =
            fan + if i % 2 == 0 {
                0.0
            } else {
                std::f32::consts::PI
            } + rng.range(-0.3, 0.3);
        leaf(
            b,
            at + Vec3::Y * (shank - 0.01),
            h,
            (0.22 + 0.04 * i as f32) * size,
            0.04 * size,
            (0.35 - 0.03 * i as f32) * size,
            0.18 * size,
            Vec3::new(0.07, 0.12, 0.09),
        );
    }
}

/// An onion: a bulb half out of the earth and a few thin hollow leaves.
fn onion(b: &mut Builder, rng: &mut Rng, at: Vec3, size: f32) {
    let bulb = Vec3::new(0.42, 0.3, 0.16);
    b.tube(
        at - Vec3::Y * 0.01,
        at + Vec3::Y * 0.05 * size,
        0.035 * size,
        0.012 * size,
        6,
        bulb,
        bulb * 0.8,
        true,
    );
    for i in 0..4 {
        let h = rng.range(0.0, TAU);
        let tip = at
            + Vec3::new(h.cos(), 0.0, h.sin()) * (0.08 * size)
            + Vec3::Y * ((0.28 + 0.03 * i as f32) * size);
        b.tube(
            at + Vec3::Y * 0.05 * size,
            tip,
            0.006,
            0.002,
            3,
            Vec3::new(0.08, 0.15, 0.07),
            Vec3::new(0.12, 0.17, 0.08),
            false,
        );
    }
}

/// Runner beans up a wigwam of four poles, as cottage gardens grew them: each pole carries a
/// twining vine thick with three-leaflet leaves, and scarlet flower sprays among them.
fn beans(b: &mut Builder, rng: &mut Rng, at: Vec3, size: f32) {
    let top = at + Vec3::Y * 1.9 * size;
    let pole = Vec3::new(0.3, 0.22, 0.14);
    let vine = Vec3::new(0.12, 0.2, 0.06);
    let leaf_col = Vec3::new(0.06, 0.14, 0.035);
    let scarlet = Vec3::new(0.62, 0.05, 0.03);
    for i in 0..4 {
        let a = i as f32 / 4.0 * TAU + std::f32::consts::FRAC_PI_4;
        let foot = at + Vec3::new(a.cos(), 0.0, a.sin()) * 0.42;
        b.tube(
            foot - Vec3::Y * 0.1,
            top + Vec3::Y * 0.12,
            0.016,
            0.012,
            4,
            pole,
            pole * 1.1,
            true,
        );
        // The vine winds round the pole, about three turns from the ground to near the top.
        let reach = rng.range(0.78, 0.92);
        let phase = rng.range(0.0, TAU);
        let wind = |t: f32| {
            let w = phase + t * 3.0 * TAU;
            foot.lerp(top, t * reach) + Vec3::new(w.cos(), 0.0, w.sin()) * 0.03
        };
        let segs = 12;
        for k in 0..segs {
            let (t0, t1) = (k as f32 / segs as f32, (k + 1) as f32 / segs as f32);
            b.tube(wind(t0), wind(t1), 0.006, 0.005, 3, vine, vine, false);
        }
        // Leaves at the vine's nodes, a little sparser toward the top.
        let n = 14;
        for k in 0..n {
            let t = (k as f32 + rng.range(0.2, 0.8)) / n as f32;
            if rng.chance(0.05 + 0.3 * t) {
                continue;
            }
            let p = wind(t);
            // Face outward from the wigwam, give or take.
            let out = (p - at) * Vec3::new(1.0, 0.0, 1.0);
            let h = out.z.atan2(out.x) + rng.range(-1.2, 1.2);
            let col = leaf_col * rng.range(0.85, 1.2);
            let l = rng.range(0.09, 0.12) * size;
            for (dh, scale) in [(0.0, 1.0), (-0.95, 0.8), (0.95, 0.8)] {
                leaf(b, p, h + dh, l * scale, l * 0.75 * scale, 0.02, 0.05, col);
            }
            // A spray of scarlet flowers on a short stalk now and then.
            if rng.chance(0.22) {
                let dir = Vec3::new((h + 0.5).cos(), 0.0, (h + 0.5).sin());
                for f in 0..4 {
                    let q = p + dir * (0.05 + 0.025 * f as f32) + Vec3::Y * (0.02 * f as f32);
                    b.tube(
                        q,
                        q + Vec3::Y * 0.02,
                        0.011,
                        0.004,
                        4,
                        scarlet,
                        scarlet * 1.15,
                        true,
                    );
                }
            }
        }
    }
}

/// A marigold: a low bushy clump of leaves under orange flower heads.
fn marigold(b: &mut Builder, rng: &mut Rng, at: Vec3, size: f32) {
    for i in 0..8 {
        let h = i as f32 / 8.0 * TAU + rng.range(-0.2, 0.2);
        leaf(
            b,
            at,
            h,
            0.13 * size,
            0.06 * size,
            0.14 * size,
            0.04 * size,
            Vec3::new(0.05, 0.1, 0.03),
        );
    }
    for _ in 0..rng.range_i32(3, 6) {
        let d = rng.in_unit_disk() * 0.08;
        let f = at + Vec3::new(d.x, rng.range(0.16, 0.22) * size, d.y);
        let c = if rng.chance(0.6) {
            Vec3::new(0.85, 0.3, 0.02)
        } else {
            Vec3::new(0.85, 0.55, 0.03)
        };
        b.tube(
            f - Vec3::Y * 0.01,
            f + Vec3::Y * 0.02,
            0.03,
            0.012,
            6,
            c * 0.8,
            c,
            true,
        );
    }
}

/// A wattle fence round the rectangle ±hx × ±hz: stakes about every half meter and hazel rods
/// woven in and out between them, a gate gap in the middle of the house side (−Z).
fn wattle(b: &mut Builder, ground: &Ground, rng: &mut Rng, hx: f32, hz: f32) {
    let corners = [
        Vec2::new(-hx, -hz),
        Vec2::new(hx, -hz),
        Vec2::new(hx, hz),
        Vec2::new(-hx, hz),
    ];
    let stake = Vec3::new(0.6, 0.53, 0.46);
    let rod = Vec3::new(0.78, 0.72, 0.64);
    let gate = GATE as f32 * 0.5;
    for k in 0..4 {
        let (a, c) = (corners[k], corners[(k + 1) % 4]);
        let len = a.distance(c);
        let dir = (c - a) / len;
        let outward = Vec2::new(dir.y, -dir.x);
        // The house side is split round the gate.
        let runs: Vec<(Vec2, Vec2)> = if k == 0 {
            vec![(a, Vec2::new(-gate, -hz)), (Vec2::new(gate, -hz), c)]
        } else {
            vec![(a, c)]
        };
        for (r0, r1) in runs {
            let len = r0.distance(r1);
            let gaps = (len / 0.5).round().max(1.0) as u32;
            let stakes: Vec<Vec2> = (0..=gaps)
                .map(|i| r0.lerp(r1, i as f32 / gaps as f32))
                .collect();
            for (i, s) in stakes.iter().enumerate() {
                let end = i == 0 || i == stakes.len() - 1;
                let g = ground.y(s.x, s.y);
                let h = if end { 1.05 } else { rng.range(0.92, 1.02) };
                let r = if end { 0.035 } else { 0.022 };
                b.tube(
                    Vec3::new(s.x, g - 0.1, s.y),
                    Vec3::new(s.x, g + h, s.y)
                        + Vec3::new(rng.range(-0.02, 0.02), 0.0, rng.range(-0.02, 0.02)),
                    r,
                    r * 0.8,
                    5,
                    stake * rng.range(0.85, 1.1),
                    stake,
                    true,
                );
            }
            // Rods woven between: each level passes in front of one stake and behind the next,
            // the level above the other way round.
            // Close together, as woven rods lie, so the fence reads solid with only chinks of
            // light between them.
            let levels = 16;
            for l in 0..levels {
                let y = 0.1 + l as f32 * 0.048;
                let tint = rod * rng.range(0.8, 1.15);
                for (i, w) in stakes.windows(2).enumerate() {
                    let side = |j: usize| {
                        if (j + l).is_multiple_of(2) {
                            0.028
                        } else {
                            -0.028
                        }
                    };
                    let p = |q: Vec2, off: f32| {
                        let q = q + outward * off;
                        Vec3::new(q.x, ground.y(q.x, q.y) + y, q.y)
                    };
                    let mid = (w[0] + w[1]) * 0.5;
                    let (a3, m3, c3) = (p(w[0], side(i)), p(mid, 0.0), p(w[1], side(i + 1)));
                    b.tube(a3, m3, 0.022, 0.022, 4, tint, tint, false);
                    b.tube(m3, c3, 0.022, 0.022, 4, tint, tint, false);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn local_axes_match_the_yaw() {
        for out in [DVec2::X, DVec2::Y, DVec2::new(-0.6, 0.8)] {
            let g = Garden {
                center: DVec2::ZERO,
                out,
                half: DVec2::new(3.0, 2.0),
                seed: 1,
            };
            let q = Quat::from_rotation_y(g.yaw());
            let x = q * Vec3::X;
            let z = q * Vec3::Z;
            assert!((DVec2::new(x.x as f64, x.z as f64) - g.along()).length() < 1e-5);
            assert!((DVec2::new(z.x as f64, z.z as f64) - g.out).length() < 1e-5);
        }
    }

    #[test]
    fn a_garden_builds_on_flat_ground() {
        let land = crate::landscape::shared();
        let g = Garden {
            center: DVec2::new(0.0, 70.0),
            out: DVec2::Y,
            half: DVec2::new(3.5, 3.0),
            seed: 7,
        };
        let m = meshes(&g, land);
        for mesh in [&m.earth, &m.beds, &m.fence, &m.plants] {
            assert!(!mesh.indices.is_empty());
            assert!(
                mesh.indices
                    .iter()
                    .all(|&i| (i as usize) < mesh.vertices.len())
            );
            assert!(
                mesh.vertices
                    .iter()
                    .all(|v| v.position.iter().all(|c| c.is_finite()))
            );
        }
        let tris: usize = [&m.earth, &m.beds, &m.fence, &m.plants]
            .iter()
            .map(|m| m.indices.len() / 3)
            .sum();
        assert!(tris < 60_000, "{tris} triangles");
    }
}
