//! Animal instincts by difficulty (`instinct`): on Easy animals let you closer and run
//! straight; on Hard a rabbit runs into tall grass and keeps still there until you are almost
//! on it, deer run into the woods and stop there to watch, and an alert herd bunches up with
//! its young in the middle; on Extreme boars charge whatever comes near their young, and
//! wolves ring a boar at bay. Each against the tier below, in a headless app (no GPU).

use std::sync::Arc;

use engine_core::prelude::*;
use engine_core::time::TimeSource;
use engine_npc::wildlife::HERD_CLOSE;
use engine_npc::*;
use engine_world::locomotion::{LocomotionMode, LocomotionState};
use engine_world::physics::Collider;
use engine_world::{CombatDifficulty, DifficultyTier, Obscurance, ObscuranceQuery, WorldPlugin};

fn app(tier: DifficultyTier) -> App {
    let mut app = App::new();
    app.world.resource_mut::<EngineConfig>().headless = true;
    app.world
        .resource_mut::<Time>()
        .set_source(TimeSource::Manual {
            step_secs: 1.0 / 60.0,
        });
    app.add_plugins((WorldPlugin, NpcPlugin));
    for (_, schedule) in app.world.resource_mut::<Schedules>().iter_mut() {
        schedule.set_executor(engine_core::ecs::schedule::SingleThreadedExecutor::new());
    }
    app.world.resource_mut::<CombatDifficulty>().tier = tier;
    app
}

/// Tall grass: boxes on the ground (x0, z0, x1, z1) of `height`, that light falls through as
/// through 5/m of haze.
struct Grass {
    patches: Vec<[f64; 4]>,
    height: f64,
}

impl Grass {
    fn inside(&self, p: DVec3) -> bool {
        p.y >= -0.1
            && p.y < self.height
            && self
                .patches
                .iter()
                .any(|b| p.x > b[0] && p.x < b[2] && p.z > b[1] && p.z < b[3])
    }
}

impl ObscuranceQuery for Grass {
    fn transmittance(&self, from: DVec3, to: DVec3) -> f32 {
        let n = (from.distance(to) / 0.05).ceil().max(1.0) as usize;
        let ds = from.distance(to) / n as f64;
        let depth: f64 = (0..n)
            .map(|k| from.lerp(to, (k as f64 + 0.5) / n as f64))
            .map(|p| self.extinction(p) as f64 * ds)
            .sum();
        (-depth).exp() as f32
    }

    fn extinction(&self, p: DVec3) -> f32 {
        if self.inside(p) { 5.0 } else { 0.0 }
    }
}

fn grass(app: &mut App, patches: Vec<[f64; 4]>) -> Arc<Grass> {
    let g = Arc::new(Grass {
        patches,
        height: 0.6,
    });
    app.world
        .resource_mut::<Obscurance>()
        .set("grass", g.clone());
    g
}

fn animal(app: &mut App, a: Animal, at: DVec3, herd: u32) -> Entity {
    app.world
        .spawn((Transform::from_translation(at), a, Herd(herd)))
        .id()
}

fn player(app: &mut App, at: DVec3, mode: LocomotionMode) -> Entity {
    app.world
        .spawn((
            Transform::from_translation(at),
            Faction::PLAYER,
            LocomotionState {
                mode,
                ..Default::default()
            },
        ))
        .id()
}

/// One step (1/60 s) of the stand-in player toward `to` at `speed`, standing still (idle)
/// once within `stop` of it.
fn step_toward(app: &mut App, who: Entity, to: DVec3, speed: f64, mode: LocomotionMode, stop: f64) {
    let at = pos(app, who);
    let d = DVec3::new(to.x - at.x, 0.0, to.z - at.z);
    let (v, mode) = if d.length() > stop {
        (d.normalize() * speed, mode)
    } else {
        (DVec3::ZERO, LocomotionMode::Idle)
    };
    app.world.get_mut::<Transform>(who).unwrap().translation += v / 60.0;
    let mut l = app.world.get_mut::<LocomotionState>(who).unwrap();
    l.velocity = v.as_vec3();
    l.mode = mode;
}

