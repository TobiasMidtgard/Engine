//! Squad tactics against a foe behind cover on the higher tiers (`besiege::plan_push`,
//! `difficulty`): archers and swordsmen of one squad against a player that went to ground
//! behind a waist-high wall, over several seeds, in a headless app (no GPU).
//!
//! * Very hard: the archer calls a swordsman round the wall ("Flush them out!") and holds its
//!   draw on the *other* edge; a player that breaks out that way is shot as it clears the
//!   wall. Two archers holding on the player loose together when it shows (a volley).
//! * Extreme: two swordsmen wait out of the player's sight while the archer lobs, then go
//!   round both edges together; a push whose flusher gets badly hurt falls back ("Fall
//!   back!") and tries again.
//! * Hard: nobody is sent round and nobody barks.

use engine_core::ecs::message::Messages;
use engine_core::prelude::*;
use engine_core::time::TimeSource;
use engine_npc::bark::{BarkLine, Barks};
use engine_npc::besiege::{Order, READY_SECS};
use engine_npc::lob::Lobs;
use engine_npc::perception::BodyHeight;
use engine_npc::*;
use engine_world::locomotion::{LocomotionMode, LocomotionState};
use engine_world::physics::Collider;
use engine_world::{
    CombatDifficulty, DamageEvent, DamageKind, DifficultyTier, SimulationFocus, WorldPlugin,
};

const STEP: f64 = 1.0 / 60.0;

fn app() -> App {
    let mut app = App::new();
    app.world
        .resource_mut::<Time>()
        .set_source(TimeSource::Manual { step_secs: STEP });
    app.add_plugins((WorldPlugin, NpcPlugin));
    for (_, schedule) in app.world.resource_mut::<Schedules>().iter_mut() {
        schedule.set_executor(engine_core::ecs::schedule::SingleThreadedExecutor::new());
    }
    app
}

fn wall(app: &mut App, center: DVec3, half: Vec3) {
    app.world
        .spawn((Transform::from_translation(center), Collider::cuboid(half)));
}

/// The player crouches at the origin behind a 1.4 m wall, 4.8 m wide, at z = 1.4.
const WALL_TOP: f64 = 1.4;
const WALL_HALF: f64 = 2.4;
const STANDING: f32 = 1.75;
/// Seen standing at first, then down from this time (s).
const DOWN_AT: f32 = 1.5;

fn yard(app: &mut App) {
    wall(
        app,
        DVec3::new(0.0, WALL_TOP * 0.5, 1.4),
        Vec3::new(WALL_HALF as f32, WALL_TOP as f32 * 0.5, 0.25),
    );
    for sx in [-1.0, 1.0] {
        wall(
            app,
            DVec3::new(3.5 * sx, 0.6, 13.0),
            Vec3::new(0.6, 0.6, 0.6),
        );
        wall(
            app,
            DVec3::new(6.5 * sx, 1.0, 15.5),
            Vec3::new(1.2, 1.0, 0.3),
        );
        wall(
            app,
            DVec3::new(8.0 * sx, 0.6, 9.0),
            Vec3::new(0.6, 0.6, 0.6),
        );
    }
}

fn fighter(
    app: &mut App,
    at: DVec3,
    weapon: NpcWeapon,
    skills: NpcSkills,
    seed: u64,
    lone_archers: bool,
) -> Entity {
    let mut t = Transform::from_translation(at);
    t.look_to(Vec3::NEG_Z, Vec3::Y);
    let e = app
        .world
        .spawn((
            t,
            Npc::default(),
            Faction::ENEMY,
            skills,
            weapon,
            Seed(hash_u64(seed * 31 + 7 + at.x.to_bits())),
        ))
        .id();
    // (Archers on their own besiege the player; a squad of archers would pin it and go round
    // it instead, its own plan.)
    if weapon.style == WeaponStyle::Melee || !lone_archers {
        app.world.entity_mut(e).insert(Squad(1));
    }
    let mut v = app.world.get_mut::<NpcVitals>(e).unwrap();
    v.max_health = 1000.0;
    v.health = 1000.0;
    e
}

/// What the player does.
#[derive(Debug, Clone, Copy, PartialEq)]
enum Player {
    /// Stays down behind the wall.
    Down,
    /// Comes up over the wall for 0.5 s every 4 s from 6 s.
    Shows,
    /// Stays down until a flusher is within 3 m, then runs out the other way, round the far
    /// edge and away, at 4 m/s.
    Breaks,
    /// Stays down, and badly hurts the first flusher that comes within 2.5 m of it.
    Fights,
}

