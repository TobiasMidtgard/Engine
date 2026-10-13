//! Market stalls: a weathered timber frame with a sagging striped canvas awning, a counter
//! and back shelves, a lantern with its light, and the goods of one trade.

use std::f32::consts::{FRAC_PI_2, FRAC_PI_4, PI, TAU};

use engine_assets::Handle;
use engine_core::prelude::*;
use engine_render::Mesh;
use engine_render::prelude::*;

use super::mesh::{beam, bevel_box, crystal, icosphere, lathe, tube};
use super::props::{barrel_mesh, crate_mesh, sack_mesh};
use super::{add_mesh, fix_winding, hex, moved, noise, rgba, sstep, tint, vtx};

/// What a stall is built from.
#[derive(Clone, Copy, Debug)]
pub struct StallMats {
    /// Planks, posts, crates and barrels.
    pub wood: Handle<StandardMaterial>,
    /// This stall's striped awning (alpha-masked valance, double sided).
    pub canvas: Handle<StandardMaterial>,
    /// Hoops, hooks and the lantern.
    pub iron: Handle<StandardMaterial>,
    pub crystal: Handle<StandardMaterial>,
    /// Glazed pots, vertex colored.
    pub pottery: Handle<StandardMaterial>,
    pub burlap: Handle<StandardMaterial>,
    pub lamp_glass: Handle<StandardMaterial>,
    /// Fruit and vegetables: waxy, vertex colored.
    pub produce: Handle<StandardMaterial>,
    /// Dyed linen, vertex colored, double sided.
    pub cloth: Handle<StandardMaterial>,
    pub wicker: Handle<StandardMaterial>,
    /// Polished blades.
    pub steel: Handle<StandardMaterial>,
    /// Scales, coins and fittings.
    pub brass: Handle<StandardMaterial>,
}

/// The trade a stall sells, which decides its goods.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Goods {
    /// Old Brannoc's frontier wares: glowing crystals, pots and flasks, brass scales and a
    /// strongbox on a draped counter, scrolls, sacks, crates and barrels.
    Trader,
    /// Baskets and crates of fruit and vegetables.
    Produce,
    /// Bolts and hanging lengths of dyed cloth.
    Cloth,
    /// Swords on a rack, shields on the back wall, an anvil beside.
    Smith,
    /// Jugs, amphorae, bowls and plates.
    Potter,
}

/// Half width and depth of the frame, and the front and back post heights.
const HW: f32 = 1.6;
const HD: f32 = 1.0;
const FRONT_H: f32 = 2.3;
const BACK_H: f32 = 2.75;
/// The counter's top and its distance in front of the stall's middle.
const TOP_Y: f32 = 1.0;
const COUNTER_Z: f32 = HD - 0.25;

/// One stall's meshes, by material.
#[derive(Default)]
struct Parts {
    wood: Mesh,
    canvas: Mesh,
    iron: Mesh,
    crystal: Mesh,
    pottery: Mesh,
    burlap: Mesh,
    glass: Mesh,
    produce: Mesh,
    cloth: Mesh,
    wicker: Mesh,
    steel: Mesh,
    brass: Mesh,
}

/// Spawns a stall at `at` (its front faces local +Z; the ground is at local y = 0) and
/// returns its entities, the lantern's light included.
pub fn spawn_stall(
    world: &mut World,
    mats: &StallMats,
    at: Transform,
    goods: Goods,
    seed: u64,
) -> Vec<Entity> {
    let mut rng = Rng::new(seed);
    let mut p = Parts::default();
    frame(&mut p, &mut rng);
    awning(&mut p, seed as u32);
    let lantern_at = Vec3::new(-0.95, FRONT_H - 0.62, HD + 0.02);
    lantern(&mut p, lantern_at);
    match goods {
        Goods::Trader => trader(&mut p, &mut rng),
        Goods::Produce => produce(&mut p, &mut rng),
        Goods::Cloth => cloth(&mut p, &mut rng),
        Goods::Smith => smith(&mut p, &mut rng),
        Goods::Potter => potter(&mut p, &mut rng),
    }
    let mut out = Vec::new();
    for (mesh, mat, shadows) in [
        (p.wood, mats.wood, true),
        (p.canvas, mats.canvas, true),
        (p.iron, mats.iron, true),
        (p.crystal, mats.crystal, true),
        (p.pottery, mats.pottery, true),
        (p.burlap, mats.burlap, true),
        (p.glass, mats.lamp_glass, false),
        (p.produce, mats.produce, true),
        (p.cloth, mats.cloth, true),
        (p.wicker, mats.wicker, true),
        (p.steel, mats.steel, true),
        (p.brass, mats.brass, true),
    ] {
        if mesh.triangle_count() == 0 {
            continue;
        }
        let h = add_mesh(world, mesh);
        let mut r = MeshRenderer::new(h, mat);
        r.cast_shadows = shadows;
        out.push(world.spawn((at, r)).id());
    }
    let light_at = at.transform_point((lantern_at + Vec3::Y * 0.1).as_dvec3());
    out.push(
        world
            .spawn((
                PointLight {
                    color: [1.0, 0.6, 0.3],
                    intensity_lm: 380.0,
                    range: 9.0,
                    radius: 0.05,
                },
                Transform::from_translation(light_at),
            ))
            .id(),
    );
    out
}

/// Posts, beams and braces, the counter and the back shelves.
fn frame(p: &mut Parts, rng: &mut Rng) {
    let weathered = Vec4::new(0.92, 0.9, 0.88, 1.0);
    for (x, z, hgt) in [
        (-HW, HD, FRONT_H),
        (HW, HD, FRONT_H),
        (-HW, -HD, BACK_H),
        (HW, -HD, BACK_H),
    ] {
        let lean = Vec3::new(rng.range(-0.02, 0.02), 0.0, rng.range(-0.02, 0.02));
        p.wood.merge(&tint(
            beam(
                Vec3::new(x, -0.3, z),
                Vec3::new(x, hgt + 0.08, z) + lean,
                0.12,
                0.12,
                0.018,
            ),
            weathered * rng.range(0.85, 1.0),
        ));
    }
    for (a, b) in [
        (
            Vec3::new(-HW - 0.15, FRONT_H, HD),
            Vec3::new(HW + 0.15, FRONT_H, HD),
        ),
        (
            Vec3::new(-HW - 0.15, BACK_H, -HD),
            Vec3::new(HW + 0.15, BACK_H, -HD),
        ),
        (
            Vec3::new(-HW, FRONT_H, HD + 0.1),
            Vec3::new(-HW, BACK_H, -HD - 0.1),
        ),
        (
            Vec3::new(HW, FRONT_H, HD + 0.1),
            Vec3::new(HW, BACK_H, -HD - 0.1),
        ),
        (
            Vec3::new(-HW, 1.5, -HD),
            Vec3::new(-HW + 0.7, BACK_H - 0.05, -HD),
        ),
        (
            Vec3::new(HW, 1.5, -HD),
            Vec3::new(HW - 0.7, BACK_H - 0.05, -HD),
        ),
    ] {
        p.wood.merge(&tint(
            beam(a, b, 0.1, 0.09, 0.015),
            weathered * rng.range(0.85, 1.0),
        ));
    }
    // Counter: a thick top over a front of vertical boards.
    p.wood.merge(&tint(
        moved(
            bevel_box(Vec3::new(2.9, 0.07, 0.62), 0.015, 0),
            Mat4::from_translation(Vec3::new(0.0, TOP_Y, COUNTER_Z - 0.05)),
        ),
        Vec4::new(1.05, 0.98, 0.9, 1.0),
    ));
    let boards = 13;
    for k in 0..boards {
        let x = -1.4 + 2.8 * (k as f32 + 0.5) / boards as f32;
        let hgt = TOP_Y - 0.04 + rng.range(-0.03, 0.0);
        p.wood.merge(&tint(
            beam(
                Vec3::new(x, 0.0, COUNTER_Z + 0.24),
                Vec3::new(x + rng.range(-0.01, 0.01), hgt, COUNTER_Z + 0.24),
                0.2,
                0.025,
                0.006,
            ),
            weathered * rng.range(0.75, 1.0),
        ));
    }
    // Back shelves.
    for y in [0.7f32, 1.35] {
        p.wood.merge(&tint(
            moved(
                bevel_box(Vec3::new(3.0, 0.04, 0.38), 0.01, 0),
                Mat4::from_translation(Vec3::new(0.0, y, -HD + 0.2)),
            ),
            weathered,
        ));
    }
}

