//! The trail's grounds, props, rails and the underpass.

use engine_core::math::{DVec3, Quat, Vec3};
use engine_core::prelude::*;
use engine_render::{Shape, StandardMaterial};

use super::rings::ring_color;
use super::{
    BEND, HALF_STRAIGHT, Rng, TRAIL_WIDTH, cuboid, mat, put, stretch_start, trail_point, yaw,
};
use crate::ReverbZone;

fn ground_material(name: &str) -> StandardMaterial {
    match name {
        "grass" => mat(0x6e8c3c, 0.95),
        "mud" => mat(0x3f2b1c, 0.22),
        "snow" => mat(0xf1f4f8, 0.85).with_subsurface(0.35),
        "ice" => mat(0xbfe2f0, 0.04).with_subsurface(0.2),
        "gravel" => mat(0x8a847c, 0.95),
        "wood" => mat(0x1d1712, 0.9),
        "metal" => mat(0x18191b, 0.6),
        "concrete" => mat(0xa9a59d, 0.8),
        "sand" => mat(0xd9c08b, 1.0),
        "leaves" => mat(0x4a3420, 0.9),
        "water" => mat(0x4e3c2b, 0.6),
        _ => mat(0x808080, 0.8),
    }
}

/// Lay the ground of one stretch (in pieces round the bends) and dress it with props.
pub(super) fn build_stretch(
    commands: &mut Commands,
    origin: DVec3,
    name: &str,
    s0: f64,
    s1: f64,
    rng: &mut Rng,
) {
    let ground = ground_material(name);
    let mut s = s0;
    // Straights are laid in one piece, bends in pieces of up to a meter.
    let straights = [
        (0.0, 2.0 * HALF_STRAIGHT),
        (2.0 * HALF_STRAIGHT + BEND, 4.0 * HALF_STRAIGHT + BEND),
    ];
    while s < s1 - 1e-6 {
        let len = match straights.iter().find(|(a, b)| s >= *a && s < *b) {
            Some(&(_, b)) => b.min(s1) - s,
            None => (s1 - s).min(1.0),
        };
        let (pm, dm) = trail_point(s + len * 0.5);
        put(
            commands,
            cuboid(TRAIL_WIDTH, 0.06, len as f32 + 0.06),
            ground,
            Transform::from_translation(origin + pm + DVec3::new(0.0, 0.03, 0.0))
                .with_rotation(yaw(dm)),
        );
        s += len;
    }

    // A post at the start of each stretch with a cap in the colour of its sound rings.
    let (p, d) = trail_point(s0);
    let side = Vec3::new(-d.z, 0.0, d.x) * (TRAIL_WIDTH * 0.5 + 0.35);
    let post = origin + p + side.as_dvec3();
    put(
        commands,
        Shape::Cylinder {
            radius: 0.05,
            height: 1.1,
            segments: 10,
        },
        mat(0x5a4632, 0.8),
        Transform::from_translation(post + DVec3::new(0.0, 0.55, 0.0)),
    );
    let c = ring_color(name);
    put(
        commands,
        Shape::Sphere {
            radius: 0.12,
            segments: 16,
        },
        StandardMaterial::color(c[0], c[1], c[2]).with_emissive(c, 2.0),
        Transform::from_translation(post + DVec3::new(0.0, 1.15, 0.0)),
    );

    // Props along the stretch.
    let n = ((s1 - s0) * 1.0) as usize;
    let along = |rng: &mut Rng| s0 + (s1 - s0) * rng.f() as f64;
    let across = |rng: &mut Rng, w: f32| rng.range(-w, w);
    let at = |s: f64, x: f32, y: f64| {
        let (p, d) = trail_point(s);
        let right = Vec3::new(-d.z, 0.0, d.x);
        (
            origin + p + (right * x).as_dvec3() + DVec3::new(0.0, y, 0.0),
            d,
        )
    };
    match name {
        "grass" => {
            let blade = mat(0x5f8f35, 0.9).with_subsurface(0.4).with_wind_sway(0.08);
            for _ in 0..n * 14 {
                let (p, _) = at(along(rng), across(rng, TRAIL_WIDTH * 0.5), 0.12);
                let h = rng.range(0.12, 0.3);
                put(
                    commands,
                    Shape::Cone {
                        radius: 0.025,
                        height: h,
                        segments: 4,
                    },
                    blade,
                    Transform::from_translation(p + DVec3::new(0.0, h as f64 * 0.5 - 0.06, 0.0))
                        .with_rotation(Quat::from_rotation_y(rng.range(0.0, 6.0))),
                );
            }
        }
        "mud" => {
            let puddle = mat(0x2a2016, 0.05);
            for _ in 0..n / 2 {
                let (p, _) = at(along(rng), across(rng, 0.8), 0.062);
                let r = rng.range(0.15, 0.4);
                put(
                    commands,
                    Shape::Cylinder {
                        radius: r,
                        height: 0.01,
                        segments: 20,
                    },
                    puddle,
                    Transform::from_translation(p).with_scale(Vec3::new(
                        1.0,
                        1.0,
                        rng.range(0.5, 1.0),
                    )),
                );
            }
        }
        "snow" => {
            let snow = mat(0xf4f6fa, 0.85).with_subsurface(0.4);
            for _ in 0..n {
                let x = if rng.f() < 0.5 { -1.0 } else { 1.0 }
                    * (TRAIL_WIDTH * 0.5 + rng.range(0.1, 0.6));
                let (p, d) = at(along(rng), x, 0.0);
                let r = rng.range(0.3, 0.7);
                put(
                    commands,
                    Shape::Sphere {
                        radius: r,
                        segments: 16,
                    },
                    snow,
                    Transform::from_translation(p)
                        .with_rotation(yaw(d))
                        .with_scale(Vec3::new(1.0, rng.range(0.3, 0.5), 1.6)),
                );
            }
        }
        "ice" => {
            let crack = mat(0xe8f6fc, 0.3);
            for _ in 0..n {
                let (p, _) = at(along(rng), across(rng, 0.9), 0.061);
                put(
                    commands,
                    cuboid(rng.range(0.4, 1.0), 0.004, 0.015),
                    crack,
                    Transform::from_translation(p)
                        .with_rotation(Quat::from_rotation_y(rng.range(0.0, 3.1))),
                );
            }
        }
        "gravel" => {
            let stones = [mat(0x9a948c, 0.9), mat(0x6e6a65, 0.9), mat(0xb3ada3, 0.9)];
            for _ in 0..n * 40 {
                let (p, _) = at(along(rng), across(rng, TRAIL_WIDTH * 0.5), 0.065);
                let r = rng.range(0.015, 0.045);
                put(
                    commands,
                    Shape::Sphere {
                        radius: r,
                        segments: 6,
                    },
                    stones[(rng.f() * 3.0) as usize % 3],
                    Transform::from_translation(p).with_scale(Vec3::new(
                        1.0,
                        0.6,
                        rng.range(0.7, 1.3),
                    )),
                );
            }
        }
        "wood" => {
            // A boardwalk: planks across the path with gaps over the dark ground, and rails.
            let planks = [
                mat(0x8a6239, 0.75),
                mat(0x7a5530, 0.75),
                mat(0x946c42, 0.75),
            ];
            let mut s = s0 + 0.16;
            let mut i = 0;
            while s < s1 {
                let (p, d) = at(s, 0.0, 0.09);
                put(
                    commands,
                    cuboid(TRAIL_WIDTH - 0.1, 0.05, 0.28),
                    planks[i % 3],
                    Transform::from_translation(p).with_rotation(yaw(d)),
                );
                s += 0.32;
                i += 1;
            }
            rails(commands, origin, s0, s1, mat(0x6b4a2a, 0.8));
        }
        "metal" => {
            // A grate: steel bars across the path over the dark pit.
            let steel = mat(0x9aa0a8, 0.35).with_metallic(1.0);
            let mut s = s0 + 0.06;
            while s < s1 {
                let (p, d) = at(s, 0.0, 0.08);
                put(
                    commands,
                    cuboid(TRAIL_WIDTH - 0.1, 0.04, 0.04),
                    steel,
                    Transform::from_translation(p).with_rotation(yaw(d)),
                );
                s += 0.12;
            }
            for x in [-1.0f32, 1.0] {
                let (p, d) = at((s0 + s1) * 0.5, x * (TRAIL_WIDTH * 0.5 - 0.05), 0.09);
                put(
                    commands,
                    cuboid(0.08, 0.06, (s1 - s0) as f32),
                    steel,
                    Transform::from_translation(p).with_rotation(yaw(d)),
                );
            }
            rails(commands, origin, s0, s1, steel);
        }
        "sand" => {
            let ripple = mat(0xcdb27c, 1.0);
            let mut s = s0 + 0.3;
            while s < s1 {
                let (p, d) = at(s, rng.range(-0.2, 0.2), 0.062);
                put(
                    commands,
                    cuboid(TRAIL_WIDTH * rng.range(0.6, 0.9), 0.012, 0.06),
                    ripple,
                    Transform::from_translation(p)
                        .with_rotation(yaw(d) * Quat::from_rotation_y(rng.range(-0.15, 0.15))),
                );
                s += 0.35;
            }
        }
        "leaves" => {
            let leaf = [
                mat(0xc2581b, 0.7).with_subsurface(0.4),
                mat(0xd99a2b, 0.7).with_subsurface(0.4),
                mat(0x8c2f1a, 0.7).with_subsurface(0.4),
                mat(0xa67c2e, 0.7).with_subsurface(0.4),
            ];
            for _ in 0..n * 45 {
                let (p, _) = at(along(rng), across(rng, TRAIL_WIDTH * 0.5 + 0.3), 0.065);
                put(
                    commands,
                    cuboid(rng.range(0.06, 0.12), 0.006, rng.range(0.04, 0.08)),
                    leaf[(rng.f() * 4.0) as usize % 4],
                    Transform::from_translation(p).with_rotation(
                        Quat::from_rotation_y(rng.range(0.0, 6.3))
                            * Quat::from_rotation_x(rng.range(-0.3, 0.3)),
                    ),
                );
            }
        }
        "water" => {
            let water = mat(0x2b3a44, 0.02).with_metallic(0.1);
            let mut s = s0 + 0.4;
            while s < s1 - 0.3 {
                let (p, d) = at(s, rng.range(-0.5, 0.5), 0.062);
                let r = rng.range(0.4, 0.9);
                put(
                    commands,
                    Shape::Cylinder {
                        radius: r,
                        height: 0.012,
                        segments: 28,
                    },
                    water,
                    Transform::from_translation(p)
                        .with_rotation(yaw(d))
                        .with_scale(Vec3::new(1.0, 1.0, rng.range(1.0, 1.6))),
                );
                s += r as f64 * 1.6;
            }
        }
        _ => {}
    }
}

