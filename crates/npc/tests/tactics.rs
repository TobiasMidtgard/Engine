//! Smarter than other games' enemies: whole squads in a headless app (no GPU), each case over
//! several seeds (every body's randomness from its `Seed`), as in `revive.rs`.
//!
//! * Suppress and flank: against a player behind a wall, one shooter pins it with steady fire
//!   while two mates go round to its side (70° to 110° round, both the same way), mostly out
//!   of its sight, bounding from cover to cover in turns (one moves while the other holds);
//!   one of them gets a shot from the side.
//! * A rookie squad (and a smart squad that doesn't work together) rushes straight in instead.
//! * Flushing: a player that stays put behind its wall gets a grenade from the squad's
//!   suppressor, no sooner than 6 s after it went to ground; one that runs from it is seen in
//!   the open at once and isn't hurt. The smarter the squad the sooner it throws; rookies never
//!   do.
//! * Punishing mistakes: a fighter strikes into a 0.4 s opening (a recovery after a missed
//!   swing) as soon as its reactions let it see it; a master nearly always does, a rookie never.

use engine_core::ecs::message::Messages;
use engine_core::prelude::*;
use engine_core::time::TimeSource;
use engine_npc::*;
use engine_world::locomotion::LocomotionState;
use engine_world::physics::Collider;
use engine_world::{DamageEvent, DamageKind, ProjectileTrace, SimulationFocus, WorldPlugin};

fn app() -> App {
    let mut app = App::new();
    app.world
        .resource_mut::<Time>()
        .set_source(TimeSource::Manual {
            step_secs: 1.0 / 60.0,
        });
    app.add_plugins((WorldPlugin, NpcPlugin));
    // A few fighters: running each system in turn on the test's own thread is far quicker
    // than handing them about a thread pool (the order is fixed either way:
    // `tests/schedule_order.rs`).
    for (_, schedule) in app.world.resource_mut::<Schedules>().iter_mut() {
        schedule.set_executor(engine_core::ecs::schedule::SingleThreadedExecutor::new());
    }
    app
}

fn wall(app: &mut App, center: DVec3, half: Vec3) {
    app.world
        .spawn((Transform::from_translation(center), Collider::cuboid(half)));
}

fn at(app: &App, e: Entity) -> DVec3 {
    app.world.get::<Transform>(e).unwrap().translation
}

fn brain(app: &App, e: Entity) -> &NpcBrain {
    app.world.get::<NpcBrain>(e).unwrap()
}

fn shots(app: &App, e: Entity) -> u32 {
    app.world.get::<NpcCombat>(e).unwrap().stats.shots
}

/// Where the player crouches behind its wall, and its eye.
const PLAYER: DVec3 = DVec3::ZERO;
const EYE: DVec3 = DVec3::new(0.0, 1.6, 0.0);

/// The yard: the player's wall (2 m high) facing the squad 20 m off along +Z, and scattered
/// cover on both sides (crates and walls, some waist-high, some tall) to bound between.
fn yard(app: &mut App) {
    wall(app, DVec3::new(0.0, 1.0, 1.4), Vec3::new(2.4, 1.0, 0.25));
    for sx in [-1.0, 1.0] {
        wall(
            app,
            DVec3::new(4.5 * sx, 1.0, 14.0),
            Vec3::new(1.2, 1.0, 0.3),
        );
        wall(
            app,
            DVec3::new(8.0 * sx, 0.6, 11.0),
            Vec3::new(0.6, 0.6, 0.6),
        );
        wall(
            app,
            DVec3::new(9.5 * sx, 1.0, 6.5),
            Vec3::new(0.3, 1.0, 1.4),
        );
        wall(
            app,
            DVec3::new(11.5 * sx, 0.6, 2.5),
            Vec3::new(0.6, 0.6, 0.6),
        );
    }
}

/// A squad of riflemen, 20 m off and facing the player, kept alive for the test.
fn squad(app: &mut App, skills: &[NpcSkills], seed: u64) -> Vec<Entity> {
    let n = skills.len();
    skills
        .iter()
        .enumerate()
        .map(|(k, &s)| {
            let x = (k as f64 - (n as f64 - 1.0) * 0.5) * 3.0;
            let mut t = Transform::from_translation(DVec3::new(x, 0.0, 20.0));
            t.look_to(Vec3::NEG_Z, Vec3::Y);
            let e = app
                .world
                .spawn((
                    t,
                    Npc::default(),
                    Faction::ENEMY,
                    Squad(5),
                    s,
                    NpcWeapon::rifle(),
                    Seed(hash_u64(seed * 31 + k as u64 + 1)),
                ))
                .id();
            let mut v = app.world.get_mut::<NpcVitals>(e).unwrap();
            v.max_health = 1e6;
            v.health = 1e6;
            e
        })
        .collect()
}

