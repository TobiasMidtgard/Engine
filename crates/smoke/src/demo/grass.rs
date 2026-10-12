//! Dry grass: tufts of crossed cards (the grass texture) for the berm, the gravel shoulder
//! around the pad and the odd weed in the joints. Every card is two one-sided quads whose
//! normals lean up, so a tuft lights like a soft clump from both sides.

use std::f32::consts::PI;

use engine_core::math::{Rng, Vec2, Vec3};
use engine_render::Mesh;

use super::art::{self, Parts};
use super::kit;

/// One tuft rooted at the origin, about `h` tall and `w` across.
pub fn tuft(rng: &mut Rng, h: f32, w: f32) -> Mesh {
    let mut m = Mesh::default();
    let cards = 3;
    let yaw0 = rng.range(0.0, PI);
    for k in 0..cards {
        let yaw = yaw0 + k as f32 * PI / cards as f32 + rng.range(-0.25, 0.25);
        // Across the card, and the card's face normal (across × up).
        let dir = Vec3::new(yaw.cos(), 0.0, -yaw.sin());
        let normal = dir.cross(Vec3::Y);
        let lean = rng.range(-0.3, 0.3);
        let flip = rng.f32() < 0.5;
        let ch = h * rng.range(0.75, 1.1);
        let cw = w * rng.range(0.8, 1.15);
        // (height fraction, how far the lean has bent the card there)
        let rows = [(0.0f32, 0.0f32), (0.5, 0.3), (1.0, 1.0)];
        for side in [1.0f32, -1.0] {
            let base = m.vertices.len() as u32;
            for (hf, lf) in rows {
                for i in 0..2 {
                    let s = i as f32 - 0.5;
                    let p = dir * (s * cw * (1.0 + hf * 0.2))
                        + Vec3::Y * (hf * ch)
                        + normal * (lean * ch * lf);
                    let n = (Vec3::Y * 0.75 + normal * (side * 0.45) + dir * (s * 0.3)).normalize();
                    let u = if flip { 0.5 - s } else { 0.5 + s };
                    m.vertices.push(kit::vtx(p, n, Vec2::new(u, 1.0 - hf)));
                }
            }
            for j in 0..2u32 {
                let (a, b, c, d) = (
                    base + j * 2,
                    base + j * 2 + 1,
                    base + j * 2 + 3,
                    base + j * 2 + 2,
                );
                if side > 0.0 {
                    m.indices.extend_from_slice(&[a, b, c, a, c, d]);
                } else {
                    m.indices.extend_from_slice(&[a, c, b, a, d, c]);
                }
            }
        }
    }
    m
}

/// A place for a tuft: root, size (1 ≈ 0.34 m tall) and how green it is (0 dry .. 1 green).
pub type Spot = (Vec3, f32, f32);

/// Plant a tuft at every spot.
pub fn scatter(parts: &mut Parts, spots: &[Spot], seed: u64) {
    let mut rng = Rng::new(seed);
    let mut all = Mesh::default();
    for &(p, size, green) in spots {
        let m = tuft(&mut rng, 0.34 * size, 0.44 * size);
        let tone = rng.range(0.8, 1.1);
        let g = (green + rng.range(-0.2, 0.2)).clamp(0.0, 1.0);
        let tint = Vec3::new(1.08, 1.0, 0.82).lerp(Vec3::new(0.72, 0.95, 0.62), g) * tone;
        let m = kit::tint(kit::placed(m, kit::at(p - Vec3::Y * (0.02 * size))), tint);
        all.merge(&m);
    }
    parts.add(art::GRASS, all);
}
