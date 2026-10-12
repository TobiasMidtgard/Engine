//! The court floor: concentric courses of worn flagstones on a bed of dirt, dark accent
//! courses, a ring of glowing runes near the rim, a curb, and a medallion at the center.

use engine_core::math::{Quat, Rng, Vec2, Vec3, smoothstep};
use engine_render::Mesh;

use super::super::meshes::nature::{grass_tuft, moss_cushion};
use super::super::meshes::stone::{
    Stone, bend_to_ring, centroid, clip, ring_courses, rune_line, split, strokes_mesh,
};
use super::super::meshes::{Outline, Profile, Slab, moved, paint, revolve, uv_shift};
use super::super::{Prop, Surface};
use super::shade;

/// Height of the walkable top of the paving, meters.
pub const TOP: f32 = 0.06;

/// Part of the floor left unpaved because something stands there (court space, meters).
#[derive(Debug, Clone, Copy)]
pub enum Clearing {
    Circle { center: Vec2, radius: f32 },
    Rect { min: Vec2, max: Vec2 },
}

impl Clearing {
    pub fn contains(&self, p: Vec2) -> bool {
        match *self {
            Clearing::Circle { center, radius } => p.distance_squared(center) < radius * radius,
            Clearing::Rect { min, max } => p.cmpge(min).all() && p.cmple(max).all(),
        }
    }
}

/// One paving stone's look: top height, tilt, dome, shade.
fn lay(rng: &mut Rng, poly: Vec<Vec2>, top: f32, bevel: (f32, f32)) -> Stone {
    let mut s = Stone::new(poly, 0.018, top, rng.range(bevel.0, bevel.1));
    s.tilt = Vec2::new(rng.range(-0.012, 0.012), rng.range(-0.012, 0.012));
    s.dome = rng.range(-0.004, 0.007);
    s.detail = 0;
    s
}

/// Mesh one stone and paint it: its own shade, dirt down its sides, its own texture offset.
fn cut(rng: &mut Rng, stone: &Stone, tint: Vec3) -> Mesh {
    let mut m = stone.mesh();
    let top = stone.y1;
    let k = shade(rng, 0.2) * tint;
    let seed = rng.next_u64();
    paint(&mut m, |p, n| {
        let side = smoothstep(0.03, top - 0.003, p.y);
        let wear = 1.0 + 0.07 * super::super::meshes::noise1(p, 3.0, seed, 2);
        // Worn, paler rounded edges.
        let edge = 1.0 + 0.1 * smoothstep(0.35, 0.8, n.y) * (1.0 - smoothstep(0.93, 0.99, n.y));
        k * (0.42 + 0.58 * side) * wear * edge
    });
    for v in &mut m.vertices {
        v.uv = [v.position[0], v.position[2]];
    }
    uv_shift(
        &mut m,
        Vec2::new(rng.range(0.0, 20.0), rng.range(0.0, 20.0)),
        rng.range(0.0, std::f32::consts::TAU),
    );
    m
}

