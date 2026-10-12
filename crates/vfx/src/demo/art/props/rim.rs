//! Around the rim: wrought-iron braziers on stone footings, and leafy shrubs.

use engine_core::math::{Quat, Rng, Vec2, Vec3};
use engine_render::Mesh;

use super::super::meshes::metal::{band, bar, scroll_path};
use super::super::meshes::nature::{RockShape, bush, grass_tuft, rock};
use super::super::meshes::{
    Outline, Profile, colored, icosphere, moved, noise1, paint, revolve, sweep, tube_loop,
};
use super::super::{Prop, Surface};
use super::dress;

/// Height of the coal bed in the brazier (where the flame sits).
pub const COALS: f32 = 2.05;

/// A brazier: an octagonal stone footing, a twisted iron post with collars, scrollwork, and a
/// bowl of iron straps and rings holding glowing coals.
pub fn brazier() -> Prop {
    let mut rng = Rng::new(0xB2A2);
    let mut stone = Mesh::default();
    let mut iron = Mesh::default();
    let mut embers = Mesh::default();
    let mut charcoal = Mesh::default();

    // Footing.
    let foot = Profile::new()
        .corner(0.05, -0.02)
        .to(0.05, 0.1)
        .arc(0.03, 0.1, 0.02, 0.0, 90.0, 2)
        .corner(0.03, 0.12)
        .corner(0.0, 0.12)
        .scale(0.62)
        .corner(0.0, 0.22)
        .scale(0.0)
        .to(0.0, 0.22);
    let mut m = sweep(
        &Outline::regular(8, 0.26, std::f32::consts::PI / 8.0),
        &foot,
    );
    dress(&mut m, 9, 1.4);
    stone.merge(&m);

    // Post.
    let post: Vec<Vec3> = (0..=40)
        .map(|i| Vec3::Y * (0.18 + 1.6 * i as f32 / 40.0))
        .collect();
    iron.merge(&bar(&post, 0.024, 4.0));
    for (y, r) in [(0.24, 0.045), (1.0, 0.04), (1.72, 0.045)] {
        iron.merge(&moved(
            band(r - 0.012, 0.05, 0.012, 16),
            Vec3::Y * y,
            Quat::IDENTITY,
        ));
    }

    // Bowl: rings and straps.
    let ring = |r: f32, y: f32| -> Vec<Vec3> {
        (0..32)
            .map(|i| {
                let a = i as f32 / 32.0 * std::f32::consts::TAU;
                Vec3::new(a.cos() * r, y, a.sin() * r)
            })
            .collect()
    };
    for (r, y, w) in [(0.07, 1.78, 0.012), (0.18, 1.97, 0.01), (0.25, 2.11, 0.013)] {
        iron.merge(&tube_loop(&ring(r, y), Vec2::new(w, w * 1.4), 6));
    }
    for k in 0..8 {
        let a = k as f32 / 8.0 * std::f32::consts::TAU;
        let d = Vec3::new(a.cos(), 0.0, a.sin());
        let path: Vec<Vec3> = (0..=10)
            .map(|i| {
                let t = i as f32 / 10.0;
                let r = 0.07 + 0.18 * t.powf(0.8) + 0.03 * (t * 10.0 - 8.5).max(0.0).powi(2) * 0.1;
                d * r + Vec3::Y * (1.78 + 0.36 * t + 0.03 * (t * 10.0 - 9.0).max(0.0))
            })
            .collect();
        iron.merge(&bar(&path, 0.008, 0.0));
        // Curled tips.
        let curl = scroll_path(0.03, 0.008, 0.85, std::f32::consts::PI, 10);
        let tip = *path.last().unwrap();
        let rot = Quat::from_rotation_arc(Vec3::Z, Vec3::new(-d.z, 0.0, d.x));
        let curl: Vec<Vec3> = curl
            .iter()
            .map(|p| tip + rot * (*p + Vec3::X * 0.03))
            .collect();
        iron.merge(&bar(&curl, 0.006, 0.0));
    }
    // Scrolls bracing the bowl from the post.
    for k in 0..4 {
        let a = (k as f32 + 0.5) / 4.0 * std::f32::consts::TAU;
        let d = Vec3::new(a.cos(), 0.0, a.sin());
        let s = scroll_path(0.09, 0.015, 1.1, -std::f32::consts::FRAC_PI_2, 24);
        let rot = Quat::from_rotation_arc(Vec3::Z, Vec3::new(-d.z, 0.0, d.x));
        let path: Vec<Vec3> = s
            .iter()
            .map(|p| Vec3::Y * 1.66 + d * 0.11 + rot * Vec3::new(p.x, p.y, 0.0))
            .collect();
        iron.merge(&bar(&path, 0.007, 0.0));
    }
    // Liner and coals.
    let liner = Profile::new()
        .to(0.0, 1.8)
        .to(0.07, 1.8)
        .to(0.18, 1.96)
        .to(0.21, 2.0);
    let mut inner = Profile::new();
    for p in liner.pts.iter().rev() {
        inner = inner.to(p.o, p.y + 0.005);
    }
    iron.merge(&colored(revolve(&inner, 24), Vec3::splat(0.4)));
    iron.merge(&colored(revolve(&liner, 24), Vec3::splat(0.6)));
    for i in 0..26 {
        let r = 0.16 * (i as f32 / 26.0).sqrt();
        let a = i as f32 * 2.4;
        let y = 1.97 + 0.07 * (1.0 - (r / 0.17).powi(2)) + rng.range(-0.01, 0.01);
        let size = rng.range(0.045, 0.075);
        let mut lump = icosphere(1);
        let seed = rng.next_u64();
        for v in &mut lump.vertices {
            let d = Vec3::from(v.position);
            v.position = (d * size * 0.5 * (1.0 + 0.3 * noise1(d, 1.5, seed, 2))).to_array();
        }
        lump.compute_normals();
        let at = Vec3::new(a.cos() * r, y, a.sin() * r);
        let lump = moved(lump, at, Quat::from_rotation_y(a));
        if i % 3 == 0 {
            charcoal.merge(&lump);
        } else {
            embers.merge(&lump);
        }
    }

    let mut prop = Prop::new();
    prop.push(Surface::CarvedStone, stone);
    prop.push(Surface::Iron, iron);
    prop.push(Surface::RuneEmber, embers);
    prop.push(Surface::DarkStone, colored(charcoal, Vec3::splat(0.25)));
    prop
}

