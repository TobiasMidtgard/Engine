//! Meshes for the counterweight catapult: its frame, its arm and its granite ball.
//!
//! As in [`super::structures`]: one mesh per material, metric UVs, vertex colors near white.
//! The frame is in the station's frame (the slab under the axle, X down range), the arm in its
//! own (the axle, X toward the counterweight, Y out of the cup).

use std::f32::consts::FRAC_PI_2;

use engine_core::math::{Quat, Vec3};
use engine_render::Mesh;

use super::kit::MeshBuilder;
use super::structures::{at, grey, hash, rot_at};
use crate::demo::catapult::{
    self, AXLE, BEAM, CROSSBAR, FRAME_GAP, LONG, SHORT, WEIGHT_HALF, arm_parts, frame_parts,
};

/// The frame's meshes, one per material.
pub struct Frame {
    /// Oak: sills, posts, ties and the crossbar.
    pub timber: Mesh,
    /// Black iron and leather: bearings, axle, straps and the crossbar's pad.
    pub iron: Mesh,
}

/// Along Z (the kit's cylinders stand along Y).
fn along_z() -> Quat {
    Quat::from_rotation_x(FRAC_PI_2)
}

pub fn frame() -> Frame {
    let mut timber = MeshBuilder::new();
    let mut iron = MeshBuilder::new();
    for (k, (center, rotation, half)) in frame_parts().into_iter().enumerate() {
        timber.colored(grey(0.9 + 0.1 * hash(k as u32 + 11)), |b| {
            b.placed(rot_at(rotation, center), |b| b.rounded_box(half, 0.018, 2))
        });
        // An iron strap where each timber meets the next, at both its ends.
        let ends = [-1.0f32, 1.0];
        for e in ends {
            let strap = rotation * Vec3::new(0.0, e * (half.y - 0.12), 0.0);
            iron.placed(rot_at(rotation, center + strap), |b| {
                b.rounded_box(Vec3::new(half.x + 0.012, 0.035, half.z + 0.012), 0.006, 1)
            });
        }
    }
    let side = FRAME_GAP + 0.11;
    // The crossbar: an oak log between the rear posts, padded with leather where the arm hits.
    let bar = catapult::crossbar();
    timber.colored(grey(0.92), |b| {
        b.placed(rot_at(along_z(), bar), |b| {
            b.cylinder(CROSSBAR * 0.8, side + 0.13, 16, 0.02)
        })
    });
    iron.placed(rot_at(along_z(), bar), |b| {
        b.cylinder(CROSSBAR, 0.32, 16, 0.03)
    });
    // The axle through the arm, in iron bearings on the front posts' heads.
    iron.placed(rot_at(along_z(), Vec3::Y * AXLE), |b| {
        b.cylinder(0.055, side + 0.14, 14, 0.01)
    });
    for s in [-1.0f32, 1.0] {
        iron.placed(at(Vec3::new(0.0, AXLE - 0.06, s * side)), |b| {
            b.rounded_box(Vec3::new(0.17, 0.11, 0.14), 0.02, 2)
        });
    }
    Frame {
        timber: timber.build(),
        iron: iron.build(),
    }
}

/// The arm's meshes, one per material.
pub struct Arm {
    /// Oak: the beam and the counterweight's planked box.
    pub timber: Mesh,
    /// Black iron: the cup, the box's bands, the beam's straps and the hub.
    pub iron: Mesh,
}

pub fn arm() -> Arm {
    let mut timber = MeshBuilder::new();
    let mut iron = MeshBuilder::new();
    let parts = arm_parts();
    let (beam_at, beam_half) = parts[0];
    timber.placed(at(beam_at), |b| b.rounded_box(beam_half, 0.02, 2));
    // The counterweight's box, planked: its boards as a slightly rounded box, banded in iron.
    timber.colored(grey(0.85), |b| {
        b.placed(at(Vec3::X * SHORT), |b| b.rounded_box(WEIGHT_HALF, 0.03, 2))
    });
    for dx in [-0.3f32, 0.0, 0.3] {
        iron.placed(at(Vec3::X * (SHORT + dx * WEIGHT_HALF.x / 0.45)), |b| {
            b.rounded_box(
                Vec3::new(0.035, WEIGHT_HALF.y + 0.012, WEIGHT_HALF.z + 0.012),
                0.008,
                1,
            )
        });
    }
    // The cup's iron walls.
    for (center, half) in &parts[2..] {
        iron.placed(at(*center), |b| b.rounded_box(*half, 0.008, 1));
    }
    // Straps along the beam, and the hub around the axle.
    for x in [-LONG + 0.45, -LONG * 0.66, -LONG * 0.33, -0.35, 0.35] {
        iron.placed(at(Vec3::X * x), |b| {
            b.rounded_box(Vec3::new(0.04, BEAM + 0.012, BEAM + 0.012), 0.006, 1)
        });
    }
    iron.placed(rot_at(along_z(), Vec3::ZERO), |b| {
        b.cylinder(0.17, BEAM + 0.03, 18, 0.02)
    });
    Arm {
        timber: timber.build(),
        iron: iron.build(),
    }
}

/// A dressed granite ball of `radius`.
pub fn ball(radius: f32) -> Mesh {
    let mut b = MeshBuilder::new();
    b.sphere(radius, 20);
    b.build()
}