/// The canvas: sags between the beams, overhangs the front, and a scalloped valance hangs
/// from its front edge. UVs: u across, v 0..0.82 roof (back to front), 0.82..1 valance.
fn awning(p: &mut Parts, seed: u32) {
    let (nu, nv) = (36usize, 26usize);
    let mut cloth = Mesh::default();
    let width = 2.0 * HW + 0.5;
    let wseed = 400 + seed % 97;
    for j in 0..=nv {
        let v = j as f32 / nv as f32;
        for i in 0..=nu {
            let u = i as f32 / nu as f32;
            let x = (u - 0.5) * width;
            let z = -HD - 0.12 + v * (2.0 * HD + 0.45);
            let y_line = BACK_H + 0.1 + (FRONT_H + 0.04 - BACK_H - 0.1) * v;
            let sag = 0.13 * (PI * u).sin().max(0.0).powf(0.8) * (PI * v.min(0.85) / 0.85).sin();
            let wrinkle = 0.012 * ((u * 23.0 + v * 4.0).sin() * (v * 11.0).cos())
                + 0.01 * noise::n2(u * 6.0, v * 6.0, wseed);
            let pos = Vec3::new(x, y_line - sag + wrinkle, z);
            cloth
                .vertices
                .push(vtx(pos, Vec3::Y, Vec2::new(u, v * 0.82), Vec4::ONE));
        }
    }
    let row = (nu + 1) as u32;
    for j in 0..nv as u32 {
        for i in 0..nu as u32 {
            let a = j * row + i;
            cloth
                .indices
                .extend_from_slice(&[a, a + row, a + 1, a + 1, a + row, a + row + 1]);
        }
    }
    cloth.compute_normals();
    for v in &mut cloth.vertices {
        if v.normal[1] < 0.0 {
            v.normal = (-Vec3::from(v.normal)).to_array();
        }
    }
    fix_winding(&mut cloth);
    let base = cloth.vertices.len() as u32;
    let front_z = HD + 0.33;
    for j in 0..=4 {
        let v = j as f32 / 4.0;
        for i in 0..=nu {
            let u = i as f32 / nu as f32;
            let x = (u - 0.5) * width;
            let y = FRONT_H + 0.04 - v * 0.3;
            let pos = Vec3::new(x, y, front_z + 0.02 * (u * 40.0).sin() * v);
            cloth
                .vertices
                .push(vtx(pos, Vec3::Z, Vec2::new(u, 0.82 + 0.18 * v), Vec4::ONE));
        }
    }
    for j in 0..4u32 {
        for i in 0..nu as u32 {
            let a = base + j * row + i;
            cloth
                .indices
                .extend_from_slice(&[a, a + row, a + 1, a + 1, a + row, a + row + 1]);
        }
    }
    p.canvas.merge(&cloth);
}

/// A lantern hanging under the front beam at `at` (its base).
fn lantern(p: &mut Parts, at: Vec3) {
    p.iron.merge(&tube(
        at + Vec3::Y * 0.3,
        Vec3::new(at.x, FRONT_H - 0.04, at.z),
        0.006,
        6,
    ));
    p.iron.merge(&moved(
        lathe(&[(0.0, 0.26), (0.1, 0.2), (0.085, 0.18)], 16, 0.1),
        Mat4::from_translation(at),
    ));
    p.iron.merge(&moved(
        lathe(
            &[(0.0, -0.01), (0.08, -0.01), (0.08, 0.02), (0.0, 0.02)],
            16,
            0.08,
        ),
        Mat4::from_translation(at),
    ));
    for k in 0..4 {
        let a = k as f32 * FRAC_PI_2 + FRAC_PI_4;
        let o = Vec3::new(a.cos() * 0.075, 0.0, a.sin() * 0.075);
        p.iron
            .merge(&tube(at + o, at + o + Vec3::Y * 0.19, 0.006, 5));
    }
    p.glass.merge(&moved(
        lathe(
            &[
                (0.0, 0.02),
                (0.06, 0.03),
                (0.068, 0.1),
                (0.06, 0.18),
                (0.0, 0.19),
            ],
            16,
            0.06,
        ),
        Mat4::from_translation(at),
    ));
}

/// A crate of planks standing on the ground at `pos` (its base center), turned by `yaw`.
fn crate_at(p: &mut Parts, rng: &mut Rng, pos: Vec3, size: Vec3, yaw: f32) {
    let c = crate_mesh(size, rng);
    p.wood.merge(&moved(
        c,
        Mat4::from_rotation_translation(Quat::from_rotation_y(yaw), pos + Vec3::Y * (size.y * 0.5)),
    ));
}

fn barrel_at(p: &mut Parts, pos: Vec3, height: f32, radius: f32) {
    let (w, i) = barrel_mesh(height, radius);
    p.wood.merge(&tint(
        moved(w, Mat4::from_translation(pos)),
        Vec4::new(1.0, 0.82, 0.68, 1.0),
    ));
    p.iron.merge(&moved(i, Mat4::from_translation(pos)));
}

/// A glazed pot: `kind` picks the profile (0 jar, 1 jug, 2 amphora, 3 bowl, 4 vase).
fn pot(kind: u32, hgt: f32, r: f32) -> Mesh {
    let prof: Vec<(f32, f32)> = match kind % 5 {
        0 => vec![
            (0.0, 0.0),
            (r * 0.6, 0.0),
            (r, hgt * 0.35),
            (r * 0.9, hgt * 0.7),
            (r * 0.45, hgt * 0.85),
            (r * 0.55, hgt),
            (r * 0.42, hgt),
        ],
        1 => vec![
            (0.0, 0.0),
            (r * 0.7, 0.0),
            (r, hgt * 0.3),
            (r * 0.85, hgt * 0.6),
            (r * 0.38, hgt * 0.8),
            (r * 0.42, hgt * 0.95),
            (r * 0.5, hgt),
            (r * 0.36, hgt),
        ],
        2 => vec![
            (0.0, 0.0),
            (r * 0.15, 0.02 * hgt),
            (r * 0.5, hgt * 0.2),
            (r, hgt * 0.55),
            (r * 0.7, hgt * 0.78),
            (r * 0.25, hgt * 0.86),
            (r * 0.3, hgt),
            (r * 0.2, hgt),
        ],
        3 => vec![
            (0.0, 0.0),
            (r * 0.45, 0.0),
            (r * 0.9, hgt * 0.6),
            (r, hgt),
            (r * 0.9, hgt),
            (r * 0.8, hgt * 0.65),
            (0.0, hgt * 0.2),
        ],
        _ => vec![
            (0.0, 0.0),
            (r * 0.55, 0.0),
            (r * 0.7, hgt * 0.15),
            (r * 0.35, hgt * 0.5),
            (r * 0.3, hgt * 0.75),
            (r * 0.6, hgt),
            (r * 0.5, hgt),
        ],
    };
    lathe(&prof, 28, r)
}