/// The court floor, `radius` meters, leaving `clearings` unpaved.
pub fn floor(radius: f32, clearings: &[Clearing]) -> Prop {
    let mut rng = Rng::new(0xF1A9);
    let mut flag = Mesh::default();
    let mut dark = Mesh::default();
    let mut carved = Mesh::default();
    let mut moss = Mesh::default();
    let mut runes = Mesh::default();
    let mut bronze = Mesh::default();
    let cleared = |p: Vec2| clearings.iter().any(|c| c.contains(p));

    // Bed of dirt the stones sit in; it shows in the joints.
    let mut bed = revolve(
        &Profile::new()
            .to(radius + 0.05, 0.034)
            .to(radius * 0.5, 0.034)
            .to(0.0, 0.034),
        96,
    );
    paint(&mut bed, |_, _| Vec3::splat(0.55));

    // Paving courses, with two dark accent courses.
    let rings = [
        (1.62, 6.4, false),
        (6.4, 6.72, true),
        (6.72, 13.3, false),
        (13.3, 13.62, true),
        (13.62, 19.98, false),
    ];
    let mut joints: Vec<(Vec2, f32)> = Vec::new();
    for (r0, r1, accent) in rings {
        let (course, length) = if accent {
            ((r1 - r0, r1 - r0), (0.9, 1.3))
        } else {
            ((0.42, 0.7), (0.45, 0.95))
        };
        for (_, poly) in ring_courses(&mut rng, r0, r1, course, length, 0.02) {
            let c = centroid(&poly);
            if cleared(c) {
                continue;
            }
            let r = c.length();
            let top = TOP + rng.range(-0.006, 0.003);
            let target = if accent { &mut dark } else { &mut flag };
            // Some stones have cracked in two.
            let pieces = if !accent && rng.chance(0.07) {
                let dir = Vec2::from_angle(rng.range(0.0, std::f32::consts::TAU));
                let (a, b) = split(&poly, c + rng.in_unit_disk() * 0.05, dir, 0.006);
                vec![a, b]
            } else {
                vec![poly.clone()]
            };
            // Some stones are a little warmer, cooler or paler than the rest.
            let tint = match rng.range_i32(0, 10) {
                0 => Vec3::new(1.08, 1.0, 0.9),
                1 => Vec3::new(0.92, 0.96, 1.04),
                2 => Vec3::splat(1.12),
                3 => Vec3::splat(0.85),
                _ => Vec3::ONE,
            };
            for mut piece in pieces {
                // Chipped corners.
                if piece.len() >= 3 && rng.chance(0.3) {
                    let i = rng.range_i32(0, piece.len() as i32) as usize;
                    let mid = centroid(&piece);
                    let n = (piece[i] - mid).normalize_or(Vec2::X);
                    let chip = rng.range(0.025, 0.07);
                    piece = clip(&piece, n, piece[i].dot(n) - chip);
                }
                if piece.len() < 3 {
                    continue;
                }
                let mut s = lay(&mut rng, piece, top, (0.006, 0.016));
                s.segs = if accent { 2 } else { 1 };
                target.merge(&cut(&mut rng, &s, tint));
            }
            // Moss gathers in the joints, more of it toward the shaded rim.
            let mossy = 0.06 + 0.22 * smoothstep(5.0, 19.0, r);
            if rng.chance(mossy) {
                let i = rng.range_i32(0, poly.len() as i32) as usize;
                let j = (i + 1) % poly.len();
                let at = poly[i].lerp(poly[j], rng.range(0.0, 1.0));
                joints.push((at, rng.range(0.03, 0.09)));
            }
        }
    }
    for (at, size) in &joints {
        for k in 0..rng.range_i32(1, 4) {
            let off = rng.in_unit_disk() * size * 0.8 * k as f32;
            let s = size * rng.range(0.6, 1.0);
            let m = moss_cushion(&mut rng, s);
            let p = Vec3::new(at.x + off.x, 0.03, at.y + off.y);
            moss.merge(&moved(m, p, Quat::from_rotation_y(rng.range(0.0, 6.3))));
        }
        // Now and then a tuft of grass has seeded in the joint.
        if rng.chance(0.35) {
            let (n, h) = (rng.range_i32(4, 9) as u32, rng.range(0.06, 0.14));
            let t = grass_tuft(&mut rng, n, h);
            let p = Vec3::new(at.x, 0.035, at.y);
            moss.merge(&moved(t, p, Quat::from_rotation_y(rng.range(0.0, 6.3))));
        }
    }

    // The rune ring: a band of dark stone with glowing runes, between the paving and the curb.
    let (band0, band1) = (19.98, 20.78);
    let mid = (band0 + band1) * 0.5;
    for (_, poly) in ring_courses(&mut rng, band0, band1, (0.8, 0.8), (1.2, 1.6), 0.012) {
        let s = lay(&mut rng, poly, TOP, (0.01, 0.02));
        dark.merge(&cut(&mut rng, &s, Vec3::splat(0.95)));
    }
    let per_side = 32;
    let arc = std::f32::consts::TAU * mid / 8.0;
    for side in 0..8 {
        let mut line = rune_line(&mut rng, per_side, 0.36, false);
        // Fit the line to an eighth of the ring with room for a diamond between sides.
        let len = arc * 0.9;
        let scale = len / (per_side as f32 * 0.36 / 1.4 * 1.6);
        for v in &mut line.vertices {
            v.position[0] = v.position[0] * scale + arc * (side as f32 + 0.5);
        }
        let mut diamond = strokes_mesh(
            &[
                (Vec2::new(-0.12, 0.0), Vec2::new(0.0, 0.12)),
                (Vec2::new(0.0, 0.12), Vec2::new(0.12, 0.0)),
                (Vec2::new(0.12, 0.0), Vec2::new(0.0, -0.12)),
                (Vec2::new(0.0, -0.12), Vec2::new(-0.12, 0.0)),
            ],
            0.035,
        );
        for v in &mut diamond.vertices {
            v.position[0] += arc * side as f32;
        }
        line.merge(&diamond);
        bend_to_ring(&mut line, mid, TOP + 0.0015);
        runes.merge(&line);
    }

    // The curb: long carved blocks, a little proud of the paving, stepping down to the lawn.
    let (c0, c1) = (band1, radius + 0.18);
    for (_, poly) in ring_courses(&mut rng, c0, c1, (c1 - c0, c1 - c0), (0.9, 1.4), 0.012) {
        let mut s = Stone::new(poly, -0.03, TOP + 0.03, rng.range(0.02, 0.035));
        s.tilt = Vec2::new(rng.range(-0.006, 0.006), rng.range(-0.006, 0.006));
        let m = cut(&mut rng, &s, Vec3::splat(1.05));
        carved.merge(&m);
    }
    // Grass where the lawn meets the curb, and in a few outer joints.
    let tufts = 170;
    for i in 0..tufts {
        let a = i as f32 / tufts as f32 * std::f32::consts::TAU + rng.range(-0.02, 0.02);
        let r = c1 + rng.range(0.0, 0.12);
        let (n, h) = (rng.range_i32(5, 10) as u32, rng.range(0.08, 0.2));
        let t = grass_tuft(&mut rng, n, h);
        moss.merge(&moved(
            t,
            Vec3::new(a.cos() * r, 0.0, a.sin() * r),
            Quat::from_rotation_y(rng.range(0.0, 6.3)),
        ));
    }

    medallion(&mut rng, &mut dark, &mut carved, &mut bronze);

    let mut prop = Prop::new();
    prop.push(Surface::Dirt, bed);
    prop.push(Surface::Flagstone, flag);
    prop.push(Surface::DarkStone, dark);
    prop.push(Surface::CarvedStone, carved);
    prop.push(Surface::Moss, moss);
    prop.push(Surface::RuneArcane, runes);
    prop.push(Surface::Bronze, bronze);
    prop
}

