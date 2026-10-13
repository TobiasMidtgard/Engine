//! Stand-in content for each zone until its module's `demo::spawn_demo` replaces it (see
//! `zones::spawn_demos`). Placeholders show where a test case lives and what it will contain;
//! they don't simulate anything.

use crate::landscape::Landscape;
use crate::meshgen::TreeKind;
use crate::scenery::{self, Library};
use crate::zones::Zone;
use engine::prelude::*;
use std::f64::consts::TAU as TAU64;

fn spawn(commands: &mut Commands, shape: Shape, material: StandardMaterial, transform: Transform) {
    commands.spawn((transform, MeshRenderer::new(shape, material)));
}

fn cuboid(x: f32, y: f32, z: f32) -> Shape {
    Shape::Cuboid { size: [x, y, z] }
}

fn cyl(radius: f32, height: f32) -> Shape {
    Shape::Cylinder {
        radius,
        height,
        segments: 24,
    }
}

fn at(p: DVec3) -> Transform {
    Transform::from_translation(p)
}

fn yawed(p: DVec3, yaw: f32) -> Transform {
    Transform::from_translation(p).with_rotation(Quat::from_rotation_y(yaw))
}

fn mat(hex: u32, roughness: f32) -> StandardMaterial {
    StandardMaterial::hex(hex).with_roughness(roughness)
}

/// Dressed stone (walls, pedestals, planters): warm cast stone, mapped in the object's meters.
fn stone() -> StandardMaterial {
    StandardMaterial::preset(MaterialPreset::Concrete)
        .with_tint_hex(0xe6ddd0)
        .with_triplanar()
}
fn wood() -> StandardMaterial {
    mat(0x7a5434, 0.8)
}
fn dark_wood() -> StandardMaterial {
    mat(0x4a3220, 0.8)
}
fn metal() -> StandardMaterial {
    StandardMaterial::hex(0xc8ccd4)
        .with_metallic(1.0)
        .with_roughness(0.3)
}
/// Vertex tint for flat rock surfaces drawn with the rock kit's material (as dark as the
/// boulders, which the textures multiply).
const ROCK_TINT: Vec3 = Vec3::new(0.31, 0.3, 0.28);

/// Finish a mesh for the rock kit's material: planar UVs in rock texture tiles, projected
/// along each face's main axis (the same mapping the boulders use).
fn rock_face(b: crate::meshgen::Builder) -> Mesh {
    let mut mesh = b.finish();
    for v in &mut mesh.vertices {
        let p = Vec3::from(v.position);
        let n = Vec3::from(v.normal).abs();
        let q = if n.x >= n.y && n.x >= n.z {
            Vec2::new(p.z, -p.y)
        } else if n.y >= n.z {
            Vec2::new(p.x, p.z)
        } else {
            Vec2::new(p.x, -p.y)
        };
        v.uv = (q / crate::rock_textures::ROCK_TILE_M).to_array();
    }
    mesh
}

/// A simple person: legs, body, head (1.8 m), facing `yaw`.
fn person(commands: &mut Commands, base: DVec3, yaw: f32, shirt: u32) {
    let skin = mat(0xc89878, 0.6).with_subsurface(0.4);
    let cloth = mat(shirt, 0.85);
    let pants = mat(0x2a2e3a, 0.85);
    for side in [-0.12, 0.12] {
        let off = Quat::from_rotation_y(yaw) * Vec3::new(side, 0.0, 0.0);
        spawn(
            commands,
            Shape::Capsule {
                radius: 0.09,
                height: 0.85,
                segments: 10,
            },
            pants,
            at(base + off.as_dvec3() + DVec3::new(0.0, 0.45, 0.0)),
        );
    }
    spawn(
        commands,
        Shape::Capsule {
            radius: 0.2,
            height: 0.7,
            segments: 14,
        },
        cloth,
        yawed(base + DVec3::new(0.0, 1.22, 0.0), yaw).with_scale(Vec3::new(1.0, 1.0, 0.65)),
    );
    spawn(
        commands,
        Shape::Sphere {
            radius: 0.12,
            segments: 16,
        },
        skin,
        at(base + DVec3::new(0.0, 1.7, 0.0)),
    );
}

