//! People a character module draws: while `AppearanceBodies` exists, a person with an
//! `Appearance` keeps the humanoid `SkeletonPose` but hides this module's mesh, and its face
//! writes `FaceControls` (ARKit weights and gaze) that blink and look where it looks. Without
//! a character module nothing changes. Runs with or without the fetched models.

use engine_anim::AnimationPlugin;
use engine_anim::face::Face;
use engine_anim::skin::SkinnedMeshRenderer;
use engine_core::prelude::*;
use engine_core::time::TimeSource;
use engine_world::WorldPlugin;
use engine_world::animation::{BodyRequest, SkeletonPose};
use engine_world::character::{Appearance, FaceControls, arkit, draws_appearances};

const STEP: f64 = 1.0 / 60.0;

fn app(drawn: bool) -> App {
    let mut app = App::new();
    app.world.resource_mut::<EngineConfig>().headless = true;
    app.world
        .resource_mut::<Time>()
        .set_source(TimeSource::Manual { step_secs: STEP });
    app.add_plugins((WorldPlugin, AnimationPlugin));
    if drawn {
        draws_appearances(&mut app, "test");
    }
    app
}

fn person(app: &mut App) -> Entity {
    let t = Transform::default();
    let e = app
        .world
        .spawn((
            t,
            GlobalTransform::from(t),
            BodyRequest::human(7),
            Appearance {
                seed: 7,
                ..Default::default()
            },
        ))
        .id();
    app.update();
    engine_anim::character::finish_loading(&mut app.world);
    e
}

#[test]
fn a_drawn_person_is_posed_and_faced_but_not_drawn() {
    let mut app = app(true);
    let e = person(&mut app);
    app.update();
    // Something to watch with the eyes, off to the person's left (it faces −Z; its left is
    // −X) and level with them.
    app.world.get_mut::<Face>(e).expect("a face").look_at = Some(DVec3::new(-2.0, 1.65, -4.0));
    let (mut blinked, mut gaze) = (false, 0.0f32);
    for _ in 0..(8.0 / STEP) as usize {
        app.update();
        if let Some(face) = app.world.get::<FaceControls>(e) {
            blinked |= face
                .weights
                .get(arkit::EYE_BLINK_LEFT)
                .copied()
                .unwrap_or(0.0)
                > 0.8;
            gaze = face.gaze_yaw;
        }
    }
    let face = app.world.get::<FaceControls>(e).expect("face controls");
    assert_eq!(face.weights.len(), 52);
    assert!(blinked, "never blinked in 8 s");
    assert!(
        gaze > 0.1,
        "eyes turned {gaze} rad toward a target on the left"
    );
    let pose = app.world.get::<SkeletonPose>(e).unwrap();
    assert!(pose.index_of("head").is_some() && pose.index_of("foot_l").is_some());
    let renderer = app.world.get::<SkinnedMeshRenderer>(e).unwrap();
    assert!(
        !renderer.visible,
        "drew its own mesh under the character module's"
    );
}

#[test]
fn without_a_character_module_the_person_is_drawn_as_before() {
    let mut app = app(false);
    let e = person(&mut app);
    for _ in 0..30 {
        app.update();
    }
    assert!(app.world.get::<SkinnedMeshRenderer>(e).unwrap().visible);
}

#[test]
fn paddock_spectators_watch_the_hammer_and_wince() {
    let mut app = app(true);
    let demo = engine_anim::demo::spawn_demo_world(&mut app.world, DVec3::ZERO);
    app.update();
    // The character module would fill in each look from its request; stand in for it.
    for (n, &e) in demo.spectators.iter().enumerate() {
        app.world.entity_mut(e).insert(Appearance {
            seed: 401 + n as u64,
            ..Default::default()
        });
    }
    engine_anim::character::finish_loading(&mut app.world);
    let (mut blinked, mut squeeze, mut calm, mut gaze_down) = ([false; 3], 0.0f32, 1.0f32, 0.0f32);
    for frame in 1..=(9.0 / STEP) as usize {
        app.update();
        let t = frame as f64 * STEP;
        for (n, &e) in demo.spectators.iter().enumerate() {
            let Some(face) = app.world.get::<FaceControls>(e) else {
                continue;
            };
            let w = |i: usize| face.weights.get(i).copied().unwrap_or(0.0);
            blinked[n] |= w(arkit::EYE_BLINK_LEFT) > 0.8;
            // The first strike lands at 1.56 s; each face squeezes just after it.
            let brow = w(arkit::BROW_DOWN_LEFT).max(w(arkit::EYE_SQUINT_LEFT));
            if (1.6..2.0).contains(&t) {
                squeeze = squeeze.max(brow);
            }
            if (4.0..6.0).contains(&t) {
                calm = calm.min(1.0 - brow);
            }
            // The first watches the tire, below and ahead.
            if n == 0 && t > 3.0 {
                gaze_down = gaze_down.min(face.gaze_pitch);
            }
        }
    }
    assert!(blinked.iter().all(|&b| b), "blinks {blinked:?}");
    assert!(squeeze > 0.4, "the wince squeezed the faces to {squeeze}");
    assert!(
        calm > 0.7,
        "faces stayed screwed up between strikes ({calm})"
    );
    assert!(
        gaze_down < -0.05,
        "the watcher's eyes didn't drop to the tire ({gaze_down})"
    );
}

