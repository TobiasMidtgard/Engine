//! The destruction plugin end to end: world messages in, damage, debris and collapse out.
//! No GPU needed (mesh assets exist, nothing renders).

use engine_assets::AssetAppExt;
use engine_core::math::{DVec3, Vec2, Vec3};
use engine_core::prelude::*;
use engine_core::registry::call_api;
use engine_core::time::TimeSource;
use engine_destruction::*;
use engine_render::{Mesh, StandardMaterial};
use engine_world::{
    DamageEvent, DamageKind, EditOp, EditShape, ProjectileTrace, TerrainEdit, WorldPlugin,
    WorldShift,
};
use serde_json::json;

const STEP: f64 = 1.0 / 60.0;

fn app() -> App {
    let mut app = App::new();
    app.world
        .resource_mut::<Time>()
        .set_source(TimeSource::Manual { step_secs: STEP });
    app.init_asset::<Mesh>().init_asset::<StandardMaterial>();
    app.add_plugins((WorldPlugin, DestructionPlugin));
    app
}

fn run_secs(app: &mut App, secs: f64) {
    app.update_n((secs / STEP).round() as usize);
}

fn spawn_wall(app: &mut App, wall: DestructibleWall, at: DVec3) -> Entity {
    app.world
        .spawn((Transform::from_translation(at), wall))
        .id()
}

fn solid_at(app: &App, wall: Entity, p: Vec2) -> bool {
    let s = app.world.get::<WallState>(wall).expect("wall state");
    let idx = s.grid.cell_at(p).expect("inside the wall");
    s.grid.solid[idx]
}

#[test]
fn bullets_punch_through_a_barricade_and_throw_chips() {
    let mut app = app();
    let wall = spawn_wall(&mut app, DestructibleWall::barricade(1.2, 2.2), DVec3::ZERO);
    app.update();
    for k in 0..6 {
        app.world.write_message(ProjectileTrace {
            from: DVec3::new(-0.3 + k as f64 * 0.12, 1.1, 6.0),
            to: DVec3::new(-0.3 + k as f64 * 0.12, 1.1, -6.0),
            radius: 0.01,
            speed: 850.0,
            shooter: None,
        });
    }
    run_secs(&mut app, 0.5);
    let state = app.world.get::<WallState>(wall).unwrap();
    assert!(state.integrity() < 1.0, "the barricade took damage");
    let stats = app.world.resource::<DestructionStats>();
    assert!(stats.cells_destroyed > 0, "{stats:?}");
    assert!(stats.chips_spawned > 0, "{stats:?}");
}

#[test]
fn thermite_burns_a_doorway_through_reinforced_concrete() {
    let mut app = app();
    let wall = spawn_wall(
        &mut app,
        DestructibleWall::reinforced(3.0, 3.0),
        DVec3::ZERO,
    );
    app.update();
    let mut charge = BreachCharge::thermite(wall, Vec2::new(0.0, 1.1), [1.0, 1.6]);
    charge.fuse_secs = 0.2;
    app.world.spawn(charge);
    run_secs(&mut app, 2.0);
    assert!(
        solid_at(&app, wall, Vec2::new(0.0, 1.1)),
        "still burning: the panel is in place"
    );
    run_secs(&mut app, 3.5);
    assert!(
        !solid_at(&app, wall, Vec2::new(0.0, 1.1)),
        "the panel fell out"
    );
    assert!(!solid_at(&app, wall, Vec2::new(0.3, 0.5)));
    assert!(
        solid_at(&app, wall, Vec2::new(1.2, 2.6)),
        "outside the cut stays"
    );
    let rebar_left = {
        let s = app.world.get::<WallState>(wall).unwrap();
        let idx = s.grid.cell_at(Vec2::new(0.0, 1.1)).unwrap();
        s.grid.rebar[idx]
    };
    assert_eq!(rebar_left, 0, "thermite cuts the bars too");
    assert!(
        app.world.query::<&BreachCharge>().iter(&app.world).count() == 0,
        "the charge is used up"
    );
    let stats = app.world.resource::<DestructionStats>();
    assert!(
        stats.chunks_spawned >= 1,
        "the panel fell as debris: {stats:?}"
    );
}