/// Material plaza: a paved square with a roughness ladder (dielectric and metal), a glass orb
/// inside an emissive ring and pedestals. The Rendering and lighting thread owns its content.
pub fn plaza(commands: &mut Commands, lib: &Library, z: &Zone) {
    let c = z.center;
    // Paving: granite setts in concentric bands (a darker band of smaller setts between two
    // light ones) round a polished marble middle, laid in world meters so the joints line up.
    let cobbles = |tile: f32, tint: u32| {
        StandardMaterial::preset(MaterialPreset::Cobblestone)
            .with_texture_scale(tile)
            .with_tint_hex(tint)
    };
    let paving = [
        (31.0f32, cobbles(1.5, 0xf2eee8)),
        (24.0, cobbles(1.0, 0xc4beb6)),
        (16.0, cobbles(1.5, 0xfaf6f0)),
        (
            8.0,
            StandardMaterial::preset(MaterialPreset::Marble).with_texture_scale(1.6),
        ),
    ];
    for (i, (r, material)) in paving.into_iter().enumerate() {
        spawn(
            commands,
            Shape::Cylinder {
                radius: r,
                height: 0.2,
                segments: 128,
            },
            material.with_triplanar_world(),
            at(c + DVec3::new(0.0, 0.1 + i as f64 * 0.005, 0.0)),
        );
    }
    // Low wall with benches around the edge.
    for i in 0..32 {
        let a = i as f64 / 32.0 * TAU64;
        if i % 8 == 0 {
            continue; // gaps for the roads
        }
        spawn(
            commands,
            cuboid(6.2, 0.55, 0.6),
            stone(),
            yawed(
                c + DVec3::new(a.cos() * 31.0, 0.35, a.sin() * 31.0),
                -(a as f32) + std::f32::consts::FRAC_PI_2,
            ),
        );
    }
    for i in 0..7 {
        let r = i as f32 / 6.0;
        let x = (i as f64 - 3.0) * 2.6;
        let ball = Shape::Sphere {
            radius: 1.0,
            segments: 48,
        };
        spawn(
            commands,
            ball,
            mat(0xc0392b, r.max(0.05)),
            at(c + DVec3::new(x, 1.2, 4.0)),
        );
        spawn(
            commands,
            ball,
            StandardMaterial::hex(0xe6b450)
                .with_metallic(1.0)
                .with_roughness(r.max(0.05)),
            at(c + DVec3::new(x, 1.2, 0.0)),
        );
        for zz in [0.0, 4.0] {
            spawn(
                commands,
                cyl(0.5, 0.2),
                stone(),
                at(c + DVec3::new(x, 0.3, zz)),
            );
        }
    }
    spawn(
        commands,
        Shape::Torus {
            radius: 2.5,
            tube: 0.25,
            segments: 64,
        },
        StandardMaterial::hex(0x202830).with_emissive([0.3, 0.7, 1.0], 600.0),
        Transform::from_translation(c + DVec3::new(0.0, 3.5, -6.0))
            .with_rotation(Quat::from_rotation_x(std::f32::consts::FRAC_PI_2)),
    );
    let mut glass = StandardMaterial::hex(0xd8f0ff)
        .with_roughness(0.05)
        .with_alpha(0.25);
    glass.reflectance = 0.6;
    spawn(
        commands,
        Shape::Sphere {
            radius: 1.5,
            segments: 64,
        },
        glass,
        at(c + DVec3::new(0.0, 3.5, -6.0)),
    );
    for (i, x) in [-9.0, 9.0].into_iter().enumerate() {
        spawn(
            commands,
            cuboid(2.0, 1.0, 2.0),
            stone(),
            at(c + DVec3::new(x, 0.7, -6.0)),
        );
        let shape = if i == 0 {
            Shape::Capsule {
                radius: 0.6,
                height: 2.4,
                segments: 32,
            }
        } else {
            Shape::Cone {
                radius: 0.9,
                height: 2.2,
                segments: 48,
            }
        };
        spawn(commands, shape, metal(), at(c + DVec3::new(x, 2.4, -6.0)));
    }
    // Trees in big planters at the four diagonals.
    for k in 0..4 {
        let a = (k as f64 + 0.5) / 4.0 * TAU64;
        let p = c + DVec3::new(a.cos() * 22.0, 0.0, a.sin() * 22.0);
        spawn(
            commands,
            cyl(2.0, 0.8),
            stone(),
            at(p + DVec3::new(0.0, 0.4, 0.0)),
        );
        let t = &lib.trees_of(TreeKind::Oak).nth(k).expect("oak");
        scenery::spawn_tree(
            commands,
            lib,
            t,
            p + DVec3::new(0.0, 0.8, 0.0),
            a as f32,
            0.8,
        );
    }
}

/// Market street: stalls with awnings (shop UI), two villagers talking (dialogue UI), a map table
/// with a globe (flat, globe and galaxy maps). The UI thread owns it.
pub fn market(commands: &mut Commands, _lib: &Library, z: &Zone) {
    let c = z.center;
    let awnings = [0xb03a2e, 0x2e6fb0, 0xd9a93a, 0x3a8f4a, 0x8e44ad];
    for (i, hex) in awnings.into_iter().enumerate() {
        let a = i as f64 / awnings.len() as f64 * TAU64 * 0.75 + 0.6;
        let p = c + DVec3::new(a.cos() * 15.0, 0.0, a.sin() * 15.0);
        let yaw = (-(a.cos()) as f32).atan2(-(a.sin()) as f32);
        let rot = Quat::from_rotation_y(yaw);
        let off = |v: Vec3| p + (rot * v).as_dvec3();
        for (x, zz) in [(-1.6, -1.0), (1.6, -1.0), (-1.6, 1.0), (1.6, 1.0)] {
            spawn(
                commands,
                cuboid(0.12, 2.6, 0.12),
                dark_wood(),
                yawed(off(Vec3::new(x, 1.3, zz)), yaw),
            );
        }
        spawn(
            commands,
            cuboid(3.4, 0.9, 1.2),
            wood(),
            yawed(off(Vec3::new(0.0, 0.45, 0.6)), yaw),
        );
        spawn(
            commands,
            cuboid(3.8, 0.06, 2.6),
            mat(hex, 0.9).with_subsurface(0.3),
            Transform::from_translation(off(Vec3::new(0.0, 2.75, 0.2)))
                .with_rotation(rot * Quat::from_rotation_x(0.18)),
        );
        // Goods: crates and round produce.
        for k in 0..4 {
            let produce = [0xc0392b, 0xe67e22, 0x27ae60, 0xf1c40f][(k + i) % 4];
            spawn(
                commands,
                Shape::Sphere {
                    radius: 0.18,
                    segments: 12,
                },
                mat(produce, 0.5).with_subsurface(0.3),
                at(off(Vec3::new(-1.2 + k as f32 * 0.8, 1.05, 0.6))),
            );
        }
        person(
            commands,
            off(Vec3::new(0.0, 0.0, -0.6)),
            yaw + std::f32::consts::PI,
            0x6b4f2a,
        );
    }
    // Two villagers in conversation.
    person(
        commands,
        c + DVec3::new(-1.0, 0.0, -2.0),
        std::f32::consts::FRAC_PI_2,
        0x2e86c1,
    );
    person(
        commands,
        c + DVec3::new(1.0, 0.0, -2.0),
        -std::f32::consts::FRAC_PI_2,
        0xa83232,
    );
    // Map table with a globe.
    spawn(
        commands,
        cuboid(3.0, 0.1, 2.0),
        dark_wood(),
        at(c + DVec3::new(0.0, 1.0, 5.0)),
    );
    spawn(
        commands,
        cuboid(2.8, 0.02, 1.8),
        mat(0xc9b88a, 0.9),
        at(c + DVec3::new(0.0, 1.06, 5.0)),
    );
    for (x, zz) in [(-1.3, -0.8), (1.3, -0.8), (-1.3, 0.8), (1.3, 0.8)] {
        spawn(
            commands,
            cuboid(0.1, 1.0, 0.1),
            dark_wood(),
            at(c + DVec3::new(x, 0.5, 5.0 + zz)),
        );
    }
    spawn(
        commands,
        Shape::Sphere {
            radius: 0.45,
            segments: 32,
        },
        mat(0x2a6fa8, 0.4),
        at(c + DVec3::new(0.9, 1.6, 5.0)),
    );
    spawn(
        commands,
        cyl(0.05, 0.4),
        metal(),
        at(c + DVec3::new(0.9, 1.25, 5.0)),
    );
}

