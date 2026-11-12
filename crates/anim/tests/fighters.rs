//! Fighters driven by intent alone, as NPC AI drives them: two requested bodies with a
//! `Loadout`, facing each other, that nothing but `MoveIntent` and `StrikeTargets` steers.
//! The loadout arms them (props, move set, stance), a strike pressed through the intent plays
//! its wind-up, active and recovery phases in `AttackWindow`, a blow lands on the foe, the
//! guard follows `MoveIntent::block`, and a new loadout swaps the props.

use engine_anim::AnimationPlugin;
use engine_anim::combat::Attacker;
use engine_anim::fighter::Armed;
use engine_core::prelude::*;
use engine_core::time::TimeSource;
use engine_world::WorldPlugin;
use engine_world::animation::{
    AnimationEvent, AnimationParams, AnimationTrigger, BodyRequest, Ragdoll, SkeletonPose,
    StrikeTargets,
};
use engine_world::combat::{AttackPhase, AttackWindow, BodyZone, ItemClass, Knockback, Loadout};
use engine_world::events::AttackKind;
use engine_world::locomotion::MoveIntent;

const STEP: f64 = 1.0 / 60.0;

fn sword_and_shield() -> Loadout {
    Loadout {
        main: ItemClass::OneHandedBlade,
        off: ItemClass::Shield,
        two_handed: false,
    }
}

/// Two fighters a step apart along X, facing each other (bodies face their −Z).
fn duel() -> (App, Entity, Entity) {
    let mut app = App::new();
    app.world.resource_mut::<EngineConfig>().headless = true;
    app.world
        .resource_mut::<Time>()
        .set_source(TimeSource::Manual { step_secs: STEP });
    app.add_plugins((WorldPlugin, AnimationPlugin));
    let fighter = |app: &mut App, x: f64, yaw: f32, seed: u64| {
        let t = Transform::from_translation(DVec3::new(x, 0.0, 0.0))
            .with_rotation(Quat::from_rotation_y(yaw.to_radians()));
        app.world
            .spawn((
                t,
                GlobalTransform::from(t),
                BodyRequest::human(seed),
                sword_and_shield(),
                MoveIntent::default(),
                StrikeTargets::default(),
            ))
            .id()
    };
    let a = fighter(&mut app, -0.7, -90.0, 11);
    let b = fighter(&mut app, 0.7, 90.0, 12);
    for (me, foe) in [(a, b), (b, a)] {
        app.world.get_mut::<StrikeTargets>(me).unwrap().target = Some(foe);
    }
    app.update();
    engine_anim::character::finish_loading(&mut app.world);
    for _ in 0..30 {
        app.update();
    }
    (app, a, b)
}

#[test]
fn a_loadout_arms_a_requested_body() {
    let (app, a, _) = duel();
    assert!(app.world.get::<SkeletonPose>(a).is_some(), "no body");
    let armed = app.world.get::<Armed>(a).expect("armed");
    assert!(
        armed.props.len() >= 4,
        "sword and shield props: {}",
        armed.props.len()
    );
    let attacker = app.world.get::<Attacker>(a).expect("attacker");
    assert_eq!(attacker.set, "sword_shield");
    assert!(!attacker.warp, "the requester moves the body");
    let params = app.world.get::<AnimationParams>(a).unwrap();
    assert_eq!(params.get("sword_stance"), 1.0);
}

/// What NPC AI's mover does with a strike's step: carries the body by the published
/// `AttackWindow::warp_delta`.
fn take_steps(app: &mut App, people: [Entity; 2], moved: &mut [DVec3; 2]) {
    for (k, e) in people.into_iter().enumerate() {
        let step = app
            .world
            .get::<AttackWindow>(e)
            .unwrap()
            .warp_delta
            .as_dvec3();
        moved[k] += step;
        let mut t = app.world.get_mut::<Transform>(e).unwrap();
        t.translation += step;
    }
}

