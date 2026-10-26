//! The scene a new editor session starts with: a small stone courtyard with crates, a barrel,
//! rocks and a lamp, built from the engine's procedural materials. The crates and the ball are
//! physics bodies, so Play shows something happen.

use engine::core::scene::{EntityDoc, SceneDoc};
use engine::prelude::*;
use engine::render::MaterialPreset;
use engine::world::physics::{Collider, ColliderShape, RigidBody};
use serde_json::{Map, Value};

struct Doc(EntityDoc);

impl Doc {
    fn new(name: &str, transform: Transform) -> Self {
        let mut components = Map::new();
        components.insert("Transform".into(), to_value(&transform));
        Self(EntityDoc {
            name: Some(name.into()),
            components,
            children: Vec::new(),
        })
    }

    fn with(mut self, component: &str, value: impl Serialize) -> Self {
        self.0.components.insert(component.into(), to_value(&value));
        self
    }

    fn mesh(self, shape: Shape, material: StandardMaterial) -> Self {
        self.with("MeshRenderer", MeshRenderer::new(shape, material))
    }

    /// A dynamic body with a box collider of `size` meters.
    fn crate_body(self, size: Vec3, density: f32) -> Self {
        self.with("RigidBody", RigidBody::dynamic()).with(
            "Collider",
            Collider::cuboid(size * 0.5).with_density(density),
        )
    }

    fn fixed_box(self, size: Vec3) -> Self {
        self.with("RigidBody", RigidBody::fixed())
            .with("Collider", Collider::cuboid(size * 0.5))
    }

    fn child(mut self, child: Doc) -> Self {
        self.0.children.push(child.0);
        self
    }
}

fn to_value(v: &impl Serialize) -> Value {
    serde_json::to_value(v).unwrap_or(Value::Null)
}

fn at(x: f64, y: f64, z: f64) -> Transform {
    Transform::from_xyz(x, y, z)
}

fn turned(t: Transform, yaw_deg: f32) -> Transform {
    t.with_rotation(Quat::from_rotation_y(yaw_deg.to_radians()))
}