/// What one fight showed.
#[derive(Debug, Default, Clone)]
struct Fight {
    /// When each swordsman was first sent round (s), the side of the wall it came round
    /// (sign of x when it got within 2.5 m of the player), and when.
    sent: Vec<Option<f32>>,
    round_side: Vec<Option<f64>>,
    reached: Vec<Option<f32>>,
    /// When each first waited for the go, and fell back (s).
    waited: Vec<Option<f32>>,
    fell_back: Vec<Option<f32>>,
    /// Each archer's watch while a flusher was out (the x of its last watched peek).
    watch_x: Vec<Option<f64>>,
    /// Barks heard (line, s).
    barks: Vec<(BarkLine, f32)>,
    /// Lobs that came down, and when the first did (s).
    lobs: u32,
    first_lob: Option<f32>,
    /// Breaking out: when it cleared the wall's edge, and when an arrow first hit it after.
    broke_at: Option<f32>,
    shot_after_break: Option<f32>,
    hit_after_break: Option<f32>,
    /// Shows over the wall: the times each archer's arrows hit the player during one (s).
    show_hits: Vec<Vec<f32>>,
    /// Hits on the player by each archer, all fight.
    archer_hits: Vec<u32>,
}

fn fight(
    tier: DifficultyTier,
    archers: &[DVec3],
    swords: &[DVec3],
    player_does: Player,
    seed: u64,
    secs: f32,
) -> Fight {
    let mut app = app();
    app.world.resource_mut::<CombatDifficulty>().tier = tier;
    yard(&mut app);
    let mut pt = Transform::from_translation(DVec3::ZERO);
    pt.look_to(Vec3::Z, Vec3::Y);
    let player = app
        .world
        .spawn((
            pt,
            Faction::PLAYER,
            LocomotionState::default(),
            BodyHeight(STANDING),
            SimulationFocus::default(),
        ))
        .id();
    // (A second archer is slow to react: alone it looses after a quick show is over.)
    let a: Vec<Entity> = archers
        .iter()
        .enumerate()
        .map(|(i, &p)| {
            let skills = NpcSkills {
                reaction_time: if i == 0 { 0.13 } else { SLOW },
                ..NpcSkills::master()
            };
            fighter(
                &mut app,
                p,
                NpcWeapon::bow(),
                skills,
                seed,
                archers.len() > 1,
            )
        })
        .collect();
    let s: Vec<Entity> = swords
        .iter()
        .map(|&p| {
            fighter(
                &mut app,
                p,
                NpcWeapon::sword(),
                NpcSkills::master(),
                seed,
                false,
            )
        })
        .collect();
    let mut hurts = app.world.resource::<Messages<DamageEvent>>().get_cursor();
    let mut traces = app
        .world
        .resource::<Messages<engine_world::ProjectileTrace>>()
        .get_cursor();
    let mut f = Fight {
        sent: vec![None; s.len()],
        round_side: vec![None; s.len()],
        reached: vec![None; s.len()],
        waited: vec![None; s.len()],
        fell_back: vec![None; s.len()],
        watch_x: vec![None; a.len()],
        archer_hits: vec![0; a.len()],
        ..Default::default()
    };
    let mut heard = 0;
    let mut show: Option<usize> = None;
    let mut run_dir: Option<f64> = None;
    let mut hurt_one = false;
    let frames = (secs * 60.0) as usize;
    for frame in 0..frames {
        let t = frame as f32 / 60.0;
        let p_at = app.world.get::<Transform>(player).unwrap().translation;
        // (Breaking out, it keeps low behind the wall and stands once round its edge.)
        let up = t < DOWN_AT
            || (player_does == Player::Shows && t >= 6.0 && (t - 6.0) % 4.0 < 0.5)
            || (run_dir.is_some() && p_at.x.abs() > WALL_HALF + 0.3);
        app.world.get_mut::<LocomotionState>(player).unwrap().mode = if up {
            LocomotionMode::Idle
        } else {
            LocomotionMode::Crouch
        };
        if player_does == Player::Shows && up && t >= 6.0 {
            if show.is_none() {
                f.show_hits.push(Vec::new());
                show = Some(f.show_hits.len() - 1);
            }
        } else {
            show = None;
        }
        if let Some(dir) = run_dir {
            // Round the far edge and away from the wall.
            let mut p = p_at;
            if p.x.abs() < WALL_HALF + 4.0 {
                p.x += dir * 4.0 * STEP;
            } else {
                p.z -= 4.0 * STEP;
            }
            if p.x.abs() > WALL_HALF + 0.3 && f.broke_at.is_none() {
                f.broke_at = Some(t);
            }
            app.world.get_mut::<Transform>(player).unwrap().translation = p;
            app.world
                .get_mut::<LocomotionState>(player)
                .unwrap()
                .velocity = ((p - p_at) / STEP).as_vec3();
        }
        app.update();
        let msgs = app.world.resource::<Messages<DamageEvent>>();
        for h in hurts.read(msgs) {
            if h.target != player {
                continue;
            }
            if let Some(i) = a.iter().position(|&e| Some(e) == h.source) {
                f.archer_hits[i] += 1;
                if let Some(k) = show {
                    f.show_hits[k].push(t);
                }
                if let Some(b) = f.broke_at
                    && f.hit_after_break.is_none()
                {
                    f.hit_after_break = Some(t - b);
                }
            }
        }
        let msgs = app
            .world
            .resource::<Messages<engine_world::ProjectileTrace>>();
        for tr in traces.read(msgs) {
            if let Some(b) = f.broke_at
                && f.shot_after_break.is_none()
                && a.iter().any(|&e| tr.shooter == Some(e))
                && tr.speed > 59.0
            {
                f.shot_after_break = Some(t - b);
            }
        }
        let barks = app.world.resource::<Barks>();
        for b in barks.said.iter().skip(heard) {
            f.barks.push((b.line, t));
        }
        heard = barks.said.len();
        let lobs = app.world.resource::<Lobs>();
        f.lobs = lobs.landed.len() as u32;
        if f.first_lob.is_none() && !lobs.landed.is_empty() {
            f.first_lob = Some(t);
        }
        let mut flusher_out = false;
        for (i, &e) in s.iter().enumerate() {
            let b = app.world.get::<NpcBrain>(e).unwrap();
            let pos = app.world.get::<Transform>(e).unwrap().translation;
            match b.push_order {
                Some(Order::Flush { .. }) => {
                    flusher_out = true;
                    f.sent[i].get_or_insert(t);
                }
                Some(Order::Wait { .. }) => {
                    f.waited[i].get_or_insert(t);
                }
                Some(Order::FallBack { .. }) => {
                    f.fell_back[i].get_or_insert(t);
                }
                _ => {}
            }
            // (From where the player hid.)
            let near = DVec2::new(pos.x, pos.z).length();
            if f.sent[i].is_some() && near < 2.5 && f.reached[i].is_none() {
                f.reached[i] = Some(t);
                f.round_side[i] = Some(pos.x.signum());
            }
            if player_does == Player::Breaks
                && run_dir.is_none()
                && f.sent[i].is_some()
                && near < 3.0
            {
                run_dir = Some(-pos.x.signum());
            }
            if player_does == Player::Fights && !hurt_one && f.sent[i].is_some() && near < 2.5 {
                hurt_one = true;
                app.world.write_message(DamageEvent {
                    target: e,
                    amount: 700.0,
                    kind: DamageKind::Slash,
                    point: pos + DVec3::Y,
                    impulse: Vec3::ZERO,
                    source: Some(player),
                });
            }
        }
        for (i, &e) in a.iter().enumerate() {
            let b = app.world.get::<NpcBrain>(e).unwrap();
            if flusher_out && let Some(sg) = b.siege {
                f.watch_x[i] = Some(sg.watch.x);
            }
        }
        if std::env::var_os("PUSH_TRACE").is_some() && frame % 6 == 0 {
            let line = |e: Entity| {
                let b = app.world.get::<NpcBrain>(e).unwrap();
                let p = app.world.get::<Transform>(e).unwrap().translation;
                format!(
                    "{:?}/{}/{} {:.1?} {:?}",
                    b.action,
                    b.role,
                    b.destination_reason,
                    p,
                    b.push_order
                        .map(|o| format!("{o:?}").chars().take(14).collect::<String>())
                )
            };
            let all: Vec<String> = a.iter().chain(&s).map(|&e| line(e)).collect();
            if std::env::var_os("PUSH_PHASE").is_some() {
                let c = app.world.get::<NpcCombat>(a[0]).unwrap();
                let b = app.world.get::<NpcBrain>(a[0]).unwrap();
                eprintln!(
                    "      archer {:?} drawn {:?} watch {:.1?}",
                    c.phase,
                    b.siege.map(|s| s.drawn_since),
                    b.siege.map(|s| s.watch)
                );
            }
            eprintln!(
                "{t:5.1} player {:.1?} {run_dir:?} | {}",
                p_at,
                all.join(" | ")
            );
        }
    }
    f
}

