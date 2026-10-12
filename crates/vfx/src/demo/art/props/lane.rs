//! The middle lane: the caster's carved pedestal with its floating focus crystal, and the
//! target wall the spells hit.

use engine_core::math::{Quat, Rng, Vec2, Vec3};
use engine_render::Mesh;

use super::super::meshes::nature::{RockShape, crystal, grass_tuft, moss_cap, moss_cushion, rock};
use super::super::meshes::stone::{sigil, strokes_mesh};
use super::super::meshes::{
    Outline, Profile, Slab, block, grime, moved, noise_warp, paint, revolve, sweep, tube,
};
use super::super::{Prop, Surface};
use super::{dress, shade};

/// Height of the pedestal's top.
pub const PEDESTAL_TOP: f32 = 1.08;

/// The caster's pedestal: a molded square die on a base slab, rune panels on its faces and a
/// bronze cradle reaching up toward the focus that floats above it.
pub fn caster_pedestal() -> Prop {
    let mut rng = Rng::new(0xCA57);
    let mut carved = Mesh::default();
    let mut dark = Mesh::default();
    let mut runes = Mesh::default();
    let mut bronze = Mesh::default();

    let mut base = block(Vec3::new(1.1, 0.16, 1.1), 0.03);
    base = moved(base, Vec3::Y * 0.06, Quat::IDENTITY);
    dress(&mut base, 1, 1.2);
    grime(&mut base, 0.0, 0.12, 0.6);
    carved.merge(&base);

    let half = 0.3;
    let prof = Profile::new()
        .corner(0.12, 0.14)
        .to(0.12, 0.28)
        .arc(0.1, 0.28, 0.02, 0.0, 90.0, 2)
        .corner(0.09, 0.3)
        .arc(0.06, 0.345, 0.045, -90.0, 90.0, 5)
        .corner(0.04, 0.39)
        .arc(0.04, 0.43, 0.04, -90.0, -180.0, 3)
        .corner(0.0, 0.43)
        .corner(0.0, 0.9)
        .arc(0.04, 0.9, 0.04, 180.0, 90.0, 3)
        .corner(0.04, 0.94)
        .corner(0.07, 0.94)
        .arc(0.07, 1.0, 0.06, -90.0, 0.0, 4)
        .corner(0.13, 1.0)
        .to(0.13, 1.06)
        .arc(0.11, 1.06, 0.02, 0.0, 90.0, 2)
        .corner(0.11, PEDESTAL_TOP)
        .scale(0.0)
        .to(0.0, PEDESTAL_TOP);
    let mut die = sweep(&Outline::rect(half, half), &prof);
    dress(&mut die, 2, 1.3);
    grime(&mut die, 0.14, 0.25, 0.7);
    carved.merge(&die);

    // A framed panel with a glowing sigil on each face of the die.
    for k in 0..4 {
        let rot = Quat::from_rotation_y(k as f32 * std::f32::consts::FRAC_PI_2);
        let mut panel = block(Vec3::new(0.38, 0.36, 0.03), 0.01);
        dress(&mut panel, 10 + k, 0.8);
        dark.merge(&moved(panel, rot * Vec3::new(0.0, 0.665, half), rot));
        for (size, at) in [
            (Vec3::new(0.46, 0.035, 0.04), Vec3::new(0.0, 0.8625, 0.0)),
            (Vec3::new(0.46, 0.035, 0.04), Vec3::new(0.0, 0.4675, 0.0)),
            (Vec3::new(0.035, 0.36, 0.04), Vec3::new(-0.2125, 0.665, 0.0)),
            (Vec3::new(0.035, 0.36, 0.04), Vec3::new(0.2125, 0.665, 0.0)),
        ] {
            let mut m = block(size, 0.008);
            dress(&mut m, rng.next_u64(), 1.0);
            carved.merge(&moved(m, rot * (at + Vec3::Z * half), rot));
        }
        let g = strokes_mesh(&sigil(&mut rng, 0.12), 0.016);
        runes.merge(&moved(g, rot * Vec3::new(0.0, 0.665, half + 0.0165), rot));
    }

    // Bronze cradle: a dish and three claws reaching up.
    let dish = Profile::new()
        .corner(0.0, PEDESTAL_TOP - 0.01)
        .corner(0.12, PEDESTAL_TOP - 0.01)
        .arc(0.12, PEDESTAL_TOP + 0.04, 0.05, -90.0, 0.0, 3)
        .to(0.2, PEDESTAL_TOP + 0.05)
        .arc(0.2, PEDESTAL_TOP + 0.065, 0.015, -90.0, 90.0, 3)
        .corner(0.18, PEDESTAL_TOP + 0.08)
        .arc(0.12, PEDESTAL_TOP + 0.08, 0.06, 0.0, -90.0, 3)
        .to(0.0, PEDESTAL_TOP + 0.02);
    bronze.merge(&revolve(&dish, 32));
    for k in 0..3 {
        let a = k as f32 / 3.0 * std::f32::consts::TAU + 0.5;
        let d = Vec3::new(a.cos(), 0.0, a.sin());
        let path: Vec<Vec3> = (0..=8)
            .map(|i| {
                let t = i as f32 / 8.0;
                let r = 0.17 + 0.05 * (t * 3.0).min(1.0) - 0.12 * t * t;
                d * r + Vec3::Y * (PEDESTAL_TOP + 0.07 + t * 0.32)
            })
            .collect();
        bronze.merge(&tube(&path, |t| 0.018 * (1.0 - t * 0.85), 6, true));
    }

    let mut prop = Prop::new();
    prop.push(Surface::CarvedStone, carved);
    prop.push(Surface::DarkStone, dark);
    prop.push(Surface::RuneEmber, runes);
    prop.push(Surface::Bronze, bronze);
    prop
}