#[test]
fn a_breach_charge_opens_drywall_but_only_scars_reinforced_concrete() {
    let mut app = app();
    let soft = spawn_wall(&mut app, DestructibleWall::soft(3.0, 3.0), DVec3::ZERO);
    let hard = spawn_wall(
        &mut app,
        DestructibleWall::reinforced(3.0, 3.0),
        DVec3::new(40.0, 0.0, 0.0),
    );
    app.update();
    for wall in [soft, hard] {
        let mut c = BreachCharge::explosive(wall, Vec2::new(0.0, 1.2), 4.0e5);
        c.fuse_secs = 0.1;
        app.world.spawn(c);
    }
    run_secs(&mut app, 0.6);
    let lost = |w: Entity| 1.0 - app.world.get::<WallState>(w).unwrap().integrity();
    assert!(
        !solid_at(&app, soft, Vec2::new(0.0, 1.2)),
        "a hole through the drywall"
    );
    assert!(!solid_at(&app, soft, Vec2::new(0.45, 1.2)), "a big one");
    assert!(
        lost(soft) > 4.0 * lost(hard),
        "soft {} vs reinforced {}",
        lost(soft),
        lost(hard)
    );
    let s = app.world.get::<WallState>(hard).unwrap();
    let bars = (0..s.grid.len()).filter(|i| s.grid.rebar[*i] != 0).count();
    assert!(
        bars > 100,
        "the rebar mesh survives ({bars} cells with bars)"
    );
}

#[test]
fn a_tower_falls_when_its_post_is_smashed() {
    let mut app = app();
    let post = app
        .world
        .spawn((
            Transform::from_xyz(0.0, 1.5, 0.0),
            StructuralPiece::new(DestructibleMaterial::Wood, Vec3::new(0.2, 3.0, 0.2)),
        ))
        .id();
    let beam = app
        .world
        .spawn((
            Transform::from_xyz(1.2, 3.1, 0.0),
            StructuralPiece::new(DestructibleMaterial::Wood, Vec3::new(2.6, 0.2, 0.2)),
        ))
        .id();
    run_secs(&mut app, 0.5);
    let state = *app.world.get::<PieceState>(beam).unwrap();
    assert!(
        state.support > 0.0 && state.collapse_in.is_none(),
        "{state:?}"
    );

    app.world.write_message(DamageEvent {
        target: post,
        amount: 2_000.0,
        kind: DamageKind::Blunt,
        point: DVec3::new(0.0, 1.0, 0.1),
        impulse: Vec3::new(0.0, 0.0, -50.0),
        source: None,
    });
    run_secs(&mut app, 2.0);
    assert!(app.world.get_entity(post).is_err(), "the post shattered");
    assert!(
        app.world.get::<StructuralPiece>(beam).is_none(),
        "the beam lost support"
    );
    assert!(
        app.world.get::<Debris>(beam).is_some(),
        "and fell as debris"
    );
    let y = app.world.get::<Transform>(beam).unwrap().translation.y;
    assert!(y < 0.5, "it is on the ground now (y = {y})");
}

#[test]
fn digging_under_an_overhang_drops_it() {
    let mut app = app();
    let mut cliff = VoxelVolume::cliff(8.0, 6.0, 5.0, 0.25);
    cliff.overhang = 1.5;
    let e = app.world.spawn((Transform::default(), cliff)).id();
    app.update();
    let before = app.world.get::<VoxelState>(e).unwrap().volume();
    // Mine a wide cave into the face.
    for x in [-2.4, -1.2, 0.0, 1.2, 2.4] {
        for z in [-2.6, -1.5, -0.4, 0.6] {
            for y in [0.9, 2.0] {
                app.world.write_message(TerrainEdit {
                    center: DVec3::new(x, y, z),
                    shape: EditShape::Sphere { radius: 0.9 },
                    op: EditOp::Remove,
                    surface: None,
                    source: None,
                });
            }
        }
    }
    run_secs(&mut app, 0.5);
    let after = app.world.get::<VoxelState>(e).unwrap().volume();
    assert!(after < before - 5.0, "dug {before} -> {after}");
    let stats = app.world.resource::<DestructionStats>().clone();
    assert!(
        stats.collapses > 0,
        "the unsupported rock above broke off: {stats:?}"
    );
    assert!(stats.chunks_spawned > 0);
}

