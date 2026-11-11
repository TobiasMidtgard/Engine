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
    AnimationEvent, AnimationParams, BodyRequest, Ragdoll, SkeletonPose, StrikeTargets,
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