/// The caster's focus: a crystal inside a tilted bronze ring, centered on the origin.
pub fn focus_crystal() -> Prop {
    let mut rng = Rng::new(0xF0C5);
    let gem = crystal(&mut rng, 0.07, 0.34, 6);
    let mut shards = Mesh::default();
    shards.merge(&gem);
    for k in 0..3 {
        let a = k as f32 / 3.0 * std::f32::consts::TAU;
        let s = crystal(&mut rng, 0.022, 0.1, 5);
        let rot = Quat::from_rotation_y(a) * Quat::from_rotation_z(0.5);
        shards.merge(&moved(
            s,
            Vec3::new(a.cos() * 0.13, -0.08, a.sin() * 0.13),
            rot,
        ));
    }
    let ring: Vec<Vec3> = (0..40)
        .map(|i| {
            let a = i as f32 / 40.0 * std::f32::consts::TAU;
            Vec3::new(a.cos() * 0.2, 0.0, a.sin() * 0.2)
        })
        .collect();
    let band = super::super::meshes::tube_loop(&ring, Vec2::new(0.006, 0.014), 6);
    let band = moved(
        band,
        Vec3::ZERO,
        Quat::from_rotation_x(0.35) * Quat::from_rotation_z(0.2),
    );
    Prop::new()
        .with(Surface::Crystal, shards)
        .with(Surface::Bronze, band)
}

