//! Rope-bridge props: timber towers (the long bridge's platforms, with stairs, railings and
//! tie-off posts), sandstone piers (the cut bridge's pillars, with iron rungs), and the
//! bridges' planks.

use std::f32::consts::FRAC_PI_4;

use engine_core::prelude::*;
use engine_render::Mesh;
use engine_world::physics::ColliderShape;

use super::kit::{Mat, Model, ModelBuilder, cuboid as box_shape, cylinder, part, part_rot};
use super::meshkit::*;
use super::props::*;
use crate::rope::RopeBridge;

/// Size of one plank of `spec`, as the bridge builder makes them (x along the span, y
/// thickness, z across).
pub(super) fn plank_size(spec: &RopeBridge) -> Vec3 {
    let planks = spec.planks.clamp(2, 400) as f32;
    let spacing = spec.length.max(1.0) / (planks + 1.0);
    let depth = (spacing - spec.plank_gap).max(0.05);
    let half_w = spec.width.max(0.2) * 0.5;
    Vec3::new(depth, spec.plank_thickness, 2.0 * (half_w + 0.05))
}

/// A bridge plank as a unit-cube mesh (the bridge stretches it to `size`): three weathered
/// boards across the span with gaps, on two battens. Modeled at full size, then scaled into
/// the unit cube with normals pre-compensated so they come out right once stretched.
pub(super) fn bridge_plank(size: Vec3, seed: u32) -> Mesh {
    let n = 3;
    let gap = 0.014;
    let bw = (size.x - gap * (n - 1) as f32) / n as f32;
    let mut m = Mesh::default();
    for k in 0..n {
        let x = -size.x * 0.5 + bw * 0.5 + k as f32 * (bw + gap);
        let len = size.z - 0.02 * rnd(seed, k);
        let shift = (rnd(seed, k + 10) - 0.5) * 0.02;
        m.merge(&board_at(
            Vec3::new(x, size.y * 0.12, shift),
            Vec3::new(bw, size.y * 0.76, len),
            seed * 7 + k,
        ));
    }
    for (k, z) in [-1.0f32, 1.0].into_iter().enumerate() {
        m.merge(&board_at(
            Vec3::new(0.0, -size.y * 0.38, z * (size.z * 0.5 - 0.17)),
            Vec3::new(size.x * 0.96, size.y * 0.24, 0.07),
            seed * 7 + 5 + k as u32,
        ));
    }
    m.transform(&Mat4::from_scale(Vec3::ONE / size));
    m
}

