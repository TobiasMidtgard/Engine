//! Weapon fighting: a sword-and-shield combo that cuts twice and thrusts, a heavy press that
//! rams the shield in, a raised shield taking an overhead, a great sword's sweep and rising
//! cut, its overhead landing through its armor, and a bow drawn to the jaw and loosed at a
//! target off to the side. Runs without the fetched models.

use engine_anim::combat::{Attacker, Shield};
use engine_anim::demo::spawn_humanoid;
use engine_anim::{AnimationPlugin, Animator, HitReaction, build::humanoid::Outfit};
use engine_core::prelude::*;
use engine_core::time::TimeSource;
use engine_world::animation::{AnimationEvent, AnimationParams};
use engine_world::{DamageEvent, DamageKind, WorldPlugin};

const DT: f32 = 1.0 / 60.0;

/// What landed, in order.
#[derive(Resource, Default)]
struct Landed(Vec<DamageEvent>);

/// Clip events, with the time they came.
#[derive(Resource, Default)]
struct Events(Vec<(String, f32)>);

fn record(
    time: Res<Time>,
    mut damage: MessageReader<DamageEvent>,
    mut events: MessageReader<AnimationEvent>,
    mut landed: ResMut<Landed>,
    mut seen: ResMut<Events>,
) {
    landed.0.extend(damage.read().cloned());
    let t = time.elapsed_secs();
    seen.0.extend(events.read().map(|e| (e.name.clone(), t)));
}

/// A fighter with `set` `gap` meters in front of a dummy that faces it (and has a shield,
/// held up, if `shield`), settled.
fn duel(set: &str, gap: f32, shield: bool) -> (App, Entity, Entity) {
    let mut app = App::new();
    app.world.resource_mut::<EngineConfig>().headless = true;
    app.world
        .resource_mut::<Time>()
        .set_source(TimeSource::Manual {
            step_secs: DT as f64,
        });
    app.add_plugins((WorldPlugin, AnimationPlugin));
    app.init_resource::<Landed>()
        .init_resource::<Events>()
        .add_systems(PostUpdate, record);
    let dummy = spawn_humanoid(
        &mut app.world,
        Outfit::Shirt,
        Transform::default().with_rotation(Quat::from_rotation_y(std::f32::consts::PI)),
    );
    app.world.entity_mut(dummy).insert(HitReaction::default());
    if shield {
        app.world.entity_mut(dummy).insert(Shield::default());
        if let Some(mut p) = app.world.get_mut::<AnimationParams>(dummy) {
            p.set("shield_guard", 1.0);
        }
    }
    let fighter = spawn_humanoid(
        &mut app.world,
        Outfit::Athlete,
        Transform::from_xyz(0.0, 0.0, gap as f64),
    );
    app.world
        .entity_mut(fighter)
        .insert(Attacker::new(set, Some(dummy)));
    for _ in 0..40 {
        app.update();
    }
    app.world.resource_mut::<Landed>().0.clear();
    app.world.resource_mut::<Events>().0.clear();
    (app, fighter, dummy)
}

/// Press `inputs` one after another, each as soon as the move playing can take it (or at
/// once from neutral), then let the last move play out. The moves played, and the attacker
/// as the last one ended.
fn combo(app: &mut App, fighter: Entity, inputs: &[&str]) -> (Vec<String>, u32, u32) {
    let mut moves: Vec<String> = Vec::new();
    let mut next = 0;
    let (mut missed, mut blocked) = (0, 0);
    let (mut last_missed, mut last_blocked) = (0, 0);
    for _ in 0..(6.0 / DT) as usize {
        let a = app.world.get::<Attacker>(fighter).unwrap().clone();
        if next < inputs.len() && (a.current.is_none() && moves.is_empty() || a.cancel_open) {
            app.world
                .get_mut::<Animator>(fighter)
                .unwrap()
                .trigger(inputs[next]);
            next += 1;
        }
        app.update();
        let a = app.world.get::<Attacker>(fighter).unwrap().clone();
        if let Some(c) = &a.current
            && moves.last() != Some(c)
        {
            moves.push(c.clone());
            (last_missed, last_blocked) = (0, 0);
        }
        missed += a.missed.saturating_sub(last_missed);
        blocked += a.blocked.saturating_sub(last_blocked);
        (last_missed, last_blocked) = (a.missed, a.blocked);
        if next == inputs.len() && a.current.is_none() && !moves.is_empty() {
            break;
        }
    }
    (moves, missed, blocked)
}