/// Clay and glaze colors for pots.
const GLAZES: [u32; 8] = [
    0x8a4a2a, 0x3d5a5e, 0x9a7a50, 0x6a3424, 0x2f4a6e, 0xb08a5a, 0x5a6e3a, 0xd8cdb0,
];

/// A piece of fruit or vegetable: `kind` 0 apple, 1 orange, 2 cabbage, 3 pumpkin, 4 pear,
/// 5 onion; base center at the origin.
fn fruit(kind: u32, size: f32, seed: u32) -> Mesh {
    let (dirs, faces) = icosphere(if kind == 3 || kind == 2 { 3 } else { 2 });
    let (color, squash, ribs, lumpy): (u32, f32, f32, f32) = match kind % 6 {
        0 => (
            if seed & 1 == 0 { 0x9c1a14 } else { 0x7aa02c },
            0.9,
            0.0,
            0.04,
        ),
        1 => (0xe0801c, 0.95, 0.0, 0.02),
        2 => (0x5f8a32, 0.85, 0.0, 0.12),
        3 => (0xd2681a, 0.62, 10.0, 0.03),
        4 => (0xb5b03a, 1.25, 0.0, 0.03),
        _ => (0xb07a3e, 1.0, 0.0, 0.03),
    };
    let base = rgba(hex(color));
    let mut m = Mesh {
        vertices: dirs
            .iter()
            .map(|&d| {
                let n = noise::n3(d.x * 3.0, d.y * 3.0, d.z * 3.0, seed);
                let a = d.z.atan2(d.x);
                // `ribs` narrow creases between rounded lobes.
                let rib = if ribs > 0.0 {
                    1.0 - 0.14 * (1.0 - (a * ribs * 0.5).sin().abs()).powi(3)
                } else {
                    1.0
                };
                let mut q = d * (1.0 + lumpy * n) * rib;
                // Pears narrow toward the top, apples dimple at the stalk.
                if kind == 4 {
                    let k = sstep(-0.2, 1.0, d.y);
                    q.x *= 1.0 - 0.45 * k;
                    q.z *= 1.0 - 0.45 * k;
                }
                if kind == 0 || kind == 3 {
                    q.y -= 0.15 * sstep(0.85, 1.0, d.y);
                }
                q.y *= squash;
                let pos = (q + Vec3::Y * squash) * size;
                let shade = 0.85 + 0.15 * n + if kind == 0 { 0.2 * d.y.max(0.0) } else { 0.0 };
                let c = Vec4::new(base.x * shade, base.y * shade, base.z * shade, 1.0);
                vtx(pos, d, Vec2::new(a * 0.1, d.y * 0.1), c)
            })
            .collect(),
        indices: faces.iter().flat_map(|f| f.iter().copied()).collect(),
    };
    m.compute_normals();
    m
}

/// A wicker basket (bowl), base at the origin; returns its rim height.
fn basket(p: &mut Parts, at: Vec3, r: f32, hgt: f32) -> f32 {
    let prof = [
        (0.0, 0.01),
        (r * 0.7, 0.0),
        (r * 0.95, hgt * 0.7),
        (r, hgt),
        (r * 1.04, hgt + 0.012),
        (r * 0.96, hgt + 0.012),
        (r * 0.9, hgt * 0.7),
        (r * 0.65, 0.02),
        (0.0, 0.03),
    ];
    p.wicker
        .merge(&moved(lathe(&prof, 32, r), Mat4::from_translation(at)));
    hgt
}

/// Fill a circle of radius `r` at `at` with a heap of fruit of `kind`, sized `size`.
fn heap(p: &mut Parts, rng: &mut Rng, at: Vec3, r: f32, kind: u32, size: f32) {
    let layers = 3;
    for l in 0..layers {
        let lr = r * (1.0 - l as f32 * 0.32);
        let n = ((lr / size).powi(2) * 0.8).ceil().max(1.0) as u32;
        for k in 0..n {
            let a = k as f32 / n as f32 * TAU + rng.range(0.0, 0.6);
            let d = if n == 1 {
                0.0
            } else {
                lr * rng.range(0.35, 0.9)
            };
            let s = size * rng.range(0.85, 1.12);
            let pos = at + Vec3::new(a.cos() * d, l as f32 * size * 1.3, a.sin() * d);
            p.produce.merge(&moved(
                fruit(kind, s, rng.next_u32()),
                Mat4::from_rotation_translation(Quat::from_rotation_y(rng.range(0.0, TAU)), pos),
            ));
        }
    }
}

/// A runner of dyed linen across the counter at `x0`, `width` wide, falling over the front
/// edge in folds to a fringed hem: indigo, with gold borders and madder bands.
fn runner(p: &mut Parts, x0: f32, width: f32) {
    let top = TOP_Y + 0.037;
    let back = COUNTER_Z - 0.3;
    // The counter top's front edge.
    let edge = COUNTER_Z + 0.26;
    let (bend, drop) = (0.016, 0.4);
    let flat = edge - back;
    let len = flat + bend * FRAC_PI_2 + drop;
    let (nu, ns) = (90usize, 70usize);
    let (indigo, gold, madder) = (hex(0x1c2650), hex(0xc39a3a), hex(0x8a1c18));
    let color = |u: f32, s: f32| -> Vec3 {
        let across = u.min(1.0 - u) * width;
        let down = len - s;
        let band = |d: f32, a: f32, b: f32| (a..b).contains(&d);
        if band(across, 0.0, 0.035) || band(down, 0.0, 0.03) {
            gold
        } else if band(across, 0.05, 0.062) || band(down, 0.045, 0.06) || band(down, 0.33, 0.345) {
            madder
        } else if band(down, 0.07, 0.08) || band(down, 0.31, 0.32) {
            gold * 0.9
        } else {
            indigo
        }
    };
    let base = p.cloth.vertices.len() as u32;
    for j in 0..=ns {
        let s = j as f32 / ns as f32 * len;
        for i in 0..=nu {
            let u = i as f32 / nu as f32;
            let x = x0 + (u - 0.5) * width;
            let pos = if s < flat {
                Vec3::new(x, top, back + s)
            } else if s < flat + bend * FRAC_PI_2 {
                let a = (s - flat) / bend;
                Vec3::new(x, top - bend + a.cos() * bend, edge + a.sin() * bend)
            } else {
                let d = s - flat - bend * FRAC_PI_2;
                let k = d / drop;
                // Folds deepen toward the hem and only ever stand out from the boards.
                let xs = u * width;
                let fold = k
                    * (0.011 * (1.0 + (xs * 21.0 + 0.6).sin())
                        + 0.005 * (1.0 + (xs * 47.0 + 1.3).sin()));
                Vec3::new(
                    x,
                    top - bend - d - 0.008 * k * (xs * 13.0).sin(),
                    edge + bend + fold,
                )
            };
            let c = color(u, s);
            p.cloth
                .vertices
                .push(vtx(pos, Vec3::Y, Vec2::new(x, s), rgba(c)));
        }
    }
    let row = (nu + 1) as u32;
    let start = p.cloth.indices.len();
    for j in 0..ns as u32 {
        for i in 0..nu as u32 {
            let a = base + j * row + i;
            p.cloth
                .indices
                .extend_from_slice(&[a, a + 1, a + row, a + 1, a + row + 1, a + row]);
        }
    }
    smooth_piece(&mut p.cloth, base, start);
    // Fringe: a gold tassel every 2 cm along the hem.
    let hem_y = top - bend - drop;
    let n = (width / 0.02) as usize;
    for k in 0..=n {
        let xs = k as f32 / n as f32 * width;
        let x = x0 - width * 0.5 + xs;
        let fold =
            0.011 * (1.0 + (xs * 21.0 + 0.6).sin()) + 0.005 * (1.0 + (xs * 47.0 + 1.3).sin());
        let a = Vec3::new(
            x,
            hem_y - 0.008 * (xs * 13.0).sin() + 0.004,
            edge + bend + fold,
        );
        let b = a + Vec3::new(0.002 * (xs * 91.0).sin(), -0.045, 0.004);
        p.cloth
            .merge(&tint(tube(a, b, 0.0032, 4), rgba(gold * 0.85)));
    }
}