/// A timber tower 3 m tall with a 3×3 m deck: concrete footings, posts, rim beams, girts,
/// zigzag braces, joists, deck boards, railings, a stair down the +Z side, and two round
/// tie-off posts with lashings and ring bolts at the bridge edge. The bridge leaves toward
/// `side` × +X (±1) at deck height; `half_w` is half the bridge's deck width.
pub(super) fn timber_tower(
    commands: &mut Commands,
    side: f32,
    half_w: f32,
) -> (Model, ColliderShape) {
    let s = side;
    let name = if s > 0.0 { "tower_east" } else { "tower_west" };
    let mut mb = ModelBuilder::new(name).grounded();
    let seed0 = if s > 0.0 { 400 } else { 600 };
    let mut seed = seed0;
    let mut next = || {
        seed += 1;
        seed
    };
    let w = Mat::Timber;
    let deck = 3.0f32;
    let c = 1.35f32;
    let corners = [(-c, -c), (c, -c), (c, c), (-c, c)];
    let mut colliders = Vec::new();
    for &(x, z) in &corners {
        mb.add(
            Mat::Concrete,
            place(
                rounded_box(Vec3::new(0.5, 0.2, 0.5), 0.025, 2, 0.5, BoxUv::PLANAR),
                Vec3::new(x, 0.05, z),
                Quat::IDENTITY,
            ),
        );
        mb.add(
            Mat::Galvanized,
            place(
                rounded_box(Vec3::new(0.3, 0.01, 0.3), 0.003, 1, 0.5, BoxUv::PLANAR),
                Vec3::new(x, 0.155, z),
                Quat::IDENTITY,
            ),
        );
        mb.add(
            w,
            board_at(
                Vec3::new(x, 0.16 + 1.945, z),
                Vec3::new(0.25, 3.89, 0.25),
                next(),
            ),
        );
        colliders.push(part(
            Vec3::new(x, 2.1, z),
            box_shape(Vec3::new(0.25, 3.9, 0.25)),
        ));
        // Bolts where the rims and girts meet the posts.
        let out = Vec3::new(x.signum(), 0.0, z.signum());
        for y in [1.4f32, 1.5, 2.74, 2.88] {
            mb.add_small(
                Mat::BlackSteel,
                bolt_heads(
                    &[
                        Vec3::new(x + out.x * 0.125, y, z),
                        Vec3::new(x, y, z + out.z * 0.125),
                    ],
                    Vec3::X * out.x,
                    0.014,
                ),
            );
        }
    }
    // Rims (under the deck) and girts (mid height) on all four sides.
    let span = 2.0 * c - 0.25;
    for (y, h) in [(2.81f32, 0.28f32), (1.45, 0.2)] {
        for z in [-c, c] {
            mb.add(
                w,
                board_at(Vec3::new(0.0, y, z), Vec3::new(span, h, 0.1), next()),
            );
        }
        for x in [-c, c] {
            mb.add(
                w,
                board_at(Vec3::new(x, y, 0.0), Vec3::new(0.1, h, span), next()),
            );
        }
    }
    // Zigzag braces on the outside of every face.
    let o = 1.475 + 0.028;
    let faces: [(Vec3, Vec3, Vec3); 4] = [
        (Vec3::new(-c, 0.0, o), Vec3::new(c, 0.0, o), Vec3::X),
        (Vec3::new(c, 0.0, -o), Vec3::new(-c, 0.0, -o), Vec3::X),
        (Vec3::new(o, 0.0, c), Vec3::new(o, 0.0, -c), Vec3::Z),
        (Vec3::new(-o, 0.0, -c), Vec3::new(-o, 0.0, c), Vec3::Z),
    ];
    for (a, b, along) in faces {
        mb.add(
            w,
            beam(
                a + Vec3::Y * 0.3,
                b + Vec3::Y * 1.38,
                0.17,
                0.05,
                along,
                next(),
            ),
        );
        mb.add(
            w,
            beam(
                b + Vec3::Y * 1.55,
                a + Vec3::Y * 2.62,
                0.17,
                0.05,
                along,
                next(),
            ),
        );
        let normal = Vec3::new(
            a.x.signum() * (along.z.abs()),
            0.0,
            a.z.signum() * along.x.abs(),
        );
        let heads = [
            a + Vec3::Y * 0.4,
            b + Vec3::Y * 1.3,
            b + Vec3::Y * 1.63,
            a + Vec3::Y * 2.55,
        ];
        let heads: Vec<Vec3> = heads
            .iter()
            .map(|&p| p + normal * 0.026 - along * along.dot(p).signum() * 0.03)
            .collect();
        mb.add_small(Mat::BlackSteel, bolt_heads(&heads, normal, 0.013));
    }
    // Joists and deck boards (top at the deck height).
    for z in [-0.8f32, -0.27, 0.27, 0.8] {
        mb.add(
            w,
            board_at(Vec3::new(0.0, 2.845, z), Vec3::new(span, 0.2, 0.08), next()),
        );
    }
    let boards = 13;
    let (bw, gap) = (0.215f32, 0.0154f32);
    for k in 0..boards {
        let x = -1.49 + bw * 0.5 + k as f32 * (bw + gap);
        let len = 3.0 - 0.02 * rnd(seed0, 50 + k);
        mb.add(
            Mat::Pine,
            board_at(
                Vec3::new(x, deck - 0.0225, 0.0),
                Vec3::new(bw, 0.045, len),
                next(),
            ),
        );
    }
    colliders.push(part(
        Vec3::new(0.0, 2.835, 0.0),
        box_shape(Vec3::new(3.0, 0.33, 3.0)),
    ));
    // Railing: corner posts run up to 4.05; mid posts, top rails and mid rails.
    let rail_y = 4.0725;
    let back = -s * c;
    let front = s * c;
    let posts = [
        Vec3::new(0.0, 0.0, -c),
        Vec3::new(0.0, 0.0, c),
        Vec3::new(back, 0.0, 0.0),
    ];
    for p in &posts {
        mb.add(
            w,
            board_at(
                Vec3::new(p.x, deck + 0.525, p.z),
                Vec3::new(0.1, 1.05, 0.1),
                next(),
            ),
        );
    }
    let rails: [(Vec3, Vec3); 5] = [
        (Vec3::new(back, 0.0, 0.0), Vec3::new(0.14, 0.045, 3.05)),
        (Vec3::new(0.0, 0.0, -c), Vec3::new(3.05, 0.045, 0.14)),
        (Vec3::new(s * 0.74, 0.0, c), Vec3::new(1.55, 0.045, 0.14)),
        (Vec3::new(front, 0.0, -1.21), Vec3::new(0.14, 0.045, 0.55)),
        (Vec3::new(front, 0.0, 1.21), Vec3::new(0.14, 0.045, 0.55)),
    ];
    for (at, size) in rails {
        mb.add(w, board_at(at + Vec3::Y * rail_y, size, next()));
        let msize = if size.x > size.z {
            Vec3::new(size.x - 0.1, 0.1, 0.04)
        } else {
            Vec3::new(0.04, 0.1, size.z - 0.1)
        };
        mb.add(w, board_at(at + Vec3::Y * 3.55, msize, next()));
    }
    // A galvanized chain across the stair opening, hooked to eye bolts on the posts.
    let chain_y = deck + 0.82;
    let ends = [
        (Vec3::new(back + s * 0.141, chain_y, c), s),
        (Vec3::new(-s * 0.066, chain_y, c), -s),
    ];
    let (a, b) = (ends[0].0, ends[1].0);
    let sag = 0.14;
    let links: Vec<Vec3> = (0..=16)
        .map(|i| {
            let t = i as f32 / 16.0;
            a.lerp(b, t) - Vec3::Y * (4.0 * sag * t * (1.0 - t))
        })
        .collect();
    mb.add_small(Mat::Galvanized, chain(&links, 0.045, 0.0042));
    for (p, toward) in ends {
        mb.add_small(
            Mat::Galvanized,
            place(
                torus(0.013, 0.0032, 12, 6),
                p,
                Quat::from_rotation_x(std::f32::consts::FRAC_PI_2),
            ),
        );
        mb.add_small(
            Mat::Galvanized,
            tube(
                &[p - Vec3::X * toward * 0.013, p - Vec3::X * toward * 0.05],
                |_| 0.0045,
                6,
                false,
                true,
                1.0,
            ),
        );
    }
    // Tie-off posts at the bridge edge, rail ropes lashed on, deck ropes on ring bolts.
    let edge = s * 1.5;
    for zs in [-1.0f32, 1.0] {
        let p = Vec3::new(s * 1.42, 0.0, zs * (half_w + 0.12));
        mb.add(
            w,
            turned_wood(
                &[
                    (0.0, 2.67),
                    (0.085, 2.67),
                    (0.085, 2.67),
                    (0.085, 4.2),
                    (0.075, 4.24),
                    (0.05, 4.265),
                    (0.0, 4.27),
                ],
                18,
                p,
                next(),
            ),
        );
        colliders.push(part(p + Vec3::Y * 3.47, cylinder(0.085, 1.6)));
        mb.add(
            Mat::Rope,
            lashing(
                p + Vec3::Y * 4.0,
                0.085,
                4,
                Some(Vec3::new(edge, 4.0, zs * (half_w + 0.05))),
            ),
        );
        let ring_at = Vec3::new(edge - s * 0.034, deck, zs * half_w);
        mb.add(Mat::BlackSteel, ring_bolt(ring_at, Vec3::X * s));
        mb.add_small(
            Mat::Rope,
            place(
                rope_ring(0.024, 0.011),
                ring_at + Vec3::new(s * 0.03, 0.035, 0.0),
                Quat::from_rotation_x(std::f32::consts::FRAC_PI_2),
            ),
        );
    }
    // Stair down the +Z side toward the bridge.
    let x0 = -s * c;
    let (rise, run, steps) = (0.2f32, 0.22f32, 15);
    let (z_in, z_out) = (1.62f32, 2.48f32);
    let zc = (z_in + z_out) * 0.5;
    for k in 1..steps {
        let y = deck - rise * k as f32;
        let x = x0 + s * (run * k as f32 - run * 0.5 + 0.015);
        mb.add(
            Mat::Pine,
            board_at(
                Vec3::new(x, y - 0.02, zc),
                Vec3::new(0.26, 0.04, z_out - z_in - 0.02),
                next(),
            ),
        );
    }
    let nose = |t: f32| {
        Vec3::new(
            x0 + s * run * steps as f32 * t,
            deck - rise * steps as f32 * t,
            0.0,
        )
    };
    for z in [z_in + 0.025, z_out - 0.025] {
        let a = nose(-0.03) + Vec3::new(0.0, -0.16, z);
        let b = nose(1.0) + Vec3::new(0.0, -0.16, z);
        mb.add(w, beam(a, b, 0.28, 0.05, Vec3::Y, next()));
    }
    mb.add(
        w,
        board_at(
            Vec3::new(x0 + s * 0.06, deck - 0.18, zc),
            Vec3::new(0.08, 0.28, z_out - z_in + 0.05),
            next(),
        ),
    );
    let foot = nose(1.0);
    mb.add(
        Mat::Concrete,
        place(
            rounded_box(Vec3::new(0.5, 0.12, 1.1), 0.02, 2, 0.5, BoxUv::PLANAR),
            Vec3::new(foot.x, 0.02, zc),
            Quat::IDENTITY,
        ),
    );
    // Stair handrail on the outer side.
    let hz = z_out + 0.04;
    for t in [0.08f32, 0.5, 0.92] {
        let p = nose(t);
        mb.add(
            w,
            board_at(
                Vec3::new(p.x, p.y + 0.47, hz),
                Vec3::new(0.08, 1.0, 0.08),
                next(),
            ),
        );
    }
    mb.add(
        w,
        beam(
            nose(0.02) + Vec3::new(0.0, 0.95, hz),
            nose(0.98) + Vec3::new(0.0, 0.95, hz),
            0.1,
            0.05,
            Vec3::Y,
            next(),
        ),
    );
    let a = nose(0.0) + Vec3::new(0.0, -0.1, zc);
    let b = nose(1.0) + Vec3::new(0.0, -0.1, zc);
    colliders.push(part_rot(
        (a + b) * 0.5,
        frame_rot(b - a, Vec3::Y),
        box_shape(Vec3::new(0.3, a.distance(b), z_out - z_in)),
    ));
    (mb.build(commands), ColliderShape::Compound(colliders))
}

