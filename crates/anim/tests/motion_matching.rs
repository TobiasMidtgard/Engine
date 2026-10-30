//! Motion matching on clips that keep their root motion: the database, the search choosing
//! the move that fits where the character is going, spring blending hiding the jumps, and the
//! graph node handing over to its fallback for sideways motion. Built from the humanoid's own
//! clips given root motion (no fetched files).

use std::sync::Arc;

use engine_anim::build::humanoid::{self, Locomotion, RUN_SPEED, WALK_SPEED};
use engine_anim::clip::AnimationClip;
use engine_anim::graph::{EvalCtx, GraphInstance};
use engine_anim::motion::{MotionDatabase, MotionPlayer};
use engine_anim::skeleton::Skeleton;
use engine_anim::xform::Xf;
use engine_core::math::{Quat, Vec3};
use engine_world::animation::AnimationParams;

const DT: f32 = 1.0 / 60.0;

/// A copy of the humanoid's clip `name` moving forward (−Z) at `speed` and turning left by
/// `turn` radians over its length (once through, unless `looping`).
fn moving(
    clips: &[AnimationClip],
    name: &str,
    speed: f32,
    turn: f32,
    looping: bool,
) -> AnimationClip {
    let mut clip = clips.iter().find(|c| c.name == name).unwrap().clone();
    clip.name = format!("{name}_moving");
    clip.looping = looping;
    if !looping {
        // Once through 1.5 s, the hips keyed every frame.
        let s = humanoid::skeleton();
        let binding = engine_anim::clip::ClipBinding::new(&clip, &s);
        let hips = s.index_of("hips").unwrap();
        let times: Vec<f32> = (0..=45).map(|f| f as f32 / 30.0).collect();
        let poses: Vec<Xf> = times
            .iter()
            .map(|&t| {
                let mut local = s.rest.clone();
                clip.sample(t, &binding, &mut local);
                local[hips]
            })
            .collect();
        clip.duration = 1.5;
        let track = clip.tracks.iter_mut().find(|t| t.bone == "hips").unwrap();
        track.rotation.times = times.clone();
        track.rotation.values = poses.iter().map(|x| x.r).collect();
        track.translation.times = times;
        track.translation.values = poses.iter().map(|x| x.t).collect();
    }
    let d = clip.duration.max(1e-4);
    for track in clip.tracks.iter_mut().filter(|t| t.bone == "hips") {
        let times = track.rotation.times.clone();
        for (t, r) in times.iter().zip(&mut track.rotation.values) {
            *r = (Quat::from_rotation_y(turn * t / d) * *r).normalize();
        }
        let times = track.translation.times.clone();
        for (t, p) in times.iter().zip(&mut track.translation.values) {
            // Travel along the turning heading.
            let steps = 16;
            let mut at = Vec3::ZERO;
            for k in 0..steps {
                let s = t * (k as f32 + 0.5) / steps as f32;
                at += Quat::from_rotation_y(turn * s / d) * Vec3::NEG_Z * speed * t / steps as f32;
            }
            *p = Quat::from_rotation_y(turn * t / d) * *p + at;
        }
    }
    clip
}

fn setup() -> (Skeleton, Vec<AnimationClip>, MotionDatabase) {
    let s = humanoid::skeleton();
    let clips = humanoid::clips(&s);
    let idle = clips.iter().find(|c| c.name == "idle").unwrap().clone();
    let moves = vec![
        idle,
        moving(&clips, "walk", WALK_SPEED, 0.0, true),
        moving(&clips, "run", RUN_SPEED, 0.0, true),
        // Stepping round on the spot: the idle turning a quarter left in 1.5 s.
        {
            let mut turn = moving(&clips, "idle", 0.0, std::f32::consts::FRAC_PI_2, false);
            turn.name = "turn_left".into();
            turn
        },
    ];
    let db = MotionDatabase::build("test/motion", &s, &moves).unwrap();
    (s, clips, db)
}

/// Sideways at walking pace, with orientation warping up to 60°.
fn warped_strafe() -> AnimationParams {
    let mut p = params(WALK_SPEED, 0.0, std::f32::consts::FRAC_PI_2);
    p.set("warp_max", 60f32.to_radians());
    p
}