/// 2D and 2.5D gallery: a side-on diorama (platforms, coins, a hero) that the 2D cameras will
/// frame. The Rendering and lighting thread owns it.
pub fn gallery2d(commands: &mut Commands, _lib: &Library, z: &Zone) {
    let c = z.center;
    spawn(
        commands,
        cuboid(30.0, 12.0, 0.4),
        mat(0x6fb3e0, 0.9),
        at(c + DVec3::new(0.0, 6.0, -6.0)),
    );
    for (x, y, w) in [
        (-10.0, 1.0, 6.0),
        (-2.0, 3.0, 4.0),
        (5.0, 5.0, 5.0),
        (11.0, 2.0, 4.0),
    ] {
        spawn(
            commands,
            cuboid(w, 1.0, 3.0),
            mat(0x7a4a2a, 0.9),
            at(c + DVec3::new(x, y, -3.0)),
        );
        spawn(
            commands,
            cuboid(w, 0.3, 3.0),
            mat(0x4caf50, 0.8),
            at(c + DVec3::new(x, y + 0.65, -3.0)),
        );
    }
    for x in [-11.0, -9.0, -2.5, -1.5, 4.0, 5.0, 6.0] {
        let y = if x < -5.0 {
            2.6
        } else if x < 0.0 {
            4.6
        } else {
            6.6
        };
        spawn(
            commands,
            Shape::Torus {
                radius: 0.3,
                tube: 0.08,
                segments: 24,
            },
            StandardMaterial::hex(0xf1c40f)
                .with_metallic(1.0)
                .with_roughness(0.25),
            at(c + DVec3::new(x, y, -3.0)),
        );
    }
    spawn(
        commands,
        cuboid(0.8, 1.2, 0.8),
        mat(0xe74c3c, 0.6),
        at(c + DVec3::new(-10.0, 2.25, -3.0)),
    );
}

/// Lake: still water (the water module replaces it), a jetty and a rowboat. The Water simulation
/// thread owns it.
pub fn lake(
    commands: &mut Commands,
    meshes: &mut Assets<Mesh>,
    _lib: &Library,
    land: &Landscape,
    z: &Zone,
) {
    stream_water(commands, meshes, land);
    let mut water = StandardMaterial::hex(0x0f2a33)
        .with_roughness(0.03)
        .with_alpha(0.88);
    water.reflectance = 0.5;
    water.receives_wetness = false;
    spawn(
        commands,
        Shape::Cylinder {
            radius: z.radius as f32 * 1.3,
            height: 0.02,
            segments: 128,
        },
        water,
        at(z.center + DVec3::new(0.0, crate::landscape::LAKE_LEVEL - 0.01, 0.0)),
    );
    // Jetty from the south-west shore.
    let start = z.center + DVec3::new(-62.0, 0.0, 22.0);
    let dir = DVec3::new(1.0, 0.0, -0.35).normalize();
    let yaw = (-(dir.z) as f32).atan2(dir.x as f32);
    for i in 0..14 {
        let p = start + dir * (i as f64 * 1.6);
        spawn(
            commands,
            cuboid(1.5, 0.12, 2.4),
            wood(),
            yawed(p + DVec3::new(0.0, 0.9, 0.0), yaw),
        );
        if i % 3 == 0 {
            for s in [-1.1, 1.1] {
                let side = DVec3::new(-dir.z, 0.0, dir.x) * s;
                spawn(
                    commands,
                    cyl(0.1, 3.0),
                    dark_wood(),
                    at(p + side + DVec3::new(0.0, 0.2, 0.0)),
                );
            }
        }
    }
    let boat = start + dir * 24.0 + DVec3::new(-3.0, 0.15, 3.0);
    spawn(
        commands,
        Shape::Capsule {
            radius: 0.6,
            height: 3.8,
            segments: 16,
        },
        mat(0x8a4b2a, 0.7),
        Transform::from_translation(boat)
            .with_rotation(
                Quat::from_rotation_y(0.7) * Quat::from_rotation_x(std::f32::consts::FRAC_PI_2),
            )
            .with_scale(Vec3::new(1.0, 1.0, 0.45)),
    );
}

