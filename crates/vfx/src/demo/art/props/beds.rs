//! Test beds: the four footstep patches the golem walks across, and the six framed impact
//! plates.

use engine_core::math::{Quat, Rng, Vec2, Vec3, smoothstep};
use engine_render::Mesh;

use super::super::meshes::metal::rivet;
use super::super::meshes::nature::{RockShape, pebble, rock};
use super::super::meshes::stone::{Stone, voronoi};
use super::super::meshes::{
    Outline, Slab, block, heightfield, moved, noise_warp, noise1, paint, warp,
};
use super::super::{Prop, Surface};
use super::{dress, shade};

/// Length of the footstep strip along +X (four patches of 2 m) and its half-width along Z.
pub const STRIP: (f32, f32) = (8.0, 0.8);

fn n2(p: Vec2, freq: f32, seed: u64, octaves: u32) -> f32 {
    noise1(Vec3::new(p.x, 0.0, p.y), freq, seed, octaves)
}

/// A curb of dark stone blocks along a rectangle's border: `inner` half-size, `width` wide,
/// `top` high.
fn curb(rng: &mut Rng, center: Vec2, inner: Vec2, width: f32, top: f32, out: &mut Mesh) {
    let o = inner + Vec2::splat(width);
    // Long sides run full length; short sides fit between them.
    let runs = [
        (Vec2::new(-o.x, inner.y), Vec2::new(o.x, o.y)),
        (Vec2::new(-o.x, -o.y), Vec2::new(o.x, -inner.y)),
        (Vec2::new(-o.x, -inner.y), Vec2::new(-inner.x, inner.y)),
        (Vec2::new(inner.x, -inner.y), Vec2::new(o.x, inner.y)),
    ];
    for (min, max) in runs {
        let along_x = max.x - min.x >= max.y - min.y;
        let len = if along_x {
            max.x - min.x
        } else {
            max.y - min.y
        };
        let pieces = (len / 1.0).ceil().max(1.0) as u32;
        for k in 0..pieces {
            let t0 = k as f32 / pieces as f32;
            let t1 = (k + 1) as f32 / pieces as f32;
            let (a, b) = if along_x {
                (
                    Vec2::new(min.x + (max.x - min.x) * t0 + 0.004, min.y),
                    Vec2::new(min.x + (max.x - min.x) * t1 - 0.004, max.y),
                )
            } else {
                (
                    Vec2::new(min.x, min.y + (max.y - min.y) * t0 + 0.004),
                    Vec2::new(max.x, min.y + (max.y - min.y) * t1 - 0.004),
                )
            };
            let poly = vec![
                center + a,
                center + Vec2::new(b.x, a.y),
                center + b,
                center + Vec2::new(a.x, b.y),
            ];
            let mut m = Slab::new(
                Outline::new(poly),
                -0.02,
                top + rng.range(-0.004, 0.003),
                0.018,
            )
            .open_bottom()
            .mesh();
            let k = shade(rng, 0.1);
            paint(&mut m, |_, _| k);
            dress(&mut m, rng.next_u64(), 1.2);
            out.merge(&m);
        }
    }
}