fn params(speed: f32, turn: f32, heading: f32) -> AnimationParams {
    let mut p = AnimationParams::default();
    p.set("speed", speed);
    p.set("turn", turn);
    p.set("heading", heading);
    p
}

#[test]
fn the_database_holds_every_move_and_its_mirror() {
    let (_, _, db) = setup();
    assert_eq!(db.clip_names.len(), 8);
    assert!(db.clip_names.iter().any(|n| n == "walk_moving (mirrored)"));
    let summary = db.summary();
    let line = |name: &str| {
        summary
            .iter()
            .find(|l| l.starts_with(&format!("{name} ")))
            .unwrap()
            .clone()
    };
    // Paces and turns come out of the root it takes out of the hips.
    let pace = |l: &str| -> f32 {
        l.split("pace")
            .nth(1)
            .unwrap()
            .split_whitespace()
            .next()
            .unwrap()
            .parse()
            .unwrap()
    };
    let turns = |l: &str| -> f32 {
        l.split("turns")
            .nth(1)
            .unwrap()
            .trim()
            .trim_end_matches('°')
            .parse()
            .unwrap()
    };
    assert!(
        (pace(&line("walk_moving")) - WALK_SPEED).abs() < 0.15,
        "{}",
        line("walk_moving")
    );
    assert!(
        (pace(&line("run_moving")) - RUN_SPEED).abs() < 0.3,
        "{}",
        line("run_moving")
    );
    assert!(pace(&line("idle")) < 0.1);
    let left = turns(&line("turn_left"));
    let right = turns(&line("turn_left (mirrored)"));
    assert!(left > 60.0 && right < -60.0, "turns {left} and {right}");
}

/// Play `secs` at these parameters; the move playing at the end and the largest change of
/// any bone's rotation between frames (radians).
fn play(
    player: &mut MotionPlayer,
    db: &MotionDatabase,
    p: &AnimationParams,
    secs: f32,
) -> (String, f32) {
    let mut pose: Vec<Xf> = vec![Xf::IDENTITY; db.bone_count()];
    let mut last: Option<Vec<Xf>> = None;
    let mut events = Vec::new();
    let mut worst = 0.0f32;
    for _ in 0..(secs / DT) as usize {
        player.update(p, DT, &mut pose, &mut events);
        if let Some(l) = &last {
            for (a, b) in l.iter().zip(&pose) {
                worst = worst.max(a.r.angle_between(b.r));
            }
        }
        last = Some(pose.clone());
    }
    (db.clip_of(player.frame().unwrap()).to_string(), worst)
}

#[test]
fn the_search_plays_the_move_that_fits_and_blends_between_them() {
    let (_, _, db) = setup();
    let db = Arc::new(db);
    let s = humanoid::skeleton();
    let mut player = MotionPlayer::new(db.clone(), &s).unwrap();
    let mut worst = 0.0f32;
    let mut expect = |p: AnimationParams, want: &str| {
        let (clip, w) = play(&mut player, &db, &p, 1.5);
        assert!(clip.starts_with(want), "wanted {want}, playing {clip}");
        worst = worst.max(w);
    };
    expect(params(0.0, 0.0, 0.0), "idle");
    expect(params(WALK_SPEED, 0.0, 0.0), "walk_moving");
    expect(params(RUN_SPEED, 0.0, 0.0), "run_moving");
    expect(params(WALK_SPEED, 0.0, 0.0), "walk_moving");
    expect(params(0.0, 0.0, 0.0), "idle");
    // Turning on the spot, left and then right (the mirrored turn).
    let (clip, w) = play(&mut player, &db, &params(0.0, 1.0, 0.0), 0.5);
    assert_eq!(clip, "turn_left");
    worst = worst.max(w);
    play(&mut player, &db, &params(0.0, 0.0, 0.0), 1.5);
    let (clip, w) = play(&mut player, &db, &params(0.0, -1.0, 0.0), 0.5);
    assert_eq!(clip, "turn_left (mirrored)");
    worst = worst.max(w);
    // Every jump between moves was blended: no bone turned more than this in a frame.
    assert!(worst < 0.3, "a bone turned {worst} rad in one frame");
}

