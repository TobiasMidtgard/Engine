//! Wild animals on request (characters week A4): the NPC module asks for a deer, a wolf, a
//! boar or a rabbit with a `BodyRequest` and moves it itself; it tells the body what the
//! animal does with the `graze`, `lie`, `alert` and `downed` parameters and the `flinch`,
//! `rear` and `howl` triggers.

use engine_anim::actions::AnimalActions;
use engine_anim::build::wild::{self, Wild};
use engine_anim::{AnimationPlugin, Animator, AnimatorSettings, Body, BodyKind, LeggedGait};
use engine_core::prelude::*;
use engine_core::time::TimeSource;
use engine_world::WorldPlugin;
use engine_world::animation::{AnimationParams, AnimationTrigger, BodyRequest, SkeletonPose};

const STEP: f64 = 1.0 / 60.0;

fn app() -> App {
    let mut app = App::new();
    app.world.resource_mut::<EngineConfig>().headless = true;
    app.world
        .resource_mut::<Time>()
        .set_source(TimeSource::Manual { step_secs: STEP });
    app.add_plugins((WorldPlugin, AnimationPlugin));
    app
}

fn animal(app: &mut App, request: BodyRequest, x: f64) -> Entity {
    let t = Transform::from_translation(DVec3::new(x, 0.0, 0.0));
    app.world
        .spawn((
            t,
            GlobalTransform::from(t),
            request,
            AnimationParams::default(),
        ))
        .id()
}

fn run(app: &mut App, secs: f64) {
    for _ in 0..(secs / STEP) as usize {
        app.update();
    }
}

fn walk(app: &mut App, e: Entity, speed: f64, secs: f64) {
    for _ in 0..(secs / STEP) as usize {
        app.world.get_mut::<Transform>(e).unwrap().translation.z -= speed * STEP;
        app.update();
    }
}

fn set(app: &mut App, e: Entity, name: &str, value: f32) {
    app.world
        .get_mut::<AnimationParams>(e)
        .unwrap()
        .set(name, value);
}

fn playing(app: &App, e: Entity) -> Option<String> {
    app.world
        .get::<AnimalActions>(e)
        .and_then(|a| a.playing.as_ref().map(|p| p.action.clone()))
}

fn bone_y(app: &App, e: Entity, bone: &str) -> f32 {
    let p = app.world.get::<SkeletonPose>(e).expect("a pose");
    p.bones[p.index_of(bone).expect(bone)].translation.y
}

#[test]
fn deer_wolves_boars_and_rabbits_get_bodies_of_their_size_and_move_on_their_gaits() {
    let mut app = app();
    let asked = [
        ("deer", None, "deer"),
        ("deer", Some(0.6), "deer@60cm"),
        ("wolf", None, "wolf"),
        ("boar", Some(0.94), "boar@94cm"),
        ("rabbit", None, "rabbit"),
    ];
    let animals: Vec<Entity> = asked
        .iter()
        .enumerate()
        .map(|(i, (kind, height, _))| {
            let mut r = BodyRequest::new(*kind).with_seed(i as u64);
            r.height = *height;
            animal(&mut app, r, 3.0 * i as f64)
        })
        .collect();
    run(&mut app, 0.2);
    for (&e, (kind, _, skeleton)) in animals.iter().zip(asked) {
        let w = &app.world;
        let want = Wild::of_kind(kind).unwrap();
        assert_eq!(w.get::<Body>(e).map(|b| b.kind), Some(BodyKind::Wild(want)));
        assert_eq!(
            w.get::<AnimatorSettings>(e).map(|s| s.skeleton.as_str()),
            Some(skeleton)
        );
        assert!(w.get::<SkeletonPose>(e).is_some(), "{kind}: a pose");
        assert!(w.get::<AnimalActions>(e).is_some(), "{kind}: actions");
        let fur = w
            .get::<engine_anim::SkinnedMeshRenderer>(e)
            .map_or(0.0, |r| r.material.fur_length);
        assert!(fur > 0.003, "{kind}: a furred coat ({fur} m)");
    }
    // A wolf trots, then gallops; a rabbit bounds.
    let wolf = animals[2];
    walk(&mut app, wolf, 2.6, 2.0);
    assert_eq!(
        app.world.get::<LeggedGait>(wolf).unwrap().stage_name(),
        "trot"
    );
    walk(&mut app, wolf, 10.0, 2.0);
    assert_eq!(
        app.world.get::<LeggedGait>(wolf).unwrap().stage_name(),
        "gallop"
    );
    let rabbit = animals[4];
    walk(&mut app, rabbit, 4.0, 2.0);
    assert_eq!(
        app.world.get::<LeggedGait>(rabbit).unwrap().stage_name(),
        "bound"
    );
}