#[test]
fn mining_a_cave_brings_the_cliff_face_down() {
    // The showcase cliff: 26 × 12 × 9 m with grass and topsoil on top. The soft topsoil must not
    // hold up the rock over a cave on its own.
    let mut app = app();
    let mut cliff = VoxelVolume::cliff(26.0, 12.0, 9.0, 0.25).with_seed(7);
    cliff.overhang = 2.4;
    app.world.spawn((Transform::default(), cliff));
    app.update();
    for col in 0..8 {
        for row in 0..8 {
            let y = if row < 4 { 1.2 } else { 2.7 };
            app.world.write_message(TerrainEdit {
                center: DVec3::new(-5.0 + col as f64 * 1.35, y, -3.2 + (row % 4) as f64 * 1.4),
                shape: EditShape::Sphere { radius: 1.3 },
                op: EditOp::Remove,
                surface: None,
                source: None,
            });
        }
    }
    run_secs(&mut app, 1.0);
    let fallen_m3: f32 = app
        .world
        .query::<&Debris>()
        .iter(&app.world)
        .filter(|d| d.material == DestructibleMaterial::Rock)
        .map(|d| d.mass_kg / 2600.0)
        .sum();
    assert!(
        fallen_m3 > 20.0,
        "the brow over the cave fell ({fallen_m3:.1} m³ of rock)"
    );
}

#[test]
fn api_functions_spawn_shoot_inspect_and_repair() {
    let mut app = app();
    app.update();
    let id = call_api(
        &mut app.world,
        "destruction.spawn_wall",
        json!({"preset": "soft", "at": [0, 0, 0], "size": [3, 3]}),
    )
    .unwrap()["id"]
        .as_u64()
        .unwrap();
    app.update();
    call_api(
        &mut app.world,
        "destruction.shoot",
        json!({"from": [0, 1.5, 8], "to": [0, 1.5, -8], "count": 8, "spread": 0.03}),
    )
    .unwrap();
    run_secs(&mut app, 0.3);
    let inspect = call_api(&mut app.world, "destruction.inspect", json!({"entity": id})).unwrap();
    assert!(inspect["integrity"].as_f64().unwrap() < 1.0, "{inspect}");
    call_api(&mut app.world, "destruction.repair", json!({"entity": id})).unwrap();
    run_secs(&mut app, 0.1);
    let inspect = call_api(&mut app.world, "destruction.inspect", json!({"entity": id})).unwrap();
    assert_eq!(inspect["integrity"].as_f64().unwrap(), 1.0, "{inspect}");
    let stats = call_api(&mut app.world, "destruction.stats", json!({})).unwrap();
    assert!(stats["hits"].as_u64().unwrap() >= 8, "{stats}");
    let err = call_api(
        &mut app.world,
        "destruction.spawn_wall",
        json!({"preset": "jelly", "at": [0, 0, 0]}),
    )
    .unwrap_err();
    assert!(err.contains("Use soft, barricade"), "{err}");
}

#[test]
fn the_demo_starts_wrecked_and_keeps_going() {
    let mut app = app();
    app.add_systems(Startup, |mut commands: Commands| {
        demo::spawn_demo(&mut commands, DVec3::new(100.0, 0.0, 0.0));
    });
    run_secs(&mut app, 0.2);
    let damaged = app
        .world
        .query::<&WallState>()
        .iter(&app.world)
        .filter(|s| s.integrity() < 0.999)
        .count();
    assert!(
        damaged >= 5,
        "the aftermath bays are wrecked from the start ({damaged})"
    );
    let resting = app
        .world
        .query::<(&Debris, &debris::DebrisBody)>()
        .iter(&app.world)
        .filter(|(_, b)| b.asleep)
        .count();
    assert!(
        resting > 20,
        "their rubble already lies on the ground ({resting})"
    );
    run_secs(&mut app, 2.5);
    let charges = app.world.query::<&BreachCharge>().iter(&app.world).count();
    assert!(charges >= 1, "the live charges are on their walls");
}