/// The target wall: dressed ashlar courses on a plinth course with a coping, its far end
/// broken away, sooty where the spells land. Front face on x = 0 facing -X, 0.8 m thick
/// toward +X, 4.8 m long along Z.
pub fn target_wall() -> Prop {
    let mut rng = Rng::new(0x3A11);
    let mut carved = Mesh::default();
    let mut mortar = Mesh::default();
    let mut rubble = Mesh::default();
    let mut moss = Mesh::default();
    let thick = 0.8;
    let (z0, z1) = (-2.4, 2.4);
    // How high the wall still stands along its length.
    let standing = |z: f32| {
        if z < 0.7 {
            3.05
        } else {
            3.05 - (z - 0.7) / (z1 - 0.7) * 1.5
        }
    };
    let mut courses = vec![(0.0, 0.42, true)];
    let mut y = 0.42;
    while y < 2.8 {
        let mut h = rng.range(0.36, 0.46);
        if 2.85 - (y + h) < 0.3 {
            h = 2.85 - y;
        }
        courses.push((y, y + h, false));
        y += h;
    }
    let coping = (y, y + 0.2);
    let mut tops: Vec<(Vec3, f32)> = Vec::new();
    for (ci, (cy0, cy1, plinth)) in courses.iter().copied().enumerate() {
        let mut z = z0
            + if ci % 2 == 0 {
                0.0
            } else {
                -rng.range(0.2, 0.45)
            };
        let mut lo = f32::MAX;
        let mut hi = f32::MIN;
        let mut low_top = cy1;
        while z < z1 {
            let mut len = rng.range(0.55, 1.1);
            let a = z.max(z0);
            if z1 - (z + len) < 0.3 {
                len = z1 - z;
            }
            let b = (z + len).min(z1);
            z += len;
            let mid = (a + b) * 0.5;
            if cy0 > standing(mid) - 0.15 {
                continue;
            }
            // Broken blocks near the ragged top are shorter.
            let top = cy1
                .min(standing(mid) + rng.range(-0.1, 0.05))
                .max(cy0 + 0.15);
            lo = lo.min(a);
            hi = hi.max(b);
            low_top = low_top.min(top);
            let out = if plinth {
                0.05
            } else {
                rng.range(-0.012, 0.018)
            };
            let poly = vec![
                Vec2::new(-out, a + 0.006),
                Vec2::new(thick + out, a + 0.006),
                Vec2::new(thick + out, b - 0.006),
                Vec2::new(-out, b - 0.006),
            ];
            let mut m = Slab::new(
                Outline::new(poly).detail(0.07),
                cy0 + 0.005,
                top - 0.005,
                rng.range(0.012, 0.026),
            )
            .detail(5)
            .open_bottom()
            .mesh();
            // Hewn faces: a gentle bulge and fine tool marks.
            noise_warp(&mut m, 0.01, 2.2, rng.next_u64());
            noise_warp(&mut m, 0.0035, 8.0, rng.next_u64());
            let tilt = Quat::from_rotation_x(rng.range(-0.008, 0.008))
                * Quat::from_rotation_z(rng.range(-0.006, 0.006));
            let c = Vec3::new(thick * 0.5, (cy0 + top) * 0.5, mid);
            m = moved(moved(m, -c, Quat::IDENTITY), c, tilt);
            let k = shade(&mut rng, 0.14);
            paint(&mut m, |_, _| k);
            dress(&mut m, rng.next_u64(), 1.3);
            carved.merge(&m);
            if top < cy1 - 0.01 || cy1 > standing(mid) - 0.5 {
                tops.push((Vec3::new(thick * 0.5, top, mid), b - a));
            }
        }
        if hi > lo {
            let top = low_top - 0.03;
            let core = block(Vec3::new(thick - 0.06, top - cy0, hi - lo - 0.02), 0.0);
            mortar.merge(&moved(
                core,
                Vec3::new(thick * 0.5, (cy0 + top) * 0.5, (lo + hi) * 0.5),
                Quat::IDENTITY,
            ));
        }
    }
    // Coping along the standing part.
    let mut z = z0;
    while z < 0.9 {
        let len = rng.range(0.9, 1.3).min(1.0 - z);
        let poly = vec![
            Vec2::new(-0.06, z + 0.006),
            Vec2::new(thick + 0.06, z + 0.006),
            Vec2::new(thick + 0.06, z + len - 0.006),
            Vec2::new(-0.06, z + len - 0.006),
        ];
        let mut m = Slab::new(Outline::new(poly).detail(0.08), coping.0, coping.1, 0.03)
            .detail(3)
            .open_bottom()
            .mesh();
        noise_warp(&mut m, 0.01, 2.0, rng.next_u64());
        noise_warp(&mut m, 0.003, 8.0, rng.next_u64());
        dress(&mut m, rng.next_u64(), 1.4);
        carved.merge(&m);
        tops.push((Vec3::new(thick * 0.5, coping.1, z + len * 0.5), len));
        z += len;
    }
    // Soot where the spells land (1.5 m up, mid-wall), grime at the foot.
    paint(&mut carved, |p, _| {
        let d2 = ((p.y - 1.5) / 0.85).powi(2) + (p.z / 0.95).powi(2);
        let front = (1.0 - p.x / 0.3).clamp(0.0, 1.0);
        Vec3::splat(1.0 - 0.72 * (-d2).exp() * front)
    });
    grime(&mut carved, 0.0, 0.5, 0.55);
    paint(&mut mortar, |_, _| Vec3::splat(0.6));

    // Fallen blocks by the broken end, moss on exposed tops, grass at the foot.
    for (dz, dx, s) in [(2.9, -0.3, 0.42), (3.3, 0.6, 0.36), (2.7, 1.1, 0.3)] {
        let outline = Outline::rect(s * 0.85, s * 0.55).detail(0.07);
        let mut m = Slab::new(outline, -s * 0.5, s * 0.5, 0.025)
            .detail(4)
            .mesh();
        noise_warp(&mut m, 0.015, 2.0, (dz * 10.0) as u64);
        noise_warp(&mut m, 0.004, 8.0, (dz * 11.0) as u64);
        dress(&mut m, (dz * 7.0) as u64, 1.5);
        let rot = Quat::from_rotation_y(rng.range(0.0, 6.3))
            * Quat::from_rotation_x(rng.range(-0.25, 0.25));
        carved.merge(&moved(m, Vec3::new(dx, s * 0.42, dz), rot));
    }
    for _ in 0..5 {
        let shape = RockShape::new(Vec3::splat(rng.range(0.12, 0.25)), rng.next_u64()).detail(1);
        let p = Vec3::new(rng.range(-0.6, 1.4), 0.03, rng.range(2.2, 3.6));
        rubble.merge(&moved(rock(shape), p, Quat::IDENTITY));
    }
    // Moss on the exposed tops, following the stone, and a few cushions in the corners.
    moss.merge(&moss_cap(&carved, 0.8, 0.4, 0.006, 0x3A12));
    for (at, len) in tops {
        if rng.chance(0.4) {
            let s = rng.range(0.08, 0.16);
            let c = moss_cushion(&mut rng, s);
            let off = Vec3::new(rng.range(-0.3, 0.3), -0.015, rng.range(-len, len) * 0.4);
            moss.merge(&moved(
                c,
                at + off,
                Quat::from_rotation_y(rng.range(0.0, 6.3)),
            ));
        }
    }
    for i in 0..40 {
        let z = z0 + (z1 - z0) * (i as f32 + rng.range(0.0, 1.0)) / 40.0;
        let x = if i % 2 == 0 {
            rng.range(-0.15, -0.05)
        } else {
            rng.range(0.85, 0.95)
        };
        let h = rng.range(0.12, 0.3);
        let t = grass_tuft(&mut rng, 7, h);
        moss.merge(&moved(
            t,
            Vec3::new(x, 0.04, z),
            Quat::from_rotation_y(rng.range(0.0, 6.3)),
        ));
    }

    let mut prop = Prop::new();
    prop.push(Surface::CarvedStone, carved);
    prop.push(Surface::Dirt, mortar);
    prop.push(Surface::Rock, rubble);
    prop.push(Surface::Moss, moss);
    prop
}
