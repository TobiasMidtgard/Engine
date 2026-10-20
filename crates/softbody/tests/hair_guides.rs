//! Hair grown by another module through the `HairGuides` contract: softbody simulates it on
//! a child of the person, doesn't draw it, and hands the posed points back as
//! `HairGuidesPosed` in the person's model space.

use std::f32::consts::{FRAC_PI_2, TAU};
use std::sync::Arc;

use engine_core::prelude::*;
use engine_core::time::{FixedTime, TimeSource};
use engine_softbody::guides::GuideHair;
use engine_softbody::{BoneCapsule, BoneColliders, SoftBodyPlugin, SoftBodySim};
use engine_world::animation::{BonePose, SkeletonPose};
use engine_world::{HairGuide, HairGuides, HairGuidesPosed, WeatherState};

const HEAD: Vec3 = Vec3::new(0.0, 1.6, 0.0);
const SKULL: Vec3 = Vec3::new(0.0, 0.1, 0.0);

fn app() -> App {
    let mut app = App::new();
    app.add_plugins((engine_world::WorldPlugin, SoftBodyPlugin));
    app.world
        .resource_mut::<Time>()
        .set_source(TimeSource::Manual {
            step_secs: 1.0 / 60.0,
        });
    app.world.resource_mut::<FixedTime>().set_hz(60.0);
    let mut weather = app.world.resource_mut::<WeatherState>();
    weather.wind = Vec3::ZERO;
    weather.gustiness = 0.0;
    app
}

fn pose() -> SkeletonPose {
    let at = |translation: Vec3| BonePose {
        translation,
        ..BonePose::default()
    };
    SkeletonPose {
        names: Arc::from(vec!["pelvis".to_string(), "head".to_string()]),
        parents: Arc::from(vec![None, Some(0)]),
        bones: vec![at(Vec3::new(0.0, 1.0, 0.0)), at(HEAD)],
    }
}

/// `n` locks of 9 points from the top half of a 10 cm skull, falling outward and down.
fn guides(version: u32, n: usize) -> HairGuides {
    let center = HEAD + SKULL;
    let guides = (0..n)
        .map(|k| {
            let a = k as f32 / n as f32 * TAU;
            let up = 0.3 + 0.6 * ((k * 7) % n) as f32 / n as f32;
            let d = Vec3::new(
                a.cos() * (1.0 - up * up).sqrt(),
                up,
                a.sin() * (1.0 - up * up).sqrt(),
            );
            let out = Vec3::new(d.x, 0.0, d.z).normalize();
            let root = center + d * 0.1;
            HairGuide {
                bones: [1, 1],
                blend: 0.0,
                points: (0..9)
                    .map(|i| {
                        root + out * (0.012 * i.min(3) as f32) + Vec3::NEG_Y * (0.035 * i as f32)
                    })
                    .collect(),
                normal: d,
                thickness: 0.004,
            }
        })
        .collect();
    HairGuides {
        version,
        guides,
        grown_on: pose().bones,
        stiffness: 0.25,
    }
}

fn steps(app: &mut App, n: usize) {
    for _ in 0..n {
        app.update();
    }
}

