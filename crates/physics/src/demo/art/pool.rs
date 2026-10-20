//! Meshes for the pool table and its balls.
//!
//! As in [`super::structures`]: one mesh per material, in the local frame of the entity that
//! carries it (here the middle of the cloth, Y up), metric UVs, and vertex colors near white
//! except on the balls, whose colors are vertex colors on a white lacquer.

use engine_core::math::{Vec2, Vec3};
use engine_render::Mesh;

use super::kit::{MeshBuilder, srgb};
use super::structures::{at, grey, hash};
use crate::demo::pool::Part;

/// Tournament green cloth (sRGB hex).
pub const CLOTH: u32 = 0x1e6b45;
/// Varnished mahogany: rails, apron and legs.
pub const MAHOGANY: u32 = 0x5b2614;
/// The pockets' black leather.
pub const POCKET: u32 = 0x161414;

/// The table's meshes, one per material.
pub struct Table {
    /// Cloth: the bed and the cushions.
    pub cloth: Mesh,
    /// Mahogany: rails, apron and the six turned legs.
    pub wood: Mesh,
    /// Black leather: the pocket cups.
    pub pockets: Mesh,
    /// Brass-colored steel: the legs' feet.
    pub steel: Mesh,
    /// Mother of pearl: the sights (diamonds) on the rails.
    pub inlay: Mesh,
}

/// The pool table from its collider boxes (`crate::demo::pool::boxes`) and pocket centers,
/// standing `bed` meters high with a playing surface of `2·half` meters.
///
/// Frame: the middle of the cloth's top, Y up; the floor is at `y = -bed`.
pub fn table(
    boxes: &[(Vec3, Vec3, Part)],
    pockets: &[(f32, f32)],
    bed: f32,
    half: [f32; 2],
) -> Table {
    let mut cloth = MeshBuilder::new();
    let mut wood = MeshBuilder::new();
    let mut leather = MeshBuilder::new();
    let mut steel = MeshBuilder::new();
    let mut inlay = MeshBuilder::new();
    let mut rail_top = 0.0f32;
    let mut outer = Vec2::ZERO;
    for (i, &(c, h, part)) in boxes.iter().enumerate() {
        match part {
            Part::Bed => cloth.colored(grey(0.97 + 0.03 * hash(i as u32)), |b| {
                b.placed(at(c), |b| b.rounded_box(h, 0.002, 1))
            }),
            Part::Cushion => cloth.colored(grey(0.92), |b| {
                b.placed(at(c), |b| b.rounded_box(h, 0.014, 3))
            }),
            Part::Rail => {
                rail_top = rail_top.max(c.y + h.y);
                outer = outer.max(Vec2::new(c.x.abs() + h.x, c.z.abs() + h.z));
                wood.colored(grey(0.96 + 0.04 * hash(i as u32 + 7)), |b| {
                    b.placed(at(c), |b| b.rounded_box(h, 0.016, 3))
                });
            }
            Part::Catch => {}
        }
    }
    // A bottom board under the apron, and a molding round the rails' top edge.
    wood.colored(grey(0.9), |b| {
        b.placed(at(Vec3::new(0.0, -0.215, 0.0)), |b| {
            b.rounded_box(Vec3::new(outer.x - 0.03, 0.015, outer.y - 0.03), 0.01, 2)
        });
        b.placed(at(Vec3::new(0.0, -0.17, 0.0)), |b| {
            b.rounded_box(Vec3::new(outer.x + 0.012, 0.02, outer.y + 0.012), 0.012, 2)
        });
    });
    // Six turned legs, under the corners and the middles of the long sides.
    let leg_top = -0.23;
    let leg_len = bed + leg_top;
    let profile: Vec<Vec2> = [
        (0.0, 0.0),
        (0.075, 0.0),
        (0.075, 0.05),
        (0.06, 0.08),
        (0.05, 0.16),
        (0.07, 0.24),
        (0.055, 0.3),
        (0.045, 0.42),
        (0.05, 0.5),
        (0.065, 0.53),
        (0.065, 0.56),
        (0.0, 0.56),
    ]
    .iter()
    .map(|&(r, y)| Vec2::new(r, y * leg_len / 0.56))
    .collect();
    let (lx, lz) = (outer.x - 0.2, outer.y - 0.14);
    for (x, z) in [
        (-lx, -lz),
        (0.0, -lz),
        (lx, -lz),
        (-lx, lz),
        (0.0, lz),
        (lx, lz),
    ] {
        wood.placed(at(Vec3::new(x, -bed, z)), |b| b.lathe(&profile, 20, 50.0));
        steel.placed(at(Vec3::new(x, -bed + 0.006, z)), |b| {
            b.cylinder(0.08, 0.006, 20, 0.003)
        });
    }
    // Pockets: an open leather cup hanging in each hole, as wide as the notch in the rails.
    let r = crate::demo::pool::NOTCH + 0.005;
    let cup: Vec<Vec2> = [(r, 0.0), (r, -0.1), (r * 0.6, -0.115), (0.0, -0.12)]
        .map(|(r, y)| Vec2::new(r, y))
        .to_vec();
    for &(x, z) in pockets {
        leather.placed(at(Vec3::new(x, -0.001, z)), |b| b.lathe(&cup, 24, 50.0));
    }
    // Sights: three between each pair of pockets.
    let [l, w] = half;
    let mid = Vec2::new((l + outer.x) * 0.5 + 0.02, (w + outer.y) * 0.5 + 0.02);
    let mut sights = Vec::new();
    for k in [1.0f32, 2.0, 3.0] {
        for s in [-1.0f32, 1.0] {
            let x = s * l * k / 4.0;
            sights.push(Vec3::new(x, rail_top + 0.001, mid.y));
            sights.push(Vec3::new(x, rail_top + 0.001, -mid.y));
        }
        let z = (k - 2.0) * w * 0.5;
        sights.push(Vec3::new(mid.x, rail_top + 0.001, z));
        sights.push(Vec3::new(-mid.x, rail_top + 0.001, z));
    }
    for p in sights {
        inlay.placed(at(p), |b| b.cylinder(0.008, 0.0015, 12, 0.001));
    }
    Table {
        cloth: cloth.build(),
        wood: wood.build(),
        pockets: leather.build(),
        steel: steel.build(),
        inlay: inlay.build(),
    }
}