/// The center: a carved boss in a bronze ring, an eight-pointed star of pale stone on dark
/// stone, and a bronze-edged ring of pale stones.
fn medallion(rng: &mut Rng, dark: &mut Mesh, carved: &mut Mesh, bronze: &mut Mesh) {
    // Boss.
    let boss = Slab::new(
        Outline::regular(20, 0.3, 0.0).rounded(1),
        0.02,
        TOP + 0.006,
        0.02,
    )
    .mesh();
    let mut boss = boss;
    super::dress(&mut boss, 3, 0.8);
    carved.merge(&boss);
    // Bronze rings, flush with the stone.
    for (r0, r1) in [(0.3, 0.345), (1.42, 1.47)] {
        // Up the outside, across the top, down the inside.
        let prof = Profile::new()
            .corner(r1, 0.03)
            .corner(r1, TOP + 0.002)
            .corner(r0, TOP + 0.002)
            .corner(r0, 0.03);
        bronze.merge(&revolve(&prof, 64));
    }
    // Dark field between the rings, as wedge stones.
    for (_, poly) in ring_courses(rng, 0.345, 1.42, (1.075, 1.075), (0.5, 0.6), 0.01) {
        let s = lay(rng, poly, TOP, (0.008, 0.015));
        dark.merge(&cut(rng, &s, Vec3::ONE));
    }
    // The star's rays, slightly raised.
    for i in 0..8 {
        let a = i as f32 / 8.0 * std::f32::consts::TAU;
        let len = if i % 2 == 0 { 1.36 } else { 0.95 };
        let w = if i % 2 == 0 { 0.16 } else { 0.12 };
        let dir = Vec2::from_angle(a);
        let side = Vec2::new(-dir.y, dir.x);
        let base = dir * 0.38;
        for half in [-1.0f32, 1.0] {
            let poly = vec![
                base,
                base + dir * (len * 0.3) + side * (w * half),
                dir * len,
            ];
            let mut s = Stone::new(poly, 0.03, TOP + 0.004, 0.006);
            s.segs = 1;
            s.detail = 0;
            let mut m = s.mesh();
            // Light and shadow halves give the rays a carved ridge.
            let k = if half > 0.0 { 1.08 } else { 0.86 };
            paint(&mut m, |_, _| Vec3::splat(k));
            super::dress(&mut m, i as u64 * 2 + (half > 0.0) as u64, 0.6);
            carved.merge(&m);
        }
    }
    // Pale ring around it.
    for (_, poly) in ring_courses(rng, 1.47, 1.62, (0.15, 0.15), (0.45, 0.6), 0.01) {
        let s = lay(rng, poly, TOP, (0.01, 0.015));
        carved.merge(&cut(rng, &s, Vec3::ONE));
    }
}
