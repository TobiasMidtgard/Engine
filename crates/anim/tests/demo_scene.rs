//! Runs the animation paddock without a renderer and checks that each test case does what
//! it shows: the hammer lands on the tire with both hands on the handle, muscles tense and
//! relax, the walker climbs the course, the horse gallops with working flanks, the dog runs
//! through all its gaits with a flexing spine, the spider and the car move.

use engine_anim::demo::{
    DemoEntities, hammer_head, lantern_wrist, spawn_demo_world, tire_strike_point,
};
use engine_anim::{AnimationPlugin, Animator, LeggedGait, MuscleRig};
use engine_core::prelude::*;
use engine_core::time::TimeSource;
use engine_world::WorldPlugin;
use engine_world::animation::AnimationParams;

const STEP: f64 = 1.0 / 60.0;

fn app() -> (App, DemoEntities) {
    let mut app = App::new();
    app.world.resource_mut::<EngineConfig>().headless = true;
    app.world
        .resource_mut::<Time>()
        .set_source(TimeSource::Manual { step_secs: STEP });
    app.add_plugins((WorldPlugin, AnimationPlugin));
    let demo = spawn_demo_world(&mut app.world, DVec3::ZERO);
    engine_anim::character::finish_loading(&mut app.world);
    (app, demo)
}

fn activation(world: &World, e: Entity, muscle: &str) -> f32 {
    world
        .get::<MuscleRig>(e)
        .map_or(0.0, |m| m.activation(muscle))
}

fn bone_world(world: &World, e: Entity, bone: &str) -> DVec3 {
    let a = world.get::<Animator>(e).unwrap();
    let gt = world.get::<GlobalTransform>(e).unwrap();
    engine_anim::ik::to_world(gt, a.model[a.skeleton.bone(bone)].t)
}

#[derive(Debug, Default)]
struct Seen {
    closest_strike: f64,
    max_hand_gap_while_gripping: f64,
    biceps_at_strike: f32,
    biceps_relaxed: f32,
    lifter_flex: f32,
    /// Farthest the reacher's right wrist gets from its hold on the lantern, m.
    lantern_miss: f64,
    /// How high the reacher's heels and balls of the feet rise while holding it, m.
    heel_rise: f64,
    toe_rise: f64,
    walker_max_height: f64,
    walker_max_speed: f32,
    walker_foot_float: f64,
    horse_max_speed: f32,
    haunch_at_gallop: f32,
    /// How far a swinging hind hoof flexes back from its cannon at the gallop (sine of the
    /// fetlock angle).
    hind_hoof_curl: f32,
    dog_gaits: Vec<String>,
    dog_spine: (f32, f32),
    /// Highest planted paw while walking, m.
    dog_paw_lift: f64,
    /// Planted paw samples at a walk or trot, and those over 1 cm above the sole.
    dog_planted: (usize, usize),
    spider_moved: f64,
    car_moved: f64,
}

