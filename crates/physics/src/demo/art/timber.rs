//! Timber for the float test: sawn beams and a bolted raft, in weathered timber.

use engine_core::math::{Mat4, Vec2, Vec3, Vec4};
use engine_render::Mesh;

use super::kit::MeshBuilder;

/// One sawn timber beam into `b`: half extents `half`, edges eased by 12 mm, grain along its
/// longest side. `seed` shifts its UVs, so beams cut from one texture don't match.
fn beam_into(b: &mut MeshBuilder, half: Vec3, seed: u32, tint: f32) {
    let start = b.mesh.vertices.len();
    b.colored(Vec4::new(tint, tint * 0.98, tint * 0.95, 1.0), |b| {
        b.rounded_box(half, 0.012f32.min(half.min_element() * 0.4), 2)
    });
    let shift = Vec2::new(
        (seed.wrapping_mul(2_654_435_761) >> 16) as f32 / 65_536.0 * 7.0,
        (seed.wrapping_mul(40_503) >> 8 & 0xff) as f32 / 256.0 * 3.0,
    );
    for v in &mut b.mesh.vertices[start..] {
        v.uv = (Vec2::from(v.uv) + shift).to_array();
    }
}

/// A sawn timber beam (a post, a plank) of half extents `half`, in weathered timber.
pub fn timber_beam(half: Vec3, seed: u32) -> Mesh {
    let mut b = MeshBuilder::new();
    beam_into(&mut b, half, seed, 1.0);
    b.build()
}

/// A raft: `beams` timbers of half extents `beam` (long along Z) side by side along X with
/// `gap` between them, and two battens across their tops at z = ±`batten_z`, bolted to each
/// beam.
pub struct Raft {
    /// Weathered timber: the beams and battens.
    pub timber: Mesh,
    /// Forged steel: the carriage bolts' domed heads on the battens.
    pub steel: Mesh,
    /// The collider's parts: (center, half extents), in the raft's frame (its center is the
    /// beams' center).
    pub boxes: Vec<(Vec3, Vec3)>,
}

pub fn raft(beam: Vec3, beams: usize, gap: f32, batten_z: f32) -> Raft {
    let mut timber = MeshBuilder::new();
    let mut steel = MeshBuilder::new();
    let mut boxes = Vec::new();
    let pitch = beam.x * 2.0 + gap;
    let x0 = -pitch * (beams as f32 - 1.0) * 0.5;
    for i in 0..beams {
        let x = x0 + pitch * i as f32;
        // Real beams differ a little in length and color.
        let len = beam.z * (1.0 - 0.025 * ((i * 7 + 3) % 5) as f32);
        let half = Vec3::new(beam.x, beam.y, len);
        let tint = 0.9 + 0.035 * ((i * 3 + 1) % 4) as f32;
        timber.placed(Mat4::from_translation(Vec3::new(x, 0.0, 0.0)), |b| {
            beam_into(b, half, 31 + i as u32 * 17, tint)
        });
        boxes.push((Vec3::new(x, 0.0, 0.0), half));
    }
    let batten = Vec3::new(-x0 + beam.x + 0.04, 0.035, 0.07);
    for (k, z) in [-batten_z, batten_z].into_iter().enumerate() {
        let y = beam.y + batten.y;
        timber.placed(Mat4::from_translation(Vec3::new(0.0, y, z)), |b| {
            beam_into(b, batten, 7 + k as u32, 0.97)
        });
        boxes.push((Vec3::new(0.0, y, z), batten));
        for i in 0..beams {
            let x = x0 + pitch * i as f32;
            let at = Vec3::new(x, y + batten.y, z);
            steel.placed(Mat4::from_translation(at), |b| {
                b.cylinder(0.02, 0.005, 12, 0.004)
            });
        }
    }
    Raft {
        timber: timber.build(),
        steel: steel.build(),
        boxes,
    }
}