fn landed(app: &App) -> Vec<DamageEvent> {
    app.world.resource::<Landed>().0.clone()
}

#[test]
fn a_sword_combo_cuts_twice_and_thrusts() {
    let (mut app, fighter, dummy) = duel("sword_shield", 1.6, false);
    let (moves, missed, _) = combo(
        &mut app,
        fighter,
        &["attack_light", "attack_light", "attack_light"],
    );
    assert_eq!(moves, ["sword_slash", "sword_backhand", "sword_thrust"]);
    let hits = landed(&app);
    let kinds: Vec<DamageKind> = hits.iter().map(|d| d.kind).collect();
    assert_eq!(
        kinds,
        [DamageKind::Slash, DamageKind::Slash, DamageKind::Pierce],
        "{missed} missed"
    );
    assert!(
        hits.iter()
            .all(|d| d.target == dummy && d.source == Some(fighter))
    );
    // Each lands on the dummy's body, between the hips and the head.
    for d in &hits {
        assert!((0.8..1.8).contains(&d.point.y), "landed at {:?}", d.point);
    }
    // The forehand pushes to the fighter's left (world −X here, as the fighter faces −Z),
    // the backhand to its right.
    assert!(
        hits[0].impulse.x < -1.0,
        "forehand push {:?}",
        hits[0].impulse
    );
    assert!(
        hits[1].impulse.x > 1.0,
        "backhand push {:?}",
        hits[1].impulse
    );
    // It closed from 1.6 m to cutting range, and stepped back out after the thrust.
    let at = |e: Entity| app.world.get::<GlobalTransform>(e).unwrap().translation();
    let gap = (at(fighter) - at(dummy)).with_y(0.0).length();
    assert!((1.2..1.9).contains(&gap), "ended {gap:.2} m away");
}

#[test]
fn a_heavy_press_after_the_slash_rams_the_shield_in() {
    let (mut app, fighter, _) = duel("sword_shield", 1.2, false);
    let (moves, missed, _) = combo(&mut app, fighter, &["attack_light", "attack_heavy"]);
    assert_eq!(moves, ["sword_slash", "shield_bash"]);
    let hits = landed(&app);
    assert_eq!(hits.len(), 2, "{missed} missed");
    assert_eq!(hits[1].kind, DamageKind::Blunt);
    // Little harm, a big shove.
    assert!(hits[1].amount < hits[0].amount);
    assert!(hits[1].impulse.length() > 60.0, "{:?}", hits[1].impulse);
}

#[test]
fn a_raised_shield_takes_the_overhead() {
    let (mut app, fighter, dummy) = duel("sword_shield", 1.3, true);
    let (moves, missed, blocked) = combo(&mut app, fighter, &["attack_heavy"]);
    assert_eq!(moves, ["sword_overhead"]);
    assert_eq!((blocked, missed), (1, 0));
    let hits = landed(&app);
    assert_eq!(hits.len(), 1);
    // The shield lets 5% through, at its face.
    assert!(
        (hits[0].amount - 24.0 * 0.05).abs() < 1e-3,
        "{}",
        hits[0].amount
    );
    let shield = app.world.get::<Shield>(dummy).unwrap().clone();
    let a = app.world.get::<Animator>(dummy).unwrap();
    let gt = app.world.get::<GlobalTransform>(dummy).unwrap();
    let hand = a.skeleton.index_of(&shield.bone).unwrap();
    let face = engine_anim::ik::to_world(gt, a.model[hand].transform_point(shield.center));
    assert!(
        hits[0].point.distance(face) < 0.45,
        "landed at {:?}, the shield at {face:?}",
        hits[0].point
    );
}