fn pos(app: &App, e: Entity) -> DVec3 {
    app.world.get::<Transform>(e).unwrap().translation
}

fn mind(app: &App, e: Entity) -> &AnimalMind {
    app.world.get::<AnimalMind>(e).unwrap()
}

fn flat(v: DVec3) -> DVec3 {
    DVec3::new(v.x, 0.0, v.z)
}

/// A runner comes straight at a grazing deer herd: how close it gets before one bolts.
fn bolt_distance(tier: DifficultyTier) -> f64 {
    let mut app = app(tier);
    let herd: Vec<Entity> = (0..3)
        .map(|k| {
            let at = DVec3::new(k as f64 * 2.0 - 2.0, 0.0, 0.0);
            animal(&mut app, Animal::new(Species::Deer), at, 1)
        })
        .collect();
    app.update_n(30);
    let man = player(&mut app, DVec3::new(0.0, 0.0, 60.0), LocomotionMode::Run);
    for _ in 0..60 * 30 {
        step_toward(&mut app, man, DVec3::ZERO, 4.0, LocomotionMode::Run, 1.0);
        app.update();
        if herd
            .iter()
            .any(|&d| mind(&app, d).state == AnimalState::Flee)
        {
            return pos(&app, man).length();
        }
    }
    0.0
}

/// A rabbit run at from close by: how often its heading swings one way then the other in
/// the first 3 s of its flight.
fn jinks(tier: DifficultyTier) -> u32 {
    let mut app = app(tier);
    let rabbit = animal(&mut app, Animal::new(Species::Rabbit), DVec3::ZERO, 1);
    app.update_n(30);
    let man = player(&mut app, DVec3::new(0.0, 0.0, 12.0), LocomotionMode::Run);
    let (mut last, mut side, mut flips, mut fled_for) = (None::<DVec3>, 0.0, 0, 0);
    for _ in 0..60 * 12 {
        let at = pos(&app, rabbit);
        step_toward(&mut app, man, at, 4.0, LocomotionMode::Run, 1.0);
        app.update();
        let now = pos(&app, rabbit);
        if mind(&app, rabbit).state != AnimalState::Flee {
            continue;
        }
        fled_for += 1;
        if fled_for > 180 {
            break;
        }
        let v = flat(now - at);
        if v.length() < 1e-4 {
            continue;
        }
        if let Some(l) = last {
            let turn = l.cross(v).y.signum();
            if l.angle_between(v) > 0.02 && turn != side {
                if side != 0.0 {
                    flips += 1;
                }
                side = turn;
            }
        }
        last = Some(v);
    }
    flips
}

#[test]
fn on_easy_animals_let_you_closer_and_run_straight() {
    let (easy, medium) = (
        bolt_distance(DifficultyTier::Easy),
        bolt_distance(DifficultyTier::Medium),
    );
    eprintln!("deer bolt at: easy {easy:.1} m, medium {medium:.1} m");
    assert!(easy > 0.0 && easy < 0.75 * medium, "{easy} vs {medium}");
    let (easy, medium) = (jinks(DifficultyTier::Easy), jinks(DifficultyTier::Medium));
    eprintln!("rabbit jinks: easy {easy}, medium {medium}");
    assert!(medium >= 3, "a rabbit zigzags on Medium: {medium}");
    assert!(easy <= 1, "and runs straight on Easy: {easy}");
}

#[derive(Debug, Default)]
struct Hiding {
    /// It ever lay hidden, and in the grass.
    hid: bool,
    in_grass: bool,
    /// How close the walker came before it broke from hiding (m).
    broke_at: Option<f64>,
}