/// Posts and a handrail on both sides of a stretch.
fn rails(commands: &mut Commands, origin: DVec3, s0: f64, s1: f64, material: StandardMaterial) {
    for x in [-1.0f32, 1.0] {
        let off = x * (TRAIL_WIDTH * 0.5 + 0.05);
        let mut s = s0 + 0.1;
        while s <= s1 {
            let (p, d) = trail_point(s);
            let right = Vec3::new(-d.z, 0.0, d.x) * off;
            put(
                commands,
                cuboid(0.07, 0.95, 0.07),
                material,
                Transform::from_translation(
                    origin + p + right.as_dvec3() + DVec3::new(0.0, 0.48, 0.0),
                ),
            );
            s += 1.5;
        }
        let (p, d) = trail_point((s0 + s1) * 0.5);
        let right = Vec3::new(-d.z, 0.0, d.x) * off;
        put(
            commands,
            cuboid(0.06, 0.05, (s1 - s0) as f32),
            material,
            Transform::from_translation(origin + p + right.as_dvec3() + DVec3::new(0.0, 0.95, 0.0))
                .with_rotation(yaw(d)),
        );
    }
}

/// The concrete stretch runs through an underpass that echoes: a wall on the inside of the
/// loop, pillars on the outside (so the camera can see in) and a slab roof.
pub(super) fn underpass(commands: &mut Commands, origin: DVec3) {
    let Some(s0) = stretch_start("concrete") else {
        return;
    };
    let s1 = s0 + 9.0;
    let (mid, d) = trail_point((s0 + s1) * 0.5);
    let right = Vec3::new(-d.z, 0.0, d.x);
    let concrete = mat(0x9d998f, 0.85);
    let dark = mat(0x6f6b64, 0.9);
    let center = origin + mid;
    let rot = yaw(d);
    let half_w = TRAIL_WIDTH as f64 * 0.5 + 0.9;
    let h: f64 = 3.0;
    // Toward the loop's middle is "inside".
    let inside = if (center - origin).dot(right.as_dvec3()) > 0.0 {
        -1.0
    } else {
        1.0
    };
    put(
        commands,
        cuboid(0.4, h as f32, 9.4),
        concrete,
        Transform::from_translation(
            center + (right * (inside * half_w as f32)).as_dvec3() + DVec3::new(0.0, h * 0.5, 0.0),
        )
        .with_rotation(rot),
    );
    put(
        commands,
        cuboid(2.0 * half_w as f32 + 0.8, 0.45, 9.8),
        dark,
        Transform::from_translation(center + DVec3::new(0.0, h + 0.22, 0.0)).with_rotation(rot),
    );
    for k in 0..4 {
        let s = s0 + 0.3 + k as f64 * (8.4 / 3.0);
        let (p, _) = trail_point(s);
        put(
            commands,
            cuboid(0.45, h as f32, 0.45),
            concrete,
            Transform::from_translation(
                origin
                    + p
                    + (right * (-inside * half_w as f32)).as_dvec3()
                    + DVec3::new(0.0, h * 0.5, 0.0),
            )
            .with_rotation(rot),
        );
    }
    commands.spawn((
        Name::new("Underpass echo"),
        Transform::from_translation(center + DVec3::new(0.0, h * 0.5, 0.0)).with_rotation(rot),
        ReverbZone {
            half_extents: Vec3::new(half_w as f32, h as f32 * 0.5 + 0.3, 4.7),
            room: 0.55,
            damping: 0.15,
            wet: 0.42,
            fade_m: 1.2,
        },
    ));
}
