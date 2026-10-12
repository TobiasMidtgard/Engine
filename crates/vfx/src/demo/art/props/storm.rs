//! A storm obelisk: stepped base, molded die, a tapering shaft of three drums carved with
//! storm runes, and a bronze crown holding the crystal the lightning arcs from.

use engine_core::math::{Quat, Rng, Vec2, Vec3};
use engine_render::Mesh;

use super::super::meshes::metal::rivet;
use super::super::meshes::nature::{crystal, moss_cushion};
use super::super::meshes::stone::{rune, rune_line, strokes_mesh};
use super::super::meshes::{
    Outline, Profile, Slab, block, grime, moved, noise_warp, paint, sweep, tube,
};
use super::super::{Prop, Surface};
use super::{dress, shade};

/// Where the crystal's center is (the lightning's end), meters above the ground.
pub const CRYSTAL_Y: f32 = 4.6;

/// One obelisk; `seed` varies its runes and wear.
pub fn obelisk(seed: u64) -> Prop {
    let mut rng = Rng::new(0x0BE1 ^ seed);
    let mut carved = Mesh::default();
    let mut dark = Mesh::default();
    let mut runes = Mesh::default();
    let mut bronze = Mesh::default();
    let mut moss = Mesh::default();

    // Two steps, each of two blocks.
    for (k, (half, y0, y1)) in [(0.8f32, -0.04f32, 0.2f32), (0.6, 0.2, 0.4)]
        .iter()
        .enumerate()
    {
        for side in [-1.0f32, 1.0] {
            let x0 = if side < 0.0 { -half } else { 0.004 };
            let x1 = if side < 0.0 { -0.004 } else { *half };
            let poly = vec![
                Vec2::new(x0, -half),
                Vec2::new(x1, -half),
                Vec2::new(x1, *half),
                Vec2::new(x0, *half),
            ];
            let mut m = Slab::new(Outline::new(poly), *y0, *y1, 0.03)
                .open_bottom()
                .mesh();
            let s = shade(&mut rng, 0.08);
            paint(&mut m, |_, _| s);
            grime(&mut m, 0.0, 0.25, 0.6);
            dress(&mut m, rng.next_u64(), 1.2);
            carved.merge(&m);
        }
        if k == 0 {
            for _ in 0..4 {
                let a = rng.range(0.0, std::f32::consts::TAU);
                let at = Vec3::new(a.cos(), 0.0, a.sin()) * 0.62 + Vec3::Y * 0.19;
                let s = rng.range(0.07, 0.15);
                moss.merge(&moved(moss_cushion(&mut rng, s), at, Quat::IDENTITY));
            }
        }
    }

    // Die with base and cornice moldings, a sigil on each face.
    let half = 0.36;
    let die = Profile::new()
        .corner(0.08, 0.4)
        .to(0.08, 0.5)
        .arc(0.06, 0.5, 0.02, 0.0, 90.0, 2)
        .corner(0.05, 0.52)
        .arc(0.05, 0.57, 0.05, -90.0, -180.0, 3)
        .corner(0.0, 0.57)
        .corner(0.0, 0.98)
        .arc(0.05, 0.98, 0.05, 180.0, 90.0, 3)
        .corner(0.05, 1.03)
        .corner(0.08, 1.03)
        .to(0.08, 1.1)
        .arc(0.06, 1.1, 0.02, 0.0, 90.0, 2)
        .corner(0.06, 1.12)
        .scale(0.0)
        .to(0.0, 1.12);
    let mut m = sweep(&Outline::rect(half, half), &die);
    dress(&mut m, rng.next_u64(), 1.3);
    grime(&mut m, 0.4, 0.3, 0.75);
    carved.merge(&m);
    for k in 0..4 {
        let rot = Quat::from_rotation_y(k as f32 * std::f32::consts::FRAC_PI_2);
        let strokes: Vec<(Vec2, Vec2)> = rune(&mut rng)
            .into_iter()
            .chain([
                (Vec2::new(-0.3, -0.2), Vec2::new(0.5, -0.45)),
                (Vec2::new(0.5, -0.45), Vec2::new(1.3, -0.2)),
            ])
            .map(|(a, b)| {
                let f = |p: Vec2| (p - Vec2::new(0.5, 0.7)) * 0.2;
                (f(a), f(b))
            })
            .collect();
        let g = strokes_mesh(&strokes, 0.03);
        runes.merge(&moved(g, rot * Vec3::new(0.0, 0.79, half + 0.002), rot));
    }

    // Shaft: three tapering drums.
    let (s0, s1) = (1.12, 4.0);
    let (w0, w1) = (0.27, 0.19);
    let width = |y: f32| w0 + (w1 - w0) * (y - s0) / (s1 - s0);
    let cuts = [
        s0,
        2.2 + rng.range(-0.1, 0.1),
        3.15 + rng.range(-0.1, 0.1),
        s1,
    ];
    for pair in cuts.windows(2) {
        let (y0, y1) = (pair[0], pair[1]);
        let (a, b) = (width(y0), width(y1));
        let mut m = Slab::new(Outline::rect(a, a), y0 + 0.004, y1 - 0.004, 0.012)
            .taper(b / a)
            .detail(1)
            .mesh();
        noise_warp(&mut m, 0.0025, 3.0, rng.next_u64());
        let s = shade(&mut rng, 0.07);
        paint(&mut m, |_, _| s);
        dress(&mut m, rng.next_u64(), 1.1);
        carved.merge(&m);
    }
    // Bronze bands over the joints between drums.
    for &y in &cuts[1..3] {
        let w = width(y) + 0.012;
        let mut m = Slab::new(Outline::rect(w, w), y - 0.035, y + 0.035, 0.008).mesh();
        paint(&mut m, |_, n| Vec3::splat(0.8 + 0.25 * n.y.max(0.0)));
        bronze.merge(&m);
        for k in 0..4 {
            let rot = Quat::from_rotation_y(k as f32 * std::f32::consts::FRAC_PI_2);
            for x in [-0.6f32, 0.6] {
                let at = rot * Vec3::new(x * w, y, w);
                bronze.merge(&moved(
                    rivet(0.011),
                    at,
                    rot * Quat::from_rotation_x(std::f32::consts::FRAC_PI_2),
                ));
            }
        }
    }
    // Dark stone panels down each face of the lower two drums, laid on the slope, carved with
    // columns of storm runes.
    let slope = ((w0 - w1) / (s1 - s0)).atan();
    for pair in cuts[..3].windows(2) {
        let mid = (pair[0] + pair[1]) * 0.5;
        let tall = (pair[1] - pair[0]) * 0.5 - 0.08;
        let size = 0.14;
        let count = ((tall * 2.0 - 0.06) / (size * 1.35)).floor().max(1.0) as u32;
        for k in 0..4 {
            let rot = Quat::from_rotation_y(k as f32 * std::f32::consts::FRAC_PI_2);
            let lay = rot * Quat::from_rotation_x(-slope);
            let face = rot * Vec3::new(0.0, mid, width(mid));
            let mut panel = Slab::new(Outline::rect(0.1, tall), -0.004, 0.008, 0.005).mesh();
            panel = moved(
                panel,
                Vec3::ZERO,
                Quat::from_rotation_x(std::f32::consts::FRAC_PI_2),
            );
            dress(&mut panel, rng.next_u64(), 0.8);
            dark.merge(&moved(panel, face, lay));
            let col = rune_line(&mut rng, count, size, true);
            runes.merge(&moved(col, face + lay * Vec3::Z * 0.0095, lay));
        }
    }

    // Bronze crown: a molded collar and four claws around the crystal.
    let collar = Profile::new()
        .corner(-0.004, 3.98)
        .corner(0.03, 3.98)
        .arc(0.03, 4.02, 0.04, -90.0, 0.0, 3)
        .corner(0.07, 4.02)
        .to(0.07, 4.1)
        .arc(0.04, 4.1, 0.03, 0.0, 90.0, 3)
        .corner(0.04, 4.13)
        .corner(0.0, 4.16)
        .scale(0.6)
        .to(0.0, 4.26)
        .scale(0.0)
        .to(0.0, 4.3);
    let mut m = sweep(&Outline::rect(w1, w1), &collar);
    paint(&mut m, |p, _| {
        Vec3::splat(0.8 + 0.3 * ((p.y - 3.98) / 0.3).clamp(0.0, 1.0))
    });
    bronze.merge(&m);
    for k in 0..4 {
        let a = (k as f32 + 0.5) * std::f32::consts::FRAC_PI_2;
        let d = Vec3::new(a.cos(), 0.0, a.sin());
        let path: Vec<Vec3> = (0..=10)
            .map(|i| {
                let t = i as f32 / 10.0;
                let r = 0.3 + 0.06 * (t * 2.5).min(1.0) - 0.2 * t * t;
                d * r + Vec3::Y * (4.06 + t * 0.78)
            })
            .collect();
        bronze.merge(&tube(&path, |t| 0.036 * (1.0 - t * 0.75), 10, true));
        // A rivet where each claw meets the collar.
        let mut stud = block(Vec3::splat(0.05), 0.015);
        stud = moved(stud, d * 0.29 + Vec3::Y * 4.07, Quat::from_rotation_y(-a));
        bronze.merge(&stud);
    }

    // The crystal, with two small shards.
    let mut gems = Mesh::default();
    gems.merge(&moved(
        crystal(&mut rng, 0.13, 0.62, 6),
        Vec3::Y * CRYSTAL_Y,
        Quat::from_rotation_y(rng.range(0.0, 1.0)),
    ));
    for k in 0..2 {
        let a = k as f32 * 2.6 + 0.4;
        let s = crystal(&mut rng, 0.035, 0.18, 5);
        let rot = Quat::from_rotation_y(a) * Quat::from_rotation_z(0.45);
        gems.merge(&moved(
            s,
            Vec3::new(a.cos() * 0.1, CRYSTAL_Y - 0.2, a.sin() * 0.1),
            rot,
        ));
    }

    let mut prop = Prop::new();
    prop.push(Surface::CarvedStone, carved);
    prop.push(Surface::DarkStone, dark);
    prop.push(Surface::RuneStorm, runes);
    prop.push(Surface::Bronze, bronze);
    prop.push(Surface::Crystal, gems);
    prop.push(Surface::Moss, moss);
    prop
}
