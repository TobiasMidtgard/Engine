//! The Weaving hall's stonework, practice dummies, crates and barrels, made from the renderer's
//! shapes and generated materials (nothing to download).
//!
//! Layout, in meters from the zone's center (+Z north): six casting plinths on a marble dais
//! along z = +10, one lane each 6 m apart, practice dummies 21 m downrange at z = −11, a rock
//! wall behind them to catch what misses, low walls along the sides and four torch pillars.

use engine_core::prelude::*;
use engine_core::registry::call_api;
use engine_render::{MaterialPreset, MaterialRef, MeshRef, MeshRenderer, Shape, StandardMaterial};
use engine_world::Readouts;
use engine_world::animation::BodyRequest;
use engine_world::physics::{BodyKind, Collider, ColliderShape, RigidBody};
use serde_json::json;

use crate::caster::{MagicCaster, Vitality};
use crate::schools::Mastery;

/// Lanes' x, west to east.
pub const LANES: [f64; 6] = [-15.0, -9.0, -3.0, 3.0, 9.0, 15.0];
/// The mages' row.
pub const MAGE_Z: f64 = 10.0;
/// Top of the plinths the mages stand on.
pub const PLINTH_TOP: f64 = 0.85;
/// The far dummies' row.
pub const DUMMY_Z: f64 = -11.0;
/// Top of the terrace floor.
pub const FLOOR: f64 = 0.2;

/// A mage of the hall, by lane.
#[derive(Component, Debug, Clone, Copy)]
pub struct HallMage {
    pub lane: usize,
}

/// Something a lane aims at: a practice dummy, crate or barrel.
#[derive(Component, Debug, Clone, Copy)]
pub struct HallTarget {
    pub lane: usize,
    /// The lane's main target (the far dummy, or a program's focus).
    pub focus: bool,
    pub home: DVec3,
}

/// Made for one program; goes when the program changes.
#[derive(Component, Debug, Clone, Copy)]
pub struct ProgramProp;

/// A stand-in body drawn until the animation module gives the mage a real one.
#[derive(Component, Debug, Clone, Copy)]
pub struct StandIn {
    pub owner: Entity,
}

fn mat(preset: MaterialPreset) -> StandardMaterial {
    StandardMaterial::preset(preset).with_triplanar_world()
}

fn part(shape: Shape, material: StandardMaterial) -> MeshRenderer {
    MeshRenderer::new(MeshRef::Shape(shape), MaterialRef::Inline(material))
}

fn solid(
    commands: &mut Commands,
    name: &str,
    at: DVec3,
    size: [f32; 3],
    material: StandardMaterial,
) {
    commands.spawn((
        Name::new(name.to_string()),
        Transform::from_translation(at),
        part(
            Shape::RoundedCuboid {
                size,
                radius: 0.04,
                segments: 0,
            },
            material,
        ),
        Collider {
            shape: ColliderShape::Cuboid {
                half_extents: Vec3::from_array(size) * 0.5,
            },
            ..Default::default()
        },
    ));
}

/// Robes by tier: plain wool for the novice to deep ascended violet.
pub fn robe_color(m: Mastery) -> [f32; 3] {
    match m {
        Mastery::Novice => [0.42, 0.36, 0.28],
        Mastery::Apprentice => [0.22, 0.3, 0.42],
        Mastery::Adept => [0.16, 0.32, 0.2],
        Mastery::Expert => [0.45, 0.12, 0.1],
        Mastery::Master => [0.12, 0.1, 0.32],
        _ => [0.32, 0.08, 0.42],
    }
}