#[test]
fn an_intended_strike_plays_its_phases_and_lands() {
    let (mut app, a, b) = duel();
    let at = |app: &App, e: Entity| app.world.get::<Transform>(e).unwrap().translation;
    let start = [at(&app, a), at(&app, b)];
    let mut moved = [DVec3::ZERO; 2];
    app.world.get_mut::<MoveIntent>(a).unwrap().attack = Some(AttackKind::Melee);
    let mut phases: Vec<AttackPhase> = Vec::new();
    let mut hits = 0;
    let mut pushed = 0.0f32;
    for f in 0..(1.6 / STEP) as usize {
        if f == 2 {
            // The AI lets go; held or not, it is one press.
            app.world.get_mut::<MoveIntent>(a).unwrap().attack = None;
        }
        app.update();
        take_steps(&mut app, [a, b], &mut moved);
        let w = app.world.get::<AttackWindow>(a).expect("window");
        if let Some(p) = w.phase
            && phases.last() != Some(&p)
        {
            phases.push(p);
            assert!(w.phase_length_secs >= 0.0 && w.phase_secs <= w.phase_length_secs + 0.05);
            assert!(!w.name.is_empty());
        }
        hits = hits.max(app.world.get::<Attacker>(a).unwrap().hits);
        pushed = pushed.max(app.world.get::<Knockback>(b).unwrap().velocity.length());
    }
    assert_eq!(
        &phases[..3],
        [
            AttackPhase::Windup,
            AttackPhase::Active,
            AttackPhase::Recovery
        ],
        "{phases:?}"
    );
    assert!(hits >= 1, "the cut didn't land");
    // The blow pushed the foe through its published Knockback, gone once the stagger ends.
    assert!(pushed > 0.1, "no knockback: {pushed}");
    assert_eq!(app.world.get::<Knockback>(b).unwrap().velocity, Vec3::ZERO);
    assert_eq!(
        app.world.get::<AttackWindow>(a).unwrap().phase,
        None,
        "still striking"
    );
    // The strike stepped in, and only through the published step: the module itself moved
    // nobody.
    assert!(moved[0].length() > 0.05, "no step in: {:?}", moved[0]);
    for (k, e) in [a, b].into_iter().enumerate() {
        assert!((at(&app, e) - start[k] - moved[k]).length() < 1e-6);
    }
}

#[test]
fn the_guard_follows_the_intent_and_a_new_loadout_swaps_the_props() {
    let (mut app, a, _) = duel();
    app.world.get_mut::<MoveIntent>(a).unwrap().block = true;
    app.update();
    assert_eq!(
        app.world
            .get::<AnimationParams>(a)
            .unwrap()
            .get("shield_guard"),
        1.0
    );
    app.world.get_mut::<MoveIntent>(a).unwrap().block = false;
    app.update();
    assert_eq!(
        app.world
            .get::<AnimationParams>(a)
            .unwrap()
            .get("shield_guard"),
        0.0
    );
    let old = app.world.get::<Armed>(a).unwrap().props.clone();
    *app.world.get_mut::<Loadout>(a).unwrap() = Loadout {
        main: ItemClass::TwoHandedBlade,
        off: ItemClass::None,
        two_handed: true,
    };
    app.update();
    app.update();
    assert!(
        old.iter().all(|p| app.world.get_entity(*p).is_err()),
        "old props left"
    );
    assert_eq!(app.world.get::<Attacker>(a).unwrap().set, "greatsword");
    let params = app.world.get::<AnimationParams>(a).unwrap();
    assert_eq!(
        (params.get("sword_stance"), params.get("greatsword_stance")),
        (0.0, 1.0)
    );
    assert!(!app.world.get::<Armed>(a).unwrap().props.is_empty());
}

#[test]
fn hide_props_hides_the_meshes_and_keeps_the_grip() {
    let (mut app, a, _) = duel();
    let props = app.world.get::<Armed>(a).unwrap().props.clone();
    let shown = |app: &App| {
        props
            .iter()
            .filter(|p| {
                app.world
                    .get::<engine_render::MeshRenderer>(**p)
                    .unwrap()
                    .visible
            })
            .count()
    };
    assert_eq!(shown(&app), props.len());
    app.world
        .get_mut::<AnimationParams>(a)
        .unwrap()
        .set("hide_props", 1.0);
    app.update();
    assert_eq!(shown(&app), 0);
    assert_eq!(
        app.world.get::<Armed>(a).unwrap().props,
        props,
        "props kept"
    );
    app.world
        .get_mut::<AnimationParams>(a)
        .unwrap()
        .set("hide_props", 0.0);
    app.update();
    assert_eq!(shown(&app), props.len());
}

#[test]
fn a_feint_pulls_the_strike_back_and_skill_sets_the_telegraph() {
    let (mut app, a, _) = duel();
    // A rookie: twice as long a wind-up.
    app.world
        .get_mut::<AnimationParams>(a)
        .unwrap()
        .set("windup_speed", 0.5);
    app.world.get_mut::<MoveIntent>(a).unwrap().attack = Some(AttackKind::Melee);
    app.update();
    app.world.get_mut::<MoveIntent>(a).unwrap().attack = None;
    app.update();
    let w = app.world.get::<AttackWindow>(a).unwrap().clone();
    assert_eq!(w.phase, Some(AttackPhase::Windup));
    let slow = w.phase_length_secs;
    app.world
        .get_mut::<engine_anim::animator::Animator>(a)
        .unwrap()
        .trigger("feint");
    let mut cancelled = false;
    for _ in 0..(1.0 / STEP) as usize {
        app.update();
        cancelled |= app.world.get::<AttackWindow>(a).unwrap().phase == Some(AttackPhase::Cancel);
    }
    let attacker = app.world.get::<Attacker>(a).unwrap();
    assert!(cancelled, "no cancel phase");
    assert_eq!((attacker.feinted, attacker.hits), (1, 0));
    assert_eq!(app.world.get::<AttackWindow>(a).unwrap().phase, None);
    // A master's telegraph is shorter.
    app.world
        .get_mut::<AnimationParams>(a)
        .unwrap()
        .set("windup_speed", 1.5);
    app.world.get_mut::<MoveIntent>(a).unwrap().attack = Some(AttackKind::Melee);
    app.update();
    app.world.get_mut::<MoveIntent>(a).unwrap().attack = None;
    app.update();
    let fast = app.world.get::<AttackWindow>(a).unwrap().phase_length_secs;
    assert!(fast < slow * 0.5, "rookie {slow} s, master {fast} s");
}

