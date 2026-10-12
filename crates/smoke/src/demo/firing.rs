//! The firing line: precast Jersey barriers with forklift slots and lifting loops, and walls
//! of individually shaped sandbags.

use std::f32::consts::{PI, TAU};

use engine_core::math::{Quat, Rng, Vec2, Vec3};
use engine_render::Mesh;

use super::art::{self, Parts};
use super::ground::{PAD_TOP, wnoise};
use super::kit::{self, tr};
use super::tex::{noise, smoothstep};

/// Half-profile of a New Jersey barrier (x across, y up), full width counter-clockwise.
fn jersey_profile(bottom: f32) -> Vec<Vec2> {
    // 0.61 m base, 0.15 m top, 0.81 m tall; 55° lower and 84° upper faces.
    let pts = [
        Vec2::new(-0.305, 0.0),
        Vec2::new(0.305, 0.0),
        Vec2::new(0.305, 0.076),
        Vec2::new(0.127, 0.330),
        Vec2::new(0.076, 0.813),
        Vec2::new(-0.076, 0.813),
        Vec2::new(-0.127, 0.330),
        Vec2::new(-0.305, 0.076),
    ];
    let radii = [0.012, 0.012, 0.03, 0.09, 0.03, 0.03, 0.09, 0.03];
    let mut p = pts.to_vec();
    if bottom > 0.0 {
        // Cut the profile at y = bottom (the forklift slot's ceiling).
        let x = 0.305 - (bottom - 0.076).max(0.0) / (55.0f32).to_radians().tan();
        p[0] = Vec2::new(-x, bottom);
        p[1] = Vec2::new(x, bottom);
        p[2] = Vec2::new(x, bottom.max(0.076));
        p[7] = Vec2::new(-x, bottom.max(0.076));
        if bottom >= 0.076 {
            p.remove(7);
            p.remove(2);
            let r = [0.008, 0.008, 0.09, 0.03, 0.03, 0.09];
            return kit::fillet_each(&p, &r, 4);
        }
    }
    kit::fillet_each(&p, &radii, 4)
}

/// One 3 m Jersey barrier along +X centered on the origin, standing on y = 0.
pub fn jersey_barrier(rng: &mut Rng) -> Parts {
    let mut parts = Parts::default();
    let len = 3.0;
    let full = jersey_profile(0.0);
    let slot = jersey_profile(0.095);
    // Sections along the length: (from, to, slot?).
    let sections = [
        (0.0, 0.42, false),
        (0.42, 1.02, true),
        (1.02, 1.98, false),
        (1.98, 2.58, true),
        (2.58, 3.0, false),
    ];
    let mut body = Mesh::default();
    for (i, &(a, b, is_slot)) in sections.iter().enumerate() {
        let first = i == 0;
        let last = i == sections.len() - 1;
        let m = if is_slot {
            kit::extrude_caps(&slot, a, b, 35.0, 0.0, false, false)
        } else {
            let mut m = kit::extrude_caps(&full, a, b, 35.0, 0.0, true, true);
            if first || last {
                // Chamfer the barrier's outer ends.
                m = kit::extrude_ends(
                    &full,
                    a,
                    b,
                    35.0,
                    if first { 0.018 } else { 0.0 },
                    if last { 0.018 } else { 0.0 },
                );
            }
            m
        };
        body.merge(&m);
    }
    // Along +Z -> along +X, centered.
    let t = tr(
        Vec3::new(-len * 0.5, 0.0, 0.0),
        Quat::from_rotation_y(std::f32::consts::FRAC_PI_2),
    );
    let mut body = kit::placed(body, t);
    body = kit::offset_uv(body, Vec2::new(rng.range(0.0, 2.0), rng.range(0.0, 2.0)));
    let tone = rng.range(0.92, 1.06);
    let mut body = kit::tint(body, Vec3::new(tone, tone, tone * 0.98));
    let seed = rng.next_u32();
    kit::shade(&mut body, |p, n| {
        // Mud splashed up the lower face, darker in the slots; chipped, dirtier top.
        let splash = 1.0 - smoothstep(0.0, 0.42, p.y + wnoise(p.x, p.y, 0.25, seed) * 0.08);
        let top = smoothstep(0.75, 0.81, p.y) * 0.15;
        let streak = smoothstep(0.3, 0.8, wnoise(p.x * 3.0, p.y * 0.3, 1.0, seed + 1)) * 0.12;
        let under = if n.y < -0.5 { 0.6 } else { 1.0 };
        Vec3::ONE.lerp(Vec3::new(0.62, 0.55, 0.46), splash * 0.75) * (1.0 - top - streak) * under
    });
    parts.add(art::PRECAST, body);
    // Lifting loops of rebar on top.
    for x in [-0.95f32, 0.95] {
        let path: Vec<Vec3> = (0..=10)
            .map(|i| {
                let a = i as f32 / 10.0 * PI;
                Vec3::new(x + a.cos() * 0.07, 0.79 + a.sin() * 0.07, 0.0)
            })
            .collect();
        parts.add(art::RUST, kit::tube(&path, 0.009, 8, false));
    }
    parts
}