/// The footstep strip: mud with puddles, snow with soft drifts, rippled sand and shallow water
/// over pebbles, each 2 m long, from x = 0 to 8 and z = -0.8 to 0.8, in a dark stone curb.
/// Tops stay at or below 8 cm.
pub fn footstep_patches() -> Prop {
    let mut rng = Rng::new(0xF007);
    let (len, half) = STRIP;
    let mut prop = Prop::new();
    let mut dark = Mesh::default();
    curb(
        &mut rng,
        Vec2::new(len * 0.5, 0.0),
        Vec2::new(len * 0.5, half),
        0.15,
        0.1,
        &mut dark,
    );
    // Flush dividers between patches.
    for i in 1..4 {
        let x = i as f32 * 2.0;
        let mut m = block(Vec3::new(0.06, 0.07, half * 2.0), 0.01);
        m = moved(m, Vec3::new(x, 0.077 - 0.035, 0.0), Quat::IDENTITY);
        dress(&mut m, i, 1.0);
        dark.merge(&m);
    }
    prop.push(Surface::DarkStone, dark);

    let edge = |p: Vec2, x0: f32| {
        // 0 at the patch's border, 1 inside.
        let dx = (p.x - x0).min(x0 + 2.0 - p.x);
        let dz = half - p.y.abs();
        smoothstep(0.0, 0.12, dx.min(dz))
    };
    // Mud: lumpy, rutted, with puddles where it dips below the water line.
    let x0 = 0.0;
    let mut mud = heightfield(
        Vec2::new(x0, -half),
        Vec2::new(x0 + 2.0, half),
        0.025,
        |p| {
            let lumps = n2(p, 3.0, 11, 3) * 0.006 + n2(p, 9.0, 12, 2) * 0.002;
            let rut = (-((p.y - 0.25).abs() / 0.08).powi(2)).exp()
                + (-((p.y + 0.3).abs() / 0.09).powi(2)).exp();
            let dip = smoothstep(0.3, 0.65, n2(p, 1.2, 13, 2)) * 0.014;
            0.063 + lumps - 0.006 * rut - dip
        },
    );
    // Wet and dark in the hollows, drier and paler on the ridges.
    paint(&mut mud, |p, _| {
        Vec3::splat(0.7 + 0.5 * smoothstep(0.052, 0.07, p.y))
    });
    prop.push(Surface::Mud, mud);
    let puddles = heightfield(
        Vec2::new(x0 + 0.02, -half + 0.02),
        Vec2::new(x0 + 1.98, half - 0.02),
        0.5,
        |_| 0.054,
    );
    prop.push(Surface::Water, puddles);

    // Snow: soft drifts, banked against the curb.
    let x0 = 2.0;
    let mut snow = heightfield(Vec2::new(x0, -half), Vec2::new(x0 + 2.0, half), 0.02, |p| {
        // Wind-carved drifts: ridges stretched across the wind, softened by noise.
        let q = Vec2::new(p.x * 0.45 + p.y * 0.2, p.y * 1.6);
        let ridge = 1.0 - n2(q, 2.2, 21, 3).abs() * 2.0;
        let drift = ridge.max(-1.0) * 0.006 + n2(p, 1.1, 23, 2) * 0.004;
        let grain = n2(p, 9.0, 22, 2) * 0.0015;
        let bank = (1.0 - edge(p, x0)) * 0.008;
        (0.066 + drift + grain + bank).min(0.08)
    });
    paint(&mut snow, |p, n| {
        // Blue in the hollows and on slopes away from the light, white on the crests.
        let crest = smoothstep(0.06, 0.076, p.y);
        let slope = smoothstep(0.9, 0.995, n.y);
        Vec3::new(0.8 + 0.2 * crest, 0.86 + 0.14 * crest, 1.0) * (0.8 + 0.2 * slope)
    });
    prop.push(Surface::Snow, snow);

    // Sand: wind ripples, wavy and asymmetric.
    let x0 = 4.0;
    let mut sand = heightfield(
        Vec2::new(x0, -half),
        Vec2::new(x0 + 2.0, half),
        0.012,
        |p| {
            let warp = n2(p, 1.5, 31, 2) * 0.35;
            let phase = (p.x * 0.9 + p.y * 0.4 + warp) / 0.085 * std::f32::consts::TAU;
            let s = phase.sin();
            let ripple = (s * 0.5 + 0.5).powf(1.6) * 0.0045;
            0.06 + ripple + n2(p, 2.0, 32, 2) * 0.004
        },
    );
    paint(&mut sand, |p, _| {
        Vec3::splat(0.85 + 0.25 * ((p.y - 0.058) / 0.01).clamp(0.0, 1.0))
    });
    prop.push(Surface::Sand, sand);

    // Shallow water over a pebble bed.
    let x0 = 6.0;
    let mut bed = heightfield(Vec2::new(x0, -half), Vec2::new(x0 + 2.0, half), 0.05, |p| {
        0.04 + n2(p, 2.0, 41, 2) * 0.006
    });
    paint(&mut bed, |_, _| Vec3::splat(0.6));
    prop.push(Surface::Dirt, bed);
    let mut stones = Mesh::default();
    for _ in 0..260 {
        let at = Vec2::new(
            rng.range(x0 + 0.05, x0 + 1.95),
            rng.range(-half + 0.05, half - 0.05),
        );
        let size = rng.range(0.03, 0.075);
        let p = pebble(&mut rng, size);
        stones.merge(&moved(
            p,
            Vec3::new(at.x, 0.045, at.y),
            Quat::from_rotation_y(rng.range(0.0, 6.3)),
        ));
    }
    prop.push(Surface::Rock, stones);
    let water = heightfield(Vec2::new(x0, -half), Vec2::new(x0 + 2.0, half), 0.5, |_| {
        0.074
    });
    prop.push(Surface::Water, water);
    prop
}

