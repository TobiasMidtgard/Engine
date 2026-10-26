//! The destruction showcase: a siege breach range plus Valheim-style building and rock, built
//! by [`spawn_demo`] around an origin.
//!
//! **Breach range** (a row of bays facing −Z, toward the viewer): each bay is one test case.
//! Five are an *aftermath*, already wrecked on the first frame with their rubble lying on the
//! ground; three are *live* and get destroyed again every cycle:
//!
//! | bay | wall | test case |
//! |---|---|---|
//! | 1 | drywall | riddled by rifle bursts, a kicked-through vault hole, a breach charge hole |
//! | 2 | drywall with a doorway and a window | wooden barricades: the door one shot and kicked open, the window one intact |
//! | 3 | reinforced concrete | thermite opening: a clean doorway, rebar cut, scorched rim |
//! | 4 | reinforced concrete | **live**: a thermite charge burns a doorway every cycle |
//! | 5 | reinforced concrete | explosive charge: concrete blown off both faces, rebar mesh left standing |
//! | 6 | drywall | **live**: rifle bursts, then a breach charge that also shatters the crates in front |
//! | 7 | brick | a heavy blast hole |
//! | 8 | drywall | **live**: Siege reinforcement. A burst goes through, a defender sets two steel panels behind it, the next bursts ring off the steel, a breach charge only strips the drywall, and thermite burns through both |
//!
//! **Behind the range** (live every cycle): a wooden watchtower in Valheim build-mode support
//! colors loses its posts one by one and comes down; a stone gate is blasted at one foot and
//! half its arch comes down; a voxel cliff is mined until its overhang loses support and breaks off; a rock arch
//! loses a pillar and its span collapses.
//!
//! **Breach house** (Siege vertical play, north of the tower): a brick room with an open front
//! and a soft timber roof deck on joists. Its west bay was shot up from below long ago
//! (sunlight comes through the holes); every cycle the room fires up through the east bay, a
//! breach charge on the roof blows a hole into the room, thermite cuts the steel hatch out and
//! a sledgehammer smashes the wooden hatch in. A stair up the back climbs to the roof and a
//! steel ladder hangs under each hatch.

use bevy_ecs::message::{Message, Messages};
use bevy_ecs::system::RunSystemOnce;
use bevy_ecs::world::CommandQueue;
use engine_core::math::{DVec3, Quat, Rng, Vec2, Vec3};
use engine_core::prelude::*;
use engine_render::{MeshRenderer, Shape, StandardMaterial};
use engine_world::{
    Climbable, EditOp, EditShape, NoiseEvent, ProjectileTrace, SmokeEmission, TerrainEdit,
    WorldImpulse,
};

use crate::DemoView;
use crate::architecture::TowerTop;
use crate::charges::BreachCharge;
use crate::debris::{Debris, PendingDebris, settle};
use crate::dress::{ArchKind, ArchModel, GravelPad, PropModel};
use crate::hits::{Hit, HitQueue};
use crate::material::{DestructibleMaterial, linear};
use crate::props::{self, PropKind};
use crate::reinforce::{ReinforceRequest, remove_reinforcement};
use crate::structure::{Fracturable, FractureShape, ShowSupport, StructuralPiece};
use crate::voxel::VoxelVolume;
use crate::wall::{DestructibleWall, WallAnchors, WallState};

/// Seconds per live cycle: the show takes its first 15, then the wreckage stands until the
/// rebuild. Looping faster piled up dust: the smoke module keeps a cloud for minutes, and
/// six shows' worth hung over the range like fog.
pub const CYCLE_SECS: f32 = 60.0;

/// How far the fortress demo's ranges and buildings reach from its origin, meters (its script
/// waits while all of it is out of every focus's simulation range).
pub const DEMO_RADIUS_M: f64 = 60.0;
/// Lifetime given to aftermath rubble (it never sinks).
const FOREVER: f32 = 1.0e9;

/// Bay centers along X (relative to the origin) and the row's Z.
const BAYS: [f64; 8] = [20.4, 13.6, 6.8, 0.0, -6.8, -13.6, -20.4, -27.2];
/// Middle of the row of bays along X.
const ROW_MID: f64 = (BAYS[0] + BAYS[BAYS.len() - 1]) * 0.5;
/// Where the live thermite goes on the reinforced bay's drywall (wall-local): in the middle of
/// one of its two panels, clear of their frames.
const REINFORCE_THERMITE: [f32; 2] = [1.35, 1.1];
const ROW_Z: f64 = -8.0;
const WALL_W: f32 = 5.4;
const WALL_H: f32 = 3.2;

const TOWER: [f64; 3] = [18.0, 0.0, 9.0];
const GATE: [f64; 3] = [-22.0, 0.0, 7.0];
const CLIFF: [f64; 3] = [-4.0, 0.0, 16.0];
const ARCH: [f64; 3] = [-36.0, 0.0, 16.0];
/// The gate and arch charges are cutting charges: a hot flash, short of lighting the grass
/// round them (engine_fire lights what a blast of 250 °C or more touches).
const CUT_CHARGE_HEAT_C: f32 = 200.0;
/// The breach house's floor middle (north of the tower, in the sun); its open front looks +Z.
pub const HOUSE: [f64; 3] = [15.0, 0.0, 22.0];
/// House-local: top of the walls and joists (the roof deck lies on them) and the deck boards.
const JOIST_TOP: f64 = 3.24;
const DECK_T: f32 = 0.05;
/// Middles (x) of the deck's four bays between joists and side walls, and their width.
const DECK_BAYS: [f64; 4] = [-2.565, -0.855, 0.855, 2.565];
const BAY_W: f32 = 1.71;
/// The deck runs from z = `DECK_FRONT` back over the back wall.
const DECK_FRONT: f64 = 3.08;
const DECK_LEN: f32 = 6.1;
/// Middles (x, z) of the wooden hatch (bay 2) and the steel one (bay 3), 1 m square.
const HATCH: [f64; 2] = [-0.855, -1.6];
const STEEL_HATCH: [f64; 2] = [0.855, -1.6];
/// How the breacher heaves the cut steel hatch plate out (hatch-local m/s): up out of the hole
/// and toward bay 4, away from the wooden hatch, so it lands on the deck and never drops onto
/// the climber waiting on the ladder below.
const HATCH_HEAVE: [f32; 3] = [2.0, 0.0, 3.5];
const HATCH_M: f32 = 1.0;
/// Where the live breach charge sits on the roof (house-local x, z): in front of the steel
/// hatch, mid-room, so what it blows through rains onto the floor.
const ROOF_CHARGE: [f64; 2] = [0.855, 0.9];

/// Entities the live script acts on.
#[derive(Debug, Clone, Default)]
pub struct DemoTargets {
    pub thermite_wall: Option<Entity>,
    pub soft_wall: Option<Entity>,
    pub cliff: Option<Entity>,
    pub arch: Option<Entity>,
    pub tower: Vec<Entity>,
    pub tower_posts: Vec<Entity>,
    pub gate: Vec<Entity>,
    pub props: Vec<Entity>,
    /// The breach house's live roof bays (the charge's, then the one shot up) and its two
    /// hatch covers.
    pub roof: Vec<Entity>,
    pub hatch: Option<Entity>,
    pub steel_hatch: Option<Entity>,
    /// The drywall a defender reinforces live (bay 8).
    pub reinforced_wall: Option<Entity>,
}

