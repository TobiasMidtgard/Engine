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