#[test]
fn six_presses_at_one_knee_give_six_cuts_that_land() {
    let (mut app, a, b) = duel();
    let skilled = engine_world::combat::CombatSkills {
        weapons: [(ItemClass::OneHandedBlade, 0.9)].into_iter().collect(),
        off_hand: 0.5,
    };
    app.world.entity_mut(a).insert(skilled);
    app.world.get_mut::<StrikeTargets>(a).unwrap().zone = Some(BodyZone::KneeL);
    let mut moved = [DVec3::ZERO; 2];
    let mut thrown: Vec<String> = Vec::new();
    let mut landed = Vec::new();
    let mut pressing = false;
    for _ in 0..(8.0 / STEP) as usize {
        let at = app.world.get::<Attacker>(a).unwrap().clone();
        // Press once the last cut is over or its follow-on opens, and let go after a frame.
        let ready = at.current.is_none() || at.cancel_open;
        let intent = if thrown.len() < 6 && ready && !pressing {
            pressing = true;
            Some(AttackKind::Melee)
        } else {
            pressing = false;
            None
        };
        app.world.get_mut::<MoveIntent>(a).unwrap().attack = intent;
        app.update();
        take_steps(&mut app, [a, b], &mut moved);
        let now = app.world.get::<Attacker>(a).unwrap();
        if let Some(c) = &now.current
            && now.time < 0.1
            && thrown.last() != Some(c)
        {
            thrown.push(c.clone());
            landed.push(0);
        }
        if let Some(l) = landed.last_mut() {
            *l = (*l).max(now.hits);
        }
        if thrown.len() == 6 && now.current.is_none() {
            break;
        }
    }
    assert_eq!(thrown.len(), 6, "{thrown:?}");
    let distinct: std::collections::HashSet<_> = thrown.iter().collect();
    assert_eq!(distinct.len(), 6, "a cut came twice: {thrown:?}");
    assert!(
        thrown
            .iter()
            .all(|c| c.starts_with("sword_line_") && c.ends_with("_low")),
        "not low cuts: {thrown:?}"
    );
    assert!(
        landed.iter().all(|&h| h >= 1),
        "{thrown:?} landed {landed:?}"
    );
}

/// The animation event names fired so far.
#[derive(Resource, Default)]
struct Said(Vec<String>);

fn listen(mut said: ResMut<Said>, mut events: MessageReader<AnimationEvent>) {
    said.0.extend(events.read().map(|e| e.name.clone()));
}

/// A bone's height above the entity, from the published pose (model space).
fn bone_y(app: &App, e: Entity, bone: &str) -> f32 {
    let pose = app.world.get::<SkeletonPose>(e).unwrap();
    pose.bones[pose.index_of(bone).unwrap()].translation.y
}

#[test]
fn without_physics_a_downed_fighter_goes_down_lies_still_and_gets_up() {
    let (mut app, a, _) = duel();
    app.world.init_resource::<Said>();
    app.add_systems(Last, listen);
    app.world.entity_mut(a).insert(Ragdoll { weight: 1.0 });
    for _ in 0..(2.5 / STEP) as usize {
        app.update();
    }
    assert!(
        bone_y(&app, a, "head") < 0.4,
        "lying: {}",
        bone_y(&app, a, "head")
    );
    assert!(bone_y(&app, a, "hips") < 0.3);
    app.world.get_mut::<Ragdoll>(a).unwrap().weight = 0.0;
    let mut secs = 0.0;
    while !app.world.resource::<Said>().0.iter().any(|s| s == "got_up") && secs < 5.0 {
        app.update();
        secs += STEP;
    }
    let said = &app.world.resource::<Said>().0;
    assert!(said.iter().any(|s| s == "getting_up"), "{said:?}");
    assert!(secs < 5.0, "never got up: {said:?}");
    for _ in 0..30 {
        app.update();
    }
    assert!(
        bone_y(&app, a, "head") > 1.4,
        "standing: {}",
        bone_y(&app, a, "head")
    );
}

