//! Static props for the yard, built from the procedural mesh toolkit ([`super::meshkit`])
//! and the procedural materials ([`super::kit`]): shared helpers (boards, beams, rope
//! lashings, ring bolts) plus the flagpoles and pennant posts. The other props live in
//! `props_stage` (curtain stage, turret, burner, bead curtain, swing), `props_bridge` (timber
//! towers and stone piers) and `props_machines` (crates, gantry, wrecking ball, press).

use std::f32::consts::FRAC_PI_2;

use engine_render::Mesh;
use engine_world::physics::ColliderShape;

use super::kit::{Mat, Model, ModelBuilder, cylinder, part};
use super::meshkit::*;
use super::texgen::rand01;
use super::*;

/// A plain box shape of full `size` (meters), for stand-in props.
#[allow(dead_code)]
pub(super) fn cuboid(size: Vec3) -> Shape {
    Shape::Cuboid {
        size: size.to_array(),
    }
}

/// A transform at world position `p`.
pub(super) fn at(p: DVec3) -> Transform {
    Transform::from_translation(p)
}

// ---------------------------------------------------------------------------------------
// Helpers

/// Deterministic random number in 0..1 for (`seed`, `k`).
pub(super) fn rnd(seed: u32, k: u32) -> f32 {
    rand01(seed as i32, k as i32, 4242)
}

/// Rotation whose +Y points along `dir` and whose +X leans toward `side`.
pub(super) fn frame_rot(dir: Vec3, side: Vec3) -> Quat {
    let y = dir.normalize_or(Vec3::Y);
    let mut x = side - y * side.dot(y);
    if x.length_squared() < 1e-8 {
        x = y.any_orthonormal_vector();
    }
    let x = x.normalize();
    let z = x.cross(y);
    Quat::from_mat3(&Mat3::from_cols(x, y, z))
}

/// A sawn board or beam of `size` (any axis may be the long one), centered: wood UVs from a
/// random strip of the atlas, a slight bow and twist, softened edges and a tint so no two
/// boards look alike.
pub(super) fn board(size: Vec3, seed: u32) -> Mesh {
    let r = |k| rnd(seed, k);
    let long = if size.x >= size.y && size.x >= size.z {
        Vec3::X
    } else if size.y >= size.z {
        Vec3::Y
    } else {
        Vec3::Z
    };
    let thin = if size.x <= size.y && size.x <= size.z {
        Vec3::X
    } else if size.y <= size.z {
        Vec3::Y
    } else {
        Vec3::Z
    };
    let len = size.dot(long);
    let cross = (size - long * len).max_element();
    let min = size.min_element();
    let bevel = (min * 0.14).clamp(0.003, 0.02);
    let offset = Vec2::new(
        r(0) * (WOOD_GRAIN_W_M - cross).max(0.0),
        r(1) * WOOD_GRAIN_H_M,
    );
    let m = timber(size, bevel, offset, r(2) > 0.5);
    let bow = (r(3) - 0.5) * 0.003 * len.min(4.0);
    let m = warp(m, long, len, thin, bow, (r(4) - 0.5) * 0.015);
    let shade = 0.84 + 0.2 * r(5);
    let warm = (r(6) - 0.5) * 0.08;
    tint(
        m,
        Vec4::new(shade * (1.0 + warm), shade, shade * (1.0 - warm), 1.0),
    )
}

/// A board from `a` to `b` (its length), `w` wide along `side` and `t` thick.
pub(super) fn beam(a: Vec3, b: Vec3, w: f32, t: f32, side: Vec3, seed: u32) -> Mesh {
    let len = a.distance(b);
    let m = board(Vec3::new(w, len, t), seed);
    place(m, (a + b) * 0.5, frame_rot(b - a, side))
}

/// A board of `size` centered at `c` (axis aligned).
pub(super) fn board_at(c: Vec3, size: Vec3, seed: u32) -> Mesh {
    place(board(size, seed), c, Quat::IDENTITY)
}