/// The player: seen beside its wall for a moment, then down behind it, firing back over it now
/// and then (at whoever pins it, else at the squad).
struct Player {
    e: Entity,
    next_shot: f32,
    /// Up over the wall to shoot until then (s).
    up_until: f32,
    /// Whether it runs from a grenade coming in, and when it did.
    flees: bool,
    fled_at: Option<f32>,
}

/// Where a player flushed out of its cover runs to: out past the end of its wall.
const ESCAPE: DVec3 = DVec3::new(-6.5, 0.0, -2.5);

impl Player {
    fn spawn(app: &mut App) -> Self {
        let mut t = Transform::from_translation(DVec3::new(3.4, 0.0, 0.0));
        t.look_to(Vec3::Z, Vec3::Y);
        let e = app
            .world
            .spawn((
                t,
                Faction::PLAYER,
                LocomotionState::default(),
                SimulationFocus::default(),
            ))
            .id();
        Player {
            e,
            next_shot: 3.0,
            up_until: 0.0,
            flees: false,
            fled_at: None,
        }
    }

    fn step(&mut self, app: &mut App, t: f32, squad: &[Entity]) {
        if self.flees
            && self.fled_at.is_none()
            && app.world.resource::<Grenades>().incoming(PLAYER, 4.0)
        {
            self.fled_at = Some(t);
        }
        if self.fled_at.is_some() {
            // Running out of cover, away from the grenade.
            let p = at(app, self.e);
            let to = ESCAPE - p;
            let step = to.normalize_or_zero() * (5.0 / 60.0f64).min(to.length());
            app.world.get_mut::<Transform>(self.e).unwrap().translation = p + step;
            app.world
                .get_mut::<LocomotionState>(self.e)
                .unwrap()
                .velocity = (step * 60.0).as_vec3();
            return;
        }
        if t >= 1.5 {
            // Down behind the wall, up over it a moment to fire.
            let up = if t < self.up_until { 0.6 } else { 0.0 };
            app.world.get_mut::<Transform>(self.e).unwrap().translation = PLAYER + DVec3::Y * up;
        }
        if t >= self.next_shot {
            self.next_shot += 2.5;
            self.up_until = t + 0.4;
            let pinned_by = squad
                .iter()
                .copied()
                .find(|&e| brain(app, e).role == "suppress")
                .unwrap_or(squad[0]);
            let to = at(app, pinned_by) + DVec3::new(0.8, 1.3, 0.0);
            app.world.write_message(ProjectileTrace {
                from: DVec3::new(0.0, 2.2, 0.6),
                to,
                radius: 0.05,
                speed: 600.0,
                shooter: Some(self.e),
            });
        }
    }
}

/// Can the player (its eye over the wall) see `p`'s body?
fn seen_by_player(app: &App, p: DVec3) -> bool {
    let world = app.world.resource::<TacticalWorld>();
    world
        .raycast(EYE + DVec3::Y * 0.4, p + DVec3::Y * 1.2)
        .is_none()
}

/// Degrees round from the way the player faces (+Z, at the squad).
fn bearing(p: DVec3) -> f64 {
    let d = DVec2::new(p.x - PLAYER.x, p.z - PLAYER.z).normalize_or_zero();
    d.y.clamp(-1.0, 1.0).acos().to_degrees()
}

/// Degrees round from the way the player faces, + toward +X.
fn signed_bearing(p: DVec3) -> f64 {
    (p.x - PLAYER.x).atan2(p.z - PLAYER.z).to_degrees()
}