/// The breach range's live moments (the zone's own stop is the overview from
/// `(-2, 17, -44)` looking at `(-3, 2.5, 4)`).
pub const VIEWS: &[DemoView] = &[
    DemoView {
        key: "fortress_gate",
        name: "The fortress gatehouse and its walls",
        eye: [-14.0, 5.0, -54.0],
        target: [0.0, 7.0, -31.0],
        hour: 15.0,
        warmup_secs: 0.2,
    },
    DemoView {
        key: "fortress_bays",
        name: "Breach bays: shot, kicked in, burned and blown open",
        eye: [12.0, 4.5, -22.0],
        target: [0.0, 1.4, -8.0],
        hour: 15.0,
        warmup_secs: 0.2,
    },
    DemoView {
        key: "fortress_thermite",
        name: "Thermite burning a doorway through rebar concrete",
        eye: [2.2, 1.7, -13.0],
        target: [0.0, 1.2, -8.0],
        hour: 15.0,
        warmup_secs: 2.0,
    },
    DemoView {
        key: "fortress_charge",
        name: "A breach charge blowing a wall open",
        eye: [-9.5, 2.4, -17.0],
        target: [-13.6, 1.3, -8.0],
        hour: 15.0,
        warmup_secs: 3.95,
    },
    DemoView {
        key: "fortress_cliff",
        name: "A mined cliff caving in",
        eye: [3.0, 5.0, -7.0],
        target: [-4.0, 3.5, 12.0],
        hour: 15.0,
        warmup_secs: 4.2,
    },
    DemoView {
        key: "fortress_tower",
        name: "A watchtower in support colors losing its posts",
        eye: [9.0, 4.5, -5.0],
        target: [18.0, 3.0, 9.0],
        hour: 15.0,
        warmup_secs: 10.1,
    },
    DemoView {
        key: "fortress_fire",
        name: "A timber storehouse burning down",
        eye: [39.0, 3.2, 8.0],
        target: [31.0, 2.0, 20.0],
        hour: 15.0,
        warmup_secs: 45.0,
    },
    DemoView {
        key: "fortress_roof",
        name: "Vertical breach: a charge blows a hole through a soft roof",
        eye: [20.5, 8.5, 13.5],
        target: [15.0, 3.0, 21.0],
        hour: 15.0,
        warmup_secs: 6.3,
    },
    DemoView {
        key: "fortress_hatches",
        name: "Under the roof: bullet holes, a blown bay and breached hatches",
        eye: [16.2, 1.6, 25.4],
        target: [15.1, 2.1, 20.4],
        hour: 15.0,
        warmup_secs: 13.0,
    },
    DemoView {
        key: "fortress_reinforce",
        name: "Siege reinforcement: steel panels braced behind a drywall",
        eye: [-23.6, 2.3, -2.6],
        target: [-27.2, 1.3, -8.2],
        hour: 15.0,
        warmup_secs: 4.2,
    },
    DemoView {
        key: "fortress_reinforce_thermite",
        name: "Thermite burning through a reinforced wall",
        eye: [-24.6, 1.8, -13.6],
        target: [-26.4, 1.3, -8.0],
        hour: 15.0,
        warmup_secs: 13.0,
    },
    DemoView {
        key: "fortress_arch",
        name: "A rock arch blown off its foot",
        eye: [-26.0, 6.0, -4.0],
        target: [-36.0, 3.5, 16.0],
        hour: 15.0,
        warmup_secs: 14.0,
    },
];

/// The running showcase script (one per [`spawn_demo`]).
#[derive(Component, Debug, Clone, Default)]
pub struct DestructionDemo {
    pub origin: DVec3,
    /// Seconds into the current cycle.
    pub time: f32,
    pub cycle: u32,
    next: usize,
    pub targets: DemoTargets,
}

#[derive(Debug, Clone, Copy)]
enum Step {
    Thermite,
    Burst(u32),
    PlantCharge,
    Dig(u32),
    BreakPost(usize),
    BlastGate,
    BlastArch,
    RoofBurst(u32),
    /// Bay 8: a burst at the drywall (through it before the panels go up, onto steel after).
    ReinforceBurst(u32),
    Reinforce,
    ReinforceCharge,
    ReinforceThermite,
    RoofCharge,
    ThermiteHatch,
    SmashHatch(u32),
    SupportView(bool),
    Clear,
    Reset,
}

const DIGS: u32 = 64;

fn timeline() -> Vec<(f32, Step)> {
    let mut t = vec![(0.3, Step::Thermite), (1.0, Step::PlantCharge)];
    for k in 0..7 {
        t.push((0.5 + k as f32 * 0.4, Step::Burst(k)));
    }
    for k in 0..DIGS {
        t.push((1.5 + k as f32 * 0.11, Step::Dig(k)));
    }
    for (k, at) in [8.0, 9.0, 10.0, 11.0].into_iter().enumerate() {
        t.push((at, Step::BreakPost(k)));
    }
    for k in 0..4 {
        t.push((2.0 + k as f32 * 0.4, Step::RoofBurst(k)));
    }
    t.push((3.4, Step::RoofCharge));
    t.push((7.6, Step::ThermiteHatch));
    for k in 0..4 {
        t.push((9.2 + k as f32 * 0.4, Step::SmashHatch(k)));
    }
    t.push((0.6, Step::ReinforceBurst(0)));
    t.push((1.6, Step::Reinforce));
    for k in 1..4 {
        t.push((4.2 + (k - 1) as f32 * 0.4, Step::ReinforceBurst(k)));
    }
    t.push((6.0, Step::ReinforceCharge));
    t.push((10.0, Step::ReinforceThermite));
    t.push((6.0, Step::SupportView(true)));
    t.push((12.0, Step::SupportView(false)));
    t.push((12.5, Step::BlastGate));
    t.push((14.0, Step::BlastArch));
    t.push((CYCLE_SECS - 2.0, Step::Clear));
    t.push((CYCLE_SECS - 0.01, Step::Reset));
    t.sort_by(|a, b| a.0.total_cmp(&b.0));
    t
}

fn mat(hex: u32, roughness: f32) -> StandardMaterial {
    StandardMaterial::hex(hex).with_roughness(roughness)
}

fn placed(at: DVec3, yaw: f32) -> (Transform, GlobalTransform) {
    let t = Transform::from_translation(at).with_rotation(Quat::from_rotation_y(yaw));
    (t, GlobalTransform::from(t))
}

fn prop(
    commands: &mut Commands,
    shape: Shape,
    material: StandardMaterial,
    at: DVec3,
    yaw: f32,
) -> Entity {
    let (t, g) = placed(at, yaw);
    commands
        .spawn((t, g, MeshRenderer::new(shape, material)))
        .id()
}

/// A detailed prop (see [`crate::props`]) centered at `at`.
fn model(commands: &mut Commands, kind: PropKind, seed: u64, at: DVec3, yaw: f32) -> Entity {
    let (t, g) = placed(at, yaw);
    commands.spawn((t, g, PropModel::new(kind, seed))).id()
}

fn cuboid(x: f32, y: f32, z: f32) -> Shape {
    Shape::Cuboid { size: [x, y, z] }
}

