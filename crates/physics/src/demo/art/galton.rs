//! Meshes for the Galton board and its balls.
//!
//! As in [`super::structures`]: one mesh per material, in the local frame of the entity that
//! carries it (here the ground under the middle of the glass, Y up, Z out of the glass),
//! metric UVs, and vertex colors near white.

use std::f32::consts::FRAC_PI_2;

use engine_core::math::{Quat, Vec3};
use engine_render::Mesh;

use super::kit::MeshBuilder;
use super::structures::{at, grey, hash, rot_at};
use crate::demo::galton::{self, BALL, BALLS, BIN_TOP, BINS, FLOOR, HALF_WIDTH, PITCH};

/// Oiled walnut: the frame, feet and funnel.
pub const WALNUT: u32 = 0x5a3a22;
/// The bell curve printed on the back board (sRGB hex).
pub const CURVE: u32 = 0xb3261e;

/// The board's meshes, one per material.
pub struct Board {
    /// Walnut: the posts, feet, top rail, bin floor and funnel.
    pub wood: Mesh,
    /// Cream paint: the back board.
    pub board: Mesh,
    /// Brass: the pins.
    pub pins: Mesh,
    /// Chrome: the bins' dividers.
    pub dividers: Mesh,
    /// Glass: the front pane.
    pub glass: Mesh,
    /// Red: the bell curve, printed on the back board.
    pub curve: Mesh,
}

/// The Galton board, sized from [`crate::demo::galton`]'s layout.
pub fn board() -> Board {
    let top = galton::top();
    let mut wood = MeshBuilder::new();
    let mut back = MeshBuilder::new();
    let mut brass = MeshBuilder::new();
    let mut chrome = MeshBuilder::new();
    let mut glass = MeshBuilder::new();
    let mut curve = MeshBuilder::new();
    let h = galton::SLOT * 0.5;
    let [front_pane, back_pane] = galton::panes();
    // Two posts from the ground to above the top, a rail across the top and the bin floor.
    let post = Vec3::new(0.03, (top + 0.06) * 0.5, h + 0.03);
    for (k, s) in [-1.0f32, 1.0].into_iter().enumerate() {
        let x = s * (HALF_WIDTH + post.x);
        wood.colored(grey(0.94 + 0.06 * hash(k as u32)), |b| {
            b.placed(at(Vec3::new(x, post.y, 0.0)), |b| {
                b.rounded_box(post, 0.008, 2)
            });
            // A foot along Z.
            b.placed(at(Vec3::new(x, 0.035, 0.0)), |b| {
                b.rounded_box(Vec3::new(0.045, 0.035, 0.3), 0.012, 2)
            });
        });
    }
    let span = HALF_WIDTH + 2.0 * post.x;
    wood.colored(grey(0.97), |b| {
        b.placed(at(Vec3::new(0.0, top + 0.04, 0.0)), |b| {
            b.rounded_box(Vec3::new(span + 0.01, 0.02, h + 0.035), 0.008, 2)
        });
        b.placed(at(Vec3::new(0.0, FLOOR - 0.04, 0.0)), |b| {
            b.rounded_box(Vec3::new(span, 0.04, h + 0.035), 0.008, 2)
        });
        // A stretcher between the feet.
        b.placed(at(Vec3::new(0.0, 0.12, -0.12)), |b| {
            b.rounded_box(Vec3::new(span, 0.025, 0.02), 0.006, 2)
        });
    });
    wood.colored(grey(0.92), |b| {
        for (c, half, slope) in galton::guides() {
            b.placed(rot_at(Quat::from_rotation_z(slope), c), |b| {
                b.rounded_box(half, 0.003, 1)
            });
        }
    });
    back.placed(at(back_pane.0), |b| b.rounded_box(back_pane.1, 0.002, 1));
    glass.placed(at(front_pane.0), |b| b.rounded_box(front_pane.1, 0.002, 1));
    // Pins: brass rods from the back board to the glass.
    for (x, y) in galton::pins() {
        brass.placed(
            rot_at(Quat::from_rotation_x(FRAC_PI_2), Vec3::new(x, y, 0.0)),
            |b| b.cylinder(galton::PIN, h, 10, 0.0008),
        );
    }
    for j in 1..BINS {
        let x = -HALF_WIDTH + j as f32 * PITCH;
        chrome.placed(at(Vec3::new(x, (FLOOR + BIN_TOP) * 0.5, 0.0)), |b| {
            b.rounded_box(Vec3::new(0.002, (BIN_TOP - FLOOR) * 0.5, h), 0.0015, 1)
        });
    }
    // The bell curve the bins fill to: each bin's share of the balls, stacked about 82% as
    // tightly as balls in a plane can be.
    let width = PITCH - 0.004;
    let per_ball = std::f32::consts::PI * BALL * BALL / 0.82;
    let z = -h;
    let path: Vec<Vec3> = (0..=120)
        .map(|i| {
            let x = -HALF_WIDTH + 2.0 * HALF_WIDTH * i as f32 / 120.0;
            let balls = BALLS as f32 * galton::share(x / PITCH);
            Vec3::new(x, FLOOR + 0.004 + balls * per_ball / width, z)
        })
        .collect();
    curve.tube(&path, 0.0018, 6, false, true);
    Board {
        wood: wood.build(),
        board: back.build(),
        pins: brass.build(),
        dividers: chrome.build(),
        glass: glass.build(),
        curve: curve.build(),
    }
}

/// A steel ball of `radius`.
pub fn ball(radius: f32) -> Mesh {
    let mut b = MeshBuilder::new();
    b.sphere(radius, 4);
    b.build()
}