/// A blade the shield takes bounces back up its arc after the hitstop instead of cutting on
/// through the shield, and the move ends with the rebound; the shield-bearer shakes side to
/// side through the hitstop.
#[test]
fn a_blocked_blade_bounces_back_and_the_struck_body_shakes() {
    use engine_anim::build::weapons::{BLADE_AXIS, GRIP, SWORD_BLADE};
    let (mut app, fighter, dummy) = duel("sword_shield", 1.3, true);
    app.world
        .get_mut::<Animator>(fighter)
        .unwrap()
        .trigger("attack_heavy");
    let tip = |app: &App| {
        let a = app.world.get::<Animator>(fighter).unwrap();
        let gt = app.world.get::<GlobalTransform>(fighter).unwrap();
        let hand = a.skeleton.index_of("hand_r").unwrap();
        let p = a.model[hand].transform_point(GRIP + BLADE_AXIS * SWORD_BLADE[1]);
        engine_anim::ik::to_world(gt, p).y
    };
    let (mut contact, mut at_contact, mut highest_after) = (None::<f32>, 0.0, f64::MIN);
    let (mut recoiled, mut over) = (false, None::<f32>);
    let mut hips_x: Vec<f32> = Vec::new();
    for frame in 1..=(2.0 / DT) as usize {
        app.update();
        let t = frame as f32 * DT;
        let a = app.world.get::<Attacker>(fighter).unwrap();
        if contact.is_none() && a.blocked > 0 {
            contact = Some(t);
            at_contact = tip(&app);
            recoiled = a.recoil > 0.0;
        } else if contact.is_some_and(|c| t - c < 0.6) {
            highest_after = highest_after.max(tip(&app));
        }
        if contact.is_some() && over.is_none() && a.current.is_none() {
            over = Some(t);
        }
        let d = app.world.get::<Animator>(dummy).unwrap();
        if contact.is_some() && d.frozen() > 0.0 {
            hips_x.push(d.local[d.skeleton.index_of("hips").unwrap()].t.x);
        }
    }
    let contact = contact.expect("the overhead never met the shield");
    assert!(recoiled, "no rebound published at contact");
    assert!(
        highest_after > at_contact + 0.12,
        "the blade went on down: tip {at_contact:.2} m at contact, at most {highest_after:.2} m after"
    );
    let over = over.expect("the move never ended") - contact;
    assert!(
        (0.45..0.8).contains(&over),
        "ended {over:.2} s after contact"
    );
    let spread = hips_x.iter().cloned().fold(f32::MIN, f32::max)
        - hips_x.iter().cloned().fold(f32::MAX, f32::min);
    assert!(
        hips_x.len() >= 3 && spread > 0.008,
        "hips shook {spread} m over {} frames",
        hips_x.len()
    );
}

#[test]
fn a_great_sword_sweeps_rises_and_lands_its_overhead_through_its_armor() {
    let (mut app, fighter, _) = duel("greatsword", 2.0, false);
    let (moves, missed, _) = combo(
        &mut app,
        fighter,
        &["attack_light", "attack_light", "attack_heavy"],
    );
    assert_eq!(
        moves,
        ["greatsword_sweep", "greatsword_rise", "greatsword_overhead"]
    );
    let hits = landed(&app);
    assert_eq!(hits.len(), 3, "{missed} missed");
    assert!(hits.iter().all(|d| d.kind == DamageKind::Slash));
    assert!(hits[2].amount > hits[0].amount * 1.5);

    // The overhead from neutral: armored through its wind-up, not after it lands.
    let (mut app, fighter, _) = duel("greatsword", 1.5, false);
    app.world
        .get_mut::<Animator>(fighter)
        .unwrap()
        .trigger("attack_heavy");
    let (mut armored_early, mut armored_late) = (0, 0);
    for _ in 0..(1.6 / DT) as usize {
        app.update();
        let a = app.world.get::<Attacker>(fighter).unwrap();
        if a.current.as_deref() == Some("greatsword_overhead") && a.armored {
            if a.time < 0.6 {
                armored_early += 1;
            } else if a.time > 0.8 {
                armored_late += 1;
            }
        }
    }
    assert!(
        armored_early > 20,
        "armored {armored_early} frames of the wind-up"
    );
    assert_eq!(armored_late, 0);
    assert_eq!(landed(&app).len(), 1);
}