/// A walker comes at a rabbit with a patch of tall grass off to its side, then on to where
/// it went.
fn rabbit_in_grass(tier: DifficultyTier) -> Hiding {
    let mut app = app(tier);
    let g = grass(&mut app, vec![[4.0, -4.0, 8.0, 1.0]]);
    let rabbit = animal(&mut app, Animal::new(Species::Rabbit), DVec3::ZERO, 1);
    app.update_n(30);
    let man = player(&mut app, DVec3::new(0.0, 0.0, 25.0), LocomotionMode::Walk);
    let mut h = Hiding::default();
    for _ in 0..60 * 40 {
        let at = pos(&app, rabbit);
        step_toward(&mut app, man, at, 1.4, LocomotionMode::Walk, 0.5);
        app.update();
        let s = mind(&app, rabbit).state;
        if s == AnimalState::Hide {
            h.hid = true;
            h.in_grass |= g.inside(pos(&app, rabbit) + DVec3::Y * 0.2);
        } else if h.hid && s == AnimalState::Flee && h.broke_at.is_none() {
            h.broke_at = Some(flat(pos(&app, man) - pos(&app, rabbit)).length());
        }
    }
    h
}

#[test]
fn on_hard_a_rabbit_runs_into_tall_grass_and_keeps_still_until_almost_trodden_on() {
    let hard = rabbit_in_grass(DifficultyTier::Hard);
    let medium = rabbit_in_grass(DifficultyTier::Medium);
    eprintln!("hard {hard:?}\nmedium {medium:?}");
    assert!(hard.hid && hard.in_grass, "{hard:?}");
    let broke = hard.broke_at.expect("it breaks cover at last");
    assert!(broke < 4.5, "only with the walker almost on it: {broke}");
    assert!(!medium.hid, "{medium:?}");
}

/// A stand of trunks, 3 m apart, over x0..x1, z0..z1.
fn woods(app: &mut App, x: (f64, f64), z: (f64, f64)) -> Vec<Entity> {
    let mut out = Vec::new();
    let mut i = 0;
    let mut px = x.0;
    while px <= x.1 {
        let mut pz = z.0;
        while pz <= z.1 {
            let jitter = if i % 2 == 0 { 0.6 } else { -0.6 };
            out.push(
                app.world
                    .spawn((
                        Transform::from_translation(DVec3::new(px + jitter, 3.0, pz)),
                        Collider::capsule(0.25, 2.8),
                    ))
                    .id(),
            );
            i += 1;
            pz += 3.0;
        }
        px += 3.0;
    }
    out
}

#[derive(Debug, Default)]
struct Watch {
    /// Seconds of the last 8 the herd's deer stood in the woods, all told.
    in_woods_secs: f64,
    /// Seconds of the last 8 they stood in the woods, done running (watching, or grazing
    /// again).
    stood_secs: f64,
    /// How far they ran from the runner's stopping place, on average (m).
    off: f64,
}

/// A runner comes at a deer herd with woods off to its side, and stops where it grazed.
fn deer_and_woods(tier: DifficultyTier) -> Watch {
    let mut app = app(tier);
    woods(&mut app, (12.0, 30.0), (-14.0, 4.0));
    let herd: Vec<Entity> = (0..3)
        .map(|k| {
            let at = DVec3::new(k as f64 * 2.0 - 2.0, 0.0, 0.0);
            animal(&mut app, Animal::new(Species::Deer), at, 1)
        })
        .collect();
    app.update_n(30);
    let man = player(&mut app, DVec3::new(0.0, 0.0, 40.0), LocomotionMode::Run);
    let mut w = Watch::default();
    let secs = 30;
    for f in 0..60 * secs {
        step_toward(&mut app, man, DVec3::ZERO, 4.0, LocomotionMode::Run, 1.0);
        app.update();
        if f < 60 * (secs - 8) {
            continue;
        }
        for &d in &herd {
            let p = pos(&app, d);
            if (10.5..31.5).contains(&p.x) && (-15.5..5.5).contains(&p.z) {
                w.in_woods_secs += 1.0 / 60.0;
                if mind(&app, d).state != AnimalState::Flee {
                    w.stood_secs += 1.0 / 60.0;
                }
            }
        }
    }
    w.off = herd
        .iter()
        .map(|&d| flat(pos(&app, d)).length())
        .sum::<f64>()
        / 3.0;
    w
}