/// Recompute smooth normals for the vertices from `base` and indices from `start` of `m`, so
/// a piece doesn't blend with what was merged before it.
fn smooth_piece(m: &mut Mesh, base: u32, start: usize) {
    let mut piece = Mesh {
        vertices: m.vertices[base as usize..].to_vec(),
        indices: m.indices[start..].iter().map(|i| i - base).collect(),
    };
    piece.compute_normals();
    for (k, v) in piece.vertices.into_iter().enumerate() {
        m.vertices[base as usize + k].normal = v.normal;
    }
}

/// A brass balance standing on the counter at `at`: a post, a beam and two pans on chains,
/// the right pan heavier with coins.
fn scales(p: &mut Parts, at: Vec3) {
    p.brass.merge(&moved(
        lathe(
            &[
                (0.0, 0.0),
                (0.075, 0.0),
                (0.075, 0.012),
                (0.055, 0.022),
                (0.016, 0.04),
                (0.013, 0.2),
                (0.018, 0.21),
                (0.011, 0.22),
                (0.009, 0.34),
                (0.0, 0.345),
            ],
            24,
            0.05,
        ),
        Mat4::from_translation(at),
    ));
    // The beam tips a little toward the heavier pan.
    let tilt = -0.06f32;
    let pivot = at + Vec3::Y * 0.33;
    let arm = Vec3::new(tilt.cos(), tilt.sin(), 0.0) * 0.18;
    p.brass.merge(&tube(pivot - arm, pivot + arm, 0.0055, 8));
    for end in [pivot - arm, pivot + arm] {
        p.brass.merge(&moved(
            lathe(&[(0.0, -0.01), (0.012, 0.0), (0.0, 0.01)], 10, 0.01),
            Mat4::from_translation(end),
        ));
    }
    p.brass.merge(&moved(
        lathe(&[(0.0, -0.012), (0.016, 0.0), (0.0, 0.03)], 12, 0.01),
        Mat4::from_translation(pivot + Vec3::Y * 0.012),
    ));
    for (k, end) in [pivot - arm, pivot + arm].into_iter().enumerate() {
        let pan = Vec3::new(end.x, at.y + 0.075 + end.y - pivot.y, end.z);
        p.brass.merge(&moved(
            lathe(
                &[
                    (0.0, 0.0),
                    (0.05, 0.004),
                    (0.074, 0.016),
                    (0.078, 0.021),
                    (0.072, 0.02),
                    (0.048, 0.01),
                    (0.0, 0.006),
                ],
                28,
                0.07,
            ),
            Mat4::from_translation(pan),
        ));
        for c in 0..3 {
            let a = c as f32 * TAU / 3.0 + 0.5;
            let rim = pan + Vec3::new(a.cos() * 0.072, 0.02, a.sin() * 0.072);
            p.brass.merge(&tube(end, rim, 0.0012, 3));
        }
        if k == 1 {
            for (c, (dx, dz)) in [
                (0.0f32, 0.0f32),
                (0.02, 0.012),
                (-0.018, 0.01),
                (0.005, -0.02),
            ]
            .into_iter()
            .enumerate()
            {
                p.brass.merge(&moved(
                    coin(),
                    Mat4::from_rotation_translation(
                        Quat::from_euler(EulerRot::YXZ, c as f32 * 1.3, 0.12 * c as f32, 0.0),
                        pan + Vec3::new(dx, 0.008 + c as f32 * 0.0025, dz),
                    ),
                ));
            }
        }
    }
}

/// A coin lying flat, its base at the origin.
fn coin() -> Mesh {
    lathe(
        &[
            (0.0, 0.0),
            (0.011, 0.0),
            (0.0125, 0.0015),
            (0.011, 0.003),
            (0.0, 0.003),
        ],
        14,
        0.012,
    )
}

/// Stacks of coins at `at` (one per offset, `n` coins high).
fn coin_stacks(p: &mut Parts, rng: &mut Rng, at: Vec3, stacks: &[(f32, f32, u32)]) {
    for &(dx, dz, n) in stacks {
        for k in 0..n {
            let j = Vec3::new(rng.range(-0.0015, 0.0015), 0.0, rng.range(-0.0015, 0.0015));
            p.brass.merge(&moved(
                coin(),
                Mat4::from_rotation_translation(
                    Quat::from_rotation_y(rng.range(0.0, TAU)),
                    at + Vec3::new(dx, k as f32 * 0.003, dz) + j,
                ),
            ));
        }
    }
}

/// An iron-bound strongbox with a rounded lid, standing at `at`, turned by `yaw`.
fn strongbox(p: &mut Parts, at: Vec3, yaw: f32) {
    let m = Mat4::from_rotation_translation(Quat::from_rotation_y(yaw), at);
    let (w, h, d) = (0.34f32, 0.15, 0.22);
    p.wood.merge(&tint(
        moved(
            bevel_box(Vec3::new(w, h, d), 0.01, 0),
            m * Mat4::from_translation(Vec3::Y * (h * 0.5)),
        ),
        Vec4::new(0.75, 0.55, 0.4, 1.0),
    ));
    // The lid: a half-round barrel top along x.
    let lid_r = d * 0.5;
    let mut lid = lathe(
        &[
            (0.0, -w * 0.5),
            (lid_r, -w * 0.5),
            (lid_r, w * 0.5),
            (0.0, w * 0.5),
        ],
        24,
        lid_r,
    );
    // Lathe around y, laid along x, then squashed and cut to its upper half.
    lid.transform(&Mat4::from_rotation_z(-FRAC_PI_2));
    for v in &mut lid.vertices {
        v.position[1] = v.position[1].max(0.0) * 0.55;
    }
    lid.compute_normals();
    p.wood.merge(&tint(
        moved(lid, m * Mat4::from_translation(Vec3::Y * h)),
        Vec4::new(0.72, 0.52, 0.38, 1.0),
    ));
    // Iron bands over body and lid, and a lock plate.
    for x in [-w * 0.32, 0.0, w * 0.32] {
        p.iron.merge(&moved(
            bevel_box(Vec3::new(0.022, h + 0.004, d + 0.008), 0.003, 1),
            m * Mat4::from_translation(Vec3::new(x, h * 0.5, 0.0)),
        ));
        let band: Vec<(f32, f32)> = (0..=8)
            .map(|k| {
                let a = k as f32 / 8.0 * PI;
                (a.cos() * (lid_r + 0.004), a.sin() * (lid_r + 0.004) * 0.55)
            })
            .collect();
        for pair in band.windows(2) {
            let (a, b) = (pair[0], pair[1]);
            p.iron.merge(&moved(
                beam(
                    Vec3::new(0.0, a.1, a.0),
                    Vec3::new(0.0, b.1, b.0),
                    0.022,
                    0.004,
                    0.0015,
                ),
                m * Mat4::from_translation(Vec3::new(x, h, 0.0)),
            ));
        }
    }
    p.brass.merge(&moved(
        bevel_box(Vec3::new(0.05, 0.06, 0.012), 0.003, 0),
        m * Mat4::from_translation(Vec3::new(0.0, h - 0.01, d * 0.5 + 0.004)),
    ));
}