/// The hall's stonework.
pub fn spawn_terrace(commands: &mut Commands, origin: DVec3) {
    let o = |x: f64, y: f64, z: f64| origin + DVec3::new(x, y, z);
    let floor = mat(MaterialPreset::Cobblestone).with_tint(0.85, 0.82, 0.78);
    solid(
        commands,
        "Weaving hall floor",
        o(0.0, 0.0, -5.0),
        [44.0, 0.4, 46.0],
        floor,
    );
    let marble = mat(MaterialPreset::Marble);
    solid(
        commands,
        "Weaving hall dais",
        o(0.0, 0.375, MAGE_Z),
        [40.0, 0.35, 5.0],
        marble,
    );
    // Steps down from the dais.
    solid(
        commands,
        "Weaving hall step",
        o(0.0, 0.29, MAGE_Z - 2.9),
        [40.0, 0.18, 0.8],
        marble,
    );
    let rock = mat(MaterialPreset::Rock).with_tint(0.72, 0.7, 0.68);
    for (i, x) in LANES.iter().enumerate() {
        commands.spawn((
            Name::new(format!("Casting plinth {}", i + 1)),
            Transform::from_translation(o(*x, 0.7, MAGE_Z)),
            part(
                Shape::Cylinder {
                    radius: 0.85,
                    height: 0.3,
                    segments: 32,
                },
                rock,
            ),
        ));
        // A bronze-ringed rune disc set in the floor where each lane's targets stand.
        commands.spawn((
            Name::new(format!("Target ring {}", i + 1)),
            Transform::from_translation(o(*x, FLOOR + 0.005, DUMMY_Z)),
            part(
                Shape::Torus {
                    radius: 1.3,
                    tube: 0.05,
                    segments: 48,
                },
                StandardMaterial::default()
                    .with_tint(0.55, 0.38, 0.18)
                    .with_metallic(1.0)
                    .with_roughness(0.35),
            ),
        ));
    }
    // Dark stone lines between the lanes.
    let dark = mat(MaterialPreset::Rock).with_tint(0.25, 0.24, 0.24);
    for x in [-12.0, -6.0, 0.0, 6.0, 12.0] {
        commands.spawn((
            Name::new("Lane line"),
            Transform::from_translation(o(x, FLOOR + 0.02, -6.5)),
            part(
                Shape::RoundedCuboid {
                    size: [0.22, 0.05, 25.0],
                    radius: 0.02,
                    segments: 0,
                },
                dark,
            ),
        ));
    }
    // A rock wall behind the targets, low walls along the sides.
    solid(
        commands,
        "Weaving hall back wall",
        o(0.0, 1.7, -25.5),
        [44.0, 3.4, 1.4],
        rock,
    );
    for x in [-22.5, 22.5] {
        solid(
            commands,
            "Weaving hall side wall",
            o(x, 0.9, -5.0),
            [1.0, 1.4, 46.0],
            rock,
        );
    }
    // Torch pillars at the dais' corners and the wall's.
    let pillars = [
        (-21.0, MAGE_Z + 1.5),
        (21.0, MAGE_Z + 1.5),
        (-21.0, -24.0),
        (21.0, -24.0),
    ];
    for (x, z) in pillars {
        commands.spawn((
            Name::new("Torch pillar"),
            Transform::from_translation(o(x, FLOOR + 1.3, z)),
            part(
                Shape::Cylinder {
                    radius: 0.32,
                    height: 2.6,
                    segments: 24,
                },
                marble,
            ),
        ));
        commands.spawn((
            Name::new("Torch bowl"),
            Transform::from_translation(o(x, FLOOR + 2.7, z)),
            part(
                Shape::Cylinder {
                    radius: 0.45,
                    height: 0.25,
                    segments: 24,
                },
                StandardMaterial::default()
                    .with_tint(0.18, 0.16, 0.15)
                    .with_metallic(1.0)
                    .with_roughness(0.55),
            ),
        ));
    }
    commands.queue(move |world: &mut World| {
        for (x, z) in pillars {
            let at = origin + DVec3::new(x, FLOOR + 2.85, z);
            let _ = call_api(
                world,
                "vfx.spawn",
                json!({"effect": "fire.torch", "position": at.to_array(), "scale": 1.3, "once": false}),
            );
        }
    });
}

/// A straw practice dummy on a post: takes spells, burns, and stands up again.
pub fn spawn_dummy(
    commands: &mut Commands,
    at: DVec3,
    lane: usize,
    focus: bool,
    dynamic: bool,
) -> Entity {
    let straw = StandardMaterial::default()
        .with_tint(0.58, 0.46, 0.26)
        .with_roughness(0.95);
    let wood = mat(MaterialPreset::WoodPlanks).with_tint(0.55, 0.42, 0.3);
    let mut e = commands.spawn((
        Name::new(format!("Practice dummy {}", lane + 1)),
        Transform::from_translation(at),
        Collider {
            shape: ColliderShape::Compound(vec![(
                Vec3::new(0.0, 1.1, 0.0),
                Quat::IDENTITY,
                ColliderShape::Capsule {
                    radius: 0.34,
                    half_height: 0.55,
                },
            )]),
            density: 300.0,
            ..Default::default()
        },
        Vitality {
            revive_secs: 3.0,
            regen: 4.0,
            ..Vitality::new(400.0)
        },
        // Not Combustible: the fire module lights the ground fuel under a burning object, and
        // the hall floor stands on the valley's grass, so one burning dummy set the whole
        // terrace alight (and hid every later shot in smoke). The Burning status still shows.
        HallTarget {
            lane,
            focus,
            home: at,
        },
    ));
    if dynamic {
        e.insert((
            RigidBody {
                kind: BodyKind::Dynamic,
                mass_kg: Some(45.0),
                ..Default::default()
            },
            ProgramProp,
        ));
    }
    let id = e.id();
    let child = |commands: &mut Commands, y: f32, rot: Quat, shape: Shape, m: StandardMaterial| {
        commands.spawn((
            Name::new("Dummy part"),
            Transform::from_xyz(0.0, y as f64, 0.0).with_rotation(rot),
            part(shape, m),
            ChildOf(id),
        ));
    };
    child(
        commands,
        0.05,
        Quat::IDENTITY,
        Shape::Cylinder {
            radius: 0.42,
            height: 0.1,
            segments: 24,
        },
        mat(MaterialPreset::Rock).with_tint(0.4, 0.38, 0.36),
    );
    child(
        commands,
        0.5,
        Quat::IDENTITY,
        Shape::Cylinder {
            radius: 0.06,
            height: 1.0,
            segments: 12,
        },
        wood,
    );
    child(
        commands,
        1.15,
        Quat::IDENTITY,
        Shape::Capsule {
            radius: 0.32,
            height: 1.0,
            segments: 16,
        },
        straw,
    );
    child(
        commands,
        1.86,
        Quat::IDENTITY,
        Shape::Sphere {
            radius: 0.2,
            segments: 16,
        },
        straw,
    );
    child(
        commands,
        1.45,
        Quat::from_rotation_z(std::f32::consts::FRAC_PI_2),
        Shape::Cylinder {
            radius: 0.045,
            height: 1.3,
            segments: 10,
        },
        wood,
    );
    id
}