/// What one fight against the player in cover showed.
#[derive(Debug, Default, Clone)]
struct Fight {
    /// Every role anyone had.
    roles: Vec<&'static str>,
    /// When a flanker first had a clear shot at the player from its side (s), if it did.
    flanked_at: Option<f32>,
    /// While flankers moved before that: the share of the time the player could see them,
    /// and the share of the time both ran at once.
    seen: f32,
    together: f32,
    /// Bounds: how many covers the flankers stopped at on the way.
    holds: u32,
    /// The suppressor: its longest wait between shots until then (s), and how many of its
    /// shots went at the player's cover (passing within 3 m of the player).
    longest_gap: f32,
    pinning_shots: u32,
    /// How far round the suppressor went until then (degrees).
    suppressor_round: f64,
    /// Where everyone ended, and the widest bearing anyone reached on the way in (until it
    /// first came within 6 m of the player; degrees).
    ends: Vec<DVec3>,
    widest: f64,
    /// Where each flanker got into position, as degrees round from the way the player's
    /// cover faces (+ toward +X), in the order they got there.
    in_position: Vec<(Entity, f64)>,
    /// Grenades and smoke thrown at the player: when (s), a frag or not, and how far from
    /// the player it landed (m).
    thrown: Vec<(f32, bool, f64)>,
    /// Blast damage the player took, when it ran out of cover (s), and when the squad first
    /// saw it out there (s).
    hurt: f32,
    fled_at: Option<f32>,
    seen_out_at: Option<f32>,
}

fn fight(skills: &[NpcSkills], seed: u64, secs: f32) -> Fight {
    fight_with(skills, seed, secs, false)
}

/// A fight in which the player, if `flees`, runs from a grenade coming in.
fn fight_with(skills: &[NpcSkills], seed: u64, secs: f32, flees: bool) -> Fight {
    let mut app = app();
    yard(&mut app);
    let mut player = Player::spawn(&mut app);
    player.flees = flees;
    let mut hurts = app.world.resource::<Messages<DamageEvent>>().get_cursor();
    let mut throws = 0;
    let squad = squad(&mut app, skills, seed);
    let mut f = Fight::default();
    let mut cursor = app
        .world
        .resource::<Messages<ProjectileTrace>>()
        .get_cursor();
    let mut suppressor: Option<(Entity, f32, u32)> = None;
    let (mut moving, mut moving_seen, mut both) = (0u32, 0u32, 0u32);
    let mut was_holding = vec![false; squad.len()];
    let mut arrived = vec![false; squad.len()];
    let mut last = squad.iter().map(|&e| at(&app, e)).collect::<Vec<_>>();
    let frames = (secs * 60.0) as usize;
    for frame in 0..frames {
        let t = frame as f32 / 60.0;
        player.step(&mut app, t, &squad);
        app.update();
        let now = app.world.resource::<Time>().elapsed_secs_f64();
        let g = app.world.resource::<Grenades>();
        if g.recent.len() > throws {
            throws = g.recent.len();
            let frag = g
                .flights
                .iter()
                .rev()
                .find(|fl| (fl.thrown - now).abs() < 1e-6);
            let landed = frag.map(|fl| fl.at).or_else(|| {
                let clouds = &app
                    .world
                    .resource::<engine_npc::smoke::SmokeClouds>()
                    .clouds;
                clouds.last().map(|c| c.at)
            });
            let off = landed.map_or(f64::MAX, |p| {
                DVec2::new(p.x - PLAYER.x, p.z - PLAYER.z).length()
            });
            f.thrown.push((t, frag.is_some(), off));
        }
        let msgs = app.world.resource::<Messages<DamageEvent>>();
        f.hurt += hurts
            .read(msgs)
            .filter(|h| h.target == player.e && h.kind == DamageKind::Explosive)
            .map(|h| h.amount)
            .sum::<f32>();
        f.fled_at = player.fled_at;
        if let Some(fled) = player.fled_at
            && f.seen_out_at.is_none()
            && squad.iter().any(|&e| {
                app.world
                    .get::<NpcMemory>(e)
                    .unwrap()
                    .get(player.e)
                    .is_some_and(|k| k.visible)
            })
            && t > fled
        {
            f.seen_out_at = Some(t);
        }
        let msgs = app.world.resource::<Messages<ProjectileTrace>>();
        let traces: Vec<ProjectileTrace> = cursor.read(msgs).copied().collect();
        let mut running = 0;
        for (k, &e) in squad.iter().enumerate() {
            let b = brain(&app, e);
            if !f.roles.contains(&b.role) {
                f.roles.push(b.role);
            }
            let p = at(&app, e);
            if p.distance(PLAYER) <= 6.0 {
                arrived[k] = true;
            } else if !arrived[k] {
                f.widest = f.widest.max(bearing(p));
            }
            let speed = DVec2::new(p.x - last[k].x, p.z - last[k].z).length() * 60.0;
            last[k] = p;
            if b.role == "suppress" && suppressor.is_none() {
                suppressor = Some((e, t, shots(&app, e)));
            }
            if b.role == "flank" && b.in_position && !f.in_position.iter().any(|&(o, _)| o == e) {
                f.in_position.push((e, signed_bearing(p)));
            }
            if b.role != "flank" || f.flanked_at.is_some() {
                continue;
            }
            if b.holding && !was_holding[k] {
                f.holds += 1;
            }
            was_holding[k] = b.holding;
            if speed > 1.5 && !b.in_position {
                running += 1;
                if frame.is_multiple_of(6) {
                    moving += 1;
                    moving_seen += u32::from(seen_by_player(&app, p));
                }
            }
            if bearing(p) >= 65.0
                && p.distance(PLAYER) < 16.0
                && seen_by_player(&app, p)
                && b.in_position
            {
                f.flanked_at = Some(t);
            }
        }
        if running >= 2 && frame.is_multiple_of(6) {
            both += 1;
        }
        if let Some((s, since, n)) = &mut suppressor
            && f.flanked_at.is_none()
        {
            let now_shots = shots(&app, *s);
            if now_shots > *n {
                f.longest_gap = f.longest_gap.max(t - *since);
                *since = t;
                *n = now_shots;
            } else {
                f.longest_gap = f.longest_gap.max(t - *since);
            }
            f.suppressor_round = f.suppressor_round.max(bearing(at(&app, *s)));
            f.pinning_shots += traces
                .iter()
                .filter(|tr| {
                    tr.shooter == Some(*s)
                        && engine_npc::perception::segment_distance(
                            PLAYER + DVec3::Y,
                            tr.from,
                            tr.to,
                        ) < 3.0
                })
                .count() as u32;
        }
    }
    f.seen = moving_seen as f32 / moving.max(1) as f32;
    f.together = both as f32 / moving.max(1) as f32;
    f.ends = squad.iter().map(|&e| at(&app, e)).collect();
    f
}

