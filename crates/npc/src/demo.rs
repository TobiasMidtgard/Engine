//! Showcase content.
//!
//! * [`spawn_demo`]: the combat arena, a sand floor ringed by stepped stone stands with team
//!   banners, pillars and braziers. Two squads of mixed skill fight across low walls, crates,
//!   explosive barrels and a raised platform: shooters take cover and climb for height, squads
//!   suppress and flank, sword fighters close in and take turns. A master and a veteran duel
//!   in a ring in front (block, parry, feint, punish). On the east side a sparring lane puts
//!   five fighters from rookie to master in front of swinging posts, so you can see skill
//!   decide who gets hit, who blocks and who parries. Downed fighters get up after a few
//!   seconds and barrels come back, so the fight never ends. Rings on the ground show where
//!   each fighter decided to go and why.
//! * [`spawn_wildlife`]: a living meadow. A deer herd grazes and wanders, rabbits nibble near
//!   cover, bird flocks peck in the open, and a wolf pack roams the far edge and hunts when
//!   hungry.

use std::f64::consts::TAU;

use engine_core::prelude::*;
use engine_render::{MeshRenderer, PointLight, Shape, StandardMaterial};
use engine_world::physics::{Collider, ColliderShape};

use crate::agent::{Npc, NpcSettings, NpcWeapon};
use crate::faction::{Faction, Factions, Squad};
use crate::features::ExplosiveProp;
use crate::skills::NpcSkills;
use crate::training::SparringPost;
use crate::visuals::StandIn;
use crate::wildlife::{Animal, Herd, Species};

/// How far the arena reaches from its origin, stands and banners included, meters. The fight
/// stays within 40 m.
pub const ARENA_RADIUS: f64 = 48.0;

/// Where the duel ring is, relative to the arena origin (in front of the south camera).
pub const DUEL_RING: DVec3 = DVec3::new(12.0, 0.0, -23.0);

/// The sparring lane's first (rookie) station relative to the arena origin; the others follow
/// every 5 m toward +Z. Fighters stand on the east side of their post, facing west.
pub const SPARRING_LANE: DVec3 = DVec3::new(28.5, 0.0, -14.0);

/// Faction of the sparring lane: allied with everyone, so the squads leave it alone.
pub const SPARRING: Faction = Faction(23);

/// Factions of the side duel: they fight each other and ignore the squads.
pub const DUEL_A: Faction = Faction(21);
pub const DUEL_B: Faction = Faction(22);

fn concrete() -> StandardMaterial {
    StandardMaterial::hex(0xa39e94).with_roughness(0.85)
}

fn wood() -> StandardMaterial {
    StandardMaterial::hex(0x8a6a44).with_roughness(0.8)
}

fn stone() -> StandardMaterial {
    StandardMaterial::hex(0x9a9184).with_roughness(0.92)
}

fn piece(commands: &mut Commands, at: DVec3, yaw: f32, shape: Shape, material: StandardMaterial) {
    commands.spawn((
        Transform::from_translation(at).with_rotation(Quat::from_rotation_y(yaw)),
        MeshRenderer::new(shape, material),
    ));
}

