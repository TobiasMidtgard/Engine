//! Market props: crates, barrels and sacks.

use engine_core::prelude::*;
use engine_render::Mesh;

use super::mesh::{bevel_box, icosphere, lathe};
use super::{moved, noise, sstep, tint, vtx};

/// A crate of planks: frame, slats and lid.
pub fn crate_mesh(size: Vec3, rng: &mut Rng) -> Mesh {
    let mut m = Mesh::default();
    let h = size * 0.5;
    let t = 0.035;
    let tone = rng.range(0.85, 1.1);
    let col = Vec4::new(tone * 1.05, tone, tone * 0.92, 1.0);
    // Corner posts.
    for sx in [-1.0, 1.0] {
        for sz in [-1.0, 1.0] {
            let c = Vec3::new(sx * (h.x - t), 0.0, sz * (h.z - t));
            m.merge(&tint(
                moved(
                    bevel_box(Vec3::new(t * 2.0, size.y, t * 2.0), 0.006, 1),
                    Mat4::from_translation(c),
                ),
                col * 0.9,
            ));
        }
    }
    // Side slats.
    let slats = 4;
    for k in 0..slats {
        let y = -h.y + (k as f32 + 0.5) * size.y / slats as f32;
        let sh = size.y / slats as f32 * 0.86;
        let jitter = rng.range(0.9, 1.08);
        for sz in [-1.0, 1.0] {
            m.merge(&tint(
                moved(
                    bevel_box(Vec3::new(size.x - 0.01, sh, 0.022), 0.005, 0),
                    Mat4::from_translation(Vec3::new(0.0, y, sz * (h.z - 0.011))),
                ),
                col * jitter,
            ));
        }
        for sx in [-1.0, 1.0] {
            m.merge(&tint(
                moved(
                    bevel_box(Vec3::new(0.022, sh, size.z - 0.01), 0.005, 2),
                    Mat4::from_translation(Vec3::new(sx * (h.x - 0.011), y, 0.0)),
                ),
                col * jitter * 0.97,
            ));
        }
    }
    // Lid planks.
    let planks = 3;
    for k in 0..planks {
        let z = -h.z + (k as f32 + 0.5) * size.z / planks as f32;
        m.merge(&tint(
            moved(
                bevel_box(
                    Vec3::new(size.x, 0.025, size.z / planks as f32 * 0.94),
                    0.005,
                    0,
                ),
                Mat4::from_translation(Vec3::new(0.0, h.y - 0.012, z)),
            ),
            col * rng.range(0.9, 1.1),
        ));
    }
    m
}

/// A barrel: bulging staves (wood) and iron hoops (returned separately).
pub fn barrel_mesh(height: f32, radius: f32) -> (Mesh, Mesh) {
    let prof: Vec<(f32, f32)> = (0..=16)
        .map(|k| {
            let t = k as f32 / 16.0;
            let bulge = 1.0 - (2.0 * t - 1.0).powi(2);
            (radius * (0.86 + 0.14 * bulge), t * height)
        })
        .collect();
    let mut wood = lathe(&prof, 40, radius);
    // Lid.
    wood.merge(&lathe(
        &[
            (radius * 0.84, height - 0.03),
            (radius * 0.84, height - 0.015),
            (0.0, height - 0.015),
        ],
        40,
        radius,
    ));
    let mut iron = Mesh::default();
    for t in [0.08f32, 0.3, 0.7, 0.92] {
        let bulge = 1.0 - (2.0 * t - 1.0).powi(2);
        let r = radius * (0.86 + 0.14 * bulge) + 0.004;
        let y = t * height;
        iron.merge(&lathe(
            &[
                (r - 0.004, y - 0.025),
                (r, y - 0.022),
                (r, y + 0.022),
                (r - 0.004, y + 0.025),
            ],
            40,
            r,
        ));
    }
    (wood, iron)
}

/// A sack: a squashed, lumpy sphere.
pub fn sack_mesh(size: Vec3, seed: u32) -> Mesh {
    let (dirs, faces) = icosphere(3);
    let mut m = Mesh {
        vertices: dirs
            .iter()
            .map(|&d| {
                let lump = 1.0 + 0.08 * noise::n3(d.x * 2.5, d.y * 2.5, d.z * 2.5, seed);
                let mut p = d * lump * size;
                if p.y < -0.6 * size.y {
                    p.y = -0.6 * size.y;
                }
                // Tied neck.
                if d.y > 0.7 {
                    let k = sstep(0.7, 1.0, d.y);
                    p.x *= 1.0 - 0.7 * k;
                    p.z *= 1.0 - 0.7 * k;
                    p.y += 0.08 * k * size.y;
                }
                let uv = Vec2::new(d.x.atan2(d.z) * 0.3, d.y * 0.4);
                vtx(p, d, uv, Vec4::new(0.95, 0.85, 0.65, 1.0))
            })
            .collect(),
        indices: faces.iter().flat_map(|f| f.iter().copied()).collect(),
    };
    m.compute_normals();
    m
}
