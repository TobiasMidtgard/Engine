//! Showcase content.
//!
//! * [`spawn_demo`]: the combat arena, a sand floor ringed by sandstone stands (seats, an
//!   arcade and four gates) with team banners, fluted columns and braziers; the art comes from
//!   [`crate::art::arena`] and sits on unchanged gameplay colliders. Two squads of mixed skill
//!   fight across ashlar walls and timber barricades, crates, explosive drums and a raised
//!   platform: shooters take cover and climb for height, squads
//!   suppress and flank, sword fighters close in and take turns. A master and a veteran duel
//!   in a ring in front (block, parry, feint, punish). On the east side a sparring lane puts
//!   five fighters from rookie to master in front of swinging posts, so you can see skill
//!   decide who gets hit, who blocks and who parries. Each squad has a medic (an armband, a
//!   carbine, the back line): it runs to a downed squad mate, kneels and gets it up where it
//!   lies while its mates cover it; one nobody reaches gets up by itself after six seconds,
//!   and barrels come back, so the fight never ends. Rings on the ground show
//!   where each fighter decided to go and why.
//! * [`spawn_wildlife`]: a living meadow. A deer herd grazes and wanders, rabbits nibble near
//!   cover, bird flocks peck in the open, a sounder of wild boar (two sows and their striped
//!   piglets) roots along the woods edge with a lone tusker not far off, and a wolf pack roams
//!   the far edge and hunts when hungry.

use std::f64::consts::TAU;

use engine_core::prelude::*;
use engine_render::PointLight;
use engine_world::physics::{Collider, ColliderShape};

use crate::agent::{Npc, NpcRole, NpcSettings, NpcWeapon};
use crate::art::arena::{self, ArenaLayout, Piece, Team};
use crate::faction::{Faction, Factions, Squad};
use crate::features::ExplosiveProp;
use crate::skills::NpcSkills;
use crate::training::SparringPost;
use crate::visuals::StandIn;
use crate::wildlife::{Animal, Herd, Species};

/// How far the arena reaches from its origin, stands and banners included, meters. The fight
/// stays on the floor inside the stands (41 m).
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

/// A gameplay solid: a box collider resting on the floor (`arena::dress` adds its art).
fn solid(commands: &mut Commands, at: DVec3, half: Vec3, yaw: f32) -> Entity {
    commands
        .spawn((
            Transform::from_translation(at + DVec3::Y * half.y as f64)
                .with_rotation(Quat::from_rotation_y(yaw)),
            Collider::cuboid(half),
        ))
        .id()
}

