//! The arcane gate: a round arch of voussoirs with a keystone on two coursed pillars, on a
//! stepped platform, with glowing rune inlays. Its front faces +Z.

use engine_core::math::{Quat, Rng, Vec2, Vec3};
use engine_render::Mesh;

use super::super::meshes::nature::{RockShape, moss_cushion, rock};
use super::super::meshes::stone::{rune, rune_line, strokes_mesh, voussoirs};
use super::super::meshes::{
    Outline, Profile, Slab, block, grime, moved, noise_warp, paint, revolve_part, sweep,
};
use super::super::{Prop, Surface};
use super::{dress, shade};

/// Height of the platform the pillars stand on.
pub const PLATFORM: f32 = 0.54;
/// Where the arch springs from the capitals.
pub const SPRING: f32 = 4.4;
/// Pillar centers (±X).
pub const PILLAR_X: f32 = 3.0;
/// Inner and outer radius of the arch ring.
pub const ARCH: (f32, f32) = (2.5, 3.5);

/// The whole gate: platform, pillars, arch, runes.
pub fn gate() -> Prop {
    let mut rng = Rng::new(0x9A7E);
    let mut carved = Mesh::default();
    let mut dark = Mesh::default();
    let mut runes = Mesh::default();
    let mut rubble = Mesh::default();
    let mut moss = Mesh::default();

    platform(&mut rng, &mut carved, &mut moss);
    for side in [-1.0f32, 1.0] {
        let x = side * PILLAR_X;
        pillar(
            &mut rng,
            Vec3::new(x, PLATFORM, 0.0),
            &mut carved,
            &mut dark,
            &mut runes,
        );
    }
    arch(&mut rng, &mut carved, &mut runes);

    // Fallen pieces at the platform's foot.
    for (x, z, s) in [
        (-5.3, 1.6, 0.42),
        (5.5, -0.9, 0.55),
        (4.9, 2.1, 0.3),
        (-5.6, -1.4, 0.36),
    ] {
        let shape = RockShape::new(Vec3::new(s * 1.3, s * 0.8, s), rng.next_u64())
            .detail(2)
            .cuts(6)
            .lumps(0.08);
        let r = rock(shape);
        rubble.merge(&moved(
            r,
            Vec3::new(x, s * 0.22, z),
            Quat::from_rotation_y(rng.range(0.0, 6.3)),
        ));
    }

    let mut prop = Prop::new();
    prop.push(Surface::CarvedStone, carved);
    prop.push(Surface::DarkStone, dark);
    prop.push(Surface::RuneArcane, runes);
    prop.push(Surface::Rock, rubble);
    prop.push(Surface::Moss, moss);
    prop
}

/// Three steps all around, each a course of blocks.
fn platform(rng: &mut Rng, carved: &mut Mesh, moss: &mut Mesh) {
    let rise = PLATFORM / 3.0;
    for k in 0..3 {
        let hx = 5.0 - 0.4 * k as f32;
        let hz = 1.8 - 0.4 * k as f32;
        let (y0, y1) = (rise * k as f32 - 0.05, rise * (k + 1) as f32);
        // Blocks along X, two rows deep.
        let mut x = -hx;
        while x < hx - 0.01 {
            let mut len = rng.range(0.8, 1.4);
            if hx - (x + len) < 0.6 {
                len = hx - x;
            }
            for (z0, z1) in [(-hz, 0.0), (0.0, hz)] {
                let poly = rect(x + 0.006, z0 + 0.006, x + len - 0.006, z1 - 0.006);
                let mut m = Slab::new(Outline::new(poly), y0, y1 + rng.range(-0.006, 0.004), 0.025)
                    .open_bottom()
                    .mesh();
                let tilt = Quat::from_rotation_x(rng.range(-0.004, 0.004))
                    * Quat::from_rotation_z(rng.range(-0.004, 0.004));
                m = moved(m, Vec3::ZERO, tilt);
                let k = shade(rng, 0.1);
                paint(&mut m, |_, _| k);
                grime(&mut m, y0 + 0.05, 0.12, 0.6);
                dress(&mut m, rng.next_u64(), 1.0);
                carved.merge(&m);
                // Moss on the treads' back corners.
                if rng.chance(0.3) {
                    let at = Vec3::new(
                        x + rng.range(0.0, len),
                        y1,
                        if z0 < 0.0 { -hz } else { hz } * 0.98,
                    );
                    let s = rng.range(0.06, 0.14);
                    let c = moss_cushion(rng, s);
                    moss.merge(&moved(c, at - Vec3::Y * 0.01, Quat::IDENTITY));
                }
            }
            x += len;
        }
    }
}