#[test]
fn on_hard_deer_run_into_the_woods_and_stop_there_to_watch() {
    let hard = deer_and_woods(DifficultyTier::Hard);
    let medium = deer_and_woods(DifficultyTier::Medium);
    eprintln!("hard {hard:?}\nmedium {medium:?}");
    // Most of the last 8 s, all three in the woods, most of that standing watching.
    assert!(hard.in_woods_secs > 0.7 * 24.0, "{hard:?}");
    assert!(hard.stood_secs > 0.9 * hard.in_woods_secs, "{hard:?}");
    assert!(
        medium.in_woods_secs < 0.5 * hard.in_woods_secs,
        "{medium:?}"
    );
    // And they stop sooner: in the woods, not far off across the open.
    assert!(hard.off < 0.75 * medium.off, "{hard:?} vs {medium:?}");
}

#[derive(Debug, Default)]
struct Bunch {
    /// Mean distance to the nearest neighbour, at the start and after the herd has watched
    /// a while (m).
    spread_before: f64,
    spread_after: f64,
    /// Mean distance to the herd's middle of the young, and of the grown (m).
    young_off: f64,
    grown_off: f64,
}

/// A loose herd of six deer (two young) watches a figure standing off at the edge of its
/// flight distance.
fn alert_herd(tier: DifficultyTier) -> Bunch {
    let mut app = app(tier);
    let herd: Vec<Entity> = (0..6)
        .map(|k| {
            let a = k as f64 / 6.0 * std::f64::consts::TAU;
            let at = DVec3::new(a.cos() * 6.0, 0.0, a.sin() * 6.0);
            let young = k % 3 == 0;
            animal(
                &mut app,
                Animal {
                    young,
                    ..Animal::new(Species::Deer)
                },
                at,
                1,
            )
        })
        .collect();
    app.update_n(30);
    let spread = |app: &App| {
        let ps: Vec<DVec3> = herd.iter().map(|&d| flat(pos(app, d))).collect();
        ps.iter()
            .enumerate()
            .map(|(i, p)| {
                ps.iter()
                    .enumerate()
                    .filter(|(j, _)| *j != i)
                    .map(|(_, q)| p.distance(*q))
                    .fold(f64::MAX, f64::min)
            })
            .sum::<f64>()
            / ps.len() as f64
    };
    let mut b = Bunch {
        spread_before: spread(&app),
        ..Default::default()
    };
    player(&mut app, DVec3::new(0.0, 0.0, 26.0), LocomotionMode::Idle);
    let mut alert = 0;
    for _ in 0..60 * 8 {
        app.update();
        alert += herd
            .iter()
            .filter(|&&d| mind(&app, d).state == AnimalState::Alert)
            .count();
    }
    assert!(alert > 60 * 8 * 3, "the herd stands watching: {alert}");
    b.spread_after = spread(&app);
    let mid = herd.iter().map(|&d| flat(pos(&app, d))).sum::<DVec3>() / 6.0;
    for (k, &d) in herd.iter().enumerate() {
        let off = flat(pos(&app, d)).distance(mid);
        if k % 3 == 0 {
            b.young_off += off / 2.0;
        } else {
            b.grown_off += off / 4.0;
        }
    }
    b
}

#[test]
fn on_hard_an_alert_herd_bunches_up_with_the_young_in_the_middle() {
    let hard = alert_herd(DifficultyTier::Hard);
    let medium = alert_herd(DifficultyTier::Medium);
    eprintln!("hard {hard:?}\nmedium {medium:?}");
    assert!(
        hard.spread_after < 0.6 * hard.spread_before,
        "the herd closes up: {hard:?}"
    );
    assert!(hard.spread_after < HERD_CLOSE as f64 * 1.6, "{hard:?}");
    assert!(
        hard.young_off < hard.grown_off,
        "the young inside: {hard:?}"
    );
    assert!(
        medium.spread_after > 0.8 * medium.spread_before,
        "on Medium it doesn't: {medium:?}"
    );
}