/// Stands with four gates, team banners, pillars, braziers, and the ramp and wall-run wall the
/// movement module uses: colliders where fighters can touch them, everything else only in
/// `layout` for the art.
fn arena_shell(commands: &mut Commands, origin: DVec3, layout: &mut ArenaLayout) {
    let o = |x: f64, y: f64, z: f64| origin + DVec3::new(x, y, z);
    // The podium wall of the stands keeps fighters in: one collider per 10° sector, open at
    // the four gates (east, north, west, south).
    const SEGMENTS: usize = 36;
    let r = 41.8;
    let width = (TAU * r / SEGMENTS as f64) as f32 + 0.25;
    let half = Vec3::new(width * 0.5, 0.4, 0.8);
    for i in 0..SEGMENTS {
        if i % (SEGMENTS / 4) == 0 {
            continue;
        }
        let a = i as f64 / SEGMENTS as f64 * TAU;
        let yaw = -(a as f32) + std::f32::consts::FRAC_PI_2;
        let e = commands
            .spawn((
                Transform::from_translation(o(a.cos() * r, 0.4, a.sin() * r))
                    .with_rotation(Quat::from_rotation_y(yaw)),
                Collider::cuboid(half),
            ))
            .id();
        layout.pieces.push((e, Piece::Podium));
    }
    // Banners on the top walkway: red behind the red squad (north), blue behind the blue (south).
    for (deg, team) in [
        (62.0, Team::Red),
        (76.0, Team::Red),
        (104.0, Team::Red),
        (118.0, Team::Red),
        (242.0, Team::Blue),
        (256.0, Team::Blue),
        (284.0, Team::Blue),
        (298.0, Team::Blue),
    ] {
        layout.banners.push((f64::to_radians(deg), team));
    }
    // Pillars, turned half a step so none blocks the south camera's view of the center.
    for i in 0..8 {
        let a = (i as f64 + 0.5) / 8.0 * TAU;
        let e = commands
            .spawn((
                Transform::from_translation(o(a.cos() * 18.0, 3.5, a.sin() * 18.0)),
                Collider {
                    shape: ColliderShape::Cylinder {
                        radius: 1.1,
                        half_height: 3.5,
                    },
                    ..Default::default()
                },
            ))
            .id();
        layout.pieces.push((e, Piece::Pillar(i % 2)));
    }
    // Braziers flanking each entrance, each with the light of its fire.
    for side in 0..4 {
        for d in [-7.0f64, 7.0] {
            let a = f64::to_radians(side as f64 * 90.0 + d);
            let at = o(a.cos() * 38.6, 0.0, a.sin() * 38.6);
            layout.braziers.push(at);
            commands.spawn((
                Transform::from_translation(at + DVec3::Y * 1.8),
                PointLight {
                    color: [1.0, 0.55, 0.25],
                    intensity_lm: 2500.0,
                    range: 16.0,
                    radius: 0.3,
                },
                arena::Flicker {
                    seed: arena::fire_seed(layout.braziers.len() - 1),
                    lumens: 2500.0,
                },
            ));
        }
    }
    // A ramp and a wall-run wall for the movement module.
    let (ramp_at, tilt) = (o(12.0, 1.6, 8.0), 0.35);
    let half = Vec3::new(2.0, 0.25, 5.0);
    let e = commands
        .spawn((
            Transform::from_translation(ramp_at).with_rotation(Quat::from_rotation_x(tilt)),
            Collider::cuboid(half),
        ))
        .id();
    let ground = (origin.y - ramp_at.y) as f32;
    layout.pieces.push((e, Piece::Ramp(half, tilt, ground)));
    let half = Vec3::new(0.4, 3.0, 12.0);
    let e = solid(commands, o(-24.0, 0.0, 0.0), half, 0.0);
    layout.pieces.push((e, Piece::WallRun(half)));
}

/// Five sword-and-shield fighters, rookie to master, each facing a post that swings at them.
/// They hold their ground (not `active`), so only their defense shows.
fn sparring_lane(commands: &mut Commands, first: DVec3, layout: &mut ArenaLayout) {
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
        let mat = at - DVec3::X * 0.85;
        layout.mats.push(DVec3::new(mat.x, layout.origin.y, mat.z));
        let rel = (mat - layout.origin).as_vec3();
        layout.trample.push((Vec2::new(rel.x, rel.z), 2.2));
    }
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

/// A squad's medic (see [`NpcRole::Medic`]): a carbine, all for its mates.
fn medic(
    commands: &mut Commands,
    at: DVec3,
    face: DVec3,
    faction: Faction,
    squad: u32,
    skills: NpcSkills,
) -> Entity {
    let skills = NpcSkills {
        teamwork: 1.0,
        ..skills
    };
    let e = fighter(
        commands,
        at,
        face,
        faction,
        Some(squad),
        skills,
        NpcWeapon::carbine(),
        3.0,
    );
    commands.entity(e).insert(Npc {
        home: Some(at),
        patrol_radius: 3.0,
        role: NpcRole::Medic,
        ..Default::default()
    });
    e
}

