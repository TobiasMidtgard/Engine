//! How the climbing quarter ([`crate::city`]) looks: lime-plastered houses in three washes
//! with stone string courses, sills and cornices, tall windows with painted shutters (a few
//! lit), zinc roof caps, chimneys with pots, a belfry, timber beams, crates and a cart, a
//! plank bridge with rope rails across the street, laundry across the alley, and cobbles.
//!
//! Every box a character touches is drawn on [`city::pieces`]' boxes exactly; the windows,
//! shutters, rails and laundry sit a few millimeters proud or in the air, where nobody stands.

use std::collections::HashMap;

use engine_core::math::{DVec3, Mat4, Quat, Vec2, Vec3};
use engine_core::prelude::*;
use engine_render::{MeshRenderer, PointLight};

use super::materials as mat;
use super::{Library, MeshBuilder};
use crate::city::{self, BUILDINGS, Kind, STOREY};

type M = MeshBuilder;

const PREFIX: &str = "city/";

/// Geometry of the quarter, one mesh per material.
#[derive(Default)]
struct Kit {
    parts: HashMap<&'static str, M>,
}

impl Kit {
    fn add(&mut self, material: &'static str, m: &M, at: Mat4) {
        self.parts.entry(material).or_default().append(m, at);
    }
}

fn mesh_key(material: &str) -> String {
    format!("{PREFIX}{material}")
}

/// The materials the quarter is drawn with (each one a mesh `city/<material>`).
const MATERIALS: &[&str] = &[
    mat::PLASTER[0],
    mat::PLASTER[1],
    mat::PLASTER[2],
    mat::STONE,
    mat::COBBLES,
    mat::ZINC,
    mat::WOOD,
    mat::WOOD_DARK,
    mat::SHUTTER[0],
    mat::SHUTTER[1],
    mat::SHUTTER[2],
    mat::GLASS,
    mat::GLASS_LIT,
    mat::CLOTH,
    mat::CLOTH_INDIGO,
    mat::CLOTH_CRIMSON,
    mat::RUSTED_STEEL,
];

fn kind_material(kind: Kind) -> &'static str {
    match kind {
        Kind::Plaster(i) => mat::PLASTER[i as usize % 3],
        Kind::Stone => mat::STONE,
        Kind::Cobbles => mat::COBBLES,
        Kind::Zinc => mat::ZINC,
        Kind::Timber => mat::WOOD_DARK,
    }
}

fn at(p: Vec3) -> Mat4 {
    Mat4::from_translation(p)
}

fn v(p: [f64; 3]) -> Vec3 {
    DVec3::from_array(p).as_vec3()
}

/// Builds the quarter's meshes (a [`Generator`](super::Generator)).
pub fn generate(lib: &mut Library) {
    let mut kit = Kit::default();
    for p in city::pieces() {
        let size = Vec3::from(p.size);
        let bevel = match p.kind {
            Kind::Plaster(_) | Kind::Cobbles => 0.0,
            Kind::Zinc => 0.02,
            _ => (size.min_element() * 0.15).min(0.03),
        };
        let m = if bevel > 0.0 {
            M::chamfer_box(size, bevel)
        } else {
            let mut m = M::rounded_box(size, 0.0, 1).flat_shaded();
            m.planar_uvs(1.0);
            m
        };
        // World-aligned UVs, so the plaster and cobbles run on across pieces.
        let mut m = m.transformed(at(v(p.center)));
        m.planar_uvs(1.0);
        kit.add(kind_material(p.kind), &m, Mat4::IDENTITY);
    }
    windows(&mut kit);
    chimney_pots(&mut kit);
    bridge_rails(&mut kit);
    laundry(&mut kit);
    cart(&mut kit);
    belfry_bell(&mut kit);
    for (material, m) in kit.parts {
        if !m.is_empty() {
            lib.mesh(&mesh_key(material), m);
        }
    }
}

