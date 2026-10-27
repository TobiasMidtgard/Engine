//! The proving ground keeps playing: stations rebuild on their cycles without piling up
//! bodies (and pause when nobody is near), and the replay API restarts one now.

use engine_core::math::{DVec3, Vec3};
use engine_core::prelude::*;
use engine_core::registry::call_api;
use engine_core::time::TimeSource;
use engine_physics::demo::{self, Station};
use engine_physics::{PhysicsPlugin, PhysicsStats};
use engine_world::physics::{BodyKind, Collider, ColliderShape, RigidBody, Velocity};
use engine_world::{SimulationFocus, WorldPlugin, WorldShift};
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
    // Only the cradle, pendulums and balls that ask for fine steps take them, far away too.
    // (The yard's character kept it coarse while they dropped to half rate, and the two
    // regions merged at the tiers' wide margin: all 1400 bodies stepped finely, 10× the cost.)
    let stats = *app.world.resource::<PhysicsStats>();
    assert!(
        stats.fine_awake * 10 < stats.awake.max(1),
        "{} of {} awake bodies step finely",
        stats.fine_awake,
        stats.awake
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
    // Only the run: what other stations throw can land in a lane (a crate of the wrecking
    // ball's wall hops the ramp's low cheek some runs) and stop a drum where it should.
    let others: Vec<Entity> = app
        .world
        .query::<(Entity, &Transform, &RigidBody)>()
        .iter(&app.world)
        .filter(|(e, t, rb)| {
            rb.kind == BodyKind::Dynamic
                && !drums.contains(e)
                && (t.translation.z - base.z).abs() > 4.0
        })
        .map(|(e, ..)| e)
        .collect();
    for e in others {
        app.world.despawn(e);
    }
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

/// The catapult (`demo::catapult`): the counterweight swings the arm up into its crossbar, the
/// ball leaves the cup at the speed the falling counterweight's energy gives it (less a little
/// lost to the hinge and the cup), flies high over the frame and brings down the top of the
/// crate wall down range, and the arm stays on its axle.
#[test]
fn the_catapult_throws_its_ball_into_the_crate_wall() {
    use demo::catapult;
    let mut app = app();
    app.update();
    let base = Station::Catapult.base() + DVec3::Y * demo::SLAB_RISE;
    let axle = base + DVec3::Y * catapult::AXLE as f64;
    let ball = named(&mut app, "Catapult ball")[0];
    let arm = named(&mut app, "Catapult arm")[0];
    let crates = named(&mut app, "Catapult target crate");
    let start: Vec<DVec3> = crates
        .iter()
        .map(|&c| app.world.get::<Transform>(c).unwrap().translation)
        .collect();
    let (mut release, mut highest) = (0.0f32, f64::MIN);
    for _ in 0..(8 * 60) {
        app.update();
        let p = app.world.get::<Transform>(ball).unwrap().translation - base;
        // Behind the axle the ball is in the cup or just out of it, climbing and slowing: its
        // fastest there is its speed leaving the cup.
        if p.x < 0.0 {
            let v = app.world.get::<Velocity>(ball).unwrap().linear.length();
            release = release.max(v);
        }
        highest = highest.max(p.y);
        let hub = app.world.get::<Transform>(arm).unwrap().translation;
        assert!(
            (hub - axle).length() < 0.03,
            "the arm left its axle by {:.3} m",
            (hub - axle).length()
        );
    }
    let r = app.world.get::<Transform>(arm).unwrap().rotation;
    let (axis, angle) = r.to_axis_angle();
    let angle = angle * axis.z.signum();
    assert!(
        (angle - catapult::STOP).abs() < 0.03,
        "the arm came to rest at {:.1}°, not against the crossbar at {:.1}°",
        angle.to_degrees(),
        catapult::STOP.to_degrees()
    );
    let ideal = catapult::release_speed();
    assert!(
        release > 0.95 * ideal && release < 1.01 * ideal,
        "the ball left at {release:.2} m/s; the counterweight's energy gives {ideal:.2} m/s"
    );
    assert!(highest > 7.0, "the ball only rose to {highest:.1} m");
    let mut fell = 0;
    for (&c, s) in crates.iter().zip(&start) {
        let p = app.world.get::<Transform>(c).unwrap().translation;
        // The yard is built around the origin here.
        assert!(
            p.x.abs() < demo::YARD_SIZE[0] * 0.5 && p.z.abs() < demo::YARD_SIZE[1] * 0.5,
            "a crate left the yard: at {p}"
        );
        if s.y - p.y > 0.4 {
            fell += 1;
        }
    }
    assert!(
        fell >= 4,
        "the ball brought down only {fell} of the wall's {} crates",
        crates.len()
    );
}

/// Giant Jenga: the rod pokes the three loose middle blocks out and the tower stands on, every
/// other block where it was; then it pushes out a block the tower rests on and the top half
/// comes down, all of it staying near the tower.
#[test]
fn the_jenga_tower_stands_three_pokes_and_falls_at_the_fourth() {
    use demo::jenga;
    let mut app = app();
    app.update();
    let base = Station::Jenga.base() + DVec3::Y * demo::SLAB_RISE;
    let mut blocks = Vec::new();
    for level in 0..jenga::LEVELS {
        for slot in [-1, 0, 1] {
            let e = named(&mut app, &format!("Jenga block {level} {slot}"))[0];
            let at = base + jenga::block(level, slot).0.as_dvec3();
            blocks.push((level, slot, e, at));
        }
    }
    let at = |app: &App, e: Entity| app.world.get::<Transform>(e).unwrap().translation;
    let poked = |level: usize, slot: i32| jenga::POKES[..3].contains(&(level, slot));
    // Just before the last poke: the three loose blocks are out on the slab, the rest stand.
    let before_last = (jenga::last_poke() * 60.0) as usize;
    app.update_n(before_last);
    let mut worst = 0.0f64;
    for &(level, slot, e, start) in &blocks {
        let p = at(&app, e);
        if poked(level, slot) {
            assert!(
                p.y - base.y < 0.5 && p.x - base.x > 0.5,
                "poked block {level} {slot} is at {:?}",
                p - base
            );
        } else {
            worst = worst.max((p - start).length());
        }
    }
    assert!(
        worst < 0.03,
        "a block of the standing tower moved {:.1} cm",
        worst * 100.0
    );
    // The last poke: the top half falls.
    app.update_n(12 * 60);
    let top: Vec<f64> = blocks
        .iter()
        .filter(|b| b.0 == jenga::LEVELS - 1)
        .map(|b| at(&app, b.2).y - base.y)
        .collect();
    let spread = blocks
        .iter()
        .map(|b| (at(&app, b.2) - base).with_y(0.0).length())
        .fold(0.0f64, f64::max);
    assert!(
        top.iter().all(|y| *y < 1.0),
        "the top level is still up at {top:?}"
    );
    assert!(
        spread < 6.0,
        "a block ended up {spread:.1} m from the tower"
    );
}

/// Someone walks into the resting yard to the ragdoll stairs and back out, the camera with
/// them: stations wake, regions split off and merge round the walker, and the stations rest
/// again behind them. (The exe crashed in Rapier's island bookkeeping while zones woke; the
/// cause, a region stepped with no time in it before its first real step, is pinned down in
/// `sim.rs`'s tests.)
#[test]
fn a_walker_wakes_the_resting_yard_and_leaves_it_resting() {
    use engine_world::physics::{CharacterController, CharacterMove};
    let mut app = app();
    // Ground round the yard, flush with its top.
    app.world.spawn((
        Transform::from_xyz(0.0, demo::SLAB_RISE - 0.5, 0.0),
        RigidBody::fixed(),
        Collider::cuboid(Vec3::new(300.0, 0.5, 300.0)),
    ));
    let start = DVec3::new(130.0, 1.0, -10.0);
    let walker = app
        .world
        .spawn((
            Transform::from_translation(start),
            CharacterController::default(),
            CharacterMove {
                velocity: Vec3::ZERO,
            },
        ))
        .id();
    let focus = app
        .world
        .spawn((
            Transform::from_translation(start),
            SimulationFocus::default(),
        ))
        .id();
    // Nobody near: the stations rest.
    app.update_n(10 * 60);
    let resting = *app.world.resource::<PhysicsStats>();
    assert!(resting.awake < 100, "the yard didn't rest: {resting:?}");
    // Walk in to the ragdoll stairs and back out, the camera following.
    let mut woke = false;
    for frame in 0..20 * 60 {
        let x = if frame < 9 * 60 { -12.0 } else { 12.0 };
        app.world.get_mut::<CharacterMove>(walker).unwrap().velocity = Vec3::new(x, 0.0, 0.0);
        app.update();
        let at = app.world.get::<Transform>(walker).unwrap().translation;
        app.world.get_mut::<Transform>(focus).unwrap().translation = at;
        woke |= app.world.resource::<PhysicsStats>().awake > 500;
    }
    assert!(woke, "the yard never woke round the walker");
    let at = app.world.get::<Transform>(walker).unwrap().translation;
    assert!(
        at.x > 90.0,
        "the walker didn't get back out of the yard: {at}"
    );
    // Gone again: the stations rest.
    app.world.get_mut::<CharacterMove>(walker).unwrap().velocity = Vec3::ZERO;
    app.update_n(10 * 60);
    let again = *app.world.resource::<PhysicsStats>();
    assert!(again.awake < 100, "the yard didn't rest again: {again:?}");
}

/// Heights of the bodies over the pile's bin: (mean, highest).
fn pile_heights(app: &mut App) -> (f64, f64) {
    let at = Station::Pile.base();
    let ys: Vec<f64> = app
        .world
        .query::<(&Transform, &RigidBody)>()
        .iter(&app.world)
        .filter(|(t, rb)| {
            rb.kind == BodyKind::Dynamic
                && (t.translation.x - at.x).abs() < 5.0
                && (t.translation.z - at.z).abs() < 5.0
        })
        .map(|(t, _)| t.translation.y)
        .collect();
    let mean = ys.iter().sum::<f64>() / ys.len().max(1) as f64;
    (mean, ys.iter().copied().fold(f64::MIN, f64::max))
}

/// A yard built with nobody near rests at once, but what its stations drop doesn't hang in
/// the air: the pile's crates, built in a grid above their bin, settle into it off the frame.
#[test]
fn a_yard_built_far_away_rests_with_its_crates_down() {
    let mut app = app();
    app.world.spawn((
        Transform::from_xyz(2_000.0, 0.0, 0.0),
        SimulationFocus::default(),
    ));
    app.update();
    let (mean, top) = pile_heights(&mut app);
    let started = std::time::Instant::now();
    let (mut now_mean, mut now_top) = (mean, top);
    while started.elapsed().as_secs() < 600 && now_mean > mean - 1.0 {
        app.update();
        (now_mean, now_top) = pile_heights(&mut app);
    }
    assert!(
        now_mean < mean - 1.0 && now_top < top - 1.0,
        "the pile's crates still hang in the air: mean {mean:.1} -> {now_mean:.1} m, top \
         {top:.1} -> {now_top:.1} m"
    );
    eprintln!(
        "pile settled in {:.1} s: mean {mean:.1} -> {now_mean:.1} m, top {top:.1} -> {now_top:.1} m",
        started.elapsed().as_secs_f64()
    );
    // Down where they landed, held asleep.
    app.update_n(60);
    let after = pile_heights(&mut app);
    assert!((after.0 - now_mean).abs() < 0.05, "{after:?} vs {now_mean}");
    let stats = *app.world.resource::<PhysicsStats>();
    assert!(stats.awake < 100, "{stats:?}");
}

/// A yard built while somebody is far away already (the showcase's camera is there before its
/// zones are): its stations rest as they are built, before their bodies are made, and those
/// bodies start asleep and stay so. Making them used to forget they were told to rest, so the
/// whole yard ran live, 150 m from the camera, at full cost.
#[test]
fn a_yard_built_after_the_camera_rests_at_once() {
    let mut app = App::new();
    app.add_plugins(WorldPlugin);
    app.add_plugins(PhysicsPlugin);
    app.world
        .resource_mut::<Time>()
        .set_source(TimeSource::Manual {
            step_secs: 1.0 / 60.0,
        });
    app.world.resource_mut::<FixedTime>().set_hz(60.0);
    app.world.spawn((
        Transform::from_xyz(150.0, 1.7, 0.0),
        SimulationFocus::default(),
    ));
    app.update_n(3);
    let mut commands = app.world.commands();
    demo::spawn_demo(&mut commands, DVec3::ZERO);
    app.world.flush();
    app.update_n(60);
    for _ in 0..120 {
        app.update();
        let stats = *app.world.resource::<PhysicsStats>();
        assert!(stats.bodies > 1000 && stats.awake == 0, "{stats:?}");
    }
}

/// A resting yard beside something that keeps moving (a turntable by the camera, 130 m off,
/// within the simulation range) leaves the turntable's region for a dormant one that sits
/// steps out, so the turntable no longer steps a thousand sleeping bodies with it. Rapier walks a region's sleeping pairs
/// every step; Box2D keeps sleeping islands in solver sets no step touches.
#[test]
fn a_resting_yard_leaves_the_region_of_a_turntable() {
    let mut app = App::new();
    app.add_plugins(WorldPlugin);
    app.add_plugins(PhysicsPlugin);
    app.world
        .resource_mut::<Time>()
        .set_source(TimeSource::Manual {
            step_secs: 1.0 / 60.0,
        });
    app.world.resource_mut::<FixedTime>().set_hz(60.0);
    app.world.spawn((
        Transform::from_xyz(130.0, 1.7, 0.0),
        SimulationFocus::default(),
    ));
    let turntable = app
        .world
        .spawn((
            Transform::from_xyz(130.0, 0.5, 5.0),
            RigidBody {
                kind: BodyKind::Kinematic,
                ..Default::default()
            },
            Collider::cuboid(Vec3::new(2.0, 0.1, 2.0)),
            Velocity {
                linear: Vec3::ZERO,
                angular: Vec3::Y,
            },
        ))
        .id();
    app.update_n(3);
    let mut commands = app.world.commands();
    demo::spawn_demo(&mut commands, DVec3::ZERO);
    app.world.flush();
    app.update_n(120);
    let stats = *app.world.resource::<PhysicsStats>();
    assert!(
        stats.bodies > 1000 && stats.dormant + 1 >= stats.bodies && stats.resting_awake == 0,
        "the yard should be dormant: {stats:?}"
    );
    let turned = app.world.get::<Transform>(turntable).unwrap().rotation;
    app.update_n(30);
    let now = app.world.get::<Transform>(turntable).unwrap().rotation;
    assert!(turned.angle_between(now) > 0.2, "the turntable stopped");
    let stats = *app.world.resource::<PhysicsStats>();
    assert!(stats.dormant + 1 >= stats.bodies, "{stats:?}");
}

/// A Galton board nobody has come near rests poured (`galton::poured`): every ball still, in
/// the bins on the printed curve, rather than held in the hopper and the pins, ready to pour
/// on whenever something woke them (the lead's PC loops found 400 pouring 150 m away).
#[test]
fn a_galton_board_nobody_has_seen_rests_poured() {
    use demo::galton;
    let mut app = App::new();
    app.add_plugins(WorldPlugin);
    app.add_plugins(PhysicsPlugin);
    app.world
        .resource_mut::<Time>()
        .set_source(TimeSource::Manual {
            step_secs: 1.0 / 60.0,
        });
    app.world.resource_mut::<FixedTime>().set_hz(60.0);
    app.world.spawn((
        Transform::from_xyz(300.0, 1.7, 0.0),
        SimulationFocus::default(),
    ));
    let mut commands = app.world.commands();
    demo::spawn_demo(&mut commands, DVec3::ZERO);
    app.world.flush();
    app.update_n(30);
    let origin = Station::Galton.base() + DVec3::Y * demo::SLAB_RISE;
    let balls = named(&mut app, "Galton ball");
    assert_eq!(balls.len(), galton::BALLS);
    let at = |app: &App| -> Vec<DVec3> {
        balls
            .iter()
            .map(|&b| app.world.get::<Transform>(b).unwrap().translation - origin)
            .collect()
    };
    let before = at(&app);
    let mut bins = [0usize; galton::BINS];
    for p in &before {
        assert!(
            p.x.abs() < galton::HALF_WIDTH as f64
                && p.z.abs() < galton::SLOT as f64
                && p.y > galton::FLOOR as f64
                && p.y < galton::BIN_TOP as f64,
            "a ball isn't in a bin: at {p}"
        );
        bins[galton::bin_of(p.x as f32)] += 1;
    }
    for (j, (&n, want)) in bins.iter().zip(galton::expected()).enumerate() {
        assert!(
            (n as f32 - want).abs() <= 1.0,
            "bin {j} holds {n}, the curve {want:.1}: {bins:?}"
        );
    }
    app.update_n(60);
    let moved = before
        .iter()
        .zip(at(&app))
        .map(|(a, b)| a.distance(b))
        .fold(0.0, f64::max);
    assert!(moved < 0.001, "a resting ball moved {moved} m");
}

/// After the world shifts (a re-homing moved the yard 3 km and 125 m down), the yard plays on
/// where it now is: the Jenga rod keeps to its tower (it was driven back to the old frame
/// every step and hung 125 m up), and a replayed station is built on the moved slab.
#[test]
fn the_yard_follows_a_world_shift() {
    let mut app = app();
    app.update();
    // The corner of the Galton hopper's heap of balls, as built.
    let hopper = |app: &mut App| {
        named(app, "Galton ball")
            .into_iter()
            .map(|e| app.world.get::<Transform>(e).unwrap().translation)
            .fold(DVec3::splat(f64::INFINITY), |a, b| a.min(b))
    };
    let built = hopper(&mut app);
    app.update_n(60);
    let at = |app: &mut App, name: &str| {
        let e = named(app, name)[0];
        app.world.get::<Transform>(e).unwrap().translation
    };
    let rod_from_tower = at(&mut app, "Jenga rod") - at(&mut app, "Jenga block 0 0");
    let s = WorldShift::translate(DVec3::new(3000.0, -125.0, 40.0));
    app.world.write_message(s);
    app.update();
    app.update_n(120);
    let now = at(&mut app, "Jenga rod") - at(&mut app, "Jenga block 0 0");
    assert!(
        (now - rod_from_tower).length() < 5.0,
        "the Jenga rod is {:?} from its tower, was {rod_from_tower:?}",
        now
    );
    let r = call_api(
        &mut app.world,
        "physics.proving_ground.replay",
        json!({ "station": "galton" }),
    );
    assert!(r.is_ok(), "{r:?}");
    app.update();
    let off = hopper(&mut app) - s.point(built);
    assert!(
        off.length() < 0.05,
        "the replayed Galton board is {off:?} off the moved yard"
    );
}
