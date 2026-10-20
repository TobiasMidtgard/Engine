//! The destruction plugin end to end: world messages in, damage, debris and collapse out.
//! No GPU needed (mesh assets exist, nothing renders).

use engine_assets::AssetAppExt;
use engine_core::math::{DVec3, Quat, Vec2, Vec3};
use engine_core::prelude::*;
use engine_core::registry::call_api;
use engine_core::time::TimeSource;
use engine_destruction::*;
use engine_render::{Mesh, StandardMaterial};
use engine_world::physics::{
    BodyKind, CharacterController, Collider, ColliderShape, PhysicsBackend, PhysicsQueries,
    PhysicsQuery, QueryFilter, RigidBody,
};
use engine_world::{
    DamageEvent, DamageKind, EditOp, EditShape, ProjectileTrace, SurfaceMaterials, TerrainEdit,
    WorldPlugin, WorldShift,
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
fn a_charge_knocks_whole_bricks_out_of_a_brick_wall() {
    let mut app = app();
    let brick_wall = DestructibleWall::masonry(DestructibleMaterial::Brick, 3.0, 3.0, 0.22);
    let [bw, bh] = brick_wall.brick_size().expect("brick is masonry");
    let wall = spawn_wall(&mut app, brick_wall, DVec3::ZERO);
    app.update();
    let mut c = BreachCharge::explosive(wall, Vec2::new(0.0, 1.2), 4.0e5);
    c.fuse_secs = 0.1;
    app.world.spawn(c);
    run_secs(&mut app, 0.6);
    let g = &app.world.get::<WallState>(wall).unwrap().grid;
    // Cells per brick (running bond from x = 0, y = 0) and how many of them are gone.
    let mut bricks: std::collections::HashMap<(i64, i64), (usize, usize)> = Default::default();
    for idx in 0..g.len() {
        let p = g.center(idx);
        let row = (p.y / bh).floor() as i64;
        let offset = if row.rem_euclid(2) == 1 {
            bw * 0.5
        } else {
            0.0
        };
        let col = ((p.x + offset) / bw).floor() as i64;
        let e = bricks.entry((col, row)).or_default();
        e.0 += 1;
        e.1 += usize::from(!g.solid[idx]);
    }
    let out = bricks.values().filter(|(n, gone)| *gone == *n).count();
    let torn: Vec<_> = bricks
        .iter()
        .filter(|(_, (n, gone))| *gone < *n && *gone as f32 >= BRICK_LOSS * *n as f32)
        .collect();
    assert!(out >= 5, "the charge knocks out bricks ({out})");
    assert!(torn.is_empty(), "bricks left torn in half: {torn:?}");
}

#[test]
fn plaster_a_charge_throws_crumbles_where_it_lands() {
    let mut app = app();
    let wall = spawn_wall(&mut app, DestructibleWall::soft(3.0, 3.0), DVec3::ZERO);
    app.update();
    let mut c = BreachCharge::explosive(wall, Vec2::new(0.0, 1.2), 4.0e5);
    c.fuse_secs = 0.1;
    app.world.spawn(c);
    run_secs(&mut app, 4.0);
    let mut q = app.world.query::<(&Debris, &Transform)>();
    let slabs: Vec<DVec3> = q
        .iter(&app.world)
        .filter(|(d, t)| {
            !d.chip && DVec3::new(t.translation.x, 0.0, t.translation.z).length() > 3.0
        })
        .map(|(_, t)| t.translation)
        .collect();
    assert!(
        slabs.is_empty(),
        "plaster slabs lie whole far from the wall: {slabs:?}"
    );
    let stats = app.world.resource::<DestructionStats>();
    assert!(stats.pieces_broken > 0, "thrown pieces crumbled");
}

#[test]
fn debris_piles_up_on_resting_chunks_and_falls_when_they_go() {
    let mut app = app();
    let half = Vec3::new(0.5, 0.1, 0.4);
    let corners: Vec<Vec3> = (0..8)
        .map(|k| {
            let s = |b: usize| if k & b == 0 { -1.0 } else { 1.0 };
            half * Vec3::new(s(1), s(2), s(4))
        })
        .collect();
    let slab = |at: DVec3, asleep: bool| {
        let mut body = debris::DebrisBody::new(80.0, half, corners.clone(), 0.6);
        body.asleep = asleep;
        (
            Transform::from_translation(at),
            Debris {
                mass_kg: 80.0,
                lifetime: 600.0,
                ..Default::default()
            },
            body,
        )
    };
    // A slab lies on the ground; another drops onto it, a little off center.
    let below = app.world.spawn(slab(DVec3::new(0.0, 0.1, 0.0), true)).id();
    let above = app
        .world
        .spawn(slab(DVec3::new(0.15, 1.5, 0.1), false))
        .id();
    run_secs(&mut app, 2.0);
    let y = |app: &App| app.world.get::<Transform>(above).unwrap().translation.y;
    assert!(
        (y(&app) - 0.3).abs() < 0.03,
        "lies on the slab below: {}",
        y(&app)
    );
    let body = app.world.get::<debris::DebrisBody>(above).unwrap();
    assert!(body.asleep && body.on == Some(below), "{:?}", body.on);
    // Take the bottom one away: the top one drops to the ground.
    app.world.despawn(below);
    run_secs(&mut app, 2.0);
    assert!((y(&app) - 0.1).abs() < 0.03, "on the ground: {}", y(&app));
}

#[test]
fn rock_debris_lands_on_ledges_and_slides_off_rock_walls() {
    let mut app = app();
    // A 6 m block of rock, 6 m tall: its front half is cut back to a ledge 3 m up, so a 3 m
    // wall rises behind the ledge with its face at z = 0, toward +Z.
    let rock = VoxelVolume {
        dims: [24, 24, 16],
        voxel_size: 0.25,
        shape: VoxelShape::Solid,
        topsoil: None,
        grass: None,
        ..VoxelVolume::default()
    };
    app.world.spawn((Transform::default(), rock));
    app.update();
    app.world.write_message(TerrainEdit {
        center: DVec3::new(0.0, 4.75, 1.5),
        shape: EditShape::Box {
            half_extents: Vec3::new(3.5, 1.75, 1.5),
            rotation: Quat::IDENTITY,
        },
        op: EditOp::Remove,
        surface: None,
        source: None,
    });
    app.update();
    let half = Vec3::splat(0.3);
    let corners: Vec<Vec3> = (0..8)
        .map(|k| {
            let s = |b: usize| if k & b == 0 { -1.0 } else { 1.0 };
            half * Vec3::new(s(1), s(2), s(4))
        })
        .collect();
    let mut boulder = |at: DVec3| {
        app.world
            .spawn((
                Transform::from_translation(at),
                Debris {
                    mass_kg: 600.0,
                    material: DestructibleMaterial::Rock,
                    lifetime: 600.0,
                    ..Default::default()
                },
                debris::DebrisBody::new(600.0, half, corners.clone(), 0.6),
            ))
            .id()
    };
    // One drops onto the ledge from above.
    let dropped = boulder(DVec3::new(-1.5, 5.0, 1.0));
    // One broke off just under the wall's top and sits 10 cm into its face: the face pushes
    // it out and it falls to the ledge, instead of being lifted onto the wall's top.
    let rim = boulder(DVec3::new(1.5, 5.4, 0.2));
    run_secs(&mut app, 3.0);
    let at = |e: Entity| app.world.get::<Transform>(e).unwrap().translation;
    let (a, b) = (at(dropped), at(rim));
    assert!((a.y - 3.3).abs() < 0.08, "lands on the ledge: {a}");
    assert!(
        (b.y - 3.3).abs() < 0.08,
        "slid off the wall onto the ledge: {b}"
    );
    assert!(b.z > 0.27, "out of the wall's face: {b}");
    for e in [dropped, rim] {
        assert!(app.world.get::<debris::DebrisBody>(e).unwrap().asleep);
    }
}

#[test]
fn rock_cut_loose_drops_through_its_narrowing_hole_onto_the_cave_floor() {
    let mut app = app();
    // A 6 m block of rock with a cave inside, its floor 1 m up, and a shaft from the block's
    // top down into the cave's roof: 1.4 m wide for its top metre, 1 m wide below.
    let rock = VoxelVolume {
        dims: [24, 24, 16],
        voxel_size: 0.25,
        shape: VoxelShape::Solid,
        topsoil: None,
        grass: None,
        ..VoxelVolume::default()
    };
    app.world.spawn((Transform::default(), rock));
    app.update();
    for (center, half) in [
        (DVec3::new(1.5, 2.0, 0.0), Vec3::new(1.3, 1.0, 1.3)),
        (DVec3::new(1.5, 5.55, 0.0), Vec3::new(0.7, 0.55, 0.7)),
        (DVec3::new(1.5, 4.0, 0.0), Vec3::new(0.5, 1.0, 0.5)),
    ] {
        app.world.write_message(TerrainEdit {
            center,
            shape: EditShape::Box {
                half_extents: half,
                rotation: Quat::IDENTITY,
            },
            op: EditOp::Remove,
            surface: None,
            source: None,
        });
    }
    app.update();
    // A 1.2 m boulder broken loose in the shaft's wide top: rather than hang on the step where
    // the shaft narrows, it drops out through the rock around it (as a cave-in's roof does),
    // then lands on the cave's rock floor, not on the ground under the block.
    let half = Vec3::splat(0.6);
    let corners: Vec<Vec3> = (0..8)
        .map(|k| {
            let s = |b: usize| if k & b == 0 { -1.0 } else { 1.0 };
            half * Vec3::new(s(1), s(2), s(4))
        })
        .collect();
    let at = DVec3::new(1.5, 5.65, 0.0);
    let mut body = debris::DebrisBody::new(4000.0, half, corners, 0.6);
    body.rock = debris::RockContact::Leaving(at);
    let boulder = app
        .world
        .spawn((
            Transform::from_translation(at),
            Debris {
                mass_kg: 4000.0,
                material: DestructibleMaterial::Rock,
                lifetime: 600.0,
                ..Default::default()
            },
            body,
        ))
        .id();
    run_secs(&mut app, 3.0);
    let p = app.world.get::<Transform>(boulder).unwrap().translation;
    assert!((p.y - 1.6).abs() < 0.08, "on the cave floor: {p}");
    let body = app.world.get::<debris::DebrisBody>(boulder).unwrap();
    assert!(body.asleep && body.rock == debris::RockContact::Lands);
}

#[test]
fn a_thin_rock_roof_holds_debris_up_and_cut_rock_drops_through_its_hole() {
    let mut app = app();
    // A 6 m block of rock hollowed into a cave up to a roof one voxel (25 cm) thick, with a
    // 1 m hole through the roof.
    let rock = VoxelVolume {
        dims: [24, 24, 16],
        voxel_size: 0.25,
        shape: VoxelShape::Solid,
        topsoil: None,
        grass: None,
        ..VoxelVolume::default()
    };
    app.world.spawn((Transform::default(), rock));
    app.update();
    for (center, half) in [
        (DVec3::new(0.0, 3.375, 0.0), Vec3::new(2.7, 2.375, 1.7)),
        (DVec3::new(1.5, 5.9, 0.0), Vec3::new(0.5, 0.3, 0.5)),
    ] {
        app.world.write_message(TerrainEdit {
            center,
            shape: EditShape::Box {
                half_extents: half,
                rotation: Quat::IDENTITY,
            },
            op: EditOp::Remove,
            surface: None,
            source: None,
        });
    }
    app.update();
    let boulder = |app: &mut App, at: DVec3, half: f32, rock: debris::RockContact| {
        let half = Vec3::splat(half);
        let corners: Vec<Vec3> = (0..8)
            .map(|k| {
                let s = |b: usize| if k & b == 0 { -1.0 } else { 1.0 };
                half * Vec3::new(s(1), s(2), s(4))
            })
            .collect();
        let mut body = debris::DebrisBody::new(1000.0, half, corners, 0.6);
        body.rock = rock;
        app.world
            .spawn((
                Transform::from_translation(at),
                Debris {
                    mass_kg: 1000.0,
                    material: DestructibleMaterial::Rock,
                    lifetime: 600.0,
                    ..Default::default()
                },
                body,
            ))
            .id()
    };
    // One drops onto the roof: the cave under it doesn't pull it through.
    let on_roof = boulder(
        &mut app,
        DVec3::new(-1.5, 7.0, 0.0),
        0.3,
        debris::RockContact::Unknown,
    );
    // One cut loose above the hole, wider than it: it drops through, and lands on the cave's
    // floor rather than hanging by its top on the roof once its corners are clear.
    let at = DVec3::new(1.5, 6.7, 0.0);
    let cut = boulder(&mut app, at, 0.6, debris::RockContact::Leaving(at));
    run_secs(&mut app, 3.0);
    let y = |e: Entity| app.world.get::<Transform>(e).unwrap().translation.y;
    assert!(
        (y(on_roof) - 6.3).abs() < 0.08,
        "on the roof: {}",
        y(on_roof)
    );
    assert!((y(cut) - 1.6).abs() < 0.08, "on the cave floor: {}", y(cut));
}

#[test]
fn reinforcement_stops_bullets_and_charges_but_thermite_burns_through() {
    let mut app = app();
    // A 3 m drywall facing +Z: two 1.5 m panels go up behind it, at x = −0.75 and +0.75.
    let wall = spawn_wall(&mut app, DestructibleWall::soft(3.0, 3.0), DVec3::ZERO);
    // The floor behind it is a gravel pad, a few centimeters up: the struts stand on that.
    let pad = Transform::from_xyz(0.0, 0.0, -1.5);
    app.world.spawn((
        pad,
        GlobalTransform::from(pad),
        dress::GravelPad { size: [6.0, 3.0] },
    ));
    app.update();
    app.world.write_message(ReinforceRequest { wall });
    app.world.write_message(ReinforceRequest { wall });
    run_secs(&mut app, 0.3);
    let panels = app
        .world
        .get::<ReinforcedBy>(wall)
        .expect("reinforced")
        .0
        .clone();
    assert_eq!(panels.len(), 2, "one set of two panels for a 3 m wall");
    let (a, b) = (panels[0], panels[1]);
    let z = |app: &App, e: Entity| app.world.get::<Transform>(e).unwrap().translation.z;
    assert!(z(&app, a) < -0.4, "still being carried in: {}", z(&app, a));
    run_secs(&mut app, 2.0);
    for p in [a, b] {
        assert!(app.world.get::<Reinforcement>(p).unwrap().is_set());
        assert!((-0.15..-0.06).contains(&z(&app, p)), "home: {}", z(&app, p));
    }
    let feet: Vec<DVec3> = app
        .world
        .query::<(&reinforce::Strut, &GlobalTransform)>()
        .iter(&app.world)
        .map(|(s, gt)| gt.transform_point(DVec3::new(0.0, -s.length as f64 * 0.5, 0.0)))
        .collect();
    assert_eq!(feet.len(), 4, "two struts a panel");
    for f in &feet {
        let floor = dress::GRAVEL_TOP as f64;
        assert!(
            (f.y - floor).abs() < 0.03,
            "a strut foot on the gravel: {f}"
        );
        assert!(f.z < -0.9, "planted well behind: {f}");
    }
    // Bullets through the drywall stop on the steel.
    for k in 0..6 {
        let x = -0.95 + k as f64 * 0.08;
        app.world.write_message(ProjectileTrace {
            from: DVec3::new(x, 1.1, 6.0),
            to: DVec3::new(x, 1.1, -6.0),
            radius: 0.01,
            speed: 850.0,
            shooter: None,
        });
    }
    run_secs(&mut app, 0.3);
    let integrity = |app: &App, e: Entity| app.world.get::<WallState>(e).unwrap().integrity();
    assert!(integrity(&app, wall) < 1.0, "the drywall is holed");
    assert!(solid_at(&app, a, Vec2::new(0.0, 1.1)), "the panel is not");
    assert!(integrity(&app, a) > 0.99, "{}", integrity(&app, a));
    // A breach charge strips the drywall and only scars the panel behind.
    let mut c = BreachCharge::explosive(wall, Vec2::new(-0.75, 1.6), 4.0e5);
    c.fuse_secs = 0.1;
    app.world.spawn(c);
    run_secs(&mut app, 0.6);
    assert!(
        !solid_at(&app, wall, Vec2::new(-0.75, 1.6)),
        "a hole in the drywall"
    );
    assert!(solid_at(&app, a, Vec2::new(0.0, 1.6)), "the panel holds");
    assert!(integrity(&app, a) > 0.95, "{}", integrity(&app, a));
    // Thermite on the drywall burns through the panel behind it, and only that one.
    let mut charge = BreachCharge::thermite(wall, Vec2::new(0.75, 1.1), [1.0, 1.6]);
    charge.fuse_secs = 0.2;
    app.world.spawn(charge);
    run_secs(&mut app, 5.5);
    assert!(!solid_at(&app, wall, Vec2::new(0.75, 1.1)));
    assert!(
        !solid_at(&app, b, Vec2::new(0.0, 1.1)),
        "cut through the panel"
    );
    assert!(!solid_at(&app, b, Vec2::new(0.3, 0.5)));
    assert!(
        solid_at(&app, b, Vec2::new(0.0, 2.6)),
        "above the cut stays"
    );
    assert!(
        solid_at(&app, a, Vec2::new(0.6, 1.1)),
        "the other panel is untouched"
    );
    // Panels go with their wall.
    app.world.despawn(wall);
    run_secs(&mut app, 0.1);
    assert!(app.world.get_entity(a).is_err() && app.world.get_entity(b).is_err());
}

#[test]
fn ai_reinforces_the_wall_nearest_a_point() {
    let mut app = app();
    let wall = spawn_wall(
        &mut app,
        DestructibleWall::soft(5.4, 3.0),
        DVec3::new(20.0, 0.0, 0.0),
    );
    app.update();
    let out = call_api(
        &mut app.world,
        "destruction.reinforce",
        json!({"near": [21, 1.5, -0.8]}),
    )
    .unwrap();
    assert_eq!(out["wall"].as_u64().unwrap(), wall.to_bits(), "{out}");
    assert_eq!(out["panels"].as_u64().unwrap(), 2, "{out}");
    run_secs(&mut app, 2.0);
    let panels = app.world.get::<ReinforcedBy>(wall).unwrap().0.clone();
    assert_eq!(panels.len(), 2);
    let again = call_api(
        &mut app.world,
        "destruction.reinforce",
        json!({"entity": wall.to_bits()}),
    )
    .unwrap_err();
    assert!(again.contains("already reinforced"), "{again}");
    let panel = call_api(
        &mut app.world,
        "destruction.reinforce",
        json!({"entity": panels[0].to_bits()}),
    )
    .unwrap_err();
    assert!(panel.contains("reinforcement panel"), "{panel}");
    let none = call_api(
        &mut app.world,
        "destruction.reinforce",
        json!({"near": [60, 1.5, 0]}),
    )
    .unwrap_err();
    assert!(none.contains("No wall"), "{none}");
    reinforce::remove_reinforcement(&mut app.world, wall);
    assert!(app.world.get_entity(panels[0]).is_err());
    call_api(
        &mut app.world,
        "destruction.reinforce",
        json!({"entity": wall.to_bits()}),
    )
    .unwrap();
}

fn spawn_flat(app: &mut App, wall: DestructibleWall, at: DVec3) -> Entity {
    let t = Transform::from_translation(at).with_rotation(DestructibleWall::flat());
    app.world.spawn((t, GlobalTransform::from(t), wall)).id()
}

#[test]
fn a_floor_is_shot_up_through_from_below_and_blown_open_from_above() {
    let mut app = app();
    // 1.6 m across, 3 m from z = 1.5 back to z = −1.5, its boards' top at 3.025 m.
    let floor = spawn_flat(
        &mut app,
        DestructibleWall::floor(1.6, 3.0),
        DVec3::new(0.0, 3.0, 1.5),
    );
    app.update();
    app.world.write_message(ProjectileTrace {
        from: DVec3::new(0.3, 1.5, 0.0),
        to: DVec3::new(0.3, 6.0, 0.0),
        radius: 0.01,
        speed: 850.0,
        shooter: None,
    });
    run_secs(&mut app, 0.3);
    // World (x, z) is wall-local (x, 1.5 − z).
    assert!(
        !solid_at(&app, floor, Vec2::new(0.3, 1.5)),
        "the bullet went up through the boards"
    );
    assert!(solid_at(&app, floor, Vec2::new(-0.5, 1.5)));
    let mut charge = BreachCharge::explosive(floor, Vec2::new(-0.2, 2.2), 4.0e5);
    charge.fuse_secs = 0.3;
    let charge = app.world.spawn(charge).id();
    app.update();
    let y = app.world.get::<Transform>(charge).unwrap().translation.y;
    assert!(y > 3.025, "the charge lies on top of the floor ({y})");
    run_secs(&mut app, 0.5);
    for p in [
        Vec2::new(-0.2, 2.2),
        Vec2::new(0.15, 2.2),
        Vec2::new(-0.2, 1.85),
    ] {
        assert!(!solid_at(&app, floor, p), "a hole to drop through at {p}");
    }
    assert!(
        solid_at(&app, floor, Vec2::new(0.7, 0.3)),
        "the rest of the floor holds on its joists"
    );
    run_secs(&mut app, 0.5);
    let below = app
        .world
        .query_filtered::<&Transform, With<Debris>>()
        .iter(&app.world)
        .filter(|t| t.translation.y < 2.8 && t.translation.z.abs() < 2.0)
        .count();
    assert!(below > 3, "the boards rain into the room below ({below})");
}

#[test]
fn a_heaved_thermite_cut_lands_the_plate_on_the_deck_beside_the_hatch() {
    let mut app = app();
    // A steel hatch at y = 3 over x -0.5..0.5, and the roof deck beside it over x 0.5..2.5.
    let steel = spawn_flat(
        &mut app,
        DestructibleWall::steel_hatch(1.0, 1.0),
        DVec3::new(0.0, 3.0, 0.5),
    );
    spawn_flat(
        &mut app,
        DestructibleWall::floor(2.0, 1.0),
        DVec3::new(1.5, 3.0, 0.5),
    );
    app.update();
    let mut c = BreachCharge::thermite(steel, Vec2::new(0.0, 0.5), [0.8, 0.8])
        .with_tip(Vec3::new(2.0, 0.0, 3.5));
    c.fuse_secs = 0.1;
    app.world.spawn(c);
    run_secs(&mut app, 6.0);
    let mut q = app.world.query::<(&Debris, &Transform)>();
    let plate: Vec<DVec3> = q
        .iter(&app.world)
        .filter(|(d, _)| !d.chip && d.material == DestructibleMaterial::Metal)
        .map(|(_, t)| t.translation)
        .collect();
    assert!(!plate.is_empty(), "the cut plate came out");
    assert!(
        plate.iter().all(|p| p.x > 0.5 && p.y > 3.0 && p.y < 3.3),
        "the plate lies on the deck beside the hatch, not in the room below: {plate:?}"
    );
}

#[test]
fn hatches_give_way_to_a_hammer_or_thermite_but_steel_stops_bullets_and_charges() {
    let mut app = app();
    let hatch = spawn_flat(
        &mut app,
        DestructibleWall::hatch(1.0, 1.0),
        DVec3::new(0.0, 3.0, 0.5),
    );
    let steel = spawn_flat(
        &mut app,
        DestructibleWall::steel_hatch(1.0, 1.0),
        DVec3::new(10.0, 3.0, 0.5),
    );
    app.update();
    for (dx, dz, energy) in [
        (0.17, -0.15, 9000.0),
        (-0.17, -0.15, 9000.0),
        (-0.15, 0.17, 9000.0),
        (0.16, 0.17, 10000.0),
    ] {
        app.world.resource_mut::<HitQueue>().hits.push(Hit::Impact {
            point: DVec3::new(dx, 3.1, dz),
            dir: Vec3::NEG_Y,
            energy_j: energy,
            min_radius: 0.16,
            melee: true,
            target: Some(hatch),
        });
        run_secs(&mut app, 0.1);
    }
    let (middle, area) = {
        let s = app.world.get::<WallState>(hatch).unwrap();
        let open: Vec<usize> = (0..s.grid.len()).filter(|i| !s.grid.solid[*i]).collect();
        let middle = (0..s.grid.len())
            .filter(|i| s.grid.center(*i).distance(Vec2::new(0.0, 0.5)) < 0.2)
            .all(|i| !s.grid.solid[i]);
        (middle, open.len() as f32 * s.grid.cell.x * s.grid.cell.y)
    };
    assert!(
        middle && area > 0.3,
        "four swings smash the hatch open, splinters left on the frame ({area} m²)"
    );
    for k in 0..6 {
        let x = 10.0 + (k as f64 - 2.5) * 0.1;
        app.world.write_message(ProjectileTrace {
            from: DVec3::new(x, 6.0, 0.0),
            to: DVec3::new(x, 0.5, 0.0),
            radius: 0.01,
            speed: 850.0,
            shooter: None,
        });
    }
    let mut charge = BreachCharge::explosive(steel, Vec2::new(0.0, 0.5), 4.0e5);
    charge.fuse_secs = 0.2;
    app.world.spawn(charge);
    run_secs(&mut app, 0.6);
    let left = app.world.get::<WallState>(steel).unwrap().integrity();
    assert!(
        left > 0.97,
        "bullets and a breach charge only dent steel ({left})"
    );
    assert!(solid_at(&app, steel, Vec2::new(0.0, 0.5)));
    let mut charge = BreachCharge::thermite(steel, Vec2::new(0.0, 0.5), [0.8, 0.8]);
    charge.fuse_secs = 0.2;
    app.world.spawn(charge);
    run_secs(&mut app, 5.0);
    assert!(
        !solid_at(&app, steel, Vec2::new(0.0, 0.5)),
        "thermite cut the plate out"
    );
    assert!(!solid_at(&app, steel, Vec2::new(-0.2, 0.7)));
    assert!(
        solid_at(&app, steel, Vec2::new(-0.47, 0.03)),
        "its rim stays on the frame"
    );
}

#[test]
fn built_openings_come_back_when_a_wall_is_rebuilt() {
    let mut app = app();
    let wall = spawn_wall(
        &mut app,
        DestructibleWall::soft(3.0, 2.8).with_opening(Vec2::new(-0.5, 0.0), Vec2::new(0.5, 2.1)),
        DVec3::ZERO,
    );
    app.update();
    assert!(!solid_at(&app, wall, Vec2::new(0.0, 1.0)), "a doorway");
    assert!(solid_at(&app, wall, Vec2::new(1.0, 1.0)));
    let full = app.world.get::<WallState>(wall).unwrap().integrity();
    assert!(
        (full - 1.0).abs() < 1e-4,
        "the doorway is no damage ({full})"
    );
    app.world.resource_mut::<HitQueue>().hits.push(Hit::Blast {
        center: DVec3::new(1.0, 1.2, 0.1),
        energy_j: 4.0e5,
    });
    run_secs(&mut app, 0.2);
    assert!(!solid_at(&app, wall, Vec2::new(1.0, 1.2)));
    app.world
        .get_mut::<DestructibleWall>(wall)
        .unwrap()
        .set_changed();
    run_secs(&mut app, 0.1);
    assert!(solid_at(&app, wall, Vec2::new(1.0, 1.2)), "repaired");
    assert!(
        !solid_at(&app, wall, Vec2::new(0.0, 1.0)),
        "the doorway is still there"
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
    // A floor lies flat (from z = 1 back to z = −1 here): shots straight down go through it.
    let floor = call_api(
        &mut app.world,
        "destruction.spawn_wall",
        json!({"preset": "floor", "at": [20, 2, 1], "size": [2, 2]}),
    )
    .unwrap()["id"]
        .as_u64()
        .unwrap();
    app.update();
    call_api(
        &mut app.world,
        "destruction.shoot",
        json!({"from": [20, 6, 0], "to": [20, -1, 0], "count": 3, "spread": 0.02}),
    )
    .unwrap();
    run_secs(&mut app, 0.3);
    let inspect = call_api(
        &mut app.world,
        "destruction.inspect",
        json!({"entity": floor}),
    )
    .unwrap();
    assert!(inspect["integrity"].as_f64().unwrap() < 1.0, "{inspect}");
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
    let reinforced: Vec<usize> = app
        .world
        .query::<&ReinforcedBy>()
        .iter(&app.world)
        .map(|b| b.0.len())
        .collect();
    assert_eq!(
        reinforced,
        [2],
        "bay 8's drywall has two panels up behind it"
    );
}

#[test]
fn the_breach_house_roof_is_breached_from_below_and_above_every_cycle() {
    let mut app = app();
    app.add_systems(Startup, |mut commands: Commands| {
        demo::spawn_demo(&mut commands, DVec3::new(100.0, 0.0, 0.0));
    });
    let wall = |app: &mut App, name: &str| {
        app.world
            .query::<(Entity, &Name)>()
            .iter(&app.world)
            .find(|(_, n)| n.as_str() == name)
            .map(|(e, _)| e)
            .unwrap_or_else(|| panic!("no {name}"))
    };
    let integrity = |app: &App, e: Entity| app.world.get::<WallState>(e).unwrap().integrity();
    run_secs(&mut app, 0.2);
    let old = wall(&mut app, "Breach house roof bay 1");
    let roof = wall(&mut app, "Breach house roof bay 3");
    let shot = wall(&mut app, "Breach house roof bay 4");
    let hatch = wall(&mut app, "Wooden hatch");
    let steel = wall(&mut app, "Steel hatch");
    let bay2 = wall(&mut app, "Breach house roof bay 2");
    // Both hatch ladders can be climbed, from a step off the floor up to the deck behind them.
    let ladders: Vec<_> = app
        .world
        .query::<(&engine_world::Climbable, &GlobalTransform)>()
        .iter(&app.world)
        .map(|(c, gt)| c.in_world(gt))
        .collect();
    assert_eq!(ladders.len(), 2);
    for l in &ladders {
        assert!((l.bottom.y - 0.27).abs() < 0.02, "{l:?}");
        assert!(l.top.y > 2.9 && l.top.y < l.exit.y, "{l:?}");
        assert!((l.exit.y - 3.29).abs() < 0.02, "onto the deck: {l:?}");
        assert!(l.exit.z < l.top.z - 0.4, "over the header behind it: {l:?}");
        assert!(l.facing.distance(Vec3::NEG_Z) < 1e-4, "{l:?}");
    }
    assert!(
        integrity(&app, old) < 0.995,
        "the west bay starts shot up ({})",
        integrity(&app, old)
    );
    // The roof charge goes off at 6.2 s.
    run_secs(&mut app, 6.3);
    assert!(
        integrity(&app, shot) < 0.999,
        "shot up from below ({})",
        integrity(&app, shot)
    );
    assert!(
        integrity(&app, roof) < 0.97,
        "a charge on top blew a hole ({})",
        integrity(&app, roof)
    );
    run_secs(&mut app, 7.0);
    assert!(
        integrity(&app, hatch) < 0.7,
        "the wooden hatch is smashed in ({})",
        integrity(&app, hatch)
    );
    assert!(
        integrity(&app, steel) < 0.45,
        "the steel hatch is cut out ({})",
        integrity(&app, steel)
    );
    run_secs(&mut app, 7.0);
    for e in [roof, shot, hatch, steel] {
        assert!(integrity(&app, e) > 0.999, "rebuilt for the next cycle");
    }
    // The hatch opening in its bay is built in, so it is back after every rebuild too.
    let s = app.world.get::<WallState>(bay2).unwrap();
    let idx = s.grid.cell_at(Vec2::new(0.0, 4.68)).unwrap();
    assert!(!s.grid.solid[idx], "the hatch opening is open");
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
    let built = |app: &mut App| {
        app.world
            .query_filtered::<&BuiltPiece, With<StructuralPiece>>()
            .iter(&app.world)
            .count()
    };
    assert_eq!(built(&mut app), 32, "the yard's cabin is up");
    let ghosts: Vec<usize> = app
        .world
        .query::<&build::BuildGhost>()
        .iter(&app.world)
        .map(|g| g.level)
        .collect();
    assert_eq!(ghosts, vec![5], "the deck's third row is a red ghost");
    let doors: Vec<(Door, Transform)> = app
        .world
        .query::<(&Door, &Transform)>()
        .iter(&app.world)
        .map(|(d, t)| (*d, *t))
        .collect();
    assert_eq!(doors.len(), 1, "a door hangs in the cabin's doorway");
    let (door, t) = doors[0];
    assert!(
        door.is_open() && door.toward > 0.0 && t.translation.z > 0.3,
        "the cabin's door swung open, out into the yard: {door:?} at {t:?}"
    );
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
    let showing = app
        .world
        .query_filtered::<&ShowSupport, With<BuiltPiece>>()
        .iter(&app.world)
        .filter(|s| s.on)
        .count();
    assert_eq!(showing, 32, "the finished cabin shows its support colors");
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
    // The terrace's shelter stands on the leveled rock: its stone floor is grounded (blue).
    let hill = app
        .world
        .query::<&hill::HillDemo>()
        .iter(&app.world)
        .next()
        .unwrap()
        .clone();
    let terrace = hill.builders[hill::Site::Terrace as usize].unwrap();
    let shelter: Vec<(BuildKind, PieceState, bool)> = app
        .world
        .query_filtered::<(&BuiltPiece, &PieceState, &ShowSupport), Without<Debris>>()
        .iter(&app.world)
        .filter(|(b, ..)| b.builder == Some(terrace))
        .map(|(b, s, show)| (b.kind, *s, show.on))
        .collect();
    assert_eq!(shelter.len(), 10, "the terrace shelter is up: {shelter:?}");
    for (kind, state, shown) in &shelter {
        assert!(shown, "the shelter shows its support");
        assert!(state.collapse_in.is_none(), "{kind:?} stands: {state:?}");
        if *kind == BuildKind::StoneFloor {
            assert_eq!(state.support, state.max_support, "on the rock: {state:?}");
        } else {
            assert!(
                state.support >= 0.7 * state.max_support,
                "{kind:?} is green: {state:?}"
            );
        }
    }
    // The yard's builder put its support view away and built stairs up to the deck's end: a
    // flight on the ground and one from its head to the deck's edge, both standing.
    let yard = hill.builders[hill::Site::Yard as usize].unwrap();
    let yard_pieces: Vec<(BuildKind, PieceState, bool)> = app
        .world
        .query_filtered::<(&BuiltPiece, &PieceState, &ShowSupport), Without<Debris>>()
        .iter(&app.world)
        .filter(|(b, ..)| b.builder == Some(yard))
        .map(|(b, s, show)| (b.kind, *s, show.on))
        .collect();
    assert_eq!(yard_pieces.len(), 34, "the cabin and its stairs");
    assert!(
        yard_pieces.iter().all(|(.., shown)| !shown),
        "the cabin is back in its own colors"
    );
    let stairs: Vec<&PieceState> = yard_pieces
        .iter()
        .filter(|(k, ..)| *k == BuildKind::WoodStair)
        .map(|(_, s, _)| s)
        .collect();
    assert_eq!(stairs.len(), 2, "two flights of stairs");
    for state in stairs {
        assert!(
            state.collapse_in.is_none() && state.support >= 0.45 * state.max_support,
            "the stairs stand: {state:?}"
        );
    }
    let yard_ghosts = app
        .world
        .query::<&build::BuildGhost>()
        .iter(&app.world)
        .filter(|g| g.builder == yard)
        .count();
    assert_eq!(yard_ghosts, 0, "done, the yard's builder shows no ghost");

    // Then a pit is dug under it and the whole shelter drops in.
    run(&mut app, 4.3);
    let fallen = app
        .world
        .query_filtered::<&BuiltPiece, With<Debris>>()
        .iter(&app.world)
        .filter(|b| b.builder == Some(terrace))
        .count();
    assert_eq!(fallen, 10, "the shelter fell into the pit dug under it");

    run(&mut app, 2.2);
    let rebuilt = volume(&mut app);
    assert!(
        (rebuilt - start).abs() < 1.0,
        "the loop rebuilt the hill ({start} -> {raised} -> {rebuilt})"
    );
    assert!(built(&mut app) <= 1, "the cabin starts over");
}

#[test]
fn pieces_stand_on_voxel_rock_until_it_is_dug_out_from_under_them() {
    let mut app = app();
    let rock = VoxelVolume {
        dims: [16, 8, 16],
        voxel_size: 0.25,
        shape: VoxelShape::Solid,
        topsoil: None,
        ..VoxelVolume::default()
    };
    app.world.spawn((Transform::default(), rock));
    app.update();
    // A builder lays a stone slab on the rock's top (2 m up, far above the flat ground).
    let b = app
        .world
        .spawn((Transform::default(), Builder::default()))
        .id();
    {
        let mut builder = app.world.get_mut::<Builder>(b).unwrap();
        builder.kind = BuildKind::StoneFloor;
        builder.aim_at(DVec3::new(0.3, 5.0, 0.6), DVec3::new(0.0, 1.0, 0.0));
    }
    app.update();
    let p = app
        .world
        .get::<build::BuildPreview>(b)
        .unwrap()
        .placement
        .unwrap();
    assert!(p.valid() && p.grounded, "rock is ground: {p:?}");
    app.world.write_message(BuildRequest {
        builder: b,
        action: BuildAction::Place,
    });
    run_secs(&mut app, 0.3);
    let slab = app
        .world
        .query_filtered::<Entity, With<BuiltPiece>>()
        .single(&app.world)
        .unwrap();
    let state = *app.world.get::<PieceState>(slab).unwrap();
    assert_eq!(state.support, state.max_support, "{state:?}");
    // Dig the rock out from under it: it falls.
    for x in [-0.6, 0.6] {
        for z in [-0.6, 0.6] {
            app.world.write_message(TerrainEdit {
                center: DVec3::new(x, 1.4, z),
                shape: EditShape::Sphere { radius: 0.9 },
                op: EditOp::Remove,
                surface: None,
                source: None,
            });
        }
    }
    run_secs(&mut app, 1.0);
    assert!(
        app.world.get::<StructuralPiece>(slab).is_none(),
        "the slab fell into the hole"
    );
    // It comes to rest in the hole, on the rock (not through it, on the flat ground under it).
    run_secs(&mut app, 4.0);
    let y = app.world.get::<Transform>(slab).unwrap().translation.y;
    let body = app.world.get::<debris::DebrisBody>(slab).unwrap();
    assert!(
        y > 0.45 && y < 2.0 && body.velocity.length() < 0.5,
        "the slab lies in the hole at y {y:.2}: {body:?}"
    );
}

/// Physics queries that see one character, a ball at a point (boxes only).
struct OneCharacter(Entity, DVec3, f32);

impl PhysicsQuery for OneCharacter {
    fn raycast(
        &self,
        _: DVec3,
        _: Vec3,
        _: f32,
        _: &QueryFilter,
    ) -> Option<engine_world::physics::Hit> {
        None
    }
    fn shapecast(
        &self,
        _: &ColliderShape,
        _: DVec3,
        _: engine_core::math::Quat,
        _: Vec3,
        _: f32,
        _: &QueryFilter,
    ) -> Option<engine_world::physics::Hit> {
        None
    }
    fn overlap(
        &self,
        shape: &ColliderShape,
        position: DVec3,
        rotation: engine_core::math::Quat,
        _: &QueryFilter,
        out: &mut Vec<Entity>,
    ) {
        let ColliderShape::Cuboid { half_extents } = shape else {
            return;
        };
        let local = rotation.inverse() * (self.1 - position).as_vec3();
        if local.clamp(-*half_extents, *half_extents).distance(local) < self.2 {
            out.push(self.0);
        }
    }
    fn closest_point(
        &self,
        _: DVec3,
        _: f32,
        _: &QueryFilter,
    ) -> Option<engine_world::physics::Hit> {
        None
    }
}

#[test]
fn build_mode_does_not_put_a_piece_where_someone_stands() {
    let mut app = app();
    let player = app
        .world
        .spawn((
            Transform::from_xyz(0.0, 0.9, 0.0),
            CharacterController::default(),
        ))
        .id();
    let stand_at = |app: &mut App, at: DVec3| {
        app.insert_resource(PhysicsQueries(std::sync::Arc::new(OneCharacter(
            player, at, 0.35,
        ))));
    };
    stand_at(&mut app, DVec3::new(0.0, 0.9, 0.0));
    let b = app
        .world
        .spawn((
            Transform::default(),
            Builder {
                kind: BuildKind::WoodWall,
                ..Builder::default()
            },
        ))
        .id();
    app.world
        .get_mut::<Builder>(b)
        .unwrap()
        .aim_at(DVec3::new(0.3, 3.0, 2.0), DVec3::ZERO);
    let place = |app: &mut App| {
        app.update();
        let p = app
            .world
            .get::<build::BuildPreview>(b)
            .unwrap()
            .placement
            .unwrap();
        app.world.write_message(BuildRequest {
            builder: b,
            action: BuildAction::Place,
        });
        app.update();
        let built = app
            .world
            .query_filtered::<(), With<BuiltPiece>>()
            .iter(&app.world)
            .count();
        (p, built)
    };
    // A wall through the player: red ghost, and placing does nothing.
    let (p, built) = place(&mut app);
    assert_eq!(p.problem, Some(build::BuildProblem::Occupied), "{p:?}");
    assert_eq!(built, 0);
    // Once they step aside it goes up.
    stand_at(&mut app, DVec3::new(3.0, 0.9, 0.0));
    let (p, built) = place(&mut app);
    assert!(p.valid(), "{p:?}");
    assert_eq!(built, 1);
}

#[test]
fn doors_hang_in_doorways_swing_away_from_you_and_fall_with_them() {
    let mut app = app();
    app.add_plugins(engine_input::InputPlugin);
    app.insert_resource(PhysicsBackend {
        name: "test".into(),
    });
    let table = app.world.resource::<DestructionMaterials>().clone();
    let doorway = |app: &mut App, at: DVec3| {
        let (piece, _) = build::built_piece(
            BuildKind::WoodDoorway,
            Transform::from_translation(at),
            None,
            false,
            &table,
        );
        app.world.spawn(piece).id()
    };
    // A doorway standing on the ground, its front toward +Z.
    let frame = doorway(&mut app, DVec3::new(0.0, 1.0, 0.0));
    app.update();
    let door = app
        .world
        .get::<door::HungDoor>(frame)
        .expect("a door hangs in the doorway")
        .0;
    // Shut, a box fills the opening; the door is a kinematic body moved by its Transform, so
    // it shoves what it swings into.
    assert_eq!(
        app.world.get::<RigidBody>(door).map(|b| b.kind),
        Some(BodyKind::Kinematic)
    );
    let Some(Collider {
        shape: ColliderShape::Cuboid { half_extents },
        ..
    }) = app.world.get::<Collider>(door)
    else {
        panic!("no box collider on the door");
    };
    assert!(
        half_extents.x > 0.4 && half_extents.y > 0.9,
        "{half_extents}"
    );
    let open_state = |app: &App| *app.world.get::<Door>(door).unwrap();
    let leaf_z = |app: &App| app.world.get::<Transform>(door).unwrap().translation.z;
    assert_eq!(open_state(&app).open, 0.0);
    assert!(leaf_z(&app).abs() < 0.01, "shut, it sits in the wall");

    // Someone in front presses interact: it swings in, away from them; again, it shuts.
    let walker = app
        .world
        .spawn((
            Transform::from_xyz(0.3, 1.0, 1.2),
            CharacterController::default(),
        ))
        .id();
    app.update();
    let interact = |app: &mut App| {
        app.world
            .resource_mut::<engine_input::ActionState>()
            .tap("interact", 1);
        run_secs(app, 1.2);
    };
    interact(&mut app);
    let d = open_state(&app);
    assert!(d.is_open() && d.toward < 0.0, "{d:?}");
    assert!(leaf_z(&app) < -0.3, "swung in to {}", leaf_z(&app));
    interact(&mut app);
    assert_eq!(open_state(&app).open, 0.0, "shut again");
    // From behind it swings out through the front.
    app.world.get_mut::<Transform>(walker).unwrap().translation = DVec3::new(0.3, 1.0, -1.2);
    app.update();
    interact(&mut app);
    let d = open_state(&app);
    assert!(d.is_open() && d.toward > 0.0, "{d:?}");
    assert!(leaf_z(&app) > 0.3, "swung out to {}", leaf_z(&app));
    // Out of reach, interact does nothing.
    app.world.get_mut::<Transform>(walker).unwrap().translation = DVec3::new(0.3, 1.0, -6.0);
    app.update();
    interact(&mut app);
    assert!(open_state(&app).is_open());
    // AI agents open and shut it by where they stand.
    let r = call_api(
        &mut app.world,
        "destruction.door",
        json!({"near": [0.3, 1.0, 1.2], "open": false}),
    )
    .unwrap();
    assert_eq!(r["opening"], json!(false));
    run_secs(&mut app, 1.2);
    assert_eq!(open_state(&app).open, 0.0);

    // A doorway hanging in the air falls; its door swings shut and falls with it.
    let high = doorway(&mut app, DVec3::new(6.0, 5.0, 0.0));
    app.update();
    let high_door = app.world.get::<door::HungDoor>(high).unwrap().0;
    run_secs(&mut app, 1.0);
    assert!(app.world.get::<Debris>(high).is_some(), "the doorway fell");
    let d = *app.world.get::<Door>(high_door).unwrap();
    assert!(d.loose && d.open == 0.0, "{d:?}");
    assert!(
        app.world.get::<Collider>(high_door).is_none()
            && app.world.get::<RigidBody>(high_door).is_none(),
        "the door rides its doorway down, no body of its own"
    );
    // Taken down, a doorway takes its door with it.
    app.world.despawn(frame);
    app.update();
    assert!(app.world.get_entity(door).is_err());
}

#[test]
fn standing_pieces_are_solid_with_a_physics_module() {
    // With no physics module, pieces get no colliders (nothing would read them).
    let mut bare = app();
    let floor = bare
        .world
        .spawn((
            Transform::default(),
            StructuralPiece::new(DestructibleMaterial::Wood, Vec3::new(2.0, 0.1, 2.0)),
        ))
        .id();
    bare.update();
    assert!(bare.world.get::<Collider>(floor).is_none());

    let mut app = app();
    app.insert_resource(PhysicsBackend {
        name: "test".into(),
    });
    let b = app
        .world
        .spawn((Transform::default(), Builder::default()))
        .id();
    for (kind, at) in [
        (BuildKind::WoodFloor, [0.0, 0.0, 0.0]),
        (BuildKind::WoodGable, [5.0, 0.0, 0.0]),
        (BuildKind::WoodStair, [10.0, 0.0, 0.0]),
    ] {
        {
            let mut builder = app.world.get_mut::<Builder>(b).unwrap();
            builder.kind = kind;
            let to = DVec3::from_array(at);
            builder.aim_at(to + DVec3::new(0.3, 3.0, 0.6), to);
        }
        app.update();
        app.world.write_message(BuildRequest {
            builder: b,
            action: BuildAction::Place,
        });
        app.update();
    }
    // A beam hanging in the air: it falls as soon as the solver sees it.
    let beam = app
        .world
        .spawn((
            Transform::from_xyz(0.0, 6.0, 8.0),
            StructuralPiece::new(DestructibleMaterial::Wood, Vec3::new(2.0, 0.2, 0.2)),
        ))
        .id();
    app.update();
    let wood = app.world.resource::<SurfaceMaterials>().id_or_default(
        &app.world
            .resource::<DestructionMaterials>()
            .get(DestructibleMaterial::Wood)
            .surface,
    );
    let built: Vec<(Entity, BuildKind)> = app
        .world
        .query::<(Entity, &BuiltPiece)>()
        .iter(&app.world)
        .map(|(e, p)| (e, p.kind))
        .collect();
    assert_eq!(built.len(), 3, "{built:?}");
    for (e, kind) in built {
        let c = app
            .world
            .get::<Collider>(e)
            .expect("a standing piece is solid");
        assert_eq!(c.surface, wood, "footsteps on it sound like wood");
        let h = app.world.get::<StructuralPiece>(e).unwrap().half_extents;
        match (kind, &c.shape) {
            // Its box, a skin smaller so snapped neighbors don't overlap.
            (BuildKind::WoodFloor, ColliderShape::Cuboid { half_extents }) => {
                let skin = h - *half_extents;
                assert!(
                    skin.min_element() > 0.005 && skin.max_element() < 0.02,
                    "{h} vs {half_extents}"
                );
            }
            // The triangle under the roof, not the box (which would stick up through it).
            (BuildKind::WoodGable, ColliderShape::ConvexHull { points }) => {
                assert_eq!(points.len(), 6);
                let low_end_top = points
                    .iter()
                    .filter(|p| p.x < 0.0)
                    .map(|p| p.y)
                    .fold(f32::MIN, f32::max);
                assert!(low_end_top < -h.y * 0.9, "{points:?}");
            }
            // A sloping slab whose top is a ramp through the treads: half a step up at the
            // foot (+Z), level with the top at the head, and open underneath the head.
            (BuildKind::WoodStair, ColliderShape::ConvexHull { points }) => {
                let at = |near_foot: bool| {
                    points
                        .iter()
                        .filter(|p| {
                            if near_foot {
                                p.z > h.z * 0.95
                            } else {
                                p.z < -h.z * 0.95
                            }
                        })
                        .map(|p| p.y)
                        .fold((f32::MAX, f32::MIN), |(lo, hi), y| (lo.min(y), hi.max(y)))
                };
                let (foot_lo, foot_hi) = at(true);
                let (head_lo, head_hi) = at(false);
                assert!(foot_lo < -h.y + 0.03 && foot_hi < -h.y + 0.12, "{points:?}");
                assert!(head_hi > h.y - 0.02, "{points:?}");
                assert!(head_lo > -h.y + 0.5, "{points:?}");
            }
            other => panic!("unexpected shape {other:?}"),
        }
    }
    // Once a piece gives way it is a debris body with its own hull, not a static box.
    assert!(matches!(
        app.world.get::<Collider>(beam).map(|c| &c.shape),
        Some(ColliderShape::Cuboid { .. })
    ));
    run_secs(&mut app, 2.0);
    assert!(
        app.world.get::<StructuralPiece>(beam).is_none(),
        "the beam fell"
    );
    assert!(app.world.get::<RigidBody>(beam).is_some());
    assert!(matches!(
        app.world.get::<Collider>(beam).map(|c| &c.shape),
        Some(ColliderShape::ConvexHull { .. })
    ));

    // Props that break keep their simple shape until they do; set dressing is solid as
    // drawn (a triangle mesh of its model).
    let crate_e = app
        .world
        .spawn((
            Transform::from_xyz(0.0, 0.4, -6.0),
            Fracturable {
                shape: FractureShape::Box {
                    half_extents: Vec3::splat(0.4),
                },
                material: DestructibleMaterial::Wood,
                ..Fracturable::default()
            },
        ))
        .id();
    let pillar = app
        .world
        .spawn((
            Transform::from_xyz(4.0, 1.9, -6.0),
            dress::PropModel::new(
                props::PropKind::ConcretePillar {
                    size: [0.7, 3.8, 0.7],
                },
                1,
            ),
        ))
        .id();
    let ladder = app
        .world
        .spawn((
            Transform::from_xyz(8.0, 1.6, -6.0),
            dress::PropModel::new(props::PropKind::SteelLadder { height: 3.18 }, 2),
            engine_world::Climbable::ladder(Vec3::ZERO, Vec3::Y * 3.0, Vec3::NEG_Z),
        ))
        .id();
    app.update();
    let rungs = app
        .world
        .get::<Collider>(ladder)
        .expect("the ladder has a surface");
    assert!(
        rungs.sensor && !app.world.get::<Collider>(pillar).unwrap().sensor,
        "a climber passes through the ladder's box, not the pillar's"
    );
    assert_eq!(
        app.world.get::<Collider>(crate_e).map(|c| c.shape.clone()),
        Some(ColliderShape::Cuboid {
            half_extents: Vec3::splat(0.4)
        })
    );
    let Some(ColliderShape::TriMesh { vertices, indices }) =
        app.world.get::<Collider>(pillar).map(|c| c.shape.clone())
    else {
        panic!("the pillar is solid");
    };
    let top = vertices.iter().map(|v| v.y).fold(f32::MIN, f32::max);
    assert!(!indices.is_empty() && (top - 1.9).abs() < 0.1, "top {top}");
}

#[test]
fn build_mode_places_snapped_pieces_and_takes_them_down() {
    let mut app = app();
    let b = app
        .world
        .spawn((
            Transform::default(),
            Builder {
                kind: BuildKind::StoneFloor,
                ..Builder::default()
            },
        ))
        .id();
    let aim = |app: &mut App, kind: BuildKind, from: [f64; 3], to: [f64; 3]| {
        let mut builder = app.world.get_mut::<Builder>(b).unwrap();
        builder.kind = kind;
        builder.aim_at(DVec3::from_array(from), DVec3::from_array(to));
    };
    let send = |app: &mut App, action: BuildAction| {
        app.world.write_message(BuildRequest { builder: b, action });
        app.update();
    };
    let standing = |app: &mut App| -> Vec<(Entity, DVec3)> {
        app.world
            .query_filtered::<(Entity, &Transform), (With<BuiltPiece>, With<StructuralPiece>)>()
            .iter(&app.world)
            .map(|(e, t)| (e, t.translation))
            .collect()
    };
    // A stone slab on the ground.
    aim(
        &mut app,
        BuildKind::StoneFloor,
        [0.4, 3.0, 0.8],
        [0.0, 0.0, 0.0],
    );
    app.update();
    let p = app
        .world
        .get::<build::BuildPreview>(b)
        .unwrap()
        .placement
        .unwrap();
    assert!(p.valid() && p.grounded, "{p:?}");
    send(&mut app, BuildAction::Place);
    assert_eq!(standing(&mut app).len(), 1);
    // A board wall aimed near the slab's back edge snaps onto it.
    aim(
        &mut app,
        BuildKind::WoodWall,
        [0.3, 3.0, 0.1],
        [0.1, 0.4, -0.85],
    );
    send(&mut app, BuildAction::Place);
    let pieces = standing(&mut app);
    assert_eq!(pieces.len(), 2);
    let wall = pieces
        .iter()
        .find(|(_, at)| at.y > 1.0)
        .copied()
        .expect("a wall");
    assert!(
        wall.1.distance(DVec3::new(0.0, 1.4, -1.0)) < 1e-3,
        "on the slab's back edge: {:?}",
        wall.1
    );
    let fuel = app
        .world
        .get::<engine_world::Combustible>(wall.0)
        .unwrap()
        .fuel_kg;
    assert!(
        (fuel - 90.0).abs() < 1.0,
        "boards hold half a solid box's wood ({fuel} kg)"
    );
    // Aimed at the same spot again, the ghost moves on to the next free snap.
    send(&mut app, BuildAction::Place);
    let pieces = standing(&mut app);
    assert_eq!(pieces.len(), 3);
    assert!(
        pieces
            .iter()
            .all(|(e, at)| *e == wall.0 || at.distance(wall.1) > 0.5),
        "not inside the first wall: {pieces:?}"
    );
    // A wall floating in the air can't be placed either.
    aim(
        &mut app,
        BuildKind::WoodWall,
        [10.0, 5.0, 0.0],
        [10.0, 0.0, 0.0],
    );
    app.update();
    let p = app
        .world
        .get::<build::BuildPreview>(b)
        .unwrap()
        .placement
        .unwrap();
    assert!(p.valid() && p.grounded, "on the ground it can: {p:?}");
    // Take the slab down: the wall it carried loses its support and falls.
    aim(
        &mut app,
        BuildKind::WoodWall,
        [0.4, 3.0, 0.8],
        [0.3, 0.4, 0.3],
    );
    send(&mut app, BuildAction::Remove);
    run_secs(&mut app, 1.5);
    assert!(standing(&mut app).is_empty(), "the wall fell with the slab");
    assert!(app.world.get::<Debris>(wall.0).is_some());

    // The AI's way: aim and place in one call, or just look.
    let out = call_api(
        &mut app.world,
        "destruction.build",
        json!({"kind": "stone_wall", "at": [5, 0, 5], "yaw_deg": 90}),
    )
    .unwrap();
    assert_eq!(out["valid"], true, "{out}");
    assert!(out["id"].is_u64(), "{out}");
    assert!(
        (out["yaw_deg"].as_f64().unwrap() - 90.0).abs() < 1e-3,
        "{out}"
    );
    let preview = call_api(
        &mut app.world,
        "destruction.build",
        json!({"kind": "wood_floor", "from": [5, 4, 9], "to": [5, 1.0, 5.2], "preview": true}),
    )
    .unwrap();
    assert!(preview.get("id").is_none(), "{preview}");
    assert_eq!(preview["snapped"], true, "{preview}");
    let err = call_api(
        &mut app.world,
        "destruction.build",
        json!({"kind": "castle"}),
    )
    .unwrap_err();
    assert!(err.contains("wood_floor"), "{err}");
    let gone = call_api(
        &mut app.world,
        "destruction.unbuild",
        json!({"entity": out["id"]}),
    );
    assert!(gone.is_ok(), "{gone:?}");
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