/// A ball's color by number (sRGB hex): 1 and 9 yellow, 2 and 10 blue … 8 black.
fn ball_color(number: u8) -> u32 {
    match (number + 7) % 8 {
        0 => 0xf2c218,
        1 => 0x1c47b0,
        2 => 0xd02a28,
        3 => 0x55287e,
        4 => 0xee6a1c,
        5 => 0x1a7a3c,
        6 => 0x7a1c22,
        _ => 0x121212,
    }
}

/// Pool ball `number` (0 the cue ball) of `radius`: solids 1–8 colored all over, stripes 9–15
/// white with a colored band round the equator, both with a white number disc on each side
/// (±Z), and the cue ball ivory. Colors are vertex colors.
pub fn ball(radius: f32, number: u8) -> Mesh {
    let mut b = MeshBuilder::new();
    b.sphere(radius, 12);
    let ivory = srgb(0xf4f0e4);
    let color = srgb(ball_color(number));
    for v in &mut b.mesh.vertices {
        let p = Vec3::from_array(v.position) / radius;
        // The number discs: 22° across around ±Z.
        let disc = p.z.abs() > 0.927;
        let c = match number {
            0 => ivory,
            1..=8 if !disc => color,
            9..=15 if !disc && p.y.abs() < 0.5 => color,
            _ => ivory,
        };
        v.color = c.to_array();
    }
    b.build()
}