/// The six impact plates in a row along +X, 1.5 m apart, each 1.2 m square in a dark stone
/// frame, their tops at 0.1 m: rock slab, wooden planks, riveted steel, dirt, snow, sand.
pub fn impact_plates() -> Prop {
    let mut rng = Rng::new(0x1A9A);
    let mut prop = Prop::new();
    let mut dark = Mesh::default();
    let top = 0.1;
    let h = 0.6;
    for i in 0..6 {
        let c = Vec2::new(i as f32 * 1.5, 0.0);
        curb(&mut rng, c, Vec2::splat(h), 0.11, top + 0.035, &mut dark);
        // A dark base under every bed, showing in the gaps.
        let base = block(Vec3::new(1.2, 0.06, 1.2), 0.0);
        prop.push(
            Surface::Dirt,
            moved(base, Vec3::new(c.x, 0.04, c.y), Quat::IDENTITY),
        );
    }
    prop.push(Surface::DarkStone, dark);

    // 0: a rock slab, cracked into pieces.
    let c = Vec2::new(0.0, 0.0);
    let mut slab = Mesh::default();
    for cell in voronoi(
        &mut rng,
        c - Vec2::splat(h - 0.005),
        c + Vec2::splat(h - 0.005),
        0.45,
        0.9,
        0.01,
    ) {
        let mut s = Stone::new(
            cell,
            0.05,
            top + rng.range(-0.006, 0.004),
            rng.range(0.008, 0.016),
        );
        s.tilt = Vec2::new(rng.range(-0.01, 0.01), rng.range(-0.01, 0.01));
        s.detail = 2;
        let mut m = s.mesh();
        noise_warp(&mut m, 0.004, 6.0, rng.next_u64());
        let k = shade(&mut rng, 0.1);
        paint(&mut m, |_, _| k);
        dress(&mut m, rng.next_u64(), 1.3);
        slab.merge(&m);
    }
    prop.push(Surface::Rock, slab);

    // 1: wooden planks, nailed down.
    let c = Vec2::new(1.5, 0.0);
    let mut planks = Mesh::default();
    let mut nails = Mesh::default();
    let n = 5;
    let w = (2.0 * h) / n as f32;
    for k in 0..n {
        let z = -h + w * (k as f32 + 0.5);
        let mut p = block(Vec3::new(2.0 * h - 0.01, 0.035, w - 0.008), 0.006);
        let seed = rng.next_u64();
        // Cupped and slightly twisted, with grain.
        warp(&mut p, |q| {
            let cup = (q.z / (w * 0.5)).powi(2) * 0.002;
            let twist = q.x * q.z * 0.01;
            q + Vec3::Y * (cup + twist)
        });
        let tone = rng.range(0.8, 1.1);
        paint(&mut p, |q, _| {
            let grain = noise1(Vec3::new(q.x * 0.6, 0.0, q.z * 30.0), 1.0, seed, 3);
            Vec3::new(1.0, 0.96, 0.9) * tone * (0.82 + 0.18 * grain)
        });
        for v in &mut p.vertices {
            v.uv = [v.position[0] + seed as f32 % 7.0, v.position[2] * 4.0];
        }
        planks.merge(&moved(
            p,
            Vec3::new(c.x, top - 0.0175, c.y + z),
            Quat::IDENTITY,
        ));
        for x in [-h + 0.06, h - 0.06] {
            for dz in [-w * 0.25, w * 0.25] {
                nails.merge(&moved(
                    rivet(0.006),
                    Vec3::new(c.x + x, top, c.y + z + dz),
                    Quat::IDENTITY,
                ));
            }
        }
    }
    prop.push(Surface::Wood, planks);

    // 2: riveted steel: two overlapping plates with rivets round the edges and the seam.
    let c = Vec2::new(3.0, 0.0);
    let mut steel = Mesh::default();
    for (k, (x0, x1, y)) in [
        (-h + 0.005, 0.03, top - 0.012),
        (-0.03, h - 0.005, top - 0.004),
    ]
    .iter()
    .enumerate()
    {
        let poly = vec![
            Vec2::new(c.x + x0, -h + 0.005),
            Vec2::new(c.x + x1, -h + 0.005),
            Vec2::new(c.x + x1, h - 0.005),
            Vec2::new(c.x + x0, h - 0.005),
        ];
        let mut m = Slab::new(Outline::new(poly).detail(0.1), y - 0.02, *y, 0.005)
            .detail(4)
            .mesh();
        // A few dents.
        let seed = k as u64 * 3 + 1;
        warp(&mut m, |p| {
            let d = noise1(p, 3.0, seed, 2).min(0.0) * 0.004;
            p + Vec3::Y * d * smoothstep(y - 0.01, *y, p.y)
        });
        steel.merge(&m);
        for t in 0..10 {
            let s = -h + 0.06 + (2.0 * h - 0.12) * t as f32 / 9.0;
            for (px, pz) in [(c.x + x0 + 0.035, s), (c.x + x1 - 0.035, s)] {
                nails.merge(&moved(rivet(0.011), Vec3::new(px, *y, pz), Quat::IDENTITY));
            }
        }
        for t in 0..5 {
            let s = c.x + x0 + 0.1 + (x1 - x0 - 0.2) * t as f32 / 4.0;
            for pz in [-h + 0.035, h - 0.035] {
                nails.merge(&moved(rivet(0.011), Vec3::new(s, *y, pz), Quat::IDENTITY));
            }
        }
    }
    paint(&mut steel, |p, _| {
        let rust = smoothstep(0.1, 0.6, noise1(p, 4.0, 77, 3));
        Vec3::new(1.0 - 0.25 * rust, 1.0 - 0.4 * rust, 1.0 - 0.55 * rust)
    });
    nails.merge(&steel);
    prop.push(Surface::Iron, nails);

    // 3: dirt, lumpy, with clods and pebbles.
    let c = Vec2::new(4.5, 0.0);
    let mut dirt = heightfield(c - Vec2::splat(h), c + Vec2::splat(h), 0.03, |p| {
        top - 0.004 + n2(p, 4.0, 51, 3) * 0.008 + n2(p, 12.0, 52, 2) * 0.003
    });
    paint(&mut dirt, |p, _| {
        Vec3::splat(0.8 + 0.2 * n2(Vec2::new(p.x, p.z), 6.0, 53, 2))
    });
    let mut clods = Mesh::default();
    for _ in 0..30 {
        let at = c + Vec2::new(rng.range(-h, h), rng.range(-h, h)) * 0.92;
        let size = rng.range(0.03, 0.08);
        let shape = RockShape::new(Vec3::new(size, size * 0.6, size), rng.next_u64())
            .detail(2)
            .lumps(0.25)
            .cuts(3);
        let at = Vec3::new(at.x, top + size * 0.08, at.y);
        clods.merge(&moved(
            rock(shape),
            at,
            Quat::from_rotation_y(rng.range(0.0, 6.3)),
        ));
    }
    dirt.merge(&clods);
    prop.push(Surface::Dirt, dirt);
    let mut grit = Mesh::default();
    for _ in 0..25 {
        let at = c + Vec2::new(rng.range(-h, h), rng.range(-h, h)) * 0.92;
        let size = rng.range(0.025, 0.05);
        let p = pebble(&mut rng, size);
        grit.merge(&moved(
            p,
            Vec3::new(at.x, top - 0.002, at.y),
            Quat::from_rotation_y(rng.range(0.0, 6.3)),
        ));
    }
    prop.push(Surface::Rock, grit);

    // 4: snow, piled softly against the frame.
    let c = Vec2::new(6.0, 0.0);
    let mut snow = heightfield(c - Vec2::splat(h), c + Vec2::splat(h), 0.02, |p| {
        let d = (h - (p - c).abs().max_element()).max(0.0);
        let bank = (1.0 - smoothstep(0.0, 0.2, d)) * 0.03;
        let q = Vec2::new(p.x * 0.5 + p.y * 0.3, p.y * 1.5);
        let ridge = 1.0 - n2(q, 2.5, 61, 3).abs() * 2.0;
        top + bank + ridge.max(-1.0) * 0.007 + n2(p, 1.5, 62, 2) * 0.006
    });
    paint(&mut snow, |p, n| {
        let crest = smoothstep(top - 0.004, top + 0.012, p.y);
        let slope = smoothstep(0.9, 0.995, n.y);
        Vec3::new(0.8 + 0.2 * crest, 0.86 + 0.14 * crest, 1.0) * (0.8 + 0.2 * slope)
    });
    prop.push(Surface::Snow, snow);

    // 5: sand, rippled.
    let c = Vec2::new(7.5, 0.0);
    let sand = heightfield(c - Vec2::splat(h), c + Vec2::splat(h), 0.015, |p| {
        let w = n2(p, 1.5, 71, 2) * 0.3;
        let s = ((p.x * 0.3 + p.y + w) / 0.09 * std::f32::consts::TAU).sin();
        top - 0.004 + (s * 0.5 + 0.5).powf(1.6) * 0.004 + n2(p, 2.5, 72, 2) * 0.004
    });
    prop.push(Surface::Sand, sand);
    prop
}