/// A tusker with three piglets of its sounder rooting 12 m from it; someone creeps up on the
/// piglets from the far side. Whether the tusker charges (on its own account a tusker only
/// turns when cornered or hurt), and how close it comes to them.
fn creep_on_piglets(tier: DifficultyTier) -> (bool, f64) {
    let mut app = app(tier);
    let boar = |app: &mut App, at: DVec3, male: bool, young: bool| {
        animal(
            app,
            Animal {
                male,
                young,
                ..Animal::new(Species::Boar)
            },
            at,
            40,
        )
    };
    let grown = [boar(&mut app, DVec3::new(0.0, 0.0, 0.0), true, false)];
    let young: Vec<Entity> = (0..3)
        .map(|k| {
            boar(
                &mut app,
                DVec3::new(12.0 + k as f64, 0.0, k as f64 - 1.0),
                false,
                true,
            )
        })
        .collect();
    app.update_n(30);
    let man = player(&mut app, DVec3::new(26.0, 0.0, 0.0), LocomotionMode::Crouch);
    let (mut charged, mut nearest) = (false, f64::MAX);
    for f in 0..60 * 12 {
        let at = pos(&app, young[0]);
        step_toward(&mut app, man, at, 1.0, LocomotionMode::Crouch, 6.0);
        app.update();
        if std::env::var("TRACE").is_ok() && f % 30 == 0 {
            let st = |e: Entity| format!("{:?}[{:.1}]", mind(&app, e).state, pos(&app, e));
            eprintln!(
                "t{:.1} man [{:.1}] tusker {} young {} {} {}",
                f as f64 / 60.0,
                pos(&app, man),
                st(grown[0]),
                st(young[0]),
                st(young[1]),
                st(young[2])
            );
        }
        for &g in &grown {
            charged |= mind(&app, g).state == AnimalState::Charge;
            nearest = nearest.min(flat(pos(&app, g) - pos(&app, man)).length());
        }
    }
    (charged, nearest)
}

#[test]
fn on_extreme_boars_charge_whatever_comes_near_their_young() {
    let (charged, nearest) = creep_on_piglets(DifficultyTier::Extreme);
    eprintln!("extreme: charged {charged}, came within {nearest:.1} m");
    assert!(charged && nearest < 2.0, "{charged} {nearest}");
    let (charged, nearest) = creep_on_piglets(DifficultyTier::VeryHard);
    eprintln!("very hard: charged {charged}, came within {nearest:.1} m");
    assert!(!charged, "{nearest}");
}

/// Three hungry wolves on a lone tusker that stands its ground: over the time all three are
/// close to it, the widest gap round it between them (radians, 2π = all on one side).
fn ring(tier: DifficultyTier) -> (f64, f64) {
    let mut app = app(tier);
    app.world.resource_mut::<WildlifeSettings>().respawn_secs = 0.0;
    let tusker = animal(
        &mut app,
        Animal {
            male: true,
            ..Animal::new(Species::Boar)
        },
        DVec3::ZERO,
        40,
    );
    let pack: Vec<Entity> = (0..3)
        .map(|k| {
            let at = DVec3::new(k as f64 * 2.0 - 2.0, 0.0, -40.0);
            animal(&mut app, Animal::new(Species::Wolf), at, 2)
        })
        .collect();
    app.update();
    for &w in &pack {
        app.world.get_mut::<AnimalMind>(w).unwrap().hunger = 1.0;
    }
    let (mut gap_sum, mut close_secs) = (0.0f64, 0.0f64);
    for f in 0..60 * 60 {
        app.update();
        if std::env::var("TRACE").is_ok() && f % 15 == 0 && (900..1500).contains(&f) {
            let st = |e: Entity| format!("{:?}[{:.1}]", mind(&app, e).state, pos(&app, e));
            eprintln!(
                "t{:.2} boar {} wolves {} {} {}",
                f as f64 / 60.0,
                st(tusker),
                st(pack[0]),
                st(pack[1]),
                st(pack[2])
            );
        }
        {
            let mut v = app.world.get_mut::<NpcVitals>(tusker).unwrap();
            v.health = v.max_health;
        }
        let b = flat(pos(&app, tusker));
        let mut angles: Vec<f64> = pack
            .iter()
            .map(|&w| flat(pos(&app, w)) - b)
            .filter(|o| o.length() < 9.0)
            .map(|o| o.z.atan2(o.x))
            .collect();
        if angles.len() < 3 {
            continue;
        }
        angles.sort_by(f64::total_cmp);
        let widest = (0..3)
            .map(|i| {
                let next = if i == 2 {
                    angles[0] + std::f64::consts::TAU
                } else {
                    angles[i + 1]
                };
                next - angles[i]
            })
            .fold(0.0, f64::max);
        gap_sum += widest / 60.0;
        close_secs += 1.0 / 60.0;
    }
    (gap_sum / close_secs.max(1e-6), close_secs)
}

