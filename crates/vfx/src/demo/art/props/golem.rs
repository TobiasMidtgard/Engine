//! The stone golem that walks the footstep strip, built from fractured boulders held together
//! by glowing ember joints around a crystal heart; and the stones dropped into the pond.
//!
//! The golem comes in pieces so it can move: the body (pelvis, hunched chest, shoulders and a
//! low head) sits on the walker entity, about 0.9 m above the ground, facing -Z; long arms hang
//! from shoulder pivots and legs from hip pivots ([`SHOULDER`], [`HIP`]), each along -Y.

use engine_core::math::{Quat, Rng, Vec2, Vec3};
use engine_render::Mesh;

use super::super::meshes::nature::{RockShape, crystal, grass_tuft, lichen, moss_cap, rock};
use super::super::meshes::{icosphere, moved, noise1, paint, placed, tube_loop};
use super::super::{Prop, Surface};

/// Shoulder pivots in body space (±X).
pub const SHOULDER: Vec3 = Vec3::new(0.47, 0.44, 0.04);
/// Hip pivots in body space (±X).
pub const HIP: Vec3 = Vec3::new(0.17, -0.2, 0.02);
/// From the hip pivot to the sole, meters.
pub const LEG: f32 = 0.62;

/// A fractured boulder of `size` at `at`, turned by `rot`.
fn chunk(size: Vec3, seed: u64, at: Vec3, rot: Quat) -> Mesh {
    let shape = RockShape::new(size, seed).cuts(8).lumps(0.08).floor(-0.85);
    moved(rock(shape), at, rot)
}

/// A glowing joint: a slightly lumpy ball.
fn orb(radius: f32, at: Vec3, seed: u64) -> Mesh {
    let mut m = icosphere(2);
    for v in &mut m.vertices {
        let d = Vec3::from(v.position);
        v.position = (d * radius * (1.0 + 0.12 * noise1(d, 2.0, seed, 2))).to_array();
    }
    m.compute_normals();
    moved(m, at, Quat::IDENTITY)
}

/// Pelvis, hunched chest, shoulder boulders, a low head with ember eyes, the crystal heart
/// and the joint embers; moss and a tuft of grass on its back.
pub fn golem_body() -> Prop {
    let mut rng = Rng::new(0x6013);
    let mut rocks = Mesh::default();
    let mut embers = Mesh::default();
    let mut gems = Mesh::default();
    let mut moss = Mesh::default();
    // Leaning forward: a negative turn about X tips the top toward -Z.
    let hunch = Quat::from_rotation_x(-0.22);
    let parts = [
        // Pelvis and waist.
        (
            Vec3::new(0.5, 0.26, 0.36),
            1,
            Vec3::new(0.0, -0.12, 0.02),
            Quat::IDENTITY,
        ),
        (
            Vec3::new(0.4, 0.22, 0.32),
            2,
            Vec3::new(0.0, 0.06, 0.03),
            Quat::from_rotation_y(0.4),
        ),
        // Chest, big and leaning forward, and a back stone.
        (
            Vec3::new(0.8, 0.52, 0.56),
            3,
            Vec3::new(0.0, 0.36, 0.04),
            hunch,
        ),
        (
            Vec3::new(0.46, 0.34, 0.26),
            4,
            Vec3::new(0.0, 0.42, 0.27),
            hunch * Quat::from_rotation_y(0.3),
        ),
        // Shoulder boulders.
        (
            Vec3::new(0.36, 0.3, 0.38),
            5,
            Vec3::new(-0.42, 0.55, 0.05),
            Quat::from_rotation_z(0.35),
        ),
        (
            Vec3::new(0.36, 0.3, 0.38),
            6,
            Vec3::new(0.42, 0.55, 0.05),
            Quat::from_rotation_z(-0.35),
        ),
        // Head, sunk low between the shoulders.
        (
            Vec3::new(0.28, 0.24, 0.3),
            7,
            Vec3::new(0.0, 0.67, -0.17),
            Quat::from_rotation_y(0.15),
        ),
    ];
    for (i, (size, seed, at, rot)) in parts.into_iter().enumerate() {
        let mut m = chunk(size, seed, at, rot);
        // Lichen all over, moss on the shoulders, back and head.
        lichen(&mut m, 0.45, seed * 7);
        if i >= 3 {
            moss.merge(&moss_cap(&m, 0.75, 0.35, 0.006, rng.next_u64()));
        }
        rocks.merge(&m);
    }
    // A brow ridge over the eyes.
    rocks.merge(&chunk(
        Vec3::new(0.26, 0.07, 0.1),
        8,
        Vec3::new(0.0, 0.725, -0.3),
        Quat::from_rotation_x(0.2),
    ));
    for (x, z) in [(-0.2f32, 0.12f32), (0.25, 0.2)] {
        let tuft = grass_tuft(&mut rng, 7, 0.1);
        moss.merge(&moved(tuft, Vec3::new(x, 0.7, z), Quat::IDENTITY));
    }
    // Eyes.
    for x in [-0.065f32, 0.065] {
        embers.merge(&orb(0.024, Vec3::new(x, 0.68, -0.305), 9));
    }
    // Heart: a crystal cluster bursting from the chest, ringed by embers.
    let heart = Vec3::new(0.0, 0.33, -0.25);
    for k in 0..5 {
        let tilt = if k == 0 {
            Quat::from_rotation_x(-1.35)
        } else {
            let a = k as f32 * 1.6;
            Quat::from_rotation_x(-1.0)
                * Quat::from_rotation_z(a.cos() * 0.7)
                * Quat::from_rotation_x(a.sin() * 0.5)
        };
        let len = if k == 0 { 0.24 } else { 0.13 };
        let c = crystal(&mut rng, len * 0.22, len, 6);
        gems.merge(&moved(c, heart + tilt * Vec3::Y * (len * 0.25), tilt));
    }
    let ring: Vec<Vec3> = (0..28)
        .map(|i| {
            let a = i as f32 / 28.0 * std::f32::consts::TAU;
            let r = 0.12 * (1.0 + 0.08 * (a * 3.0).sin());
            Vec3::new(a.cos() * r, a.sin() * r, 0.0)
        })
        .collect();
    let ring = tube_loop(&ring, Vec2::splat(0.016), 6);
    embers.merge(&placed(
        ring,
        heart + Vec3::Z * 0.03,
        Quat::from_rotation_x(-0.22),
        Vec3::new(1.0, 1.15, 1.0),
    ));
    // Joint embers at the shoulders, waist and hips.
    for s in [-1.0f32, 1.0] {
        embers.merge(&orb(0.07, SHOULDER * Vec3::new(s, 1.0, 1.0), 4));
        embers.merge(&orb(0.055, HIP * Vec3::new(s, 1.0, 1.0), 5));
    }
    embers.merge(&orb(0.06, Vec3::new(0.0, -0.02, 0.02), 3));
    Prop::new()
        .with(Surface::Rock, rocks)
        .with(Surface::RuneEmber, embers)
        .with(Surface::Crystal, gems)
        .with(Surface::Moss, moss)
}