/// The runs of each case, each with its own seeds (the same ones every test run).
const SEEDS: u64 = 8;

fn each_seed<T: Send>(run: impl Fn(u64) -> T + Sync) -> Vec<T> {
    let run = &run;
    std::thread::scope(|s| {
        let runs: Vec<_> = (0..SEEDS).map(|seed| s.spawn(move || run(seed))).collect();
        runs.into_iter()
            .map(|r| r.join().unwrap_or_else(|e| std::panic::resume_unwind(e)))
            .collect()
    })
}

#[test]
fn a_suppressor_pins_the_player_while_two_mates_bound_round_to_its_flank() {
    let skills = [
        NpcSkills::veteran(),
        NpcSkills::elite(),
        NpcSkills::veteran(),
    ];
    let runs = each_seed(|seed| fight(&skills, seed, 35.0));
    for (seed, f) in runs.iter().enumerate() {
        eprintln!(
            "seed {seed}: flanked at {:?} s, seen {:.2}, together {:.2}, holds {}, gap {:.1} s, \
             pinning shots {}, suppressor round {:.0}°, roles {:?}",
            f.flanked_at,
            f.seen,
            f.together,
            f.holds,
            f.longest_gap,
            f.pinning_shots,
            f.suppressor_round,
            f.roles
        );
        let round: Vec<String> = f
            .in_position
            .iter()
            .map(|x| format!("{:.0}", x.1))
            .collect();
        eprintln!("seed {seed}: in position at {round:?}°");
    }
    for (seed, f) in runs.iter().enumerate() {
        assert!(
            f.roles.contains(&"suppress") && f.roles.contains(&"flank"),
            "seed {seed}: {:?}",
            f.roles
        );
        let at = f
            .flanked_at
            .unwrap_or_else(|| panic!("seed {seed}: never got round"));
        assert!(at < 12.0, "seed {seed}: round only after {at} s");
        // Off the player's side, not round behind it, and both on the same side (the yard is
        // the same on both).
        for &(_, round) in &f.in_position {
            assert!(
                (70.0..110.0).contains(&round.abs()),
                "seed {seed}: in position {round:.0}° round"
            );
        }
        assert_eq!(f.in_position.len(), 2, "seed {seed}: {:?}", f.in_position);
        assert!(
            f.in_position[0].1.signum() == f.in_position[1].1.signum(),
            "seed {seed}: split {:?}",
            f.in_position
        );
        assert!(
            f.longest_gap < 3.0,
            "seed {seed}: the suppressor let up for {} s",
            f.longest_gap
        );
        assert!(
            f.pinning_shots >= 4,
            "seed {seed}: {} shots at the cover",
            f.pinning_shots
        );
        assert!(
            f.holds >= 3,
            "seed {seed}: {} covers held on the way",
            f.holds
        );
        assert!(
            f.suppressor_round < 40.0,
            "seed {seed}: the suppressor went {:.0}° round itself",
            f.suppressor_round
        );
    }
    let n = runs.len() as f32;
    let seen = runs.iter().map(|f| f.seen).sum::<f32>() / n;
    let together = runs.iter().map(|f| f.together).sum::<f32>() / n;
    assert!(
        seen < 0.15,
        "the player saw the flankers {seen:.2} of the way"
    );
    assert!(
        together < 0.1,
        "both flankers ran at once {together:.2} of the time"
    );
}