#[test]
fn a_bow_is_drawn_to_the_jaw_turned_on_the_target_and_loosed() {
    let mut app = App::new();
    app.world.resource_mut::<EngineConfig>().headless = true;
    app.world
        .resource_mut::<Time>()
        .set_source(TimeSource::Manual {
            step_secs: DT as f64,
        });
    app.add_plugins((WorldPlugin, AnimationPlugin));
    app.init_resource::<Landed>()
        .init_resource::<Events>()
        .add_systems(PostUpdate, record);
    // A target 10 m off to the archer's right front.
    let target = spawn_humanoid(
        &mut app.world,
        Outfit::Shirt,
        Transform::from_xyz(6.0, 0.0, -8.0),
    );
    let archer = spawn_humanoid(&mut app.world, Outfit::Athlete, Transform::default());
    app.world
        .entity_mut(archer)
        .insert(Attacker::new("bow", Some(target)));
    for _ in 0..30 {
        app.update();
    }
    app.world.resource_mut::<Events>().0.clear();
    let start = app.world.resource::<Time>().elapsed_secs();
    app.world
        .get_mut::<Animator>(archer)
        .unwrap()
        .trigger("attack_light");
    let mut drawn_hand = None;
    let mut facing_at_release = None;
    for _ in 0..(1.6 / DT) as usize {
        app.update();
        let a = app.world.get::<Animator>(archer).unwrap();
        let attacker = app.world.get::<Attacker>(archer).unwrap();
        if attacker.current.as_deref() == Some("bow_shot") && (0.85..0.9).contains(&attacker.time) {
            let hand = a.model[a.skeleton.index_of("hand_r").unwrap()].t;
            let head = a.model[a.skeleton.index_of("head").unwrap()].t;
            drawn_hand = Some((hand, head, a.curve("draw")));
        }
        let released = app
            .world
            .resource::<Events>()
            .0
            .iter()
            .any(|e| e.0 == "release");
        if released && facing_at_release.is_none() {
            let gt = app.world.get::<GlobalTransform>(archer).unwrap();
            facing_at_release = Some(gt.rotation() * Vec3::NEG_Z);
        }
    }
    let release = app
        .world
        .resource::<Events>()
        .0
        .iter()
        .find(|e| e.0 == "release")
        .map(|e| e.1 - start)
        .expect("the arrow was never loosed");
    assert!((0.85..1.1).contains(&release), "loosed {release:.2} s in");
    // At full draw the hand is by the jaw: a little below the head, close beside it.
    let (hand, head, draw) = drawn_hand.expect("no full draw seen");
    assert!(draw > 0.95, "drawn {draw}");
    assert!(
        hand.y < head.y && hand.y > head.y - 0.25,
        "hand {hand} head {head}"
    );
    assert!(
        hand.with_y(0.0).distance(head.with_y(0.0)) < 0.2,
        "hand {hand} head {head}"
    );
    // Turned on the target as it drew.
    let want = Vec3::new(6.0, 0.0, -8.0).normalize();
    let facing = facing_at_release.unwrap().with_y(0.0).normalize();
    assert!(facing.dot(want) > 0.98, "facing {facing}, target {want}");
}

