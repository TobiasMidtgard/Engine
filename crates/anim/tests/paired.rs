//! Paired ripostes: a press at a foe whose blade was just deflected, or whose guard was just
//! broken, throws a riposte played with the foe. The foe is lined up in front facing the
//! attacker, the blade goes through it on the keyed frame (unblockable, three times a
//! thrust), it ends on its back and gets up; nobody else's blows touch the pair meanwhile.
//! Runs without the fetched models.

use engine_anim::build::humanoid::{Outfit, RIPOSTED, RIPOSTED_LANDS};
use engine_anim::build::weapons::{
    BLADE_AXIS, GREATSWORD_RIPOSTE, GREATSWORD_RIPOSTE_DISTANCE, GRIP, SWORD_BLADE, SWORD_RIPOSTE,
    SWORD_RIPOSTE_DISTANCE,
};
use engine_anim::combat::{Attacker, Shield};
use engine_anim::demo::spawn_humanoid;
use engine_anim::paired::Riposted;
use engine_anim::{AnimationPlugin, Animator, HitReaction};
use engine_core::prelude::*;
use engine_core::time::TimeSource;
use engine_world::animation::AnimationParams;
use engine_world::{DamageEvent, DamageKind, WorldPlugin};

const DT: f32 = 1.0 / 60.0;

/// What landed, and when.
#[derive(Resource, Default)]
struct Landed(Vec<(DamageEvent, f32)>);

fn record(time: Res<Time>, mut damage: MessageReader<DamageEvent>, mut landed: ResMut<Landed>) {
    let t = time.elapsed_secs();
    landed.0.extend(damage.read().map(|d| (d.clone(), t)));
}

fn app() -> App {
    let mut app = App::new();
    app.world.resource_mut::<EngineConfig>().headless = true;
    app.world
        .resource_mut::<Time>()
        .set_source(TimeSource::Manual {
            step_secs: DT as f64,
        });
    app.add_plugins((WorldPlugin, AnimationPlugin));
    app.init_resource::<Landed>()
        .add_systems(PostUpdate, record);
    app
}

/// A fighter with the attack set `moves` at the origin facing −Z (turned round if `away`), and a foe `gap` m
/// ahead facing back with `foe_set` aimed at the fighter, a shield on the foe if `shield`.
fn pair(moves: &str, foe_set: &str, gap: f32, shield: bool, away: bool) -> (App, Entity, Entity) {
    let mut app = app();
    let turn = if away { std::f32::consts::PI } else { 0.0 };
    let fighter = spawn_humanoid(
        &mut app.world,
        Outfit::Athlete,
        Transform::default().with_rotation(Quat::from_rotation_y(turn)),
    );
    let foe = spawn_humanoid(
        &mut app.world,
        Outfit::Shirt,
        Transform::from_xyz(0.0, 0.0, -gap as f64)
            .with_rotation(Quat::from_rotation_y(std::f32::consts::PI)),
    );
    app.world.entity_mut(foe).insert((
        HitReaction::default(),
        Attacker::new(foe_set, Some(fighter)),
    ));
    if shield {
        app.world.entity_mut(foe).insert(Shield::default());
        set(&mut app, foe, "shield_guard", 1.0);
    }
    app.world
        .entity_mut(fighter)
        .insert((HitReaction::default(), Attacker::new(moves, Some(foe))));
    for _ in 0..30 {
        app.update();
    }
    app.world.resource_mut::<Landed>().0.clear();
    (app, fighter, foe)
}

fn set(app: &mut App, who: Entity, name: &str, value: f32) {
    if let Some(mut p) = app.world.get_mut::<AnimationParams>(who) {
        p.set(name, value);
    }
}

fn press(app: &mut App, who: Entity, input: &str) {
    app.world.get_mut::<Animator>(who).unwrap().trigger(input);
}

fn current(app: &App, who: Entity) -> Option<String> {
    app.world.get::<Attacker>(who).unwrap().current.clone()
}

fn flat(app: &App, who: Entity) -> Vec3 {
    app.world
        .get::<GlobalTransform>(who)
        .unwrap()
        .translation()
        .as_vec3()
        .with_y(0.0)
}

fn hips_height(app: &App, who: Entity) -> f32 {
    let a = app.world.get::<Animator>(who).unwrap();
    let gt = app.world.get::<GlobalTransform>(who).unwrap();
    let hips = a.skeleton.index_of("hips").unwrap();
    (a.bone_world(gt, hips).0.y - gt.translation().y) as f32
}