#[test]
fn kneel_presses_both_hands_on_a_chest_ahead() {
    let (mut app, a, _) = duel();
    app.world
        .get_mut::<AnimationParams>(a)
        .unwrap()
        .set("kneel", 1.0);
    for _ in 0..(1.5 / STEP) as usize {
        app.update();
    }
    let pose = app.world.get::<SkeletonPose>(a).unwrap();
    let at = |b: &str| pose.bones[pose.index_of(b).unwrap()].translation;
    let want = engine_anim::build::humanoid::KNEEL_HANDS;
    for hand in ["hand_l", "hand_r"] {
        let h = at(hand);
        assert!(
            (Vec2::new(h.y, h.z) - Vec2::new(want.y, want.z)).length() < 0.12,
            "{hand} at {h}, chest at {want}"
        );
    }
    assert!(at("hips").y < 0.6, "kneeling: {}", at("hips"));
    app.world
        .get_mut::<AnimationParams>(a)
        .unwrap()
        .set("kneel", 0.0);
    for _ in 0..(1.5 / STEP) as usize {
        app.update();
    }
    assert!(bone_y(&app, a, "head") > 1.4, "stood up");
}

fn send(app: &mut App, entity: Entity, name: &str) {
    app.world.write_message(AnimationTrigger {
        entity,
        name: name.into(),
    });
}

fn in_state(app: &App, e: Entity, state: &str) -> bool {
    app.world
        .get::<engine_anim::animator::Animator>(e)
        .unwrap()
        .graph()
        .is_some_and(|g| g.in_state(state))
}

#[test]
fn a_bow_draws_holds_at_full_draw_and_looses_on_cue() {
    use engine_anim::build::weapons::{BOW_HOLD, BOW_LOOSE, BOW_LOWER};
    let (mut app, a, _) = duel();
    app.world.init_resource::<Said>();
    app.add_systems(Last, listen);
    *app.world.get_mut::<Loadout>(a).unwrap() = Loadout {
        main: ItemClass::Bow,
        off: ItemClass::None,
        two_handed: true,
    };
    for _ in 0..20 {
        app.update();
    }
    let curve = |app: &App, name: &str| {
        app.world
            .get::<engine_anim::animator::Animator>(a)
            .unwrap()
            .curve(name)
    };
    send(&mut app, a, "draw");
    for _ in 0..(1.2 / STEP) as usize {
        app.update();
    }
    assert!(
        app.world.resource::<Said>().0.iter().any(|s| s == "drawn"),
        "{:?}",
        app.world.resource::<Said>().0
    );
    // Held as long as it likes: three seconds on, still at full draw, nothing loosed.
    for _ in 0..(3.0 / STEP) as usize {
        app.update();
    }
    assert!(in_state(&app, a, BOW_HOLD));
    assert!(curve(&app, "draw") > 0.99 && curve(&app, "nocked") > 0.99);
    assert!(
        !app.world
            .resource::<Said>()
            .0
            .iter()
            .any(|s| s == "release")
    );
    send(&mut app, a, "loose");
    for _ in 0..6 {
        app.update();
    }
    assert!(in_state(&app, a, BOW_LOOSE));
    assert!(
        app.world
            .resource::<Said>()
            .0
            .iter()
            .any(|s| s == "release"),
        "{:?}",
        app.world.resource::<Said>().0
    );
    assert!(curve(&app, "draw") < 0.5, "string still back");
    for _ in 0..(1.0 / STEP) as usize {
        app.update();
    }
    assert!(in_state(&app, a, "move"));
    // Drawn again and let down without a shot.
    app.world.resource_mut::<Said>().0.clear();
    send(&mut app, a, "draw");
    for _ in 0..(1.2 / STEP) as usize {
        app.update();
    }
    send(&mut app, a, "lower");
    app.update();
    app.update();
    assert!(in_state(&app, a, BOW_LOWER));
    for _ in 0..(1.0 / STEP) as usize {
        app.update();
    }
    assert!(in_state(&app, a, "move"));
    assert!(
        !app.world
            .resource::<Said>()
            .0
            .iter()
            .any(|s| s == "release")
    );
    // A sword fighter ignores the cue.
    *app.world.get_mut::<Loadout>(a).unwrap() = sword_and_shield();
    for _ in 0..30 {
        app.update();
    }
    send(&mut app, a, "draw");
    for _ in 0..30 {
        app.update();
    }
    assert!(!in_state(&app, a, engine_anim::build::weapons::BOW_DRAW));
}

