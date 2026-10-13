#![doc = include_str!("../README.md")]
// ECS systems take many queries and resources by design.
#![allow(clippy::type_complexity, clippy::too_many_arguments)]

pub mod architecture;
pub mod assets;
pub mod barn;
pub mod build;
pub mod burn;
pub mod charges;
pub mod collapse;
pub mod cracks;
pub mod debris;
pub mod demo;
pub mod dress;
pub mod fracture;
pub mod hill;
pub mod hits;
pub mod material;
pub mod meshing;
pub mod props;
pub mod shift;
pub mod structure;
pub mod sync;
pub mod textures;
pub mod voxel;
pub mod wall;

use engine_core::math::{DVec3, Quat, Vec2, Vec3};
use engine_core::prelude::*;
use engine_core::registry::resolve_entity;
use engine_world::physics::ContactEvent;
use engine_world::{
    DamageEvent, EditOp, EditShape, GroundContact, NoiseEvent, ProjectileTrace, SmokeEmission,
    TerrainEdit, WorldImpulse,
};
use serde_json::{Value, json};

pub use build::{BuildAction, BuildKind, BuildRequest, Builder, BuiltPiece};
pub use charges::{BreachCharge, ChargeKind};
pub use debris::Debris;
pub use hits::{Hit, HitQueue};
pub use material::{DestructibleMaterial, DestructionMaterials, MaterialProps};
pub use structure::{Fracturable, FractureShape, PieceState, ShowSupport, StructuralPiece};
pub use voxel::{VoxelShape, VoxelState, VoxelVolume};
pub use wall::{DestructibleWall, WallAnchors, WallState};

/// A suggested viewpoint on a demo (camera bookmark or screenshot-tour stop), relative to the
/// demo's origin. Call `destruction.demo_restart` on arrival, then wait `warmup_secs` (a
/// tour that settles 0.4 s before its warm-up is already counted in) to see the named moment.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct DemoView {
    pub key: &'static str,
    pub name: &'static str,
    pub eye: [f64; 3],
    pub target: [f64; 3],
    pub hour: f32,
    pub warmup_secs: f32,
}

/// The function a viewpoint calls on arrival so its moment plays from the start.
pub const RESTART_API: &str = "destruction.demo_restart";

/// Who moves big debris chunks.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DebrisMode {
    /// The physics module when one is installed, else the built-in solver.
    #[default]
    Auto,
    /// Always the built-in solver (cheap; debris lands on the ground but not on each other).
    Builtin,
    /// Always physics-contract bodies (needs a physics module).
    Physics,
}

/// Global destruction tuning. Every number has a unit; change them live through the AI bridge
/// (`resource.set DestructionSettings`).
#[derive(Resource, Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct DestructionSettings {
    pub debris_mode: DebrisMode,
    /// Most chunks alive at once; the oldest sink away first.
    pub max_debris: usize,
    /// Most small chips alive at once.
    pub max_chips: usize,
    /// Seconds a chunk stays before sinking into the ground.
    pub debris_lifetime_secs: f32,
    /// Seconds a chip stays before sinking.
    pub chip_lifetime_secs: f32,
    /// Seconds debris takes to sink out of sight.
    pub sink_secs: f32,
    /// Spawn dust puffs.
    pub dust: bool,
    /// Joules of breaking energy per point of `DamageEvent` damage (bullets, blades, fists).
    pub joules_per_damage: f32,
    /// Joules per point of explosive damage.
    pub explosive_joules_per_damage: f32,
    /// Bullets (`ProjectileTrace`) punch through walls.
    pub projectile_traces: bool,
    /// Mass of a bullet for `ProjectileTrace` energy, kg.
    pub bullet_mass_kg: f32,
    /// Share of a bullet's kinetic energy that goes into breaking material.
    pub ballistic_efficiency: f32,
    /// Hard physics contacts (a car into a wall) damage destructibles.
    pub contact_damage: bool,
    /// Smallest contact impulse that does damage, N·s.
    pub contact_min_impulse: f32,
    /// Explosions near the ground also dig a crater in the terrain (`TerrainEdit`).
    pub terrain_craters: bool,
    /// Most chunks one hit breaks off.
    pub max_chunks_per_hit: usize,
    /// Wall cells per flying chunk.
    pub cells_per_chunk: usize,
    /// Seconds an unsupported building piece hangs before it falls.
    pub collapse_delay_secs: f32,
    /// Wooden building pieces get an `engine_world::Combustible`, so a fire module burns
    /// them: they char, weaken and come down.
    pub wood_burns: bool,
}

