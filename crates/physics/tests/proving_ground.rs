//! The proving ground keeps playing: stations rebuild on their cycles without piling up
//! bodies (and pause when nobody is near), and the replay API restarts one now.

use engine_core::math::{DVec3, Vec3};
use engine_core::prelude::*;
use engine_core::registry::call_api;
use engine_core::time::TimeSource;
use engine_physics::demo::{self, Station};
use engine_physics::{PhysicsPlugin, PhysicsStats};
use engine_world::physics::{BodyKind, Collider, ColliderShape, RigidBody, Velocity};
use engine_world::{SimulationFocus, WorldPlugin};
use serde_json::json;

fn app() -> App {
    let mut app = App::new();
    app.add_plugins(WorldPlugin);
    app.add_plugins(PhysicsPlugin);
    app.world
        .resource_mut::<Time>()
        .set_source(TimeSource::Manual {
            step_secs: 1.0 / 60.0,
        });
    app.world.resource_mut::<FixedTime>().set_hz(60.0);
    let mut commands = app.world.commands();
    demo::spawn_demo(&mut commands, DVec3::ZERO);
    app.world.flush();
    app
}

fn named(app: &mut App, name: &str) -> Vec<Entity> {
    app.world
        .query::<(Entity, &Name)>()
        .iter(&app.world)
        .filter(|(_, n)| n.as_str() == name)
        .map(|(e, _)| e)
        .collect()
}

fn bodies(app: &mut App) -> usize {
    app.world.query::<&RigidBody>().iter(&app.world).count()
}

/// How far the domino's long axis leans from upright, radians.
fn lean(app: &App, e: Entity) -> f32 {
    let up = app.world.get::<Transform>(e).unwrap().rotation * Vec3::Y;
    up.angle_between(Vec3::Y)
}

#[test]
fn stations_replay_on_their_cycles() {
    let mut app = app();
    app.update();
    let total = bodies(&mut app);
    let first = named(&mut app, "First domino")[0];
    let cycle = Station::Dominoes.cycle_secs().unwrap();
    app.update_n(((cycle - 1.0) * 60.0) as usize);
    assert!(
        lean(&app, first) > 1.0,
        "the first domino stands after {} s",
        cycle - 1.0
    );
    // Step to the replay: the first domino is a new one, standing.
    let mut again = Vec::new();
    for _ in 0..2 * 60 {
        app.update();
        again = named(&mut app, "First domino");
        if again != [first] {
            break;
        }
    }
    assert_eq!(again.len(), 1, "dominoes piled up: {again:?}");
    assert_ne!(again[0], first, "the dominoes didn't replay");
    assert!(
        lean(&app, again[0]) < 0.1,
        "the new first domino isn't standing"
    );
    assert_eq!(bodies(&mut app), total, "replays changed the body count");
}

#[test]
fn far_away_proving_grounds_pause() {
    let mut app = app();
    // A camera 2 km away.
    app.world.spawn((
        Transform::from_xyz(2_000.0, 0.0, 0.0),
        SimulationFocus::default(),
    ));
    app.update();
    let first = named(&mut app, "First domino");
    let cycle = Station::Dominoes.cycle_secs().unwrap();
    app.update_n(((cycle + 2.0) * 60.0) as usize);
    assert_eq!(
        named(&mut app, "First domino"),
        first,
        "replayed with nobody near"
    );
}

#[test]
fn the_api_replays_a_station_now() {
    let mut app = app();
    app.update_n(60);
    let before = named(&mut app, "Ragdoll (red)");
    let total = bodies(&mut app);
    let r = call_api(
        &mut app.world,
        "physics.proving_ground.replay",
        json!({ "station": "ragdolls" }),
    )
    .unwrap();
    assert_eq!(r["stations"], json!(["ragdolls"]));
    assert_eq!(r["proving_grounds"], 1);
    app.update();
    let after = named(&mut app, "Ragdoll (red)");
    assert_eq!(after.len(), 1);
    assert_ne!(after, before, "the ragdolls weren't rebuilt");
    assert_eq!(bodies(&mut app), total);
    let err = call_api(
        &mut app.world,
        "physics.proving_ground.replay",
        json!({ "station": "trampoline" }),
    );
    assert!(
        err.as_ref().is_err_and(|e| e.contains("dominoes")),
        "{err:?}"
    );
}

