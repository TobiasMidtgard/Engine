//! Meshes for the giant Jenga station: its blocks, the test rig's frame and its pusher rod.
//!
//! As in [`super::structures`]: one mesh per material, metric UVs, vertex colors near white.

use engine_core::math::Vec3;
use engine_render::Mesh;

use super::kit::MeshBuilder;
use super::structures::{at, grey, hash};
use crate::demo::jenga::{ROD, ROD_LONG, rig_frame};

/// A block of `half` extents; `seed` picks its shade and which patch of the grain it shows.
pub fn block(half: Vec3, seed: u32) -> Mesh {
    let mut b = MeshBuilder::new();
    b.colored(grey(0.88 + 0.12 * hash(seed * 7 + 3)), |b| {
        b.rounded_box(half, 0.006, 2)
    });
    let shift = [hash(seed * 13 + 1) * 0.9, hash(seed * 17 + 2) * 0.9];
    let mut mesh = b.build();
    for v in &mut mesh.vertices {
        v.uv[0] += shift[0];
        v.uv[1] += shift[1];
    }
    mesh
}

/// The rig's frame: yellow posts, the beam across them and the foot plate.
pub fn rig() -> Mesh {
    let mut b = MeshBuilder::new();
    for (center, half) in rig_frame() {
        b.placed(at(center), |b| b.rounded_box(half, 0.012, 2));
    }
    b.build()
}

/// The pusher's meshes, in its body's frame (along Y, the tip at +Y).
pub struct Rod {
    /// The polished steel rod with a rounded tip.
    pub steel: Mesh,
    /// The yellow carriage at its back end.
    pub carriage: Mesh,
}

pub fn rod() -> Rod {
    let mut steel = MeshBuilder::new();
    steel.cylinder(ROD, ROD_LONG * 0.5 - ROD * 0.5, 16, ROD * 0.4);
    let mut carriage = MeshBuilder::new();
    let back = -ROD_LONG * 0.5;
    carriage.placed(at(Vec3::Y * (back + 0.09)), |b| {
        b.rounded_box(Vec3::new(0.16, 0.09, 0.16), 0.015, 2)
    });
    // A collar where the rod leaves the carriage.
    carriage.placed(at(Vec3::Y * (back + 0.21)), |b| {
        b.cylinder(ROD * 1.8, 0.03, 16, 0.006)
    });
    Rod {
        steel: steel.build(),
        carriage: carriage.build(),
    }
}