/// The starter courtyard.
pub fn starter_scene() -> SceneDoc {
    let stone = || StandardMaterial::preset(MaterialPreset::Cobblestone);
    let wood = || StandardMaterial::preset(MaterialPreset::WoodPlanks).with_triplanar();
    let brick = || StandardMaterial::preset(MaterialPreset::Brick).with_triplanar_world();
    let crate_size = Vec3::splat(0.9);
    let crate_shape = || Shape::RoundedCuboid {
        size: crate_size.to_array(),
        radius: 0.04,
        segments: 0,
    };
    let wall_size = Vec3::new(14.0, 3.2, 0.7);
    let wall_shape = Shape::RoundedCuboid {
        size: wall_size.to_array(),
        radius: 0.06,
        segments: 0,
    };

    let mut entities = vec![
        Doc::new("Courtyard floor", at(0.0, 0.0, 0.0))
            .mesh(
                Shape::Plane {
                    size: [40.0, 40.0],
                    subdivisions: 1,
                },
                stone(),
            )
            .with("RigidBody", RigidBody::fixed())
            .with("Collider", Collider::cuboid(Vec3::new(20.0, 0.05, 20.0))),
        Doc::new("Back wall", at(0.0, 1.6, -6.0))
            .mesh(wall_shape, brick())
            .fixed_box(wall_size),
        Doc::new("Side wall", turned(at(-7.0, 1.6, 0.5), 90.0))
            .mesh(
                Shape::RoundedCuboid {
                    size: [13.0, 3.2, 0.7],
                    radius: 0.06,
                    segments: 0,
                },
                brick(),
            )
            .fixed_box(Vec3::new(13.0, 3.2, 0.7)),
    ];

    // A crate stack: three on the ground, two on top, one crowning it.
    let mut stack = Doc::new("Crate stack", at(-2.5, 0.0, -3.5));
    let mut n = 0;
    for (row, count) in [(0, 3), (1, 2), (2, 1)] {
        for i in 0..count {
            n += 1;
            let x = (i as f64 - (count - 1) as f64 * 0.5) * 0.95;
            let y = 0.45 + row as f64 * 0.9;
            stack = stack.child(
                Doc::new(
                    &format!("Crate {n}"),
                    turned(at(x, y, 0.0), (n as f32 * 7.0) % 9.0),
                )
                .mesh(crate_shape(), wood())
                .crate_body(crate_size, 450.0),
            );
        }
    }
    entities.push(stack);

    entities.extend([
        Doc::new("Barrel", at(1.8, 0.6, -4.2))
            .mesh(
                Shape::Cylinder {
                    radius: 0.42,
                    height: 1.2,
                    segments: 0,
                },
                StandardMaterial::preset(MaterialPreset::RustySteel).with_triplanar(),
            )
            .with("RigidBody", RigidBody::dynamic())
            .with(
                "Collider",
                Collider {
                    shape: ColliderShape::Cylinder {
                        radius: 0.42,
                        half_height: 0.6,
                    },
                    density: 300.0,
                    ..Default::default()
                },
            ),
        Doc::new(
            "Ramp",
            turned(at(4.0, 0.55, -1.5), -90.0).with_rotation(
                Quat::from_rotation_y(-90f32.to_radians()) * Quat::from_rotation_x(-0.22),
            ),
        )
        .mesh(
            Shape::RoundedCuboid {
                size: [1.6, 0.2, 5.0],
                radius: 0.03,
                segments: 0,
            },
            wood(),
        )
        .fixed_box(Vec3::new(1.6, 0.2, 5.0)),
        Doc::new("Marble ball", at(6.2, 1.45, -1.5))
            .mesh(
                Shape::Sphere {
                    radius: 0.35,
                    segments: 0,
                },
                StandardMaterial::preset(MaterialPreset::Marble),
            )
            .with("RigidBody", RigidBody::dynamic())
            .with("Collider", Collider::sphere(0.35).with_density(2700.0)),
        Doc::new("Boulder", turned(at(-4.5, 0.0, 2.5), 30.0)).mesh(
            Shape::Rock {
                size: [2.2, 1.4, 1.8],
                seed: 11,
                detail: 0,
            },
            StandardMaterial::preset(MaterialPreset::Rock),
        ),
        Doc::new("Stone", turned(at(-2.8, 0.0, 3.6), 75.0)).mesh(
            Shape::Rock {
                size: [0.9, 0.6, 0.8],
                seed: 4,
                detail: 0,
            },
            StandardMaterial::preset(MaterialPreset::Rock),
        ),
        Doc::new("Lamp post", at(2.5, 0.0, 1.5))
            .mesh(
                Shape::Cylinder {
                    radius: 0.07,
                    height: 3.0,
                    segments: 0,
                },
                StandardMaterial::preset(MaterialPreset::PaintedMetal),
            )
            .child(
                Doc::new("Lamp", at(0.0, 1.6, 0.0))
                    .mesh(
                        Shape::Sphere {
                            radius: 0.18,
                            segments: 0,
                        },
                        StandardMaterial::hex(0xffe2b0).with_emissive([1.0, 0.8, 0.55], 2000.0),
                    )
                    .with(
                        "PointLight",
                        PointLight {
                            intensity_lm: 1800.0,
                            range: 14.0,
                            ..Default::default()
                        },
                    ),
            ),
    ]);
    // Nudge the lamp post's mesh up so it stands on the floor (cylinders are centered).
    if let Some(post) = entities
        .iter_mut()
        .find(|d| d.0.name.as_deref() == Some("Lamp post"))
    {
        post.0
            .components
            .insert("Transform".into(), to_value(&at(2.5, 1.5, 1.5)));
    }
    SceneDoc {
        entities: entities.into_iter().map(|d| d.0).collect(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn starter_scene_spawns_through_the_registry() {
        let mut app = App::new();
        app.add_plugins(engine::world::WorldPlugin);
        app.register_component::<MeshRenderer>("mesh")
            .register_component::<PointLight>("lamp");
        app.update();
        let scene = starter_scene();
        let roots = engine::core::scene::spawn_scene(&mut app.world, &scene, None);
        assert!(roots.is_ok(), "{:?}", roots.err());
        let json = scene.to_json_pretty();
        assert!(SceneDoc::from_json(&json).is_ok());
    }
}