/// A straight rope from `a` to `b` (radius `r`) with the laid-rope texture.
pub(super) fn rope_line(points: &[Vec3], r: f32) -> Mesh {
    tube(
        points,
        |_| r,
        8,
        false,
        true,
        1.0 / (std::f32::consts::TAU * r),
    )
}

/// A ring of rope (one turn of a lashing) around +Y.
pub(super) fn rope_ring(major: f32, r: f32) -> Mesh {
    let n = 20;
    let path: Vec<Vec3> = (0..n)
        .map(|i| {
            let a = i as f32 / n as f32 * std::f32::consts::TAU;
            Vec3::new(a.cos() * major, 0.0, -a.sin() * major)
        })
        .collect();
    tube(
        &path,
        |_| r,
        8,
        true,
        false,
        1.0 / (std::f32::consts::TAU * r),
    )
}

/// Rope lashing: `turns` turns of rope around a vertical post of radius `post_r` at `c`,
/// plus a tail from the lashing to `tail_to` (where a soft rope's end is pinned).
pub(super) fn lashing(c: Vec3, post_r: f32, turns: u32, tail_to: Option<Vec3>) -> Mesh {
    let r = 0.013;
    let mut m = Mesh::default();
    for k in 0..turns {
        let y = (k as f32 - (turns as f32 - 1.0) * 0.5) * r * 1.9;
        let tilt = Quat::from_rotation_z(0.06 * if k % 2 == 0 { 1.0 } else { -1.0 });
        m.merge(&place(
            rope_ring(post_r + r * 0.9, r),
            c + Vec3::Y * y,
            tilt,
        ));
    }
    if let Some(end) = tail_to {
        let flat = Vec3::new(end.x - c.x, 0.0, end.z - c.z).normalize_or(Vec3::X);
        let start = c + flat * (post_r + r * 0.5);
        let mid = start.lerp(end, 0.5) - Vec3::Y * 0.01;
        m.merge(&rope_line(&[start, mid, end], r));
    }
    m
}

/// An eye bolt with a ring standing on a surface at `base` (the ring's plane faces `facing`).
pub(super) fn ring_bolt(base: Vec3, facing: Vec3) -> Mesh {
    let mut m = place(dome(0.03, 0.012, 12), base, Quat::IDENTITY);
    m.merge(&turned(
        &[(0.0, 0.0), (0.009, 0.0), (0.009, 0.025), (0.0, 0.025)],
        8,
        base,
    ));
    // The ring stands in the plane of Y and `facing`, so a rope pulled that way runs through.
    let axis = facing.cross(Vec3::Y).normalize_or(Vec3::Z);
    let rot = Quat::from_rotation_arc(Vec3::Y, axis);
    m.merge(&place(
        torus(0.032, 0.0065, 20, 8),
        base + Vec3::Y * 0.06,
        rot,
    ));
    m
}

/// Carriage-bolt heads (domes) at `points` on a surface facing `normal`.
pub(super) fn bolt_heads(points: &[Vec3], normal: Vec3, r: f32) -> Mesh {
    let head = dome(r, r * 0.45, 10);
    let rot = Quat::from_rotation_arc(Vec3::Y, normal.normalize());
    join(points.iter().map(|&p| place(head.clone(), p, rot)))
}

/// Hex nuts with washers at `points` on a surface facing `normal`.
pub(super) fn nuts(points: &[Vec3], normal: Vec3, size: f32) -> Mesh {
    let b = bolt(size);
    let rot = Quat::from_rotation_arc(Vec3::Y, normal.normalize());
    join(points.iter().map(|&p| place(b.clone(), p, rot)))
}

/// A turned profile (radius, y) as a lathe with `segments`, offset to `at`.
pub(super) fn turned(profile: &[(f32, f32)], segments: u32, at: Vec3) -> Mesh {
    let p: Vec<Vec2> = profile.iter().map(|&(r, y)| Vec2::new(r, y)).collect();
    let r_ref = p.iter().map(|v| v.x).fold(0.0, f32::max).max(0.01);
    place(
        lathe(&p, segments, LatheUv::meters(r_ref)),
        at,
        Quat::IDENTITY,
    )
}