/// Half width, half depth and height of the cave's inside (m), around the cave zone's center;
/// the mouth faces −Z.
pub const CAVE_INSIDE: (f64, f64, f64) = (15.0, 17.0, 9.0);

/// The cave in its hill: rock walls, a roof that carries the hillside on over it, boulders round
/// the mouth, and no sky light inside. Scenery, so it stays when the zone's demo moves in.
pub fn cave_shell(
    commands: &mut Commands,
    meshes: &mut Assets<Mesh>,
    lib: &Library,
    land: &Landscape,
    rng: &mut Rng,
) {
    let c = crate::zones::zone("cave").center;
    let (hx, hz, h) = CAVE_INSIDE;
    let kit = &lib.rocks;
    // Back and side walls, thick enough to reach the edge of the hole the landscape digs, and
    // the front wall either side of the 10 m mouth: the kit's rock, so the face reads as stone.
    let (fx, fz, fh) = (hx as f32, hz as f32, h as f32);
    let mut walls = crate::meshgen::Builder::default();
    walls.cuboid(
        Vec3::new(0.0, 5.0, fz + 3.5),
        Vec3::new(22.0, 5.0, 3.5),
        Quat::IDENTITY,
        ROCK_TINT,
    );
    for s in [-1.0, 1.0] {
        walls.cuboid(
            Vec3::new(s * (fx + 3.5), 5.0, 2.5),
            Vec3::new(3.5, 5.0, 21.5),
            Quat::IDENTITY,
            ROCK_TINT,
        );
        walls.cuboid(
            Vec3::new(s * 10.0, fh * 0.5, -fz - 0.5),
            Vec3::new(5.0, fh * 0.5, 1.5),
            Quat::IDENTITY,
            ROCK_TINT,
        );
    }
    commands.spawn((
        at(c),
        MeshRenderer::new(meshes.add(rock_face(walls)), kit.mats.rock),
    ));
    cave_roof(commands, meshes, lib, land, c, h, rng);
    // Boulders sunk into the walls and hanging from the ceiling, so the inside reads as rock
    // rather than a box. Each sits with its flat base in the wall and its rounded face out.
    // (Shapes and tilts come from their own stream; the shared one advances as before.)
    let mut pick = Rng::new(0x00ca_7eb0_u64);
    for k in 0..64 {
        let along = rng.range(-1.0, 1.0) as f64;
        let y = rng.range(-0.5, h as f32 - 1.0) as f64;
        let (p, inward) = match k % 4 {
            0 => (DVec3::new(-hx - 0.4, y, along * hz), Vec3::X),
            1 => (DVec3::new(hx + 0.4, y, along * hz), Vec3::NEG_X),
            2 => (DVec3::new(along * hx, y, hz + 0.4), Vec3::NEG_Z),
            _ => (
                DVec3::new(along * hx, h + 0.4, rng.range(-hz as f32, hz as f32) as f64),
                Vec3::NEG_Y,
            ),
        };
        let _ = rng.range_i32(0, 6);
        let yaw = rng.range(0.0, TAU);
        let tilt = rng.range(-0.6, 0.6) * 0.5;
        let scale = rng.range(1.4, 3.0) * 0.6;
        let v = kit.pick(crate::rocks::RockSize::Large, pick.chance(0.5), &mut pick);
        let sink = (inward * v.extent.y * scale * 0.45).as_dvec3();
        let tf = Transform::from_translation(c + p - sink)
            .with_rotation(
                Quat::from_rotation_arc(Vec3::Y, inward)
                    * Quat::from_rotation_y(yaw)
                    * Quat::from_rotation_x(tilt),
            )
            .with_scale(Vec3::new(scale, scale * pick.range(0.7, 1.0), scale));
        kit.spawn(commands, v, tf, crate::rocks::RockLook::default());
    }
    // Rocky arch over the mouth: blocks set into the hillside, faces out and a little up.
    for i in 0..9 {
        let a = i as f64 / 8.0 * std::f64::consts::PI;
        let s = rng.range(1.4, 2.2);
        let v = kit.pick(crate::rocks::RockSize::Large, pick.chance(0.6), &mut pick);
        let out = Vec3::new(a.cos() as f32 * 0.5, a.sin() as f32 * 0.5, -1.0).normalize();
        let scale = s * 0.62;
        let p = c + DVec3::new(a.cos() * 6.0, 6.0 + a.sin() * 3.5, -hz - 1.5)
            - (out * v.extent.y * scale * 0.4).as_dvec3();
        let tf = Transform::from_translation(p)
            .with_rotation(
                Quat::from_rotation_arc(Vec3::Y, out) * Quat::from_rotation_y(pick.range(0.0, TAU)),
            )
            .with_scale(Vec3::new(scale, scale * 0.8, scale));
        let look = crate::rocks::RockLook {
            wet: false,
            moss: i % 3 != 1,
        };
        kit.spawn(commands, v, tf, look);
    }
    // Boulders piled around the mouth so it reads as a rock face in the hillside.
    for i in 0..26 {
        let side = if i % 2 == 0 { -1.0 } else { 1.0 };
        let x = side * rng.range(5.5, 22.0) as f64;
        let y = rng.range(0.0, 10.0) as f64;
        let s = rng.range(1.2, 3.2);
        let z = -hz - 2.0 - rng.range(0.0, 3.0) as f64;
        let (size, scale) = if s > 2.0 {
            (crate::rocks::RockSize::Large, s * 0.55)
        } else {
            (crate::rocks::RockSize::Medium, s * 1.05)
        };
        let v = kit.pick(size, pick.chance(0.4), &mut pick);
        let out = Vec3::new(side as f32 * 0.25, 0.7, -1.0).normalize();
        let p = c + DVec3::new(x, y, z) - (out * v.extent.y * scale * 0.35).as_dvec3();
        let tf = Transform::from_translation(p)
            .with_rotation(
                Quat::from_rotation_arc(Vec3::Y, out) * Quat::from_rotation_y(pick.range(0.0, TAU)),
            )
            .with_scale(Vec3::new(scale, scale * pick.range(0.7, 0.95), scale));
        let look = crate::rocks::RockLook {
            wet: false,
            moss: true,
        };
        kit.spawn(commands, v, tf, look);
    }
    // Blocks stacked up the face above the mouth to where the hill's grass starts, so it breaks
    // into ledges instead of standing as one flat wall. (Their own stream, like the shapes.)
    let face_z = -hz - 2.6;
    let mut x = -23.0;
    while x <= 23.0 {
        let top = land.cave_roof_height(c.x + x, c.z + face_z) - c.y;
        let mut y = h - 0.8;
        while y < top - 0.6 {
            let scale = pick.range(1.0, 1.5);
            let v = kit.pick(crate::rocks::RockSize::Large, pick.chance(0.6), &mut pick);
            let out = Vec3::new(pick.range(-0.2, 0.2), pick.range(0.2, 0.6), -1.0).normalize();
            let at_face = DVec3::new(
                x + pick.range(-0.9, 0.9) as f64,
                y + pick.range(-0.5, 0.5) as f64,
                face_z,
            );
            let p = c + at_face - (out * v.extent.y * scale * 0.45).as_dvec3();
            let tf = Transform::from_translation(p)
                .with_rotation(
                    Quat::from_rotation_arc(Vec3::Y, out)
                        * Quat::from_rotation_y(pick.range(0.0, TAU)),
                )
                .with_scale(Vec3::new(scale, scale * pick.range(0.6, 0.85), scale));
            let look = crate::rocks::RockLook {
                wet: false,
                moss: pick.chance(0.7),
            };
            kit.spawn(commands, v, tf, look);
            y += 2.4;
        }
        x += 2.9;
    }
    commands.spawn((
        at(c + DVec3::new(0.0, h * 0.5, 0.0)),
        // Just past the inner wall faces so the walls count as inside too.
        SkyLightVolume::new([hx as f32 + 0.5, h as f32 * 0.5 + 0.5, hz as f32 + 0.5])
            .open(BoxFace::NegZ)
            .with_fade(12.0),
    ));
}