/// A row of Jersey barriers from x0 to x1 (along X) at depth z.
pub fn barrier_row(parts: &mut Parts, x0: f32, x1: f32, z: f32, seed: u64) {
    let mut rng = Rng::new(seed);
    let n = ((x1 - x0) / 3.1).floor().max(1.0) as i32;
    let gap = ((x1 - x0) - n as f32 * 3.0) / (n - 1).max(1) as f32;
    for k in 0..n {
        let cx = x0 + 1.5 + k as f32 * (3.0 + gap);
        let b = jersey_barrier(&mut rng);
        let t = tr(
            Vec3::new(
                cx + rng.range(-0.03, 0.03),
                PAD_TOP,
                z + rng.range(-0.04, 0.04),
            ),
            Quat::from_rotation_y(rng.range(-0.02, 0.02)),
        );
        parts.absorb(b, t);
    }
}

/// One filled sandbag lying flat: about `len` × `wid` × `hgt` m, long axis along X, bottom on
/// y = 0. Every seed gives a differently slumped, lumpy bag with one tied and one folded end.
pub fn sandbag(seed: u32, len: f32, wid: f32, hgt: f32) -> Mesh {
    let (a, b, h) = (len * 0.5, wid * 0.5, hgt * 0.5);
    let tied_end = if seed & 1 == 0 { 1.0 } else { -1.0 };
    let sc = |t: f32, e: f32| t.cos().signum() * t.cos().abs().powf(e);
    let ss = |t: f32, e: f32| t.sin().signum() * t.sin().abs().powf(e);
    let mut m = kit::surface(30, 16, true, |u, v| {
        let w = (0.5 - u) * TAU; // around (longitude), so the surface faces out
        let eta = (v - 0.5) * PI; // bottom to top (latitude)
        let e1 = if eta < 0.0 { 0.22 } else { 0.5 };
        let e2 = 0.28;
        let mut x = a * sc(eta, e1) * sc(w, e2);
        let mut z = b * sc(eta, e1) * ss(w, e2);
        let mut y = h * ss(eta, e1);
        let t = (x / a).abs();
        let end = smoothstep(0.55, 1.0, t);
        let tied = if x * tied_end > 0.0 { 1.0 } else { 0.55 };
        // Ends pinch and flatten; the middle bulges and slumps.
        z *= 1.0 - end * 0.42 * tied + (1.0 - t * t) * 0.05;
        if y > 0.0 {
            y *= 1.0 - end * 0.55 * tied - (1.0 - t * t) * (1.0 - (z / b).powi(2)) * 0.06;
        } else {
            y *= 1.0 - end * 0.2;
        }
        x *= 1.0 + end * 0.04 * tied;
        // Lumps of sand and creases running into the tied end.
        let lump = noise(u * 6.0, v * 3.0 + seed as f32 * 0.37, 6, 1000, seed) * 0.012
            + noise(u * 14.0, v * 7.0, 14, 1000, seed + 1) * 0.004;
        let crease = (w * 11.0).sin() * end * end * 0.01 * tied;
        let up = if y > 0.0 { 1.0 } else { 0.3 };
        let rad = Vec3::new(x / a, y / h * 0.6, z / b).normalize_or(Vec3::Y);
        let p = Vec3::new(x, y + h * 0.82, z) + rad * (lump * up + crease);
        let uv = Vec2::new(u * (a + b) * 3.6, v * (h * 2.0 + b) * 1.7);
        (Vec3::new(p.x, p.y.max(0.0), p.z), uv)
    });
    // Darker where it sits on the course below and in the creases.
    kit::shade(&mut m, |p, _| {
        let low = 1.0 - smoothstep(0.0, hgt * 0.35, p.y);
        Vec3::splat(1.0 - low * 0.4)
    });
    m
}