impl Default for DestructionSettings {
    fn default() -> Self {
        Self {
            debris_mode: DebrisMode::Auto,
            max_debris: 400,
            max_chips: 700,
            debris_lifetime_secs: 60.0,
            chip_lifetime_secs: 8.0,
            sink_secs: 4.0,
            dust: true,
            joules_per_damage: 50.0,
            explosive_joules_per_damage: 5_000.0,
            projectile_traces: true,
            bullet_mass_kg: 0.008,
            ballistic_efficiency: 0.5,
            contact_damage: true,
            contact_min_impulse: 400.0,
            terrain_craters: true,
            max_chunks_per_hit: 14,
            cells_per_chunk: 6,
            collapse_delay_secs: 0.3,
            wood_burns: true,
        }
    }
}

/// Live counters (read with `destruction.stats`).
#[derive(Resource, Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct DestructionStats {
    pub walls: usize,
    pub voxel_volumes: usize,
    pub chunks: usize,
    pub chips: usize,
    pub dust: usize,
    pub hits: u64,
    pub cells_destroyed: u64,
    pub voxels_removed_m3: f32,
    pub collapses: u64,
    pub pieces_broken: u64,
    pub chunks_spawned: u64,
    pub chips_spawned: u64,
    pub despawned: u64,
}

/// Destructible walls and barricades, voxel cliffs, collapsing buildings and fracturing props.
/// See the crate docs.
pub struct DestructionPlugin;

