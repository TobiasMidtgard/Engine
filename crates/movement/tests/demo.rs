//! The showcase demo plays out as designed: the three duel rounds (an untrained player is cut
//! down in a blink; trained, the same elite looks slow and gets countered; a pack of grunts
//! falls one cinematic beat at a time) and the parkour runner's full route.

mod support;

use engine_core::math::DVec3;
use engine_core::prelude::*;
use engine_movement::combat::{CombatLog, CombatState, Outcome};
use engine_movement::demo::{self, Brain, DuelDirector, Round};
use engine_movement::{Mover, PerceptionStatus};
use engine_world::{AttackEvent, LocomotionMode};
use support::*;

/// Attack telegraphs seen: (attacker, world seconds, real seconds).
#[derive(Resource, Default)]
struct Telegraphs(Vec<(Entity, f64, f64)>);

fn collect_attacks(
    time: Res<Time>,
    mut seen: ResMut<Telegraphs>,
    mut events: MessageReader<AttackEvent>,
) {
    let (world, real) = (time.elapsed_secs_f64(), time.real_elapsed_secs_f64());
    seen.0
        .extend(events.read().map(|e| (e.attacker, world, real)));
}

fn demo_app(only: Option<Round>) -> (App, DuelDirector) {
    let mut app = app();
    app.init_resource::<Telegraphs>()
        .add_systems(Last, collect_attacks);
    with_commands(&mut app, |c| demo::spawn_demo(c, DVec3::ZERO));
    let mut q = app.world.query::<&mut DuelDirector>();
    let mut d = q.single_mut(&mut app.world).unwrap();
    if let Some(r) = only {
        d.round = r;
        d.only = Some(r);
    }
    let d = d.clone();
    (app, d)
}

fn combat(app: &App, e: Entity) -> CombatState {
    app.world.get::<CombatState>(e).unwrap().clone()
}

