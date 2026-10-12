//! The trail walkers' bodies.

use engine_core::math::Quat;
use engine_core::prelude::*;
use engine_render::{MeshRenderer, Shape, StandardMaterial};

use super::{TrailWalker, cuboid, mat};

fn body_part(
    parent: &mut ChildSpawnerCommands,
    shape: Shape,
    material: StandardMaterial,
    t: Transform,
) {
    parent.spawn((t, MeshRenderer::new(shape, material)));
}

pub(super) fn hiker(
    commands: &mut Commands,
    walker: TrailWalker,
    jacket: u32,
    scale: Option<LocalTimeScale>,
) {
    let jacket = mat(jacket, 0.7);
    let skin = mat(0xc99a78, 0.6).with_subsurface(0.5);
    let trousers = mat(0x2f3338, 0.85);
    let mut e = commands.spawn((Name::new("Trail walker"), walker));
    if let Some(s) = scale {
        e.insert(s);
    }
    e.with_children(|p| {
        body_part(
            p,
            Shape::Capsule {
                radius: 0.2,
                height: 0.75,
                segments: 16,
            },
            jacket,
            Transform::from_xyz(0.0, 1.22, 0.0),
        );
        body_part(
            p,
            Shape::Sphere {
                radius: 0.12,
                segments: 16,
            },
            skin,
            Transform::from_xyz(0.0, 1.72, 0.0),
        );
        for x in [-0.1, 0.1] {
            body_part(
                p,
                Shape::Capsule {
                    radius: 0.075,
                    height: 0.9,
                    segments: 10,
                },
                trousers,
                Transform::from_xyz(x, 0.47, 0.0),
            );
        }
        body_part(
            p,
            cuboid(0.3, 0.4, 0.16),
            mat(0x3c4a2e, 0.8),
            Transform::from_xyz(0.0, 1.28, 0.2),
        );
    });
}

pub(super) fn dog(commands: &mut Commands, walker: TrailWalker) {
    let fur = mat(0x8b5a2b, 0.85).with_subsurface(0.2);
    commands
        .spawn((Name::new("Trail dog"), walker))
        .with_children(|p| {
            body_part(
                p,
                Shape::Capsule {
                    radius: 0.14,
                    height: 0.7,
                    segments: 14,
                },
                fur,
                Transform::from_xyz(0.0, 0.42, 0.0)
                    .with_rotation(Quat::from_rotation_x(std::f32::consts::FRAC_PI_2)),
            );
            body_part(
                p,
                Shape::Sphere {
                    radius: 0.11,
                    segments: 14,
                },
                fur,
                Transform::from_xyz(0.0, 0.58, -0.4),
            );
            body_part(
                p,
                cuboid(0.08, 0.07, 0.14),
                mat(0x5c3a1b, 0.8),
                Transform::from_xyz(0.0, 0.55, -0.52),
            );
            for (x, z) in [(-0.08, -0.22), (0.08, -0.22), (-0.08, 0.22), (0.08, 0.22)] {
                body_part(
                    p,
                    Shape::Cylinder {
                        radius: 0.035,
                        height: 0.36,
                        segments: 8,
                    },
                    fur,
                    Transform::from_xyz(x, 0.18, z),
                );
            }
            body_part(
                p,
                Shape::Cylinder {
                    radius: 0.025,
                    height: 0.3,
                    segments: 6,
                },
                fur,
                Transform::from_xyz(0.0, 0.55, 0.42).with_rotation(Quat::from_rotation_x(-0.8)),
            );
        });
}

pub(super) fn horse(commands: &mut Commands, walker: TrailWalker) {
    let coat = mat(0x6b4423, 0.6).with_subsurface(0.15);
    let mane = mat(0x1f1712, 0.9);
    commands
        .spawn((Name::new("Trail horse"), walker))
        .with_children(|p| {
            body_part(
                p,
                Shape::Capsule {
                    radius: 0.38,
                    height: 1.8,
                    segments: 18,
                },
                coat,
                Transform::from_xyz(0.0, 1.3, 0.0)
                    .with_rotation(Quat::from_rotation_x(std::f32::consts::FRAC_PI_2)),
            );
            body_part(
                p,
                cuboid(0.22, 0.75, 0.3),
                coat,
                Transform::from_xyz(0.0, 1.75, -0.8).with_rotation(Quat::from_rotation_x(-0.5)),
            );
            body_part(
                p,
                cuboid(0.2, 0.22, 0.55),
                coat,
                Transform::from_xyz(0.0, 2.05, -1.1).with_rotation(Quat::from_rotation_x(0.35)),
            );
            body_part(
                p,
                cuboid(0.06, 0.6, 0.12),
                mane,
                Transform::from_xyz(0.0, 1.95, -0.7).with_rotation(Quat::from_rotation_x(-0.5)),
            );
            body_part(
                p,
                Shape::Cylinder {
                    radius: 0.05,
                    height: 0.7,
                    segments: 6,
                },
                mane,
                Transform::from_xyz(0.0, 1.2, 0.95).with_rotation(Quat::from_rotation_x(0.4)),
            );
            for (x, z) in [(-0.2, -0.62), (0.2, -0.62), (-0.2, 0.62), (0.2, 0.62)] {
                body_part(
                    p,
                    Shape::Cylinder {
                        radius: 0.07,
                        height: 1.05,
                        segments: 8,
                    },
                    coat,
                    Transform::from_xyz(x, 0.53, z),
                );
                body_part(
                    p,
                    Shape::Cylinder {
                        radius: 0.08,
                        height: 0.08,
                        segments: 8,
                    },
                    mane,
                    Transform::from_xyz(x, 0.04, z),
                );
            }
        });
}