#[test]
fn guides_from_another_module_are_simulated_and_handed_back_in_model_space() {
    let mut app = app();
    // Far from the origin and turned, so model space and world space differ.
    let person = app
        .world
        .spawn((
            Transform::from_xyz(500.0, 20.0, -300.0)
                .with_rotation(Quat::from_rotation_y(FRAC_PI_2)),
            pose(),
            BoneColliders {
                capsules: vec![BoneCapsule::sphere("head", 0.1, SKULL.to_array())],
                friction: 0.3,
            },
            guides(1, 24),
        ))
        .id();
    steps(&mut app, 60);

    // Simulated on a child, and not drawn here (the module that grew it draws it).
    let mut q = app.world.query::<(&GuideHair, &SoftBodySim)>();
    let sims: Vec<_> = q.iter(&app.world).collect();
    assert_eq!(sims.len(), 1);
    assert_eq!(sims[0].0.person, person);
    assert!(sims[0].1.drawn_elsewhere);

    let rest = guides(1, 24);
    let posed = app
        .world
        .get::<HairGuidesPosed>(person)
        .expect("posed guides")
        .clone();
    assert_eq!(posed.version, 1);
    assert_eq!(posed.points.len(), rest.point_count());
    for (i, g) in rest.guides.iter().enumerate() {
        let p = posed.guide(&rest, i).expect("matching guide");
        // Roots stay on the scalp; the free rest hangs below them, outside the skull.
        assert!(
            p[0].distance(g.points[0]) < 1e-3,
            "root {i}: {} vs {}",
            p[0],
            g.points[0]
        );
        let tip = *p.last().unwrap();
        assert!(tip.y < p[0].y - 0.15, "tip {i} {tip} root {}", p[0]);
        for q in &p[2..] {
            assert!(q.distance(HEAD + SKULL) > 0.1, "point {q} inside the skull");
        }
    }

    // The head moves 30 cm to the side: the roots go with it, the tips trail behind.
    app.world.get_mut::<SkeletonPose>(person).unwrap().bones[1].translation += Vec3::X * 0.3;
    steps(&mut app, 2);
    let posed = app.world.get::<HairGuidesPosed>(person).unwrap().clone();
    let p = posed.guide(&rest, 0).unwrap();
    let moved = rest.guides[0].points[0] + Vec3::X * 0.3;
    assert!(p[0].distance(moved) < 2e-3, "root {} vs {moved}", p[0]);
    assert!(
        p[8].x < rest.guides[0].points[8].x + 0.25,
        "tip didn't lag: {}",
        p[8]
    );
    steps(&mut app, 90);
    let posed = app.world.get::<HairGuidesPosed>(person).unwrap().clone();
    let p = posed.guide(&rest, 0).unwrap();
    assert!(
        p[8].distance(rest.guides[0].points[8] + Vec3::X * 0.3) < 0.08,
        "tip didn't settle back under the head: {}",
        p[8]
    );

    // Regrown with fewer locks: rebuilt, and the version echoes it.
    app.world.entity_mut(person).insert(guides(2, 12));
    app.world.get_mut::<SkeletonPose>(person).unwrap().bones[1].translation = HEAD;
    steps(&mut app, 30);
    let regrown = guides(2, 12);
    let posed = app.world.get::<HairGuidesPosed>(person).unwrap();
    assert_eq!(posed.version, 2);
    assert_eq!(posed.points.len(), regrown.point_count());
    assert_eq!(app.world.query::<&GuideHair>().iter(&app.world).count(), 1);

    // No more guides: no more simulation.
    app.world.entity_mut(person).remove::<HairGuides>();
    steps(&mut app, 2);
    assert_eq!(app.world.query::<&GuideHair>().iter(&app.world).count(), 0);
}

#[test]
fn people_without_colliders_get_humanoid_ones_and_bad_guides_are_reported_once() {
    let mut app = app();
    let person = app
        .world
        .spawn((Transform::default(), pose(), guides(1, 6)))
        .id();
    let broken = app
        .world
        .spawn((
            Transform::default(),
            pose(),
            HairGuides {
                grown_on: vec![BonePose::default(); 5],
                ..guides(1, 6)
            },
        ))
        .id();
    steps(&mut app, 10);
    assert!(app.world.get::<BoneColliders>(person).is_some());
    assert!(app.world.get::<HairGuidesPosed>(person).is_some());
    assert!(app.world.get::<HairGuidesPosed>(broken).is_none());
    let mut q = app.world.query::<(&GuideHair, Has<SoftBodySim>)>();
    let mut sims: Vec<_> = q.iter(&app.world).map(|(g, s)| (g.person, s)).collect();
    sims.sort();
    let mut want = vec![(person, true), (broken, false)];
    want.sort();
    assert_eq!(sims, want);
}
