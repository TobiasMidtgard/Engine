//! Static props for the yard: posts, blocks and decoration.

use super::*;

pub(super) fn mat(hex: u32, roughness: f32) -> StandardMaterial {
    StandardMaterial::hex(hex).with_roughness(roughness)
}

pub(super) fn cuboid(size: Vec3) -> Shape {
    Shape::Cuboid {
        size: size.to_array(),
    }
}

pub(super) fn at(p: DVec3) -> Transform {
    Transform::from_translation(p)
}

/// A static mesh that soft bodies collide with.
pub(super) fn block(
    commands: &mut Commands,
    size: Vec3,
    material: StandardMaterial,
    p: DVec3,
) -> Entity {
    commands
        .spawn((
            at(p),
            MeshRenderer::new(cuboid(size), material),
            RigidBody::fixed(),
            Collider::cuboid(size * 0.5),
        ))
        .id()
}

/// A static mesh soft bodies pass through (decoration).
pub(super) fn prop(
    commands: &mut Commands,
    shape: Shape,
    material: StandardMaterial,
    t: Transform,
) {
    commands.spawn((t, MeshRenderer::new(shape, material)));
}

pub(super) fn post(
    commands: &mut Commands,
    base: DVec3,
    height: f32,
    radius: f32,
    material: StandardMaterial,
) {
    commands.spawn((
        at(base + DVec3::Y * (height * 0.5) as f64),
        MeshRenderer::new(
            Shape::Cylinder {
                radius,
                height,
                segments: 16,
            },
            material,
        ),
        RigidBody::fixed(),
        Collider {
            shape: ColliderShape::Cylinder {
                radius,
                half_height: height * 0.5,
            },
            ..Collider::default()
        },
    ));
}
