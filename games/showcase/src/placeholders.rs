//! Stand-in content for each zone until its module's `demo::spawn_demo` replaces it (see
//! `zones::spawn_demos`). Placeholders show where a test case lives and what it will contain;
//! they don't simulate anything.

use crate::landscape::{Cover, Landscape};
use crate::meshgen::TreeKind;
use crate::scenery::{self, Library};
use crate::zones::{FOOTSTEP_HALF_WIDTH, FOOTSTEP_STRIPS, RUNWAY, Zone};
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

fn concrete() -> StandardMaterial {
    mat(0xa8a49c, 0.8)
}
fn stone() -> StandardMaterial {
    mat(0x8a857c, 0.85)
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
fn rock() -> StandardMaterial {
    mat(0x5a5650, 0.9)
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

/// A four-legged animal of boxes and capsules (deer-sized at scale 1), facing `yaw`.
fn quadruped(commands: &mut Commands, base: DVec3, yaw: f32, scale: f32, coat: u32) {
    let fur = mat(coat, 0.9).with_subsurface(0.2);
    let rot = Quat::from_rotation_y(yaw);
    let p = |v: Vec3| base + (rot * (v * scale)).as_dvec3();
    spawn(
        commands,
        Shape::Capsule {
            radius: 0.28,
            height: 1.3,
            segments: 12,
        },
        fur,
        Transform::from_translation(p(Vec3::new(0.0, 1.05, 0.0)))
            .with_rotation(rot * Quat::from_rotation_x(std::f32::consts::FRAC_PI_2))
            .with_scale(Vec3::splat(scale)),
    );
    for (x, z) in [(-0.17, 0.45), (0.17, 0.45), (-0.17, -0.45), (0.17, -0.45)] {
        spawn(
            commands,
            Shape::Capsule {
                radius: 0.05,
                height: 0.85,
                segments: 8,
            },
            fur,
            Transform::from_translation(p(Vec3::new(x, 0.45, z))).with_scale(Vec3::splat(scale)),
        );
    }
    spawn(
        commands,
        Shape::Capsule {
            radius: 0.09,
            height: 0.55,
            segments: 8,
        },
        fur,
        Transform::from_translation(p(Vec3::new(0.0, 1.45, 0.62)))
            .with_rotation(rot * Quat::from_rotation_x(0.6))
            .with_scale(Vec3::splat(scale)),
    );
    spawn(
        commands,
        Shape::Capsule {
            radius: 0.1,
            height: 0.32,
            segments: 8,
        },
        fur,
        Transform::from_translation(p(Vec3::new(0.0, 1.68, 0.8)))
            .with_rotation(rot * Quat::from_rotation_x(1.3))
            .with_scale(Vec3::splat(scale)),
    );
}

/// Material plaza: a paved square with a roughness ladder (dielectric and metal), a glass orb
/// inside an emissive ring and pedestals. The Rendering and lighting thread owns its content.
pub fn plaza(commands: &mut Commands, lib: &Library, z: &Zone) {
    let c = z.center;
    // Paving: concentric rings of slightly different stone.
    for (i, r) in [31.0f32, 24.0, 16.0, 8.0].into_iter().enumerate() {
        let shade = [0x8f877c, 0x9d958a, 0x8a8277, 0xa39b90][i];
        spawn(
            commands,
            cyl(r, 0.2),
            mat(shade, 0.85),
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
        StandardMaterial::hex(0x202830).with_emissive([0.3, 0.7, 1.0], 8_000.0),
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

/// Animation stage: a wooden stage with mannequins, a horse and a cart (humanoids, animals,
/// vehicles; muscles and cloth tension). The Animation thread owns it.
pub fn stage(commands: &mut Commands, _lib: &Library, z: &Zone) {
    let c = z.center;
    spawn(
        commands,
        cyl(14.0, 1.0),
        wood(),
        at(c + DVec3::new(0.0, 0.5, 0.0)),
    );
    spawn(
        commands,
        cyl(14.4, 0.3),
        dark_wood(),
        at(c + DVec3::new(0.0, 0.15, 0.0)),
    );
    let top = c + DVec3::new(0.0, 1.0, 0.0);
    for (i, x) in [-6.0, -3.0, 0.0, 3.0].into_iter().enumerate() {
        person(
            commands,
            top + DVec3::new(x, 0.0, -3.0),
            (i as f32) * 0.5,
            [0xd35400, 0x2980b9, 0x16a085, 0x8e44ad][i],
        );
    }
    quadruped(
        commands,
        top + DVec3::new(-3.0, 0.0, 4.0),
        0.4,
        1.35,
        0x5a3a22,
    );
    quadruped(
        commands,
        top + DVec3::new(4.0, 0.0, 3.0),
        -0.8,
        0.55,
        0x9a8a70,
    );
    // A cart.
    spawn(
        commands,
        cuboid(2.0, 0.6, 3.0),
        wood(),
        at(top + DVec3::new(6.0, 0.9, -4.0)),
    );
    for (x, zz) in [(-1.1, -1.0), (1.1, -1.0), (-1.1, 1.0), (1.1, 1.0)] {
        spawn(
            commands,
            Shape::Torus {
                radius: 0.45,
                tube: 0.06,
                segments: 20,
            },
            dark_wood(),
            Transform::from_translation(top + DVec3::new(6.0 + x, 0.5, -4.0 + zz))
                .with_rotation(Quat::from_rotation_z(std::f32::consts::FRAC_PI_2)),
        );
    }
    // Backdrop and stage lights.
    spawn(
        commands,
        cuboid(26.0, 7.0, 0.4),
        mat(0x5b2333, 0.9).with_subsurface(0.3),
        at(c + DVec3::new(0.0, 4.5, -12.5)),
    );
    for x in [-12.0, 12.0] {
        spawn(
            commands,
            cuboid(0.4, 9.0, 0.4),
            dark_wood(),
            at(c + DVec3::new(x, 4.5, -12.0)),
        );
    }
}

/// Physics playground: domino run, crate pyramid, Newton's cradle, ramp with balls, seesaw.
/// The Physics and rigid bodies thread owns it.
pub fn physics(commands: &mut Commands, _lib: &Library, z: &Zone) {
    let c = z.center;
    spawn(
        commands,
        cyl(32.0, 0.15),
        concrete(),
        at(c + DVec3::new(0.0, 0.075, 0.0)),
    );
    // Dominoes on a spiral.
    for i in 0..60 {
        let t = i as f64 * 0.18;
        let r = 4.0 + t * 1.1;
        let p = c + DVec3::new(-12.0 + t.cos() * r * 0.5, 0.65, t.sin() * r * 0.5);
        spawn(
            commands,
            cuboid(0.1, 1.0, 0.5),
            mat([0xe74c3c, 0xf1c40f, 0x3498db][i % 3], 0.4),
            yawed(p, -(t as f32)),
        );
    }
    // Crate pyramid.
    for row in 0..6 {
        for i in 0..(6 - row) {
            let x = (i as f64 - (5 - row) as f64 * 0.5) * 1.05;
            spawn(
                commands,
                cuboid(1.0, 1.0, 1.0),
                wood(),
                at(c + DVec3::new(10.0 + x, 0.65 + row as f64, -8.0)),
            );
        }
    }
    // Newton's cradle.
    let nc = c + DVec3::new(8.0, 0.0, 9.0);
    for x in [-1.5, 1.5] {
        for zz in [-0.6, 0.6] {
            spawn(
                commands,
                cuboid(0.08, 2.4, 0.08),
                metal(),
                at(nc + DVec3::new(x, 1.35, zz)),
            );
        }
        spawn(
            commands,
            cuboid(0.08, 0.08, 1.3),
            metal(),
            at(nc + DVec3::new(x, 2.55, 0.0)),
        );
    }
    spawn(
        commands,
        cuboid(3.1, 0.08, 0.08),
        metal(),
        at(nc + DVec3::new(0.0, 2.55, 0.0)),
    );
    for i in 0..5 {
        let x = -0.8 + i as f64 * 0.4;
        spawn(
            commands,
            Shape::Sphere {
                radius: 0.2,
                segments: 24,
            },
            metal(),
            at(nc + DVec3::new(x, 1.2, 0.0)),
        );
    }
    // Ramp with balls and a seesaw.
    spawn(
        commands,
        cuboid(3.0, 0.3, 12.0),
        concrete(),
        Transform::from_translation(c + DVec3::new(-8.0, 2.2, -14.0))
            .with_rotation(Quat::from_rotation_x(0.35)),
    );
    for i in 0..4 {
        spawn(
            commands,
            Shape::Sphere {
                radius: 0.5,
                segments: 24,
            },
            mat([0x8e44ad, 0x27ae60, 0xe67e22, 0x2c3e50][i], 0.3),
            at(c + DVec3::new(-8.0 + (i as f64 - 1.5) * 0.7, 4.6, -18.5)),
        );
    }
    spawn(
        commands,
        cuboid(0.6, 0.6, 0.6),
        dark_wood(),
        at(c + DVec3::new(-2.0, 0.45, 14.0)),
    );
    spawn(
        commands,
        cuboid(8.0, 0.2, 1.0),
        wood(),
        Transform::from_translation(c + DVec3::new(-2.0, 0.95, 14.0))
            .with_rotation(Quat::from_rotation_z(0.12)),
    );
}

/// Flags, a curtain, a rope bridge and jelly cubes: rigid stand-ins until `engine_softbody`
/// makes them move. The Cloth, soft bodies and hair thread owns it.
pub fn yard(commands: &mut Commands, _lib: &Library, z: &Zone) {
    let c = z.center;
    let flag_colors = [0xc0392b, 0x2e86c1, 0xf1c40f, 0x27ae60, 0x8e44ad];
    for (i, hex) in flag_colors.into_iter().enumerate() {
        let base = c + DVec3::new(-16.0 + i as f64 * 8.0, 0.0, -18.0);
        spawn(
            commands,
            cyl(0.08, 9.0),
            metal(),
            at(base + DVec3::new(0.0, 4.5, 0.0)),
        );
        spawn(
            commands,
            cuboid(3.0, 2.0, 0.03),
            mat(hex, 0.8).with_subsurface(0.4),
            at(base + DVec3::new(1.55, 7.8, 0.0)),
        );
    }
    let frame = c + DVec3::new(-12.0, 0.0, 6.0);
    for x in [-2.6, 2.6] {
        spawn(
            commands,
            cuboid(0.3, 6.0, 0.3),
            wood(),
            at(frame + DVec3::new(x, 3.0, 0.0)),
        );
    }
    spawn(
        commands,
        cuboid(5.5, 0.3, 0.3),
        wood(),
        at(frame + DVec3::new(0.0, 6.1, 0.0)),
    );
    spawn(
        commands,
        cuboid(4.8, 5.6, 0.04),
        mat(0x7b1e2b, 0.9).with_subsurface(0.3),
        at(frame + DVec3::new(0.0, 3.2, 0.0)),
    );
    let (a, b) = (
        c + DVec3::new(2.0, 0.0, 10.0),
        c + DVec3::new(24.0, 0.0, 10.0),
    );
    for p in [a, b] {
        spawn(
            commands,
            cuboid(3.0, 3.0, 3.0),
            rock(),
            at(p + DVec3::new(0.0, 1.5, 0.0)),
        );
    }
    for i in 0..18 {
        let t = (i as f64 + 0.5) / 18.0;
        let sag = 1.2 * (1.0 - (2.0 * t - 1.0).powi(2));
        spawn(
            commands,
            cuboid(0.9, 0.06, 1.6),
            wood(),
            at(a.lerp(b, t) + DVec3::new(0.0, 3.0 - sag, 0.0)),
        );
    }
    let jelly = StandardMaterial::hex(0x5fd38d)
        .with_roughness(0.15)
        .with_subsurface(0.8)
        .with_alpha(0.75);
    for (i, x) in [0.0, 1.6, 0.8].into_iter().enumerate() {
        let y = if i < 2 { 0.0 } else { 1.0 };
        spawn(
            commands,
            cuboid(1.0, 1.0, 1.0),
            wood(),
            at(c + DVec3::new(10.0 + x, 0.5 + y, -4.0)),
        );
        spawn(
            commands,
            cuboid(0.9, 0.9, 0.9),
            jelly,
            at(c + DVec3::new(10.0 + x, 1.45 + y, -4.0)),
        );
    }
    // A washing line and a hairy, furry pair: a long-haired person and a shaggy dog.
    person(commands, c + DVec3::new(-2.0, 0.0, -6.0), 0.3, 0xe8e0d0);
    quadruped(commands, c + DVec3::new(1.0, 0.0, -7.0), 2.2, 0.5, 0x6b5030);
}

/// Magic circle: standing stones around a rune disc. The Particles and magic effects thread
/// owns it.
pub fn magic(commands: &mut Commands, _lib: &Library, z: &Zone) {
    let c = z.center;
    let mut rng = Rng::new(42);
    for i in 0..10 {
        let a = i as f64 / 10.0 * TAU64;
        let h = rng.range(4.0, 6.5);
        spawn(
            commands,
            cuboid(1.6, h, 0.9),
            mat(0x6e6a62, 0.95),
            Transform::from_translation(
                c + DVec3::new(a.cos() * 16.0, h as f64 * 0.5 - 0.3, a.sin() * 16.0),
            )
            .with_rotation(
                Quat::from_rotation_y(-(a as f32)) * Quat::from_rotation_z(rng.range(-0.06, 0.06)),
            ),
        );
        if i % 2 == 0 {
            spawn(
                commands,
                cuboid(4.0, 0.9, 1.0),
                mat(0x6e6a62, 0.95),
                Transform::from_translation(
                    c + DVec3::new(a.cos() * 16.0, h as f64 + 0.1, a.sin() * 16.0),
                )
                .with_rotation(Quat::from_rotation_y(
                    -(a as f32) + std::f32::consts::FRAC_PI_2,
                )),
            );
        }
    }
    spawn(
        commands,
        cyl(7.0, 0.2),
        mat(0x3a3836, 0.9),
        at(c + DVec3::new(0.0, 0.1, 0.0)),
    );
    for (r, hex) in [(6.0, 0x6a3cff), (3.5, 0x2fd6ff)] {
        spawn(
            commands,
            Shape::Torus {
                radius: r,
                tube: 0.08,
                segments: 96,
            },
            StandardMaterial::hex(0x101018).with_emissive(
                [
                    ((hex >> 16) & 255) as f32 / 255.0,
                    ((hex >> 8) & 255) as f32 / 255.0,
                    (hex & 255) as f32 / 255.0,
                ],
                2_000.0,
            ),
            at(c + DVec3::new(0.0, 0.22, 0.0)),
        );
    }
    spawn(
        commands,
        cuboid(1.2, 1.1, 1.2),
        mat(0x4a4744, 0.9),
        at(c + DVec3::new(0.0, 0.75, 0.0)),
    );
    spawn(
        commands,
        Shape::Sphere {
            radius: 0.35,
            segments: 32,
        },
        StandardMaterial::hex(0x203040).with_emissive([0.5, 0.8, 1.0], 6_000.0),
        at(c + DVec3::new(0.0, 1.7, 0.0)),
    );
    commands.spawn((
        at(c + DVec3::new(0.0, 2.0, 0.0)),
        PointLight {
            color: [0.5, 0.75, 1.0],
            intensity_lm: 1200.0,
            range: 14.0,
            radius: 0.3,
        },
    ));
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

/// The living forest: dense mixed woodland, a clearing with stumps and a fallen trunk (chop and
/// regrow). The Foliage and ecology thread owns it.
pub fn forest(commands: &mut Commands, lib: &Library, land: &Landscape, z: &Zone, rng: &mut Rng) {
    let mut count = 0;
    while count < 520 {
        let p = rng.in_unit_disk() * z.radius as f32;
        let (x, zz) = (z.center.x + p.x as f64, z.center.z + p.y as f64);
        // A clearing in the middle.
        if p.length() < 18.0 {
            continue;
        }
        let kind = match rng.f32() {
            r if r < 0.5 => TreeKind::Spruce,
            r if r < 0.85 => TreeKind::Oak,
            _ => TreeKind::Birch,
        };
        let t = scenery::pick_tree(lib, kind, rng);
        let pos = DVec3::new(x, land.height(x, zz) - 0.1, zz);
        scenery::spawn_tree(
            commands,
            lib,
            t,
            pos,
            rng.range(0.0, TAU),
            rng.range(0.8, 1.3),
        );
        count += 1;
    }
    let c = z.center;
    let bark = mat(0x4a3424, 0.95);
    for (x, zz) in [(-6.0, 4.0), (5.0, -3.0), (2.0, 8.0)] {
        spawn(
            commands,
            cyl(0.45, 0.6),
            bark,
            at(c + DVec3::new(x, 0.3, zz)),
        );
        spawn(
            commands,
            cyl(0.4, 0.02),
            mat(0xb08a5a, 0.8),
            at(c + DVec3::new(x, 0.61, zz)),
        );
    }
    spawn(
        commands,
        Shape::Cylinder {
            radius: 0.4,
            height: 12.0,
            segments: 12,
        },
        bark,
        Transform::from_translation(c + DVec3::new(-2.0, 0.4, -6.0)).with_rotation(
            Quat::from_rotation_z(std::f32::consts::FRAC_PI_2) * Quat::from_rotation_x(0.4),
        ),
    );
    // Saplings regrowing in the clearing.
    for i in 0..6 {
        let a = i as f64 / 6.0 * TAU64;
        let t = scenery::pick_tree(lib, TreeKind::Spruce, rng);
        scenery::spawn_tree(
            commands,
            lib,
            t,
            c + DVec3::new(a.cos() * 9.0, 0.0, a.sin() * 9.0),
            0.0,
            0.12 + i as f32 * 0.04,
        );
    }
}

/// Wildlife meadow: grazing deer, a few hares. The Smart NPC AI thread (wildlife) and the
/// Animation thread own it.
pub fn meadow(commands: &mut Commands, _lib: &Library, z: &Zone, rng: &mut Rng) {
    for _ in 0..7 {
        let p = rng.in_unit_disk() * 25.0;
        quadruped(
            commands,
            z.center + DVec3::new(p.x as f64, 0.0, p.y as f64),
            rng.range(0.0, TAU),
            rng.range(0.9, 1.1),
            0x8a5a32,
        );
    }
    for _ in 0..5 {
        let p = rng.in_unit_disk() * 30.0;
        quadruped(
            commands,
            z.center + DVec3::new(p.x as f64, 0.0, p.y as f64),
            rng.range(0.0, TAU),
            0.25,
            0x9a8a70,
        );
    }
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

/// Footstep trail: strips of grass, mud, a puddle, sand, gravel, snow, boards, a metal grate
/// and dirt across the road (the ground query reports each surface). The Audio and footsteps
/// and Planets and terrain threads own it.
pub fn footsteps(commands: &mut Commands, _lib: &Library, z: &Zone) {
    let c = z.center;
    let mut start = c.z - FOOTSTEP_STRIPS.iter().map(|s| s.1).sum::<f64>() * 0.5;
    let w = FOOTSTEP_HALF_WIDTH as f32 * 2.0;
    for (cover, len) in FOOTSTEP_STRIPS {
        let mid = DVec3::new(c.x, 0.0, start + len * 0.5);
        match cover {
            Cover::Puddle => {
                let mut water = StandardMaterial::hex(0x2a2a24)
                    .with_roughness(0.02)
                    .with_alpha(0.7);
                water.receives_wetness = false;
                spawn(
                    commands,
                    cuboid(w, 0.02, *len as f32 - 0.5),
                    water,
                    at(mid + DVec3::new(0.0, 0.03, 0.0)),
                );
            }
            Cover::Mud => {
                let mut mud = StandardMaterial::hex(0x2a1d12).with_roughness(0.35);
                mud.receives_wetness = false;
                spawn(
                    commands,
                    cuboid(w, 0.04, *len as f32),
                    mud,
                    at(mid + DVec3::new(0.0, 0.02, 0.0)),
                );
            }
            Cover::Wood => {
                for k in 0..(*len as i32 * 4) {
                    let zz = start + 0.125 + k as f64 * 0.25;
                    spawn(
                        commands,
                        cuboid(w, 0.06, 0.22),
                        wood(),
                        at(DVec3::new(c.x, 0.12, zz)),
                    );
                }
                for x in [-5.0, 0.0, 5.0] {
                    spawn(
                        commands,
                        cuboid(0.2, 0.1, *len as f32),
                        dark_wood(),
                        at(DVec3::new(c.x + x, 0.05, mid.z)),
                    );
                }
            }
            Cover::Metal => {
                for k in 0..(*len as i32 * 5) {
                    let zz = start + 0.1 + k as f64 * 0.2;
                    spawn(
                        commands,
                        cuboid(w, 0.05, 0.04),
                        metal(),
                        at(DVec3::new(c.x, 0.1, zz)),
                    );
                }
                spawn(
                    commands,
                    cuboid(w, 0.05, 0.04),
                    metal(),
                    at(DVec3::new(c.x, 0.1, mid.z)),
                );
            }
            _ => {}
        }
        start += len;
    }
    // Sign posts at the start.
    spawn(
        commands,
        cuboid(0.15, 2.2, 0.15),
        dark_wood(),
        at(DVec3::new(c.x + 8.5, 1.1, c.z - 36.0)),
    );
    spawn(
        commands,
        cuboid(1.6, 0.6, 0.08),
        wood(),
        at(DVec3::new(c.x + 8.5, 2.0, c.z - 36.0)),
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
    let wall = rock();
    // Back and side walls, thick enough to reach the edge of the hole the landscape digs.
    spawn(
        commands,
        cuboid(44.0, 10.0, 7.0),
        wall,
        at(c + DVec3::new(0.0, 5.0, hz + 3.5)),
    );
    for s in [-1.0, 1.0] {
        spawn(
            commands,
            cuboid(7.0, 10.0, 43.0),
            wall,
            at(c + DVec3::new(s * (hx + 3.5), 5.0, 2.5)),
        );
    }
    cave_roof(commands, meshes, lib, land, c, h, rng);
    // Boulders sunk into the walls and hanging from the ceiling, so the inside reads as rock
    // rather than a box. Each sits with its flat base in the wall and its rounded face out.
    // (Shapes and tilts come from their own stream; the shared one advances as before.)
    let kit = &lib.rocks;
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
    // Front wall with the mouth (10 m wide).
    for s in [-1.0, 1.0] {
        spawn(
            commands,
            cuboid(10.0, h as f32, 3.0),
            wall,
            at(c + DVec3::new(s * 10.0, h * 0.5, -hz - 0.5)),
        );
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
    commands.spawn((
        at(c + DVec3::new(0.0, h * 0.5, 0.0)),
        // Just past the inner wall faces so the walls count as inside too.
        SkyLightVolume::new([hx as f32 + 0.5, h as f32 * 0.5 + 0.5, hz as f32 + 0.5])
            .open(BoxFace::NegZ)
            .with_fade(12.0),
    ));
}

/// Footprint lab: strips of mud, snow, sand and grass (painted into the ground by the
/// landscape) with walkers that will leave tracks. The Planets and terrain thread owns it.
pub fn tracks(commands: &mut Commands, z: &Zone) {
    let c = z.center;
    for (i, x) in [-15.0, -5.0, 5.0, 15.0].into_iter().enumerate() {
        person(
            commands,
            c + DVec3::new(x, 0.0, -10.0 + i as f64 * 5.0),
            0.0,
            [0x8e44ad, 0x2c3e50, 0xc0392b, 0x16a085][i],
        );
    }
    quadruped(
        commands,
        c + DVec3::new(-5.0, 0.0, 8.0),
        0.0,
        1.35,
        0x5a3a22,
    );
    quadruped(
        commands,
        c + DVec3::new(5.0, 0.0, 10.0),
        0.3,
        0.55,
        0x9a8a70,
    );
    spawn(
        commands,
        cuboid(0.12, 2.0, 0.12),
        dark_wood(),
        at(c + DVec3::new(-22.0, 1.0, -18.0)),
    );
    spawn(
        commands,
        cuboid(1.6, 0.8, 0.08),
        wood(),
        at(c + DVec3::new(-22.0, 1.9, -18.0)),
    );
}

/// Mushroom cave: a mineral spring and glowing mushrooms inside the cave ([`cave_shell`]). The
/// Foliage and ecology thread owns it.
pub fn cave(commands: &mut Commands, z: &Zone, rng: &mut Rng) {
    let c = z.center;
    let (hx, hz, _) = CAVE_INSIDE;
    // A mineral spring the cave plants drink from, faintly glowing.
    let mut pool = StandardMaterial::hex(0x0f3038)
        .with_roughness(0.02)
        .with_emissive([0.1, 0.55, 0.6], 1.5);
    pool.receives_wetness = false;
    spawn(
        commands,
        Shape::Cylinder {
            radius: 3.2,
            height: 0.04,
            segments: 48,
        },
        pool,
        at(c + DVec3::new(0.0, 0.03, 4.0)),
    );
    let caps = [
        StandardMaterial::hex(0x1a3a40)
            .with_emissive([0.2, 1.0, 0.9], 160.0)
            .with_subsurface(0.5),
        StandardMaterial::hex(0x2a2040)
            .with_emissive([0.6, 0.3, 1.0], 160.0)
            .with_subsurface(0.5),
    ];
    let stem = mat(0xd8d0c0, 0.6).with_subsurface(0.6);
    for i in 0..56 {
        // Clustered along the walls (so their light shows the cave's shape) and around the
        // spring.
        let base = if i % 2 == 0 {
            let along = rng.range(-1.0, 1.0) as f64;
            let off = rng.range(0.6, 2.5) as f64;
            match i % 6 {
                0 => c + DVec3::new(-hx + off, 0.0, along * hz),
                2 => c + DVec3::new(hx - off, 0.0, along * hz),
                _ => c + DVec3::new(along * hx, 0.0, hz - off),
            }
        } else {
            let a = rng.range(0.0, TAU);
            let r = rng.range(4.5, 9.0);
            c + DVec3::new((a.cos() * r) as f64, 0.0, (a.sin() * r) as f64 + 4.0)
        };
        let s = rng.range(0.4, 1.4);
        spawn(
            commands,
            Shape::Cylinder {
                radius: 0.08 * s,
                height: 0.6 * s,
                segments: 8,
            },
            stem,
            at(base + DVec3::new(0.0, 0.3 * s as f64, 0.0)),
        );
        spawn(
            commands,
            Shape::Sphere {
                radius: 0.35 * s,
                segments: 16,
            },
            caps[i % 2],
            Transform::from_translation(base + DVec3::new(0.0, 0.62 * s as f64, 0.0))
                .with_scale(Vec3::new(1.0, 0.45, 1.0)),
        );
        if i % 3 == 0 {
            commands.spawn((
                at(base + DVec3::new(0.0, 0.9, 0.0)),
                PointLight {
                    color: if i % 2 == 0 {
                        [0.3, 1.0, 0.9]
                    } else {
                        [0.7, 0.4, 1.0]
                    },
                    intensity_lm: 1600.0,
                    range: 16.0,
                    radius: 0.2,
                },
            ));
        }
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
        Vec3::ONE,
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
        b.quad(q, Vec3::ONE);
    };
    for i in 0..nx {
        skirt((i, 0), (i + 1, 0), Vec3::NEG_Z);
        skirt((i, nz), (i + 1, nz), Vec3::Z);
    }
    for j in 0..nz {
        skirt((0, j), (0, j + 1), Vec3::NEG_X);
        skirt((nx, j), (nx, j + 1), Vec3::X);
    }
    commands.spawn((at(c), MeshRenderer::new(meshes.add(b.finish()), rock())));

    // Grass on the roof like on the rest of the hill.
    let mut x = x0 + 1.5;
    while x < x1 - 1.0 {
        let mut z = z0 + 1.5;
        while z < z1 - 1.0 {
            let (wx, wz) = (
                c.x + x + rng.range(-1.2, 1.2) as f64,
                c.z + z + rng.range(-1.2, 1.2) as f64,
            );
            let mesh = lib.grass[rng.range_i32(0, lib.grass.len() as i32) as usize];
            commands.spawn((
                Transform::from_translation(DVec3::new(wx, land.cave_roof_height(wx, wz), wz))
                    .with_rotation(Quat::from_rotation_y(rng.range(0.0, TAU)))
                    .with_scale(Vec3::new(1.0, rng.range(0.8, 1.3), 1.0)),
                MeshRenderer::new(mesh, lib.mats.grass).without_shadows(),
                scenery::Lod::hide_beyond(mesh, rng.range(45.0, 95.0)),
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
        for i in 0..n as u32 - 1 {
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
        if let (Some((c, _)), Some((_, bottom))) = (st.pool, st.falls()) {
            let mut froth = StandardMaterial::hex(0xc8d4d8)
                .with_roughness(0.4)
                .with_alpha(0.75);
            froth.receives_wetness = false;
            spawn(
                commands,
                Shape::Cylinder {
                    radius: 5.0,
                    height: 0.02,
                    segments: 40,
                },
                froth,
                at(DVec3::new(c.x, level(bottom) + 0.02, c.y)),
            );
        }
    }
}

/// Combat arena: sand floor ringed by pillars, cover, a ramp and a wall-run wall; fighters. The
/// Smart NPC AI and Movement and slow motion threads own it.
pub fn arena(commands: &mut Commands, _lib: &Library, z: &Zone) {
    let c = z.center;
    spawn(
        commands,
        cyl(40.0, 0.3),
        mat(0x9c8662, 0.95),
        at(c + DVec3::new(0.0, 0.15, 0.0)),
    );
    // Stands: stepped stone rings.
    for k in 0..4 {
        let r = 41.5 + k as f64 * 1.6;
        for i in 0..40 {
            let a = i as f64 / 40.0 * TAU64;
            if i % 10 == 0 {
                continue;
            }
            spawn(
                commands,
                cuboid(6.8, 0.8 + k as f32 * 0.8, 1.6),
                stone(),
                yawed(
                    c + DVec3::new(a.cos() * r, (0.8 + k as f64 * 0.8) * 0.5, a.sin() * r),
                    -(a as f32) + std::f32::consts::FRAC_PI_2,
                ),
            );
        }
    }
    for i in 0..8 {
        let a = i as f64 / 8.0 * TAU64;
        spawn(
            commands,
            cyl(1.2, 8.0),
            concrete(),
            at(c + DVec3::new(a.cos() * 18.0, 4.0, a.sin() * 18.0)),
        );
    }
    for (x, zz) in [(-8.0, 4.0), (6.0, -6.0), (0.0, 10.0)] {
        spawn(
            commands,
            cuboid(4.0, 1.2, 1.0),
            concrete(),
            at(c + DVec3::new(x, 0.6, zz)),
        );
    }
    spawn(
        commands,
        cuboid(4.0, 0.5, 10.0),
        concrete(),
        Transform::from_translation(c + DVec3::new(12.0, 1.6, 8.0))
            .with_rotation(Quat::from_rotation_x(0.35)),
    );
    spawn(
        commands,
        cuboid(0.8, 6.0, 24.0),
        concrete(),
        at(c + DVec3::new(-24.0, 3.0, 0.0)),
    );
    for (i, (x, zz)) in [(-3.0, -2.0), (4.0, 3.0), (2.0, -8.0)]
        .into_iter()
        .enumerate()
    {
        person(
            commands,
            c + DVec3::new(x, 0.3, zz),
            i as f32 * 2.0,
            0xa83232,
        );
    }
    person(commands, c + DVec3::new(0.0, 0.3, 0.0), 0.0, 0x3260a8);
}

/// Movement course: wall-run lanes, slide tunnels, platforms and gaps. The Movement and slow
/// motion thread owns it.
pub fn parkour(commands: &mut Commands, _lib: &Library, z: &Zone) {
    let c = z.center;
    let panel = mat(0xd0ccc4, 0.7);
    let accent = mat(0xe67e22, 0.5);
    for (i, x) in [-14.0, -6.0].into_iter().enumerate() {
        spawn(
            commands,
            cuboid(0.6, 7.0, 30.0),
            panel,
            at(c + DVec3::new(x, 3.5 + i as f64, -4.0)),
        );
        spawn(
            commands,
            cuboid(0.62, 0.3, 30.0),
            accent,
            at(c + DVec3::new(x, 6.5 + i as f64, -4.0)),
        );
    }
    for (k, (x, zz, h)) in [
        (4.0, -14.0, 2.0),
        (8.0, -6.0, 3.5),
        (13.0, 2.0, 5.0),
        (8.0, 10.0, 6.5),
        (2.0, 16.0, 8.0),
    ]
    .into_iter()
    .enumerate()
    {
        spawn(
            commands,
            cuboid(4.0, h as f32, 4.0),
            panel,
            at(c + DVec3::new(x, h * 0.5, zz)),
        );
        spawn(
            commands,
            cuboid(4.02, 0.2, 4.02),
            if k % 2 == 0 { accent } else { panel },
            at(c + DVec3::new(x, h, zz)),
        );
    }
    // Slide tunnel.
    spawn(
        commands,
        cuboid(4.0, 0.4, 10.0),
        panel,
        at(c + DVec3::new(-20.0, 1.2, 14.0)),
    );
    for s in [-1.8, 1.8] {
        spawn(
            commands,
            cuboid(0.4, 1.0, 10.0),
            panel,
            at(c + DVec3::new(-20.0 + s, 0.5, 14.0)),
        );
    }
    // Grapple/zip line tower.
    spawn(
        commands,
        cuboid(2.0, 14.0, 2.0),
        panel,
        at(c + DVec3::new(20.0, 7.0, -20.0)),
    );
}

/// Siege fortress on its hill: brick wall (breakable), towers, wooden barricades. The
/// Destructible terrain and buildings thread owns it.
pub fn fortress(commands: &mut Commands, _lib: &Library, z: &Zone) {
    let c = z.center;
    let brick = mat(0x9c8670, 0.9);
    for row in 0..8 {
        for i in 0..24 {
            let x = (i as f64 - 12.0) * 2.0 + if row % 2 == 0 { 0.0 } else { 1.0 };
            spawn(
                commands,
                cuboid(1.96, 0.98, 1.0),
                brick,
                at(c + DVec3::new(x, 0.5 + row as f64, 0.0)),
            );
        }
    }
    // Crenellations.
    for i in 0..12 {
        spawn(
            commands,
            cuboid(1.6, 0.9, 1.1),
            brick,
            at(c + DVec3::new((i as f64 - 5.5) * 4.0, 8.45, 0.0)),
        );
    }
    let keep = mat(0x8a7864, 0.9);
    for x in [-26.0, 26.0] {
        spawn(
            commands,
            cyl(4.0, 14.0),
            keep,
            at(c + DVec3::new(x, 7.0, 0.0)),
        );
        spawn(
            commands,
            Shape::Cone {
                radius: 4.6,
                height: 5.0,
                segments: 32,
            },
            mat(0x5e2f22, 0.8),
            at(c + DVec3::new(x, 16.5, 0.0)),
        );
    }
    // Side walls and a keep behind.
    for s in [-1.0, 1.0] {
        spawn(
            commands,
            cuboid(2.0, 9.0, 34.0),
            keep,
            at(c + DVec3::new(s * 26.0, 4.5, 18.0)),
        );
    }
    spawn(
        commands,
        cuboid(54.0, 9.0, 2.0),
        keep,
        at(c + DVec3::new(0.0, 4.5, 35.0)),
    );
    spawn(
        commands,
        cuboid(14.0, 18.0, 12.0),
        keep,
        at(c + DVec3::new(0.0, 9.0, 22.0)),
    );
    spawn(
        commands,
        Shape::Cone {
            radius: 10.0,
            height: 7.0,
            segments: 4,
        },
        mat(0x4a2a20, 0.8),
        Transform::from_translation(c + DVec3::new(0.0, 21.5, 22.0))
            .with_rotation(Quat::from_rotation_y(std::f32::consts::FRAC_PI_4)),
    );
    for i in 0..4 {
        spawn(
            commands,
            cuboid(4.0, 2.0, 0.3),
            wood(),
            at(c + DVec3::new(-12.0 + i as f64 * 8.0, 1.0, -10.0)),
        );
    }
}

/// Voxel hill: a mound of 1 m blocks (grass on dirt on stone) with a dug pit and a block house
/// (Valheim-style digging and building). The Destructible terrain and buildings and Planets and
/// terrain threads own it.
pub fn voxel(commands: &mut Commands, meshes: &mut Assets<Mesh>, z: &Zone, rng: &mut Rng) {
    use engine::core::math::noise::fbm2;
    let mut b = crate::meshgen::Builder::default();
    let grass = Vec3::new(0.08, 0.17, 0.04);
    let dirt = Vec3::new(0.18, 0.12, 0.07);
    let stone_c = Vec3::new(0.22, 0.21, 0.2);
    let n = 36i32;
    let height = |i: i32, j: i32| -> i32 {
        let (x, y) = (i as f64 - n as f64 * 0.5, j as f64 - n as f64 * 0.5);
        let r = (x * x + y * y).sqrt() / (n as f64 * 0.5);
        let mound = (1.0 - r).max(0.0) * 9.0 + fbm2(x / 6.0, y / 6.0, 3, 3, 2.0, 0.5) as f64 * 2.0;
        // The dug pit.
        let pit = ((x - 4.0).powi(2) + (y + 3.0).powi(2)).sqrt() < 4.0;
        if pit {
            (mound as i32 - 4).max(0)
        } else {
            mound.max(0.0) as i32
        }
    };
    for i in 0..n {
        for j in 0..n {
            let top = height(i, j);
            for y in 0..top {
                let c = if y == top - 1 {
                    grass
                } else if y > top - 4 {
                    dirt
                } else {
                    stone_c
                } * (0.9 + 0.2 * rng.f32());
                // Only blocks with an exposed side need faces; boxes are cheap enough here.
                let exposed = y == top - 1
                    || height(i - 1, j) <= y
                    || height(i + 1, j) <= y
                    || height(i, j - 1) <= y
                    || height(i, j + 1) <= y;
                if exposed {
                    let p = Vec3::new(
                        i as f32 - n as f32 * 0.5,
                        y as f32 + 0.5,
                        j as f32 - n as f32 * 0.5,
                    );
                    b.aabb(p - Vec3::splat(0.5), p + Vec3::splat(0.5), c);
                }
            }
        }
    }
    commands.spawn((
        at(z.center),
        MeshRenderer::new(meshes.add(b.finish()), mat(0xffffff, 0.9)),
    ));
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
            let mesh = lib.dry_grass[rng.range_i32(0, lib.dry_grass.len() as i32) as usize];
            commands.spawn((
                Transform::from_translation(c + DVec3::new(x, -0.02, zz))
                    .with_rotation(Quat::from_rotation_y(rng.range(0.0, TAU)))
                    .with_scale(Vec3::new(1.0, rng.range(1.3, 1.9), 1.0)),
                MeshRenderer::new(mesh, lib.mats.grass).without_shadows(),
                scenery::Lod::hide_beyond(mesh, 140.0),
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

/// Smoke range: firing line, targets, a fan bank and a smoke chamber. The Fire and smoke thread
/// owns it.
pub fn range(commands: &mut Commands, _lib: &Library, z: &Zone) {
    let c = z.center;
    spawn(
        commands,
        cuboid(40.0, 0.12, 50.0),
        concrete(),
        at(c + DVec3::new(0.0, 0.06, 0.0)),
    );
    spawn(
        commands,
        cuboid(30.0, 1.0, 1.0),
        stone(),
        at(c + DVec3::new(0.0, 0.5, -20.0)),
    );
    for i in 0..5 {
        let x = -12.0 + i as f64 * 6.0;
        spawn(
            commands,
            cuboid(0.1, 1.6, 0.1),
            dark_wood(),
            at(c + DVec3::new(x, 0.8, 18.0)),
        );
        spawn(
            commands,
            cyl(0.6, 0.05),
            mat(0xeeeeee, 0.8),
            Transform::from_translation(c + DVec3::new(x, 1.8, 18.0))
                .with_rotation(Quat::from_rotation_x(std::f32::consts::FRAC_PI_2)),
        );
        spawn(
            commands,
            cyl(0.3, 0.06),
            mat(0xc0392b, 0.8),
            Transform::from_translation(c + DVec3::new(x, 1.8, 17.98))
                .with_rotation(Quat::from_rotation_x(std::f32::consts::FRAC_PI_2)),
        );
    }
    // Back wall and fans that blow across the lanes.
    spawn(
        commands,
        cuboid(40.0, 6.0, 1.0),
        concrete(),
        at(c + DVec3::new(0.0, 3.0, 25.0)),
    );
    for zz in [-8.0, 0.0, 8.0] {
        spawn(
            commands,
            cyl(1.4, 0.4),
            metal(),
            Transform::from_translation(c + DVec3::new(-19.0, 2.0, zz))
                .with_rotation(Quat::from_rotation_z(std::f32::consts::FRAC_PI_2)),
        );
        spawn(
            commands,
            cuboid(0.4, 2.0, 0.4),
            metal(),
            at(c + DVec3::new(-19.0, 1.0, zz)),
        );
    }
}

/// Weather station on its hill: mast with anemometer and wind sock, instrument screen, rain
/// gauge. The Weather, wind and clouds thread owns it.
pub fn weather(commands: &mut Commands, _lib: &Library, z: &Zone) {
    let c = z.center;
    spawn(
        commands,
        cyl(0.1, 12.0),
        metal(),
        at(c + DVec3::new(0.0, 6.0, 0.0)),
    );
    for k in 0..3 {
        let a = k as f32 / 3.0 * TAU;
        spawn(
            commands,
            cuboid(0.9, 0.04, 0.04),
            metal(),
            yawed(
                c + DVec3::new((a.cos() * 0.45) as f64, 12.1, (a.sin() * 0.45) as f64),
                -a,
            ),
        );
        spawn(
            commands,
            Shape::Sphere {
                radius: 0.12,
                segments: 10,
            },
            metal(),
            at(c + DVec3::new((a.cos() * 0.9) as f64, 12.1, (a.sin() * 0.9) as f64)),
        );
    }
    spawn(
        commands,
        Shape::Cone {
            radius: 0.3,
            height: 1.6,
            segments: 16,
        },
        mat(0xff6a00, 0.7).with_subsurface(0.4),
        Transform::from_translation(c + DVec3::new(0.9, 9.0, 0.0))
            .with_rotation(Quat::from_rotation_z(-std::f32::consts::FRAC_PI_2)),
    );
    spawn(
        commands,
        cuboid(1.0, 0.8, 0.8),
        mat(0xf4f4f0, 0.6),
        at(c + DVec3::new(4.0, 1.4, 2.0)),
    );
    for (x, zz) in [(-0.4, -0.3), (0.4, -0.3), (-0.4, 0.3), (0.4, 0.3)] {
        spawn(
            commands,
            cuboid(0.06, 1.0, 0.06),
            mat(0xf4f4f0, 0.6),
            at(c + DVec3::new(4.0 + x, 0.5, 2.0 + zz)),
        );
    }
    spawn(
        commands,
        cyl(0.15, 0.6),
        metal(),
        at(c + DVec3::new(-3.0, 0.3, 3.0)),
    );
    spawn(
        commands,
        cuboid(2.0, 0.05, 1.2),
        mat(0x1a2a4a, 0.2).with_metallic(0.5),
        Transform::from_translation(c + DVec3::new(-4.0, 1.4, -3.0))
            .with_rotation(Quat::from_rotation_x(-0.6)),
    );
}

/// Vehicle track: asphalt oval with a jump and a car. The Cars, planes and spaceships thread
/// owns it.
pub fn track(commands: &mut Commands, _lib: &Library, z: &Zone) {
    let segs = 64;
    let (rx, rz) = (z.radius, z.radius * 0.55);
    let asphalt = mat(0x2c2c2e, 0.75);
    let kerb = [mat(0xc0392b, 0.6), mat(0xeeeeee, 0.6)];
    for i in 0..segs {
        let a0 = i as f64 / segs as f64 * TAU64;
        let a1 = (i + 1) as f64 / segs as f64 * TAU64;
        let p0 = DVec3::new(a0.cos() * rx, 0.0, a0.sin() * rz);
        let p1 = DVec3::new(a1.cos() * rx, 0.0, a1.sin() * rz);
        let mid = (p0 + p1) * 0.5;
        let dir = (p1 - p0).as_vec3();
        let len = dir.length();
        let yaw = (-dir.z).atan2(dir.x);
        spawn(
            commands,
            cuboid(len + 0.6, 0.05, 12.0),
            asphalt,
            yawed(z.center + mid + DVec3::new(0.0, 0.03, 0.0), yaw),
        );
        let side = Quat::from_rotation_y(yaw) * Vec3::Z;
        for s in [-6.3, 6.3] {
            spawn(
                commands,
                cuboid(len + 0.1, 0.1, 0.6),
                kerb[i % 2],
                yawed(
                    z.center + mid + (side * s).as_dvec3() + DVec3::new(0.0, 0.05, 0.0),
                    yaw,
                ),
            );
        }
    }
    spawn(
        commands,
        cuboid(10.0, 0.5, 12.0),
        concrete(),
        Transform::from_translation(z.center + DVec3::new(0.0, 1.2, rz))
            .with_rotation(Quat::from_rotation_z(0.25)),
    );
    let body = StandardMaterial::hex(0xb02020)
        .with_metallic(0.6)
        .with_roughness(0.25);
    let car = z.center + DVec3::new(rx, 0.0, 0.0);
    spawn(
        commands,
        cuboid(1.9, 0.7, 4.4),
        body,
        at(car + DVec3::new(0.0, 0.75, 0.0)),
    );
    spawn(
        commands,
        cuboid(1.7, 0.55, 2.2),
        body,
        at(car + DVec3::new(0.0, 1.35, -0.3)),
    );
    for (x, zz) in [(-0.95, 1.4), (0.95, 1.4), (-0.95, -1.4), (0.95, -1.4)] {
        spawn(
            commands,
            Shape::Cylinder {
                radius: 0.36,
                height: 0.25,
                segments: 20,
            },
            mat(0x151515, 0.9),
            Transform::from_translation(car + DVec3::new(x, 0.36, zz))
                .with_rotation(Quat::from_rotation_z(std::f32::consts::FRAC_PI_2)),
        );
    }
    // Grandstand.
    for k in 0..6 {
        spawn(
            commands,
            cuboid(60.0, 0.6, 1.4),
            concrete(),
            at(z.center + DVec3::new(0.0, 0.3 + k as f64 * 0.6, rz + 14.0 + k as f64 * 1.4)),
        );
    }
}

/// Airfield: a 1.2 km runway with markings, a control tower, a hangar and a plane. The Cars,
/// planes and spaceships thread owns it.
pub fn airfield(commands: &mut Commands, _lib: &Library, z: &Zone) {
    let (a, b) = RUNWAY;
    let len = (b[1] - a[1]) as f32;
    let mid = DVec3::new(a[0], 0.04, (a[1] + b[1]) * 0.5);
    spawn(
        commands,
        cuboid(45.0, 0.08, len),
        mat(0x303032, 0.8),
        at(mid),
    );
    let white = mat(0xe8e8e8, 0.6);
    let n = (len / 30.0) as i32;
    for i in 0..n {
        spawn(
            commands,
            cuboid(0.9, 0.1, 15.0),
            white,
            at(DVec3::new(a[0], 0.06, a[1] + 15.0 + i as f64 * 30.0)),
        );
    }
    for s in [-21.0, 21.0] {
        spawn(
            commands,
            cuboid(0.6, 0.1, len),
            white,
            at(DVec3::new(a[0] + s, 0.06, mid.z)),
        );
    }
    let c = z.center + DVec3::new(-90.0, 0.0, 0.0);
    spawn(
        commands,
        cuboid(60.0, 0.1, 120.0),
        concrete(),
        at(c + DVec3::new(15.0, 0.03, 0.0)),
    );
    // Control tower.
    spawn(
        commands,
        cyl(3.0, 22.0),
        concrete(),
        at(c + DVec3::new(-10.0, 11.0, -50.0)),
    );
    let mut glass = StandardMaterial::hex(0x2a4050)
        .with_roughness(0.05)
        .with_alpha(0.6);
    glass.reflectance = 0.7;
    spawn(
        commands,
        cyl(5.0, 4.0),
        glass,
        at(c + DVec3::new(-10.0, 24.0, -50.0)),
    );
    spawn(
        commands,
        cyl(5.6, 0.6),
        concrete(),
        at(c + DVec3::new(-10.0, 26.3, -50.0)),
    );
    // Hangar.
    spawn(
        commands,
        Shape::Cylinder {
            radius: 16.0,
            height: 50.0,
            segments: 32,
        },
        mat(0x9aa2a8, 0.5).with_metallic(0.6),
        Transform::from_translation(c + DVec3::new(-12.0, 0.0, 20.0))
            .with_rotation(Quat::from_rotation_x(std::f32::consts::FRAC_PI_2)),
    );
    // A plane on the apron.
    let p = c + DVec3::new(20.0, 2.2, 0.0);
    let hull = StandardMaterial::hex(0xe8ecf0)
        .with_metallic(0.3)
        .with_roughness(0.35);
    spawn(
        commands,
        Shape::Capsule {
            radius: 1.3,
            height: 16.0,
            segments: 24,
        },
        hull,
        Transform::from_translation(p)
            .with_rotation(Quat::from_rotation_x(std::f32::consts::FRAC_PI_2)),
    );
    spawn(
        commands,
        cuboid(22.0, 0.25, 3.0),
        hull,
        at(p + DVec3::new(0.0, -0.3, 0.5)),
    );
    spawn(
        commands,
        cuboid(7.0, 0.2, 1.8),
        hull,
        at(p + DVec3::new(0.0, 0.4, 8.0)),
    );
    spawn(
        commands,
        cuboid(0.25, 3.0, 2.2),
        mat(0x2e6fb0, 0.4),
        at(p + DVec3::new(0.0, 1.8, 8.3)),
    );
    for x in [-5.5, 5.5] {
        spawn(
            commands,
            Shape::Cylinder {
                radius: 0.6,
                height: 2.6,
                segments: 16,
            },
            metal(),
            Transform::from_translation(p + DVec3::new(x, -1.0, -0.5))
                .with_rotation(Quat::from_rotation_x(std::f32::consts::FRAC_PI_2)),
        );
    }
}

/// Spaceport: landing pad with lights, a ship and a 600 m launch tower for scale. The Planets
/// and terrain (space) and Cars, planes and spaceships threads own it.
pub fn spaceport(commands: &mut Commands, _lib: &Library, z: &Zone) {
    let c = z.center;
    spawn(
        commands,
        cyl(30.0, 0.4),
        concrete(),
        at(c + DVec3::new(0.0, 0.2, 0.0)),
    );
    spawn(
        commands,
        Shape::Torus {
            radius: 22.0,
            tube: 0.3,
            segments: 96,
        },
        mat(0xf1c40f, 0.5),
        at(c + DVec3::new(0.0, 0.42, 0.0)),
    );
    for i in 0..12 {
        let a = i as f64 / 12.0 * TAU64;
        commands.spawn((
            at(c + DVec3::new(a.cos() * 28.0, 0.6, a.sin() * 28.0)),
            PointLight {
                color: [1.0, 0.5, 0.2],
                intensity_lm: 1500.0,
                range: 12.0,
                radius: 0.1,
            },
        ));
        spawn(
            commands,
            Shape::Sphere {
                radius: 0.2,
                segments: 8,
            },
            StandardMaterial::hex(0x331100).with_emissive([1.0, 0.5, 0.2], 800.0),
            at(c + DVec3::new(a.cos() * 28.0, 0.5, a.sin() * 28.0)),
        );
    }
    let hull = StandardMaterial::hex(0xdfe3e8)
        .with_metallic(0.8)
        .with_roughness(0.35);
    let ship = c + DVec3::new(0.0, 4.0, 0.0);
    let along_z = Quat::from_rotation_x(std::f32::consts::FRAC_PI_2);
    spawn(
        commands,
        Shape::Capsule {
            radius: 2.2,
            height: 16.0,
            segments: 32,
        },
        hull,
        Transform::from_translation(ship).with_rotation(along_z),
    );
    spawn(
        commands,
        Shape::Cone {
            radius: 2.2,
            height: 4.0,
            segments: 32,
        },
        hull,
        Transform::from_translation(ship + DVec3::new(0.0, 0.0, -9.5))
            .with_rotation(Quat::from_rotation_x(-std::f32::consts::FRAC_PI_2)),
    );
    spawn(
        commands,
        cuboid(16.0, 0.4, 5.0),
        hull,
        at(ship + DVec3::new(0.0, -0.5, 3.0)),
    );
    spawn(
        commands,
        Shape::Cylinder {
            radius: 1.4,
            height: 1.0,
            segments: 24,
        },
        StandardMaterial::hex(0x101820).with_emissive([0.4, 0.7, 1.0], 4_000.0),
        Transform::from_translation(ship + DVec3::new(0.0, 0.0, 8.3)).with_rotation(along_z),
    );
    // Hangars along the side.
    for k in 0..3 {
        spawn(
            commands,
            Shape::Cylinder {
                radius: 12.0,
                height: 30.0,
                segments: 32,
            },
            mat(0x9aa2a8, 0.45).with_metallic(0.6),
            Transform::from_translation(c + DVec3::new(-70.0, 0.0, -40.0 + k as f64 * 40.0))
                .with_rotation(Quat::from_rotation_z(std::f32::consts::FRAC_PI_2)),
        );
    }
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