/// A stoneware flask with a cork, base at the origin.
fn flask(hgt: f32, r: f32) -> (Mesh, Mesh) {
    let body = lathe(
        &[
            (0.0, 0.0),
            (r * 0.85, 0.0),
            (r, hgt * 0.08),
            (r, hgt * 0.52),
            (r * 0.85, hgt * 0.66),
            (r * 0.34, hgt * 0.78),
            (r * 0.3, hgt * 0.95),
            (r * 0.37, hgt * 0.97),
            (r * 0.33, hgt),
            (0.0, hgt),
        ],
        22,
        r,
    );
    let cork = lathe(
        &[
            (0.0, hgt * 0.96),
            (r * 0.27, hgt * 0.96),
            (r * 0.31, hgt + 0.022),
            (0.0, hgt + 0.024),
        ],
        10,
        r * 0.3,
    );
    (body, cork)
}

/// Old Brannoc's stall, as on the gallery's backdrop.
fn trader(p: &mut Parts, rng: &mut Rng) {
    for (pos, size, yaw) in [
        (Vec3::new(-2.25, 0.0, 0.7), Vec3::new(0.7, 0.55, 0.55), 0.3),
        (
            Vec3::new(-2.2, 0.55, 0.65),
            Vec3::new(0.55, 0.42, 0.45),
            0.1,
        ),
        (
            Vec3::new(-2.5, 0.0, -0.35),
            Vec3::new(0.62, 0.62, 0.62),
            -0.2,
        ),
        (Vec3::new(2.05, 0.0, -1.5), Vec3::new(0.8, 0.5, 0.6), 0.5),
    ] {
        crate_at(p, rng, pos, size, yaw);
    }
    barrel_at(p, Vec3::new(2.2, 0.0, 0.65), 0.95, 0.3);
    barrel_at(p, Vec3::new(2.45, 0.0, 0.05), 0.8, 0.27);
    for k in 0..7 {
        let a = k as f32 * 2.3;
        let lean = Quat::from_euler(EulerRot::YXZ, a, rng.range(0.1, 0.5), 0.0);
        let len = rng.range(0.12, 0.3);
        p.crystal.merge(&moved(
            crystal(len, len * 0.18),
            Mat4::from_rotation_translation(
                lean,
                Vec3::new(-0.75, TOP_Y + 0.035, COUNTER_Z - 0.05)
                    + Vec3::new(a.cos(), 0.0, a.sin()) * 0.05,
            ),
        ));
    }
    for k in 0..4 {
        let len = rng.range(0.1, 0.18);
        p.crystal.merge(&moved(
            crystal(len, len * 0.2),
            Mat4::from_rotation_translation(
                Quat::from_euler(EulerRot::YXZ, k as f32 * 1.7, 0.3, 0.0),
                Vec3::new(0.9 + k as f32 * 0.06, 1.37, -HD + 0.2),
            ),
        ));
    }
    for (x, y, z, hgt, r, color) in [
        (0.1f32, TOP_Y + 0.035, COUNTER_Z - 0.1, 0.26, 0.11, 0x8a4a2a),
        (0.38, TOP_Y + 0.035, COUNTER_Z - 0.02, 0.18, 0.08, 0x3d5a5e),
        (-0.3, 1.37, -HD + 0.2, 0.22, 0.09, 0x9a7a50),
        (-0.05, 1.37, -HD + 0.18, 0.3, 0.1, 0x6a3424),
        (0.4, 0.72, -HD + 0.2, 0.24, 0.12, 0x8a4a2a),
    ] {
        p.pottery.merge(&tint(
            moved(pot(0, hgt, r), Mat4::from_translation(Vec3::new(x, y, z))),
            rgba(hex(color)),
        ));
    }
    for (k, (x, z, s)) in [
        (-1.9f32, -1.25f32, 0.32f32),
        (-1.45, -1.35, 0.27),
        (1.4, 1.3, 0.3),
    ]
    .into_iter()
    .enumerate()
    {
        p.burlap.merge(&moved(
            sack_mesh(Vec3::new(s, s * 1.2, s), 500 + k as u32),
            Mat4::from_translation(Vec3::new(x, s * 0.7, z)),
        ));
    }
    let top = TOP_Y + 0.037;
    runner(p, -0.2, 1.0);
    scales(p, Vec3::new(0.95, top, COUNTER_Z - 0.12));
    coin_stacks(
        p,
        rng,
        Vec3::new(0.62, top, COUNTER_Z + 0.08),
        &[(0.0, 0.0, 9), (0.03, 0.02, 5), (-0.01, 0.035, 3)],
    );
    coin_stacks(
        p,
        rng,
        Vec3::new(1.28, top, COUNTER_Z + 0.06),
        &[(0.0, 0.0, 6)],
    );
    strongbox(p, Vec3::new(-1.15, top, COUNTER_Z - 0.12), 0.18);
    // Flasks along the top shelf, corked.
    for (k, x) in [-1.38f32, -1.22, -1.07, -0.9, 0.3, 0.46, 0.62]
        .into_iter()
        .enumerate()
    {
        let (hgt, r) = (rng.range(0.13, 0.2), rng.range(0.03, 0.042));
        let (body, cork) = flask(hgt, r);
        let at = Mat4::from_translation(Vec3::new(x, 1.37, -HD + 0.2 + rng.range(-0.04, 0.04)));
        let glaze = rgba(hex([0x5a3a22, 0x3d5a3a, 0x2f4a6e, 0x8a6a3a][k % 4]));
        p.pottery.merge(&tint(moved(body, at), glaze));
        p.wood
            .merge(&tint(moved(cork, at), Vec4::new(1.2, 1.0, 0.75, 1.0)));
    }
    // Rolled scrolls stacked on the lower shelf, tied with red cord.
    let parchment = rgba(hex(0xd9c79c));
    for (x, y) in [
        (-1.32f32, 0.0f32),
        (-1.24, 0.0),
        (-1.16, 0.0),
        (-1.28, 0.068),
        (-1.2, 0.068),
    ] {
        let r = 0.034;
        let y = 0.72 + r + y;
        let a = Vec3::new(x, y, -HD + 0.04);
        let b = Vec3::new(x + rng.range(-0.01, 0.01), y, -HD + 0.36);
        p.cloth
            .merge(&tint(tube(a, b, r, 14), parchment * rng.range(0.9, 1.05)));
        let mid = (a + b) * 0.5 + Vec3::Z * rng.range(-0.05, 0.05);
        p.cloth.merge(&tint(
            tube(mid - Vec3::Z * 0.006, mid + Vec3::Z * 0.006, r + 0.002, 14),
            rgba(hex(0x8a1c18)),
        ));
    }
    // Folded rugs.
    let mut y = 0.72;
    for (k, color) in [0x7a1f2bu32, 0x264d7a, 0xb5532a, 0x2a2a30]
        .into_iter()
        .enumerate()
    {
        let h = 0.04 + 0.01 * (k % 2) as f32;
        p.cloth.merge(&tint(
            moved(
                bevel_box(Vec3::new(0.4, h, 0.3), h * 0.45, 0),
                Mat4::from_rotation_translation(
                    Quat::from_rotation_y(rng.range(-0.08, 0.08)),
                    Vec3::new(-0.4, y + h * 0.5, -HD + 0.2),
                ),
            ),
            rgba(hex(color)),
        ));
        y += h;
    }
    // Charms on cords from the front beam: small crystals that catch the light.
    for (k, x) in [0.75f32, 0.88, 1.0, 1.12].into_iter().enumerate() {
        let len = 0.18 + 0.1 * ((k * 7) % 3) as f32;
        let top_at = Vec3::new(x, FRONT_H - 0.05, HD + 0.03);
        let end = top_at - Vec3::Y * len;
        p.iron.merge(&tube(end, top_at, 0.0015, 3));
        p.crystal.merge(&moved(
            crystal(0.06, 0.012),
            Mat4::from_rotation_translation(Quat::from_rotation_x(PI), end + Vec3::Y * 0.005),
        ));
    }
}