/// Sand floor, stepped stands with four entrances, team banners, pillars, braziers, and the
/// ramp and wall-run wall the movement module uses.
fn arena_shell(commands: &mut Commands, origin: DVec3) {
    let o = |x: f64, y: f64, z: f64| origin + DVec3::new(x, y, z);
    piece(
        commands,
        o(0.0, 0.02, 0.0),
        0.0,
        Shape::Cylinder {
            radius: 40.0,
            height: 0.04,
            segments: 96,
        },
        StandardMaterial::hex(0xa48d66).with_roughness(0.97),
    );
    piece(
        commands,
        o(0.0, 0.06, 0.0),
        0.0,
        Shape::Torus {
            radius: 40.4,
            tube: 0.35,
            segments: 96,
        },
        stone(),
    );
    // Stands: four tiers of stone blocks, open at the four entrances (east, north, west,
    // south). The first tier has colliders and keeps fighters in.
    const SEGMENTS: usize = 36;
    for k in 0..4 {
        let r = 41.8 + k as f64 * 1.6;
        let h = 0.8 * (k + 1) as f32;
        let width = (TAU * r / SEGMENTS as f64) as f32 + 0.25;
        let half = Vec3::new(width * 0.5, h * 0.5, 0.8);
        for i in 0..SEGMENTS {
            if i % (SEGMENTS / 4) == 0 {
                continue;
            }
            let a = i as f64 / SEGMENTS as f64 * TAU;
            let at = o(a.cos() * r, h as f64 * 0.5, a.sin() * r);
            let yaw = -(a as f32) + std::f32::consts::FRAC_PI_2;
            let mut e = commands.spawn((
                Transform::from_translation(at).with_rotation(Quat::from_rotation_y(yaw)),
                MeshRenderer::new(
                    Shape::Cuboid {
                        size: (half * 2.0).to_array(),
                    },
                    stone(),
                ),
            ));
            if k == 0 {
                e.insert(Collider::cuboid(half));
            }
        }
    }
    // Banners on the top tier: red behind the red squad (north), blue behind the blue (south).
    for (deg, color) in [
        (62.0, 0xa83232),
        (76.0, 0xa83232),
        (104.0, 0xa83232),
        (118.0, 0xa83232),
        (242.0, 0x2f5fa8),
        (256.0, 0x2f5fa8),
        (284.0, 0x2f5fa8),
        (298.0, 0x2f5fa8),
    ] {
        let a = f64::to_radians(deg);
        let (c, s) = (a.cos(), a.sin());
        let base = 3.2;
        let r = 47.0;
        piece(
            commands,
            o(c * r, base + 3.5, s * r),
            0.0,
            Shape::Cylinder {
                radius: 0.07,
                height: 7.0,
                segments: 8,
            },
            StandardMaterial::hex(0x3b2f25).with_roughness(0.7),
        );
        let yaw = -(a as f32) + std::f32::consts::FRAC_PI_2;
        let cloth = o(c * (r - 0.12), base + 4.7, s * (r - 0.12));
        piece(
            commands,
            cloth,
            yaw,
            Shape::Cuboid {
                size: [1.5, 3.4, 0.04],
            },
            StandardMaterial::hex(color)
                .with_roughness(0.85)
                .with_subsurface(0.4)
                .with_wind_sway(0.12),
        );
        piece(
            commands,
            cloth + DVec3::Y * 1.75,
            yaw,
            Shape::Cuboid {
                size: [1.7, 0.08, 0.08],
            },
            StandardMaterial::hex(0xc9a227)
                .with_metallic(0.8)
                .with_roughness(0.35),
        );
    }
    // Pillars, turned half a step so none blocks the south camera's view of the center.
    for i in 0..8 {
        let a = (i as f64 + 0.5) / 8.0 * TAU;
        let at = o(a.cos() * 18.0, 0.0, a.sin() * 18.0);
        commands.spawn((
            Transform::from_translation(at + DVec3::Y * 3.5),
            MeshRenderer::new(
                Shape::Cylinder {
                    radius: 1.0,
                    height: 7.0,
                    segments: 24,
                },
                concrete(),
            ),
            Collider {
                shape: ColliderShape::Cylinder {
                    radius: 1.1,
                    half_height: 3.5,
                },
                ..Default::default()
            },
        ));
        for (y, size) in [(0.25, [2.4, 0.5, 2.4]), (7.15, [2.2, 0.3, 2.2])] {
            piece(
                commands,
                at + DVec3::Y * y,
                0.0,
                Shape::Cuboid { size },
                stone(),
            );
        }
    }
    // Braziers flanking each entrance.
    for side in 0..4 {
        for d in [-7.0f64, 7.0] {
            let a = f64::to_radians(side as f64 * 90.0 + d);
            let at = o(a.cos() * 38.6, 0.0, a.sin() * 38.6);
            brazier(commands, at);
        }
    }
    // A ramp and a wall-run wall for the movement module.
    commands.spawn((
        Transform::from_translation(o(12.0, 1.6, 8.0)).with_rotation(Quat::from_rotation_x(0.35)),
        MeshRenderer::new(
            Shape::Cuboid {
                size: [4.0, 0.5, 10.0],
            },
            concrete(),
        ),
        Collider::cuboid(Vec3::new(2.0, 0.25, 5.0)),
    ));
    block(
        commands,
        o(-24.0, 0.0, 0.0),
        Vec3::new(0.4, 3.0, 12.0),
        0.0,
        concrete(),
    );
}