#[test]
fn the_hill_demo_tunnels_caves_in_builds_and_rebuilds() {
    let mut app = App::new();
    app.world
        .resource_mut::<Time>()
        .set_source(TimeSource::Manual {
            step_secs: 1.0 / 30.0,
        });
    app.init_asset::<Mesh>().init_asset::<StandardMaterial>();
    app.add_plugins((WorldPlugin, DestructionPlugin));
    app.add_systems(Startup, |mut commands: Commands| {
        hill::spawn_voxel_hill(&mut commands, DVec3::new(-50.0, 0.0, 30.0));
    });
    let run = |app: &mut App, secs: f32| app.update_n((secs * 30.0).round() as usize);
    let volume = |app: &mut App| -> f32 {
        app.world
            .query::<&VoxelState>()
            .iter(&app.world)
            .map(|v| v.volume())
            .sum()
    };
    run(&mut app, 0.2);
    let start = volume(&mut app);
    assert!(start > 1000.0, "a big hill ({start} m³)");

    run(&mut app, 8.0);
    let pieces = app
        .world
        .query::<&StructuralPiece>()
        .iter(&app.world)
        .count();
    assert!(pieces >= 11, "the hut is up ({pieces} pieces)");

    run(&mut app, 6.0);
    let fallen: f32 = app
        .world
        .query::<&Debris>()
        .iter(&app.world)
        .filter(|d| {
            matches!(
                d.material,
                DestructibleMaterial::Rock | DestructibleMaterial::Dirt
            )
        })
        .map(|d| d.mass_kg / 2000.0)
        .sum();
    assert!(fallen > 3.0, "the chamber's roof caved in ({fallen:.1} m³)");

    run(&mut app, 4.0);
    let raised = volume(&mut app);
    run(&mut app, 4.0);
    let hut_down = app
        .world
        .query::<(&Name, &Debris)>()
        .iter(&app.world)
        .filter(|(n, _)| n.as_str().starts_with("Hut"))
        .count();
    assert!(
        hut_down >= 2,
        "the front of the hut fell ({hut_down} pieces)"
    );

    run(&mut app, 6.5);
    let rebuilt = volume(&mut app);
    assert!(
        (rebuilt - start).abs() < 1.0,
        "the loop rebuilt the hill ({start} -> {raised} -> {rebuilt})"
    );
}

#[test]
fn a_world_shift_carries_the_demos_and_falling_debris_along() {
    let mut app = app();
    app.add_systems(Startup, |mut commands: Commands| {
        hill::spawn_voxel_hill(&mut commands, DVec3::new(-50.0, 0.0, 30.0));
        demo::spawn_demo(&mut commands, DVec3::new(100.0, 0.0, 0.0));
    });
    run_secs(&mut app, 0.1);
    let mut body = debris::DebrisBody::new(10.0, Vec3::splat(0.2), Vec::new(), 0.5);
    body.velocity = Vec3::new(2.0, 0.0, 0.0);
    let falling = app
        .world
        .spawn((
            Transform::from_xyz(5.0, 40.0, 5.0),
            GlobalTransform::from(Transform::from_xyz(5.0, 40.0, 5.0)),
            body,
        ))
        .id();
    // Re-anchor 1 km along −X into a frame where the old one drifts at 3 m/s along +Z.
    app.world.write_message(WorldShift {
        translation: DVec3::new(-1000.0, 0.0, 0.0),
        velocity: DVec3::new(0.0, 0.0, 3.0),
        ..WorldShift::IDENTITY
    });
    app.update();
    let hill = app
        .world
        .query::<&hill::HillDemo>()
        .single(&app.world)
        .unwrap();
    assert_eq!(hill.origin, DVec3::new(-1050.0, 0.0, 30.0));
    let demo = app
        .world
        .query::<&demo::DestructionDemo>()
        .single(&app.world)
        .unwrap();
    assert_eq!(demo.origin, DVec3::new(-900.0, 0.0, 0.0));
    let b = app.world.get::<debris::DebrisBody>(falling).unwrap();
    assert!(
        (b.velocity.x - 2.0).abs() < 0.1 && (b.velocity.z - 3.0).abs() < 0.1,
        "falling debris keeps its motion and picks up the frame's ({:?})",
        b.velocity
    );
}