/// Fruit and vegetables: baskets on the counter, tilted crates on the shelf, sacks below.
fn produce(p: &mut Parts, rng: &mut Rng) {
    let top = TOP_Y + 0.035;
    for (x, z, r, kind, size) in [
        (-1.05f32, COUNTER_Z - 0.08, 0.22f32, 0u32, 0.04f32),
        (-0.5, COUNTER_Z - 0.02, 0.2, 1, 0.042),
        (0.05, COUNTER_Z - 0.1, 0.24, 4, 0.038),
        (0.62, COUNTER_Z - 0.04, 0.2, 5, 0.035),
        (1.12, COUNTER_Z - 0.08, 0.22, 0, 0.04),
    ] {
        let rim = basket(p, Vec3::new(x, top, z), r, 0.12);
        heap(
            p,
            rng,
            Vec3::new(x, top + rim * 0.45, z),
            r * 0.82,
            kind,
            size,
        );
    }
    // Shelf crates, tilted toward the customer, heaped with cabbages and onions.
    for (k, x) in [-0.95f32, 0.0, 0.95].into_iter().enumerate() {
        let size = Vec3::new(0.7, 0.22, 0.32);
        let tilt = Quat::from_rotation_x(0.35);
        let at = Vec3::new(x, 1.37 + 0.12, -HD + 0.24);
        p.wood.merge(&moved(
            crate_mesh(size, rng),
            Mat4::from_rotation_translation(tilt, at),
        ));
        let kind = [2u32, 5, 2][k];
        let s = if kind == 2 { 0.07 } else { 0.035 };
        for i in 0..(if kind == 2 { 4 } else { 9 }) {
            let fx = -0.27 + 0.54 * (i as f32 + 0.5) / if kind == 2 { 4.0 } else { 9.0 };
            let q = tilt * Vec3::new(fx, 0.11, rng.range(-0.06, 0.06));
            p.produce.merge(&moved(
                fruit(kind, s * rng.range(0.9, 1.1), rng.next_u32()),
                Mat4::from_translation(at + q),
            ));
        }
    }
    // Pumpkins on the ground in front, sacks of grain at the side.
    for (x, z, s) in [
        (-1.25f32, HD + 0.55, 0.16f32),
        (-0.85, HD + 0.6, 0.12),
        (1.3, HD + 0.5, 0.15),
    ] {
        let m = Mat4::from_rotation_translation(
            Quat::from_rotation_y(rng.range(0.0, TAU)),
            Vec3::new(x, 0.0, z),
        );
        p.produce.merge(&moved(fruit(3, s, rng.next_u32()), m));
        // A woody stalk, bent over, out of the dimple on top.
        let top = Vec3::Y * (s * 1.12);
        let bend = top + Vec3::new(0.02, s * 0.2, 0.0);
        let stalk = tint(
            tube(top, bend, s * 0.07, 7),
            Vec4::new(0.75, 0.85, 0.45, 1.0),
        );
        p.wood.merge(&moved(stalk, m));
        p.wood.merge(&moved(
            tint(
                tube(bend, bend + Vec3::new(0.035, 0.008, 0.0), s * 0.055, 6),
                Vec4::new(0.6, 0.6, 0.4, 1.0),
            ),
            m,
        ));
    }
    for (k, (x, z, s)) in [
        (2.2f32, 0.3f32, 0.3f32),
        (2.25, -0.35, 0.27),
        (-2.15, 0.4, 0.29),
    ]
    .into_iter()
    .enumerate()
    {
        p.burlap.merge(&moved(
            sack_mesh(Vec3::new(s, s * 1.2, s), 600 + k as u32),
            Mat4::from_translation(Vec3::new(x, s * 0.7, z)),
        ));
    }
    crate_at(
        p,
        rng,
        Vec3::new(-2.3, 0.0, -0.5),
        Vec3::new(0.6, 0.45, 0.5),
        0.2,
    );
}

/// Linen and wool dyes.
const DYES: [u32; 8] = [
    0x7a1f2b, 0x264d7a, 0xc9a23a, 0x3f6b3a, 0x6b3a6e, 0xd8cdb0, 0xb5532a, 0x2a2a30,
];

/// A length of cloth hanging from a rod at `top` (a line along x of width `w`), falling
/// `len` with folds.
fn hanging_cloth(p: &mut Parts, top: Vec3, w: f32, len: f32, color: Vec4, seed: u32) {
    let (nu, nv) = (14usize, 18usize);
    let base = p.cloth.vertices.len() as u32;
    for j in 0..=nv {
        let v = j as f32 / nv as f32;
        for i in 0..=nu {
            let u = i as f32 / nu as f32;
            let fold = (u * TAU * 3.0 + noise::n2(u * 3.0, v * 2.0, seed) * 1.5).sin();
            let pos = top
                + Vec3::new(
                    (u - 0.5) * w * (1.0 - 0.1 * v),
                    -v * len,
                    0.025 * fold * (0.3 + v) + 0.03 * v * v,
                );
            p.cloth
                .vertices
                .push(vtx(pos, Vec3::Z, Vec2::new(u * w, v * len), color));
        }
    }
    let row = (nu + 1) as u32;
    let start = p.cloth.indices.len();
    for j in 0..nv as u32 {
        for i in 0..nu as u32 {
            let a = base + j * row + i;
            p.cloth
                .indices
                .extend_from_slice(&[a, a + row, a + 1, a + 1, a + row, a + row + 1]);
        }
    }
    smooth_piece(&mut p.cloth, base, start);
}

/// Bolts on the counter, folded stacks on the shelves, lengths hanging from the back beam.
fn cloth(p: &mut Parts, rng: &mut Rng) {
    let top = TOP_Y + 0.035;
    for k in 0..6 {
        let color = rgba(hex(DYES[k % DYES.len()]));
        let r = rng.range(0.06, 0.085);
        let x = -1.2 + k as f32 * 0.42;
        let a = Vec3::new(x, top + r, COUNTER_Z - 0.32);
        let b = a + Vec3::new(rng.range(-0.04, 0.04), 0.0, 0.5);
        p.cloth.merge(&tint(tube(a, b, r, 20), color));
        // The cardboard core showing at the near end.
        p.wood
            .merge(&tube(b - Vec3::Z * 0.01, b + Vec3::Z * 0.01, r * 0.3, 10));
    }
    for (y, n) in [(0.72f32, 5), (1.37, 4)] {
        for k in 0..n {
            let x = -1.2 + k as f32 * 2.4 / (n - 1) as f32;
            let mut yy = y;
            for _ in 0..rng.range_i32(2, 5) {
                let h = rng.range(0.035, 0.05);
                let color = rgba(hex(DYES[rng.range_i32(0, DYES.len() as i32) as usize]));
                p.cloth.merge(&tint(
                    moved(
                        bevel_box(Vec3::new(0.42, h, 0.3), h * 0.45, 0),
                        Mat4::from_rotation_translation(
                            Quat::from_rotation_y(rng.range(-0.08, 0.08)),
                            Vec3::new(x, yy + h * 0.5, -HD + 0.2),
                        ),
                    ),
                    color,
                ));
                yy += h;
            }
        }
    }
    // A rod under the back beam with four hanging lengths.
    let rod_y = BACK_H - 0.12;
    p.wood.merge(&tube(
        Vec3::new(-HW + 0.1, rod_y, -HD + 0.05),
        Vec3::new(HW - 0.1, rod_y, -HD + 0.05),
        0.018,
        10,
    ));
    for k in 0..4 {
        let x = -1.05 + k as f32 * 0.7;
        let color = rgba(hex(DYES[(k * 3 + 1) % DYES.len()]));
        hanging_cloth(
            p,
            Vec3::new(x, rod_y, -HD + 0.07),
            0.6,
            rng.range(1.0, 1.35),
            color,
            700 + k as u32,
        );
    }
    crate_at(
        p,
        rng,
        Vec3::new(2.25, 0.0, 0.4),
        Vec3::new(0.62, 0.5, 0.55),
        -0.3,
    );
    barrel_at(p, Vec3::new(-2.2, 0.0, 0.5), 0.85, 0.28);
}