fn brazier(commands: &mut Commands, at: DVec3) {
    piece(
        commands,
        at + DVec3::Y * 0.5,
        0.0,
        Shape::Cylinder {
            radius: 0.28,
            height: 1.0,
            segments: 12,
        },
        stone(),
    );
    piece(
        commands,
        at + DVec3::Y * 1.12,
        0.0,
        Shape::Cylinder {
            radius: 0.55,
            height: 0.26,
            segments: 16,
        },
        StandardMaterial::hex(0x2b2724)
            .with_metallic(0.8)
            .with_roughness(0.5),
    );
    piece(
        commands,
        at + DVec3::Y * 1.27,
        0.0,
        Shape::Cylinder {
            radius: 0.46,
            height: 0.06,
            segments: 16,
        },
        StandardMaterial::hex(0xff6a1a)
            .with_emissive([1.0, 0.38, 0.08], 8.0)
            .with_roughness(0.6),
    );
    commands.spawn((
        Transform::from_translation(at + DVec3::Y * 1.8),
        PointLight {
            color: [1.0, 0.55, 0.25],
            intensity_lm: 2500.0,
            range: 16.0,
            radius: 0.3,
        },
    ));
}

/// Five sword-and-shield fighters, rookie to master, each facing a post that swings at them.
/// They hold their ground (not `active`), so only their defense shows.
fn sparring_lane(commands: &mut Commands, first: DVec3) {
    for (i, skills) in NpcSkills::PRESETS.into_iter().enumerate() {
        let at = first + DVec3::Z * (i as f64 * 5.0);
        let post_at = at - DVec3::X * 1.7;
        let npc = commands
            .spawn((
                Transform::from_translation(at)
                    .with_rotation(Quat::from_rotation_y(std::f32::consts::FRAC_PI_2)),
                Npc {
                    active: false,
                    home: Some(at),
                    ..Default::default()
                },
                SPARRING,
                NpcSkills::preset(skills).unwrap_or_default(),
                NpcWeapon::sword_and_shield(),
                StandIn::default(),
            ))
            .id();
        commands.spawn((
            Transform::from_translation(post_at),
            SPARRING,
            SparringPost::new(npc),
        ));
        // A mat under each station.
        piece(
            commands,
            at - DVec3::X * 0.85 + DVec3::Y * 0.05,
            0.0,
            Shape::Cuboid {
                size: [3.6, 0.04, 2.6],
            },
            StandardMaterial::hex(0x5b4636).with_roughness(0.95),
        );
    }
}

fn block(commands: &mut Commands, at: DVec3, half: Vec3, yaw: f32, material: StandardMaterial) {
    commands.spawn((
        Transform::from_translation(at + DVec3::Y * half.y as f64)
            .with_rotation(Quat::from_rotation_y(yaw)),
        MeshRenderer::new(
            Shape::Cuboid {
                size: (half * 2.0).to_array(),
            },
            material,
        ),
        Collider::cuboid(half),
    ));
}

#[allow(clippy::too_many_arguments)]
fn fighter(
    commands: &mut Commands,
    at: DVec3,
    face: DVec3,
    faction: Faction,
    squad: Option<u32>,
    skills: NpcSkills,
    weapon: NpcWeapon,
    patrol: f32,
) -> Entity {
    let dir = (face - at).as_vec3();
    let yaw = (-dir.x).atan2(-dir.z);
    let mut e = commands.spawn((
        Transform::from_translation(at).with_rotation(Quat::from_rotation_y(yaw)),
        Npc {
            home: Some(at),
            patrol_radius: patrol,
            ..Default::default()
        },
        faction,
        skills,
        weapon,
        StandIn::default(),
    ));
    if let Some(s) = squad {
        e.insert(Squad(s));
    }
    e.id()
}