/// Stand-in for a fire module: everything hotter than its ignition temperature burns
/// through its fuel in `burn_secs`, then is burnt.
fn burn_everything_lit(app: &mut App, burn_secs: f32) {
    use engine_world::{BurnState, Combustible};
    let dt = STEP as f32;
    let mut q = app.world.query::<&mut Combustible>();
    for mut c in q.iter_mut(&mut app.world) {
        if c.state == BurnState::Intact && c.temperature_c >= c.ignition_c {
            c.state = BurnState::Burning;
        }
        if c.state == BurnState::Burning {
            c.fuel_kg -= c.fuel_kg.max(0.5) * dt / burn_secs;
            if c.fuel_kg <= 0.0 {
                c.fuel_kg = 0.0;
                c.state = BurnState::Burnt;
            }
        }
    }
}

#[test]
fn a_burning_shed_chars_and_comes_down() {
    use engine_destruction::barn::{BarnFire, BarnPiece, spawn_barn};
    use engine_destruction::burn::Charring;
    use engine_world::{BurnState, Combustible};
    let mut app = app();
    let barn = {
        let mut queue = bevy_ecs::world::CommandQueue::default();
        let mut commands = Commands::new(&mut queue, &app.world);
        let e = spawn_barn(&mut commands, DVec3::ZERO, 0.0);
        queue.apply(&mut app.world);
        e
    };
    run_secs(&mut app, 0.5);
    let standing = |app: &mut App| {
        app.world
            .query_filtered::<(), (With<BarnPiece>, With<StructuralPiece>)>()
            .iter(&app.world)
            .count()
    };
    let total = standing(&mut app);
    let fallen: Vec<String> = app
        .world
        .query_filtered::<(&Name, &Transform), (With<BarnPiece>, Without<StructuralPiece>)>()
        .iter(&app.world)
        .map(|(n, t)| format!("{} at {:?}", n.as_str(), t.translation))
        .collect();
    assert!(total == 22, "{total} pieces standing; fell: {fallen:?}");
    // Every piece is fuel, and nothing burns before the woodpile is lit.
    let fuel = app
        .world
        .query_filtered::<&Combustible, With<BarnPiece>>()
        .iter(&app.world)
        .count();
    assert_eq!(fuel, total);
    // Lit at 2 s; the stand-in fire spreads by lighting everything once the woodpile burns.
    for k in 0..(40.0 / STEP) as usize {
        burn_everything_lit(&mut app, 12.0);
        if k as f64 * STEP > 4.0 {
            let mut q = app.world.query::<&mut Combustible>();
            for mut c in q.iter_mut(&mut app.world) {
                c.temperature_c = c.temperature_c.max(400.0);
            }
        }
        app.update();
    }
    let charred = app
        .world
        .query::<&Charring>()
        .iter(&app.world)
        .filter(|c| c.level.is_some())
        .count();
    assert!(charred >= total / 2, "{charred} of {total} charred");
    let left = standing(&mut app);
    assert!(left < total / 2, "{left} of {total} pieces still standing");
    let burnt = app
        .world
        .query::<&Combustible>()
        .iter(&app.world)
        .filter(|c| c.state == BurnState::Burnt)
        .count();
    assert!(burnt > 0);
    // The loop rebuilds it whole.
    let cycle = app.world.get::<BarnFire>(barn).unwrap().cycle;
    call_api(&mut app.world, RESTART_API, json!({})).unwrap();
    run_secs(&mut app, 0.5);
    assert_eq!(app.world.get::<BarnFire>(barn).unwrap().cycle, cycle + 1);
    assert_eq!(standing(&mut app), total);
}
