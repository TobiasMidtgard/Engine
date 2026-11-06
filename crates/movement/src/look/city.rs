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
    mat::IRON,
    mat::ROCK,
    mat::ICE,
    mat::BARK,
    mat::LEAVES,
    mat::HAY,
    mat::AWNING,
    mat::LANTERN,
    mat::BRUSHED_METAL,
];

fn kind_material(kind: Kind) -> &'static str {
    match kind {
        Kind::Plaster(i) => mat::PLASTER[i as usize % 3],
        Kind::Stone => mat::STONE,
        Kind::Cobbles => mat::COBBLES,
        Kind::Zinc => mat::ZINC,
        Kind::Timber => mat::WOOD_DARK,
        Kind::Iron => mat::IRON,
        Kind::Rock => mat::ROCK,
        Kind::Ice => mat::ICE,
        Kind::Bark => mat::BARK,
        Kind::Hay => mat::HAY,
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
        if sculpted(p.kind) {
            continue;
        }
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
    lamps(&mut kit);
    balcony(&mut kit);
    bars(&mut kit);
    awning(&mut kit);
    hay(&mut kit);
    crag(&mut kit);
    icefall(&mut kit);
    oak(&mut kit);
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
        // The hand ropes are tightropes too, drawn sagging under whoever walks them
        // (see [`super::lines`]).
    }
}