/// The swordsman (with a shield) deflects its partner's slash: frames from the press until
/// the slash meets the swordsman's shield held up all along.
fn slash_contact() -> usize {
    let (mut app, fighter, partner) = pair("sword_shield", "sword_shield", 1.3, false, false);
    app.world.entity_mut(fighter).insert(Shield::default());
    set(&mut app, fighter, "shield_guard", 1.0);
    press(&mut app, partner, "attack_light");
    for frame in 1..=(2.0 / DT) as usize {
        app.update();
        if app.world.get::<Attacker>(partner).unwrap().blocked > 0 {
            return frame;
        }
    }
    panic!("the slash never met the shield");
}

/// Follow a riposte from its first frame: the foe lined up, the blow, the fall and the
/// get-up. The riposte must have just started.
fn follow_riposte(app: &mut App, fighter: Entity, foe: Entity, distance: f32, damage: f32) {
    let start = app.world.resource::<Time>().elapsed_secs();
    assert!(
        app.world.get::<Animator>(foe).unwrap().overlay().is_some()
            || app.world.get::<Riposted>(foe).is_some(),
        "the foe's clip never started"
    );
    let mut worst_gap: f32 = 0.0;
    let mut worst_facing: f32 = 1.0;
    let mut tip_at_landing = None;
    let mut lowest_hips = f32::MAX;
    let mut played_clip = false;
    let mut damaged = false;
    for _ in 0..((RIPOSTED_LANDS + 4.8) / DT) as usize {
        app.update();
        let t = app.world.resource::<Time>().elapsed_secs() - start;
        let paired = app
            .world
            .get::<Riposted>(foe)
            .is_some_and(|r| !r.getting_up);
        played_clip |= app
            .world
            .get::<Animator>(foe)
            .unwrap()
            .overlay()
            .is_some_and(|o| o.clip.name == RIPOSTED);
        if paired && t > 0.3 {
            let to = flat(app, fighter) - flat(app, foe);
            worst_gap = worst_gap.max((to.length() - distance).abs());
            let facing = app.world.get::<GlobalTransform>(foe).unwrap().rotation() * Vec3::NEG_Z;
            worst_facing = worst_facing.min(facing.with_y(0.0).normalize().dot(to.normalize()));
        }
        if !damaged && !app.world.resource::<Landed>().0.is_empty() {
            damaged = true;
            let a = app.world.get::<Animator>(fighter).unwrap();
            let gt = app.world.get::<GlobalTransform>(fighter).unwrap();
            let hand = a.model[a.skeleton.index_of("hand_r").unwrap()];
            let length = if distance > 1.5 { 1.24 } else { SWORD_BLADE[1] };
            let tip =
                engine_anim::ik::to_world(gt, hand.transform_point(GRIP + BLADE_AXIS * length));
            let v = app.world.get::<Animator>(foe).unwrap();
            let vgt = app.world.get::<GlobalTransform>(foe).unwrap();
            let spine = v.bone_world(vgt, v.skeleton.index_of("spine").unwrap()).0;
            let neck = v.bone_world(vgt, v.skeleton.index_of("neck").unwrap()).0;
            tip_at_landing = Some((t, tip, spine, neck));
        }
        if t < RIPOSTED_LANDS + 2.0 {
            lowest_hips = lowest_hips.min(hips_height(app, foe));
        }
    }
    assert!(played_clip, "the foe never played {RIPOSTED}");
    assert!(
        worst_gap < 0.04,
        "the foe was {worst_gap:.3} m off its spot"
    );
    assert!(
        worst_facing > 0.995,
        "the foe faced the attacker {worst_facing:.4}"
    );
    let landed = app.world.resource::<Landed>().0.clone();
    assert_eq!(landed.len(), 1, "{landed:?}");
    let (hit, _) = &landed[0];
    assert_eq!(hit.target, foe);
    assert_eq!(hit.kind, DamageKind::Pierce);
    assert!((hit.amount - damage).abs() < 1e-3, "{}", hit.amount);
    let (t, tip, spine, neck) = tip_at_landing.unwrap();
    assert!(
        (t - RIPOSTED_LANDS).abs() < 0.08,
        "the blade went in {t:.2} s into the riposte"
    );
    // The tip is inside the body: behind its front, near the spine line, between the small
    // of the back and the neck.
    let off = (tip - spine).with_y(0.0).length();
    assert!(off < 0.2, "the tip was {off:.2} m from the spine line");
    assert!(
        tip.y > spine.y - 0.1 && tip.y < neck.y,
        "tip at {:.2} m, spine {:.2}, neck {:.2}",
        tip.y,
        spine.y,
        neck.y
    );
    assert!(
        lowest_hips < 0.3,
        "the foe never went down: hips at {lowest_hips:.2} m"
    );
    assert!(
        app.world.get::<Riposted>(foe).is_none(),
        "still in the pair after the get-up"
    );
    assert!(
        hips_height(app, foe) > 0.85,
        "not standing again: hips at {:.2} m",
        hips_height(app, foe)
    );
}