/// The cave's roof: the hillside carried on over the cave (grass on top, rock underneath and
/// in the face above the mouth), so the hill reads as one piece.
fn cave_roof(
    commands: &mut Commands,
    meshes: &mut Assets<Mesh>,
    lib: &Library,
    land: &Landscape,
    c: DVec3,
    ceiling: f64,
    rng: &mut Rng,
) {
    let hole = land.cave.hole;
    // Mesh origin: the middle of the hole.
    let c = DVec3::new(land.cave.mouth.x, c.y, land.cave.mouth.y);
    // Past the hole by more than a terrain cell at the sides and back (tucked under the
    // terrain there); flush with the rock face at the front.
    let (x0, x1) = (-hole.x - 4.0, hole.x + 4.0);
    let (z0, z1) = (-hole.y - 0.5, hole.y + 4.0);
    let (nx, nz) = ((x1 - x0) as usize, (z1 - z0) as usize);
    let local = |i: usize, j: usize| {
        (
            x0 + (x1 - x0) * i as f64 / nx as f64,
            z0 + (z1 - z0) * j as f64 / nz as f64,
        )
    };
    let top = |i: usize, j: usize| {
        let (x, z) = local(i, j);
        let rim = i == 0 || i == nx || j == nz;
        let lift = if rim { -0.5 } else { 0.06 };
        land.cave_roof_height(c.x + x, c.z + z) + lift - c.y
    };
    let mut grass_top = Mesh::default();
    for j in 0..=nz {
        for i in 0..=nx {
            let (x, z) = local(i, j);
            let (wx, wz) = (c.x + x, c.z + z);
            let e = 0.5;
            let n = Vec3::new(
                (land.cave_roof_height(wx - e, wz) - land.cave_roof_height(wx + e, wz)) as f32,
                (2.0 * e) as f32,
                (land.cave_roof_height(wx, wz - e) - land.cave_roof_height(wx, wz + e)) as f32,
            )
            .normalize();
            let y = top(i, j);
            let color = land.roof_color(wx, wz, y + c.y, n);
            grass_top.vertices.push(
                engine::render::Vertex::new(
                    Vec3::new(x as f32, y as f32, z as f32),
                    n,
                    Vec2::new((wx / 6.0) as f32, (wz / 6.0) as f32),
                )
                .with_color(color.extend(1.0)),
            );
        }
    }
    let row = nx as u32 + 1;
    for j in 0..nz as u32 {
        for i in 0..nx as u32 {
            let a = j * row + i;
            let (b, cc) = (a + 1, a + row);
            grass_top
                .indices
                .extend_from_slice(&[a, cc, b, b, cc, cc + 1]);
        }
    }
    commands.spawn((
        at(c),
        MeshRenderer::new(meshes.add(grass_top), lib.mats.terrain),
    ));

    // Rock: the ceiling and the skirts down to it.
    let mut b = crate::meshgen::Builder::default();
    let y0 = ceiling as f32;
    let p = |i: usize, j: usize, y: f32| {
        let (x, z) = local(i, j);
        Vec3::new(x as f32, y, z as f32)
    };
    b.quad(
        [p(0, 0, y0), p(nx, 0, y0), p(nx, nz, y0), p(0, nz, y0)],
        ROCK_TINT,
    );
    let mut skirt = |a: (usize, usize), e: (usize, usize), out: Vec3| {
        let (ta, te) = (top(a.0, a.1) as f32, top(e.0, e.1) as f32);
        let mut q = [
            p(a.0, a.1, y0),
            p(e.0, e.1, y0),
            p(e.0, e.1, te),
            p(a.0, a.1, ta),
        ];
        if (q[1] - q[0]).cross(q[2] - q[0]).dot(out) < 0.0 {
            q = [q[1], q[0], q[3], q[2]];
        }
        b.quad(q, ROCK_TINT);
    };
    for i in 0..nx {
        skirt((i, 0), (i + 1, 0), Vec3::NEG_Z);
        skirt((i, nz), (i + 1, nz), Vec3::Z);
    }
    for j in 0..nz {
        skirt((0, j), (0, j + 1), Vec3::NEG_X);
        skirt((nx, j), (nx, j + 1), Vec3::X);
    }
    commands.spawn((
        at(c),
        MeshRenderer::new(meshes.add(rock_face(b)), lib.rocks.mats.rock),
    ));

    // Grass on the roof like on the rest of the hill.
    let mut x = x0 + 1.5;
    while x < x1 - 1.0 {
        let mut z = z0 + 1.5;
        while z < z1 - 1.0 {
            let (wx, wz) = (
                c.x + x + rng.range(-1.2, 1.2) as f64,
                c.z + z + rng.range(-1.2, 1.2) as f64,
            );
            let k = rng.range_i32(0, lib.grass.len() as i32) as usize;
            commands.spawn((
                Transform::from_translation(DVec3::new(wx, land.cave_roof_height(wx, wz), wz))
                    .with_rotation(Quat::from_rotation_y(rng.range(0.0, TAU)))
                    .with_scale(Vec3::new(1.0, rng.range(0.8, 1.3), 1.0)),
                MeshRenderer::new(lib.grass[k], lib.mats.grass).without_shadows(),
                scenery::Lod::swap_then_hide(
                    lib.grass[k],
                    lib.grass_mid[k],
                    rng.range(20.0, 26.0),
                    rng.range(45.0, 95.0),
                ),
            ));
            z += 3.6;
        }
        x += 3.6;
    }
}