/// Windows on every windowed face: glass between the sill and a stone lintel, two shutters
/// folded back against the wall either side (a few closed), and a few lit from inside.
fn windows(kit: &mut Kit) {
    let w = city::WINDOW_WIDTH as f32;
    let tall = 1.45;
    let shutter = M::chamfer_box(Vec3::new(w * 0.5, tall, 0.045), 0.008);
    let lintel = M::chamfer_box(Vec3::new(w + 0.3, 0.16, 0.12), 0.015);
    let glass = M::rounded_box(Vec3::new(w, tall, 0.02), 0.0, 1);
    let mut n = 0u32;
    for (bi, b) in BUILDINGS.iter().enumerate() {
        let shutter_mat = mat::SHUTTER[bi % 3];
        for face in 0..4 {
            if !b.windows[face] {
                continue;
            }
            for (along, normal) in city::windows(b, face) {
                for floor in 0..b.storeys {
                    n += 1;
                    let sill = (floor as f64 * STOREY + STOREY * 0.5) as f32;
                    let y = sill + tall * 0.5 + 0.02;
                    let (face_at, right) = match face {
                        0 => (Vec3::new(b.x[0] as f32, y, along as f32), Vec3::NEG_Z),
                        1 => (Vec3::new(b.x[1] as f32, y, along as f32), Vec3::Z),
                        2 => (Vec3::new(along as f32, y, b.z[0] as f32), Vec3::X),
                        _ => (Vec3::new(along as f32, y, b.z[1] as f32), Vec3::NEG_X),
                    };
                    let rot = Quat::from_rotation_arc(Vec3::Z, normal);
                    let frame =
                        |offset: Vec3| Mat4::from_rotation_translation(rot, face_at + offset);
                    // Lit windows: one in seven, at random but fixed.
                    let lit = (n.wrapping_mul(2_654_435_761) >> 28).is_multiple_of(7);
                    let glass_mat = if lit { mat::GLASS_LIT } else { mat::GLASS };
                    kit.add(glass_mat, &glass, frame(normal * 0.012));
                    kit.add(
                        mat::STONE,
                        &lintel,
                        frame(normal * 0.06 + Vec3::Y * (tall * 0.5 + 0.1)),
                    );
                    // Shutters folded back flat against the wall either side.
                    let closed = (n.wrapping_mul(40_503) >> 12).is_multiple_of(9);
                    for side in [-1.0f32, 1.0] {
                        let off = if closed {
                            right * (side * w * 0.25) + normal * 0.04
                        } else {
                            right * (side * w * 0.78) + normal * 0.03
                        };
                        kit.add(shutter_mat, &shutter, frame(off));
                    }
                }
            }
        }
    }
}

/// Clay pots on the chimneys.
fn chimney_pots(kit: &mut Kit) {
    let pot = M::lathe(
        &[
            Vec2::new(0.0, 0.0),
            Vec2::new(0.14, 0.0),
            Vec2::new(0.12, 0.3),
            Vec2::new(0.09, 0.42),
            Vec2::new(0.11, 0.46),
            Vec2::new(0.0, 0.46),
        ],
        12,
    );
    for p in city::pieces() {
        let size = Vec3::from(p.size);
        let is_chimney =
            p.kind == Kind::Stone && (size.x - 0.9).abs() < 1e-3 && (size.z - 0.9).abs() < 1e-3;
        if is_chimney {
            let top = p.max().as_vec3();
            let c = v(p.center);
            for dx in [-0.18f32, 0.18] {
                kit.add(mat::RUSTED_STEEL, &pot, at(Vec3::new(c.x + dx, top.y, c.z)));
            }
        }
    }
}

/// Rope rails along the plank bridge across the street.
fn bridge_rails(kit: &mut Kit) {
    let (x0, x1, z0, z1, y) = (-7.2f32, -5.8f32, -3.4f32, 3.4f32, 10.5f32);
    let post = M::chamfer_box(Vec3::new(0.08, 1.0, 0.08), 0.01);
    for x in [x0 + 0.05, x1 - 0.05] {
        for z in [z0 + 0.1, z1 - 0.1] {
            kit.add(mat::WOOD_DARK, &post, at(Vec3::new(x, y + 0.5, z)));
        }
        // The hand rope sags a little between the posts.
        let path: Vec<Vec3> = (0..=12)
            .map(|i| {
                let t = i as f32 / 12.0;
                let z = z0 + 0.1 + (z1 - z0 - 0.2) * t;
                Vec3::new(x, y + 0.95 - 0.18 * 4.0 * t * (1.0 - t), z)
            })
            .collect();
        kit.add(mat::CLOTH, &M::tube(&path, 0.018, 6), Mat4::IDENTITY);
    }
}