/// Build the destruction showcase around `origin` (ground level). The breach range faces −Z.
pub fn spawn_demo(commands: &mut Commands, origin: DVec3) {
    let o = origin;
    let yaw = std::f32::consts::PI;
    spawn_fortress(commands, o);
    // The range: a gravel pad, concrete pillars between the bays and a steel lintel on top.
    let (t, g) = placed(o + DVec3::new(ROW_MID, 0.0, ROW_Z - 1.0), 0.0);
    commands.spawn((
        Name::new("Range gravel"),
        t,
        g,
        GravelPad { size: [59.0, 9.0] },
    ));
    for k in 0..=BAYS.len() {
        let x = BAYS[0] + 3.4 - k as f64 * 6.8;
        model(
            commands,
            PropKind::ConcretePillar {
                size: [0.7, 3.8, 0.7],
            },
            k as u64,
            o + DVec3::new(x, 1.9, ROW_Z),
            0.0,
        );
    }
    model(
        commands,
        PropKind::SteelIBeam {
            length: 6.8 * BAYS.len() as f32 + 0.7,
            depth: 0.4,
            end_plates: true,
        },
        1,
        o + DVec3::new(ROW_MID, 4.0, ROW_Z),
        0.0,
    );

    let wall = |commands: &mut Commands, bay: usize, name: &str, w: DestructibleWall| {
        let (t, g) = placed(o + DVec3::new(BAYS[bay], 0.0, ROW_Z), yaw);
        commands.spawn((Name::new(name.to_string()), t, g, w)).id()
    };
    let a1 = wall(
        commands,
        0,
        "Drywall (bullets)",
        DestructibleWall::soft(WALL_W, WALL_H).with_seed(11),
    );
    // Bay 2: a doorway and a window (wall-local x is mirrored by the yaw).
    let door = (Vec2::new(-1.4, 0.0), Vec2::new(-0.2, 2.2));
    let window = (Vec2::new(0.6, 1.0), Vec2::new(1.9, 2.1));
    let a2 = wall(
        commands,
        1,
        "Drywall (barricades)",
        DestructibleWall::soft(WALL_W, WALL_H)
            .with_seed(12)
            .with_paint([0.55, 0.6, 0.62])
            .with_opening(door.0, door.1)
            .with_opening(window.0, window.1),
    );
    let a3 = wall(
        commands,
        2,
        "Reinforced wall (thermite)",
        DestructibleWall::reinforced(WALL_W, WALL_H).with_seed(13),
    );
    let b1 = wall(
        commands,
        3,
        "Reinforced wall (live thermite)",
        DestructibleWall::reinforced(WALL_W, WALL_H).with_seed(14),
    );
    let a4 = wall(
        commands,
        4,
        "Reinforced wall (explosive)",
        DestructibleWall::reinforced(WALL_W, WALL_H).with_seed(15),
    );
    let b2 = wall(
        commands,
        5,
        "Drywall (live)",
        DestructibleWall::soft(WALL_W, WALL_H)
            .with_seed(16)
            .with_paint([0.62, 0.52, 0.42]),
    );
    let a5 = wall(
        commands,
        6,
        "Brick wall (blast)",
        DestructibleWall::masonry(DestructibleMaterial::Brick, WALL_W, WALL_H, 0.22).with_seed(17),
    );
    let b3 = wall(
        commands,
        7,
        "Drywall (reinforced live)",
        DestructibleWall::soft(WALL_W, WALL_H)
            .with_seed(18)
            .with_paint([0.6, 0.63, 0.6]),
    );

    // Bay 2: barricades in the doorway and the window.
    let local = |bay: usize, x: f32, y: f32, z: f32| {
        let (t, _) = placed(o + DVec3::new(BAYS[bay], 0.0, ROW_Z), yaw);
        GlobalTransform::from(t).transform_point(DVec3::new(x as f64, y as f64, z as f64))
    };
    let mut barricades = Vec::new();
    for (name, (lo, hi), seed) in [
        ("Door barricade", door, 21),
        ("Window barricade", window, 22),
    ] {
        let size = hi - lo;
        let (t, g) = placed(local(1, (lo.x + hi.x) * 0.5, lo.y, 0.0), yaw);
        let b = DestructibleWall::barricade(size.x, size.y)
            .with_seed(seed)
            .with_anchors(WallAnchors::SIDES);
        barricades.push(commands.spawn((Name::new(name), t, g, b)).id());
        // A painted wooden frame around the opening.
        let trim = mat(0xe8e4da, 0.6);
        let cx = (lo.x + hi.x) * 0.5;
        prop(
            commands,
            cuboid(size.x + 0.2, 0.1, 0.18),
            trim,
            local(1, cx, hi.y + 0.05, 0.0),
            yaw,
        );
        for x in [lo.x - 0.05, hi.x + 0.05] {
            prop(
                commands,
                cuboid(0.1, size.y, 0.18),
                trim,
                local(1, x, lo.y + size.y * 0.5, 0.0),
                yaw,
            );
        }
        if lo.y > 0.0 {
            prop(
                commands,
                cuboid(size.x + 0.3, 0.08, 0.26),
                trim,
                local(1, cx, lo.y - 0.04, 0.0),
                yaw,
            );
        }
    }

    let house = spawn_breach_house(commands, o);
    let targets = DemoTargets {
        thermite_wall: Some(b1),
        soft_wall: Some(b2),
        cliff: Some(spawn_cliff(commands, o)),
        arch: Some(spawn_arch(commands, o)),
        tower: Vec::new(),
        tower_posts: Vec::new(),
        gate: spawn_gate(commands, o),
        props: Vec::new(),
        roof: house.roof,
        hatch: Some(house.hatch),
        steel_hatch: Some(house.steel_hatch),
        reinforced_wall: Some(b3),
    };
    let (tower, posts) = spawn_tower(commands, o);
    crate::barn::spawn_barn(commands, o + DVec3::from_array(crate::barn::BARN), 0.0);
    let mut targets = targets;
    targets.tower = tower;
    targets.tower_posts = posts;

    let demo = commands
        .spawn((
            Name::new("Destruction demo"),
            DestructionDemo {
                origin: o,
                targets,
                ..DestructionDemo::default()
            },
        ))
        .id();
    let ids = Aftermath {
        demo,
        origin: o,
        a1,
        a2,
        door: barricades[0],
        a3,
        a4,
        a5,
    };
    commands.queue(move |world: &mut World| apply_aftermath(world, ids));
}

/// Board thickness of the range's wooden crates, meters.
const CRATE_WALL: f32 = 0.025;

fn spawn_props(commands: &mut Commands, o: DVec3) -> Vec<Entity> {
    let mut out = Vec::new();
    let x0 = BAYS[5];
    for (k, (dx, dz, s)) in [
        (-1.4, -1.6, 0.8f32),
        (-0.4, -2.0, 0.7),
        (-1.3, -1.7, 0.6),
        (1.4, -1.8, 0.9),
    ]
    .into_iter()
    .enumerate()
    {
        let y = if k == 2 { 0.8 + 0.3 } else { s as f64 * 0.5 };
        let (t, g) = placed(o + DVec3::new(x0 + dx, y, ROW_Z + dz), 0.3 * k as f32);
        out.push(
            commands
                .spawn((
                    Name::new("Crate"),
                    t,
                    g,
                    PropModel::new(PropKind::WoodenCrate { size: [s, s, s] }, k as u64),
                    Fracturable {
                        shape: FractureShape::Crate {
                            half_extents: Vec3::splat(s * 0.5),
                            wall: CRATE_WALL,
                        },
                        material: DestructibleMaterial::Wood,
                        break_energy_j: 2500.0,
                        pieces: 8,
                        color: Some(linear(0x8a6238)),
                        ..Fracturable::default()
                    },
                ))
                .id(),
        );
    }
    for dx in [-3.4, 3.4] {
        let (t, g) = placed(o + DVec3::new(x0 + dx, 1.3, ROW_Z - 2.2), 0.0);
        out.push(
            commands
                .spawn((
                    Name::new("Stone column"),
                    t,
                    g,
                    PropModel::new(
                        PropKind::StoneColumn {
                            height: 2.6,
                            radius: 0.3,
                        },
                        (dx > 0.0) as u64,
                    ),
                    Fracturable {
                        shape: FractureShape::Column {
                            radius: 0.3,
                            half_height: 1.3,
                            sides: 12,
                        },
                        material: DestructibleMaterial::Stone,
                        break_energy_j: 9000.0,
                        pieces: 10,
                        color: Some(linear(0xb9b2a4)),
                        ..Fracturable::default()
                    },
                ))
                .id(),
        );
    }
    out
}

/// The fortress the range stands in: curtain walls with a stair to each wall walk, a
/// breach in the east wall, a twin-towered gatehouse in the south wall and a tower on each
/// corner (each with its own top). Set dressing only; nothing here breaks. It stays inside
/// the zone's 60 m pad.
fn spawn_fortress(commands: &mut Commands, o: DVec3) {
    use std::f32::consts::{FRAC_PI_2, PI};
    let (west, east, south, north) = (-47.0, 44.0, -31.0, 30.0);
    let mut arch = |name: &str, kind: ArchKind, seed: u64, x: f64, z: f64, yaw: f32| {
        let (t, g) = placed(o + DVec3::new(x, 0.0, z), yaw);
        commands.spawn((
            Name::new(name.to_string()),
            t,
            g,
            ArchModel::new(kind, seed),
        ));
    };
    let wall = |length: f64| ArchKind::CurtainWall {
        length: length as f32,
        height: 8.5,
        thickness: 3.0,
    };
    // Outer faces look away from the courtyard.
    arch(
        "North wall",
        wall(east - west),
        11,
        (west + east) * 0.5,
        north,
        PI,
    );
    arch(
        "West wall",
        wall(north - south),
        12,
        west,
        (south + north) * 0.5,
        FRAC_PI_2,
    );
    let (gap_s, gap_n) = (-6.0, 10.0);
    arch(
        "East wall",
        wall(gap_s - south),
        13,
        east,
        (south + gap_s) * 0.5,
        -FRAC_PI_2,
    );
    arch(
        "East wall breach",
        ArchKind::RuinedWall {
            length: (gap_n - gap_s) as f32,
            height: 8.5,
        },
        14,
        east,
        (gap_s + gap_n) * 0.5,
        -FRAC_PI_2,
    );
    arch(
        "East wall",
        wall(north - gap_n),
        15,
        east,
        (gap_n + north) * 0.5,
        -FRAC_PI_2,
    );
    let gate = 8.0;
    arch(
        "South wall",
        wall(-gate - west),
        16,
        (west - gate) * 0.5,
        south,
        0.0,
    );
    arch(
        "South wall",
        wall(east - gate),
        17,
        (east + gate) * 0.5,
        south,
        0.0,
    );
    arch(
        "Gatehouse",
        ArchKind::Gatehouse {
            width: 16.0,
            height: 13.0,
            depth: 10.0,
        },
        18,
        0.0,
        south,
        0.0,
    );
    for (k, (x, z, radius, height, top)) in [
        (west, north, 4.2, 13.0, TowerTop::SlateRoof),
        (east, north, 4.2, 14.0, TowerTop::Hoarding),
        (west, south, 3.8, 12.0, TowerTop::Crenellated),
        (east, south, 3.8, 11.0, TowerTop::ShingleRoof),
    ]
    .into_iter()
    .enumerate()
    {
        arch(
            "Corner tower",
            ArchKind::RoundTower {
                radius,
                height,
                top,
            },
            20 + k as u64,
            x,
            z,
            0.0,
        );
    }
}