const SEEDS: u64 = 6;
/// The second archer's reactions (s): slower than a show over the wall lasts, less its draw.
const SLOW: f32 = 0.55;

fn each_seed<T: Send>(run: impl Fn(u64) -> T + Sync) -> Vec<T> {
    let run = &run;
    std::thread::scope(|s| {
        let runs: Vec<_> = (0..SEEDS).map(|seed| s.spawn(move || run(seed))).collect();
        runs.into_iter()
            .map(|r| r.join().unwrap_or_else(|e| std::panic::resume_unwind(e)))
            .collect()
    })
}

const ARCHER: DVec3 = DVec3::new(0.0, 0.0, 18.0);
/// Swordsmen start behind the archer, to its left and right (too far to reach the player
/// before it goes to ground).
const SWORD_L: DVec3 = DVec3::new(-3.0, 0.0, 24.0);
const SWORD_R: DVec3 = DVec3::new(3.0, 0.0, 24.5);

fn tier_from_env() -> DifficultyTier {
    match std::env::var("TIER").as_deref() {
        Ok("easy") => DifficultyTier::Easy,
        Ok("medium") => DifficultyTier::Medium,
        Ok("hard") => DifficultyTier::Hard,
        Ok("extreme") => DifficultyTier::Extreme,
        _ => DifficultyTier::VeryHard,
    }
}