impl Plugin for DestructionPlugin {
    fn build(&self, app: &mut App) {
        let source = app.world.spawn(Name::new("destruction")).id();
        app.add_message::<DamageEvent>()
            .add_message::<WorldImpulse>()
            .add_message::<ProjectileTrace>()
            .add_message::<ContactEvent>()
            .add_message::<TerrainEdit>()
            .add_message::<NoiseEvent>()
            .add_message::<SmokeEmission>()
            .add_message::<GroundContact>()
            .add_message::<build::BuildRequest>()
            .init_resource::<DestructionSettings>()
            .init_resource::<DestructionStats>()
            .init_resource::<DestructionMaterials>()
            .init_resource::<HitQueue>()
            .init_resource::<debris::PendingDebris>()
            .init_resource::<structure::StructureGraph>()
            .init_resource::<dress::PropMeshes>()
            .insert_resource(hits::DestructionSource(source))
            .register_component::<DestructibleWall>(
                "Siege-style destructible wall or barricade: size [w,h] m, thickness m, cell_size m, \
                 material, anchors, rebar/stud/plank spacing m. Bullets chip holes, charges breach it, \
                 unsupported parts fall.",
            )
            .register_component::<VoxelVolume>(
                "Valheim-style destructible voxel rock/earth: dims (voxels), voxel_size m, material, \
                 shape (cliff/arch/boulder/solid), anchored faces. Dig it, blast it; overhangs that \
                 lose support break off.",
            )
            .register_component::<StructuralPiece>(
                "Building piece (post, beam, floor, block) with Valheim-style support: half_extents m, \
                 material, break_energy_j (0 = from material), foundation. Unsupported pieces fall.",
            )
            .register_component::<PieceState>(
                "Live support of a StructuralPiece: support, max_support, damage_j, collapse_in s.",
            )
            .register_component::<ShowSupport>(
                "Paint a StructuralPiece in Valheim build-mode support colors (blue grounded → red failing).",
            )
            .register_component::<Builder>(
                "Valheim-style build mode on this entity (the player's camera, a script): active, kind \
                 (wood_floor, wood_wall, wood_pole, wood_beam, wood_roof, wood_roof_45, stone_floor, \
                 stone_wall, stone_pillar), yaw rad, reach_m, use_input, aim ray, show_support.",
            )
            .register_component::<BuiltPiece>("A building piece placed in build mode: kind.")
            .register_component::<Fracturable>(
                "Prop that shatters into Voronoi pieces: shape, material, break_energy_j, pieces, damage_j.",
            )
            .register_component::<Debris>(
                "A falling or resting piece of debris: material, mass_kg, age s, lifetime s, chip.",
            )
            .register_component::<BreachCharge>(
                "A breaching charge on a wall: kind (explosive energy_j / thermite size m), fuse_secs.",
            )
            .register_resource::<DestructionSettings>(
                "Destruction tuning: debris budget and lifetimes, J per damage point, bullet mass kg, \
                 contact damage, terrain craters, collapse delay s.",
            )
            .register_resource::<DestructionMaterials>(
                "Breakable materials: toughness J/m³, density kg/m³, crater shape, colors, support and \
                 losses per meter (Valheim-style), shatter speed m/s.",
            )
            .register_resource::<DestructionStats>("Destruction counters (read-only).")
            .add_systems(PreUpdate, assets::ensure_assets)
            .add_systems(Startup, build::bind_actions)
            .add_systems(
                FixedUpdate,
                (
                    (demo::demo_script_system,
                hill::hill_script_system, barn::barn_script_system, charges::charges_system)
                        .chain()
                        .in_set(SimSet::Gameplay),
                    debris::simulate_debris_system.in_set(SimSet::Physics),
                    (
                        hits::collect_hits_system,
                        hits::apply_hits_system,
                        burn::burn_weaken_system,
                        collapse::structure_system,
                        collapse::voxel_support_system,
                        collapse::shatter_system,
                        debris::spawn_debris_system,
                    )
                        .chain()
                        .in_set(SimSet::PostPhysics),
                ),
            )
            .add_systems(
                Update,
                (
                    sync::init_walls_system,
                    sync::init_voxels_system,
                    sync::rebuild_walls_system,
                    sync::rebuild_voxels_system,
                    debris::age_debris_system,
                    charges::charge_visuals_system,
                    build::build_input_system,
                    build::build_system,
                    sync::piece_colliders_system,
                    dress::build_prop_models_system,
                    build::ghost_look_system,
                    barn::barn_fuel_system,
                    burn::make_wood_combustible_system,
                    structure::support_tint_system,
                    burn::char_system,
                )
                    .chain()
                    .in_set(UpdateSet::Logic),
            )
            .add_systems(
                PostUpdate,
                shift::shift_system.in_set(engine_world::WorldShiftSet::Apply),
            );
        register_api(app);
    }
}

#[derive(Deserialize)]
struct BlastArgs {
    at: [f64; 3],
    #[serde(default = "default_blast")]
    energy_j: f32,
}

fn default_blast() -> f32 {
    1.0e6
}

#[derive(Deserialize)]
struct ShootArgs {
    from: [f64; 3],
    to: [f64; 3],
    #[serde(default = "default_speed")]
    speed: f32,
    #[serde(default)]
    count: Option<u32>,
    #[serde(default)]
    spread: Option<f32>,
}

fn default_speed() -> f32 {
    850.0
}

#[derive(Deserialize)]
struct HitArgs {
    at: [f64; 3],
    #[serde(default)]
    dir: Option<[f32; 3]>,
    #[serde(default = "default_hit")]
    energy_j: f32,
    #[serde(default)]
    melee: bool,
}

fn default_hit() -> f32 {
    3000.0
}

#[derive(Deserialize)]
struct DigArgs {
    at: [f64; 3],
    #[serde(default = "default_radius")]
    radius: f32,
    #[serde(default)]
    op: Option<String>,
}

fn default_radius() -> f32 {
    1.0
}