fn rect(x0: f32, z0: f32, x1: f32, z1: f32) -> Vec<Vec2> {
    vec![
        Vec2::new(x0, z0),
        Vec2::new(x1, z0),
        Vec2::new(x1, z1),
        Vec2::new(x0, z1),
    ]
}

/// A square pillar: molded base, a shaft of stacked drums with a rune panel front and back,
/// and a molded capital. `at` is the center of its foot.
fn pillar(rng: &mut Rng, at: Vec3, carved: &mut Mesh, dark: &mut Mesh, runes: &mut Mesh) {
    let half = 0.45;
    let square = Outline::rect(half, half);
    let y = |v: f32| v - PLATFORM;
    // Base: plinth, torus, fillet, cavetto into the shaft.
    let base = Profile::new()
        .corner(0.17, y(0.54))
        .to(0.17, y(0.71))
        .arc(0.155, y(0.71), 0.015, 0.0, 90.0, 2)
        .corner(0.12, y(0.725))
        .arc(0.09, y(0.785), 0.06, -90.0, 90.0, 6)
        .corner(0.06, y(0.845))
        .corner(0.06, y(0.865))
        .arc(0.06, y(0.925), 0.06, -90.0, -180.0, 4)
        .corner(0.0, y(0.925))
        .to(0.0, y(0.98))
        .scale(0.0)
        .to(0.0, y(0.98));
    let mut m = sweep(&square, &base);
    dress(&mut m, rng.next_u64(), 1.2);
    grime(&mut m, 0.0, 0.3, 0.65);
    carved.merge(&moved(m, at, Quat::IDENTITY));

    // Shaft drums.
    let (s0, s1) = (0.98, 3.84);
    let drums = 4;
    let mut y0 = s0;
    for i in 0..drums {
        let y1 = if i + 1 == drums {
            s1
        } else {
            s0 + (s1 - s0) * (i + 1) as f32 / drums as f32 + rng.range(-0.08, 0.08)
        };
        let h = y1 - y0 - 0.008;
        let mut d = block(Vec3::new(half * 2.0 - 0.01, h, half * 2.0 - 0.01), 0.03);
        noise_warp(&mut d, 0.006, 2.5, rng.next_u64());
        let k = shade(rng, 0.08);
        paint(&mut d, |_, _| k);
        dress(&mut d, rng.next_u64(), 1.0);
        let c = at + Vec3::new(0.0, (y0 + y1) * 0.5 - PLATFORM, 0.0);
        carved.merge(&moved(
            d,
            c,
            Quat::from_rotation_y(rng.range(-0.006, 0.006)),
        ));
        y0 = y1;
    }

    // Rune panels front and back: dark stone, raised a little, with a column of runes.
    let mid = (s0 + s1) * 0.5 - PLATFORM;
    for face in [1.0f32, -1.0] {
        let panel = block(Vec3::new(0.36, 2.3, 0.04), 0.012);
        let mut panel = panel;
        dress(&mut panel, rng.next_u64(), 0.8);
        let rot = if face > 0.0 {
            Quat::IDENTITY
        } else {
            Quat::from_rotation_y(std::f32::consts::PI)
        };
        dark.merge(&moved(
            panel,
            at + rot * Vec3::new(0.0, mid, half + 0.005),
            rot,
        ));
        let col = rune_line(rng, 6, 0.24, true);
        runes.merge(&moved(
            col,
            at + rot * Vec3::new(0.0, mid, half + 0.026),
            rot,
        ));
    }

    // Capital: astragal, neck, echinus, abacus.
    let cap = Profile::new()
        .corner(0.0, y(3.84))
        .to(0.0, y(3.85))
        .arc(0.0, y(3.89), 0.04, -90.0, 90.0, 4)
        .corner(0.0, y(3.93))
        .corner(0.0, y(4.06))
        .arc(0.0, y(4.2), 0.14, -90.0, 0.0, 5)
        .corner(0.14, y(4.2))
        .corner(0.18, y(4.2))
        .to(0.18, y(4.37))
        .arc(0.15, y(4.37), 0.03, 0.0, 90.0, 2)
        .corner(0.15, y(SPRING))
        .scale(0.0)
        .to(0.0, y(SPRING));
    let mut m = sweep(&square, &cap);
    dress(&mut m, rng.next_u64(), 1.2);
    carved.merge(&moved(m, at, Quat::IDENTITY));
}