/// The barrel run's drums roll down the ramp and the whole runway (one triangle mesh, an edge
/// every 2 m) flat and in their lanes, then stop in the sand pit.
#[test]
fn drums_roll_down_the_barrel_run_into_the_sand() {
    let mut app = app();
    app.update();
    let drums: Vec<Entity> = (1..=3)
        .map(|k| named(&mut app, &format!("Run drum {k}"))[0])
        .collect();
    let base = Station::BarrelRun.base();
    let local = |app: &App, e: Entity| app.world.get::<Transform>(e).unwrap().translation - base;
    // The runway's flat part (station x), its top, and the pit.
    let (flat, deck_y, pit) = ((-19.0, 24.0), 0.04, (25.0, 31.4));
    let (mut hop, mut drift, mut fastest) = (0.0f64, 0.0f64, 0.0f32);
    // Until just before the station replays.
    let secs = Station::BarrelRun.cycle_secs().unwrap() - 0.5;
    for _ in 0..(secs * 60.0) as usize {
        app.update();
        for (k, &d) in drums.iter().enumerate() {
            let p = local(&app, d);
            if p.x > flat.0 && p.x < flat.1 {
                hop = hop.max((p.y - demo::SLAB_RISE - (deck_y + 0.3)).abs());
                drift = drift.max((p.z - (k as f64 - 1.0) * 1.8).abs());
            }
            let v = app.world.get::<Velocity>(d).unwrap().linear.length();
            fastest = fastest.max(v);
        }
    }
    assert!(fastest > 4.0, "the drums never got going: {fastest} m/s");
    assert!(hop < 0.01, "the drums hopped {hop} m on the runway");
    assert!(
        drift < 0.3,
        "the drums wandered {drift} m out of their lanes"
    );
    for &d in &drums {
        let p = local(&app, d);
        let v = app.world.get::<Velocity>(d).unwrap().linear.length();
        assert!(
            p.x > pit.0 && p.x < pit.1 && v < 0.05,
            "a drum isn't resting in the sand: at {p}, {v} m/s"
        );
    }
}

/// Ten crash test dummies stumble off the stairs' landing together, tumble down the flight,
/// and all come to rest and fall asleep well before the station replays (without the friction
/// in `cones.rs` and `rolling.rs`, a head or a limb kept them awake).
#[test]
fn ten_dummies_tumble_down_the_stairs_and_fall_asleep() {
    let mut app = app();
    app.update();
    let base = Station::Ragdolls.base();
    // The landing: 14 steps of 0.22 m up and 0.32 m deep from the station's base toward -z.
    let top = 0.22 * 14.0;
    let edge = -0.32 * 14.0;
    let local = |app: &App, e: Entity| app.world.get::<Transform>(e).unwrap().translation - base;
    let parts: Vec<Entity> = app
        .world
        .query::<(Entity, &RigidBody)>()
        .iter(&app.world)
        .filter(|(_, b)| b.kind == BodyKind::Dynamic)
        .map(|(e, _)| e)
        .filter(|&e| {
            let p = local(&app, e);
            p.y > top - 0.05 && p.x.abs() < 3.5 && p.z < edge + 0.1
        })
        .collect();
    let chests: Vec<Entity> = app
        .world
        .query::<(Entity, &Name)>()
        .iter(&app.world)
        .filter(|(_, n)| n.as_str().starts_with("Ragdoll ("))
        .map(|(e, _)| e)
        .collect();
    assert_eq!(chests.len(), 10, "dummies on the landing");
    assert_eq!(parts.len(), 110, "dummy parts on the landing");
    // Asleep from then on, with time to spare before the replay.
    let deadline = Station::Ragdolls.cycle_secs().unwrap() - 5.0;
    let (mut asleep_since, mut lowest) = (None, f64::MAX);
    for f in 1..=(deadline * 60.0) as usize {
        app.update();
        let awake = parts
            .iter()
            .any(|&e| *app.world.get::<Velocity>(e).unwrap() != Velocity::default());
        if awake {
            asleep_since = None;
        } else {
            asleep_since.get_or_insert(f as f64 / 60.0);
        }
        for &e in &parts {
            lowest = lowest.min(local(&app, e).y);
        }
    }
    let at: Vec<DVec3> = chests.iter().map(|&c| local(&app, c)).collect();
    println!("asleep since {asleep_since:?} s; chests at {at:?}");
    assert!(
        asleep_since.is_some(),
        "the dummies are still moving after {deadline} s"
    );
    assert!(
        at.iter().all(|p| p.y < top - 0.5),
        "a dummy stayed on the landing: {at:?}"
    );
    assert!(lowest > 0.02, "a dummy sank into the yard ({lowest} m)");
}