/// Washing lines across the alley between the bakery and the townhouse, with sheets and
/// shirts pegged on them.
fn laundry(kit: &mut Kit) {
    for (i, (y, z)) in [(5.9f32, 5.0f32), (9.3, 8.0), (6.1, 11.0)]
        .into_iter()
        .enumerate()
    {
        // The line itself is drawn dipping under the hands (see [`super::lines`]).
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
    for z in [-1.78f32, -0.22] {
        kit.add(mat::WOOD, &wheel, at(Vec3::new(-18.0, 0.6, z)));
    }
    let shaft = M::chamfer_box(Vec3::new(2.2, 0.08, 0.08), 0.01);
    for z in [-1.5f32, -0.5] {
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
    for (x, z) in city::LAMPS {
        commands.spawn((
            Name::new("Lamp post light"),
            Transform::from_translation(
                origin + DVec3::new(x, city::LAMP_SHAFT + city::LAMP_HEAD * 0.45, z),
            ),
            PointLight {
                color: [1.0, 0.68, 0.36],
                intensity_lm: 1500.0,
                range: 10.0,
                ..Default::default()
            },
        ));
    }
    for p in [[-19.0, 3.2, -2.4], [18.0, 3.2, -2.4], [-28.0, 3.0, 3.0]] {
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

/// Pieces drawn by their own sculpted meshes, not as boxes.
fn sculpted(kind: Kind) -> bool {
    matches!(
        kind,
        Kind::Iron | Kind::Rock | Kind::Ice | Kind::Bark | Kind::Hay
    )
}

/// A small xorshift for the dressing's scatter (the same every run).
struct Rng(u32);

impl Rng {
    fn next(&mut self) -> f32 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 17;
        self.0 ^= self.0 << 5;
        self.0 as f32 / u32::MAX as f32
    }

    fn range(&mut self, a: f32, b: f32) -> f32 {
        a + (b - a) * self.next()
    }

    fn unit(&mut self) -> Vec3 {
        loop {
            let v = Vec3::new(
                self.range(-1.0, 1.0),
                self.range(-1.0, 1.0),
                self.range(-1.0, 1.0),
            );
            let l = v.length();
            if l > 0.05 && l <= 1.0 {
                return v / l;
            }
        }
    }
}

/// Cast-iron lamp posts: a fluted shaft on a plinth, a glowing lantern on top with a flat
/// iron cap to crouch on.
fn lamps(kit: &mut Kit) {
    let shaft_top = city::LAMP_SHAFT as f32;
    let shaft = M::lathe(
        &[
            Vec2::new(0.0, 0.0),
            Vec2::new(0.17, 0.0),
            Vec2::new(0.17, 0.12),
            Vec2::new(0.13, 0.18),
            Vec2::new(0.12, 0.42),
            Vec2::new(0.085, 0.55),
            Vec2::new(0.075, 0.9),
            Vec2::new(0.095, 0.95),
            Vec2::new(0.075, 1.0),
            Vec2::new(0.07, shaft_top - 0.6),
            Vec2::new(0.09, shaft_top - 0.55),
            Vec2::new(0.07, shaft_top - 0.5),
            Vec2::new(0.072, shaft_top - 0.08),
            Vec2::new(0.13, shaft_top),
            Vec2::new(0.0, shaft_top),
        ],
        16,
    );
    let head = city::LAMP_HEAD as f32;
    let lw = city::LANTERN_WIDTH as f32;
    let plate = M::chamfer_box(Vec3::new(lw, 0.05, lw), 0.01);
    let post = M::chamfer_box(Vec3::new(0.03, head - 0.1, 0.03), 0.004);
    let glass = M::rounded_box(Vec3::new(lw - 0.06, head - 0.12, lw - 0.06), 0.0, 1);
    let cap = M::lathe(
        &[
            Vec2::new(0.0, 0.0),
            Vec2::new(lw * 0.62, 0.0),
            Vec2::new(lw * 0.62, 0.025),
            Vec2::new(lw * 0.5, 0.06),
            Vec2::new(lw * 0.5, 0.08),
            Vec2::new(0.0, 0.08),
        ],
        4,
    )
    .transformed(Mat4::from_rotation_y(std::f32::consts::FRAC_PI_4));
    for (x, z) in city::LAMPS {
        let (x, z) = (x as f32, z as f32);
        kit.add(mat::IRON, &shaft, at(Vec3::new(x, 0.0, z)));
        let base = Vec3::new(x, shaft_top, z);
        kit.add(mat::IRON, &plate, at(base + Vec3::Y * 0.025));
        kit.add(mat::LANTERN, &glass, at(base + Vec3::Y * (head * 0.5)));
        for (dx, dz) in [(-1.0f32, -1.0f32), (1.0, -1.0), (-1.0, 1.0), (1.0, 1.0)] {
            let o = Vec3::new(dx, 0.0, dz) * (lw * 0.5 - 0.02);
            kit.add(mat::IRON, &post, at(base + o + Vec3::Y * (head * 0.5)));
        }
        // The cap's flat top is the perch, at the lantern's top.
        kit.add(mat::IRON, &cap, at(base + Vec3::Y * (head - 0.08)));
    }
}

/// The guild hall balcony's iron railing: a top rail, a bottom rail and balusters.
fn balcony(kit: &mut Kit) {
    let ([x0, x1], rz, ry) = city::BALCONY;
    let (x0, x1, rz, ry) = (x0 as f32, x1 as f32, rz as f32, ry as f32);
    let rail = M::chamfer_box(Vec3::new(x1 - x0, 0.06, 0.08), 0.012);
    kit.add(
        mat::IRON,
        &rail,
        at(Vec3::new((x0 + x1) * 0.5, ry - 0.03, rz)),
    );
    let low = M::chamfer_box(Vec3::new(x1 - x0, 0.03, 0.04), 0.005);
    kit.add(mat::IRON, &low, at(Vec3::new((x0 + x1) * 0.5, 7.12, rz)));
    let tall = ry - 0.06 - 7.0;
    let baluster = M::chamfer_box(Vec3::new(0.018, tall, 0.018), 0.003);
    let newel = M::chamfer_box(Vec3::new(0.06, tall, 0.06), 0.008);
    let n = ((x1 - x0) / 0.13) as i32;
    for i in 0..=n {
        let x = x0 + 0.05 + (x1 - x0 - 0.1) * i as f32 / n as f32;
        let m = if i == 0 || i == n || i == n / 2 {
            &newel
        } else {
            &baluster
        };
        kit.add(mat::IRON, m, at(Vec3::new(x, 7.0 + tall * 0.5, rz)));
    }
    // The sides back to the wall.
    for x in [x0 + 0.03, x1 - 0.03] {
        let side = M::chamfer_box(Vec3::new(0.06, 0.06, (rz + 2.7).abs()), 0.01);
        kit.add(
            mat::IRON,
            &side,
            at(Vec3::new(x, ry - 0.03, (rz - 2.7) * 0.5)),
        );
    }
}

/// Steel bars across the street, their ends in iron plates bolted to the walls.
fn bars(kit: &mut Kit) {
    let y = city::BAR_Y as f32;
    let [z0, z1] = city::BAR_Z;
    let plate = M::chamfer_box(Vec3::new(0.22, 0.32, 0.03), 0.006);
    for x in city::BARS_X {
        let x = x as f32;
        let bar = M::tube(
            &[
                Vec3::new(x, y, z0 as f32 - 0.3),
                Vec3::new(x, y, z1 as f32 + 0.3),
            ],
            0.025,
            10,
        );
        kit.add(mat::BRUSHED_METAL, &bar, Mat4::IDENTITY);
        for z in [z0 as f32 - 0.3, z1 as f32 + 0.3] {
            kit.add(mat::IRON, &plate, at(Vec3::new(x, y, z)));
        }
    }
}

/// The striped awning over the bakery's door, its scalloped valance and iron arms.
fn awning(kit: &mut Kit) {
    let (a, b) = city::AWNING;
    let (a, b) = (v(a), v(b));
    let width = (b.x - a.x).abs();
    let slope = Vec3::new(0.0, b.y - a.y, b.z - a.z);
    let depth = slope.length();
    let center = (a + b) * 0.5;
    let rot = Quat::from_rotation_arc(Vec3::Z, Vec3::new(0.0, slope.y, slope.z) / depth);
    let mut cloth = M::rounded_box(Vec3::new(width, 0.02, depth), 0.0, 1);
    cloth.planar_uvs(1.0);
    kit.add(
        mat::AWNING,
        &cloth,
        Mat4::from_rotation_translation(rot, center),
    );
    // The valance: tabs hanging off the front edge.
    let tab = M::chamfer_box(Vec3::new(0.24, 0.22, 0.01), 0.002);
    let n = (width / 0.25) as i32;
    for i in 0..n {
        let x = a.x.min(b.x) + 0.125 + 0.25 * i as f32;
        kit.add(mat::AWNING, &tab, at(Vec3::new(x, b.y - 0.12, b.z)));
    }
    for x in [a.x, b.x] {
        let arm = M::tube(
            &[Vec3::new(x, a.y - 0.6, a.z), Vec3::new(x, b.y, b.z)],
            0.015,
            6,
        );
        kit.add(mat::IRON, &arm, Mat4::IDENTITY);
    }
}

/// Hay heaped on the cart, straws sticking out of it.
fn hay(kit: &mut Kit) {
    let (top, size) = city::HAY;
    let top = v(top);
    let size = size as f32;
    let mut heap = M::rounded_box(Vec3::new(size, 0.75, 1.35), 0.32, 4);
    // Lumpy.
    for vert in &mut heap.vertices {
        let p = Vec3::from(vert.position);
        let n = super::texgen::fbm(
            Vec2::new(p.x * 2.0 + 9.0, p.z * 2.0 + p.y * 3.0),
            64,
            3,
            0.5,
            461,
        );
        let normal = Vec3::from(vert.normal);
        vert.position = (p + normal * (0.06 * n)).to_array();
    }
    heap.smooth_normals();
    heap.planar_uvs(1.0);
    kit.add(mat::HAY, &heap, at(top - Vec3::Y * 0.33));
    let mut rng = Rng(4611);
    for _ in 0..260 {
        let x = rng.range(-size * 0.5, size * 0.5);
        let z = rng.range(-0.65, 0.65);
        let base = top + Vec3::new(x, -0.05 - 0.25 * (z.abs() / 0.65).powi(2), z);
        let dir = (Vec3::new(
            rng.range(-1.0, 1.0),
            rng.range(0.1, 0.8),
            rng.range(-1.0, 1.0),
        ))
        .normalize();
        let straw = M::tube(&[base, base + dir * rng.range(0.12, 0.3)], 0.0035, 3);
        kit.add(mat::HAY, &straw, Mat4::IDENTITY);
    }
}

/// A grid over a face patch, pushed out of it by `push(x, y)`: the front of the crag and the
/// icefall.
fn sculpted_face(
    origin: Vec3,
    across: Vec3,
    up: Vec3,
    normal: Vec3,
    (w, h): (f32, f32),
    step: f32,
    push: impl Fn(f32, f32) -> f32,
) -> M {
    let mut m = M::new();
    let (nx, ny) = ((w / step).ceil() as u32, (h / step).ceil() as u32);
    for j in 0..=ny {
        for i in 0..=nx {
            let (x, y) = (w * i as f32 / nx as f32, h * j as f32 / ny as f32);
            let p = origin + across * x + up * y + normal * push(x, y);
            m.vertex(p, normal, Vec2::new(x, y));
        }
    }
    let row = nx + 1;
    for j in 0..ny {
        for i in 0..nx {
            let k = j * row + i;
            m.quad(k, k + 1, k + row + 1, k + row);
        }
    }
    m.smooth_normals();
    m
}

/// A rough boulder of about `r` m.
fn boulder(r: f32, seed: u32) -> M {
    let mut m = M::rounded_box(Vec3::splat(r * 2.0), r * 0.7, 3);
    for vert in &mut m.vertices {
        let p = Vec3::from(vert.position);
        let n = super::texgen::fbm(
            Vec2::new(p.x * 3.0 + p.y * 2.0, p.z * 3.0 - p.y),
            64,
            3,
            0.5,
            seed,
        );
        vert.position = (p * (1.0 + 0.25 * n)).to_array();
    }
    m.smooth_normals();
    m.planar_uvs(1.0);
    m
}

/// The crag: a granite outcrop with its climbing face toward the square, the holds the
/// climbing finds on it (chalked jugs, crimps, crack lines, footholds), boulders at its
/// foot and on its top.
fn crag(kit: &mut Kit) {
    let ([x0, x1], [z0, z1], h) = city::CRAG;
    let (x0, x1, z0, z1, h) = (x0 as f32, x1 as f32, z0 as f32, z1 as f32, h as f32);
    // The body, a little inside the face so the sculpted front shows.
    let mut body = M::chamfer_box(Vec3::new(x1 - x0 - 0.15, h, z1 - z0), 0.25);
    body.planar_uvs(1.0);
    kit.add(
        mat::ROCK,
        &body,
        at(Vec3::new((x0 + x1) * 0.5 - 0.075, h * 0.5, (z0 + z1) * 0.5)),
    );
    let fbm = super::texgen::fbm;
    let face = sculpted_face(
        Vec3::new(x1, 0.0, z1),
        Vec3::NEG_Z,
        Vec3::Y,
        Vec3::X,
        (z1 - z0, h),
        0.18,
        |x, y| {
            let lump = fbm(Vec2::new(x * 0.45, y * 0.45), 64, 4, 0.55, 471);
            let bed = (y * 1.3 + lump * 0.5).fract();
            let ledge = super::texgen::smoothstep(0.85, 1.0, bed) * 0.05;
            // Stay within the collider's face (−0.14..0): the hands meet the holds.
            (-0.07 + 0.06 * lump + ledge).clamp(-0.14, 0.0)
        },
    );
    kit.add(mat::ROCK, &face, Mat4::IDENTITY);
    // The holds, as the climbing finds them.
    let el = crate::traverse::TraversalElement::face(
        crate::traverse::ElementKind::Rock,
        Vec3::new(x1, 0.3, z0 + 0.4),
        Vec3::new(x1, h, z1 - 0.4),
        Vec3::X,
        7,
    )
    .in_world(Entity::PLACEHOLDER, &Transform::IDENTITY);
    let jug = boulder(0.09, 481).transformed(Mat4::from_scale(Vec3::new(0.7, 0.6, 1.1)));
    let crimp = M::chamfer_box(Vec3::new(0.05, 0.03, 0.2), 0.008);
    let foot = boulder(0.05, 482);
    let mut chalked = jug.clone();
    chalked.tint(engine_core::math::Vec4::new(1.6, 1.6, 1.55, 1.0));
    let slot = {
        let mut m = M::chamfer_box(Vec3::new(0.02, 0.46, 0.05), 0.005);
        m.tint(engine_core::math::Vec4::new(0.15, 0.14, 0.13, 1.0));
        m
    };
    for (i, hold) in crate::traverse::rock_holds(&el, Vec3::Y).iter().enumerate() {
        let p = hold.at.as_vec3();
        use crate::traverse::HoldKind;
        let (m, out) = match hold.kind {
            HoldKind::Jug if i % 3 == 0 => (&chalked, 0.03),
            HoldKind::Jug => (&jug, 0.03),
            HoldKind::Crimp => (&crimp, 0.015),
            HoldKind::Crack => (&slot, 0.0),
            HoldKind::Foot => (&foot, 0.02),
        };
        kit.add(mat::ROCK, m, at(p + Vec3::X * out));
    }
    let mut rng = Rng(9173);
    for (k, (p, r)) in [
        (Vec3::new(x1 + 0.3, 0.2, z0 + 0.3), 0.5),
        (Vec3::new(x1 + 0.1, 0.15, z1 - 0.2), 0.35),
        (Vec3::new(x0 + 0.6, h + 0.1, z0 + 1.0), 0.6),
        (Vec3::new(x0 + 0.5, h + 0.1, z1 - 1.5), 0.45),
    ]
    .into_iter()
    .enumerate()
    {
        let b = boulder(r, 490 + k as u32);
        kit.add(
            mat::ROCK,
            &b,
            Mat4::from_rotation_translation(Quat::from_rotation_y(rng.range(0.0, 6.0)), p),
        );
    }
}

/// The frozen waterfall: blue ice in bulging columns over a rock body, a frost cap and
/// icicles at its corners.
fn icefall(kit: &mut Kit) {
    let ([x0, x1], [z0, z1], h) = city::ICEFALL;
    let (x0, x1, z0, z1, h) = (x0 as f32, x1 as f32, z0 as f32, z1 as f32, h as f32);
    let mut body = M::chamfer_box(Vec3::new(x1 - x0 - 0.15, h, z1 - z0), 0.2);
    body.planar_uvs(1.0);
    kit.add(
        mat::ROCK,
        &body,
        at(Vec3::new((x0 + x1) * 0.5 - 0.075, h * 0.5, (z0 + z1) * 0.5)),
    );
    let fbm = super::texgen::fbm;
    let face = sculpted_face(
        Vec3::new(x1, 0.0, z1),
        Vec3::NEG_Z,
        Vec3::Y,
        Vec3::X,
        (z1 - z0, h),
        0.15,
        |x, y| {
            let warp = fbm(Vec2::new(x * 0.6, y * 0.15), 64, 3, 0.5, 501);
            let column = 0.5 + 0.5 * (x * 4.2 + warp * 3.0).sin();
            let drip = fbm(Vec2::new(x * 3.0, y * 0.5), 64, 3, 0.5, 502);
            (-0.12 + 0.08 * column + 0.03 * drip).clamp(-0.14, 0.0)
        },
    );
    kit.add(mat::ICE, &face, Mat4::IDENTITY);
    let mut cap = M::rounded_box(Vec3::new(x1 - x0, 0.25, z1 - z0), 0.12, 2);
    cap.tint(engine_core::math::Vec4::new(1.3, 1.3, 1.3, 1.0));
    kit.add(
        mat::ICE,
        &cap,
        at(Vec3::new((x0 + x1) * 0.5 - 0.05, h + 0.06, (z0 + z1) * 0.5)),
    );
    let mut rng = Rng(5521);
    for _ in 0..34 {
        let z = if rng.next() < 0.5 {
            rng.range(z0 + 0.05, z0 + 0.6)
        } else {
            rng.range(z1 - 0.6, z1 - 0.05)
        };
        let len = rng.range(0.3, 1.6);
        let r = len * rng.range(0.05, 0.09);
        let icicle = M::lathe(
            &[
                Vec2::new(0.0, -len),
                Vec2::new(r * 0.3, -len * 0.8),
                Vec2::new(r * 0.7, -len * 0.4),
                Vec2::new(r, 0.0),
                Vec2::new(0.0, 0.0),
            ],
            7,
        );
        let y = h - rng.range(0.0, 2.5);
        kit.add(mat::ICE, &icicle, at(Vec3::new(x1 + 0.02, y, z)));
    }
}

/// Leaves scattered through an ellipsoid of half sizes `half` round `center`: `count`
/// small two-sided leaves, denser and darker inside.
pub(crate) fn leaf_cluster(center: Vec3, half: Vec3, count: u32, seed: u32) -> M {
    let mut m = M::new();
    let mut rng = Rng(seed.wrapping_mul(2_654_435_761) | 1);
    for _ in 0..count {
        let d = rng.unit() * rng.next().powf(0.4);
        let p = center + d * half;
        let n = rng.unit();
        let right = n.cross(Vec3::Y).normalize_or(Vec3::X);
        let fwd = right.cross(n);
        let (w, l) = (rng.range(0.04, 0.06), rng.range(0.07, 0.11));
        let shade = 0.55 + 0.45 * d.length() * rng.range(0.7, 1.1);
        let tint = engine_core::math::Vec4::new(
            shade * rng.range(0.85, 1.15),
            shade,
            shade * rng.range(0.8, 1.0),
            1.0,
        );
        let base = m.vertices.len() as u32;
        for (q, uv) in [
            (p - fwd * l * 0.5, Vec2::new(0.5, 0.0)),
            (p + right * w, Vec2::new(1.0, 0.5)),
            (p + fwd * l * 0.5, Vec2::new(0.5, 1.0)),
            (p - right * w, Vec2::new(0.0, 0.5)),
        ] {
            m.vertex(q, n, uv);
        }
        for v in &mut m.vertices[base as usize..] {
            v.color = tint.to_array();
        }
        m.quad(base, base + 1, base + 2, base + 3);
    }
    m
}

/// The climbing oak: a flared trunk to the first fork, the upper trunk to a second, limbs
/// into a wide crown of leaves. Its walkable branches are drawn bending under the walker
/// (see [`super::lines`]).
fn oak(kit: &mut Kit) {
    use super::meshgen::{Cap, Section};
    let base = v(city::OAK);
    let fork = base + Vec3::Y * city::OAK_TRUNK as f32;
    let upper = v(city::OAK_UPPER_FORK);
    let crown = Vec3::new(-26.2, 9.6, 11.7);
    let ring = |c: Vec3, r: f32| Section::ellipse(c, Vec2::splat(r));
    let mut trunk = M::loft(
        &[
            ring(base - Vec3::Y * 0.1, 0.62),
            ring(base + Vec3::Y * 0.3, 0.42),
            ring(base + Vec3::Y * 1.2, 0.33),
            ring(fork - Vec3::Y * 0.3, 0.3),
            ring(fork, 0.31),
            ring(upper, 0.24),
            ring(upper + Vec3::Y * 0.4, 0.2),
            ring(crown, 0.07),
        ],
        14,
        Cap::Flat,
        Cap::Round,
    );
    trunk.planar_uvs(1.0);
    kit.add(mat::BARK, &trunk, Mat4::IDENTITY);
    // Roots spreading at the foot.
    let mut rng = Rng(331);
    for k in 0..5 {
        let a = k as f32 * 1.256 + rng.range(-0.3, 0.3);
        let dir = Vec3::new(a.cos(), 0.0, a.sin());
        let root = M::tube(
            &[
                base + dir * 0.2 + Vec3::Y * 0.35,
                base + dir * 0.7 + Vec3::Y * 0.05,
                base + dir * 1.3 - Vec3::Y * 0.05,
            ],
            0.1,
            6,
        );
        kit.add(mat::BARK, &root, Mat4::IDENTITY);
    }
    // Limbs into the crown, and the leaves.
    let limbs = [
        (upper + Vec3::Y * 0.6, Vec3::new(-29.0, 8.6, 9.8)),
        (upper + Vec3::Y * 1.0, Vec3::new(-24.4, 9.0, 12.8)),
        (upper + Vec3::Y * 1.4, Vec3::new(-27.6, 10.0, 14.2)),
        (fork + Vec3::Y * 0.3, Vec3::new(-29.2, 6.8, 13.0)),
    ];
    for (a, b) in limbs {
        let limb = M::tube(&[a, (a + b) * 0.5 + Vec3::Y * 0.3, b], 0.09, 7);
        kit.add(mat::BARK, &limb, Mat4::IDENTITY);
    }
    for (k, (c, half, n)) in [
        (crown + Vec3::Y * 0.4, Vec3::new(2.4, 1.6, 2.4), 5200),
        (Vec3::new(-29.0, 8.8, 9.8), Vec3::new(1.6, 1.2, 1.6), 2400),
        (Vec3::new(-24.4, 9.2, 12.8), Vec3::new(1.6, 1.2, 1.6), 2400),
        (Vec3::new(-27.6, 10.2, 14.2), Vec3::new(1.5, 1.1, 1.5), 2200),
        (Vec3::new(-29.4, 7.2, 13.2), Vec3::new(1.3, 1.0, 1.3), 1800),
    ]
    .into_iter()
    .enumerate()
    {
        kit.add(
            mat::LEAVES,
            &leaf_cluster(c, half, n, 600 + k as u32),
            Mat4::IDENTITY,
        );
    }
}
