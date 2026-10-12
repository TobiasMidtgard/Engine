//! The destruction showcase: a siege breach range plus Valheim-style building and rock, built
//! by [`spawn_demo`] around an origin.
//!
//! **Breach range** (a row of bays facing −Z, toward the viewer): each bay is one test case.
//! Five are an *aftermath*, already wrecked on the first frame with their rubble lying on the
//! ground; two are *live* and get destroyed again every cycle:
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
//!
//! **Behind the range** (live every cycle): a wooden watchtower in Valheim build-mode support
//! colors loses its posts one by one and comes down; a stone gate is blasted and its lintel
//! falls; a voxel cliff is mined until its overhang loses support and breaks off; a rock arch
//! loses a pillar and its span collapses.

use bevy_ecs::system::RunSystemOnce;
use bevy_ecs::world::CommandQueue;
use engine_core::math::{DVec3, Quat, Rng, Vec2, Vec3};
use engine_core::prelude::*;
use engine_render::{MeshRenderer, Shape, StandardMaterial};
use engine_world::{EditOp, EditShape, ProjectileTrace, TerrainEdit, WorldImpulse};

use crate::DemoView;
use crate::charges::BreachCharge;
use crate::debris::{Debris, PendingDebris, settle};
use crate::dress::PropModel;
use crate::hits::{Hit, HitQueue};
use crate::material::{DestructibleMaterial, linear};
use crate::props::PropKind;
use crate::structure::{Fracturable, FractureShape, ShowSupport, StructuralPiece};
use crate::voxel::VoxelVolume;
use crate::wall::{DestructibleWall, WallAnchors, WallState};

/// Seconds per live cycle.
pub const CYCLE_SECS: f32 = 20.0;
/// Lifetime given to aftermath rubble (it never sinks).
const FOREVER: f32 = 1.0e9;

/// Bay centers along X (relative to the origin) and the row's Z.
const BAYS: [f64; 7] = [20.4, 13.6, 6.8, 0.0, -6.8, -13.6, -20.4];
const ROW_Z: f64 = -8.0;
const WALL_W: f32 = 5.4;
const WALL_H: f32 = 3.2;

const TOWER: [f64; 3] = [18.0, 0.0, 9.0];
const GATE: [f64; 3] = [-22.0, 0.0, 7.0];
const CLIFF: [f64; 3] = [-4.0, 0.0, 16.0];
const ARCH: [f64; 3] = [-36.0, 0.0, 16.0];

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
}