/// The stream's water: a ribbon down its bed, white where it tumbles over the cliff, and a
/// foaming plunge pool. Stand-in until the water module fills the stream.
fn stream_water(commands: &mut Commands, meshes: &mut Assets<Mesh>, land: &Landscape) {
    let deep = Vec3::new(0.03, 0.09, 0.1);
    let foam = Vec3::new(0.75, 0.8, 0.82);
    let mut water = StandardMaterial::hex(0xffffff)
        .with_roughness(0.06)
        .with_alpha(0.9);
    water.reflectance = 0.5;
    water.receives_wetness = false;
    water.double_sided = true;
    for st in &land.streams {
        let n = st.path.len();
        let o = st.path[0];
        let origin = DVec3::new(o.x, 0.0, o.y);
        let level = |i: usize| st.bed[i] + st.depth * 0.55;
        let mut mesh = Mesh::default();
        for i in 0..n {
            let p = st.path[i];
            let dir =
                (st.path[(i + 1).min(n - 1)] - st.path[i.saturating_sub(1)]).normalize_or(DVec2::Y);
            let side = DVec2::new(-dir.y, dir.x);
            let drop = (level(i.saturating_sub(2)) - level((i + 2).min(n - 1))).max(0.0);
            let c = deep.lerp(foam, smoothstep(0.5, 1.8, drop as f32));
            let half = st.width * 0.5 + 1.2;
            for s in [-1.0, 1.0] {
                let q = p + side * half * s - o;
                mesh.vertices.push(
                    engine::render::Vertex::new(
                        Vec3::new(q.x as f32, level(i) as f32, q.y as f32),
                        Vec3::Y,
                        Vec2::new(s as f32 * 0.5 + 0.5, i as f32 * 0.2),
                    )
                    .with_color(c.extend(1.0)),
                );
            }
        }
        // The falls get their own curved sheet; the ribbon stops at the lip and starts again
        // at the foot.
        let falls = st.falls();
        for i in 0..n as u32 - 1 {
            if falls.is_some_and(|(top, bottom)| (top..bottom).contains(&(i as usize))) {
                continue;
            }
            let (a, b, c, d) = (2 * i, 2 * i + 1, 2 * i + 2, 2 * i + 3);
            let pos = |k: u32| Vec3::from(mesh.vertices[k as usize].position);
            // Wind so the ribbon faces up (or downstream where it falls).
            let up = (pos(b) - pos(a)).cross(pos(c) - pos(a)).y > 0.0;
            if up {
                mesh.indices.extend_from_slice(&[a, b, c, b, d, c]);
            } else {
                mesh.indices.extend_from_slice(&[a, c, b, b, c, d]);
            }
        }
        // Face normals averaged per vertex, so the falls shade as a sheet.
        let mut normals = vec![Vec3::ZERO; mesh.vertices.len()];
        for t in mesh.indices.chunks(3) {
            let p = |k: u32| Vec3::from(mesh.vertices[k as usize].position);
            let nrm = (p(t[1]) - p(t[0])).cross(p(t[2]) - p(t[0]));
            for &k in t {
                normals[k as usize] += nrm;
            }
        }
        for (v, nrm) in mesh.vertices.iter_mut().zip(normals) {
            v.normal = nrm.normalize_or(Vec3::Y).to_array();
        }
        commands.spawn((
            at(origin),
            MeshRenderer::new(meshes.add(mesh), water).without_shadows(),
        ));
        if let Some((top, bottom)) = falls {
            // Falling water is mostly air: a rough, white, barely mirror-like sheet.
            let mut white_water = StandardMaterial::hex(0xffffff)
                .with_roughness(0.35)
                .with_subsurface(0.7)
                .with_alpha(0.93);
            white_water.reflectance = 0.25;
            white_water.receives_wetness = false;
            white_water.double_sided = true;
            let sheet = falls_sheet(st, top, bottom, &level, o);
            commands.spawn((
                at(origin),
                MeshRenderer::new(meshes.add(sheet), white_water).without_shadows(),
            ));
        }
        if let (Some((c, _)), Some((_, bottom))) = (st.pool, falls) {
            let mut froth = StandardMaterial::hex(0xffffff)
                .with_roughness(0.45)
                .with_subsurface(0.5)
                .with_alpha(0.85);
            froth.receives_wetness = false;
            froth.double_sided = true;
            commands.spawn((
                at(DVec3::new(c.x, level(bottom) + 0.03, c.y)),
                MeshRenderer::new(meshes.add(froth_patch(5.5)), froth).without_shadows(),
            ));
        }
    }
}