/// A clump of shrubs (three bushes, grass and a stone or two) around the origin. `variant`
/// picks one of a few arrangements.
pub fn shrubs(variant: u64) -> Prop {
    let mut rng = Rng::new(0x5A8B + variant * 977);
    let mut wood = Mesh::default();
    let mut leaves = Mesh::default();
    let mut grass = Mesh::default();
    let mut stones = Mesh::default();
    let spots = [
        (Vec3::new(-0.9, 0.0, 0.0), 0.85, 1.25),
        (Vec3::new(0.05, 0.0, 0.55), 0.72, 1.05),
        (Vec3::new(0.95, 0.0, -0.1), 0.6, 0.85),
    ];
    for (k, (at, radius, height)) in spots.iter().enumerate() {
        let leaves_n = (2200.0 * radius * radius) as u32;
        let (w, l) = bush(rng.next_u64() + k as u64, *radius, *height, leaves_n, 0.075);
        let rot = Quat::from_rotation_y(rng.range(0.0, 6.3));
        wood.merge(&moved(w, *at, rot));
        leaves.merge(&moved(l, *at, rot));
    }
    for _ in 0..40 {
        let a = rng.range(0.0, std::f32::consts::TAU);
        let r = rng.range(0.3, 2.0);
        let (n, h) = (rng.range_i32(6, 12) as u32, rng.range(0.12, 0.35));
        let t = grass_tuft(&mut rng, n, h);
        grass.merge(&moved(
            t,
            Vec3::new(a.cos() * r * 1.3, 0.0, a.sin() * r * 0.8),
            Quat::IDENTITY,
        ));
    }
    for _ in 0..2 {
        let s = rng.range(0.25, 0.45);
        let shape = RockShape::new(Vec3::new(s * 1.3, s * 0.6, s), rng.next_u64()).detail(2);
        let at = Vec3::new(rng.range(-1.6, 1.6), s * 0.1, rng.range(0.6, 1.0));
        stones.merge(&moved(
            rock(shape),
            at,
            Quat::from_rotation_y(rng.range(0.0, 6.3)),
        ));
    }
    paint(&mut wood, |_, _| Vec3::new(0.7, 0.62, 0.55));
    let mut prop = Prop::new();
    prop.push(Surface::Wood, wood);
    prop.push(Surface::Leaves, leaves);
    prop.push(Surface::Moss, grass);
    prop.push(Surface::Rock, stones);
    prop
}