#[test]
fn a_rifle_shoulders_on_aim_fires_on_cue_and_changes_its_magazine() {
    use engine_anim::build::weapons::{RIFLE_AIM, RIFLE_AXIS, RIFLE_FORE, RIFLE_RELOAD};
    let (mut app, a, _) = duel();
    app.world.init_resource::<Said>();
    app.add_systems(Last, listen);
    *app.world.get_mut::<Loadout>(a).unwrap() = Loadout {
        main: ItemClass::Firearm,
        off: ItemClass::None,
        two_handed: true,
    };
    for _ in 0..20 {
        app.update();
    }
    let said = |app: &App, what: &str| app.world.resource::<Said>().0.iter().any(|s| s == what);
    // Model space: the muzzle's way, and how far the left fist is from the fore grip.
    let hands = |app: &App| {
        let pose = app.world.get::<SkeletonPose>(a).unwrap();
        let bone = |b: &str| pose.bones[pose.index_of(b).unwrap()];
        let (r, l) = (bone("hand_r"), bone("hand_l"));
        // The fists' middles (each hand's standard fist).
        let fist = |b: engine_world::animation::BonePose| {
            b.translation + b.rotation * engine_anim::grip::GRIP
        };
        let muzzle = r.rotation * RIFLE_AXIS;
        let fore = fist(r) + muzzle * RIFLE_FORE;
        (muzzle, (fist(l) - fore).length())
    };
    // At the low ready the muzzle points down ahead.
    let (low, _) = hands(&app);
    assert!(low.y < -0.45 && low.z < -0.6, "low ready: {low}");
    app.world
        .get_mut::<AnimationParams>(a)
        .unwrap()
        .set("aim", 1.0);
    for _ in 0..(0.8 / STEP) as usize {
        app.update();
    }
    assert!(in_state(&app, a, RIFLE_AIM));
    let (muzzle, off) = hands(&app);
    assert!(muzzle.dot(Vec3::NEG_Z) > 0.995, "aimed: {muzzle}");
    assert!(off < 0.06, "left hand {off} m off the fore grip");
    // A press shoots.
    app.world.get_mut::<MoveIntent>(a).unwrap().attack = Some(AttackKind::Ranged);
    app.update();
    app.world.get_mut::<MoveIntent>(a).unwrap().attack = None;
    for _ in 0..20 {
        app.update();
    }
    assert!(said(&app, "shot"));
    send(&mut app, a, "reload");
    app.update();
    app.update();
    assert!(in_state(&app, a, RIFLE_RELOAD));
    for _ in 0..(2.5 / STEP) as usize {
        app.update();
    }
    assert!(said(&app, "mag_out") && said(&app, "mag_in"));
    assert!(in_state(&app, a, RIFLE_AIM));
    let (muzzle, off) = hands(&app);
    assert!(
        muzzle.dot(Vec3::NEG_Z) > 0.995 && off < 0.06,
        "back on aim: {muzzle}, {off}"
    );
    // Lowered, a shot snaps up to the shoulder and goes back down.
    app.world
        .get_mut::<AnimationParams>(a)
        .unwrap()
        .set("aim", 0.0);
    for _ in 0..40 {
        app.update();
    }
    app.world.resource_mut::<Said>().0.clear();
    send(&mut app, a, "fire");
    for _ in 0..40 {
        app.update();
    }
    assert!(said(&app, "shot"));
    assert!(in_state(&app, a, "move"));
}

#[test]
fn draw_speed_paces_the_bow_draw() {
    let drawn_after = |speed: Option<f32>| {
        let (mut app, a, _) = duel();
        app.world.init_resource::<Said>();
        app.add_systems(Last, listen);
        *app.world.get_mut::<Loadout>(a).unwrap() = Loadout {
            main: ItemClass::Bow,
            off: ItemClass::None,
            two_handed: true,
        };
        if let Some(s) = speed {
            app.world
                .get_mut::<AnimationParams>(a)
                .unwrap()
                .set("draw_speed", s);
        }
        for _ in 0..20 {
            app.update();
        }
        send(&mut app, a, "draw");
        let mut secs = 0.0;
        while !app.world.resource::<Said>().0.iter().any(|s| s == "drawn") && secs < 3.0 {
            app.update();
            secs += STEP;
        }
        secs
    };
    let normal = drawn_after(None);
    let quick = drawn_after(Some(2.0));
    assert!((0.7..0.9).contains(&normal), "unset: {normal}");
    assert!((0.33..0.48).contains(&quick), "draw_speed 2: {quick}");
}