/// Churned foam in the plunge pool: a disc with a ragged edge, white in the middle and fading
/// toward the water's color at the rim.
fn froth_patch(radius: f64) -> Mesh {
    const SEGMENTS: usize = 64;
    const RINGS: usize = 5;
    let mut rng = Rng::new(0xF20F);
    let edge: Vec<f64> = (0..SEGMENTS)
        .map(|_| radius * rng.range(0.6, 1.05) as f64)
        .collect();
    let white = Vec3::new(0.92, 0.95, 0.96);
    let rim = Vec3::new(0.3, 0.38, 0.4);
    let mut mesh = Mesh::default();
    mesh.vertices.push(
        engine::render::Vertex::new(Vec3::ZERO, Vec3::Y, Vec2::splat(0.5))
            .with_color(white.extend(1.0)),
    );
    for r in 1..=RINGS {
        let f = r as f64 / RINGS as f64;
        for (k, &e) in edge.iter().enumerate() {
            // Smooth the ragged edge a little between neighbours.
            let e = (e + edge[(k + 1) % SEGMENTS] + edge[(k + SEGMENTS - 1) % SEGMENTS]) / 3.0;
            let a = k as f64 / SEGMENTS as f64 * std::f64::consts::TAU;
            let p = DVec2::new(a.cos(), a.sin()) * e * f;
            let streak = rng.range(0.75, 1.0);
            let c = white.lerp(rim, smoothstep(0.45, 1.0, f as f32)) * streak;
            mesh.vertices.push(
                engine::render::Vertex::new(
                    Vec3::new(p.x as f32, 0.0, p.y as f32),
                    Vec3::Y,
                    Vec2::new(p.x as f32, p.y as f32) * 0.2,
                )
                .with_color(c.extend(1.0)),
            );
        }
    }
    let ring = |r: usize, k: usize| (1 + (r - 1) * SEGMENTS + k % SEGMENTS) as u32;
    for k in 0..SEGMENTS {
        mesh.indices
            .extend_from_slice(&[0, ring(1, k + 1), ring(1, k)]);
        for r in 1..RINGS {
            let (a, b, c, d) = (
                ring(r, k),
                ring(r, k + 1),
                ring(r + 1, k),
                ring(r + 1, k + 1),
            );
            mesh.indices.extend_from_slice(&[a, b, c, b, d, c]);
        }
    }
    mesh
}

/// The waterfall as a sheet that leaves the lip, arcs out and spreads as it falls: ragged
/// columns of glassy water near the lip turning to white, aerated streaks below. Positions are
/// relative to `o` (the stream's first point).
fn falls_sheet(
    st: &crate::landscape::Stream,
    top: usize,
    bottom: usize,
    level: &dyn Fn(usize) -> f64,
    o: DVec2,
) -> Mesh {
    const COLUMNS: usize = 28;
    const ROWS: usize = 22;
    let glassy = Vec3::new(0.1, 0.2, 0.2);
    let white = Vec3::new(0.93, 0.95, 0.96);
    let lip = st.path[top];
    let dir = (st.path[top + 1] - st.path[top.saturating_sub(1)]).normalize_or(DVec2::Y);
    let side = DVec2::new(-dir.y, dir.x);
    let (y0, y1) = (level(top), level(bottom));
    let drop = (y0 - y1).max(0.5);
    let fall_time = (2.0 * drop / 9.81).sqrt();
    let reach = (st.path[bottom] - lip).length() * 0.75;
    let mut rng = Rng::new(0xFA11);
    let mut mesh = Mesh::default();
    // Each column leaves the lip a little faster or slower, so the sheet tears into streaks.
    let speeds: Vec<f64> = (0..=COLUMNS)
        .map(|_| reach / fall_time * rng.range(0.8, 1.15) as f64)
        .collect();
    let streak: Vec<f32> = (0..=COLUMNS).map(|_| rng.range(0.0, 1.0)).collect();
    for (k, (&speed, &streak)) in speeds.iter().zip(&streak).enumerate() {
        let u = k as f64 / COLUMNS as f64 - 0.5;
        for j in 0..=ROWS {
            let t = j as f64 / ROWS as f64 * fall_time;
            let width = st.width * (0.8 + 0.4 * j as f64 / ROWS as f64);
            let p = lip + dir * (speed * t) + side * (width * u) - o;
            let y = (y0 - 0.5 * 9.81 * t * t).max(y1);
            // Glassy over the lip, white and streaky once it breaks up.
            let foam = (smoothstep(0.0, 0.25, j as f32 / ROWS as f32) * (0.7 + 0.3 * streak))
                .clamp(0.0, 1.0);
            let c = glassy.lerp(white, foam);
            mesh.vertices.push(
                engine::render::Vertex::new(
                    Vec3::new(p.x as f32, y as f32, p.y as f32),
                    Vec3::Y,
                    Vec2::new(k as f32 / COLUMNS as f32, j as f32 / ROWS as f32),
                )
                .with_color(c.extend(1.0)),
            );
        }
    }
    let at_ = |k: usize, j: usize| (k * (ROWS + 1) + j) as u32;
    for k in 0..COLUMNS {
        for j in 0..ROWS {
            let (a, b, c, d) = (at_(k, j), at_(k + 1, j), at_(k, j + 1), at_(k + 1, j + 1));
            mesh.indices.extend_from_slice(&[a, b, c, b, d, c]);
        }
    }
    // Smooth normals facing away from the cliff (the material is double-sided).
    let mut normals = vec![Vec3::ZERO; mesh.vertices.len()];
    for t in mesh.indices.chunks(3) {
        let p = |k: u32| Vec3::from(mesh.vertices[k as usize].position);
        let nrm = (p(t[1]) - p(t[0])).cross(p(t[2]) - p(t[0]));
        for &k in t {
            normals[k as usize] += nrm;
        }
    }
    let out = Vec3::new(dir.x as f32, 0.0, dir.y as f32);
    for (v, nrm) in mesh.vertices.iter_mut().zip(normals) {
        let n = nrm.normalize_or(out);
        v.normal = (if n.dot(out) < 0.0 { -n } else { n }).to_array();
    }
    mesh
}