/// A loose crate or barrel for force spells.
pub fn spawn_crate(
    commands: &mut Commands,
    at: DVec3,
    lane: usize,
    focus: bool,
    barrel: bool,
) -> Entity {
    let wood = mat(MaterialPreset::WoodPlanks);
    let (shape, collider, mass) = if barrel {
        (
            Shape::Cylinder {
                radius: 0.32,
                height: 0.9,
                segments: 24,
            },
            ColliderShape::Cylinder {
                radius: 0.32,
                half_height: 0.45,
            },
            22.0,
        )
    } else {
        (
            Shape::RoundedCuboid {
                size: [0.75, 0.75, 0.75],
                radius: 0.03,
                segments: 0,
            },
            ColliderShape::Cuboid {
                half_extents: Vec3::splat(0.375),
            },
            16.0,
        )
    };
    commands
        .spawn((
            Name::new(if barrel { "Barrel" } else { "Crate" }),
            Transform::from_translation(at),
            part(
                shape,
                if barrel {
                    wood.with_tint(0.45, 0.3, 0.2)
                } else {
                    wood.with_tint(0.75, 0.6, 0.42)
                },
            ),
            Collider {
                shape: collider,
                density: 500.0,
                ..Default::default()
            },
            RigidBody {
                kind: BodyKind::Dynamic,
                mass_kg: Some(mass),
                ..Default::default()
            },
            HallTarget {
                lane,
                focus,
                home: at,
            },
            ProgramProp,
        ))
        .id()
}

/// A mage on a plinth, facing down the lane.
pub fn spawn_mage(commands: &mut Commands, at: DVec3, lane: usize, mastery: Mastery) -> Entity {
    let id = commands
        .spawn((
            Name::new(format!("Mage {}", lane + 1)),
            Transform::from_translation(at),
            MagicCaster {
                learns: false,
                side: 1,
                seed: 1000 + lane as u64,
                ..MagicCaster::new(60.0, mastery)
            },
            Vitality::new(300.0),
            BodyRequest::human(77 + lane as u64 * 13),
            HallMage { lane },
        ))
        .id();
    let robe = robe_color(mastery);
    let cloth = StandardMaterial::default()
        .with_tint(robe[0], robe[1], robe[2])
        .with_roughness(0.9);
    for (y, shape, m) in [
        (
            0.75,
            Shape::Cone {
                radius: 0.42,
                height: 1.5,
                segments: 20,
            },
            cloth,
        ),
        (
            1.62,
            Shape::Sphere {
                radius: 0.15,
                segments: 16,
            },
            StandardMaterial::default().with_tint(0.75, 0.6, 0.5),
        ),
        (
            1.78,
            Shape::Cone {
                radius: 0.2,
                height: 0.45,
                segments: 16,
            },
            cloth,
        ),
    ] {
        commands.spawn((
            Name::new("Mage stand-in"),
            Transform::from_xyz(0.0, y, 0.0),
            part(shape, m),
            StandIn { owner: id },
            ChildOf(id),
        ));
    }
    id
}

/// Hide stand-in bodies once a real one has appeared.
pub fn hide_stand_ins(
    mut parts: Query<(&StandIn, &mut MeshRenderer)>,
    bodies: Query<(), With<engine_world::animation::SkeletonPose>>,
) {
    for (s, mut m) in &mut parts {
        let hide = bodies.contains(s.owner);
        if m.visible == hide {
            m.visible = !hide;
        }
    }
}

/// The readout lines for the hall.
pub fn write_readout(readouts: &mut Readouts, title: &str, lines: &[String]) {
    readouts.set("Weaving hall", title);
    for (i, l) in lines.iter().enumerate() {
        if !l.is_empty() {
            readouts.set(format!("Lane {}", i + 1), l.clone());
        }
    }
}