/// A sword lying along +Y from the origin (pommel at the origin): blade, guard, grip.
fn sword(p: &mut Parts, m: Mat4, len: f32) {
    let blade_w = 0.024;
    let blade = lathe(
        &[
            (0.0, 0.0),
            (blade_w, 0.0),
            (blade_w * 0.9, len * 0.82),
            (0.0, len),
        ],
        4,
        blade_w,
    );
    let blade = moved(
        blade,
        Mat4::from_translation(Vec3::Y * 0.22)
            * Mat4::from_scale(Vec3::new(1.0, 1.0, 0.18))
            * Mat4::from_rotation_y(FRAC_PI_4),
    );
    let mut b = blade;
    b.compute_flat_normals();
    p.steel.merge(&moved(b, m));
    p.iron.merge(&moved(
        bevel_box(Vec3::new(0.16, 0.022, 0.03), 0.008, 0),
        m * Mat4::from_translation(Vec3::Y * 0.21),
    ));
    p.wood.merge(&tint(
        moved(
            lathe(
                &[(0.0, 0.03), (0.016, 0.03), (0.018, 0.2), (0.0, 0.2)],
                10,
                0.016,
            ),
            m,
        ),
        Vec4::new(0.35, 0.22, 0.16, 1.0),
    ));
    p.iron.merge(&moved(
        lathe(
            &[(0.0, 0.0), (0.025, 0.012), (0.022, 0.035), (0.0, 0.04)],
            10,
            0.02,
        ),
        m,
    ));
}

/// A round shield facing +Z: painted boards, an iron rim and boss.
fn shield(p: &mut Parts, m: Mat4, r: f32, color: Vec4) {
    let face = Mat4::from_rotation_x(FRAC_PI_2);
    let mut boards = lathe(
        &[
            (0.0, 0.02),
            (r * 0.95, 0.012),
            (r * 0.95, -0.005),
            (0.0, -0.005),
        ],
        36,
        r,
    );
    // Paint: a quartered design in the shield's color and cream.
    for v in &mut boards.vertices {
        let a = v.position[2].atan2(v.position[0]);
        let quarter = ((a / FRAC_PI_2).floor() as i32).rem_euclid(2) == 0;
        let c = if quarter { color } else { rgba(hex(0xd8cdb0)) };
        v.color = c.to_array();
    }
    p.wood.merge(&moved(boards, m * face));
    p.iron.merge(&moved(
        lathe(
            &[(r * 0.93, 0.0), (r, 0.01), (r, -0.008), (r * 0.93, -0.008)],
            36,
            r,
        ),
        m * face,
    ));
    p.iron.merge(&moved(
        lathe(
            &[(0.0, 0.07), (0.05, 0.06), (0.075, 0.02), (0.08, 0.015)],
            18,
            0.07,
        ),
        m * face,
    ));
}

/// A smith's anvil on a stump, base center at the origin.
fn anvil(p: &mut Parts, m: Mat4) {
    p.wood.merge(&tint(
        moved(
            lathe(
                &[(0.0, 0.0), (0.22, 0.0), (0.2, 0.45), (0.0, 0.45)],
                20,
                0.2,
            ),
            m,
        ),
        Vec4::new(0.8, 0.7, 0.6, 1.0),
    ));
    let iron = [
        (Vec3::new(0.0, 0.5, 0.0), Vec3::new(0.18, 0.1, 0.16)),
        (Vec3::new(0.0, 0.6, 0.0), Vec3::new(0.1, 0.12, 0.09)),
        (Vec3::new(0.0, 0.71, 0.0), Vec3::new(0.34, 0.1, 0.12)),
    ];
    for (c, s) in iron {
        p.iron
            .merge(&moved(bevel_box(s, 0.01, 0), m * Mat4::from_translation(c)));
    }
    // The horn.
    let horn = lathe(&[(0.05, 0.0), (0.035, 0.08), (0.0, 0.17)], 12, 0.04);
    p.iron.merge(&moved(
        horn,
        m * Mat4::from_rotation_translation(
            Quat::from_rotation_z(-FRAC_PI_2),
            Vec3::new(0.17, 0.72, 0.0),
        ),
    ));
}

/// Swords upright in a rack on the counter, shields on the back wall, an anvil beside.
fn smith(p: &mut Parts, rng: &mut Rng) {
    let top = TOP_Y + 0.035;
    // The rack: two rails with slots.
    for z in [COUNTER_Z - 0.25, COUNTER_Z - 0.05] {
        p.wood.merge(&moved(
            bevel_box(Vec3::new(1.5, 0.05, 0.05), 0.01, 0),
            Mat4::from_translation(Vec3::new(-0.45, top + 0.35, z)),
        ));
    }
    for x in [-1.15f32, 0.25] {
        for z in [COUNTER_Z - 0.25, COUNTER_Z - 0.05] {
            p.wood.merge(&beam(
                Vec3::new(x, top, z),
                Vec3::new(x, top + 0.38, z),
                0.04,
                0.04,
                0.008,
            ));
        }
    }
    for k in 0..6 {
        let x = -1.05 + k as f32 * 0.24;
        let lean = Quat::from_euler(EulerRot::ZXY, rng.range(-0.06, 0.06), -0.12, 0.0);
        // Point down in the rack, hilt up.
        let m = Mat4::from_rotation_translation(
            lean * Quat::from_rotation_z(PI),
            Vec3::new(x, top + 1.05 + rng.range(-0.04, 0.04), COUNTER_Z - 0.15),
        );
        sword(p, m, rng.range(0.75, 0.88));
    }
    // Daggers laid on the counter's right half.
    for k in 0..3 {
        let m = Mat4::from_rotation_translation(
            Quat::from_euler(EulerRot::YXZ, rng.range(-0.3, 0.3), -FRAC_PI_2, 0.0),
            Vec3::new(0.45 + k as f32 * 0.17, top + 0.012, COUNTER_Z - 0.3),
        );
        sword(p, m * Mat4::from_scale(Vec3::splat(0.5)), 0.6);
    }
    // Helmets: a broad-brimmed kettle hat on the counter, a nasal helm on a stand.
    let kettle = lathe(
        &[
            (0.0, 0.17),
            (0.05, 0.166),
            (0.09, 0.142),
            (0.11, 0.095),
            (0.115, 0.045),
            (0.175, 0.016),
            (0.185, 0.006),
            (0.178, 0.0),
            (0.11, 0.03),
            (0.104, 0.09),
            (0.085, 0.135),
            (0.0, 0.16),
        ],
        32,
        0.11,
    );
    p.steel.merge(&moved(
        kettle,
        Mat4::from_translation(Vec3::new(1.12, top, COUNTER_Z - 0.02)),
    ));
    let stand = Vec3::new(1.12, top, COUNTER_Z - 0.32);
    p.wood
        .merge(&beam(stand, stand + Vec3::Y * 0.2, 0.035, 0.035, 0.008));
    p.wood.merge(&moved(
        bevel_box(Vec3::new(0.12, 0.02, 0.12), 0.006, 0),
        Mat4::from_translation(stand + Vec3::Y * 0.01),
    ));
    let rim = stand + Vec3::Y * 0.12;
    p.steel.merge(&moved(
        lathe(
            &[
                (0.0, 0.25),
                (0.025, 0.238),
                (0.07, 0.175),
                (0.096, 0.1),
                (0.1, 0.035),
                (0.103, 0.0),
                (0.094, 0.0),
                (0.09, 0.04),
                (0.086, 0.1),
                (0.062, 0.17),
                (0.0, 0.232),
            ],
            32,
            0.1,
        ),
        Mat4::from_translation(rim),
    ));
    // The nasal bar and a brow band riveted round the rim.
    p.steel.merge(&moved(
        bevel_box(Vec3::new(0.026, 0.1, 0.008), 0.003, 1),
        Mat4::from_translation(rim + Vec3::new(0.0, -0.02, 0.104)),
    ));
    p.iron.merge(&moved(
        lathe(
            &[(0.104, 0.0), (0.108, 0.006), (0.108, 0.03), (0.102, 0.036)],
            32,
            0.1,
        ),
        Mat4::from_translation(rim),
    ));
    // Shields on the back wall boards.
    p.wood.merge(&tint(
        moved(
            bevel_box(Vec3::new(3.1, 1.0, 0.05), 0.01, 0),
            Mat4::from_translation(Vec3::new(0.0, 1.9, -HD - 0.03)),
        ),
        Vec4::new(0.85, 0.8, 0.75, 1.0),
    ));
    for (k, x) in [-1.0f32, 0.0, 1.0].into_iter().enumerate() {
        let color = rgba(hex([0x7a1f2b, 0x264d7a, 0x3f6b3a][k]));
        shield(
            p,
            Mat4::from_translation(Vec3::new(x, 1.95 + rng.range(-0.05, 0.05), -HD + 0.01)),
            0.3,
            color,
        );
    }
    anvil(
        p,
        Mat4::from_rotation_translation(Quat::from_rotation_y(0.4), Vec3::new(2.35, 0.0, 0.55)),
    );
    barrel_at(p, Vec3::new(-2.2, 0.0, 0.45), 0.8, 0.28);
    crate_at(
        p,
        rng,
        Vec3::new(-2.3, 0.0, -0.45),
        Vec3::new(0.6, 0.5, 0.5),
        0.15,
    );
}