#[test]
fn on_extreme_wolves_ring_a_boar_at_bay() {
    let (extreme, x_secs) = ring(DifficultyTier::Extreme);
    let (vh, vh_secs) = ring(DifficultyTier::VeryHard);
    eprintln!(
        "widest gap: extreme {extreme:.2} rad over {x_secs:.1} s, very hard {vh:.2} rad over {vh_secs:.1} s"
    );
    assert!(x_secs > 10.0, "the pack stays on the boar: {x_secs}");
    // Round it: no gap much over a third of the circle and a half.
    assert!(extreme < 3.3, "{extreme}");
    assert!(vh_secs < 5.0 || extreme < vh - 0.5, "{extreme} vs {vh}");
}

/// A pack of four hungry wolves stalks a deer herd 45 m off with tall grass near where its
/// ambusher would lie in wait: where the ambusher lay as the pack ran the quarry
/// at it, and whether that was in the grass.
fn ambush(tier: DifficultyTier, patches: Vec<[f64; 4]>) -> Option<(DVec3, bool)> {
    let mut app = app(tier);
    app.world.resource_mut::<WildlifeSettings>().respawn_secs = 0.0;
    let g = grass(&mut app, patches);
    for k in 0..3 {
        let a = k as f64 / 3.0 * std::f64::consts::TAU;
        let at = DVec3::new(a.cos() * 2.5, 0.0, a.sin() * 2.5);
        animal(&mut app, Animal::new(Species::Deer), at, 1);
    }
    let pack: Vec<Entity> = (0..4)
        .map(|k| {
            let a = k as f64 / 4.0 * std::f64::consts::TAU;
            let at = DVec3::new(a.cos() * 2.5, 0.0, -45.0 + a.sin() * 2.5);
            animal(&mut app, Animal::new(Species::Wolf), at, 2)
        })
        .collect();
    app.update();
    for &w in &pack {
        app.world.get_mut::<AnimalMind>(w).unwrap().hunger = 1.0;
    }
    // Where it lay as the pack ran the quarry at it, up to when it sprang.
    let mut lay = None;
    for _ in 0..60 * 60 {
        app.update();
        let running = pack
            .iter()
            .any(|&w| mind(&app, w).state == AnimalState::Chase);
        for &w in &pack {
            let m = mind(&app, w);
            if m.role != PackRole::Ambusher || !running {
                continue;
            }
            if m.state == AnimalState::Stalk {
                let p = pos(&app, w);
                lay = Some((p, g.inside(p + DVec3::Y * 0.3)));
            } else if lay.is_some() {
                return lay;
            }
        }
    }
    lay
}

#[test]
fn on_extreme_the_wolf_lying_in_wait_lies_in_tall_grass() {
    // Two strips of grass, either side of the open ground where it would lie.
    let patches = vec![[4.0, 16.0, 10.5, 36.0], [19.5, 16.0, 27.0, 36.0]];
    let x = ambush(DifficultyTier::Extreme, patches.clone());
    let vh = ambush(DifficultyTier::VeryHard, patches);
    eprintln!("extreme {x:?}\nvery hard {vh:?}");
    assert!(x.is_some_and(|a| a.1), "{x:?}");
    assert!(vh.is_some_and(|a| !a.1), "{vh:?}");
}