/// The combat arena around `origin` (see the module docs). Fits in a 36 m radius and leaves
/// the line from the arena's south side (−Z) to its center open for the camera.
pub fn spawn_demo(commands: &mut Commands, origin: DVec3) {
    arena_shell(commands, origin);
    sparring_lane(commands, origin + SPARRING_LANE);
    commands.queue(|world: &mut World| {
        if let Some(mut s) = world.get_resource_mut::<NpcSettings>() {
            s.revive_secs = 6.0;
            s.show_intent = true;
        }
        if let Some(mut f) = world.get_resource_mut::<Factions>() {
            for duelist in [DUEL_A, DUEL_B, SPARRING] {
                for other in [
                    Faction::PLAYER,
                    Faction::ENEMY,
                    Faction::RIVAL,
                    Faction::PREY,
                    Faction::PREDATOR,
                ] {
                    f.set_allied(duelist, other, true);
                }
            }
            f.set_allied(SPARRING, DUEL_A, true);
            f.set_allied(SPARRING, DUEL_B, true);
        }
    });
    let o = |x: f64, z: f64| origin + DVec3::new(x, 0.0, z);
    // ------------------------------------------------------------------ cover
    let low = Vec3::new(1.6, 0.55, 0.25);
    for (x, z, yaw) in [
        (-6.0, 5.0, 0.0),
        (5.5, 6.5, 0.2),
        (0.0, 10.5, 0.0),
        (-9.5, -5.5, -0.3),
        (8.5, -6.0, 0.25),
        (-1.0, -9.5, 0.0),
        (13.0, 1.0, 1.4),
    ] {
        block(commands, o(x, z), low, yaw, concrete());
    }
    let crate_half = Vec3::new(0.8, 1.1, 0.8);
    for (x, z) in [(3.0, 1.5), (-4.0, -1.5), (11.5, -11.0), (-11.0, 11.5)] {
        block(commands, o(x, z), crate_half, 0.3, wood());
    }
    // A raised platform with steps (high ground), on the west side.
    block(
        commands,
        o(-17.0, 2.0),
        Vec3::new(2.5, 0.8, 3.0),
        0.0,
        concrete(),
    );
    for k in 0..3 {
        let h = 0.2 + k as f32 * 0.2;
        block(
            commands,
            o(-14.15 + 0.7 * (2 - k) as f64, 2.0),
            Vec3::new(0.35, h, 1.2),
            0.0,
            concrete(),
        );
    }
    // Explosive barrels near where each side takes cover.
    for (x, z) in [(1.8, 11.2), (-6.8, 5.8), (-1.5, -10.4), (9.2, -6.9)] {
        commands.spawn((
            Transform::from_translation(o(x, z) + DVec3::Y * 0.5),
            MeshRenderer::new(
                Shape::Cylinder {
                    radius: 0.32,
                    height: 1.0,
                    segments: 16,
                },
                StandardMaterial::hex(0xc0281e)
                    .with_roughness(0.45)
                    .with_metallic(0.4),
            ),
            Collider {
                shape: ColliderShape::Cylinder {
                    radius: 0.32,
                    half_height: 0.5,
                },
                ..Default::default()
            },
            ExplosiveProp {
                rearm_secs: 14.0,
                ..Default::default()
            },
        ));
    }
    // ------------------------------------------------------------------ squads
    // Red (north) and blue (south), each with a spread of skill.
    let north = o(0.0, 0.0) + DVec3::Z * 30.0;
    let south = o(0.0, 0.0) - DVec3::Z * 30.0;
    for (x, z, skills, weapon) in [
        (0.0, 17.0, NpcSkills::master(), NpcWeapon::rifle()),
        (-4.0, 18.0, NpcSkills::veteran(), NpcWeapon::rifle()),
        (4.5, 18.0, NpcSkills::soldier(), NpcWeapon::rifle()),
        (8.5, 16.0, NpcSkills::rookie(), NpcWeapon::rifle()),
        (
            -8.5,
            16.0,
            NpcSkills::elite(),
            NpcWeapon::sword_and_shield(),
        ),
    ] {
        fighter(
            commands,
            o(x, z),
            south,
            Faction::ENEMY,
            Some(101),
            skills,
            weapon,
            3.0,
        );
    }
    for (x, z, skills, weapon) in [
        (0.0, -16.0, NpcSkills::elite(), NpcWeapon::rifle()),
        (5.0, -17.0, NpcSkills::veteran(), NpcWeapon::rifle()),
        (-5.0, -17.0, NpcSkills::soldier(), NpcWeapon::bow()),
        (-9.0, -14.0, NpcSkills::veteran(), NpcWeapon::sword()),
        (9.0, -14.0, NpcSkills::master(), NpcWeapon::sword()),
    ] {
        fighter(
            commands,
            o(x, z),
            north,
            Faction::RIVAL,
            Some(102),
            skills,
            weapon,
            3.0,
        );
    }
    // ------------------------------------------------------------------ duel ring (south)
    let ring = origin + DUEL_RING;
    commands.spawn((
        Transform::from_translation(ring + DVec3::Y * 0.02),
        MeshRenderer::new(
            Shape::Torus {
                radius: 5.0,
                tube: 0.12,
                segments: 48,
            },
            StandardMaterial::hex(0x3a3530).with_roughness(0.9),
        ),
    ));
    let a = ring + DVec3::new(2.2, 0.0, 0.0);
    let b = ring - DVec3::new(2.2, 0.0, 0.0);
    let master = fighter(
        commands,
        a,
        b,
        DUEL_A,
        None,
        NpcSkills::master(),
        NpcWeapon::sword_and_shield(),
        1.0,
    );
    let veteran = fighter(
        commands,
        b,
        a,
        DUEL_B,
        None,
        NpcSkills::veteran(),
        NpcWeapon::sword(),
        1.0,
    );
    for (e, color) in [(master, 0x6b3fa0), (veteran, 0x2f8f86)] {
        commands.entity(e).insert(StandIn {
            color: Some(color),
            ..Default::default()
        });
    }
}