fn log(app: &App) -> String {
    let names = |id: Option<u64>| {
        id.and_then(Entity::try_from_bits)
            .and_then(|e| app.world.get::<Name>(e))
            .map_or("?".to_string(), |n| n.as_str().to_string())
    };
    app.world
        .resource::<CombatLog>()
        .records
        .iter()
        .map(|r| {
            format!(
                "  {:.2}s real: {} → {}: {:?} {:?} ({})",
                r.real_secs,
                names(Some(r.attacker_id)),
                names(r.target_id),
                r.kind,
                r.outcome,
                r.damage
            )
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn duelist(app: &App, e: Entity) -> demo::DuelistBrain {
    match app.world.get::<Brain>(e).unwrap() {
        Brain::Duelist(b) => *b,
        other => panic!("not a duelist: {other:?}"),
    }
}

#[test]
fn blink_an_elite_cuts_down_an_untrained_player_before_they_can_react() {
    let (mut app, d) = demo_app(Some(Round::Blink));
    let (player, elite) = (d.player.unwrap(), d.elite.unwrap());
    run(&mut app, 3.0);
    let pc = combat(&app, player);
    let brain = duelist(&app, player);
    assert!(
        pc.hits_taken >= 1,
        "the elite hit the player:\n{}",
        log(&app)
    );
    let hit_after = brain.hit_after.expect("hit time recorded");
    assert!(
        hit_after < demo::REACTION_SECS,
        "hit {hit_after:.2} real s after the telegraph, faster than a {} s reaction",
        demo::REACTION_SECS
    );
    assert_eq!(brain.reacted_after, None, "no time to react");
    assert_eq!(combat(&app, elite).hits_taken, 0);
    let status = app.world.resource::<PerceptionStatus>();
    assert_eq!(
        status.focus_count, 0,
        "an untrained eye gets no slow motion"
    );
}

#[test]
fn focus_a_trained_player_sees_the_same_elite_in_slow_motion_dodges_and_counters() {
    let (mut app, d) = demo_app(Some(Round::Focus));
    let (player, elite) = (d.player.unwrap(), d.elite.unwrap());
    let secs = run_until(&mut app, 8.0, |w| {
        w.get::<CombatState>(elite).unwrap().defeated
    });
    assert!(
        combat(&app, elite).defeated,
        "elite still standing after {secs:.1} s:\n{}",
        log(&app)
    );
    assert_eq!(
        combat(&app, player).hits_taken,
        0,
        "player was hit:\n{}",
        log(&app)
    );
    let brain = duelist(&app, player);
    let reacted = brain.reacted_after.expect("the player reacted");
    assert!(
        reacted >= demo::REACTION_SECS,
        "only after a human reaction time"
    );

    let status = app.world.resource::<PerceptionStatus>().clone();
    assert!(status.focus_count >= 1, "reflex slow motion engaged");
    let records = &app.world.resource::<CombatLog>().records;
    let elite_id = elite.to_bits();
    let player_id = player.to_bits();
    assert!(
        records
            .iter()
            .any(|r| r.attacker_id == elite_id
                && matches!(r.outcome, Outcome::Dodged | Outcome::Whiff)),
        "the elite's strike missed:\n{}",
        log(&app)
    );
    assert!(
        records
            .iter()
            .any(|r| r.attacker_id == player_id && r.outcome == Outcome::Counter),
        "the player countered:\n{}",
        log(&app)
    );
    // To the player the elite's attack took several times longer than it did for the world.
    let (_, tw, tr) = *app
        .world
        .resource::<Telegraphs>()
        .0
        .iter()
        .find(|(e, _, _)| *e == elite)
        .unwrap();
    let strike = records.iter().find(|r| r.attacker_id == elite_id).unwrap();
    let (world, real) = (strike.world_secs - tw, strike.real_secs - tr);
    assert!(
        real > 3.0 * world && real > 0.4,
        "elite's strike landed {real:.2} real s ({world:.3} world s) after its telegraph"
    );
}

#[test]
fn pack_every_grunt_falls_in_slow_motion() {
    let (mut app, d) = demo_app(Some(Round::Pack));
    let player = d.player.unwrap();
    let grunts = d.grunts.clone();
    let secs = run_until(&mut app, 17.0, |w| {
        grunts
            .iter()
            .all(|g| w.get::<CombatState>(*g).unwrap().defeated)
    });
    for g in &grunts {
        assert!(
            combat(&app, *g).defeated,
            "a grunt is standing after {secs:.1} s:\n{}",
            log(&app)
        );
    }
    assert_eq!(
        combat(&app, player).hits_taken,
        0,
        "player was hit:\n{}",
        log(&app)
    );
    let status = app.world.resource::<PerceptionStatus>();
    assert!(status.focus_count >= 4, "focus on every attack: {status:?}");
    assert!(
        status.cinematic_count >= 4,
        "a beat on every kill: {status:?}"
    );
}

#[test]
fn the_duel_cycles_through_all_three_rounds() {
    let (mut app, _) = demo_app(None);
    let secs = run_until(&mut app, 45.0, |w| {
        let mut q = w.query::<&DuelDirector>();
        q.single(w).unwrap().rounds_played >= 3
    });
    let mut q = app.world.query::<&DuelDirector>();
    let d = q.single(&app.world).unwrap();
    assert_eq!(d.rounds_played, 3, "after {secs:.1} s");
    assert_eq!(d.round, Round::Blink, "back to the start");
}

#[test]
fn the_runner_completes_the_parkour_route() {
    let (mut app, _) = demo_app(None);
    let mut q = app.world.query::<(Entity, &Name)>();
    let runner = q
        .iter(&app.world)
        .find(|(_, n)| n.as_str() == "Runner")
        .unwrap()
        .0;
    let mut modes = Vec::new();
    let mut top: f64 = 0.0;
    run_until(&mut app, 16.0, |w| {
        let m = w.get::<Mover>(runner).unwrap().mode;
        if modes.last() != Some(&m) {
            modes.push(m);
        }
        let t = w.get::<Transform>(runner).unwrap().translation;
        if w.get::<Mover>(runner).unwrap().grounded && (19.5..24.5).contains(&t.x) {
            top = top.max(t.y - 0.9);
        }
        false
    });
    let count = |name: &str| triggered(&app, runner, name);
    let summary = format!("modes {modes:?}");
    assert!(count("slide") >= 1, "{summary}");
    assert!(count("wallrun") >= 2, "{summary}");
    assert!(count("wall_jump") >= 1, "{summary}");
    assert!(count("mantle") >= 1, "{summary}");
    assert!(count("double_jump") >= 1, "{summary}");
    assert!(
        top >= demo::layout::TOWER_TOP - 0.05,
        "stood on the tower (top {top:.2} m): {summary}"
    );
    assert!(modes.contains(&LocomotionMode::Sprint));
}

#[test]
fn the_climber_catches_the_ledge_shimmies_around_the_corner_climbs_up_and_drops_off() {
    let (mut app, _) = demo_app(None);
    let mut q = app.world.query::<(Entity, &Name)>();
    let climber = q
        .iter(&app.world)
        .find(|(_, n)| n.as_str() == "Climber")
        .unwrap()
        .0;
    let origin = DVec3::ZERO;
    let top = demo::layout::LEDGE_TOP;
    let (mut hung_from, mut shimmied, mut stood_on_top, mut back_down) = (None, 0.0, false, false);
    run_until(&mut app, 12.0, |w| {
        let m = w.get::<Mover>(climber).unwrap();
        let p = w.get::<Transform>(climber).unwrap().translation - origin;
        if m.mode == LocomotionMode::Climb {
            let x0 = *hung_from.get_or_insert(p.x);
            shimmied = (p.x - x0).max(shimmied);
        }
        stood_on_top |= m.grounded && p.y > top + 0.8;
        back_down |= stood_on_top && m.grounded && p.y < 1.5;
        back_down
    });
    let summary = format!("hung from {hung_from:?}, shimmied {shimmied:.2} m");
    assert_eq!(triggered(&app, climber, "ledge_grab"), 1, "{summary}");
    assert!(shimmied > 2.5, "{summary}");
    assert_eq!(triggered(&app, climber, "ledge_corner"), 1, "{summary}");
    assert!(triggered(&app, climber, "mantle") >= 1, "{summary}");
    assert!(stood_on_top && back_down, "{summary}");
    assert_eq!(triggered(&app, climber, "jump"), 1, "{summary}");
}

#[test]
fn the_grappler_hooks_the_tower_climbs_over_and_rides_the_zip_line_down() {
    let (mut app, _) = demo_app(None);
    let mut q = app.world.query::<(Entity, &Name)>();
    let grappler = q
        .iter(&app.world)
        .find(|(_, n)| n.as_str() == "Grappler")
        .unwrap()
        .0;
    let top = demo::layout::GRAPPLE_TOP;
    let target = DVec3::from_array(demo::layout::GRAPPLE_TARGET);
    let (mut bit_at, mut highest, mut stood_on_top) = (None, 0.0, false);
    let (mut rode, mut fastest, mut landed) = (0.0, 0.0f32, None);
    run_until(&mut app, 16.0, |w| {
        let m = w.get::<Mover>(grappler).unwrap();
        let p = w.get::<Transform>(grappler).unwrap().translation;
        if let Some(g) = m.grapple.filter(|g| g.attached) {
            bit_at.get_or_insert(g.anchor);
        }
        if !stood_on_top {
            highest = p.y.max(highest);
        }
        stood_on_top |= m.grounded && p.y > top + 0.8;
        if let Some(z) = m.zip {
            rode = z.s.max(rode);
            fastest = fastest.max(z.speed);
        }
        if rode > 0.0 && m.grounded && p.y < 1.5 {
            landed.get_or_insert(p);
        }
        landed.is_some()
    });
    let summary = format!(
        "hook bit at {bit_at:?}, highest {highest:.2} m, rode to s = {rode:.2} at up to \
         {fastest:.1} m/s, landed at {landed:?}"
    );
    assert_eq!(triggered(&app, grappler, "grapple_fire"), 1, "{summary}");
    assert_eq!(triggered(&app, grappler, "grapple_hit"), 1, "{summary}");
    let bit = bit_at.expect("the hook bit");
    assert!(bit.distance(target) < 0.3, "{summary}");
    assert_eq!(triggered(&app, grappler, "mantle"), 1, "{summary}");
    // Over the edge, not flung high above the deck.
    assert!(highest < top + 0.9 + 1.0, "{summary}");
    assert!(stood_on_top, "{summary}");
    // Caught the zip line once, rode it nearly to the gate, fast, and let go there.
    assert_eq!(triggered(&app, grappler, "zip_grab"), 1, "{summary}");
    assert_eq!(triggered(&app, grappler, "zip_release"), 1, "{summary}");
    assert!(rode > 0.8, "{summary}");
    assert!(fastest > 12.0 && fastest < 18.0, "{summary}");
    // Down through the gate onto its pad.
    let landed = landed.expect(&summary);
    let pad = demo::layout::ZIP_PAD;
    for (i, p) in [landed.x, landed.z].into_iter().enumerate() {
        let (c, h) = (pad.center[i * 2], pad.size[i * 2] as f64 * 0.5);
        assert!((p - c).abs() < h - 0.3, "{summary}");
    }
    assert!(landed.y > pad.center[1] + 0.5, "{summary}");
}

#[test]
fn the_rider_wall_runs_along_the_moving_car_climbs_aboard_and_rides_it() {
    let (mut app, _) = demo_app(None);
    let mut q = app.world.query::<(Entity, &Name)>();
    let named = |w: &World, name: &str| {
        let mut q = w.try_query::<(Entity, &Name)>().unwrap();
        q.iter(w).find(|(_, n)| n.as_str() == name).unwrap().0
    };
    let _ = &mut q;
    let rider = named(&app.world, "Rider");
    let car = named(&app.world, "Freight car");
    let roof = demo::layout::FREIGHT_ROOF;
    let (mut ran_on_car, mut aboard_from, mut rode, mut landed) = (0.0, None, 0.0, None);
    let mut fastest: f32 = 0.0;
    run_until(&mut app, 16.0, |w| {
        let m = w.get::<Mover>(rider).unwrap().clone();
        let p = w.get::<Transform>(rider).unwrap().translation;
        let c = w.get::<Transform>(car).unwrap().translation;
        if m.mode == LocomotionMode::WallRun && m.platform.is_some_and(|d| d.entity == car) {
            ran_on_car += DT;
            let v = w
                .get::<engine_world::locomotion::LocomotionState>(rider)
                .unwrap()
                .velocity;
            fastest = fastest.max(v.x);
        }
        // Standing on the roof, over the car.
        if m.grounded && p.y > roof + 0.8 && (p.z - c.z).abs() < 1.5 {
            let start = *aboard_from.get_or_insert(c.x);
            rode = c.x - start;
        }
        if aboard_from.is_some() && m.grounded && p.y < roof - 1.0 {
            landed.get_or_insert(p);
        }
        landed.is_some()
    });
    let summary = format!(
        "wall-ran {ran_on_car:.2} s on the car (fastest {fastest:.1} m/s), rode it {rode:.1} m, \
         landed at {landed:?}"
    );
    assert!(ran_on_car > 0.1, "{summary}");
    // Faster along the car than a wall-run anywhere else: its speed plus the car's.
    assert!(fastest > 10.5, "{summary}");
    assert_eq!(triggered(&app, rider, "mantle"), 1, "{summary}");
    assert!(rode > 3.0, "{summary}");
    let landed = landed.expect(&summary);
    let platform = demo::layout::FREIGHT_PLATFORM;
    assert!(
        (landed.y - (platform.center[1] + 0.5 + 0.9)).abs() < 0.1,
        "back on the platform: {summary}"
    );
}

#[test]
fn the_duel_waits_while_no_camera_is_near() {
    let (mut app, d) = demo_app(None);
    let camera = app
        .world
        .spawn((
            engine_render::Camera::default(),
            Transform::from_xyz(0.0, 5.0, 200.0),
        ))
        .id();
    run(&mut app, 4.0);
    assert!(
        app.world.resource::<Telegraphs>().0.is_empty(),
        "nobody attacked"
    );
    assert_eq!(app.world.resource::<PerceptionStatus>().focus_count, 0);
    assert_eq!(app.world.resource::<Time>().scale(), 1.0);
    // The camera comes over: the fight starts.
    let near = DVec3::new(0.0, 3.0, -2.0);
    app.world
        .entity_mut(camera)
        .insert(Transform::from_translation(near));
    run(&mut app, 3.0);
    assert!(
        combat(&app, d.player.unwrap()).hits_taken >= 1,
        "{}",
        log(&app)
    );
}

#[test]
fn the_duel_api_restarts_a_chosen_round() {
    let (mut app, d) = demo_app(None);
    run(&mut app, 2.0);
    let r = engine_core::registry::call_api(
        &mut app.world,
        "movement.duel",
        serde_json::json!({"round": "Focus"}),
    )
    .unwrap();
    assert_eq!(r["duels"][0]["round"], "Focus");
    // The focus round plays from its setup: the trained player counters the elite.
    let elite = d.elite.unwrap();
    run_until(&mut app, 4.0, |w| {
        w.get::<CombatState>(elite).unwrap().defeated
    });
    assert!(combat(&app, elite).defeated, "{}", log(&app));
    assert!(
        engine_core::registry::call_api(
            &mut app.world,
            "movement.duel",
            serde_json::json!({"round": "Nope"})
        )
        .is_err()
    );
    let r = engine_core::registry::call_api(
        &mut app.world,
        "movement.duel",
        serde_json::json!({"round": null}),
    )
    .unwrap();
    assert_eq!(r["duels"][0]["only"], serde_json::Value::Null);
}

/// The set dressing is drawn around the demo's collider boxes and never changes them: the
/// demo spawns exactly the static boxes it always has (center, half extents), and nothing
/// else that is static and solid.
#[test]
fn the_demo_spawns_the_same_static_collider_boxes() {
    use engine_core::math::Quat;
    use engine_world::physics::{BodyKind, Collider, ColliderShape, RigidBody};
    let mut app = app();
    let origin = DVec3::new(100.0, 0.0, -40.0);
    with_commands(&mut app, |c| demo::spawn_demo(c, origin));
    let mut q = app.world.query::<(&Transform, &RigidBody, &Collider)>();
    let mut boxes: Vec<([f64; 3], [f32; 3])> = q
        .iter(&app.world)
        .filter(|(_, b, c)| b.kind == BodyKind::Static && !c.sensor)
        .map(|(t, _, c)| {
            assert_eq!(t.rotation, Quat::IDENTITY, "static boxes are axis aligned");
            let ColliderShape::Cuboid { half_extents } = c.shape else {
                panic!("static collider that isn't a box: {:?}", c.shape);
            };
            ((t.translation - origin).to_array(), half_extents.to_array())
        })
        .collect();
    // The demo's boxes before the course and ring were dressed (meters, from the origin).
    let mut expected: Vec<([f64; 3], [f32; 3])> = vec![
        ([0.0, -0.5, 0.0], [40.0, 0.5, 40.0]),      // ground
        ([0.0, 0.05, 12.0], [28.0, 0.05, 5.0]),     // runway
        ([-14.0, 1.35, 12.0], [0.15, 0.2, 3.5]),    // slide bar
        ([-14.0, 0.775, 8.5], [0.15, 0.775, 0.15]), // slide bar posts
        ([-14.0, 0.775, 15.5], [0.15, 0.775, 0.15]),
        ([-2.0, 2.6, 14.8], [6.0, 2.5, 0.3]),       // wall A
        ([12.5, 2.6, 9.2], [6.5, 2.5, 0.3]),        // wall B
        ([22.0, 2.75, 11.0], [3.0, 2.75, 3.5]),     // tower
        ([0.0, 0.05, -14.0], [8.0, 0.05, 8.0]),     // ring
        ([0.0, 0.15, -25.5], [8.0, 0.15, 1.5]),     // stands
        ([17.0, 0.05, 2.0], [6.0, 0.05, 3.5]),      // ledge station
        ([17.0, 1.9, 1.2], [3.0, 1.8, 0.8]),        // ledge block
        ([8.0, 0.05, 29.0], [5.0, 0.05, 11.0]),     // grapple pad
        ([8.0, 4.5, 31.0], [4.0, 4.5, 3.0]),        // grapple tower
        ([-14.0, 0.5, 25.4], [12.0, 0.5, 1.9]),     // freight platform
        ([33.75, 0.05, 20.75], [5.25, 0.05, 5.25]), // zip line pad
    ];
    // The posts of the zip line's gantry and gate.
    for p in demo::layout::zip_posts() {
        expected.push((p.center, p.size.map(|s| s * 0.5)));
    }
    let key = |b: &([f64; 3], [f32; 3])| {
        b.0.map(|v| (v * 1000.0).round() as i64)
            .into_iter()
            .chain(b.1.map(|v| (v * 1000.0).round() as i64))
            .collect::<Vec<_>>()
    };
    boxes.sort_by_key(key);
    expected.sort_by_key(key);
    assert_eq!(boxes.len(), expected.len(), "{boxes:?}");
    for (got, want) in boxes.iter().zip(&expected) {
        for i in 0..3 {
            assert!((got.0[i] - want.0[i]).abs() < 1e-9, "{got:?} vs {want:?}");
            assert!((got.1[i] - want.1[i]).abs() < 1e-6, "{got:?} vs {want:?}");
        }
    }
}