#[test]
fn wounds_show_a_knee_gone_an_arm_hanging_and_a_hand_cut_off() {
    use engine_world::combat::BodyInjuries;
    let (mut app, a, _) = duel();
    let mut inj = BodyInjuries::default();
    inj.get_mut(BodyZone::KneeL).wound = 0.7;
    inj.get_mut(BodyZone::ForearmR).wound = 0.65;
    app.world.entity_mut(a).insert(inj);
    for _ in 0..(1.0 / STEP) as usize {
        app.update();
    }
    let at = |app: &App, b: &str| {
        let pose = app.world.get::<SkeletonPose>(a).unwrap();
        pose.bones[pose.index_of(b).unwrap_or_else(|| panic!("no {b}"))]
    };
    // Down on the left knee, the right foot on the floor ahead, still upright.
    assert!(
        at(&app, "hips").translation.y < 0.7,
        "hips {}",
        at(&app, "hips").translation
    );
    assert!(
        at(&app, "foot_l").translation.y < 0.2 && at(&app, "lower_leg_l").translation.y < 0.15,
        "left knee {}",
        at(&app, "lower_leg_l").translation
    );
    assert!(at(&app, "foot_r").translation.y < 0.15);
    assert!(at(&app, "head").translation.y > 1.0, "upright");
    // The right arm hangs: the hand below the elbow, the elbow below the shoulder.
    let (sh, el, ha) = (
        at(&app, "upper_arm_r").translation,
        at(&app, "lower_arm_r").translation,
        at(&app, "hand_r").translation,
    );
    assert!(
        el.y < sh.y - 0.2 && ha.y < el.y - 0.15,
        "arm {sh} {el} {ha}"
    );
    // A hand cut off at the wrist: gone (everything it carries shrinks with it).
    app.world
        .get_mut::<BodyInjuries>(a)
        .unwrap()
        .get_mut(BodyZone::HandL)
        .severed = true;
    app.update();
    app.update();
    let hand = at(&app, "hand_l");
    assert!(hand.scale.max_element() < 0.01, "hand scale {}", hand.scale);
    // Healed (say, by magic), it stands again with both hands.
    app.world.entity_mut(a).insert(BodyInjuries::default());
    for _ in 0..(1.0 / STEP) as usize {
        app.update();
    }
    assert!(at(&app, "hips").translation.y > 0.85);
    assert!(at(&app, "hand_l").scale.min_element() > 0.99);
}

/// Where blows landed, world.
#[derive(Resource, Default)]
struct Blows(Vec<(Entity, DVec3)>);

fn record_blows(mut damage: MessageReader<engine_world::DamageEvent>, mut blows: ResMut<Blows>) {
    blows.0.extend(damage.read().map(|d| (d.target, d.point)));
}

#[test]
fn an_aimed_cut_goes_where_the_rules_scattered_its_point() {
    // The same chest cut aimed at the chest's left and then its right edge lands on that side.
    let mut sides = Vec::new();
    for side in [-1.0f32, 1.0] {
        let (mut app, a, b) = duel();
        app.init_resource::<Blows>()
            .add_systems(PostUpdate, record_blows);
        {
            let mut st = app.world.get_mut::<StrikeTargets>(a).unwrap();
            st.zone = Some(BodyZone::Belly);
            st.point = Some(Vec3::new(0.16 * side, 1.05, -0.1));
        }
        app.world.get_mut::<MoveIntent>(a).unwrap().attack = Some(AttackKind::Melee);
        app.update();
        app.world.get_mut::<MoveIntent>(a).unwrap().attack = None;
        for _ in 0..(1.5 / STEP) as usize {
            app.update();
        }
        let gt = *app.world.get::<GlobalTransform>(b).unwrap();
        let blows: Vec<Vec3> = app
            .world
            .resource::<Blows>()
            .0
            .iter()
            .filter(|(t, _)| *t == b)
            .map(|(_, p)| {
                let local = gt.rotation().inverse() * (*p - gt.translation()).as_vec3();
                local / gt.scale()
            })
            .collect();
        assert!(!blows.is_empty(), "the cut at side {side} missed");
        sides.push(blows[0]);
    }
    let [left, right] = [sides[0], sides[1]];
    assert!(
        left.x < -0.04 && right.x > 0.04,
        "landed at {left:?} and {right:?} (target model space)"
    );
}

/// Contacts sent to a melee judge.
#[derive(Resource, Default)]
struct Contacts(Vec<engine_world::MeleeContact>);

fn record_contacts(
    mut contacts: MessageReader<engine_world::MeleeContact>,
    mut seen: ResMut<Contacts>,
) {
    seen.0.extend(contacts.read().cloned());
}

/// Press light strikes at `zone` until `n` have been thrown; the contacts sent.
fn aimed_presses(zone: BodyZone, loadout: Loadout, n: usize) -> Vec<engine_world::MeleeContact> {
    aimed_presses_at(zone, loadout, sword_and_shield(), n)
}

/// [`aimed_presses`] at a foe holding `foe`.
fn aimed_presses_at(
    zone: BodyZone,
    loadout: Loadout,
    foe: Loadout,
    n: usize,
) -> Vec<engine_world::MeleeContact> {
    aimed_presses_with(zone, loadout, foe, n, false)
}

