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
use engine_world::character::{Appearance, Outfit};
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
/// Top of the marble dais the plinths stand on.
pub const DAIS_TOP: f64 = 0.55;
/// The Spellcrafter's altar: on the dais between the middle plinths, facing down the hall.
pub const ALTAR: [f64; 3] = [0.0, DAIS_TOP, MAGE_Z];
/// Where the altar's practice dummy stands while the altar is in use, 9 m in front of it.
pub const ALTAR_DUMMY: [f64; 3] = [0.0, FLOOR, MAGE_Z - 9.0];
/// The altar's crystal, above its top, in the altar's frame.
pub const CRYSTAL: [f32; 3] = [0.0, 1.38, -0.04];

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
        // Undyed wool, then woad blue, verdigris green, madder crimson, violet, and the
        // Ascended's bleached white: the ladder reads from the clothes.
        Mastery::Novice => [0.42, 0.36, 0.27],
        Mastery::Apprentice => [0.07, 0.13, 0.36],
        Mastery::Adept => [0.06, 0.24, 0.12],
        Mastery::Expert => [0.42, 0.04, 0.04],
        Mastery::Master => [0.17, 0.05, 0.32],
        _ => [0.78, 0.76, 0.7],
    }
}

/// A necromancer's robe: near black.
pub const NECRO_ROBE: [f32; 3] = [0.035, 0.03, 0.04];

/// A necromancer's look: a black robe and hood, a violet collar.
pub fn necromancer_look(lane: usize) -> Appearance {
    let mut a = mage_look(lane, Mastery::Master, None);
    a.outfit.top_color = NECRO_ROBE;
    a.outfit.bottom_color = NECRO_ROBE.map(|c| c * 0.8);
    a.outfit.accent = [0.45, 0.2, 0.75];
    a.outfit.accent_on = "collar".into();
    a
}