/// A turned wooden part (grain along the axis).
pub(super) fn turned_wood(profile: &[(f32, f32)], segments: u32, at: Vec3, seed: u32) -> Mesh {
    let p: Vec<Vec2> = profile.iter().map(|&(r, y)| Vec2::new(r, y)).collect();
    let r_ref = p.iter().map(|v| v.x).fold(0.0, f32::max).max(0.01);
    let off = Vec2::new(rnd(seed, 0) * 0.2, rnd(seed, 1) * WOOD_GRAIN_H_M);
    place(
        lathe(&p, segments, LatheUv::wood(r_ref, off)),
        at,
        Quat::IDENTITY,
    )
}

/// A tapered round post: radius `r0` at `y0` to `r1` at `y1`, with flat caps.
pub(super) fn taper(r0: f32, r1: f32, y0: f32, y1: f32, rings: u32, segments: u32) -> Mesh {
    let mut p = vec![(0.0, y0), (r0, y0), (r0, y0)];
    for i in 0..=rings {
        let t = i as f32 / rings as f32;
        p.push((r0 + (r1 - r0) * t, y0 + (y1 - y0) * t));
    }
    p.push((r1, y1));
    p.push((0.0, y1));
    turned(&p, segments, Vec3::ZERO)
}

// ---------------------------------------------------------------------------------------
// Flagpole

/// Height of the banner flagpoles, meters.
pub(super) const FLAGPOLE_HEIGHT: f32 = 9.0;

/// Radius of the galvanized flagpole at height `y`.
fn pole_radius(y: f32) -> f32 {
    0.085 - 0.035 * (y / FLAGPOLE_HEIGHT).clamp(0.0, 1.0)
}

