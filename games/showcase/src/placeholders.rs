//! Stand-in content for each zone until its module's `demo::spawn_demo` replaces it (see
//! `zones::spawn_cell`). Placeholders show where a test case lives and what it will contain;
//! they don't simulate anything.

use crate::landscape::Landscape;
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

fn at(p: DVec3) -> Transform {
    Transform::from_translation(p)
}

fn yawed(p: DVec3, yaw: f32) -> Transform {
    Transform::from_translation(p).with_rotation(Quat::from_rotation_y(yaw))
}

fn mat(hex: u32, roughness: f32) -> StandardMaterial {
    StandardMaterial::hex(hex).with_roughness(roughness)
}

fn wood() -> StandardMaterial {
    mat(0x7a5434, 0.8)
}
fn dark_wood() -> StandardMaterial {
    mat(0x4a3220, 0.8)
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
    let (_, hz, h) = CAVE_INSIDE;
    let kit = &lib.rocks;
    // The rock inside: Foliage's sculpted chamber (walls bulging into alcoves, ledges,
    // flowstone, stalactites, a vaulted roof, the 10 m mouth in its −Z wall and no sky light
    // but through the mouth), the same size as the hole the landscape digs.
    engine_foliage::demo::spawn_cave_chamber(commands, c);
    cave_roof(commands, meshes, lib, land, c, h, rng);
    // (The boulders that once lined the box walls are gone with them; their draws stay, so
    // everything laid out after them keeps its place.)
    let mut pick = Rng::new(0x00ca_7eb0_u64);
    for k in 0..64 {
        let _ = (rng.range(-1.0, 1.0), rng.range(-0.5, h as f32 - 1.0));
        if k % 4 == 3 {
            let _ = rng.range(-hz as f32, hz as f32);
        }
        let _ = (
            rng.range_i32(0, 6),
            rng.range(0.0, TAU),
            rng.range(-0.6, 0.6),
        );
        let _ = rng.range(1.4, 3.0);
        let _ = kit.pick(crate::rocks::RockSize::Large, pick.chance(0.5), &mut pick);
        let _ = pick.range(0.7, 1.0);
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

/// Spaceport: a 600 m observation tower for scale (the pad, its lights, hangars and the ships
/// come from `engine_vehicles::demo::spawn_spaceport`). The Planets and terrain (space) and
/// Cars, planes and spaceships threads own it.
pub fn spaceport(commands: &mut Commands, lib: &Library, z: &Zone) {
    let c = z.center;
    // The 600 m telecom and observation tower (`tower.rs`).
    crate::tower::spawn(commands, &lib.tower, c + DVec3::new(70.0, 0.0, 60.0));
}