/// The arch: voussoirs with a keystone, a molded archivolt, a rune on every stone and a
/// glowing line along the soffit.
fn arch(rng: &mut Rng, carved: &mut Mesh, runes: &mut Mesh) {
    let depth = 0.9;
    let (r_in, r_out) = ARCH;
    let spring = Vec3::new(0.0, SPRING, 0.0);
    let count = 13;
    for (i, (mut m, angle)) in voussoirs(r_in, r_out, depth, count, 0.012, (0.32, 0.07), 0.03)
        .into_iter()
        .enumerate()
    {
        noise_warp(&mut m, 0.005, 2.5, i as u64 + 40);
        let k = shade(rng, 0.08);
        paint(&mut m, |_, _| k);
        dress(&mut m, rng.next_u64(), 1.1);
        carved.merge(&moved(m, spring, Quat::IDENTITY));
        // A rune on each face of the stone, upright along its radius.
        let keystone = i == count as usize / 2;
        let size = if keystone { 0.5 } else { 0.32 };
        let r_mid = if keystone {
            (r_in + r_out + 0.32) * 0.5
        } else {
            (r_in + r_out) * 0.5
        };
        let out = if keystone { 0.07 } else { 0.0 };
        for face in [1.0f32, -1.0] {
            let strokes: Vec<(Vec2, Vec2)> = if keystone {
                // The keystone carries a ringed sigil.
                let mut s = Vec::new();
                for j in 0..8 {
                    let a0 = j as f32 / 8.0 * std::f32::consts::TAU;
                    let a1 = (j + 1) as f32 / 8.0 * std::f32::consts::TAU;
                    s.push((Vec2::from_angle(a0) * 0.42, Vec2::from_angle(a1) * 0.42));
                }
                s.extend(rune(rng).into_iter().map(|(a, b)| {
                    let f = |p: Vec2| (p - Vec2::new(0.5, 0.7)) * 0.45;
                    (f(a), f(b))
                }));
                s.into_iter().map(|(a, b)| (a * size, b * size)).collect()
            } else {
                rune(rng)
                    .into_iter()
                    .map(|(a, b)| {
                        let f = |p: Vec2| (p - Vec2::new(0.5, 0.7)) * (size / 1.4);
                        (f(a), f(b))
                    })
                    .collect()
            };
            let g = strokes_mesh(&strokes, size * 0.09);
            let up = Quat::from_rotation_z(angle - std::f32::consts::FRAC_PI_2);
            let flip = if face > 0.0 {
                Quat::IDENTITY
            } else {
                Quat::from_rotation_y(std::f32::consts::PI)
            };
            let c = spring
                + Vec3::new(angle.cos() * r_mid, angle.sin() * r_mid, 0.0)
                + Vec3::Z * face * (depth * 0.5 + out + 0.003);
            runes.merge(&moved(g, c, up * flip));
        }
    }
    // Archivolt: a molded band around the extrados, front and back.
    let band = Profile::new()
        .corner(r_out - 0.01, depth * 0.5 - 0.05)
        .corner(r_out + 0.1, depth * 0.5 - 0.05)
        .arc(r_out + 0.1, depth * 0.5 - 0.0, 0.05, -90.0, 90.0, 4)
        .corner(r_out - 0.01, depth * 0.5 + 0.05);
    for face in [1.0f32, -1.0] {
        let mut m = revolve_part(&band, 0.0, std::f32::consts::PI, 48);
        // Revolve space (x, z) is the arch plane (x, y); its height (y) runs along Z.
        m = moved(
            m,
            Vec3::ZERO,
            Quat::from_rotation_x(-std::f32::consts::FRAC_PI_2),
        );
        if face < 0.0 {
            m = moved(m, Vec3::ZERO, Quat::from_rotation_y(std::f32::consts::PI));
        }
        dress(&mut m, rng.next_u64(), 1.0);
        carved.merge(&moved(m, spring, Quat::IDENTITY));
    }
    // A glowing line along the soffit.
    let line = Profile::new()
        .corner(r_in - 0.006, 0.03)
        .corner(r_in - 0.006, -0.03)
        .corner(r_in + 0.004, -0.03)
        .corner(r_in + 0.004, 0.03);
    let mut m = revolve_part(&line, 0.0, std::f32::consts::PI, 64);
    m = moved(
        m,
        spring,
        Quat::from_rotation_x(-std::f32::consts::FRAC_PI_2),
    );
    runes.merge(&m);
}