#[test]
fn a_rookie_squad_rushes_straight_in_and_a_squad_without_teamwork_does_not_flank() {
    let loners = NpcSkills {
        teamwork: 0.1,
        ..NpcSkills::veteran()
    };
    for (name, skills) in [
        ("rookies", [NpcSkills::rookie(); 3]),
        ("loners", [loners; 3]),
    ] {
        let runs = each_seed(|seed| fight(&skills, seed, 20.0));
        for (seed, f) in runs.iter().enumerate() {
            eprintln!(
                "{name} seed {seed}: roles {:?}, widest {:.0}°, ends {:?}",
                f.roles, f.widest, f.ends
            );
            assert!(
                !f.roles.contains(&"flank") && !f.roles.contains(&"suppress"),
                "{name} seed {seed}: {:?}",
                f.roles
            );
        }
        if name == "rookies" {
            for (seed, f) in runs.iter().enumerate() {
                assert!(
                    f.widest < 40.0,
                    "seed {seed}: went round to {:.0}°",
                    f.widest
                );
                let close = f.ends.iter().filter(|p| p.distance(PLAYER) < 8.0).count();
                assert!(close >= 2, "seed {seed}: ended at {:?}", f.ends);
            }
        }
    }
}

/// A fighter with `skills` and a sword before a player that now and then leaves itself open
/// for 0.4 s (recovering from a missed swing) while the fighter is ready to strike. Returns how
/// many of the openings it struck into while they lasted, of how many.
fn punished(skills: NpcSkills, seed: u64) -> (u32, u32) {
    let mut app = app();
    let mut t = Transform::from_translation(DVec3::ZERO);
    t.look_to(Vec3::Z, Vec3::Y);
    let player = app
        .world
        .spawn((
            t,
            Faction::PLAYER,
            LocomotionState::default(),
            SimulationFocus::default(),
        ))
        .id();
    let mut t = Transform::from_translation(DVec3::new(0.0, 0.0, 3.0));
    t.look_to(Vec3::NEG_Z, Vec3::Y);
    let npc = app
        .world
        .spawn((
            t,
            Npc::default(),
            Faction::ENEMY,
            skills,
            NpcWeapon::sword(),
            Seed(hash_u64(seed + 77)),
        ))
        .id();
    app.update_n(120);
    let (mut opened, mut struck) = (0, 0);
    let mut wait = 60;
    let mut rng = hash_u64(seed ^ 0xfeed);
    while opened < 12 && app.world.get::<NpcClock>(npc).unwrap().now < 90.0 {
        app.update();
        if wait > 0 {
            wait -= 1;
            continue;
        }
        let now = app.world.get::<NpcClock>(npc).unwrap().now;
        let c = app.world.get::<NpcCombat>(npc).unwrap();
        let ready = c.phase == AttackPhase::Ready && now >= c.ready_at;
        let d = at(&app, npc).distance(at(&app, player));
        if !ready || !(2.0..4.5).contains(&d) {
            continue;
        }
        // The opening: 0.4 s.
        let before = c.stats.punishes;
        app.world
            .get_mut::<LocomotionState>(player)
            .unwrap()
            .vulnerable = true;
        let mut hit = false;
        for _ in 0..24 {
            app.update();
            hit |= app.world.get::<NpcCombat>(npc).unwrap().stats.punishes > before;
        }
        app.world
            .get_mut::<LocomotionState>(player)
            .unwrap()
            .vulnerable = false;
        opened += 1;
        struck += u32::from(hit);
        rng = hash_u64(rng);
        wait = 70 + (rng % 60) as usize;
    }
    (struck, opened)
}