#[derive(Deserialize)]
struct BreachArgs {
    wall: Value,
    #[serde(default)]
    kind: Option<String>,
    /// Wall-local XY of the charge's center.
    #[serde(default)]
    at: Option<[f32; 2]>,
    #[serde(default)]
    size: Option<[f32; 2]>,
    #[serde(default)]
    energy_j: Option<f32>,
    #[serde(default)]
    fuse_secs: Option<f32>,
    #[serde(default)]
    back: bool,
}

#[derive(Deserialize)]
struct SpawnWallArgs {
    #[serde(default)]
    preset: Option<String>,
    at: [f64; 3],
    #[serde(default)]
    yaw_deg: f32,
    #[serde(default)]
    size: Option<[f32; 2]>,
    #[serde(default)]
    thickness: Option<f32>,
    #[serde(default)]
    material: Option<String>,
}

#[derive(Deserialize)]
struct SpawnVoxelArgs {
    #[serde(default)]
    preset: Option<String>,
    at: [f64; 3],
    #[serde(default)]
    yaw_deg: f32,
    #[serde(default)]
    size: Option<[f32; 3]>,
    #[serde(default)]
    voxel_size: Option<f32>,
    #[serde(default)]
    material: Option<String>,
}

fn parse<T: serde::de::DeserializeOwned>(args: Value, example: &str) -> Result<T, String> {
    serde_json::from_value(args).map_err(|e| format!("Bad arguments ({e}). Example: {example}"))
}

fn material_arg(name: Option<&str>) -> Result<Option<DestructibleMaterial>, String> {
    match name {
        None => Ok(None),
        Some(n) => DestructibleMaterial::from_name(n).map(Some).ok_or_else(|| {
            format!(
                "Unknown material \"{n}\". Use one of: {}.",
                DestructibleMaterial::names()
            )
        }),
    }
}