/// The rockfall's rocks all pour out of the hopper and off the chute, heap up at its foot (a
/// heap, not a sheet spread over the slab) and come to rest before the station replays.
#[test]
fn rocks_pour_down_the_chute_into_a_heap_and_come_to_rest() {
    let mut app = app();
    app.update();
    let base = Station::Rockfall.base() + DVec3::Y * demo::SLAB_RISE;
    let rocks: Vec<Entity> = app
        .world
        .query::<(Entity, &Transform, &Collider)>()
        .iter(&app.world)
        .filter(|(_, t, c)| {
            matches!(c.shape, ColliderShape::ConvexHull { .. })
                && t.translation.distance(base) < 15.0
        })
        .map(|(e, _, _)| e)
        .collect();
    assert_eq!(rocks.len(), 300);
    let secs = Station::Rockfall.cycle_secs().unwrap() - 1.0;
    app.update_n((secs * 60.0) as usize);
    let local: Vec<DVec3> = rocks
        .iter()
        .map(|&e| app.world.get::<Transform>(e).unwrap().translation - base)
        .collect();
    // The chute's lip is 1.6 m up at x = 0; it runs up toward -x.
    let left = local.iter().filter(|p| p.x < -0.2 && p.y > 1.2).count();
    assert!(
        left <= 5,
        "{left} rocks stayed in the hopper or on the chute"
    );
    let top = local.iter().map(|p| p.y).fold(0.0, f64::max);
    assert!(top > 0.6, "the heap is only {top:.2} m high (rock centers)");
    let near = local
        .iter()
        .filter(|p| p.x > -1.0 && p.x < 4.5 && p.z.abs() < 3.0)
        .count();
    assert!(
        near >= 270,
        "only {near} of 300 rocks heaped within 4.5 m of the chute"
    );
    for &e in &rocks {
        let v = app.world.get::<Velocity>(e).unwrap();
        assert!(
            v.linear.length() < 0.01 && v.angular.length() < 0.05,
            "a rock is still moving: {v:?}"
        );
    }
}

/// The pool table's break (`demo::pool`): the cue ball at 8 m/s opens the rack, a few balls
/// drop into the pockets, the rest spread over the cloth, and everything is still before the
/// table resets.
#[test]
fn the_break_scatters_the_rack_into_the_pockets_and_the_balls_stop() {
    let mut app = app();
    app.update();
    let origin = Station::Pool.base() + DVec3::Y * demo::SLAB_RISE;
    let cloth = origin.y + demo::pool::BED as f64;
    let balls: Vec<(String, Entity)> = (0..16)
        .map(|n| {
            let name = match n {
                0 => "Cue ball".to_string(),
                n => format!("Pool ball {n}"),
            };
            let found = named(&mut app, &name);
            assert_eq!(found.len(), 1, "{name}: {found:?}");
            (name, found[0])
        })
        .collect();
    let at = |app: &App, e: Entity| app.world.get::<Transform>(e).unwrap().translation;
    let start: Vec<DVec3> = balls.iter().map(|(_, b)| at(&app, *b)).collect();
    // The cue ball reaches the rack after about 0.16 s; the pack steps finely meanwhile.
    app.update_n(18);
    let s = *app.world.resource::<PhysicsStats>();
    assert!(s.fine_awake >= 16, "{s:?}");
    // The table resets 25 s after it was built.
    app.update_n(20 * 60);
    let [hl, hw] = demo::pool::HALF.map(|h| h as f64);
    let r = demo::pool::BALL as f64;
    let (mut pocketed, mut moved) = (0, 0);
    for (i, (name, b)) in balls.iter().enumerate() {
        let p = at(&app, *b);
        let d = p - origin;
        let v = app.world.get::<Velocity>(*b).unwrap();
        assert!(
            v.linear.length() < 0.01 && v.angular.length() < 0.5,
            "{name} is still rolling: {v:?}"
        );
        // On the cloth inside the cushions, or down a pocket on the catch under the table.
        let on_cloth = (p.y - cloth - r).abs() < 0.005 && d.x.abs() < hl && d.z.abs() < hw;
        let in_pocket = p.y < cloth - 0.1
            && p.y > cloth - 0.25
            && d.x.abs() < hl + 0.18
            && d.z.abs() < hw + 0.18;
        assert!(on_cloth || in_pocket, "{name} left the table: at {d}");
        pocketed += in_pocket as usize;
        if i > 0 && (p - start[i]).length() > 0.3 {
            moved += 1;
        }
    }
    assert!(pocketed >= 1, "the break pocketed nothing");
    assert!(pocketed <= 6, "{pocketed} balls went down");
    assert!(moved >= 12, "only {moved} of the 15 balls spread out");
}