#[test]
fn every_test_case_does_what_it_shows() {
    let (mut app, d) = app();
    let strike = tire_strike_point(DVec3::ZERO);
    let start_spider = app.world.get::<Transform>(d.spider).unwrap().translation;
    let start_car = app.world.get::<Transform>(d.car).unwrap().translation;
    let mut seen = Seen {
        closest_strike: f64::MAX,
        dog_spine: (f32::MAX, f32::MIN),
        ..Default::default()
    };
    let frames = (14.0 / STEP) as usize;
    let mut rest_feet = (0.0, 0.0);
    for frame in 1..=frames {
        app.update();
        let t = frame as f64 * STEP;
        let w = &app.world;
        if frame == 20 {
            rest_feet = (
                bone_world(w, d.reacher, "foot_l").y,
                bone_world(w, d.reacher, "toe_l").y,
            );
        }
        // Swing: triggered at 0.5 s, impact at 53% of 2 s.
        if (1.45..1.7).contains(&t) {
            let head = hammer_head(w, d.athlete).unwrap();
            seen.closest_strike = seen.closest_strike.min(head.distance(strike));
            seen.biceps_at_strike = seen
                .biceps_at_strike
                .max(activation(w, d.athlete, "biceps_r"));
        }
        if (1.0..1.4).contains(&t) {
            let gap =
                bone_world(w, d.athlete, "hand_l").distance(bone_world(w, d.athlete, "hand_r"));
            seen.max_hand_gap_while_gripping = seen.max_hand_gap_while_gripping.max(gap);
        }
        if (3.4..3.5).contains(&t) {
            seen.biceps_relaxed = activation(w, d.athlete, "biceps_r");
        }
        if (2.2..2.8).contains(&t) {
            seen.lifter_flex = seen.lifter_flex.max(activation(w, d.lifter, "biceps_l"));
        }
        // The reach: triggered at 0.5 s, holding the lantern from 40% to 70% of 3 s.
        if (1.8..2.5).contains(&t) {
            let hand = bone_world(w, d.reacher, "hand_r");
            seen.lantern_miss = seen
                .lantern_miss
                .max(hand.distance(lantern_wrist(DVec3::ZERO)));
            let heel = bone_world(w, d.reacher, "foot_l")
                .y
                .min(bone_world(w, d.reacher, "foot_r").y);
            let toe = bone_world(w, d.reacher, "toe_l")
                .y
                .max(bone_world(w, d.reacher, "toe_r").y);
            seen.heel_rise = seen.heel_rise.max(heel - rest_feet.0);
            seen.toe_rise = seen.toe_rise.max(toe - rest_feet.1);
        }
        let walker = w.get::<GlobalTransform>(d.walker).unwrap().translation();
        seen.walker_max_height = seen.walker_max_height.max(walker.y);
        let params = w.get::<AnimationParams>(d.walker).unwrap();
        seen.walker_max_speed = seen.walker_max_speed.max(params.get("speed"));
        // On flat ground away from the course, the lower foot stays near the floor.
        if walker.x < -15.5 && walker.z > -5.0 && walker.y.abs() < 0.01 {
            let low = bone_world(w, d.walker, "foot_l")
                .y
                .min(bone_world(w, d.walker, "foot_r").y);
            seen.walker_foot_float = seen.walker_foot_float.max(low);
        }
        let horse_speed = w.get::<AnimationParams>(d.horse).unwrap().get("speed");
        seen.horse_max_speed = seen.horse_max_speed.max(horse_speed);
        if horse_speed > 9.0 {
            let haunch = activation(w, d.horse, "haunch_l").max(activation(w, d.horse, "haunch_r"));
            seen.haunch_at_gallop = seen.haunch_at_gallop.max(haunch);
            // In the swing the hind hooves curl back behind the fetlocks.
            let a = w.get::<Animator>(d.horse).unwrap();
            let horse_gait = w.get::<LeggedGait>(d.horse).unwrap();
            let turn = |bone: &str| {
                let b = a.skeleton.bone(bone);
                a.model[b].r * a.skeleton.rest_model[b].r.inverse()
            };
            for (leg, side) in [(2, "l"), (3, "r")] {
                if horse_gait.planted(leg) {
                    continue;
                }
                let fetlock = turn(&format!("hind_cannon_{side}")).inverse()
                    * turn(&format!("hind_hoof_{side}"));
                seen.hind_hoof_curl = seen.hind_hoof_curl.max((fetlock * Vec3::NEG_Y).z);
            }
        }
        let gait = w.get::<LeggedGait>(d.dog).unwrap();
        let stage = gait.stage_name().to_string();
        if !seen.dog_gaits.contains(&stage) {
            seen.dog_gaits.push(stage.clone());
        }
        if stage == "gallop" {
            // The spine: the loin's pitch against the chest.
            let a = w.get::<Animator>(d.dog).unwrap();
            let (body, loin) = (a.skeleton.bone("body"), a.skeleton.bone("loin"));
            let rel = a.model[body].r.inverse() * a.model[loin].r;
            let pitch = (rel * Vec3::NEG_Z).y.asin().to_degrees();
            seen.dog_spine = (seen.dog_spine.0.min(pitch), seen.dog_spine.1.max(pitch));
        }
        // On the flat, planted paws stay on the ground (the sole is 3.5 cm).
        if (stage == "walk" || stage == "trot") && t > 1.0 {
            let paws = ["front_paw_l", "front_paw_r", "hind_paw_l", "hind_paw_r"];
            for (i, paw) in paws.iter().enumerate() {
                if !gait.planted(i) {
                    continue;
                }
                let y = bone_world(w, d.dog, paw).y;
                if stage == "walk" {
                    seen.dog_paw_lift = seen.dog_paw_lift.max(y);
                }
                seen.dog_planted.0 += 1;
                seen.dog_planted.1 += usize::from(y > 0.045);
            }
        }
    }
    let w = &app.world;
    seen.spider_moved = w
        .get::<Transform>(d.spider)
        .unwrap()
        .translation
        .distance(start_spider);
    seen.car_moved = w
        .get::<Transform>(d.car)
        .unwrap()
        .translation
        .distance(start_car);

    eprintln!("{seen:#?}");
    assert!(
        seen.closest_strike < 0.3,
        "hammer missed the tire by {:.2} m",
        seen.closest_strike
    );
    assert!(
        seen.max_hand_gap_while_gripping < 0.25,
        "hands {:.2} m apart on the handle",
        seen.max_hand_gap_while_gripping
    );
    assert!(
        seen.biceps_at_strike > 0.5,
        "biceps at the strike {}",
        seen.biceps_at_strike
    );
    assert!(
        seen.biceps_relaxed < seen.biceps_at_strike * 0.5,
        "biceps did not relax: {} after {}",
        seen.biceps_relaxed,
        seen.biceps_at_strike
    );
    assert!(
        seen.lifter_flex > 0.6,
        "flex activation {}",
        seen.lifter_flex
    );
    assert!(
        seen.lantern_miss < 0.05,
        "the reacher's hand missed the lantern by {:.2} m",
        seen.lantern_miss
    );
    assert!(
        seen.heel_rise > 0.035 && seen.toe_rise < 0.015,
        "the reacher didn't go up on tiptoe: heels {:.3} m, balls of the feet {:.3} m",
        seen.heel_rise,
        seen.toe_rise
    );
    assert!(
        seen.walker_max_height > 0.6,
        "walker never got up the ramp ({:.2} m)",
        seen.walker_max_height
    );
    assert!(
        seen.walker_max_speed > 3.0,
        "walker never ran ({})",
        seen.walker_max_speed
    );
    assert!(
        seen.walker_foot_float < 0.2,
        "walker's feet float {:.2} m",
        seen.walker_foot_float
    );
    assert!(
        seen.horse_max_speed > 9.0,
        "horse never galloped ({})",
        seen.horse_max_speed
    );
    assert!(
        seen.haunch_at_gallop > 0.4,
        "haunches at the gallop {}",
        seen.haunch_at_gallop
    );
    assert!(
        seen.hind_hoof_curl > 0.5,
        "the swinging hind hooves don't curl back: {:.2}",
        seen.hind_hoof_curl
    );
    for g in ["walk", "trot", "gallop"] {
        assert!(
            seen.dog_gaits.iter().any(|s| s == g),
            "the dog never went to a {g}: {:?}",
            seen.dog_gaits
        );
    }
    assert!(
        seen.dog_spine.1 - seen.dog_spine.0 > 4.0,
        "the dog's spine is stiff at the gallop: {:?}",
        seen.dog_spine
    );
    assert!(
        seen.dog_paw_lift < 0.065,
        "the walking dog's planted paws float up to {:.3} m",
        seen.dog_paw_lift
    );
    let (planted, floating) = seen.dog_planted;
    assert!(
        planted > 200 && floating * 10 < planted,
        "{floating} of {planted} planted paws float at a walk or trot"
    );
    assert!(
        seen.spider_moved > 1.0,
        "spider moved {:.2} m",
        seen.spider_moved
    );
    assert!(seen.car_moved > 5.0, "car moved {:.2} m", seen.car_moved);
}