#[test]
#[ignore]
fn trace() {
    let player = match std::env::var("PLAYER").as_deref() {
        Ok("shows") => Player::Shows,
        Ok("breaks") => Player::Breaks,
        Ok("fights") => Player::Fights,
        _ => Player::Down,
    };
    let seed = std::env::var("SEED")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(0);
    let archers = if std::env::var_os("TWO").is_some() {
        vec![ARCHER, DVec3::new(4.0, 0.0, 17.0)]
    } else {
        vec![ARCHER]
    };
    let f = fight(
        tier_from_env(),
        &archers,
        &[SWORD_L, SWORD_R],
        player,
        seed,
        24.0,
    );
    eprintln!("{f:#?}");
}

#[test]
fn on_very_hard_the_archer_sends_a_swordsman_round_and_holds_on_the_other_edge() {
    let runs = each_seed(|seed| {
        fight(
            DifficultyTier::VeryHard,
            &[ARCHER],
            &[SWORD_L, SWORD_R],
            Player::Breaks,
            seed,
            20.0,
        )
    });
    for (seed, f) in runs.iter().enumerate() {
        let sent: Vec<usize> = (0..2).filter(|&i| f.sent[i].is_some()).collect();
        assert_eq!(
            sent.len(),
            1,
            "seed {seed}: one swordsman is sent round: {f:#?}"
        );
        let i = sent[0];
        assert!(
            f.sent[i].unwrap() < DOWN_AT + 3.0,
            "seed {seed}: sent soon: {f:#?}"
        );
        let heard = |l: BarkLine| f.barks.iter().any(|b| b.0 == l);
        assert!(
            heard(BarkLine::Flush) && heard(BarkLine::GoingRound),
            "seed {seed}: {f:#?}"
        );
        let side = f.round_side[i].expect("it comes round the wall to the player");
        assert!(f.reached[i].unwrap() < 14.0, "seed {seed}: {f:#?}");
        let watch = f.watch_x[0].expect("the archer holds while it goes");
        assert!(
            watch * side < 0.0 && watch.abs() > WALL_HALF - 0.5,
            "seed {seed}: the archer holds on the edge the swordsman doesn't come round: {f:#?}"
        );
        // Its draw held on that edge, it looses as the player clears the wall.
        let after = f
            .shot_after_break
            .expect("it shoots at the player breaking out");
        assert!(
            after < 0.6,
            "seed {seed}: shot {after:.2} s after clearing the wall: {f:#?}"
        );
    }
    let hit = runs
        .iter()
        .filter(|f| f.hit_after_break.is_some_and(|t| t < 1.5))
        .count();
    assert!(
        hit * 3 >= runs.len() * 2,
        "most breaks are hit within 1.5 s: {runs:#?}"
    );
}