fn spawn_cliff(commands: &mut Commands, o: DVec3) -> Entity {
    let (t, g) = placed(o + DVec3::from_array(CLIFF), 0.0);
    let mut v = VoxelVolume::cliff(26.0, 12.0, 9.0, 0.25).with_seed(7);
    v.overhang = 2.4;
    commands.spawn((Name::new("Voxel cliff"), t, g, v)).id()
}

fn spawn_arch(commands: &mut Commands, o: DVec3) -> Entity {
    let (t, g) = placed(o + DVec3::from_array(ARCH), 0.35);
    let v = VoxelVolume::arch(12.0, 8.0, 4.0, 0.2).with_seed(5);
    commands.spawn((Name::new("Rock arch"), t, g, v)).id()
}

fn piece(
    commands: &mut Commands,
    name: &str,
    at: DVec3,
    rotation: Quat,
    size: Vec3,
    material: DestructibleMaterial,
    look: StandardMaterial,
    tint: bool,
) -> Entity {
    let t = Transform::from_translation(at).with_rotation(rotation);
    let mut p = StructuralPiece::new(material, size);
    if material == DestructibleMaterial::Wood {
        p.break_energy_j = 6000.0;
    }
    let mut e = commands.spawn((Name::new(name.to_string()), t, GlobalTransform::from(t), p));
    // A stable seed per spot.
    let seed = (at.x * 7.0 + at.y * 13.0 + at.z * 17.0).abs() as u64;
    if material == DestructibleMaterial::Wood {
        // Sawn timber, its grain along the piece.
        e.insert(PropModel::new(
            PropKind::Timber {
                size: size.to_array(),
            },
            seed,
        ));
    } else if material == DestructibleMaterial::Stone {
        e.insert(PropModel::new(
            PropKind::StoneBlock {
                size: size.to_array(),
            },
            seed,
        ));
    } else {
        e.insert(MeshRenderer::new(cuboid(size.x, size.y, size.z), look));
    }
    if tint {
        e.insert(ShowSupport {
            on: false,
            children: true,
        });
    }
    e.id()
}

/// A two-storey wooden watchtower with knee braces, a board deck, a railed cantilevered
/// balcony and a shingled roof (Valheim build-mode colors).
fn spawn_tower(commands: &mut Commands, o: DVec3) -> (Vec<Entity>, Vec<Entity>) {
    let base = o + DVec3::from_array(TOWER);
    let wood = mat(0x7a5634, 0.85);
    let w = DestructibleMaterial::Wood;
    let mut all = Vec::new();
    let mut posts = Vec::new();
    let p = |x: f64, y: f64, z: f64| base + DVec3::new(x, y, z);
    for (x, z) in [(1.6, -1.6), (1.6, 1.6), (-1.6, -1.6), (-1.6, 1.6)] {
        let e = piece(
            commands,
            "Post",
            p(x, 1.5, z),
            Quat::IDENTITY,
            Vec3::new(0.22, 3.0, 0.22),
            w,
            wood,
            true,
        );
        posts.push(e);
        all.push(e);
        all.push(piece(
            commands,
            "Upper post",
            p(x, 4.3, z),
            Quat::IDENTITY,
            Vec3::new(0.2, 2.6, 0.2),
            w,
            wood,
            true,
        ));
    }
    for (y, size) in [(3.1, 0.2), (5.7, 0.18)] {
        for s in [-1.6, 1.6] {
            all.push(piece(
                commands,
                "Beam",
                p(0.0, y, s),
                Quat::IDENTITY,
                Vec3::new(3.6, size, size),
                w,
                wood,
                true,
            ));
            all.push(piece(
                commands,
                "Beam",
                p(s, y, 0.0),
                Quat::IDENTITY,
                Vec3::new(size, size, 3.6),
                w,
                wood,
                true,
            ));
        }
    }
    // Knee braces from each post up to the beams it carries, on all four faces.
    let quarter = std::f32::consts::FRAC_PI_4;
    let brace = Vec3::new(0.12, 1.0, 0.12);
    for s in [-1.6, 1.6] {
        for (along, lean) in [(1.25, quarter), (-1.25, -quarter)] {
            all.push(piece(
                commands,
                "Knee brace",
                p(along, 2.65, s),
                Quat::from_rotation_z(lean),
                brace,
                w,
                wood,
                true,
            ));
            all.push(piece(
                commands,
                "Knee brace",
                p(s, 2.65, along),
                Quat::from_rotation_x(-lean),
                brace,
                w,
                wood,
                true,
            ));
        }
    }
    // Board decking (battens underneath) and a balcony sticking out over the range (−Z).
    let deck = Quat::from_rotation_x(-std::f32::consts::FRAC_PI_2);
    for (k, (x, z)) in [
        (-0.9, -0.9),
        (0.9, -0.9),
        (-0.9, 0.9),
        (0.9, 0.9),
        (-0.9, -2.7),
        (0.9, -2.7),
    ]
    .into_iter()
    .enumerate()
    {
        let size = Vec3::new(1.8, 1.8, 0.12);
        let e = piece(commands, "Floor", p(x, 3.26, z), deck, size, w, wood, true);
        commands.entity(e).insert(PropModel::new(
            PropKind::BoardWall {
                size: size.to_array(),
            },
            20 + k as u64,
        ));
        all.push(e);
    }
    // Hand and mid rails round the lookout and the balcony, on two balcony stanchions.
    for x in [-1.75, 1.75] {
        all.push(piece(
            commands,
            "Stanchion",
            p(x, 3.85, -3.5),
            Quat::IDENTITY,
            Vec3::new(0.1, 1.05, 0.1),
            w,
            wood,
            true,
        ));
    }
    for y in [3.85, 4.35] {
        let rails = [
            (p(0.0, y, 1.6), Vec3::new(3.2, 0.1, 0.07)),
            (p(-1.6, y, 0.0), Vec3::new(0.07, 0.1, 3.2)),
            (p(1.6, y, 0.0), Vec3::new(0.07, 0.1, 3.2)),
            (p(0.0, y, -3.5), Vec3::new(3.4, 0.1, 0.07)),
            (p(-1.75, y, -2.6), Vec3::new(0.07, 0.1, 1.75)),
            (p(1.75, y, -2.6), Vec3::new(0.07, 0.1, 1.75)),
        ];
        for (at, size) in rails {
            all.push(piece(
                commands,
                "Rail",
                at,
                Quat::IDENTITY,
                size,
                w,
                wood,
                true,
            ));
        }
    }
    // Roof: two shingled panels on the upper beams (each panel's X runs down its slope).
    for s in [-1.0f32, 1.0] {
        let rot =
            Quat::from_rotation_x(s * 0.55) * Quat::from_rotation_y(-std::f32::consts::FRAC_PI_2);
        let size = Vec3::new(2.3, 0.12, 4.0);
        let e = piece(
            commands,
            "Roof",
            p(0.0, 6.3, s as f64 * 1.0),
            rot,
            size,
            w,
            wood,
            true,
        );
        let look = PropKind::ShinglePanel {
            size: size.to_array(),
            eave_at_plus_x: s > 0.0,
        };
        commands
            .entity(e)
            .insert(PropModel::new(look, if s > 0.0 { 8 } else { 7 }));
        all.push(e);
    }
    (all, posts)
}