#[test]
fn the_more_skilled_punish_more_openings_a_master_nearly_all_a_rookie_none() {
    let mut rates = Vec::new();
    for name in ["rookie", "soldier", "veteran", "master"] {
        let skills = NpcSkills::preset(name).unwrap();
        let runs = each_seed(|seed| punished(skills, seed));
        let (struck, opened) = runs.iter().fold((0, 0), |(a, b), &(s, o)| (a + s, b + o));
        assert!(opened >= SEEDS as u32 * 8, "{name}: only {opened} openings");
        let rate = struck as f32 / opened as f32;
        eprintln!("{name}: punished {struck}/{opened} ({rate:.2})");
        rates.push(rate);
    }
    assert!(rates[0] < 0.05, "a rookie punished {:.2}", rates[0]);
    assert!(rates[3] > 0.75, "a master punished {:.2}", rates[3]);
    assert!(
        rates.windows(2).all(|w| w[0] <= w[1]) && rates[2] > rates[1],
        "punishing should rise with skill: {rates:?}"
    );
}

/// The first grenade or smoke thrown in a fight, if any.
fn first_throw(f: &Fight) -> Option<(f32, bool, f64)> {
    f.thrown.first().copied()
}

#[test]
fn a_player_that_stays_put_behind_cover_gets_a_grenade_and_one_that_runs_is_seen_in_the_open() {
    let skills = [
        NpcSkills::veteran(),
        NpcSkills::elite(),
        NpcSkills::veteran(),
    ];
    for flees in [false, true] {
        let runs = each_seed(|seed| fight_with(&skills, seed, 30.0, flees));
        for (seed, f) in runs.iter().enumerate() {
            eprintln!(
                "flees {flees} seed {seed}: thrown {:?}, hurt {:.0}, fled at {:?}, seen out at {:?}",
                f.thrown, f.hurt, f.fled_at, f.seen_out_at
            );
        }
        for (seed, f) in runs.iter().enumerate() {
            let (when, _, off) =
                first_throw(f).unwrap_or_else(|| panic!("seed {seed}: nothing thrown"));
            // It went down behind its wall at 1.5 s: at least 6 s still before anything comes.
            assert!(when >= 7.5, "seed {seed}: thrown at {when:.1} s");
            assert!(when < 14.0, "seed {seed}: thrown only at {when:.1} s");
            assert!(off < 1.5, "seed {seed}: landed {off:.1} m off");
            if flees {
                let fled = f.fled_at.expect("it never ran");
                let seen = f.seen_out_at.expect("nobody saw it out of cover");
                assert!(
                    seen - fled < 0.5,
                    "seed {seed}: seen {:.2} s after it ran",
                    seen - fled
                );
                assert!(
                    f.hurt == 0.0,
                    "seed {seed}: hurt {:.0} though it ran",
                    f.hurt
                );
            } else {
                assert!(
                    f.hurt > 50.0,
                    "seed {seed}: the blast did only {:.0}",
                    f.hurt
                );
            }
        }
    }
}

#[test]
fn logic_decides_when_a_squad_throws_and_a_rookie_squad_never_does() {
    let mut means = Vec::new();
    for name in ["rookie", "soldier", "elite"] {
        let skills = [NpcSkills::preset(name).unwrap(); 3];
        let runs = each_seed(|seed| fight(&skills, seed, 30.0));
        let times: Vec<f32> = runs.iter().filter_map(first_throw).map(|t| t.0).collect();
        eprintln!("{name}: first throws at {times:?}");
        if name == "rookie" {
            assert!(times.is_empty(), "rookies threw at {times:?}");
            continue;
        }
        assert_eq!(
            times.len(),
            runs.len(),
            "{name}: thrown in only {} fights",
            times.len()
        );
        means.push(times.iter().sum::<f32>() / times.len() as f32);
    }
    assert!(
        means[1] + 2.5 < means[0],
        "elites threw at {:.1} s on average, soldiers at {:.1} s",
        means[1],
        means[0]
    );
}