/// The Galton board (`demo::galton`): the balls pour through the pins without jamming, all (one
/// at most may jam, as on a real board; with contacts reused for 5 cm, four to ten hung on old
/// contact planes) end in the bins and come to rest, and the bins hold a bell curve about the
/// middle, spread about as wide as the curve printed behind them.
#[test]
fn the_galton_board_fills_its_bins_in_a_bell_curve() {
    use demo::galton;
    let mut app = app();
    app.update();
    let origin = Station::Galton.base() + DVec3::Y * demo::SLAB_RISE;
    let balls = named(&mut app, "Galton ball");
    assert_eq!(balls.len(), galton::BALLS);
    // The board replays 40 s after it was built; the pour is over in about 20.
    app.update_n(30 * 60);
    let mut bins = [0usize; galton::BINS];
    let mut caught = Vec::new();
    for &b in &balls {
        let p = app.world.get::<Transform>(b).unwrap().translation - origin;
        let v = app.world.get::<Velocity>(b).unwrap();
        assert!(
            p.x.abs() < galton::HALF_WIDTH as f64 && p.z.abs() < galton::SLOT as f64,
            "a ball left the board: at {p}"
        );
        assert!(v.linear.length() < 0.05, "a ball is still moving: {v:?}");
        if p.y > galton::BIN_TOP as f64 {
            caught.push(p);
        } else {
            bins[galton::bin_of(p.x as f32)] += 1;
        }
    }
    assert!(
        caught.len() <= 1,
        "{} balls are caught on the pins: {caught:?}",
        caught.len()
    );
    let n = bins.iter().sum::<usize>() as f64;
    let mid = (galton::BINS / 2) as f64;
    let mean = bins
        .iter()
        .enumerate()
        .map(|(j, &c)| j as f64 * c as f64)
        .sum::<f64>()
        / n;
    let spread = (bins
        .iter()
        .enumerate()
        .map(|(j, &c)| (j as f64 - mean).powi(2) * c as f64)
        .sum::<f64>()
        / n)
        .sqrt();
    let near = |r: std::ops::RangeInclusive<usize>| bins[r].iter().sum::<usize>() as f64 / n;
    let last = galton::BINS - 1;
    assert!(
        (mean - mid).abs() < 0.6,
        "the heap is off center: {mean:.2} {bins:?}"
    );
    assert!(
        (2.5..4.0).contains(&spread),
        "the heap's spread is {spread:.2} bins, the curve's {}: {bins:?}",
        galton::SPREAD
    );
    assert!(near(7..=9) > 0.28, "the middle bins hold too few: {bins:?}");
    assert!(
        near(0..=1) + near(last - 1..=last) < 0.08,
        "the end bins hold too many: {bins:?}"
    );
    assert!(
        (near(0..=7) - near(9..=last)).abs() < 0.12,
        "the heap leans to one side: {bins:?}"
    );
}