/// A stone gate: two piers of blocks carrying a round arch of seven voussoirs, spandrels over
/// the piers, a capping course and merlons (stone: strong upward, weak sideways, so when one
/// pier goes, half the arch hangs from the other like a ruin).
fn spawn_gate(commands: &mut Commands, o: DVec3) -> Vec<Entity> {
    let base = o + DVec3::from_array(GATE);
    let mut out = Vec::new();
    let mut rng = Rng::new(9);
    let s = DestructibleMaterial::Stone;
    let mut block =
        |commands: &mut Commands, x: f64, y: f64, turn: f32, size: Vec3, rng: &mut Rng| {
            let shade = rng.range(0.85, 1.08);
            let c = linear(0x9a9386);
            let look = StandardMaterial::color(c[0] * shade, c[1] * shade, c[2] * shade * 0.98)
                .with_roughness(0.9);
            out.push(piece(
                commands,
                "Stone block",
                base + DVec3::new(x, y, 0.0),
                Quat::from_rotation_z(turn),
                size,
                s,
                look,
                false,
            ));
        };
    for x in [-1.6, 1.6] {
        for k in 0..4 {
            block(
                commands,
                x,
                0.5 + k as f64,
                0.0,
                Vec3::new(1.2, 0.98, 1.2),
                &mut rng,
            );
        }
    }
    // The arch springs from the piers' inner edges at 4 m: 1 m intrados, 0.5 m deep rings.
    let (spring, inner, ring) = (4.0, 1.0, 0.5);
    let n = 7;
    for k in 0..n {
        let a = (k as f32 + 0.5) / n as f32 * std::f32::consts::PI;
        let r = inner + ring * 0.5;
        // Tangential width meets the next voussoir at the extrados.
        let width = 2.0 * (inner + ring) * (std::f32::consts::PI / n as f32 * 0.5).tan();
        block(
            commands,
            (r * a.cos()) as f64,
            spring + (r * a.sin()) as f64,
            a - std::f32::consts::FRAC_PI_2,
            Vec3::new(width * 0.92, ring * 0.98, 1.1),
            &mut rng,
        );
    }
    // Spandrels over the piers, outside the arch's extrados.
    for side in [-1.0, 1.0] {
        for (x, y, size) in [
            (1.85, 4.37, Vec3::new(0.68, 0.72, 1.1)),
            (1.85, 5.12, Vec3::new(0.68, 0.72, 1.1)),
            (1.3, 5.26, Vec3::new(0.38, 0.46, 1.1)),
        ] {
            block(commands, side * x, y, 0.0, size, &mut rng);
        }
    }
    for x in [-1.65, -0.55, 0.55, 1.65] {
        block(
            commands,
            x,
            5.74,
            0.0,
            Vec3::new(1.08, 0.46, 1.16),
            &mut rng,
        );
    }
    for x in [-2.0, -1.2, 1.2, 2.0] {
        block(commands, x, 6.23, 0.0, Vec3::new(0.45, 0.5, 0.9), &mut rng);
    }
    out
}

/// The breach house's breakable parts.
struct House {
    roof: Vec<Entity>,
    hatch: Entity,
    steel_hatch: Entity,
}

/// The breach house: brick walls on three sides with a window in each side wall, an open
/// front on concrete pillars and a steel beam, joists across and a timber roof deck in four
/// bays, a framed and rimmed hatch in each middle bay (one wooden, one steel) with a steel
/// ladder under it, a concrete floor and a wooden stair up the outside of the back wall.
fn spawn_breach_house(commands: &mut Commands, o: DVec3) -> House {
    use std::f32::consts::{FRAC_PI_2, PI};
    let h = o + DVec3::from_array(HOUSE);
    let at = |x: f64, y: f64, z: f64| h + DVec3::new(x, y, z);
    let wall_h = JOIST_TOP as f32;
    let brick = |w: f32, seed: u64| {
        DestructibleWall::masonry(DestructibleMaterial::Brick, w, wall_h, 0.22)
            .with_seed(seed)
            .with_anchors(WallAnchors::ALL)
    };
    let (t, g) = placed(at(0.0, 0.0, -2.91), PI);
    commands.spawn((Name::new("Breach house back wall"), t, g, brick(6.84, 31)));
    // Side walls with a window each (a concrete lintel and sill; bricks are 7.5 cm courses).
    for (k, side) in [1.0f64, -1.0].into_iter().enumerate() {
        let x = side * 3.31;
        let wall =
            brick(5.6, 32 + k as u64).with_opening(Vec2::new(-0.6, 1.05), Vec2::new(0.6, 2.25));
        let (t, g) = placed(at(x, 0.0, 0.0), side as f32 * FRAC_PI_2);
        let name = if side > 0.0 { "east" } else { "west" };
        commands.spawn((Name::new(format!("Breach house {name} wall")), t, g, wall));
        for (size, y, out, seed) in [
            ([0.25, 0.15, 1.5], 2.175, 0.0, 34),
            ([0.32, 0.06, 1.35], 1.02, 0.05, 36),
        ] {
            model(
                commands,
                PropKind::ConcreteSlab { size },
                seed + k as u64,
                at(x + side * out, y, 0.0),
                0.0,
            );
        }
    }
    model(
        commands,
        PropKind::ConcreteSlab {
            size: [6.4, 0.12, 6.0],
        },
        3,
        at(0.0, -0.04, 0.2),
        0.0,
    );
    // The open front: two pillars and a steel beam under the joist ends.
    for (k, x) in [-3.31, 3.31].into_iter().enumerate() {
        model(
            commands,
            PropKind::ConcretePillar {
                size: [0.44, 2.7, 0.44],
            },
            40 + k as u64,
            at(x, 1.35, 2.95),
            0.0,
        );
    }
    model(
        commands,
        PropKind::SteelIBeam {
            length: 7.06,
            depth: 0.3,
            end_plates: false,
        },
        2,
        at(0.0, 2.85, 2.95),
        0.0,
    );
    // Joists from the back wall over the beam, a framed hatch opening in each middle bay.
    let joist_len = DECK_FRONT + 2.8;
    for (k, x) in [-1.71, 0.0, 1.71].into_iter().enumerate() {
        model(
            commands,
            PropKind::Timber {
                size: [0.1, 0.24, joist_len as f32],
            },
            50 + k as u64,
            at(x, JOIST_TOP - 0.12, DECK_FRONT - joist_len * 0.5),
            0.0,
        );
    }
    let half = HATCH_M as f64 * 0.5;
    for (k, [hx, hz]) in [HATCH, STEEL_HATCH].into_iter().enumerate() {
        let seed = 60 + 4 * k as u64;
        let header = (BAY_W - 0.1) as f64;
        for (j, dz) in [-half, half].into_iter().enumerate() {
            model(
                commands,
                PropKind::Timber {
                    size: [header as f32, 0.24, 0.1],
                },
                seed + j as u64,
                at(hx, JOIST_TOP - 0.12, hz + dz),
                0.0,
            );
        }
        for (j, dx) in [-half, half].into_iter().enumerate() {
            model(
                commands,
                PropKind::Timber {
                    size: [0.1, 0.2, HATCH_M - 0.1],
                },
                seed + 2 + j as u64,
                at(hx + dx, JOIST_TOP - 0.1, hz),
                0.0,
            );
        }
        // The ladder hangs off the back header, its brackets bolted to its face. It is climbed
        // facing the header and stepped off over it onto the deck.
        let ladder_h = 3.18;
        let ladder_at = at(hx, 0.02 + ladder_h as f64 * 0.5, hz - half + 0.05 + 0.12);
        let ladder = model(
            commands,
            PropKind::SteelLadder { height: ladder_h },
            k as u64,
            ladder_at,
            0.0,
        );
        let (low, high) = props::ladder_rungs(ladder_h);
        let feet = |y: f32| Vec3::new(0.0, y - ladder_h * 0.5, props::LADDER_STILE_Z + 0.2);
        let deck = at(hx, JOIST_TOP + DECK_T as f64, hz - half - 0.35) - ladder_at;
        commands.entity(ladder).insert(
            Climbable::ladder(feet(low), feet(high), Vec3::NEG_Z).with_exit(deck.as_vec3()),
        );
    }
    // The roof deck: one board panel per bay, the hatch openings left out.
    let deck_y = JOIST_TOP + DECK_T as f64 * 0.5;
    let rel = |x: f64, z: f64, bay: f64| Vec2::new((x - bay) as f32, (DECK_FRONT - z) as f32);
    let mut bays = Vec::new();
    for (k, x) in DECK_BAYS.into_iter().enumerate() {
        let mut deck = DestructibleWall::floor(BAY_W, DECK_LEN).with_seed(70 + k as u64);
        for [hx, hz] in [HATCH, STEEL_HATCH] {
            if (hx - x).abs() < 0.1 {
                let c = rel(hx, hz, x);
                let r = Vec2::splat(HATCH_M * 0.5);
                deck = deck.with_opening(c - r, c + r);
            }
        }
        let t = Transform::from_translation(at(x, deck_y, DECK_FRONT))
            .with_rotation(DestructibleWall::flat());
        bays.push(
            commands
                .spawn((
                    Name::new(format!("Breach house roof bay {}", k + 1)),
                    t,
                    GlobalTransform::from(t),
                    deck,
                ))
                .id(),
        );
    }
    // Hatch covers resting on the frames: wooden boards across the deck's, a steel plate.
    let (hx, hz) = (HATCH[0], HATCH[1]);
    let t = Transform::from_translation(at(hx + half, JOIST_TOP + 0.03, hz))
        .with_rotation(Quat::from_rotation_y(FRAC_PI_2) * DestructibleWall::flat());
    let hatch = commands
        .spawn((
            Name::new("Wooden hatch"),
            t,
            GlobalTransform::from(t),
            DestructibleWall::hatch(HATCH_M, HATCH_M).with_seed(80),
        ))
        .id();
    let (sx, sz) = (STEEL_HATCH[0], STEEL_HATCH[1]);
    let t = Transform::from_translation(at(sx, JOIST_TOP + 0.025, sz + half))
        .with_rotation(DestructibleWall::flat());
    let steel_hatch = commands
        .spawn((
            Name::new("Steel hatch"),
            t,
            GlobalTransform::from(t),
            DestructibleWall::steel_hatch(HATCH_M, HATCH_M).with_seed(81),
        ))
        .id();
    // Steel rims around the hatch openings, screwed onto the boards.
    let deck_top = JOIST_TOP + DECK_T as f64;
    for (k, [x, z]) in [HATCH, STEEL_HATCH].into_iter().enumerate() {
        let rim = PropKind::HatchRim {
            size: [HATCH_M, HATCH_M],
        };
        let y = deck_top + rim.size().y as f64 * 0.5;
        model(commands, rim, k as u64, at(x, y, z), 0.0);
    }
    // The stair up the outside of the back wall, climbing west to the deck's back edge.
    model(
        commands,
        PropKind::WoodStair {
            size: [1.0, deck_top as f32, 5.0],
        },
        5,
        at(0.0, deck_top * 0.5, -3.57),
        FRAC_PI_2,
    );
    // Guard railings round the roof, open at the stair's head, and up the stair's open side.
    let rail = |length: f32, rise: f32, side_mount: bool| {
        let kind = PropKind::PipeRailing {
            length,
            rise,
            side_mount,
        };
        (
            kind,
            props::pipe_railing_foot(length, rise, side_mount) as f64,
        )
    };
    let edge = 0.1;
    let (px, pz) = (3.42 - edge, DECK_FRONT - edge);
    let back = DECK_FRONT - DECK_LEN as f64 + edge;
    let flange = 0.065;
    let gap = 0.3;
    let side_len = pz - gap - back;
    let back_x0 = -1.75;
    let back_x1 = px - gap;
    for (k, (length, x, z, yaw)) in [
        (2.0 * px, 0.0, pz, PI),
        (side_len, px, back + side_len * 0.5, -FRAC_PI_2),
        (side_len, -px, back + side_len * 0.5, FRAC_PI_2),
        (back_x1 - back_x0, (back_x0 + back_x1) * 0.5, back, 0.0),
    ]
    .into_iter()
    .enumerate()
    {
        let (kind, foot) = rail(length as f32 + 2.0 * flange, 0.0, false);
        model(
            commands,
            kind,
            90 + k as u64,
            at(x, deck_top - foot, z),
            yaw,
        );
    }
    // The stair's: palms bolted to its outer stringer, climbing west with it.
    let (kind, foot) = rail(5.0, deck_top as f32, true);
    model(
        commands,
        kind,
        94,
        at(
            0.0,
            deck_top * 0.5 - foot,
            -4.07 - props::RAIL_PALM_BACK as f64,
        ),
        PI,
    );
    House {
        roof: vec![bays[2], bays[3]],
        hatch,
        steel_hatch,
    }
}