/// Half width of the stone pier at height `y` (it steps in course by course).
fn pier_half(y: f32) -> f32 {
    0.95 - 0.15 * (y / 3.8)
}

/// A sandstone pier 4 m tall (deck height): coursed blocks with staggered joints that step
/// in toward the top, lime mortar behind them, a two-slab cap, round timber tie-off posts in
/// iron shoes for the hand ropes, ring bolts for the deck ropes and iron rungs up the back.
/// The bridge leaves toward `side` × +X.
pub(super) fn stone_pier(
    commands: &mut Commands,
    side: f32,
    half_w: f32,
    seed: u32,
) -> (Model, ColliderShape) {
    let s = side;
    let mut mb = ModelBuilder::new(format!("pier_{seed}")).grounded();
    let top = 3.8f32;
    // Mortar core (a square frustum).
    let core = |r: f32| r * std::f32::consts::SQRT_2;
    let mut mortar = lathe(
        &[
            Vec2::new(0.0, 0.0),
            Vec2::new(core(pier_half(0.0) - 0.03), 0.0),
            Vec2::new(core(pier_half(0.0) - 0.03), 0.0),
            Vec2::new(core(pier_half(top) - 0.03), top),
            Vec2::new(core(pier_half(top) - 0.03), top),
            Vec2::new(0.0, top),
        ],
        4,
        LatheUv::meters(1.0),
    );
    mortar.transform(&Mat4::from_rotation_y(FRAC_PI_4));
    mortar.compute_flat_normals();
    mb.add(Mat::Mortar, mortar);
    // Courses.
    let mut y = 0.0f32;
    let mut course = 0u32;
    let mut k = 0u32;
    while y < top - 0.05 {
        let mut h = 0.33 + 0.1 * rnd(seed, course * 31);
        if top - (y + h) < 0.25 {
            h = top - y;
        }
        let ym = y + h * 0.5;
        let hw = pier_half(ym);
        for face in 0..4u32 {
            let normal = [Vec3::X, Vec3::Z, Vec3::NEG_X, Vec3::NEG_Z][face as usize];
            let along = Vec3::new(-normal.z, 0.0, normal.x);
            let owns_corners = face.is_multiple_of(2) == course.is_multiple_of(2);
            let depth = 0.26;
            let half_run = if owns_corners { hw } else { hw - depth };
            let mut a = -half_run;
            while a < half_run - 0.01 {
                let r = |n: u32| rnd(seed, k * 13 + n);
                let mut len = 0.38 + 0.42 * r(0);
                if half_run - (a + len) < 0.28 {
                    len = half_run - a;
                }
                let gap = 0.012;
                let size = Vec3::new(len - gap, h - gap, depth + 0.04 * r(1));
                let inset = 0.008 * r(2);
                let center =
                    normal * (hw - size.z * 0.5 - inset) + along * (a + len * 0.5) + Vec3::Y * ym;
                let rot = Quat::from_rotation_arc(Vec3::Z, normal)
                    * Quat::from_rotation_y((r(3) - 0.5) * 0.012);
                let bevel = 0.02 + 0.02 * r(4);
                let block = rounded_box(
                    size,
                    bevel,
                    1,
                    0.5,
                    BoxUv::Planar {
                        offset: Vec2::new(r(5) * 3.0, r(6) * 3.0),
                    },
                );
                let block = jitter(block, 0.006, 4.0, seed + k);
                let shade = 0.84 + 0.22 * r(7);
                let warm = (r(8) - 0.5) * 0.1;
                let block = tint(
                    block,
                    Vec4::new(shade * (1.0 + warm), shade, shade * (1.0 - warm * 1.4), 1.0),
                );
                mb.add(Mat::Stone, place(block, center, rot));
                a += len;
                k += 1;
            }
        }
        y += h;
        course += 1;
    }
    // Cap: two slabs.
    for zs in [-1.0f32, 1.0] {
        let slab = rounded_box(
            Vec3::new(1.86, 0.2, 0.925),
            0.03,
            2,
            0.5,
            BoxUv::Planar {
                offset: Vec2::new(zs, 0.3),
            },
        );
        let slab = jitter(slab, 0.004, 3.0, seed + 90 + zs as u32);
        mb.add(
            Mat::Stone,
            place(slab, Vec3::new(0.0, top + 0.1, zs * 0.4675), Quat::IDENTITY),
        );
    }
    let deck = top + 0.2;
    // Tie-off posts in iron shoes, rail ropes lashed on; ring bolts for the deck ropes.
    let mut colliders = Vec::new();
    for zs in [-1.0f32, 1.0] {
        let p = Vec3::new(s * 0.66, deck, zs * (half_w + 0.12));
        mb.add(
            Mat::Timber,
            turned_wood(
                &[
                    (0.0, 0.0),
                    (0.08, 0.0),
                    (0.08, 0.0),
                    (0.08, 1.22),
                    (0.07, 1.26),
                    (0.045, 1.285),
                    (0.0, 1.29),
                ],
                18,
                p,
                seed + 200 + zs as u32,
            ),
        );
        colliders.push(part(p + Vec3::Y * 0.65, cylinder(0.08, 1.3)));
        mb.add(
            Mat::BlackSteel,
            turned(
                &[
                    (0.0, 0.0),
                    (0.12, 0.0),
                    (0.12, 0.01),
                    (0.1, 0.02),
                    (0.088, 0.02),
                    (0.088, 0.24),
                    (0.0, 0.24),
                ],
                18,
                p,
            ),
        );
        let heads: Vec<Vec3> = (0..4)
            .map(|i| {
                let a = i as f32 * std::f32::consts::FRAC_PI_2 + FRAC_PI_4;
                p + Vec3::new(a.cos() * 0.1, 0.012, a.sin() * 0.1)
            })
            .collect();
        mb.add_small(Mat::BlackSteel, nuts(&heads, Vec3::Y, 0.012));
        mb.add(
            Mat::Rope,
            lashing(
                p + Vec3::Y * 1.0,
                0.08,
                4,
                Some(Vec3::new(s * 0.8, deck + 1.0, zs * (half_w + 0.05))),
            ),
        );
        let ring_at = Vec3::new(s * 0.765, deck, zs * half_w);
        mb.add(Mat::BlackSteel, ring_bolt(ring_at, Vec3::X * s));
        mb.add_small(
            Mat::Rope,
            place(
                rope_ring(0.024, 0.011),
                ring_at + Vec3::new(s * 0.03, 0.035, 0.0),
                Quat::from_rotation_x(std::f32::consts::FRAC_PI_2),
            ),
        );
    }
    // Iron rungs up the back face, and grab handles over the cap.
    let mut rungs = Mesh::default();
    for i in 0..11 {
        let y = 0.45 + i as f32 * 0.3;
        let x = -s * pier_half(y);
        let path = smooth_path(
            &[
                Vec3::new(x + s * 0.05, y, -0.2),
                Vec3::new(x - s * 0.1, y, -0.2),
                Vec3::new(x - s * 0.14, y, -0.12),
                Vec3::new(x - s * 0.14, y, 0.12),
                Vec3::new(x - s * 0.1, y, 0.2),
                Vec3::new(x + s * 0.05, y, 0.2),
            ],
            4,
        );
        rungs.merge(&tube(&path, |_| 0.013, 8, false, true, 1.0));
    }
    for z in [-0.28f32, 0.28] {
        let path = smooth_path(
            &[
                Vec3::new(-s * 0.78, deck - 0.02, z),
                Vec3::new(-s * 0.8, deck + 0.75, z),
                Vec3::new(-s * 0.92, deck + 0.9, z),
                Vec3::new(-s * 1.04, deck + 0.75, z),
                Vec3::new(-s * 1.04, deck - 0.12, z),
                Vec3::new(-s * 0.94, deck - 0.14, z),
            ],
            5,
        );
        rungs.merge(&tube(&path, |_| 0.016, 8, false, true, 1.0));
    }
    mb.add(Mat::BlackSteel, rungs);
    let (b0, b1) = (pier_half(0.0), pier_half(top));
    let mut hull = Vec::new();
    for (y, h) in [(0.0f32, b0), (top, b1)] {
        for (x, z) in [(-1.0f32, -1.0f32), (1.0, -1.0), (1.0, 1.0), (-1.0, 1.0)] {
            hull.push(Vec3::new(x * h, y, z * h));
        }
    }
    colliders.push(part(Vec3::ZERO, ColliderShape::ConvexHull { points: hull }));
    colliders.push(part(
        Vec3::Y * (top + 0.1),
        box_shape(Vec3::new(1.86, 0.2, 1.86)),
    ));
    (mb.build(commands), ColliderShape::Compound(colliders))
}