/// One long arm, hanging from its shoulder pivot: upper arm, ember elbow, a heavy forearm and
/// a big fist that hangs near the knees.
pub fn golem_arm() -> Prop {
    let mut rocks = Mesh::default();
    let mut moss = Mesh::default();
    let parts = [
        (
            Vec3::new(0.22, 0.34, 0.22),
            11,
            Vec3::new(0.0, -0.2, 0.0),
            Quat::IDENTITY,
        ),
        (
            Vec3::new(0.27, 0.36, 0.27),
            12,
            Vec3::new(0.0, -0.6, -0.02),
            Quat::from_rotation_y(0.7),
        ),
        (
            Vec3::new(0.31, 0.25, 0.3),
            13,
            Vec3::new(0.0, -0.86, -0.03),
            Quat::from_rotation_y(-0.4),
        ),
    ];
    for (size, seed, at, rot) in parts {
        let mut m = chunk(size, seed, at, rot);
        lichen(&mut m, 0.4, seed * 7);
        if seed == 12 {
            moss.merge(&moss_cap(&m, 0.6, 0.4, 0.005, 31));
        }
        rocks.merge(&m);
    }
    Prop::new()
        .with(Surface::Rock, rocks)
        .with(Surface::RuneEmber, orb(0.05, Vec3::new(0.0, -0.4, 0.0), 6))
        .with(Surface::Moss, moss)
}

/// One leg, hanging from its hip pivot: thigh, ember knee, shin and a flat-soled foot whose
/// sole is [`LEG`] below the pivot.
pub fn golem_leg() -> Prop {
    let mut rocks = Mesh::default();
    rocks.merge(&chunk(
        Vec3::new(0.26, 0.3, 0.26),
        21,
        Vec3::new(0.0, -0.16, 0.0),
        Quat::IDENTITY,
    ));
    rocks.merge(&chunk(
        Vec3::new(0.24, 0.26, 0.24),
        22,
        Vec3::new(0.0, -0.45, 0.0),
        Quat::from_rotation_y(0.9),
    ));
    let foot = RockShape::new(Vec3::new(0.3, 0.13, 0.38), 23)
        .cuts(6)
        .lumps(0.06)
        .floor(-0.4);
    let mut foot = rock(foot);
    // Dirt on the sole and toes.
    paint(&mut foot, |p, _| {
        Vec3::splat(if p.y < -0.03 { 0.7 } else { 1.0 })
    });
    // Put the flat sole exactly LEG below the pivot.
    let sole = foot.aabb().min.y;
    rocks.merge(&moved(
        foot,
        Vec3::new(0.0, -LEG - sole, -0.05),
        Quat::IDENTITY,
    ));
    Prop::new().with(Surface::Rock, rocks).with(
        Surface::RuneEmber,
        orb(0.045, Vec3::new(0.0, -0.32, 0.0), 7),
    )
}

/// A rough fist-sized stone (about 0.3 m) for dropping into the pond; `variant` 0..3.
pub fn dropped_stone(variant: u64) -> Prop {
    let size = Vec3::new(0.34, 0.26, 0.3) * (1.0 + 0.1 * variant as f32);
    let shape = RockShape::new(size, 100 + variant)
        .detail(2)
        .cuts(6)
        .floor(-1.0);
    Prop::new().with(Surface::Rock, rock(shape))
}