fn register_api(app: &mut App) {
    build_api(app);
    const BLAST: &str = r#"{"at": [0, 1, 0], "energy_j": 1000000}"#;
    app.register_api(
        "destruction.blast",
        "Explode at a point (joules; 1e5 = grenade, 1e6 = breach charge, 1e7 = car bomb). \
         Writes a WorldImpulse, so smoke, foliage and physics react too.",
        BLAST,
        |world, args| {
            let a: BlastArgs = parse(args, BLAST)?;
            let center = DVec3::from_array(a.at);
            let scale = (a.energy_j / 1.0e5).cbrt();
            world.write_message(WorldImpulse {
                center,
                radius: (4.0 * scale).clamp(1.0, 60.0),
                strength: (25.0 * scale).clamp(5.0, 300.0),
                energy_j: a.energy_j,
                heat_c: 400.0,
                source: None,
            });
            Ok(json!({"ok": true}))
        },
    );
    const SHOOT: &str =
        r#"{"from": [0, 1.5, 10], "to": [0, 1.5, -10], "speed": 850, "count": 1, "spread": 0.0}"#;
    app.register_api(
        "destruction.shoot",
        "Fire bullets along a line (writes ProjectileTrace): holes in walls, chips off rock. \
         count > 1 with spread (radians) fires a burst.",
        SHOOT,
        |world, args| {
            let a: ShootArgs = parse(args, SHOOT)?;
            let from = DVec3::from_array(a.from);
            let to = DVec3::from_array(a.to);
            let n = a.count.unwrap_or(1).clamp(1, 64);
            let spread = a.spread.unwrap_or(0.0);
            let mut rng =
                engine_core::math::Rng::new(world.resource::<DestructionStats>().hits ^ 0xB0_11E7);
            let dir = (to - from).as_vec3();
            for _ in 0..n {
                let jitter = if spread > 0.0 {
                    Quat::from_euler(
                        engine_core::math::EulerRot::YXZ,
                        rng.range(-spread, spread),
                        rng.range(-spread, spread),
                        0.0,
                    )
                } else {
                    Quat::IDENTITY
                };
                world.write_message(ProjectileTrace {
                    from,
                    to: from + (jitter * dir).as_dvec3(),
                    radius: 0.01,
                    speed: a.speed,
                    shooter: None,
                });
            }
            Ok(json!({"shots": n}))
        },
    );
    const HIT: &str = r#"{"at": [0, 1, 0.2], "dir": [0, 0, -1], "energy_j": 3000, "melee": true}"#;
    app.register_api(
        "destruction.hit",
        "Hit whatever destructible is at a point (a hammer, a punch, a crash). energy_j is what \
         goes into breaking material.",
        HIT,
        |world, args| {
            let a: HitArgs = parse(args, HIT)?;
            let dir = a
                .dir
                .map(Vec3::from_array)
                .unwrap_or(Vec3::NEG_Z)
                .normalize_or(Vec3::NEG_Z);
            world.resource_mut::<HitQueue>().hits.push(Hit::Impact {
                point: DVec3::from_array(a.at),
                dir,
                energy_j: a.energy_j,
                min_radius: if a.melee { 0.1 } else { 0.02 },
                melee: a.melee,
                target: None,
            });
            Ok(json!({"ok": true}))
        },
    );
    const DIG: &str = r#"{"at": [0, 2, 0], "radius": 1.0, "op": "remove"}"#;
    app.register_api(
        "destruction.dig",
        "Dig (remove), fill (add), smooth or paint voxel rock and terrain in a sphere (writes \
         TerrainEdit, so the terrain module digs too).",
        DIG,
        |world, args| {
            let a: DigArgs = parse(args, DIG)?;
            let op = match a.op.as_deref().unwrap_or("remove") {
                "remove" | "dig" => EditOp::Remove,
                "add" | "fill" => EditOp::Add,
                "smooth" => EditOp::Smooth { strength: 0.5 },
                "paint" => EditOp::Paint,
                other => {
                    return Err(format!(
                        "Unknown op \"{other}\". Use remove, add, smooth or paint."
                    ));
                }
            };
            world.write_message(TerrainEdit {
                center: DVec3::from_array(a.at),
                shape: EditShape::Sphere {
                    radius: a.radius.clamp(0.05, 50.0),
                },
                op,
                surface: None,
                source: None,
            });
            Ok(json!({"ok": true}))
        },
    );
    const BREACH: &str = r#"{"wall": {"name": "Reinforced wall"}, "kind": "thermite", "at": [0, 1.1], "size": [1.0, 1.2], "fuse_secs": 1.0}"#;
    app.register_api(
        "destruction.breach",
        "Put a breaching charge on a wall: kind \"explosive\" (energy_j) blows a hole, \"thermite\" \
         burns a clean size [w,h] m opening even through reinforced walls. `at` is wall-local [x,y].",
        BREACH,
        |world, args| {
            let a: BreachArgs = parse(args, BREACH)?;
            let wall = resolve_entity(world, &a.wall)?;
            let Some(w) = world.get::<DestructibleWall>(wall).cloned() else {
                return Err("That entity is not a DestructibleWall. Pick one with entity.list.".into());
            };
            let kind = match a.kind.as_deref().unwrap_or("explosive") {
                "explosive" => ChargeKind::Explosive {
                    energy_j: a.energy_j.unwrap_or(4.0e5),
                },
                "thermite" => ChargeKind::Thermite {
                    size: a.size.unwrap_or([1.0, 1.2]),
                    burn_secs: 4.0,
                },
                other => return Err(format!("Unknown kind \"{other}\". Use explosive or thermite.")),
            };
            let at = a.at.unwrap_or([0.0, (w.size[1] * 0.5).min(1.2)]);
            let charge = world
                .spawn(BreachCharge {
                    wall: Some(wall),
                    at: Vec2::from_array(at),
                    back: a.back,
                    fuse_secs: a.fuse_secs.unwrap_or(2.0),
                    kind,
                    ..BreachCharge::default()
                })
                .id();
            Ok(json!({"id": charge.to_bits()}))
        },
    );
    const REPAIR: &str = r#"{"entity": {"name": "Reinforced wall"}}"#;
    app.register_api(
        "destruction.repair",
        "Restore a wall or voxel volume to undamaged (omit entity to repair all of them).",
        REPAIR,
        |world, args| {
            let target = match args.get("entity") {
                Some(sel) => Some(resolve_entity(world, sel)?),
                None => None,
            };
            let mut walls = world.query::<(Entity, &mut DestructibleWall)>();
            let mut n = 0;
            for (e, mut w) in walls.iter_mut(world) {
                if target.is_none_or(|t| t == e) {
                    w.set_changed();
                    n += 1;
                }
            }
            let mut vols = world.query::<(Entity, &mut VoxelVolume)>();
            for (e, mut v) in vols.iter_mut(world) {
                if target.is_none_or(|t| t == e) {
                    v.set_changed();
                    n += 1;
                }
            }
            Ok(json!({"repaired": n}))
        },
    );
    app.register_api(
        "destruction.clear_debris",
        "Sink all debris and chips out of sight now.",
        "{}",
        |world, _| {
            let mut q = world.query::<&mut Debris>();
            let mut n = 0;
            for mut d in q.iter_mut(world) {
                d.age = d.age.max(d.lifetime);
                n += 1;
            }
            Ok(json!({"cleared": n}))
        },
    );
    app.register_api(
        "destruction.demo_restart",
        "Start the destruction demos (the breach range, the burning storehouse and the \
         dig-and-build hill) over from the beginning of their loops, so a camera arriving now \
         sees every step.",
        "{}",
        |world, _| {
            let range = demo::restart_demos(world);
            let hills = hill::restart_hills(world);
            let barns = barn::restart_barns(world);
            Ok(json!({"range": range, "hills": hills, "barns": barns}))
        },
    );
    app.register_api(
        "destruction.stats",
        "Counters: walls, voxel volumes, live chunks/chips/dust, cells destroyed, m³ of rock removed, collapses.",
        "{}",
        |world, _| serde_json::to_value(world.resource::<DestructionStats>().clone()).map_err(|e| e.to_string()),
    );
    const INSPECT: &str = r#"{"entity": {"name": "Reinforced wall"}}"#;
    app.register_api(
        "destruction.inspect",
        "How damaged a wall, voxel volume or building piece is (integrity 0..1, support).",
        INSPECT,
        |world, args| {
            let sel = args.get("entity").cloned().unwrap_or(args);
            let e = resolve_entity(world, &sel)?;
            if let Some(s) = world.get::<WallState>(e) {
                return Ok(json!({"kind": "wall", "integrity": s.integrity(), "material": s.material.name()}));
            }
            if let Some(s) = world.get::<VoxelState>(e) {
                return Ok(json!({"kind": "voxels", "integrity": s.integrity(), "volume_m3": s.volume()}));
            }
            if let Some(s) = world.get::<PieceState>(e) {
                return Ok(json!({
                    "kind": "piece",
                    "support": s.support,
                    "support_fraction": s.support_fraction(),
                    "damage_j": s.damage_j,
                    "collapsing": s.collapse_in.is_some(),
                }));
            }
            Err("That entity is not destructible (no wall, voxel volume or structural piece).".into())
        },
    );
    const SPAWN_WALL: &str =
        r#"{"preset": "reinforced", "at": [0, 0, 0], "yaw_deg": 0, "size": [4, 3]}"#;
    app.register_api(
        "destruction.spawn_wall",
        "Spawn a destructible wall: preset soft (drywall), barricade (planks), reinforced, brick, \
         stone or concrete; size [w,h] m. Returns its id.",
        SPAWN_WALL,
        |world, args| {
            let a: SpawnWallArgs = parse(args, SPAWN_WALL)?;
            let [w, h] = a.size.unwrap_or([4.0, 3.0]);
            let mut wall = match a.preset.as_deref().unwrap_or("soft") {
                "soft" => DestructibleWall::soft(w, h),
                "barricade" => DestructibleWall::barricade(w, h),
                "reinforced" => DestructibleWall::reinforced(w, h),
                "brick" => DestructibleWall::masonry(DestructibleMaterial::Brick, w, h, 0.22),
                "stone" => DestructibleWall::masonry(DestructibleMaterial::Stone, w, h, 0.4),
                "concrete" => DestructibleWall::concrete(w, h, 0.2),
                other => {
                    return Err(format!(
                        "Unknown preset \"{other}\". Use soft, barricade, reinforced, brick, stone or concrete."
                    ));
                }
            };
            if let Some(t) = a.thickness {
                wall.thickness = t.clamp(0.02, 2.0);
            }
            if let Some(m) = material_arg(a.material.as_deref())? {
                wall.material = m;
            }
            let e = world
                .spawn((
                    Transform::from_translation(DVec3::from_array(a.at))
                        .with_rotation(Quat::from_rotation_y(a.yaw_deg.to_radians())),
                    wall,
                ))
                .id();
            Ok(json!({"id": e.to_bits()}))
        },
    );
    const SPAWN_VOX: &str =
        r#"{"preset": "cliff", "at": [0, 0, 0], "size": [10, 8, 5], "voxel_size": 0.25}"#;
    app.register_api(
        "destruction.spawn_voxels",
        "Spawn a diggable voxel rock: preset cliff, arch, boulder or hill; size [w,h,d] m; voxel_size m. \
         Returns its id.",
        SPAWN_VOX,
        |world, args| {
            let a: SpawnVoxelArgs = parse(args, SPAWN_VOX)?;
            let s = a.voxel_size.unwrap_or(0.25).clamp(0.05, 2.0);
            let [w, h, d] = a.size.unwrap_or([10.0, 8.0, 5.0]);
            let mut v = match a.preset.as_deref().unwrap_or("cliff") {
                "cliff" => VoxelVolume::cliff(w, h, d, s),
                "arch" => VoxelVolume::arch(w, h, d, s),
                "boulder" => VoxelVolume::boulder(w, s),
                "hill" => VoxelVolume::hill(w, h, d, s),
                other => return Err(format!("Unknown preset \"{other}\". Use cliff, arch, boulder or hill.")),
            };
            let cells = v.dims.iter().map(|d| *d as u64).product::<u64>();
            if cells > 4_000_000 {
                return Err(format!(
                    "That is {cells} voxels; keep it under 4 million (use a bigger voxel_size or a smaller size)."
                ));
            }
            if let Some(m) = material_arg(a.material.as_deref())? {
                v = v.with_material(m);
            }
            let e = world
                .spawn((
                    Transform::from_translation(DVec3::from_array(a.at))
                        .with_rotation(Quat::from_rotation_y(a.yaw_deg.to_radians())),
                    v,
                ))
                .id();
            Ok(json!({"id": e.to_bits()}))
        },
    );
}

