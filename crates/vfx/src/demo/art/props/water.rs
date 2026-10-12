//! Water and healing: the stepped healing dais, the tiered fountain and the pond.

use engine_core::math::{Quat, Rng, Vec2, Vec3, smoothstep};
use engine_render::{Mesh, Vertex};

use super::super::meshes::nature::{
    RockShape, grass_tuft, lichen, moss_cap, moss_cushion, pebble, reeds, rock,
};
use super::super::meshes::stone::{Stone, bend_to_ring, ring_courses, rune_line};
use super::super::meshes::{Outline, Profile, grime, moved, noise1, paint, revolve, warp};
use super::super::{Prop, Surface};
use super::{dress, shade};

/// Height of the dais' top (the healing aura sits on it).
pub const DAIS_TOP: f32 = 0.265;
/// Height of the fountain's water surface.
pub const FOUNTAIN_WATER: f32 = 0.52;
/// Height of the fountain's nozzle (where the jet starts).
pub const NOZZLE: f32 = 1.12;
/// Height of the pond's water surface.
pub const POND_WATER: f32 = 0.07;

/// Paint vertical joint lines on a round piece: `count` joints, `width` meters wide.
fn joints(m: &mut Mesh, count: u32, phase: f32, width: f32) {
    paint(m, |p, _| {
        let r = Vec2::new(p.x, p.z).length().max(0.05);
        let a = p.z.atan2(p.x) + phase;
        let step = std::f32::consts::TAU / count as f32;
        let d = ((a / step).round() * step - a).abs() * r;
        Vec3::splat(0.45 + 0.55 * smoothstep(width * 0.3, width, d))
    });
}

/// The healing dais: a ring step and a raised ring of carved blocks around a round center
/// stone with a dark flower inlay; a glowing joint and a ring of life runes.
pub fn dais() -> Prop {
    let mut rng = Rng::new(0xDA15);
    let mut carved = Mesh::default();
    let mut dark = Mesh::default();
    let mut runes = Mesh::default();
    let mut moss = Mesh::default();
    for (r0, r1, top, len) in [
        (1.78, 2.2, 0.13, (0.9, 1.2)),
        (1.17, 1.78, DAIS_TOP, (0.8, 1.1)),
    ] {
        for (_, poly) in ring_courses(&mut rng, r0, r1, (r1 - r0, r1 - r0), len, 0.012) {
            let mut s = Stone::new(poly, -0.02, top + rng.range(-0.004, 0.002), 0.028);
            s.tilt = Vec2::new(rng.range(-0.004, 0.004), rng.range(-0.004, 0.004));
            let mut m = s.mesh();
            let k = shade(&mut rng, 0.1);
            paint(&mut m, |_, _| k);
            dress(&mut m, rng.next_u64(), 1.2);
            carved.merge(&m);
        }
    }
    let mut center = Stone::new(
        Outline::regular(40, 1.15, 0.0).pts,
        0.0,
        DAIS_TOP + 0.003,
        0.03,
    );
    center.detail = 0;
    let mut m = center.mesh();
    dress(&mut m, 77, 1.0);
    carved.merge(&m);
    grime(&mut carved, 0.0, 0.18, 0.6);

    // A dark six-petaled flower inlaid in the center.
    for k in 0..6 {
        let a = k as f32 / 6.0 * std::f32::consts::TAU;
        let d = Vec2::from_angle(a);
        let s = Vec2::new(-d.y, d.x);
        let poly = vec![d * 0.1, d * 0.32 + s * 0.11, d * 0.5, d * 0.32 - s * 0.11];
        let mut st = Stone::new(poly, DAIS_TOP - 0.01, DAIS_TOP + 0.006, 0.006);
        st.segs = 1;
        st.detail = 0;
        let mut m = st.mesh();
        dress(&mut m, k as u64, 0.6);
        dark.merge(&m);
    }
    let mut boss = Stone::new(
        Outline::regular(16, 0.1, 0.0).pts,
        DAIS_TOP - 0.01,
        DAIS_TOP + 0.008,
        0.01,
    );
    boss.dome = 0.003;
    dark.merge(&boss.mesh());

    // Glowing joint around the center stone and a ring of runes on the raised ring.
    let glow = Profile::new()
        .corner(1.162, DAIS_TOP - 0.01)
        .corner(1.162, DAIS_TOP + 0.001)
        .corner(1.148, DAIS_TOP + 0.001)
        .corner(1.148, DAIS_TOP - 0.01);
    runes.merge(&revolve(&glow, 96));
    let mut line = rune_line(&mut rng, 30, 0.22, false);
    let circumference = std::f32::consts::TAU * 1.47;
    let natural = 30.0 * 0.22 / 1.4 * 1.6;
    for v in &mut line.vertices {
        v.position[0] = v.position[0] * circumference / natural;
    }
    bend_to_ring(&mut line, 1.47, DAIS_TOP + 0.004);
    runes.merge(&line);

    for _ in 0..7 {
        let a = rng.range(0.0, std::f32::consts::TAU);
        let r = rng.range(2.1, 2.25);
        let s = rng.range(0.06, 0.13);
        let c = moss_cushion(&mut rng, s);
        moss.merge(&moved(
            c,
            Vec3::new(a.cos() * r, 0.02, a.sin() * r),
            Quat::IDENTITY,
        ));
    }

    let mut prop = Prop::new();
    prop.push(Surface::CarvedStone, carved);
    prop.push(Surface::DarkStone, dark);
    prop.push(Surface::RuneLife, runes);
    prop.push(Surface::Moss, moss);
    prop
}