/// Dry grass around the fire module's burning meadow (which fills the middle of the zone), out
/// to the fence.
pub fn field_edge(commands: &mut Commands, lib: &Library, z: &Zone, rng: &mut Rng) {
    let c = z.center;
    let fire = engine_fire::demo::FOOTPRINT;
    let spacing = 2.6;
    let n = (z.radius * 2.0 / spacing) as i32;
    for i in 0..n {
        for j in 0..n {
            let x = -z.radius + (i as f64 + rng.range(0.2, 0.8) as f64) * spacing;
            let zz = -z.radius + (j as f64 + rng.range(0.2, 0.8) as f64) * spacing;
            let inside_fire = x.abs() < fire[0] as f64 + 1.5 && zz.abs() < fire[1] as f64 + 1.5;
            if x * x + zz * zz > z.radius * z.radius || inside_fire {
                continue;
            }
            let k = rng.range_i32(0, lib.dry_grass.len() as i32) as usize;
            commands.spawn((
                Transform::from_translation(c + DVec3::new(x, -0.02, zz))
                    .with_rotation(Quat::from_rotation_y(rng.range(0.0, TAU)))
                    .with_scale(Vec3::new(1.0, rng.range(1.3, 1.9), 1.0)),
                MeshRenderer::new(lib.dry_grass[k], lib.mats.grass).without_shadows(),
                scenery::Lod::swap_then_hide(lib.dry_grass[k], lib.dry_grass_mid[k], 26.0, 140.0),
            ));
        }
    }
    // Fence around the field.
    for i in 0..48 {
        let a = i as f64 / 48.0 * TAU64;
        let p = c + DVec3::new(a.cos() * (z.radius + 2.0), 0.0, a.sin() * (z.radius + 2.0));
        spawn(
            commands,
            cuboid(0.12, 1.2, 0.12),
            dark_wood(),
            at(p + DVec3::new(0.0, 0.6, 0.0)),
        );
        spawn(
            commands,
            cuboid(8.0, 0.08, 0.06),
            wood(),
            yawed(
                p + DVec3::new(0.0, 0.9, 0.0),
                -(a as f32) - std::f32::consts::FRAC_PI_2 + std::f32::consts::FRAC_PI_2,
            ),
        );
    }
}

/// Spaceport: a 600 m launch tower for scale (the pad, its lights, hangars and the ships
/// come from `engine_vehicles::demo::spawn_spaceport`). The Planets and terrain (space) and
/// Cars, planes and spaceships threads own it.
pub fn spaceport(commands: &mut Commands, _lib: &Library, z: &Zone) {
    let c = z.center;
    // The 600 m launch tower: a tapering spire with a lit band every 50 m.
    let tower = c + DVec3::new(70.0, 0.0, 60.0);
    let shell = mat(0xd8dce0, 0.4).with_metallic(0.3);
    for i in 0..40 {
        let y = i as f64 * 15.0;
        let r0 = 14.0 - i as f32 * 0.3;
        spawn(
            commands,
            Shape::Cylinder {
                radius: r0,
                height: 15.2,
                segments: 48,
            },
            shell,
            at(tower + DVec3::new(0.0, y + 7.5, 0.0)),
        );
        if i % 4 == 3 {
            spawn(
                commands,
                Shape::Torus {
                    radius: r0 + 0.3,
                    tube: 0.35,
                    segments: 64,
                },
                StandardMaterial::hex(0x101820).with_emissive([0.5, 0.8, 1.0], 3_000.0),
                at(tower + DVec3::new(0.0, y + 14.0, 0.0)),
            );
        }
    }
    spawn(
        commands,
        Shape::Sphere {
            radius: 2.0,
            segments: 24,
        },
        StandardMaterial::hex(0x400000).with_emissive([1.0, 0.1, 0.05], 50_000.0),
        at(tower + DVec3::new(0.0, 606.0, 0.0)),
    );
}