/// [`aimed_presses_at`], pressing heavy strikes when `heavy`.
fn aimed_presses_with(
    zone: BodyZone,
    loadout: Loadout,
    foe: Loadout,
    n: usize,
    heavy: bool,
) -> Vec<engine_world::MeleeContact> {
    let (mut app, a, b) = duel();
    app.world.entity_mut(b).insert(foe);
    app.world.insert_resource(engine_world::MeleeJudge {
        judged_by: "test".into(),
    });
    app.init_resource::<Contacts>()
        .add_systems(PostUpdate, record_contacts);
    app.world.entity_mut(a).insert(loadout);
    for _ in 0..20 {
        app.update();
    }
    app.world.get_mut::<StrikeTargets>(a).unwrap().zone = Some(zone);
    let mut moved = [DVec3::ZERO; 2];
    if heavy {
        // One heavy blow at a time, as the yard throws them.
        for _ in 0..n {
            app.world.get_mut::<MoveIntent>(a).unwrap().attack = Some(AttackKind::Dash);
            app.update();
            app.world.get_mut::<MoveIntent>(a).unwrap().attack = None;
            for _ in 0..(2.5 / STEP) as usize {
                app.update();
                take_steps(&mut app, [a, b], &mut moved);
            }
        }
        return app.world.resource::<Contacts>().0.clone();
    }
    let (mut thrown, mut pressing) = (0, false);
    let mut last: Option<String> = None;
    for _ in 0..(12.0 / STEP) as usize {
        let at = app.world.get::<Attacker>(a).unwrap().clone();
        let ready = at.current.is_none() || at.cancel_open;
        let intent = if thrown < n && ready && !pressing {
            pressing = true;
            Some(AttackKind::Melee)
        } else {
            pressing = false;
            None
        };
        app.world.get_mut::<MoveIntent>(a).unwrap().attack = intent;
        app.update();
        take_steps(&mut app, [a, b], &mut moved);
        let now = app.world.get::<Attacker>(a).unwrap();
        if now.current.is_some() && now.current != last {
            thrown += 1;
            last = now.current.clone();
        }
        if now.current.is_none() {
            last = None;
            if thrown >= n {
                break;
            }
        }
    }
    app.world.resource::<Contacts>().0.clone()
}

#[test]
fn cuts_aimed_at_a_forearm_or_a_knee_go_in_there() {
    // A raised forearm sits by the face, a knee between thigh and shin: the blade goes in
    // at the part it was aimed at when it crosses it.
    let greatsword = Loadout {
        main: ItemClass::TwoHandedBlade,
        off: ItemClass::None,
        two_handed: true,
    };
    // (zone, loadout, the bones it goes in at, share of cuts that must)
    for (zone, loadout, bones, share) in [
        (
            BodyZone::ForearmR,
            greatsword,
            &["lower_arm_r", "hand_r"][..],
            0.75,
        ),
        // A cut down at the knee lands the frame it comes in reach, sometimes on the thigh
        // just above: always on that leg.
        (
            BodyZone::KneeR,
            greatsword,
            &["lower_leg_r", "upper_leg_r"][..],
            1.0,
        ),
        (
            BodyZone::ForearmR,
            sword_and_shield(),
            &["lower_arm_r", "hand_r"][..],
            0.75,
        ),
    ] {
        let contacts = aimed_presses(zone, loadout, 4);
        let entered: Vec<Option<String>> = contacts.iter().map(|c| c.bone.clone()).collect();
        let on = entered
            .iter()
            .filter(|b| b.as_deref().is_some_and(|b| bones.contains(&b)))
            .count();
        assert!(
            contacts.len() >= 3 && on as f32 >= contacts.len() as f32 * share,
            "{zone:?} with {:?}: went in at {entered:?}",
            loadout.main
        );
    }
}

#[test]
fn moving_on_a_ruined_knee_drags_along_on_it() {
    use engine_world::combat::BodyInjuries;
    let (mut app, a, _) = duel();
    let mut inj = BodyInjuries::default();
    inj.get_mut(BodyZone::KneeL).wound = 0.7;
    app.world.entity_mut(a).insert(inj);
    for _ in 0..(1.0 / STEP) as usize {
        app.update();
    }
    let at = |app: &App, b: &str| {
        let pose = app.world.get::<SkeletonPose>(a).unwrap();
        pose.bones[pose.index_of(b).unwrap()].translation
    };
    // Still: the good foot stays put ahead.
    let still = at(&app, "foot_r");
    for _ in 0..30 {
        app.update();
    }
    assert!(
        at(&app, "foot_r").distance(still) < 0.03,
        "the foot wandered kneeling still"
    );
    // Moving on at 0.3 m/s (the way it faces, −Z): the good foot plants ahead, slides back
    // under the body and steps on again, the knee stays down and the body low.
    let (mut near, mut far, mut knee_high, mut hips_high) = (f32::MAX, f32::MIN, 0.0f32, 0.0f32);
    for _ in 0..(2.4 / STEP) as usize {
        let mut t = app.world.get_mut::<Transform>(a).unwrap();
        let ahead = t.rotation * Vec3::NEG_Z;
        t.translation += (ahead * 0.3 * STEP as f32).as_dvec3();
        app.update();
        let foot = at(&app, "foot_r");
        (near, far) = (near.min(foot.z), far.max(foot.z));
        knee_high = knee_high.max(at(&app, "lower_leg_l").y);
        hips_high = hips_high.max(at(&app, "hips").y);
    }
    assert!(
        far - near > 0.15,
        "the good foot only moved {:.2} m",
        far - near
    );
    assert!(knee_high < 0.25, "the knee came up {knee_high:.2} m");
    assert!(hips_high < 0.75, "the hips rose to {hips_high:.2} m");
}

