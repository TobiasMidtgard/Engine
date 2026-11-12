//! Difficulty that changes how an archer fights (`besiege`, `difficulty`): one archer against
//! a player that went to ground behind a waist-high wall and comes up over it now and then to
//! shoot, played at each tier over several seeds, in a headless app (no GPU).
//!
//! * Easy: it walks round the wall to a clear shot at where the player crouches.
//! * Medium: it stays off in the open and shoots at where the player shows, now and then;
//!   when the player comes up it starts its draw from scratch, too late for most shows.
//! * Hard: it takes cover with a line to where the player shows, draws and holds the draw on
//!   it, and looses as soon as it sees the player come up; it hits far more of the shows.
//! * A skilled archer on Hard lobs arrows over the wall onto the player when it stays down; a
//!   rookie on Hard takes cover and holds but never lobs, and nobody lobs on Medium.

use engine_core::ecs::message::Messages;
use engine_core::prelude::*;
use engine_core::time::TimeSource;
use engine_npc::besiege::Siege;
use engine_npc::difficulty::Tactic;
use engine_npc::lob::Lobs;
use engine_npc::perception::BodyHeight;
use engine_npc::*;
use engine_world::locomotion::{LocomotionMode, LocomotionState};
use engine_world::physics::Collider;
use engine_world::{
    CombatDifficulty, DamageEvent, DifficultyTier, ProjectileTrace, SimulationFocus, WorldPlugin,
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

/// Where the player crouches behind its wall.
const PLAYER: DVec3 = DVec3::ZERO;
/// The player's wall: 1.4 m high, 4.8 m wide, facing the archer 18 m off along +Z.
const WALL_TOP: f64 = 1.4;
/// Where the archer starts.
const ARCHER: DVec3 = DVec3::new(0.0, 0.0, 18.0);
/// Standing, the player's head clears the wall; crouched (`perception::CROUCH_HEIGHT` of
/// that), it is 0.4 m under its top.
const STANDING: f32 = 1.75;
/// Seen standing at first, then down from this time (s), up over the wall for
/// [`UP_SECS`] every [`UP_EVERY`] from [`FIRST_UP`].
const DOWN_AT: f32 = 2.0;
const FIRST_UP: f32 = 6.0;
const UP_EVERY: f32 = 4.0;
const UP_SECS: f32 = 0.5;

/// The yard: the player's wall, and cover on the archer's side (crates and walls, some
/// waist-high, some tall) as in `tactics.rs`.
fn yard(app: &mut App) {
    wall(
        app,
        DVec3::new(0.0, WALL_TOP * 0.5, 1.4),
        Vec3::new(2.4, WALL_TOP as f32 * 0.5, 0.25),
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
        wall(
            app,
            DVec3::new(9.5 * sx, 1.0, 4.5),
            Vec3::new(0.3, 1.0, 1.4),
        );
    }
}

fn archer(app: &mut App, skills: NpcSkills, seed: u64) -> Entity {
    let mut t = Transform::from_translation(ARCHER);
    t.look_to(Vec3::NEG_Z, Vec3::Y);
    let e = app
        .world
        .spawn((
            t,
            Npc::default(),
            Faction::ENEMY,
            skills,
            NpcWeapon::bow(),
            Seed(hash_u64(seed * 31 + 7)),
        ))
        .id();
    let mut v = app.world.get_mut::<NpcVitals>(e).unwrap();
    v.max_health = 1e6;
    v.health = 1e6;
    e
}

/// Up over the wall at `t`, for a player that comes up to shoot (`shows`) or stays down.
fn is_up(t: f32, shows: bool) -> bool {
    t < DOWN_AT || (shows && t >= FIRST_UP && (t - FIRST_UP) % UP_EVERY < UP_SECS)
}

/// What one fight showed.
#[derive(Debug, Default, Clone)]
struct Fight {
    /// The tactics it besieged the player with.
    tactics: Vec<Tactic>,
    /// The player's shows over the wall, and how many of them an arrow hit (shot straight).
    shows: u32,
    shows_hit: u32,
    /// Seconds from the player coming up to the arrow that hit it, for each show hit.
    hit_after: Vec<f32>,
    /// Arrows shot straight while the player was down, and how many of them passed within
    /// 1.5 m of where the player shows (over the wall's top).
    shots_down: u32,
    at_the_show: u32,
    /// Lobs: how many came down, how many within 1.5 m of the player, and how many hit it.
    lobs: u32,
    lobs_near: u32,
    lobs_hit: u32,
    /// When the first lob was loosed (s).
    first_lob: Option<f32>,
    /// Hits on the player while it was down (from round the wall).
    hit_down: u32,
    /// The longest it held a draw (s).
    longest_hold: f32,
    /// Where it was and whether it was in cover while the player was down (sampled), and the
    /// nearest it came to the player.
    in_cover: f32,
    nearest: f64,
    end: DVec3,
}

fn fight(tier: DifficultyTier, skills: NpcSkills, seed: u64, secs: f32, shows: bool) -> Fight {
    let mut app = app();
    app.world.resource_mut::<CombatDifficulty>().tier = tier;
    yard(&mut app);
    let mut pt = Transform::from_translation(PLAYER);
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
    let a = archer(&mut app, skills, seed);
    let mut hurts = app.world.resource::<Messages<DamageEvent>>().get_cursor();
    let mut traces = app
        .world
        .resource::<Messages<ProjectileTrace>>()
        .get_cursor();
    let mut f = Fight {
        nearest: f64::MAX,
        ..Default::default()
    };
    let mut landed = 0;
    let (mut samples, mut covered) = (0u32, 0u32);
    let mut show: Option<(f32, bool)> = None;
    let mut drawn_from: Option<f32> = None;
    let frames = (secs * 60.0) as usize;
    for frame in 0..frames {
        let t = frame as f32 / 60.0;
        let up = is_up(t, shows);
        app.world.get_mut::<LocomotionState>(player).unwrap().mode = if up {
            LocomotionMode::Idle
        } else {
            LocomotionMode::Crouch
        };
        if up && t >= FIRST_UP && show.is_none() {
            show = Some((t, false));
            f.shows += 1;
        } else if !up {
            show = None;
        }
        app.update();
        let msgs = app.world.resource::<Messages<DamageEvent>>();
        for h in hurts.read(msgs) {
            if h.target != player || h.source != Some(a) {
                continue;
            }
            if h.kind == engine_world::DamageKind::Pierce {
                continue; // a lob, counted below
            }
            match &mut show {
                Some((from, hit)) if !*hit => {
                    *hit = true;
                    f.shows_hit += 1;
                    f.hit_after.push(t - *from);
                }
                Some(_) => {}
                None => f.hit_down += 1,
            }
        }
        let msgs = app.world.resource::<Messages<ProjectileTrace>>();
        let straight: Vec<ProjectileTrace> = traces
            .read(msgs)
            .filter(|tr| tr.shooter == Some(a) && tr.speed > 59.0 && tr.from.distance(tr.to) > 3.0)
            .copied()
            .collect();
        let peek = PLAYER + DVec3::Y * 1.55;
        for tr in straight {
            if !up {
                f.shots_down += 1;
                if engine_npc::perception::segment_distance(peek, tr.from, tr.to) < 1.5 {
                    f.at_the_show += 1;
                }
            }
        }
        let lobs = app.world.resource::<Lobs>();
        if f.first_lob.is_none() && lobs.flights.iter().any(|l| l.shooter == a) {
            f.first_lob = Some(t);
        }
        for l in lobs.landed.iter().filter(|l| l.3 == a).skip(landed) {
            f.lobs += 1;
            if DVec2::new(l.1.x - PLAYER.x, l.1.z - PLAYER.z).length() < 1.5 {
                f.lobs_near += 1;
            }
            if l.2 == Some(player) {
                f.lobs_hit += 1;
            }
        }
        landed = lobs.landed.iter().filter(|l| l.3 == a).count();
        let b = app.world.get::<NpcBrain>(a).unwrap();
        if let Some(Siege { tactic, .. }) = b.siege
            && b.action == NpcAction::Besiege
            && !f.tactics.contains(&tactic)
        {
            f.tactics.push(tactic);
        }
        let in_cover = b.in_cover && b.action == NpcAction::Besiege;
        let held = b
            .siege
            .and_then(|s| s.drawn_since)
            .filter(|_| b.action == NpcAction::Besiege);
        match (held, drawn_from) {
            (Some(_), None) => drawn_from = Some(t),
            (Some(_), Some(from)) => f.longest_hold = f.longest_hold.max(t - from),
            (None, _) => drawn_from = None,
        }
        let p = app.world.get::<Transform>(a).unwrap().translation;
        f.nearest = f
            .nearest
            .min(DVec2::new(p.x - PLAYER.x, p.z - PLAYER.z).length());
        if !up && t > DOWN_AT + 4.0 && frame.is_multiple_of(6) {
            samples += 1;
            covered += u32::from(in_cover);
        }
        f.end = p;
        if std::env::var_os("BESIEGE_TRACE").is_some() && frame % 30 == 0 {
            eprintln!(
                "{t:5.1} up {up} {:?} {} pos {:.1?} dest {:.1?} watch {:.2?} cover {} vis {}",
                b.action,
                b.destination_reason,
                p,
                b.destination,
                b.siege.map(|s| s.watch),
                b.in_cover,
                app.world
                    .get::<NpcMemory>(a)
                    .unwrap()
                    .get(player)
                    .is_some_and(|k| k.visible)
            );
        }
    }
    f.in_cover = covered as f32 / samples.max(1) as f32;
    f
}

const SEEDS: u64 = 6;

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
#[ignore]
fn trace() {
    let tier = match std::env::var("TIER").as_deref() {
        Ok("easy") => DifficultyTier::Easy,
        Ok("hard") => DifficultyTier::Hard,
        _ => DifficultyTier::Medium,
    };
    let lvl: f32 = std::env::var("LEVEL")
        .ok()
        .and_then(|l| l.parse().ok())
        .unwrap_or(1.0);
    let f = fight(
        tier,
        NpcSkills::at_level(lvl),
        0,
        30.0,
        std::env::var_os("DOWN").is_none(),
    );
    eprintln!("{f:?}");
}

#[test]
#[ignore]
fn report() {
    use DifficultyTier::*;
    for (tier, skills, name) in [
        (Easy, NpcSkills::master(), "easy master"),
        (Medium, NpcSkills::master(), "medium master"),
        (Medium, NpcSkills::soldier(), "medium soldier"),
        (Hard, NpcSkills::master(), "hard master"),
        (Hard, NpcSkills::rookie(), "hard rookie"),
        (Hard, NpcSkills::soldier(), "hard soldier"),
    ] {
        for f in each_seed(|seed| fight(tier, skills, seed, 30.0, true))
            .into_iter()
            .chain(
                each_seed(|seed| fight(tier, skills, seed, 30.0, false))
                    .into_iter()
                    .take(2),
            )
        {
            println!(
                "{name}: {:?} shows {}/{} after {:?} down {} at {} lobs {}/{}/{} hitdown {} hold {:.1} cover {:.2} nearest {:.1} end {:.1?}",
                f.tactics,
                f.shows_hit,
                f.shows,
                f.hit_after
                    .iter()
                    .map(|x| (x * 100.0).round() / 100.0)
                    .collect::<Vec<_>>(),
                f.shots_down,
                f.at_the_show,
                f.lobs,
                f.lobs_near,
                f.lobs_hit,
                f.hit_down,
                f.longest_hold,
                f.in_cover,
                f.nearest,
                f.end
            );
        }
    }
}

/// The fights at `tier` for each seed: the player coming up to shoot (`shows`) or staying down.
fn fights(tier: DifficultyTier, skills: NpcSkills, shows: bool) -> Vec<Fight> {
    let secs = if shows { 26.0 } else { 20.0 };
    each_seed(|seed| fight(tier, skills, seed, secs, shows))
}

#[test]
fn an_easy_archer_walks_round_the_wall_and_a_medium_one_shoots_at_its_top_from_the_open() {
    let easy = fights(DifficultyTier::Easy, NpcSkills::master(), true);
    let medium = fights(DifficultyTier::Medium, NpcSkills::master(), true);
    for f in &easy {
        assert_eq!(f.tactics, [Tactic::CloseOnHidden], "{f:?}");
        // Round the wall's end to within a few metres, where it shoots the player crouching.
        assert!(f.nearest < 9.0 && f.end.x.abs() > 4.0, "{f:?}");
        assert!(f.hit_down >= 4, "{f:?}");
        assert!(f.longest_hold == 0.0 && f.lobs == 0, "{f:?}");
    }
    for f in &medium {
        assert_eq!(f.tactics, [Tactic::HarassEdge], "{f:?}");
        // Off where it was, in the open, shooting now and then at where the player shows.
        assert!(f.nearest > 12.0 && f.in_cover < 0.2, "{f:?}");
        assert!(f.shots_down >= 5, "{f:?}");
        assert!(f.at_the_show as f32 >= 0.8 * f.shots_down as f32, "{f:?}");
        // Its draw starts when the player shows: too late for most quick shows.
        assert!(f.shows_hit * 2 < f.shows, "{f:?}");
        assert!(f.longest_hold == 0.0 && f.lobs == 0, "{f:?}");
    }
}

#[test]
fn a_hard_archer_takes_cover_holds_its_draw_and_looses_as_soon_as_the_player_shows() {
    let medium = fights(DifficultyTier::Medium, NpcSkills::master(), true);
    let hard = fights(DifficultyTier::Hard, NpcSkills::master(), true);
    let soldier = fights(DifficultyTier::Hard, NpcSkills::soldier(), true);
    let master_react = NpcSkills::master().reaction_time;
    for f in &hard {
        assert_eq!(f.tactics, [Tactic::CoverAndHold], "{f:?}");
        assert!(f.in_cover > 0.9, "{f:?}");
        // Held on where the player shows: all but one show hit, each its reaction time on.
        assert!(f.shows_hit + 1 >= f.shows, "{f:?}");
        assert!(
            f.hit_after.iter().all(|&s| s <= master_react + 0.12),
            "{f:?}"
        );
        // It doesn't shoot at the wall while the player is down.
        assert!(f.shots_down <= 1, "{f:?}");
    }
    let hits = |fs: &[Fight]| fs.iter().map(|f| f.shows_hit).sum::<u32>();
    assert!(
        hits(&hard) >= 3 * hits(&medium).max(1),
        "{} vs {}",
        hits(&hard),
        hits(&medium)
    );
    // A soldier holds too, but sees the player later and shoots less true.
    let soldier_react = NpcSkills::soldier().reaction_time;
    for f in &soldier {
        assert_eq!(f.tactics, [Tactic::CoverAndHold], "{f:?}");
        assert!(f.hit_after.iter().all(|&s| s >= soldier_react), "{f:?}");
    }
    assert!(
        hits(&soldier) < hits(&hard),
        "{} vs {}",
        hits(&soldier),
        hits(&hard)
    );
}

#[test]
fn a_skilled_archer_on_hard_lobs_onto_a_player_that_stays_down_and_a_rookie_never_does() {
    let master = fights(DifficultyTier::Hard, NpcSkills::master(), false);
    let rookie = fights(DifficultyTier::Hard, NpcSkills::rookie(), false);
    let medium = fights(DifficultyTier::Medium, NpcSkills::master(), false);
    for f in &master {
        // Once it has held its draw a while with no show.
        let first = f.first_lob.expect("a lob");
        assert!(
            first >= DOWN_AT + engine_npc::besiege::LOB_AFTER as f32,
            "{f:?}"
        );
        assert!(f.lobs >= 2 && f.lobs_near == f.lobs, "{f:?}");
        assert!(f.lobs_hit >= 1, "{f:?}");
    }
    for f in &rookie {
        assert_eq!(f.tactics, [Tactic::CoverAndHold], "{f:?}");
        assert_eq!(f.lobs, 0, "{f:?}");
        // It holds its draw until its arms tire, then lowers.
        let most = engine_npc::besiege::HOLD_MAX as f32;
        assert!(
            f.longest_hold > most - 0.5 && f.longest_hold < most + 0.3,
            "{f:?}"
        );
    }
    for f in &medium {
        assert_eq!(f.lobs, 0, "{f:?}");
    }
}