/// A drawn person's own proportions (`BodyFit`, written by the character module): the logic
/// skeleton is fitted to them, so feet and hands land where the drawn body's are.
#[test]
fn a_person_with_a_body_fit_gets_a_skeleton_of_their_proportions() {
    use engine_world::animation::BonePose;
    use engine_world::character::BodyFit;
    let mut app = app(true);
    let e = person(&mut app);
    app.update();
    let stock = app
        .world
        .get::<engine_anim::Animator>(e)
        .unwrap()
        .skeleton
        .clone();
    // The stock rest, with legs and forearms 10% longer (and everything above the knees
    // raised to match).
    let mut fit = BodyFit {
        names: stock.names.to_vec(),
        parents: stock.parents.to_vec(),
        rest: stock
            .rest_model
            .iter()
            .map(|x| BonePose {
                translation: x.t,
                rotation: x.r,
                scale: Vec3::ONE,
            })
            .collect(),
        height: 1.75,
    };
    let bone = |n: &str| stock.index_of(n).unwrap();
    let shin = stock.rest_model[bone("lower_leg_l")].t.y - stock.rest_model[bone("foot_l")].t.y;
    for (i, name) in stock.names.iter().enumerate() {
        let is_foot = name.starts_with("foot") || name.starts_with("toe");
        if !is_foot {
            fit.rest[i].translation.y += 0.1 * shin;
        }
        if name.starts_with("hand") {
            fit.rest[i].translation.y -= 0.025;
        }
    }
    let forearm = fit.rest[bone("lower_arm_r")]
        .translation
        .distance(fit.rest[bone("hand_r")].translation);
    app.world.entity_mut(e).insert(fit);
    for _ in 0..3 {
        app.update();
    }
    let a = app.world.get::<engine_anim::Animator>(e).unwrap();
    assert!(
        a.skeleton.name.starts_with(engine_anim::body::FIT_PREFIX),
        "{}",
        a.skeleton.name
    );
    let s = &a.skeleton;
    let len = |p: &str, c: &str| {
        s.rest_model[s.index_of(p).unwrap()]
            .t
            .distance(s.rest_model[s.index_of(c).unwrap()].t)
    };
    let stock_len = |p: &str, c: &str| {
        stock.rest_model[bone(p)]
            .t
            .distance(stock.rest_model[bone(c)].t)
    };
    assert!(
        (len("lower_leg_l", "foot_l") - 1.1 * stock_len("lower_leg_l", "foot_l")).abs() < 0.002
    );
    assert!((len("lower_arm_r", "hand_r") - forearm).abs() < 0.002);
    assert!(forearm > stock_len("lower_arm_r", "hand_r") + 0.015);
    // Standing, the feet still reach the ground.
    for _ in 0..30 {
        app.update();
    }
    let a = app.world.get::<engine_anim::Animator>(e).unwrap();
    let foot = a.model[a.skeleton.index_of("foot_l").unwrap()].t;
    assert!(foot.y < 0.15, "{foot}");
}

/// What each hand holds picks its shape from the hand pose library, and an empty hand hangs
/// relaxed (the natural finger cascade).
#[test]
fn hands_take_their_shapes_from_what_they_hold() {
    use engine_anim::grip::{Handle, HeldItems};
    use engine_world::character::{HandControls, HandShape};
    let mut app = app(true);
    let e = person(&mut app);
    app.update();
    let mut held = HeldItems::default().hold(1, Handle::default());
    held.hands[0] = Some(Handle::default());
    held.presets[0] = "shield".into();
    app.world.entity_mut(e).insert(held);
    app.update();
    app.update();
    let h = *app.world.get::<HandControls>(e).unwrap();
    assert_eq!(h.hands[0].shape, HandShape::ShieldStrap);
    assert_eq!(h.hands[1].shape, HandShape::PowerGrip);
    assert!((h.hands[1].handle_radius_m - Handle::default().radius).abs() < 1e-6);
    app.world.entity_mut(e).remove::<HeldItems>();
    app.update();
    app.update();
    let h = *app.world.get::<HandControls>(e).unwrap();
    assert_eq!(h.hands[1].shape, HandShape::Relaxed, "{h:?}");
}
