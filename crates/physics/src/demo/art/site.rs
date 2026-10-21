//! Meshes that make the yard read as a working test site: the painted walkway around it,
//! floodlight masts at its corners and the site office, a converted shipping container.
//!
//! As in [`super::structures`]: one mesh per material, metric UVs, vertex colors near white.

use std::f32::consts::FRAC_PI_2;

use engine_core::math::{Quat, Vec3};
use engine_render::Mesh;

use super::kit::MeshBuilder;
use super::structures::{at, bar, grey, hash, plate, rot_at};

/// Lines of the walkway inside the curb: their insets from the curb's inner face and width,
/// meters.
pub const WALK_OUTER: f32 = 0.8;
pub const WALK_INNER: f32 = 2.0;
pub const LINE: f32 = 0.1;

/// The walkway's paint: two lines around the yard, `inner` the curb's inner half extents
/// (x, z), on the slab at y = 0, with chevron hatching across each corner.
pub fn walkway(inner: [f32; 2]) -> Mesh {
    let mut b = MeshBuilder::new();
    let y = 0.003;
    let strip = |b: &mut MeshBuilder, a: Vec3, c: Vec3, w: f32, k: u32| {
        let d = (c - a).normalize();
        let side = Vec3::new(-d.z, 0.0, d.x) * (w * 0.5);
        // Worn paint: each run a slightly different white-yellow.
        b.colored(grey(0.86 + 0.14 * hash(k * 31 + 5)), |b| {
            b.flat_quad([a - side, c - side, c + side, a + side], Vec3::Y, d)
        });
    };
    let mut k = 0;
    for inset in [WALK_OUTER, WALK_INNER] {
        let (hx, hz) = (inner[0] - inset, inner[1] - inset);
        let corners = [
            Vec3::new(-hx, y, -hz),
            Vec3::new(hx, y, -hz),
            Vec3::new(hx, y, hz),
            Vec3::new(-hx, y, hz),
        ];
        for i in 0..4 {
            let (a, c) = (corners[i], corners[(i + 1) % 4]);
            let d = (c - a).normalize();
            // Lines run to the corner's outer edge so the corners close.
            strip(&mut b, a - d * (LINE * 0.5), c + d * (LINE * 0.5), LINE, k);
            k += 1;
        }
    }
    // Hatching across the corners, where the walkway turns: stripes at 45 degrees.
    let (ox, oz) = (inner[0] - WALK_OUTER, inner[1] - WALK_OUTER);
    let span = WALK_INNER - WALK_OUTER;
    for (sx, sz) in [(-1.0f32, -1.0f32), (1.0, -1.0), (1.0, 1.0), (-1.0, 1.0)] {
        for s in 0..4 {
            let t = (s as f32 + 0.5) / 4.0 * span;
            let a = Vec3::new(sx * (ox - t), y, sz * oz);
            let c = Vec3::new(sx * ox, y, sz * (oz - t));
            strip(&mut b, a, c, LINE * 0.8, k);
            k += 1;
        }
    }
    b.build()
}

/// A floodlight mast's meshes, in its frame: the pole stands on the origin along +Y and its
/// lamps look toward +Z.
pub struct Mast {
    /// Galvanized steel: the pole, its base plate and the crossarm.
    pub steel: Mesh,
    /// Dark painted steel: the lamp housings and the anchor nuts.
    pub dark: Mesh,
    /// The lamps' glass.
    pub glass: Mesh,
}

/// A floodlight mast's height, meters.
pub const MAST_HIGH: f32 = 12.0;
/// The pole's radius at its foot, meters (it tapers to 60% at the top).
pub const MAST_FOOT: f32 = 0.16;