/// The fountain: a round basin with a molded rim on a plinth step, a baluster column rising
/// from the water to a lobed upper bowl, and a bronze spout whose nozzle is at [`NOZZLE`].
pub fn fountain() -> Prop {
    let mut carved = Mesh::default();
    let mut water = Mesh::default();
    let mut bronze = Mesh::default();
    let w = FOUNTAIN_WATER;
    // Plinth step, basin wall with base and rim moldings, inner wall, floor.
    let basin = Profile::new()
        .corner(2.64, -0.02)
        .to(2.64, 0.09)
        .arc(2.61, 0.09, 0.03, 0.0, 90.0, 2)
        .corner(2.5, 0.12)
        .arc(2.46, 0.165, 0.045, -90.0, 90.0, 5)
        .corner(2.42, 0.21)
        .to(2.4, 0.3)
        .to(2.405, 0.4)
        .corner(2.41, 0.42)
        .arc(2.41, 0.46, 0.04, -90.0, 0.0, 3)
        .corner(2.45, 0.46)
        .corner(2.47, 0.46)
        .to(2.47, 0.52)
        .arc(2.39, 0.52, 0.08, 0.0, 90.0, 4)
        .to(2.27, 0.6)
        .arc(2.27, 0.56, 0.04, 90.0, 180.0, 3)
        .to(2.23, 0.45)
        .to(2.23, 0.15)
        .corner(2.2, 0.12)
        .to(0.0, 0.12);
    let mut m = revolve(&basin, 128);
    joints(&mut m, 10, 0.1, 0.014);
    dress(&mut m, 5, 1.3);
    grime(&mut m, 0.0, 0.2, 0.55);
    // A tide line just above the water inside, and wet darkening below it.
    paint(&mut m, |p, _| {
        let r = Vec2::new(p.x, p.z).length();
        if r < 2.25 && r > 2.1 {
            Vec3::splat(if p.y < w + 0.03 { 0.6 } else { 0.95 })
        } else {
            Vec3::ONE
        }
    });
    carved.merge(&m);

    // Baluster column and the lobed upper bowl.
    let column = Profile::new()
        .corner(0.42, 0.11)
        .to(0.42, 0.18)
        .arc(0.38, 0.2, 0.045, -30.0, 90.0, 4)
        .corner(0.3, 0.245)
        .to(0.21, 0.32)
        .to(0.2, 0.4)
        .to(0.25, 0.52)
        .to(0.28, 0.6)
        .to(0.25, 0.68)
        .to(0.16, 0.75)
        .to(0.13, 0.79)
        .corner(0.13, 0.8)
        .to(0.3, 0.83)
        .to(0.5, 0.88)
        .to(0.6, 0.93)
        .arc(0.6, 0.955, 0.025, -90.0, 90.0, 4)
        .corner(0.6, 0.98)
        .to(0.54, 0.985)
        .to(0.52, 0.95)
        .to(0.45, 0.91)
        .to(0.0, 0.9);
    let mut m = revolve(&column, 64);
    // Lobes (gadroons) on the bowl's underside.
    warp(&mut m, |p| {
        let r = Vec2::new(p.x, p.z).length();
        let band = smoothstep(0.8, 0.86, p.y) * (1.0 - smoothstep(0.92, 0.95, p.y));
        let a = p.z.atan2(p.x);
        let lobe = (0.5 + 0.5 * (a * 16.0).cos()).powi(2);
        let k = 1.0 + 0.06 * lobe * band * (r / 0.6).min(1.0);
        Vec3::new(p.x * k, p.y, p.z * k)
    });
    dress(&mut m, 6, 1.4);
    carved.merge(&m);

    // Water: the basin and the bowl.
    for (r, y) in [(2.235, w), (0.53, 0.955)] {
        let disc = Profile::new().to(r, y).to(r * 0.6, y).to(0.0, y);
        let mut d = revolve(&disc, 96);
        for v in &mut d.vertices {
            v.uv = [v.position[0], v.position[2]];
        }
        water.merge(&d);
    }

    // Bronze spout: a lotus bud on a stem, the nozzle at the top.
    let spout = Profile::new()
        .corner(0.11, 0.89)
        .to(0.1, 0.93)
        .to(0.065, 0.97)
        .to(0.07, 1.0)
        .to(0.085, 1.025)
        .to(0.07, 1.05)
        .to(0.038, 1.075)
        .to(0.034, 1.095)
        .to(0.046, 1.11)
        .corner(0.044, NOZZLE)
        .corner(0.02, NOZZLE)
        .to(0.016, NOZZLE - 0.02)
        .to(0.0, NOZZLE - 0.025);
    let mut m = revolve(&spout, 24);
    // Petals: lobes on the bud.
    warp(&mut m, |p| {
        let a = p.z.atan2(p.x);
        let band = smoothstep(0.93, 0.97, p.y) * (1.0 - smoothstep(1.04, 1.07, p.y));
        let k = 1.0 + 0.18 * band * (0.5 + 0.5 * (a * 6.0).cos()).powi(3);
        Vec3::new(p.x * k, p.y, p.z * k)
    });
    paint(&mut m, |p, _| {
        Vec3::splat(0.75 + 0.35 * smoothstep(0.95, 1.1, p.y))
    });
    bronze.merge(&m);

    let mut prop = Prop::new();
    prop.push(Surface::CarvedStone, carved);
    prop.push(Surface::Water, water);
    prop.push(Surface::Bronze, bronze);
    prop
}