#[derive(Deserialize)]
struct BuildArgs {
    #[serde(default)]
    kind: Option<String>,
    /// Aim from here …
    #[serde(default)]
    from: Option<[f64; 3]>,
    /// … toward here.
    #[serde(default)]
    to: Option<[f64; 3]>,
    /// Or aim straight down at this point.
    #[serde(default)]
    at: Option<[f64; 3]>,
    #[serde(default)]
    yaw_deg: f32,
    /// Only show where it would go.
    #[serde(default)]
    preview: bool,
}

impl BuildArgs {
    fn ray(&self, example: &str) -> Result<(DVec3, Vec3, f32), String> {
        let (from, to) = match (self.from, self.to, self.at) {
            (Some(f), Some(t), _) => (DVec3::from_array(f), DVec3::from_array(t)),
            (_, _, Some(a)) => {
                let a = DVec3::from_array(a);
                (a + DVec3::new(0.0, 3.0, 0.0), a)
            }
            _ => {
                return Err(format!(
                    "Say where to aim: \"from\" and \"to\", or \"at\". Example: {example}"
                ));
            }
        };
        let d = (to - from).as_vec3();
        if d.length() < 1e-3 {
            return Err(format!(
                "\"from\" and \"to\" are the same point. Example: {example}"
            ));
        }
        Ok((from, d.normalize(), d.length() + 2.0))
    }
}