#[test]
fn a_press_after_a_deflect_runs_the_foe_through_and_it_falls_and_gets_up() {
    let contact = slash_contact();
    let (mut app, fighter, partner) = pair("sword_shield", "sword_shield", 1.3, false, false);
    app.world.entity_mut(fighter).insert(Shield::default());
    press(&mut app, partner, "attack_light");
    let mut deflected = false;
    for frame in 1..=(1.5 / DT) as usize {
        if frame == contact - 6 {
            set(&mut app, fighter, "shield_guard", 1.0);
        }
        app.update();
        if !deflected && app.world.get::<Attacker>(partner).unwrap().deflected > 0 {
            deflected = true;
            set(&mut app, fighter, "shield_guard", 0.0);
            press(&mut app, fighter, "attack_light");
        }
        if current(&app, fighter).as_deref() == Some(SWORD_RIPOSTE) {
            break;
        }
    }
    assert!(deflected, "the slash wasn't deflected");
    assert_eq!(current(&app, fighter).as_deref(), Some(SWORD_RIPOSTE));
    follow_riposte(&mut app, fighter, partner, SWORD_RIPOSTE_DISTANCE, 54.0);
}

#[test]
fn a_press_after_a_broken_guard_throws_the_great_sword_riposte() {
    let (mut app, fighter, dummy) = pair("greatsword", "boxing", 2.0, true, false);
    press(&mut app, fighter, "attack_heavy");
    let mut moves: Vec<String> = Vec::new();
    for _ in 1..=(3.0 / DT) as usize {
        let a = app.world.get::<Attacker>(fighter).unwrap();
        if moves.len() == 1 && a.current.is_none() {
            press(&mut app, fighter, "attack_light");
        }
        app.update();
        if let Some(c) = current(&app, fighter)
            && moves.last() != Some(&c)
        {
            moves.push(c.clone());
            if c == GREATSWORD_RIPOSTE {
                break;
            }
        }
    }
    assert_eq!(moves, ["greatsword_overhead", GREATSWORD_RIPOSTE]);
    // The overhead's block share already landed.
    app.world.resource_mut::<Landed>().0.clear();
    follow_riposte(&mut app, fighter, dummy, GREATSWORD_RIPOSTE_DISTANCE, 60.0);
}

#[test]
fn no_riposte_unless_the_foe_is_open_close_and_in_front() {
    for (open, gap, away, want) in [
        (false, 1.4, false, "sword_slash"),
        (true, 3.0, false, "sword_slash"),
        (true, 1.4, true, "sword_slash"),
        (true, 1.4, false, SWORD_RIPOSTE),
    ] {
        let (mut app, fighter, foe) = pair("sword_shield", "sword_shield", gap, false, away);
        if open {
            app.world.get_mut::<Attacker>(foe).unwrap().open = 0.9;
        }
        press(&mut app, fighter, "attack_light");
        app.update();
        assert_eq!(
            current(&app, fighter).as_deref(),
            Some(want),
            "open {open}, {gap} m, turned away {away}"
        );
    }
}

#[test]
fn nobody_else_touches_a_pair() {
    let (mut app, fighter, foe) = pair("sword_shield", "sword_shield", 1.3, false, false);
    // A second fighter beside the foe, cutting at it as the riposte starts.
    let other = spawn_humanoid(
        &mut app.world,
        Outfit::Athlete,
        Transform::from_xyz(1.2, 0.0, -1.3)
            .with_rotation(Quat::from_rotation_y(std::f32::consts::FRAC_PI_2)),
    );
    app.world
        .entity_mut(other)
        .insert(Attacker::new("sword_shield", Some(foe)));
    for _ in 0..10 {
        app.update();
    }
    app.world.get_mut::<Attacker>(foe).unwrap().open = 0.9;
    press(&mut app, fighter, "attack_light");
    press(&mut app, other, "attack_light");
    for _ in 0..(1.5 / DT) as usize {
        app.update();
    }
    let landed = app.world.resource::<Landed>().0.clone();
    assert!(
        landed.iter().all(|(d, _)| d.source == Some(fighter)),
        "{landed:?}"
    );
    assert_eq!(landed.len(), 1, "{landed:?}");
    assert!(
        app.world.get::<Attacker>(other).unwrap().current.is_some() || {
            // It swung (and whiffed through the pair).
            app.world.get::<Attacker>(other).unwrap().missed > 0
        }
    );
}