/// Washing lines across the alley between the bakery and the townhouse, with sheets and
/// shirts pegged on them.
fn laundry(kit: &mut Kit) {
    for (i, (y, z)) in [(5.9f32, 5.0f32), (9.3, 8.0), (6.1, 11.0)]
        .into_iter()
        .enumerate()
    {
        let path: Vec<Vec3> = (0..=10)
            .map(|k| {
                let t = k as f32 / 10.0;
                Vec3::new(-2.0 + 4.0 * t, y - 0.35 * 4.0 * t * (1.0 - t), z)
            })
            .collect();
        kit.add(mat::CLOTH, &M::tube(&path, 0.006, 4), Mat4::IDENTITY);
        for k in 0..3 {
            let t = 0.22 + 0.28 * k as f32;
            let x = -2.0 + 4.0 * t;
            let line = y - 0.35 * 4.0 * t * (1.0 - t);
            let (w, h) = if (i + k) % 2 == 0 {
                (0.9, 1.1)
            } else {
                (0.55, 0.7)
            };
            let cloth = [mat::CLOTH, mat::CLOTH_INDIGO, mat::CLOTH_CRIMSON][(i + k) % 3];
            let sheet = M::chamfer_box(Vec3::new(w, h, 0.01), 0.002);
            kit.add(
                cloth,
                &sheet,
                Mat4::from_rotation_translation(
                    Quat::from_rotation_x(0.06 * (k as f32 - 1.0)),
                    Vec3::new(x, line - h * 0.5, z),
                ),
            );
        }
    }
}

/// The cart's wheels and shafts (its bed is a piece).
fn cart(kit: &mut Kit) {
    let wheel = {
        let mut m = M::lathe(
            &[
                Vec2::new(0.0, -0.05),
                Vec2::new(0.55, -0.05),
                Vec2::new(0.6, -0.03),
                Vec2::new(0.6, 0.03),
                Vec2::new(0.55, 0.05),
                Vec2::new(0.0, 0.05),
            ],
            20,
        );
        m = m.transformed(Mat4::from_rotation_x(std::f32::consts::FRAC_PI_2));
        m
    };
    for z in [-2.78f32, -1.22] {
        kit.add(mat::WOOD, &wheel, at(Vec3::new(-18.0, 0.6, z)));
    }
    let shaft = M::chamfer_box(Vec3::new(2.2, 0.08, 0.08), 0.01);
    for z in [-2.5f32, -1.5] {
        kit.add(
            mat::WOOD,
            &shaft,
            Mat4::from_rotation_translation(
                Quat::from_rotation_z(-0.18),
                Vec3::new(-15.5, 0.75, z),
            ),
        );
    }
}

/// A bronze bell hanging in the belfry.
fn belfry_bell(kit: &mut Kit) {
    let tower = &BUILDINGS[5];
    let top = tower.height() as f32;
    let cx = ((tower.x[0] + tower.x[1]) * 0.5) as f32;
    let cz = ((tower.z[0] + tower.z[1]) * 0.5) as f32;
    let bell = M::lathe(
        &[
            Vec2::new(0.0, 0.0),
            Vec2::new(0.62, 0.0),
            Vec2::new(0.58, 0.12),
            Vec2::new(0.42, 0.45),
            Vec2::new(0.36, 0.9),
            Vec2::new(0.2, 1.05),
            Vec2::new(0.0, 1.08),
        ],
        24,
    );
    kit.add(
        mat::RUSTED_STEEL,
        &bell,
        at(Vec3::new(cx, top + city::BELFRY_HEIGHT as f32 - 1.6, cz)),
    );
    let beam = M::chamfer_box(Vec3::new(5.0, 0.25, 0.25), 0.02);
    kit.add(
        mat::WOOD_DARK,
        &beam,
        at(Vec3::new(cx, top + city::BELFRY_HEIGHT as f32 - 0.4, cz)),
    );
}

/// Spawns the quarter's meshes at `origin` and a few warm lamps by doors and on the square.
pub fn dress_city(commands: &mut Commands, origin: DVec3) {
    for &material in MATERIALS {
        commands.spawn((
            Name::new("Climbing quarter"),
            Transform::from_translation(origin),
            MeshRenderer::new(super::mesh(&mesh_key(material)), mat::material(material)),
        ));
    }
    for p in [
        [-12.0, 3.2, 2.4],
        [6.5, 3.2, 2.4],
        [-19.0, 3.2, -2.4],
        [18.0, 3.2, -2.4],
        [-28.0, 3.0, 3.0],
    ] {
        commands.spawn((
            Name::new("Street lamp"),
            Transform::from_translation(origin + DVec3::from_array(p)),
            PointLight {
                color: [1.0, 0.7, 0.4],
                intensity_lm: 1800.0,
                range: 9.0,
                ..Default::default()
            },
        ));
    }
}