/// Entities and openings the aftermath damages.
#[derive(Debug, Clone, Copy)]
struct Aftermath {
    demo: Entity,
    origin: DVec3,
    a1: Entity,
    a2: Entity,
    door: Entity,
    a3: Entity,
    a4: Entity,
    a5: Entity,
}

fn wall_world(world: &World, wall: Entity, local: Vec3) -> DVec3 {
    let t = world.get::<Transform>(wall).copied().unwrap_or_default();
    GlobalTransform::from(t).transform_point(local.as_dvec3())
}

/// Aftermath bays waiting for [`crate::DestructionAssets`] to load (their rubble needs it).
#[derive(Resource, Default)]
struct PendingAftermath(Vec<Aftermath>);

/// Wreck the aftermath bays at once and lay their rubble on the ground (once the assets have
/// loaded, if they are loading).
fn apply_aftermath(world: &mut World, ids: Aftermath) {
    crate::assets::ensure_assets(world);
    if world.contains_resource::<crate::assets::AssetsLoading>() {
        world.get_resource_or_init::<PendingAftermath>().0.push(ids);
        return;
    }
    let _ = world.run_system_once(crate::sync::init_walls_system);
    let mut rng = Rng::new(42);
    let mut hits = Vec::new();
    // Bay 1: three rifle bursts, a vault hole kicked through low down, a breach charge hole.
    for (cx, cy) in [(-1.7f32, 1.5f32), (0.2, 2.1), (1.6, 1.2)] {
        let shooter = Vec3::new(cx * 0.3 + 0.5, 1.6, 9.0);
        for _ in 0..12 {
            let d = rng.in_unit_disk() * 0.38;
            let aim = Vec3::new(cx + d.x, cy + d.y, 0.0);
            let dir = (aim - shooter).normalize();
            hits.push(Hit::Trace {
                from: wall_world(world, ids.a1, shooter),
                to: wall_world(world, ids.a1, aim + dir * 4.0),
                energy_j: 1400.0,
                caliber: 0.008,
            });
        }
    }
    for k in 0..7 {
        let p = Vec3::new(
            0.7 + (k % 3) as f32 * 0.22,
            0.35 + (k / 3) as f32 * 0.25,
            0.3,
        );
        hits.push(Hit::Impact {
            point: wall_world(world, ids.a1, p),
            dir: world_dir(world, ids.a1, Vec3::NEG_Z),
            energy_j: 2500.0,
            min_radius: 0.14,
            melee: true,
            target: Some(ids.a1),
        });
    }
    hits.push(Hit::Blast {
        center: wall_world(world, ids.a1, Vec3::new(-0.9, 0.9, 0.12)),
        energy_j: 1.6e5,
    });
    // Bay 2: the door barricade shot up and kicked open in the middle; the window one intact.
    let door_mid = Vec2::new(-0.8, 1.1);
    for k in 0..8 {
        let p = Vec3::new(
            door_mid.x + ((k % 4) as f32 - 1.5) * 0.2,
            0.9 + (k / 4) as f32 * 0.3,
            0.3,
        );
        hits.push(Hit::Impact {
            point: wall_world(world, ids.a2, p),
            dir: world_dir(world, ids.a2, Vec3::NEG_Z),
            energy_j: 2500.0,
            min_radius: 0.12,
            melee: true,
            target: Some(ids.door),
        });
    }
    for _ in 0..10 {
        let d = rng.in_unit_disk();
        let p = Vec3::new(door_mid.x + d.x * 0.5, 1.6 + d.y * 0.4, 0.0);
        hits.push(Hit::Trace {
            from: wall_world(world, ids.a2, p + Vec3::new(0.3, 0.1, 8.0)),
            to: wall_world(world, ids.a2, p - Vec3::new(0.0, 0.0, 4.0)),
            energy_j: 1400.0,
            caliber: 0.008,
        });
    }
    // Bay 3: a finished thermite doorway (the panel fell out the far side).
    hits.push(Hit::Cut {
        wall: ids.a3,
        min: Vec2::new(-0.6, 0.0),
        max: Vec2::new(0.6, 2.1),
        front: true,
        melt: false,
        tip: Vec3::ZERO,
    });
    // Bay 5: an explosive charge against reinforced concrete: the bars survive.
    hits.push(Hit::Blast {
        center: wall_world(world, ids.a4, Vec3::new(0.2, 1.3, 0.16)),
        energy_j: 3.5e6,
    });
    for _ in 0..8 {
        let d = rng.in_unit_disk();
        let p = Vec3::new(-1.6 + d.x * 0.5, 2.2 + d.y * 0.3, 0.0);
        hits.push(Hit::Trace {
            from: wall_world(world, ids.a4, p + Vec3::new(0.0, 0.0, 8.0)),
            to: wall_world(world, ids.a4, p - Vec3::new(0.0, 0.0, 2.0)),
            energy_j: 3500.0,
            caliber: 0.012,
        });
    }
    // Bay 7: a heavy blast against brick.
    hits.push(Hit::Blast {
        center: wall_world(world, ids.a5, Vec3::new(0.6, 1.0, 0.35)),
        energy_j: 1.8e6,
    });
    // The breach house's west roof bay: three bursts fired up through it from the room.
    let h = ids.origin + DVec3::from_array(HOUSE);
    for (cx, cz) in [(-2.6f64, -2.0f64), (-2.2, -0.4), (-2.5, 1.3)] {
        let shooter = h + DVec3::new(-1.8, 1.5, cz + 1.4);
        for _ in 0..12 {
            let d = rng.in_unit_disk() * 0.35;
            let aim = h + DVec3::new(cx + d.x as f64, JOIST_TOP, cz + d.y as f64);
            hits.push(Hit::Trace {
                from: shooter,
                to: shooter + (aim - shooter) * 1.6,
                energy_j: 1400.0,
                caliber: 0.008,
            });
        }
    }
    world.resource_mut::<HitQueue>().hits.extend(hits);
    // The aftermath is old damage: its blasts' dust clouds and bangs are long gone, so only
    // what other systems wrote is kept (a cloud per wrecked bay hid the range on arrival).
    let smoke = take_messages::<SmokeEmission>(world);
    let noise = take_messages::<NoiseEvent>(world);
    let _ = world.run_system_once(crate::hits::apply_hits_system);
    put_messages(world, smoke);
    put_messages(world, noise);
    let ground = {
        let g = world.resource::<engine_world::Ground>().clone();
        move |p: DVec3| g.height(p).unwrap_or(0.0)
    };
    let mut pending = world.resource_mut::<PendingDebris>();
    pending.dust.clear();
    pending
        .items
        .retain(|i| !matches!(i.look, crate::debris::DebrisLook::Spark { .. }));
    for item in &mut pending.items {
        settle(item, &ground, &mut rng);
        item.lifetime = Some(FOREVER);
    }
    // Crates and stone columns in front of the live drywall (its charge shatters them),
    // placed after the aftermath blasts so those don't break them first.
    let mut props = Vec::new();
    run_commands(world, |commands| props = spawn_props(commands, ids.origin));
    if let Some(mut d) = world.get_mut::<DestructionDemo>(ids.demo) {
        d.targets.props = props;
    }
}