/// Animals around `center` within `radius` meters (see the module docs). Deer drink at water
/// they can find within three times their range; birds stay in the open.
pub fn spawn_wildlife(commands: &mut Commands, center: DVec3, radius: f64) {
    let r = radius.max(20.0);
    let mut seed = 0x5eed_u64;
    let mut rand = move || {
        seed = engine_core::math::hash_u64(seed.wrapping_add(0x9e37_79b9_7f4a_7c15));
        (seed >> 40) as f64 / (1u64 << 24) as f64
    };
    let spawn = |commands: &mut Commands,
                 species: Species,
                 at: DVec3,
                 herd: u32,
                 range: f32,
                 male: bool,
                 yaw: f32| {
        commands.spawn((
            Transform::from_translation(at).with_rotation(Quat::from_rotation_y(yaw)),
            Animal {
                range,
                male,
                ..Animal::new(species)
            },
            Herd(herd),
            StandIn::default(),
        ));
    };
    // Deer herd in the middle.
    let herd_at = center + DVec3::new(-r * 0.1, 0.0, r * 0.1);
    for k in 0..7 {
        let a = rand() * std::f64::consts::TAU;
        let d = 1.5 + rand() * 5.0;
        let p = herd_at + DVec3::new(a.cos() * d, 0.0, a.sin() * d);
        spawn(
            commands,
            Species::Deer,
            p,
            1,
            (r * 0.45) as f32,
            k == 0,
            (rand() * std::f64::consts::TAU) as f32,
        );
    }
    // Rabbits in two groups.
    for (g, (dx, dz)) in [(0.35, -0.3), (-0.4, -0.25)].into_iter().enumerate() {
        let at = center + DVec3::new(r * dx, 0.0, r * dz);
        for _ in 0..4 {
            let p = at + DVec3::new(rand() * 4.0 - 2.0, 0.0, rand() * 4.0 - 2.0);
            spawn(
                commands,
                Species::Rabbit,
                p,
                10 + g as u32,
                8.0,
                false,
                (rand() * std::f64::consts::TAU) as f32,
            );
        }
    }
    // Bird flocks pecking in the open.
    for (g, (dx, dz)) in [(0.2, 0.3), (-0.25, 0.4)].into_iter().enumerate() {
        let at = center + DVec3::new(r * dx, 0.0, r * dz);
        for _ in 0..9 {
            let p = at + DVec3::new(rand() * 5.0 - 2.5, 0.0, rand() * 5.0 - 2.5);
            spawn(
                commands,
                Species::Bird,
                p,
                20 + g as u32,
                12.0,
                false,
                (rand() * std::f64::consts::TAU) as f32,
            );
        }
    }
    // A wolf pack at the far edge: it roams and hunts when hungry.
    let pack_at = center + DVec3::new(r * 0.8, 0.0, r * 0.9);
    for _ in 0..3 {
        let p = pack_at + DVec3::new(rand() * 4.0 - 2.0, 0.0, rand() * 4.0 - 2.0);
        spawn(
            commands,
            Species::Wolf,
            p,
            30,
            (r * 0.8) as f32,
            false,
            (rand() * std::f64::consts::TAU) as f32,
        );
    }
}