/// The paddock's weapons yard plays its script: the swordsman's combo cuts twice and thrusts,
/// its overhead bounces off the partner's raised shield and the partner cuts back while it
/// recovers, then a slash, a bash and a thrust land; the partner's cut bounces off the
/// swordsman's shield and the swordsman answers; the great sword's sweep, rising cut and
/// overhead land; and the archer's arrows stick in the butt's face.
#[test]
fn the_paddock_weapons_yard_plays_out() {
    use engine_anim::demo::spawn_demo_world;
    use engine_anim::props::Arrow;
    let mut app = App::new();
    app.world.resource_mut::<EngineConfig>().headless = true;
    app.world
        .resource_mut::<Time>()
        .set_source(TimeSource::Manual {
            step_secs: DT as f64,
        });
    app.add_plugins((WorldPlugin, AnimationPlugin));
    let d = spawn_demo_world(&mut app.world, DVec3::ZERO);
    engine_anim::character::finish_loading(&mut app.world);
    let fighters = [
        ("swordsman", d.swordsman),
        ("partner", d.shield_partner),
        ("greatsword", d.greatsword),
        ("archer", d.archer),
    ];
    // Each move played: who, which, when it started, and what it did (hits, blocked, missed).
    let mut moves: Vec<(&str, String, f32, [u32; 3])> = Vec::new();
    let mut last: Vec<Option<(String, f32)>> = vec![None; fighters.len()];
    for frame in 1..=(13.0 / DT) as usize {
        app.update();
        let t = frame as f32 * DT;
        for (k, (who, e)) in fighters.into_iter().enumerate() {
            let a = app.world.get::<Attacker>(e).unwrap();
            let now = a.current.clone().map(|c| (c, a.time));
            let started = match (&last[k], &now) {
                (None, Some(_)) => true,
                (Some((was, wt)), Some((c, ct))) => was != c || ct < wt,
                _ => false,
            };
            if started {
                moves.push((who, now.clone().unwrap().0, t, [0; 3]));
            }
            if now.is_some()
                && let Some(m) = moves.iter_mut().rev().find(|m| m.0 == who)
            {
                m.3 = [a.hits, a.blocked, a.missed];
            }
            last[k] = now;
        }
    }
    // What each played in its first round (the duel's is 13 s, the great sword's 10, the
    // archer's 9).
    let played = |who: &str| -> Vec<(String, [u32; 3])> {
        let round = match who {
            "greatsword" => 10.0,
            "archer" => 9.0,
            _ => 13.0,
        };
        moves
            .iter()
            .filter(|m| m.0 == who && m.2 < round)
            .map(|m| (m.1.clone(), m.3))
            .collect()
    };
    let (hit, blocked) = ([1, 0, 0], [0, 1, 0]);
    let named = |list: &[(&str, [u32; 3])]| -> Vec<(String, [u32; 3])> {
        list.iter().map(|(m, r)| (m.to_string(), *r)).collect()
    };
    assert_eq!(
        played("swordsman"),
        named(&[
            ("sword_slash", hit),
            ("sword_backhand", hit),
            ("sword_thrust", hit),
            ("sword_overhead", blocked),
            ("sword_slash", hit),
            ("shield_bash", hit),
            ("sword_thrust", hit),
            ("sword_slash", hit),
        ])
    );
    // Each cuts back while the other's blade bounces off its shield.
    assert_eq!(
        played("partner"),
        named(&[("sword_slash", hit), ("sword_slash", blocked)])
    );
    assert_eq!(
        played("greatsword"),
        named(&[
            ("greatsword_sweep", hit),
            ("greatsword_rise", hit),
            ("greatsword_overhead", hit),
        ])
    );
    let shots: Vec<String> = played("archer").into_iter().map(|m| m.0).collect();
    assert_eq!(shots, ["bow_shot", "bow_shot_full", "bow_shot"]);
    // Every arrow still there (the first is cleared after 8 s) has stuck in the butt's face,
    // its head in the straw, within the rings.
    let butt = app
        .world
        .query::<(&Name, &Transform)>()
        .iter(&app.world)
        .find(|(n, _)| n.as_str() == "Archery butt")
        .map(|(_, t)| t.translation)
        .unwrap();
    let face = butt.z + 0.15;
    let mut q = app.world.query::<(&Arrow, &Transform)>();
    let arrows: Vec<(Option<f32>, DVec3)> = q
        .iter(&app.world)
        .map(|(a, t)| (a.stuck, t.translation))
        .collect();
    assert_eq!(arrows.len(), 3, "{arrows:?}");
    for (stuck, mid) in arrows {
        assert!(stuck.is_some(), "an arrow still flying: {mid:?}");
        let head = mid.z - engine_anim::props::ARROW_LENGTH as f64 * 0.5;
        assert!(
            (face - 0.15..face).contains(&head),
            "head at z {head}, face at {face}"
        );
        let off = (mid - butt).with_z(0.0).length();
        assert!(off < 0.5, "{off} m off the gold");
    }
}