pub fn mast() -> Mast {
    let (mut steel, mut dark, mut glass) =
        (MeshBuilder::new(), MeshBuilder::new(), MeshBuilder::new());
    // The pole: three sections, each a little thinner, with a collar at each joint.
    let sections = 3;
    for i in 0..sections {
        let (y0, y1) = (
            MAST_HIGH * i as f32 / sections as f32,
            MAST_HIGH * (i + 1) as f32 / sections as f32,
        );
        let r = MAST_FOOT * (1.0 - 0.4 * i as f32 / sections as f32);
        steel.placed(at(Vec3::Y * ((y0 + y1) * 0.5)), |b| {
            b.cylinder(r, (y1 - y0) * 0.5, 20, 0.0)
        });
        if i > 0 {
            steel.placed(at(Vec3::Y * y0), |b| b.cylinder(r * 1.25, 0.05, 20, 0.01));
        }
    }
    steel.placed(at(Vec3::Y * MAST_HIGH), |b| {
        b.cylinder(MAST_FOOT * 0.7, 0.03, 20, 0.01)
    });
    // Base plate and anchor bolts.
    plate(
        &mut steel,
        at(Vec3::Y * 0.02),
        Vec3::new(0.32, 0.02, 0.32),
        0.01,
    );
    for (sx, sz) in [(-1.0f32, -1.0f32), (1.0, -1.0), (1.0, 1.0), (-1.0, 1.0)] {
        dark.placed(at(Vec3::new(sx * 0.25, 0.07, sz * 0.25)), |b| {
            b.cylinder(0.025, 0.03, 6, 0.004)
        });
    }
    // The crossarm and three floodlights hanging off it, tilted down toward the yard.
    let arm_y = MAST_HIGH - 0.35;
    bar(
        &mut steel,
        Vec3::new(-1.1, arm_y, 0.0),
        Vec3::new(1.1, arm_y, 0.0),
        0.05,
        12,
    );
    bar(
        &mut steel,
        Vec3::new(0.0, arm_y - 0.6, 0.0),
        Vec3::new(-0.9, arm_y, 0.0),
        0.03,
        10,
    );
    bar(
        &mut steel,
        Vec3::new(0.0, arm_y - 0.6, 0.0),
        Vec3::new(0.9, arm_y, 0.0),
        0.03,
        10,
    );
    let tilt = Quat::from_rotation_x(0.6);
    for x in [-0.85f32, 0.0, 0.85] {
        let hub = Vec3::new(x, arm_y + 0.08, 0.12);
        dark.placed(rot_at(tilt, hub + Vec3::new(0.0, 0.0, 0.16)), |b| {
            b.rounded_box(Vec3::new(0.3, 0.24, 0.1), 0.03, 2)
        });
        // Cooling fins on the back.
        for f in 0..5 {
            let fx = (f as f32 - 2.0) * 0.11;
            dark.placed(rot_at(tilt, hub + Vec3::new(fx, 0.0, 0.03)), |b| {
                b.rounded_box(Vec3::new(0.012, 0.2, 0.04), 0.004, 1)
            });
        }
        glass.placed(rot_at(tilt, hub + Vec3::new(0.0, 0.0, 0.265)), |b| {
            b.rounded_box(Vec3::new(0.26, 0.2, 0.006), 0.004, 1)
        });
    }
    Mast {
        steel: steel.build(),
        dark: dark.build(),
        glass: glass.build(),
    }
}

/// The site office's meshes, in its frame: a 20-foot container on the origin, long along X,
/// its door and windows on the +Z side.
pub struct Office {
    /// Its painted, corrugated steel.
    pub paint: Mesh,
    /// Dark steel: corner castings, the window and door frames, the step.
    pub dark: Mesh,
    /// The windows' glass.
    pub glass: Mesh,
}

/// The container's half extents, meters.
pub const OFFICE: Vec3 = Vec3::new(3.03, 1.295, 1.22);