/// A mage's look: hood, a long-sleeved shirt over a long skirt in the mastery's colour (a
/// robe, from the character contract's garments), a gold collar from Expert up, or a side's
/// armband in the duel. Faces vary by lane, the same every run.
pub fn mage_look(lane: usize, mastery: Mastery, side: Option<[f32; 3]>) -> Appearance {
    let robe = robe_color(mastery);
    let k = lane as u64 % 100;
    let mut a = Appearance {
        seed: 4100 + k * 37,
        sex: if k % 3 == 1 { 0.1 } else { 0.9 },
        age: 22.0
            + ((k * 13) % 9) as f32 * 5.0
            + if mastery >= Mastery::Master {
                18.0
            } else {
                0.0
            },
        headgear: "hood".into(),
        ..Default::default()
    };
    a.outfit = Outfit {
        top: "shirt".into(),
        top_color: robe,
        bottom: "skirt".into(),
        bottom_color: robe.map(|c| c * 0.82),
        accent: side.unwrap_or([0.75, 0.52, 0.14]),
        accent_on: if side.is_some() {
            "armband_l"
        } else if mastery >= Mastery::Expert {
            "collar"
        } else {
            "none"
        }
        .into(),
    };
    a
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

/// The Spellcrafter's altar (round-7-plan.md): a dark stone pedestal with a bronze ring on its
/// top and a crystal floating over it that glows in the color of the spell being woven, and a
/// practice dummy that rises 9 m in front of it while someone weaves there. Press E at its
/// back ("Weave a spell").
pub fn spawn_altar(commands: &mut Commands, origin: DVec3) -> Entity {
    use crate::crafter::{Altar, AltarGlow, crystal_caster};
    use engine_render::PointLight;
    use engine_world::interact::Interactable;
    let at = origin + DVec3::from_array(ALTAR);
    let stand = origin + DVec3::from_array(ALTAR_DUMMY);
    let dummy = spawn_dummy(commands, stand - DVec3::Y * 40.0, 0, false, false);
    commands
        .entity(dummy)
        .insert(Name::new("Altar's practice dummy"))
        .remove::<HallTarget>();
    let stone = mat(MaterialPreset::Rock)
        .with_tint(0.2, 0.19, 0.22)
        .with_roughness(0.7);
    let altar = commands
        .spawn((
            Name::new("Spellcrafter's altar"),
            Transform::from_translation(at),
            Altar {
                dummy: Some(dummy),
                stand,
            },
            crystal_caster(Vec3::from_array(CRYSTAL)),
            Interactable::new("Weave a spell")
                .reach(2.2)
                .at(Vec3::new(0.0, 1.0, 0.55)),
            Collider {
                shape: ColliderShape::Compound(vec![(
                    Vec3::new(0.0, 0.535, 0.0),
                    Quat::IDENTITY,
                    ColliderShape::Cuboid {
                        half_extents: Vec3::new(0.58, 0.535, 0.38),
                    },
                )]),
                ..Default::default()
            },
        ))
        .id();
    let piece = |commands: &mut Commands, name: &str, y: f64, shape: Shape, m: StandardMaterial| {
        commands.spawn((
            Name::new(name.to_string()),
            Transform::from_xyz(0.0, y, 0.0),
            part(shape, m),
            ChildOf(altar),
        ));
    };
    let block = |size: [f32; 3]| Shape::RoundedCuboid {
        size,
        radius: 0.035,
        segments: 0,
    };
    piece(
        commands,
        "Altar foot",
        0.07,
        block([1.2, 0.14, 0.82]),
        stone,
    );
    piece(
        commands,
        "Altar body",
        0.54,
        block([0.9, 0.82, 0.56]),
        stone,
    );
    piece(
        commands,
        "Altar top",
        1.01,
        block([1.16, 0.12, 0.76]),
        stone,
    );
    let bronze = StandardMaterial::default()
        .with_tint(0.55, 0.38, 0.18)
        .with_metallic(1.0)
        .with_roughness(0.35);
    commands.spawn((
        Name::new("Altar ring"),
        Transform::from_xyz(0.0, 1.08, -0.04),
        part(
            Shape::Torus {
                radius: 0.24,
                tube: 0.022,
                segments: 40,
            },
            bronze,
        ),
        ChildOf(altar),
    ));
    // A band of bronze runes round the body.
    piece(
        commands,
        "Altar band",
        0.78,
        block([0.93, 0.05, 0.59]),
        bronze,
    );
    // The crystal: two six-sided cones, point up and point down.
    let glow = crate::crafter::IDLE_GLOW;
    let crystal = StandardMaterial::default()
        .with_tint(glow[0] * 0.3, glow[1] * 0.3, glow[2] * 0.3)
        .with_roughness(0.15)
        .with_emissive(glow, 6.0);
    let c = Vec3::from_array(CRYSTAL).as_dvec3();
    for (up, h) in [(true, 0.3f32), (false, 0.18)] {
        let rot = if up {
            Quat::IDENTITY
        } else {
            Quat::from_rotation_x(std::f32::consts::PI)
        };
        let dy = if up { h * 0.5 } else { -h * 0.5 } as f64;
        commands.spawn((
            Name::new("Altar crystal"),
            Transform::from_translation(c + DVec3::Y * dy).with_rotation(rot),
            part(
                Shape::Cone {
                    radius: 0.11,
                    height: h,
                    segments: 6,
                },
                crystal,
            ),
            AltarGlow {
                altar,
                strength: 6.0,
            },
            ChildOf(altar),
        ));
    }
    commands.spawn((
        Name::new("Altar light"),
        Transform::from_translation(c + DVec3::Y * 0.1),
        PointLight {
            color: glow,
            intensity_lm: 700.0,
            range: 7.0,
            radius: 0.1,
        },
        AltarGlow {
            altar,
            strength: 700.0,
        },
        ChildOf(altar),
    ));
    altar
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
            mage_look(lane, mastery, None),
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