#[test]
fn on_very_hard_a_slow_archer_looses_with_a_quick_one_when_the_player_shows() {
    let shows = |tier| {
        let runs = each_seed(|seed| {
            fight(
                tier,
                &[ARCHER, DVec3::new(4.0, 0.0, 17.0)],
                &[],
                Player::Shows,
                seed,
                24.0,
            )
        });
        // Shows hit by both archers, within 0.3 s of each other; and all shows.
        let together = runs
            .iter()
            .flat_map(|f| &f.show_hits)
            .filter(|h| h.len() >= 2 && h[1] - h[0] < 0.3)
            .count();
        let all: usize = runs.iter().map(|f| f.show_hits.len()).sum();
        (together, all)
    };
    let (alone, n_hard) = shows(DifficultyTier::Hard);
    let (volley, n) = shows(DifficultyTier::VeryHard);
    eprintln!("both hit: hard {alone} of {n_hard} shows, very hard {volley} of {n}");
    assert!(
        alone * 4 <= n_hard,
        "on Hard the slow one is too late for most shows"
    );
    assert!(
        volley * 4 >= n * 3,
        "on Very hard it looses with the quick one"
    );
}

#[test]
fn on_extreme_two_swordsmen_wait_for_the_lobs_then_go_round_both_edges() {
    let runs = each_seed(|seed| {
        fight(
            DifficultyTier::Extreme,
            &[ARCHER],
            &[SWORD_L, SWORD_R],
            Player::Down,
            seed,
            20.0,
        )
    });
    for (seed, f) in runs.iter().enumerate() {
        for i in 0..2 {
            let waited = f.waited[i].expect("each waits for the go");
            let sent = f.sent[i].expect("each is sent");
            assert!(
                sent - waited >= READY_SECS as f32 - 0.6,
                "seed {seed}: {f:#?}"
            );
            assert!(
                f.reached[i].is_some(),
                "seed {seed}: each reaches the player: {f:#?}"
            );
        }
        assert!(
            f.round_side[0].unwrap() * f.round_side[1].unwrap() < 0.0,
            "seed {seed}: one round each edge: {f:#?}"
        );
        let first_lob = f.first_lob.expect("the archer lobs while they wait");
        assert!(
            first_lob < f.sent[0].unwrap().max(f.sent[1].unwrap()) + 1.0,
            "seed {seed}: the lobs soften the player up first: {f:#?}"
        );
    }
}

#[test]
fn on_extreme_a_push_whose_flusher_is_badly_hurt_falls_back_and_tries_again() {
    let runs = each_seed(|seed| {
        fight(
            DifficultyTier::Extreme,
            &[ARCHER],
            &[SWORD_L, SWORD_R],
            Player::Fights,
            seed,
            24.0,
        )
    });
    for (seed, f) in runs.iter().enumerate() {
        let fell = f
            .fell_back
            .iter()
            .flatten()
            .copied()
            .fold(f32::MAX, f32::min);
        assert!(fell < 24.0, "seed {seed}: it falls back: {f:#?}");
        assert!(
            f.barks.iter().any(|b| b.0 == BarkLine::FallBack),
            "seed {seed}: {f:#?}"
        );
        // And waits for the go again.
        let again = f
            .barks
            .iter()
            .filter(|b| b.0 == BarkLine::Flush && b.1 > fell)
            .count();
        assert!(again >= 1, "seed {seed}: it tries again: {f:#?}");
    }
}

#[test]
fn on_hard_nobody_is_sent_round_and_nobody_barks_a_plan() {
    let runs = each_seed(|seed| {
        fight(
            DifficultyTier::Hard,
            &[ARCHER],
            &[SWORD_L, SWORD_R],
            Player::Down,
            seed,
            12.0,
        )
    });
    for (seed, f) in runs.iter().enumerate() {
        assert!(f.sent.iter().all(Option::is_none), "seed {seed}: {f:#?}");
        assert!(
            f.barks.iter().all(|b| b.0 == BarkLine::Lob),
            "seed {seed}: only lobs are called: {f:#?}"
        );
    }
}