/// The combat arena around `origin` (see the module docs). Fits in a 36 m radius and leaves
/// the line from the arena's south side (−Z) to its center open for the camera.
pub fn spawn_demo(commands: &mut Commands, origin: DVec3) {
    let mut layout = ArenaLayout {
        origin,
        duel_ring: origin + DUEL_RING,
        ..Default::default()
    };
    arena_shell(commands, origin, &mut layout);
    sparring_lane(commands, origin + SPARRING_LANE, &mut layout);
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
    for (k, (x, z, yaw)) in [
        (-6.0, 5.0, 0.0),
        (5.5, 6.5, 0.2),
        (0.0, 10.5, 0.0),
        (-9.5, -5.5, -0.3),
        (8.5, -6.0, 0.25),
        (-1.0, -9.5, 0.0),
        (13.0, 1.0, 1.4),
        // Two barricades where the fight in the middle most often leaves someone down: cover
        // for a mate to kneel behind.
        (-8.5, -1.2, -0.1),
        (8.0, 2.8, 0.1),
    ]
    .into_iter()
    .enumerate()
    {
        let e = solid(commands, o(x, z), low, yaw);
        // Ashlar walls and timber barricades, mixed.
        let piece = if matches!(k, 1 | 3 | 6 | 7 | 8) {
            Piece::TimberWall
        } else {
            Piece::StoneWall(k as u32)
        };
        layout.pieces.push((e, piece));
        layout.trample.push((Vec2::new(x as f32, z as f32), 2.6));
    }
    let crate_half = Vec3::new(0.8, 1.1, 0.8);
    for (k, (x, z)) in [(3.0, 1.5), (-4.0, -1.5), (11.5, -11.0), (-11.0, 11.5)]
        .into_iter()
        .enumerate()
    {
        let e = solid(commands, o(x, z), crate_half, 0.3);
        layout.pieces.push((e, Piece::Crates(k as u32)));
        layout.trample.push((Vec2::new(x as f32, z as f32), 2.2));
    }
    // A raised platform with steps (high ground), on the west side.
    let half = Vec3::new(2.5, 0.8, 3.0);
    let e = solid(commands, o(-17.0, 2.0), half, 0.0);
    layout.pieces.push((e, Piece::Platform(half)));
    for k in 0..3 {
        let half = Vec3::new(0.35, 0.2 + k as f32 * 0.2, 1.2);
        let e = solid(commands, o(-14.15 + 0.7 * (2 - k) as f64, 2.0), half, 0.0);
        layout.pieces.push((e, Piece::Step(k, half)));
    }
    layout.trample.push((Vec2::new(-12.5, 2.0), 2.5));
    // Explosive barrels near where each side takes cover.
    for (k, (x, z)) in [(1.8, 11.2), (-6.8, 5.8), (-1.5, -10.4), (9.2, -6.9)]
        .into_iter()
        .enumerate()
    {
        let e = commands
            .spawn((
                Transform::from_translation(o(x, z) + DVec3::Y * 0.5),
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
            ))
            .id();
        layout.pieces.push((e, Piece::Barrel(k as u32)));
    }
    // ------------------------------------------------------------------ squads
    // Red (north) and blue (south), each with a spread of skill and a medic in the back line.
    let north = o(0.0, 0.0) + DVec3::Z * 30.0;
    let south = o(0.0, 0.0) - DVec3::Z * 30.0;
    medic(
        commands,
        o(2.0, 22.0),
        south,
        Faction::ENEMY,
        101,
        NpcSkills::soldier(),
    );
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
    medic(
        commands,
        o(2.5, -21.0),
        north,
        Faction::RIVAL,
        102,
        NpcSkills::veteran(),
    );
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
    // ------------------------------------------------------------------ art
    let d = DUEL_RING.as_vec3();
    layout.trample.extend([
        (Vec2::new(d.x, d.z), 4.6),
        (Vec2::ZERO, 7.0),
        (Vec2::new(0.0, 17.0), 6.0),
        (Vec2::new(0.0, -16.0), 6.0),
    ]);
    commands.queue(move |world: &mut World| arena::dress(world, &layout));
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
    // Wild boar along the woods edge: a sounder of two sows and their piglets, and a lone
    // tusker further along.
    let boar = |commands: &mut Commands,
                at: DVec3,
                herd: u32,
                range: f32,
                male: bool,
                young: bool,
                yaw: f32| {
        commands.spawn((
            Transform::from_translation(at).with_rotation(Quat::from_rotation_y(yaw)),
            Animal {
                range,
                male,
                young,
                ..Animal::new(Species::Boar)
            },
            Herd(herd),
            StandIn::default(),
        ));
    };
    let sounder_at = center + DVec3::new(-r * 0.7, 0.0, -r * 0.75);
    for k in 0..6 {
        let a = rand() * std::f64::consts::TAU;
        let d = 1.0 + rand() * 3.0;
        let p = sounder_at + DVec3::new(a.cos() * d, 0.0, a.sin() * d);
        let yaw = (rand() * std::f64::consts::TAU) as f32;
        boar(commands, p, 40, (r * 0.4) as f32, false, k >= 2, yaw);
    }
    let yaw = (rand() * std::f64::consts::TAU) as f32;
    boar(
        commands,
        center + DVec3::new(r * 0.75, 0.0, -r * 0.6),
        41,
        (r * 0.5) as f32,
        true,
        false,
        yaw,
    );
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