pub fn office() -> Office {
    let (mut paint, mut dark, mut glass) =
        (MeshBuilder::new(), MeshBuilder::new(), MeshBuilder::new());
    let h = OFFICE;
    let mid = Vec3::Y * h.y;
    paint.placed(at(mid), |b| b.rounded_box(h - Vec3::splat(0.02), 0.01, 1));
    // Corrugations on the long sides and the ends: shallow vertical ribs.
    let rib = |b: &mut MeshBuilder, center: Vec3, along_x: bool| {
        let half = if along_x {
            Vec3::new(0.05, h.y - 0.12, 0.018)
        } else {
            Vec3::new(0.018, h.y - 0.12, 0.05)
        };
        b.placed(at(center), |b| b.rounded_box(half, 0.012, 1));
    };
    let ribs = (2.0 * (h.x - 0.2) / 0.28) as i32;
    for i in 0..=ribs {
        let x = -h.x + 0.2 + i as f32 * 0.28;
        // Leave the door and windows (on +Z) flat.
        let opening = (x - 1.6).abs() < 0.55 || (x + 0.3).abs() < 0.75 || (x + 2.0).abs() < 0.5;
        if !opening {
            rib(&mut paint, Vec3::new(x, h.y, h.z - 0.005), true);
        }
        rib(&mut paint, Vec3::new(x, h.y, -h.z + 0.005), true);
    }
    for i in 0..8 {
        let z = -h.z + 0.25 + i as f32 * 0.28;
        for sx in [-1.0f32, 1.0] {
            rib(&mut paint, Vec3::new(sx * (h.x - 0.005), h.y, z), false);
        }
    }
    // Corner castings and the rails between them.
    for (sx, sy, sz) in [
        (-1.0f32, 0.0f32, -1.0f32),
        (1.0, 0.0, -1.0),
        (1.0, 0.0, 1.0),
        (-1.0, 0.0, 1.0),
        (-1.0, 2.0, -1.0),
        (1.0, 2.0, -1.0),
        (1.0, 2.0, 1.0),
        (-1.0, 2.0, 1.0),
    ] {
        let c = Vec3::new(
            sx * (h.x - 0.08),
            sy * h.y + 0.08 * (1.0 - sy),
            sz * (h.z - 0.08),
        );
        dark.placed(at(c), |b| b.rounded_box(Vec3::splat(0.09), 0.012, 1));
    }
    for (y, sz) in [
        (0.06f32, -1.0f32),
        (0.06, 1.0),
        (2.0 * h.y - 0.06, -1.0),
        (2.0 * h.y - 0.06, 1.0),
    ] {
        dark.placed(at(Vec3::new(0.0, y, sz * (h.z - 0.03))), |b| {
            b.rounded_box(Vec3::new(h.x - 0.17, 0.06, 0.04), 0.01, 1)
        });
    }
    // Windows: dark frames around glass, with a steel bar grille.
    for (x, w) in [(-0.3f32, 0.65f32), (-2.0, 0.4)] {
        let center = Vec3::new(x, 1.45, h.z + 0.01);
        dark.placed(at(center), |b| {
            b.rounded_box(Vec3::new(w + 0.05, 0.5, 0.03), 0.008, 1)
        });
        glass.placed(at(center + Vec3::Z * 0.03), |b| {
            b.rounded_box(Vec3::new(w, 0.45, 0.004), 0.002, 1)
        });
        let bars = (w / 0.13) as i32;
        for i in 0..=bars {
            let bx = x - w + 2.0 * w * i as f32 / bars as f32;
            bar(
                &mut dark,
                Vec3::new(bx, 1.0, h.z + 0.08),
                Vec3::new(bx, 1.9, h.z + 0.08),
                0.01,
                6,
            );
        }
    }
    // The door, its frame and handle, and a steel step below it.
    let door = Vec3::new(1.6, 1.08, h.z + 0.012);
    dark.placed(at(door), |b| {
        b.rounded_box(Vec3::new(0.5, 1.03, 0.03), 0.008, 1)
    });
    paint.placed(at(door + Vec3::Z * 0.03), |b| {
        b.rounded_box(Vec3::new(0.45, 0.98, 0.012), 0.006, 1)
    });
    dark.placed(at(door + Vec3::new(-0.33, 0.0, 0.07)), |b| {
        b.rounded_box(Vec3::new(0.015, 0.09, 0.025), 0.006, 1)
    });
    dark.placed(at(Vec3::new(1.6, 0.06, h.z + 0.3)), |b| {
        b.rounded_box(Vec3::new(0.55, 0.06, 0.28), 0.01, 1)
    });
    // A hooded wall lamp over the door.
    dark.placed(
        rot_at(
            Quat::from_rotation_x(FRAC_PI_2 * 0.2),
            door + Vec3::new(0.0, 1.25, 0.1),
        ),
        |b| b.rounded_box(Vec3::new(0.12, 0.05, 0.09), 0.02, 2),
    );
    Office {
        paint: paint.build(),
        dark: dark.build(),
        glass: glass.build(),
    }
}