fn placement_json(p: &build::Placement) -> Value {
    let x = p.rotation * p.kind.base_rotation().inverse() * Vec3::X;
    let yaw = (-x.z).atan2(x.x);
    json!({
        "kind": p.kind.name(),
        "valid": p.valid(),
        "problem": p.problem.map(|x| x.describe()),
        "center": p.center.to_array(),
        "yaw_deg": yaw.to_degrees(),
        "snapped": p.snapped,
        "grounded": p.grounded,
        "support": p.support,
        "support_fraction": p.support_fraction(),
    })
}

fn build_api(app: &mut App) {
    const BUILD: &str =
        r#"{"kind": "wood_wall", "from": [0, 3, -4], "to": [0, 0.4, -0.8], "yaw_deg": 0}"#;
    app.register_api(
        "destruction.build",
        "Place a building piece the way a player in build mode would: aim a ray (from → to, or \
         straight down at \"at\"), the piece snaps to the pieces around it and is placed if it is \
         free and supported. Kinds: wood_floor, wood_wall, wood_pole, wood_beam, wood_roof, \
         wood_roof_45, stone_floor, stone_wall, stone_pillar. \"preview\": true only says where it \
         would go. Returns where, its support and the new piece's id.",
        BUILD,
        |world, args| {
            let a: BuildArgs = parse(args, BUILD)?;
            let kind = match a.kind.as_deref() {
                None => BuildKind::WoodFloor,
                Some(n) => BuildKind::from_name(n).ok_or_else(|| {
                    format!("Unknown piece \"{n}\". Use one of: {}.", BuildKind::names())
                })?,
            };
            let (origin, dir, reach) = a.ray(BUILD)?;
            let (p, _) = build::plan_in_world(world, kind, a.yaw_deg.to_radians(), origin, dir, reach)
                .ok_or_else(|| {
                    format!("The aim hits nothing (no piece, rock or ground within {reach:.1} m). Example: {BUILD}")
                })?;
            let mut out = placement_json(&p);
            if !a.preview && p.valid() {
                let table = world.resource::<DestructionMaterials>().clone();
                let (bundle, fuel) = build::built_piece(p.kind, p.transform(), None, true, &table);
                let mut e = world.spawn(bundle);
                if let Some(fuel) = fuel {
                    e.insert(fuel);
                }
                out["id"] = json!(e.id().to_bits());
            }
            Ok(out)
        },
    );
    const UNBUILD: &str = r#"{"from": [0, 3, -4], "to": [0, 1.4, -1]}"#;
    app.register_api(
        "destruction.unbuild",
        "Take down a piece placed in build mode: the one an aim ray (from → to, or down at \"at\") \
         hits, or {\"entity\": …}. What it held up falls.",
        UNBUILD,
        |world, args| {
            let target = if let Some(sel) = args.get("entity") {
                resolve_entity(world, sel)?
            } else {
                let a: BuildArgs = parse(args, UNBUILD)?;
                let (origin, dir, reach) = a.ray(UNBUILD)?;
                build::plan_in_world(world, BuildKind::WoodPole, 0.0, origin, dir, reach)
                    .and_then(|(_, t)| t)
                    .ok_or("The aim hits no piece placed in build mode.")?
            };
            if world.get::<BuiltPiece>(target).is_none()
                || world.get::<StructuralPiece>(target).is_none()
            {
                return Err("That entity is not a standing piece placed in build mode.".into());
            }
            world.despawn(target);
            Ok(json!({"removed": target.to_bits()}))
        },
    );
    app.register_api(
        "destruction.build_pieces",
        "The pieces build mode can place: name, label, material and size in meters.",
        "{}",
        |_, _| {
            Ok(Value::Array(
                BuildKind::ALL
                    .iter()
                    .map(|k| {
                        json!({
                            "name": k.name(),
                            "label": k.label(),
                            "material": k.material().name(),
                            "size": k.size().to_array(),
                        })
                    })
                    .collect(),
            ))
        },
    );
}