/// Top of the summoning floor: just under the portal's ground rune circle (drawn 5 cm up).
pub const SUMMONING_TOP: f32 = 0.04;

/// The summoning floor in front of the gate: a disc of polished dark stone in two rings around
/// a round center stone, inside a carved curb with eight glowing studs. Radius 3.6 m.
pub fn summoning_circle() -> Prop {
    use super::super::meshes::stone::{Stone, ring_courses};
    let mut rng = Rng::new(0x5C1C);
    let mut dark = Mesh::default();
    let mut carved = Mesh::default();
    let mut runes = Mesh::default();
    let mut center = Stone::new(
        Outline::regular(24, 0.8, 0.0).pts,
        0.0,
        SUMMONING_TOP,
        0.015,
    );
    center.detail = 2;
    dark.merge(&center.mesh());
    for (r0, r1, len) in [(0.8, 2.0, (0.9, 1.2)), (2.0, 3.3, (1.0, 1.4))] {
        for (_, poly) in ring_courses(&mut rng, r0, r1, (r1 - r0, r1 - r0), len, 0.01) {
            let mut s = Stone::new(poly, 0.0, SUMMONING_TOP + rng.range(-0.002, 0.001), 0.012);
            s.tilt = Vec2::new(rng.range(-0.002, 0.002), rng.range(-0.002, 0.002));
            let mut m = s.mesh();
            let k = shade(&mut rng, 0.06) * 1.1;
            paint(&mut m, |_, _| k);
            dark.merge(&m);
        }
    }
    dress(&mut dark, 31, 0.6);
    for (_, poly) in ring_courses(&mut rng, 3.3, 3.62, (0.32, 0.32), (0.9, 1.2), 0.012) {
        let mut s = Stone::new(poly, -0.02, 0.075 + rng.range(-0.004, 0.003), 0.025);
        s.tilt = Vec2::new(rng.range(-0.004, 0.004), rng.range(-0.004, 0.004));
        let mut m = s.mesh();
        let k = shade(&mut rng, 0.1);
        paint(&mut m, |_, _| k);
        dress(&mut m, rng.next_u64(), 1.2);
        carved.merge(&m);
    }
    for k in 0..8 {
        let a = k as f32 / 8.0 * std::f32::consts::TAU;
        let stud = Slab::new(Outline::regular(4, 0.07, a).rounded(1), 0.07, 0.082, 0.004).mesh();
        runes.merge(&moved(
            stud,
            Vec3::new(a.cos(), 0.0, a.sin()) * 3.46,
            Quat::IDENTITY,
        ));
    }
    let mut prop = Prop::new();
    prop.push(Surface::DarkStone, dark);
    prop.push(Surface::CarvedStone, carved);
    prop.push(Surface::RuneArcane, runes);
    prop
}