/// The pond: a muddy hollow inside an earthen bank, rimmed with mossy boulders half sunk into
/// the bank, grass and pebbles between them, reeds at the edge and lily pads on the water
/// ([`POND_WATER`] high, about 3 m in radius; stones drop within 2.2 m of the middle).
pub fn pond() -> Prop {
    let mut rng = Rng::new(0x9011D);
    let mut rocks = Mesh::default();
    let mut moss = Mesh::default();
    let mut mud = Mesh::default();
    let mut bank = Mesh::default();
    let mut water = Mesh::default();
    let mut leaves = Mesh::default();

    // Height of the bank's crest line at angle `a` wobbles a little.
    let crest = |a: f32| 3.32 + 0.12 * noise1(Vec3::new(a.cos(), 0.0, a.sin()), 1.3, 77, 2);

    // Mud bottom, deepest (and darkest) in the middle, rising to meet the bank under water.
    let mut prof = Profile::new();
    for i in (0..=16).rev() {
        let r = i as f32 / 16.0 * 3.1;
        let y = 0.032 + 0.034 * smoothstep(1.2, 3.1, r);
        prof = prof.to(r, y);
    }
    let mut m = revolve(&prof, 96);
    warp(&mut m, |p| p + Vec3::Y * (noise1(p, 1.3, 4, 3) * 0.006));
    paint(&mut m, |p, _| {
        let r = Vec2::new(p.x, p.z).length();
        Vec3::splat(0.5 + 0.5 * smoothstep(0.5, 3.0, r))
    });
    for v in &mut m.vertices {
        v.uv = [v.position[0], v.position[2]];
    }
    mud.merge(&m);

    // The bank: up out of the water to a crest, down under the paving outside.
    let prof = Profile::new()
        .to(4.0, 0.025)
        .to(3.78, 0.045)
        .to(3.6, 0.1)
        .to(3.42, 0.145)
        .to(3.25, 0.15)
        .to(3.1, 0.115)
        .to(2.98, 0.075)
        .to(2.85, 0.05);
    let shape_bank = |p: Vec3| {
        let a = p.z.atan2(p.x);
        let r = Vec2::new(p.x, p.z).length();
        let shift = crest(a) - 3.32;
        let k = (r + shift) / r.max(1e-3);
        let lump = noise1(p, 2.2, 8, 3) * 0.02 * smoothstep(0.03, 0.1, p.y);
        Vec3::new(p.x * k, p.y + lump, p.z * k)
    };
    let mut m = revolve(&prof, 160);
    warp(&mut m, shape_bank);
    paint(&mut m, |p, n| {
        // Damp and dark at the waterline, drier on the crest.
        let wet = 1.0 - smoothstep(POND_WATER, POND_WATER + 0.05, p.y);
        Vec3::splat((0.85 - 0.35 * wet) * (0.8 + 0.2 * n.y))
    });
    for v in &mut m.vertices {
        v.uv = [v.position[0], v.position[2]];
    }
    bank.merge(&m);
    // Turf over the crest and the outer slope (the boulders hide its inner edge, the paving
    // its outer one).
    let turf = Profile::new()
        .to(3.95, 0.03)
        .to(3.78, 0.05)
        .to(3.6, 0.105)
        .to(3.42, 0.15)
        .to(3.25, 0.155)
        .to(3.12, 0.11);
    let mut m = revolve(&turf, 160);
    warp(&mut m, |p| {
        let a = p.z.atan2(p.x);
        // A ragged inner edge, tucked into the bank.
        let r = Vec2::new(p.x, p.z).length();
        let ragged = if r < 3.15 {
            noise1(p, 5.0, 12, 2).abs() * 0.12
        } else {
            0.0
        };
        let q = Vec3::new(a.cos() * (r + ragged), p.y, a.sin() * (r + ragged));
        shape_bank(q)
    });
    paint(&mut m, |p, _| {
        Vec3::splat(0.75 + 0.35 * noise1(p, 3.0, 13, 2).abs())
    });
    for v in &mut m.vertices {
        v.uv = [v.position[0], v.position[2]];
    }
    moss.merge(&m);

    let disc = Profile::new()
        .to(3.05, POND_WATER)
        .to(2.0, POND_WATER)
        .to(0.0, POND_WATER);
    let mut d = revolve(&disc, 96);
    for v in &mut d.vertices {
        v.uv = [v.position[0], v.position[2]];
    }
    water.merge(&d);

    // Boulders along the crest, nearly touching, sunk into the bank; a few in the shallows.
    let mut a = 0.0f32;
    while a < std::f32::consts::TAU - 0.2 {
        let big = rng.chance(0.5);
        let s = if big {
            rng.range(0.7, 1.0)
        } else {
            rng.range(0.42, 0.6)
        };
        let r = crest(a) + rng.range(-0.12, 0.1);
        let size = Vec3::new(s * rng.range(1.1, 1.5), s * rng.range(0.55, 0.8), s);
        let shape = RockShape::new(size, rng.next_u64())
            .detail(if big { 3 } else { 2 })
            .cuts(rng.range_i32(5, 9) as u32)
            .floor(-0.6);
        let at = Vec3::new(
            a.cos() * r,
            0.11 + size.y * rng.range(0.12, 0.25),
            a.sin() * r,
        );
        // Long side along the rim.
        let rot = Quat::from_rotation_y(-a + rng.range(-0.3, 0.3))
            * Quat::from_rotation_z(rng.range(-0.12, 0.12));
        let mut b = moved(rock(shape), at, rot);
        grime(&mut b, POND_WATER, 0.1, 0.5);
        lichen(&mut b, 0.5, rng.next_u64());
        moss.merge(&moss_cap(&b, 0.85, 0.2, 0.006, rng.next_u64()));
        rocks.merge(&b);
        let step = size.x * 0.8 / r;
        a += step + rng.range(0.0, 0.05);
    }
    for _ in 0..7 {
        let a = rng.range(0.0, std::f32::consts::TAU);
        let r = rng.range(2.75, 2.95);
        let s = rng.range(0.22, 0.38);
        let size = Vec3::new(s * 1.3, s * 0.6, s);
        let shape = RockShape::new(size, rng.next_u64()).detail(2).floor(-0.5);
        let at = Vec3::new(a.cos() * r, POND_WATER - size.y * 0.1, a.sin() * r);
        let mut b = moved(rock(shape), at, Quat::from_rotation_y(rng.range(0.0, 6.3)));
        grime(&mut b, POND_WATER - 0.02, 0.06, 0.45);
        rocks.merge(&b);
    }
    // Pebbles in the shallows and on the bank.
    for _ in 0..90 {
        let a = rng.range(0.0, std::f32::consts::TAU);
        let r = rng.range(2.6, 3.9);
        let size = rng.range(0.05, 0.14);
        // In the shallows and under the paving low, on the crest of the bank high.
        let y = if (3.0..=3.6).contains(&r) { 0.12 } else { 0.06 };
        let p = pebble(&mut rng, size);
        rocks.merge(&moved(
            p,
            Vec3::new(a.cos() * r, y, a.sin() * r),
            Quat::from_rotation_y(rng.range(0.0, 6.3)),
        ));
    }
    // Grass and moss on the bank, between and behind the boulders.
    for _ in 0..380 {
        let a = rng.range(0.0, std::f32::consts::TAU);
        let r = crest(a) + rng.range(-0.15, 0.45);
        let y = if r > 3.6 { 0.07 } else { 0.13 };
        let n = rng.range_i32(6, 12) as u32;
        let h = rng.range(0.1, 0.32);
        let t = grass_tuft(&mut rng, n, h);
        moss.merge(&moved(
            t,
            Vec3::new(a.cos() * r, y, a.sin() * r),
            Quat::from_rotation_y(rng.range(0.0, 6.3)),
        ));
    }
    for _ in 0..40 {
        let a = rng.range(0.0, std::f32::consts::TAU);
        let r = crest(a) + rng.range(-0.2, 0.3);
        let size = rng.range(0.12, 0.3);
        let c = moss_cushion(&mut rng, size);
        moss.merge(&moved(
            c,
            Vec3::new(a.cos() * r, 0.1, a.sin() * r),
            Quat::from_rotation_y(rng.range(0.0, 6.3)),
        ));
    }
    // Reeds in three clumps at the water's edge.
    for (a, n) in [(0.6f32, 46), (2.9, 34), (4.4, 52)] {
        let r = 2.9;
        let h = rng.range(0.9, 1.3);
        let c = reeds(&mut rng, n, h, 0.35);
        leaves.merge(&moved(
            c,
            Vec3::new(a.cos() * r, 0.05, a.sin() * r),
            Quat::IDENTITY,
        ));
    }
    // Lily pads, each a disc with a notch, outside where the stones drop.
    for _ in 0..11 {
        let a = rng.range(0.0, std::f32::consts::TAU);
        let r = rng.range(2.3, 2.8);
        let size = rng.range(0.11, 0.2);
        let pad = lily_pad(&mut rng, size);
        leaves.merge(&moved(
            pad,
            Vec3::new(a.cos() * r, POND_WATER + 0.002, a.sin() * r),
            Quat::from_rotation_y(rng.range(0.0, 6.3)),
        ));
    }

    let mut prop = Prop::new();
    prop.push(Surface::Mud, mud);
    prop.push(Surface::Dirt, bank);
    prop.push(Surface::Water, water);
    prop.push(Surface::Rock, rocks);
    prop.push(Surface::Moss, moss);
    prop.push(Surface::Leaves, leaves);
    prop
}

/// A lily pad of `radius`, facing up, with a notch to its center and a slightly raised rim.
fn lily_pad(rng: &mut Rng, radius: f32) -> Mesh {
    let n = 20;
    let notch = 0.35;
    let mut m = Mesh::default();
    m.vertices
        .push(Vertex::new(Vec3::ZERO, Vec3::Y, Vec2::ZERO));
    for i in 0..=n {
        let a = notch * 0.5 + (std::f32::consts::TAU - notch) * i as f32 / n as f32;
        let r = radius * rng.range(0.95, 1.03);
        let p = Vec3::new(a.cos() * r, 0.006, a.sin() * r);
        m.vertices
            .push(Vertex::new(p, Vec3::Y, Vec2::new(p.x, p.z)));
    }
    for i in 0..n {
        m.indices.extend_from_slice(&[0, i + 2, i + 1]);
    }
    m.compute_normals();
    let k = rng.range(0.75, 1.1);
    paint(&mut m, |p, _| {
        let r = Vec2::new(p.x, p.z).length() / radius;
        Vec3::new(1.0 + 0.15 * r, 1.0, 0.9) * k
    });
    m
}