/// Bag tints: new tan, faded sand, olive and old dirty bags.
fn bag_tint(rng: &mut Rng) -> Vec3 {
    let palette = [
        Vec3::new(1.0, 0.94, 0.82),
        Vec3::new(1.08, 1.02, 0.9),
        Vec3::new(0.78, 0.8, 0.6),
        Vec3::new(0.72, 0.64, 0.52),
        Vec3::new(0.92, 0.84, 0.7),
    ];
    let c = palette[rng.range_i32(0, palette.len() as i32) as usize];
    c * rng.range(0.9, 1.08)
}

fn place_bag(parts: &mut Parts, rng: &mut Rng, pos: Vec3, yaw: f32, size: Vec3) {
    let s = size
        * Vec3::new(
            rng.range(0.94, 1.06),
            rng.range(0.95, 1.05),
            rng.range(0.93, 1.07),
        );
    let bag = sandbag(rng.next_u32(), s.x, s.z, s.y);
    let rot = Quat::from_rotation_y(yaw + rng.range(-0.07, 0.07))
        * Quat::from_rotation_x(rng.range(-0.04, 0.04))
        * Quat::from_rotation_z(rng.range(-0.04, 0.04));
    let t = tr(
        pos + Vec3::new(rng.range(-0.02, 0.02), 0.0, rng.range(-0.02, 0.02)),
        rot,
    );
    let mut m = kit::tint(kit::placed(bag, t), bag_tint(rng));
    m = kit::offset_uv(m, Vec2::new(rng.f32(), rng.f32()));
    kit::ground_grime(&mut m, PAD_TOP, 0.25, 0.7, Vec3::new(0.85, 0.78, 0.68));
    parts.add(art::SANDBAG, m);
}

/// Bag size laid flat (length, height, width).
pub const BAG: Vec3 = Vec3::new(0.58, 0.15, 0.32);
/// Height a course adds (bags settle into the ones below).
const COURSE: f32 = 0.128;

/// A curved sandbag wall: `rows` bags thick, `courses` high, around `center` at `radius`
/// from angle `a0` to `a1` (radians from +Z toward +X), with a course of headers on top.
pub fn sandbag_arc(
    parts: &mut Parts,
    center: Vec2,
    radius: f32,
    [a0, a1]: [f32; 2],
    courses: u32,
    rows: u32,
    seed: u64,
) {
    let mut rng = Rng::new(seed);
    for c in 0..courses {
        let y = PAD_TOP + c as f32 * COURSE;
        for r in 0..rows {
            let rad = radius + r as f32 * (BAG.z + 0.01);
            let len = rad * (a1 - a0);
            let n = (len / (BAG.x - 0.02)).floor().max(1.0) as i32;
            let step = (a1 - a0) / n as f32;
            let offset = if (c + r) % 2 == 0 { 0.0 } else { 0.5 };
            let count = if offset > 0.0 { n - 1 } else { n };
            for k in 0..count {
                let a = a0 + step * (k as f32 + 0.5 + offset);
                let pos = Vec3::new(center.x + rad * a.sin(), y, center.y + rad * a.cos());
                // Long axis along the wall (tangent).
                place_bag(parts, &mut rng, pos, a, BAG);
            }
        }
    }
    // Headers across the top.
    let y = PAD_TOP + courses as f32 * COURSE;
    let rad = radius + (rows as f32 - 1.0) * (BAG.z + 0.01) * 0.5;
    let n = ((rad * (a1 - a0)) / (BAG.z + 0.03)).floor().max(1.0) as i32;
    let step = (a1 - a0) / n as f32;
    for k in 0..n {
        let a = a0 + step * (k as f32 + 0.5);
        let pos = Vec3::new(center.x + rad * a.sin(), y, center.y + rad * a.cos());
        place_bag(parts, &mut rng, pos, a + std::f32::consts::FRAC_PI_2, BAG);
    }
}

/// A single bag at `p` (on the pad or on something at height `y`), for weighing things down.
pub fn loose_bag(parts: &mut Parts, p: Vec3, yaw: f32, seed: u64) {
    let mut rng = Rng::new(seed);
    place_bag(parts, &mut rng, p, yaw, BAG * Vec3::new(0.9, 1.0, 0.9));
}