/// The breach range's live moments (the zone's own stop is the overview from
/// `(-2, 17, -44)` looking at `(-3, 2.5, 4)`).
pub const VIEWS: &[DemoView] = &[
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
    // The range: a gravel pad, concrete pillars between the bays and a steel lintel on top.
    prop(
        commands,
        Shape::Plane {
            size: [52.0, 9.0],
            subdivisions: 1,
        },
        mat(0x6d695f, 0.95),
        o + DVec3::new(0.0, 0.012, ROW_Z - 1.0),
        0.0,
    );
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
        o + DVec3::new(0.0, 4.0, ROW_Z),
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
    let a2 = wall(
        commands,
        1,
        "Drywall (barricades)",
        DestructibleWall::soft(WALL_W, WALL_H)
            .with_seed(12)
            .with_paint([0.55, 0.6, 0.62]),
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

    // Bay 2: barricades in a doorway and a window (wall-local x is mirrored by the yaw).
    let local = |bay: usize, x: f32, y: f32, z: f32| {
        let (t, _) = placed(o + DVec3::new(BAYS[bay], 0.0, ROW_Z), yaw);
        GlobalTransform::from(t).transform_point(DVec3::new(x as f64, y as f64, z as f64))
    };
    let door = (Vec2::new(-1.4, 0.0), Vec2::new(-0.2, 2.2));
    let window = (Vec2::new(0.6, 1.0), Vec2::new(1.9, 2.1));
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

    let targets = DemoTargets {
        thermite_wall: Some(b1),
        soft_wall: Some(b2),
        cliff: Some(spawn_cliff(commands, o)),
        arch: Some(spawn_arch(commands, o)),
        tower: Vec::new(),
        tower_posts: Vec::new(),
        gate: spawn_gate(commands, o),
        props: Vec::new(),
    };
    let (tower, posts) = spawn_tower(commands, o);
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
        door_rect: door,
        window_rect: window,
    };
    commands.queue(move |world: &mut World| apply_aftermath(world, ids));
}

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
                        shape: FractureShape::Box {
                            half_extents: Vec3::splat(s * 0.5),
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

fn spawn_cliff(commands: &mut Commands, o: DVec3) -> Entity {
    let (t, g) = placed(o + DVec3::from_array(CLIFF), 0.0);
    let mut v = VoxelVolume::cliff(26.0, 12.0, 9.0, 0.25).with_seed(7);
    v.overhang = 2.4;
    commands.spawn((Name::new("Voxel cliff"), t, g, v)).id()
}

fn spawn_arch(commands: &mut Commands, o: DVec3) -> Entity {
    let (t, g) = placed(o + DVec3::from_array(ARCH), 0.35);
    let v = VoxelVolume::arch(12.0, 8.0, 3.0, 0.2).with_seed(5);
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
    if material == DestructibleMaterial::Wood {
        // Sawn timber, its grain along the piece; a stable seed per spot.
        let seed = (at.x * 7.0 + at.y * 13.0 + at.z * 17.0).abs() as u64;
        e.insert(PropModel::new(
            PropKind::Timber {
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

/// A two-storey wooden watchtower with a cantilevered balcony (Valheim build-mode colors).
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
    // Floor and a balcony sticking out over the range (−Z).
    for (x, z) in [
        (-0.9, -0.9),
        (0.9, -0.9),
        (-0.9, 0.9),
        (0.9, 0.9),
        (-0.9, -2.7),
        (0.9, -2.7),
    ] {
        all.push(piece(
            commands,
            "Floor",
            p(x, 3.26, z),
            Quat::IDENTITY,
            Vec3::new(1.8, 0.12, 1.8),
            w,
            wood,
            true,
        ));
    }
    // Roof: two pitched panels on the upper beams.
    for s in [-1.0f32, 1.0] {
        let rot = Quat::from_rotation_x(s * 0.55);
        all.push(piece(
            commands,
            "Roof",
            p(0.0, 6.3, s as f64 * 1.0),
            rot,
            Vec3::new(4.0, 0.12, 2.3),
            w,
            wood,
            true,
        ));
    }
    (all, posts)
}

/// A stone gate: two towers of blocks and a three-block lintel (stone: strong upward, weak
/// sideways).
fn spawn_gate(commands: &mut Commands, o: DVec3) -> Vec<Entity> {
    let base = o + DVec3::from_array(GATE);
    let mut out = Vec::new();
    let mut rng = Rng::new(9);
    let s = DestructibleMaterial::Stone;
    let mut block = |commands: &mut Commands, x: f64, y: f64, size: Vec3, rng: &mut Rng| {
        let shade = rng.range(0.85, 1.08);
        let c = linear(0x9a9386);
        let look = StandardMaterial::color(c[0] * shade, c[1] * shade, c[2] * shade * 0.98)
            .with_roughness(0.9);
        out.push(piece(
            commands,
            "Stone block",
            base + DVec3::new(x, y, 0.0),
            Quat::IDENTITY,
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
                Vec3::new(1.2, 0.98, 1.2),
                &mut rng,
            );
        }
    }
    for x in [-1.1, 0.0, 1.1] {
        block(commands, x, 4.45, Vec3::new(1.08, 0.9, 1.1), &mut rng);
    }
    for x in [-2.0, -1.2, 1.2, 2.0] {
        block(commands, x, 5.15, Vec3::new(0.45, 0.5, 0.9), &mut rng);
    }
    out
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
    door_rect: (Vec2, Vec2),
    window_rect: (Vec2, Vec2),
}

fn wall_world(world: &World, wall: Entity, local: Vec3) -> DVec3 {
    let t = world.get::<Transform>(wall).copied().unwrap_or_default();
    GlobalTransform::from(t).transform_point(local.as_dvec3())
}

/// Wreck the aftermath bays at once and lay their rubble on the ground.
fn apply_aftermath(world: &mut World, ids: Aftermath) {
    crate::assets::ensure_assets(world);
    let _ = world.run_system_once(crate::sync::init_walls_system);
    // Built openings in bay 2.
    if let Some(mut s) = world.get_mut::<WallState>(ids.a2) {
        s.open(ids.door_rect.0, ids.door_rect.1);
        s.open(ids.window_rect.0, ids.window_rect.1);
    }
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
    let door_mid = (ids.door_rect.0 + ids.door_rect.1) * 0.5;
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
    world.resource_mut::<HitQueue>().hits.extend(hits);
    let _ = world.run_system_once(crate::hits::apply_hits_system);
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
    let demos: Vec<Entity> = world
        .query_filtered::<Entity, With<DestructionDemo>>()
        .iter(world)
        .collect();
    let steps = timeline();
    for e in demos {
        let Some(mut demo) = world.get::<DestructionDemo>(e).cloned() else {
            continue;
        };
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
            world.write_message(WorldImpulse {
                center: o + DVec3::from_array(GATE) + DVec3::new(-1.6, 0.6, -0.9),
                radius: 6.0,
                strength: 60.0,
                energy_j: 2.5e6,
                heat_c: 400.0,
                source: None,
            });
        }
        Step::BlastArch => {
            if let Some(arch) = t.arch {
                let tr = world.get::<Transform>(arch).copied().unwrap_or_default();
                let g = GlobalTransform::from(tr);
                let foot = Vec3::new(-3.72, 1.0, 0.0);
                world.write_message(WorldImpulse {
                    center: g.transform_point((foot + Vec3::new(0.0, 0.0, -1.6)).as_dvec3()),
                    radius: 8.0,
                    strength: 80.0,
                    energy_j: 6.0e6,
                    heat_c: 400.0,
                    source: None,
                });
                // The blast cuts through the pillar.
                world.write_message(TerrainEdit {
                    center: g.transform_point((foot + Vec3::new(0.0, 0.0, -2.0)).as_dvec3()),
                    shape: EditShape::Capsule {
                        to: tr.rotation * Vec3::new(0.0, 0.6, 4.0),
                        radius: 1.35,
                    },
                    op: EditOp::Remove,
                    surface: None,
                    source: None,
                });
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
            let mut q = world.query::<&mut Debris>();
            for mut d in q.iter_mut(world) {
                if d.lifetime < FOREVER * 0.5 {
                    d.age = d.age.max(d.lifetime);
                }
            }
        }
        Step::Reset => {
            for w in [t.thermite_wall, t.soft_wall].into_iter().flatten() {
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