/// A 9 m tapered galvanized flagpole on a concrete footing: bolted base plate with gussets,
/// truck with a sheave, gilded ball finial, halyard down to a cleat with a coiled hank.
/// Origin at the foot; the flag's hoist hangs at +X (0.08 m from the axis).
pub(super) fn flagpole(commands: &mut Commands) -> (Model, ColliderShape) {
    let mut mb = ModelBuilder::new("flagpole").grounded();
    let top = FLAGPOLE_HEIGHT;
    // Footing and base plate.
    mb.add(
        Mat::Concrete,
        turned(
            &[
                (0.0, -0.12),
                (0.36, -0.12),
                (0.36, -0.12),
                (0.36, 0.07),
                (0.35, 0.09),
                (0.33, 0.1),
                (0.33, 0.1),
                (0.0, 0.1),
            ],
            32,
            Vec3::ZERO,
        ),
    );
    mb.add(
        Mat::Galvanized,
        place(
            rounded_box(Vec3::new(0.34, 0.022, 0.34), 0.006, 2, 0.4, BoxUv::PLANAR),
            Vec3::Y * 0.111,
            Quat::IDENTITY,
        ),
    );
    let corners: Vec<Vec3> = [(-1.0, -1.0), (1.0, -1.0), (1.0, 1.0), (-1.0, 1.0)]
        .iter()
        .map(|&(x, z)| Vec3::new(x * 0.125, 0.122, z * 0.125))
        .collect();
    mb.add_small(Mat::Galvanized, nuts(&corners, Vec3::Y, 0.016));
    // Sleeve and gussets.
    mb.add(
        Mat::Galvanized,
        turned(
            &[
                (0.0, 0.122),
                (0.112, 0.122),
                (0.112, 0.122),
                (0.112, 0.14),
                (0.104, 0.16),
                (0.096, 0.37),
                (0.103, 0.385),
                (0.103, 0.41),
                (0.094, 0.43),
                (0.0, 0.43),
            ],
            28,
            Vec3::ZERO,
        ),
    );
    let gusset = extrude(
        &[
            Vec2::new(0.09, 0.0),
            Vec2::new(0.165, 0.0),
            Vec2::new(0.165, 0.02),
            Vec2::new(0.095, 0.21),
            Vec2::new(0.09, 0.21),
        ],
        0.012,
        0.002,
        20.0,
    );
    for k in 0..4 {
        let rot = Quat::from_rotation_y(k as f32 * FRAC_PI_2 + std::f32::consts::FRAC_PI_4);
        mb.add(Mat::Galvanized, place(gusset.clone(), Vec3::Y * 0.122, rot));
    }
    // The pole: tapered, with a faint weld seam ring near the base.
    let mut prof = vec![(0.0, 0.4)];
    for i in 0..=12 {
        let y = 0.4 + (top - 0.12 - 0.4) * i as f32 / 12.0;
        prof.push((pole_radius(y), y));
    }
    prof.push((0.0, top - 0.12));
    mb.add(Mat::Galvanized, turned(&prof, 24, Vec3::ZERO));
    mb.add_small(
        Mat::Galvanized,
        place(
            torus(pole_radius(2.5), 0.004, 24, 6),
            Vec3::Y * 2.5,
            Quat::IDENTITY,
        ),
    );
    // Truck with sheave, collar and ball.
    mb.add(
        Mat::Galvanized,
        turned(
            &[
                (0.0, top - 0.13),
                (0.058, top - 0.13),
                (0.064, top - 0.1),
                (0.064, top - 0.03),
                (0.052, top - 0.01),
                (0.02, top - 0.01),
                (0.02, top - 0.01),
                (0.02, top + 0.03),
                (0.0, top + 0.03),
            ],
            24,
            Vec3::ZERO,
        ),
    );
    let sheave = place(
        turned(
            &[
                (0.0, -0.007),
                (0.024, -0.007),
                (0.019, 0.0),
                (0.024, 0.007),
                (0.0, 0.007),
            ],
            16,
            Vec3::ZERO,
        ),
        Vec3::new(0.066, top - 0.06, 0.0),
        Quat::from_rotation_x(FRAC_PI_2),
    );
    mb.add_small(Mat::Brass, sheave);
    mb.add(
        Mat::Gold,
        turned(
            &[
                (0.0, top + 0.02),
                (0.034, top + 0.02),
                (0.034, top + 0.035),
                (0.022, top + 0.05),
                (0.0, top + 0.05),
            ],
            16,
            Vec3::ZERO,
        ),
    );
    mb.add(
        Mat::Gold,
        place(sphere(0.11, 28), Vec3::Y * (top + 0.15), Quat::IDENTITY),
    );
    // Halyard: two falls from the sheave to the cleat.
    let cleat_y = 1.3;
    let sheave_out = Vec3::new(0.088, top - 0.06, 0.0);
    for (dz, end_y) in [(0.008f32, cleat_y + 0.02), (-0.008, cleat_y + 0.05)] {
        mb.add(
            Mat::Rope,
            rope_line(
                &[sheave_out + Vec3::Z * dz, Vec3::new(0.118, end_y, dz * 1.5)],
                0.0045,
            ),
        );
    }
    // Cleat: two standoffs and a horned bar, bolted to the pole.
    let cr = pole_radius(cleat_y);
    let mut cleat = Mesh::default();
    cleat.merge(&place(
        rounded_box(Vec3::new(0.018, 0.13, 0.034), 0.006, 2, 0.2, BoxUv::PLANAR),
        Vec3::new(cr + 0.006, cleat_y, 0.0),
        Quat::IDENTITY,
    ));
    for dy in [-0.028f32, 0.028] {
        cleat.merge(&place(
            rounded_box(Vec3::new(0.034, 0.014, 0.016), 0.005, 2, 0.2, BoxUv::PLANAR),
            Vec3::new(cr + 0.03, cleat_y + dy, 0.0),
            Quat::IDENTITY,
        ));
    }
    let horn: Vec<Vec3> = (0..=10)
        .map(|i| {
            let t = i as f32 / 10.0;
            Vec3::new(
                cr + 0.05 + 0.006 * (std::f32::consts::PI * t).sin(),
                cleat_y - 0.075 + 0.15 * t,
                0.0,
            )
        })
        .collect();
    cleat.merge(&tube(
        &horn,
        |t| 0.006 + 0.005 * (std::f32::consts::PI * t).sin(),
        10,
        false,
        true,
        1.0,
    ));
    mb.add(Mat::Galvanized, cleat);
    // Halyard made fast on the cleat (figure eight) and a coiled hank hanging below it.
    let hx = cr + 0.058;
    let fig8 = [
        Vec3::new(hx, cleat_y + 0.06, 0.012),
        Vec3::new(hx + 0.004, cleat_y, 0.0),
        Vec3::new(hx, cleat_y - 0.06, -0.012),
        Vec3::new(hx - 0.012, cleat_y - 0.064, 0.0),
        Vec3::new(hx, cleat_y - 0.06, 0.012),
        Vec3::new(hx + 0.004, cleat_y, 0.0),
        Vec3::new(hx, cleat_y + 0.06, -0.012),
    ];
    mb.add_small(Mat::Rope, rope_line(&fig8, 0.0045));
    for k in 0..4 {
        let a = 0.25 * (k as f32 - 1.5);
        let loop_r = 0.085 + 0.006 * k as f32;
        let ring = rope_ring(loop_r, 0.0045);
        let rot = Quat::from_rotation_y(a * 0.3) * Quat::from_rotation_z(FRAC_PI_2);
        mb.add_small(
            Mat::Rope,
            place(
                ring,
                Vec3::new(hx + 0.012, cleat_y - 0.07 - loop_r, 0.004 * k as f32),
                rot,
            ),
        );
    }
    let collider = ColliderShape::Compound(vec![
        part(Vec3::Y * -0.01, cylinder(0.36, 0.22)),
        part(Vec3::Y * 1.5, cylinder(0.08, 3.0)),
        part(Vec3::Y * 4.5, cylinder(0.068, 3.0)),
        part(Vec3::Y * 7.5, cylinder(0.057, 3.0)),
        part(
            Vec3::Y * (top + 0.15),
            ColliderShape::Sphere { radius: 0.11 },
        ),
    ]);
    (mb.build(commands), collider)
}