#[test]
fn the_graph_node_hands_sideways_motion_to_its_fallback() {
    let (s, clips, db) = setup();
    let db = Arc::new(db);
    let clips: Vec<Arc<AnimationClip>> = clips.into_iter().map(Arc::new).collect();
    let graph = humanoid::graph_with(
        "test",
        &Locomotion {
            motion: "test/motion".into(),
            ..Default::default()
        },
    );
    let lookup = |n: &str| clips.iter().find(|c| c.name == n).cloned();
    let mut g = GraphInstance::compile_with(&graph, &s, &lookup, &|n| {
        (n == "test/motion").then(|| db.clone())
    })
    .unwrap();
    let mut run = |p: &AnimationParams, secs: f32| -> (f32, usize, f32) {
        let mut triggers = Vec::new();
        let mut events = Vec::new();
        let mut curves = Vec::new();
        let mut out = Vec::new();
        for _ in 0..(secs / DT) as usize {
            curves.clear();
            let mut ctx = EvalCtx {
                params: p,
                triggers: &mut triggers,
                dt: DT,
                events: &mut events,
                curves: &mut curves,
                root_delta: Vec3::ZERO,
            };
            g.evaluate(&mut ctx, &mut out);
        }
        let fallback = g.describe()["motion"][0]["fallback"].as_f64().unwrap() as f32;
        let steps = events.iter().filter(|e| e.name == "footstep").count();
        let stride = g.motion().map_or(0.0, |m| m.stride_speed());
        (fallback, steps, stride)
    };
    let (fallback, steps, _) = run(&params(WALK_SPEED, 0.0, 0.0), 3.0);
    assert_eq!(fallback, 0.0);
    assert!(
        steps >= 3,
        "{steps} footsteps in 3 s of motion-matched walking"
    );
    let (fallback, _, _) = run(&params(WALK_SPEED, 0.0, std::f32::consts::FRAC_PI_2), 1.0);
    assert_eq!(fallback, 1.0, "strafing stayed on motion matching");
    let (fallback, _, _) = run(&params(WALK_SPEED, 0.0, 0.0), 1.0);
    assert_eq!(fallback, 0.0);
    // A 45° diagonal hands over too, unless the lower body is turned toward it (orientation
    // warping publishes `warp_max`): then it stays on the database and plays a forward walk.
    let diagonal = std::f32::consts::FRAC_PI_4;
    let (fallback, _, _) = run(&params(WALK_SPEED, 0.0, diagonal), 1.0);
    assert_eq!(fallback, 1.0, "a diagonal stayed on motion matching");
    let mut warped = params(WALK_SPEED, 0.0, diagonal);
    warped.set("warp_max", 60f32.to_radians());
    let (fallback, steps, stride) = run(&warped, 3.0);
    assert_eq!(fallback, 0.0, "the warped diagonal handed over");
    assert!(
        steps >= 3,
        "{steps} footsteps in 3 s of the warped diagonal"
    );
    // The pace its feet cover is known, for stride warping.
    assert!(stride > 0.5 * WALK_SPEED, "the feet cover {stride} m/s");
    let (fallback, _, _) = run(&warped_strafe(), 1.0);
    assert_eq!(
        fallback, 1.0,
        "strafing past the warp stayed on motion matching"
    );

    // Without the database the graph names it.
    let err = GraphInstance::compile(&graph, &s, &lookup).err().unwrap();
    assert!(err.contains("test/motion"), "{err}");
}

#[test]
fn every_frame_is_labelled_with_the_feet_on_the_ground() {
    use engine_anim::motion::contact_bits;
    let (_, _, db) = setup();
    let down = |b: u8, foot: usize| b & (contact_bits(foot, false) | contact_bits(foot, true)) != 0;
    // Standing, both feet are down throughout.
    let idle = db.first_frame("idle").unwrap();
    for f in idle..idle + 30 {
        assert!(
            down(db.contacts(f), 0) && down(db.contacts(f), 1),
            "frame {f}"
        );
    }
    // These test walks carry their travel on the hips without matching the stride (a planted
    // foot still drifts at 0.5-1.4 m/s), so they never count as standing: a foot sliding
    // over the ground is not a contact.
    let walk = db.first_frame("walk_moving").unwrap();
    assert!((walk..walk + 30).all(|f| !down(db.contacts(f), 0)));
}