#[test]
fn a_grazing_deer_holds_its_head_down_until_it_stops_then_lies_and_is_downed() {
    let mut app = app();
    let deer = animal(&mut app, BodyRequest::new("deer").with_seed(2), 0.0);
    run(&mut app, 0.5);
    let standing = bone_y(&app, deer, "head");
    set(&mut app, deer, "graze", 1.0);
    run(&mut app, 6.0);
    assert_eq!(playing(&app, deer).as_deref(), Some("graze"));
    let grazing = bone_y(&app, deer, "head");
    assert!(
        grazing < standing - 0.6,
        "head from {standing} to {grazing}"
    );
    // The head stays down as long as it grazes.
    run(&mut app, 4.0);
    assert!(bone_y(&app, deer, "head") < standing - 0.6);
    // Done grazing: the head comes up and the action ends.
    set(&mut app, deer, "graze", 0.0);
    run(&mut app, 3.0);
    assert_eq!(playing(&app, deer), None);
    assert!(bone_y(&app, deer, "head") > standing - 0.05);

    set(&mut app, deer, "lie", 1.0);
    run(&mut app, 4.0);
    assert_eq!(playing(&app, deer).as_deref(), Some("lie"));
    assert!(bone_y(&app, deer, "body") < 0.5, "lying");
    // Killed while lying: it falls on its side at once and stays there.
    set(&mut app, deer, "downed", 1.0);
    run(&mut app, 0.1);
    assert_eq!(playing(&app, deer).as_deref(), Some("fall"));
    run(&mut app, 5.0);
    assert_eq!(playing(&app, deer).as_deref(), Some("fall"));
    let body = bone_y(&app, deer, "body");
    assert!(body < 0.3, "down on its side: body at {body}");
    // A flinch doesn't stir a dead animal.
    app.world.write_message(AnimationTrigger {
        entity: deer,
        name: "flinch".into(),
    });
    run(&mut app, 0.2);
    assert_eq!(playing(&app, deer).as_deref(), Some("fall"));
}

#[test]
fn a_bolting_deer_rears_a_howling_wolf_sits_and_a_hurt_boar_flinches_on_the_run() {
    let mut app = app();
    let deer = animal(&mut app, BodyRequest::new("deer").with_seed(3), 0.0);
    let wolf = animal(&mut app, BodyRequest::new("wolf").with_seed(4), 4.0);
    let boar = animal(&mut app, BodyRequest::new("boar").with_seed(5), 8.0);
    run(&mut app, 0.5);
    for (e, name) in [(deer, "rear"), (wolf, "howl"), (boar, "flinch")] {
        app.world.write_message(AnimationTrigger {
            entity: e,
            name: name.into(),
        });
    }
    // The deer bolts as it rears, the boar runs on as it flinches.
    for _ in 0..12 {
        for (e, speed) in [(deer, 3.0), (boar, 4.0)] {
            app.world.get_mut::<Transform>(e).unwrap().translation.z -= speed * STEP;
        }
        app.update();
    }
    assert_eq!(playing(&app, deer).as_deref(), Some("rear"));
    assert_eq!(playing(&app, boar).as_deref(), Some("flinch"));
    run(&mut app, 2.0);
    assert_eq!(playing(&app, wolf).as_deref(), Some("howl"));
    let head = app.world.get::<SkeletonPose>(wolf).unwrap();
    let up = head.bones[head.index_of("head").unwrap()].rotation * Vec3::NEG_Z;
    assert!(up.y > 0.4, "the wolf's muzzle points {up}");
    // A rabbit sits up when alert.
    let rabbit = animal(&mut app, BodyRequest::new("rabbit").with_seed(6), 12.0);
    run(&mut app, 0.5);
    let crouched = bone_y(&app, rabbit, "head");
    set(&mut app, rabbit, "alert", 1.0);
    run(&mut app, 2.0);
    assert!(bone_y(&app, rabbit, "head") > crouched + 0.05);
    let _ = (wild::ALL, app.world.get::<Animator>(rabbit).is_some());
}