/// Pots of every shape on the counter and shelves, stacked plates, big jars on the ground.
fn potter(p: &mut Parts, rng: &mut Rng) {
    let top = TOP_Y + 0.035;
    let place = |p: &mut Parts, rng: &mut Rng, at: Vec3, kind: u32, hgt: f32, r: f32| {
        let glaze = rgba(hex(GLAZES[rng.range_i32(0, GLAZES.len() as i32) as usize]));
        p.pottery.merge(&tint(
            moved(
                pot(kind, hgt, r),
                Mat4::from_rotation_translation(Quat::from_rotation_y(rng.range(0.0, TAU)), at),
            ),
            glaze * rng.range(0.85, 1.1),
        ));
    };
    let mut x = -1.25;
    while x < 1.25 {
        let kind = rng.range_i32(0, 5) as u32;
        let (hgt, r) = match kind {
            2 => (rng.range(0.32, 0.4), 0.09),
            3 => (rng.range(0.07, 0.1), rng.range(0.1, 0.13)),
            _ => (rng.range(0.16, 0.28), rng.range(0.06, 0.1)),
        };
        let z = COUNTER_Z - rng.range(0.0, 0.25);
        place(p, rng, Vec3::new(x, top, z), kind, hgt, r);
        x += r * 2.0 + rng.range(0.04, 0.1);
    }
    for y in [0.72f32, 1.37] {
        let mut x = -1.35;
        while x < 1.35 {
            let kind = [0u32, 1, 4][rng.range_i32(0, 3) as usize];
            let (hgt, r) = (rng.range(0.15, 0.26), rng.range(0.06, 0.09));
            place(p, rng, Vec3::new(x, y + 0.02, -HD + 0.2), kind, hgt, r);
            x += r * 2.0 + rng.range(0.03, 0.08);
        }
    }
    // Stacks of plates.
    for (sx, n) in [(0.5f32, 7), (0.85, 5)] {
        for k in 0..n {
            let plate = lathe(
                &[
                    (0.0, 0.0),
                    (0.09, 0.0),
                    (0.13, 0.012),
                    (0.14, 0.018),
                    (0.0, 0.006),
                ],
                28,
                0.13,
            );
            p.pottery.merge(&tint(
                moved(
                    plate,
                    Mat4::from_translation(Vec3::new(
                        sx,
                        0.72 + 0.02 + k as f32 * 0.014,
                        -HD + 0.55,
                    )),
                ),
                rgba(hex(0xd8cdb0)),
            ));
        }
    }
    // Big storage jars on the ground at either side.
    for (pos, hgt) in [
        (Vec3::new(-2.2, 0.0, 0.5), 0.7f32),
        (Vec3::new(-2.35, 0.0, -0.2), 0.55),
        (Vec3::new(2.25, 0.0, 0.45), 0.65),
    ] {
        place(p, rng, pos, 0, hgt, hgt * 0.38);
    }
    for (k, (x, z, s)) in [(2.3f32, -0.4f32, 0.27f32)].into_iter().enumerate() {
        p.burlap.merge(&moved(
            sack_mesh(Vec3::new(s, s * 1.2, s), 800 + k as u32),
            Mat4::from_translation(Vec3::new(x, s * 0.7, z)),
        ));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use engine_assets::Assets;
    use engine_render::MeshRef;

    /// Every trade's stall builds meshes with finite positions, unit normals and finite UVs,
    /// all within the stall's footprint (crates and barrels stand beside it).
    #[test]
    fn every_trade_builds_clean_meshes() {
        let mut world = World::new();
        world.init_resource::<Assets<Mesh>>();
        world.init_resource::<Assets<StandardMaterial>>();
        let m = world
            .resource_mut::<Assets<StandardMaterial>>()
            .add(StandardMaterial::default());
        let mats = StallMats {
            wood: m,
            canvas: m,
            iron: m,
            crystal: m,
            pottery: m,
            burlap: m,
            lamp_glass: m,
            produce: m,
            cloth: m,
            wicker: m,
            steel: m,
            brass: m,
        };
        for goods in [
            Goods::Trader,
            Goods::Produce,
            Goods::Cloth,
            Goods::Smith,
            Goods::Potter,
        ] {
            let entities = spawn_stall(&mut world, &mats, Transform::default(), goods, 7);
            let mut tris = 0;
            for e in entities {
                let Some(MeshRef::Handle(h)) = world.get::<MeshRenderer>(e).map(|r| r.mesh.clone())
                else {
                    continue;
                };
                let meshes = world.resource::<Assets<Mesh>>();
                let mesh = meshes.get(h).expect("mesh");
                tris += mesh.triangle_count();
                for v in &mesh.vertices {
                    let p = Vec3::from(v.position);
                    let n = Vec3::from(v.normal);
                    assert!(
                        p.is_finite() && n.is_finite(),
                        "{goods:?}: bad vertex {v:?}"
                    );
                    assert!((n.length() - 1.0).abs() < 0.02, "{goods:?}: normal {n}");
                    assert!(v.uv.iter().all(|c| c.is_finite()), "{goods:?}: uv");
                    assert!(
                        p.x.abs() < 3.2 && (-0.4..3.4).contains(&p.y) && p.z.abs() < 2.2,
                        "{goods:?}: vertex outside the stall at {p}"
                    );
                }
                for i in &mesh.indices {
                    assert!((*i as usize) < mesh.vertices.len());
                }
            }
            assert!(tris > 20_000, "{goods:?}: only {tris} triangles");
        }
    }
}