/// Wound `zones` on the near fighter, settle a second, then move it ahead at `speed` for
/// `secs`; each frame's pose (model space) after the settle.
fn wounded_moving(
    zones: &[BodyZone],
    speed: f32,
    secs: f64,
) -> Vec<std::collections::HashMap<String, Vec3>> {
    use engine_world::combat::BodyInjuries;
    let (mut app, a, b) = duel();
    // The other one out of the way.
    app.world.get_mut::<Transform>(b).unwrap().translation.z = -40.0;
    app.world.get_mut::<StrikeTargets>(a).unwrap().target = None;
    let mut inj = BodyInjuries::default();
    for z in zones {
        inj.get_mut(*z).wound = 0.7;
    }
    app.world.entity_mut(a).insert(inj);
    for _ in 0..(1.0 / STEP) as usize {
        app.update();
    }
    let mut frames = Vec::new();
    for _ in 0..(secs / STEP) as usize {
        let mut t = app.world.get_mut::<Transform>(a).unwrap();
        let ahead = t.rotation * Vec3::NEG_Z;
        t.translation += (ahead * speed * STEP as f32).as_dvec3();
        app.update();
        let pose = app.world.get::<SkeletonPose>(a).unwrap();
        frames.push(
            [
                "hips",
                "head",
                "foot_l",
                "foot_r",
                "lower_leg_l",
                "lower_leg_r",
            ]
            .iter()
            .map(|b| {
                (
                    b.to_string(),
                    pose.bones[pose.index_of(b).unwrap()].translation,
                )
            })
            .collect(),
        );
    }
    frames
}

#[test]
fn with_both_legs_out_it_kneels_on_both_and_walks_on_its_knees() {
    let frames = wounded_moving(&[BodyZone::KneeL, BodyZone::KneeR], 0.3, 2.8);
    let most =
        |b: &str, f: fn(&Vec3) -> f32| frames.iter().map(|p| f(&p[b])).fold(f32::MIN, f32::max);
    let least =
        |b: &str, f: fn(&Vec3) -> f32| frames.iter().map(|p| f(&p[b])).fold(f32::MAX, f32::min);
    for knee in ["lower_leg_l", "lower_leg_r"] {
        assert!(
            most(knee, |v| v.y) < 0.2,
            "{knee} came up to {}",
            most(knee, |v| v.y)
        );
    }
    assert!(most("hips", |v| v.y) < 0.75);
    assert!(least("head", |v| v.y) > 1.0, "upright on its knees");
    // Each knee steps: its foot goes back and comes on again.
    for foot in ["foot_l", "foot_r"] {
        let span = most(foot, |v| v.z) - least(foot, |v| v.z);
        assert!(span > 0.1, "{foot} moved {span:.2} m");
    }
}

#[test]
fn a_limping_body_sinks_and_leans_over_its_hurt_leg() {
    let mean_x = |frames: &[std::collections::HashMap<String, Vec3>]| {
        frames.iter().map(|p| p["head"].x).sum::<f32>() / frames.len() as f32
    };
    let sound = wounded_moving(&[], 1.2, 3.0);
    let hurt = wounded_moving(&[BodyZone::ThighR], 1.2, 3.0);
    // Its head leans out over the right (+X) leg as it walks, more than walking sound.
    let lean = mean_x(&hurt) - mean_x(&sound);
    assert!(lean > 0.01, "leaned {lean:.3} m toward the hurt leg");
    // And it still walks upright on both feet.
    assert!(hurt.iter().all(|p| p["hips"].y > 0.75));
}

#[test]
fn a_great_sword_cuts_at_a_beginners_sword_arm_go_in_there() {
    // The combat yard's sever pair: light cuts at the sword arm of a fighter without a
    // shield go in at that arm, not the head. (Its heavy overhead keeps its own line and comes
    // down on the head first.)
    let greatsword = Loadout {
        main: ItemClass::TwoHandedBlade,
        off: ItemClass::None,
        two_handed: true,
    };
    let sword = Loadout {
        main: ItemClass::OneHandedBlade,
        off: ItemClass::None,
        two_handed: false,
    };
    let c = aimed_presses_at(BodyZone::ForearmR, greatsword, sword, 6);
    let bones: Vec<_> = c.iter().map(|c| c.bone.clone()).collect();
    let on = bones
        .iter()
        .filter(|b| matches!(b.as_deref(), Some("lower_arm_r" | "hand_r")))
        .count();
    assert!(
        c.len() >= 4 && on * 4 >= c.len() * 3,
        "went in at {bones:?}"
    );
    // Heavies still land, wherever their line takes them.
    let heavy = aimed_presses_with(BodyZone::ForearmR, greatsword, sword, 3, true);
    assert_eq!(heavy.len(), 3, "{heavy:?}");
}