// ---------------------------------------------------------------------------------------
// Pennant posts

/// Height of a pennant post, meters.
pub(super) const PENNANT_POST_HEIGHT: f32 = 2.4;

/// A slim blackened-steel pennant rod in a cast ground socket, with a brass ball finial and
/// two brass clip rings holding the pennant's hoist (at +X, 0.03 m from the axis).
pub(super) fn pennant_post(commands: &mut Commands) -> (Model, ColliderShape) {
    let h = PENNANT_POST_HEIGHT;
    let mut mb = ModelBuilder::new("pennant_post").grounded();
    mb.add(
        Mat::CastIron,
        turned(
            &[
                (0.0, -0.02),
                (0.065, -0.02),
                (0.065, 0.0),
                (0.06, 0.014),
                (0.044, 0.03),
                (0.03, 0.09),
                (0.033, 0.1),
                (0.033, 0.115),
                (0.026, 0.125),
                (0.0, 0.125),
            ],
            16,
            Vec3::ZERO,
        ),
    );
    mb.add(Mat::BlackSteel, taper(0.021, 0.019, 0.12, h, 4, 10));
    mb.add(
        Mat::Brass,
        turned(
            &[
                (0.0, h - 0.005),
                (0.026, h - 0.005),
                (0.026, h + 0.012),
                (0.016, h + 0.022),
                (0.012, h + 0.03),
                (0.0, h + 0.03),
            ],
            14,
            Vec3::ZERO,
        ),
    );
    mb.add(
        Mat::Brass,
        place(sphere(0.032, 16), Vec3::Y * (h + 0.058), Quat::IDENTITY),
    );
    mb.add(
        Mat::Brass,
        turned(
            &[(0.0, h + 0.085), (0.008, h + 0.085), (0.0, h + 0.115)],
            8,
            Vec3::ZERO,
        ),
    );
    for y in [h - 0.05, h - 0.65] {
        mb.add_small(
            Mat::Brass,
            place(
                torus(0.027, 0.0028, 16, 6),
                Vec3::new(0.006, y, 0.0),
                Quat::IDENTITY,
            ),
        );
    }
    (
        mb.build(commands),
        ColliderShape::Compound(vec![part(Vec3::Y * (h * 0.5), cylinder(0.025, h))]),
    )
}