/// Take every pending `T` message out of `world`.
fn take_messages<T: Message>(world: &mut World) -> Vec<T> {
    world
        .get_resource_mut::<Messages<T>>()
        .map(|mut m| m.drain().collect())
        .unwrap_or_default()
}

/// Replace the pending `T` messages with `kept`, dropping any written since they were taken.
fn put_messages<T: Message>(world: &mut World, kept: Vec<T>) {
    if let Some(mut m) = world.get_resource_mut::<Messages<T>>() {
        m.clear();
        for msg in kept {
            m.write(msg);
        }
    }
}

fn world_dir(world: &World, wall: Entity, local: Vec3) -> Vec3 {
    world
        .get::<Transform>(wall)
        .map(|t| t.rotation * local)
        .unwrap_or(local)
}

fn run_commands(world: &mut World, f: impl FnOnce(&mut Commands)) {
    let mut queue = CommandQueue::default();
    {
        let mut commands = Commands::new(&mut queue, world);
        f(&mut commands);
    }
    queue.apply(world);
}

/// Advance every [`DestructionDemo`]'s live script (FixedUpdate, `SimSet::Gameplay`).
pub fn demo_script_system(world: &mut World) {
    let dt = world.resource::<Time>().delta_secs();
    if dt <= 0.0 {
        return;
    }
    // The demos start once their assets have loaded, with the aftermath bays wrecked first.
    if world.contains_resource::<crate::assets::AssetsLoading>() {
        return;
    }
    if let Some(pending) = world.remove_resource::<PendingAftermath>() {
        for ids in pending.0 {
            apply_aftermath(world, ids);
        }
    }
    let demos: Vec<Entity> = world
        .query_filtered::<Entity, With<DestructionDemo>>()
        .iter(world)
        .collect();
    let steps = timeline();
    for e in demos {
        let Some(mut demo) = world.get::<DestructionDemo>(e).cloned() else {
            continue;
        };
        if crate::area_rests_in(world, demo.origin, DEMO_RADIUS_M) {
            continue;
        }
        demo.time += dt;
        while demo.next < steps.len() && steps[demo.next].0 <= demo.time {
            let step = steps[demo.next].1;
            demo.next += 1;
            run_step(world, &mut demo, step);
        }
        if demo.time >= CYCLE_SECS {
            demo.time -= CYCLE_SECS;
            demo.next = 0;
            demo.cycle += 1;
        }
        if let Some(mut d) = world.get_mut::<DestructionDemo>(e) {
            *d = demo;
        }
    }
}

/// Start every [`DestructionDemo`]'s live script over from the beginning: loose debris sinks,
/// the live walls, cliff, arch, tower and gate are rebuilt. Returns how many demos restarted.
pub fn restart_demos(world: &mut World) -> usize {
    let demos: Vec<Entity> = world
        .query_filtered::<Entity, With<DestructionDemo>>()
        .iter(world)
        .collect();
    for &e in &demos {
        let Some(mut demo) = world.get::<DestructionDemo>(e).cloned() else {
            continue;
        };
        run_step(world, &mut demo, Step::Clear);
        run_step(world, &mut demo, Step::Reset);
        demo.time = 0.0;
        demo.next = 0;
        demo.cycle += 1;
        if let Some(mut d) = world.get_mut::<DestructionDemo>(e) {
            *d = demo;
        }
    }
    demos.len()
}

fn run_step(world: &mut World, demo: &mut DestructionDemo, step: Step) {
    let o = demo.origin;
    let t = demo.targets.clone();
    let seed = demo.cycle as u64 * 977 + 13;
    match step {
        Step::Thermite => {
            if let Some(w) = t.thermite_wall {
                let mut c = BreachCharge::thermite(w, Vec2::new(0.0, 1.0), [1.15, 1.95]);
                c.fuse_secs = 0.6;
                world.spawn((Name::new("Thermite charge"), c));
            }
        }
        Step::PlantCharge => {
            if let Some(w) = t.soft_wall {
                let mut c = BreachCharge::explosive(w, Vec2::new(0.4, 1.1), 4.0e5);
                c.fuse_secs = 3.2;
                world.spawn((Name::new("Breach charge"), c));
            }
        }
        Step::Burst(k) => {
            let Some(w) = t.soft_wall else {
                return;
            };
            let mut rng = Rng::new(seed + k as u64);
            let cx = -1.9 + (k as f32 * 0.9) % 3.6;
            let shooter = Vec3::new(cx * 0.5, 1.6, 10.0);
            for _ in 0..5 {
                let d = rng.in_unit_disk() * 0.3;
                let aim = Vec3::new(cx + d.x, 1.9 + d.y + 0.15 * (k % 3) as f32, 0.0);
                let dir = (aim - shooter).normalize();
                world.write_message(ProjectileTrace {
                    from: wall_world(world, w, shooter),
                    to: wall_world(world, w, aim + dir * 5.0),
                    radius: 0.01,
                    speed: 850.0,
                    shooter: None,
                });
            }
        }
        Step::Dig(k) => {
            let Some(cliff) = t.cliff else {
                return;
            };
            // A pick working into the cliff foot: rows across, getting deeper, then higher.
            let per_row = 8;
            let (col, row) = (k % per_row, k / per_row);
            let x = -5.0 + col as f32 * 1.35;
            let depth = (row % 4) as f32;
            let y = if row < 4 { 1.2 } else { 2.7 };
            let local = Vec3::new(x, y, -3.2 + depth * 1.4);
            let center = world
                .get::<Transform>(cliff)
                .map(|t| GlobalTransform::from(*t).transform_point(local.as_dvec3()))
                .unwrap_or(o);
            world.write_message(TerrainEdit {
                center,
                shape: EditShape::Sphere { radius: 1.3 },
                op: EditOp::Remove,
                surface: None,
                source: None,
            });
        }
        Step::BreakPost(k) => {
            if let Some(&post) = t.tower_posts.get(k)
                && world.get::<StructuralPiece>(post).is_some()
            {
                let at = world
                    .get::<Transform>(post)
                    .map(|t| t.translation)
                    .unwrap_or(o)
                    + DVec3::new(0.0, -0.6, 0.0);
                world.resource_mut::<HitQueue>().hits.push(Hit::Impact {
                    point: at,
                    dir: Vec3::new(-0.6, 0.0, 0.8).normalize(),
                    energy_j: 9000.0,
                    min_radius: 0.12,
                    melee: true,
                    target: Some(post),
                });
            }
        }
        Step::BlastGate => {
            crate::blast(
                world,
                WorldImpulse {
                    center: o + DVec3::from_array(GATE) + DVec3::new(-1.6, 0.6, -0.9),
                    radius: 6.0,
                    strength: 60.0,
                    energy_j: 2.5e6,
                    heat_c: CUT_CHARGE_HEAT_C,
                    source: None,
                },
            );
        }
        Step::BlastArch => {
            if let Some(arch) = t.arch {
                let tr = world.get::<Transform>(arch).copied().unwrap_or_default();
                let g = GlobalTransform::from(tr);
                let foot = Vec3::new(-3.72, 1.0, 0.0);
                crate::blast(
                    world,
                    WorldImpulse {
                        center: g.transform_point((foot + Vec3::new(0.0, 0.0, -1.6)).as_dvec3()),
                        radius: 8.0,
                        strength: 80.0,
                        energy_j: 6.0e6,
                        heat_c: CUT_CHARGE_HEAT_C,
                        source: None,
                    },
                );
                // The blast cuts through the pillar.
                world.write_message(TerrainEdit {
                    center: g.transform_point((foot + Vec3::new(0.0, 0.0, -2.0)).as_dvec3()),
                    shape: EditShape::Capsule {
                        to: tr.rotation * Vec3::new(0.0, 0.6, 4.0),
                        radius: 1.7,
                    },
                    op: EditOp::Remove,
                    surface: None,
                    source: None,
                });
            }
        }
        Step::RoofBurst(k) => {
            // Fired up through the east roof bay from the room below.
            let mut rng = Rng::new(seed + 50 + k as u64);
            let h = o + DVec3::from_array(HOUSE);
            let shooter = h + DVec3::new(2.3 + 0.1 * k as f64, 1.5, 0.3);
            let (cx, cz) = (2.2 + 0.35 * (k % 3) as f64, -2.2 + 0.3 * k as f64);
            for _ in 0..5 {
                let d = rng.in_unit_disk() * 0.25;
                let aim = h + DVec3::new(cx + d.x as f64, JOIST_TOP, cz + d.y as f64);
                world.write_message(ProjectileTrace {
                    from: shooter,
                    to: shooter + (aim - shooter) * 1.6,
                    radius: 0.01,
                    speed: 850.0,
                    shooter: None,
                });
            }
        }
        Step::RoofCharge => {
            if let Some(&roof) = t.roof.first() {
                let [x, z] = ROOF_CHARGE;
                let at = Vec2::new((x - DECK_BAYS[2]) as f32, (DECK_FRONT - z) as f32);
                let mut c = BreachCharge::explosive(roof, at, 6.0e5);
                c.fuse_secs = 2.8;
                world.spawn((Name::new("Roof breach charge"), c));
            }
        }
        Step::ThermiteHatch => {
            if let Some(hatch) = t.steel_hatch {
                let mid = Vec2::new(0.0, HATCH_M * 0.5);
                let mut c = BreachCharge::thermite(hatch, mid, [0.8, 0.8])
                    .with_tip(Vec3::from_array(HATCH_HEAVE));
                c.fuse_secs = 0.6;
                world.spawn((Name::new("Hatch thermite charge"), c));
            }
        }
        Step::SmashHatch(k) => {
            // A sledgehammer from above, a swing into each quarter of the hatch.
            if let Some(hatch) = t.hatch
                && world.get::<WallState>(hatch).is_some()
            {
                let [dx, dz] =
                    [[0.17, -0.15], [-0.17, -0.15], [-0.15, 0.17], [0.16, 0.17]][k as usize % 4];
                let at = o
                    + DVec3::from_array(HOUSE)
                    + DVec3::new(HATCH[0] + dx, JOIST_TOP + 0.08, HATCH[1] + dz);
                world.resource_mut::<HitQueue>().hits.push(Hit::Impact {
                    point: at,
                    dir: Vec3::NEG_Y,
                    energy_j: 9000.0 + 1000.0 * (k / 3) as f32,
                    min_radius: 0.16,
                    melee: true,
                    target: Some(hatch),
                });
            }
        }
        Step::ReinforceBurst(k) => {
            let Some(w) = t.reinforced_wall else {
                return;
            };
            let mut rng = Rng::new(seed + 40 + k as u64);
            let cx = [-0.9f32, 0.6, -1.6, 1.5][k as usize % 4];
            let shooter = Vec3::new(cx * 0.4, 1.6, 9.0);
            for _ in 0..6 {
                let d = rng.in_unit_disk() * 0.3;
                let aim = Vec3::new(cx + d.x, 1.5 + d.y + 0.2 * (k % 2) as f32, 0.0);
                let dir = (aim - shooter).normalize();
                world.write_message(ProjectileTrace {
                    from: wall_world(world, w, shooter),
                    to: wall_world(world, w, aim + dir * 5.0),
                    radius: 0.01,
                    speed: 850.0,
                    shooter: None,
                });
            }
        }
        Step::Reinforce => {
            if let Some(w) = t.reinforced_wall {
                world.write_message(ReinforceRequest { wall: w });
            }
        }
        Step::ReinforceCharge => {
            if let Some(w) = t.reinforced_wall {
                let mut c = BreachCharge::explosive(w, Vec2::new(-0.6, 1.1), 4.0e5);
                c.fuse_secs = 1.5;
                world.spawn((Name::new("Reinforced bay breach charge"), c));
            }
        }
        Step::ReinforceThermite => {
            if let Some(w) = t.reinforced_wall {
                let [x, y] = REINFORCE_THERMITE;
                let mut c = BreachCharge::thermite(w, Vec2::new(x, y), [1.1, 1.9]);
                c.fuse_secs = 0.6;
                world.spawn((Name::new("Reinforced bay thermite"), c));
            }
        }
        Step::SupportView(on) => {
            for &e in &t.tower {
                if let Some(mut s) = world.get_mut::<ShowSupport>(e) {
                    s.on = on;
                }
            }
        }
        Step::Clear => {
            // The storehouse keeps its own (longer) loop.
            let mut q = world.query_filtered::<&mut Debris, Without<crate::barn::BarnPiece>>();
            for mut d in q.iter_mut(world) {
                if d.lifetime < FOREVER * 0.5 {
                    d.age = d.age.max(d.lifetime);
                }
            }
        }
        Step::Reset => {
            if let Some(w) = t.reinforced_wall {
                remove_reinforcement(world, w);
            }
            let live = [
                t.thermite_wall,
                t.soft_wall,
                t.hatch,
                t.steel_hatch,
                t.reinforced_wall,
            ];
            for w in live.into_iter().flatten().chain(t.roof.iter().copied()) {
                if let Some(mut wall) = world.get_mut::<DestructibleWall>(w) {
                    wall.set_changed();
                }
            }
            for v in [t.cliff, t.arch].into_iter().flatten() {
                if let Some(mut vol) = world.get_mut::<VoxelVolume>(v) {
                    vol.set_changed();
                }
            }
            let charges: Vec<Entity> = world
                .query_filtered::<Entity, With<BreachCharge>>()
                .iter(world)
                .collect();
            let stale: Vec<Entity> = t
                .tower
                .iter()
                .chain(&t.gate)
                .chain(&t.props)
                .copied()
                .filter(|e| world.get::<Debris>(*e).is_none())
                .chain(charges)
                .collect();
            for e in stale {
                if let Ok(ec) = world.get_entity_mut(e) {
                    ec.despawn();
                }
            }
            run_commands(world, |commands| {
                let (tower, posts) = spawn_tower(commands, o);
                demo.targets.tower = tower;
                demo.targets.tower_posts = posts;
                demo.targets.gate = spawn_gate(commands, o);
                demo.targets.props = spawn_props(commands, o);
            });
        }
    }
}
