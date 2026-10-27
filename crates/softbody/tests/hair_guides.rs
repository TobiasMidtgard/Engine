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

/// Locks from the front hairline, combed back over the crown along the skull and falling
/// behind it.
fn swept_back(version: u32) -> HairGuides {
    let center = HEAD + SKULL;
    let guides = [-0.04f32, 0.0, 0.04]
        .into_iter()
        .map(|x| {
            // On the great circle through the top of the head, from 60° in front of the top.
            let at = |phi: f32| {
                let d = Vec3::new(x * 10.0, phi.cos(), phi.sin()).normalize();
                center + d * 0.104
            };
            let step = 0.035 / 0.104;
            let points = (0..9).map(|i| at(-1.05 + step * i as f32)).collect();
            HairGuide {
                bones: [1, 1],
                blend: 0.0,
                points,
                normal: (at(-1.05) - center).normalize(),
                thickness: 0.004,
            }
        })
        .collect();
    HairGuides {
        version,
        guides,
        grown_on: pose().bones,
        stiffness: 0.2,
    }
}

#[test]
fn hair_combed_back_from_the_hairline_stays_back_when_the_head_bows() {
    let mut app = app();
    let person = app
        .world
        .spawn((
            Transform::default(),
            pose(),
            BoneColliders {
                capsules: vec![BoneCapsule::sphere("head", 0.1, SKULL.to_array())],
                friction: 0.3,
            },
            swept_back(1),
        ))
        .id();
    steps(&mut app, 30);
    // The head bows 50° forward and stays bowed.
    let bow = Quat::from_rotation_x(-50f32.to_radians());
    app.world.get_mut::<SkeletonPose>(person).unwrap().bones[1].rotation = bow;
    steps(&mut app, 120);

    let rest = swept_back(1);
    let posed = app.world.get::<HairGuidesPosed>(person).unwrap().clone();
    for (i, g) in rest.guides.iter().enumerate() {
        let p = posed.guide(&rest, i).unwrap();
        // Back in the head's frame, the sweep over the crown keeps its grown shape (unheld,
        // it slid 6 to 25 cm forward over the face).
        for (k, (q, r)) in p.iter().zip(&g.points).enumerate().take(5) {
            let local = HEAD + bow.inverse() * (*q - HEAD);
            let slid = local.distance(*r);
            assert!(
                slid < 0.03,
                "lock {i} point {k} slid {slid:.3} m off the sweep"
            );
        }
    }
}

/// A person standing in a T pose: hips, chest, neck, head and arms held out to the sides.
fn t_pose() -> SkeletonPose {
    let bones = [
        ("pelvis", None, Vec3::new(0.0, 1.0, 0.0)),
        ("chest", Some(0), Vec3::new(0.0, 1.3, 0.0)),
        ("neck", Some(1), Vec3::new(0.0, 1.45, 0.0)),
        ("head", Some(2), Vec3::new(0.0, 1.55, 0.0)),
        ("upper_arm_l", Some(1), Vec3::new(0.18, 1.42, 0.0)),
        ("lower_arm_l", Some(4), Vec3::new(0.46, 1.42, 0.0)),
        ("hand_l", Some(5), Vec3::new(0.7, 1.42, 0.0)),
    ];
    SkeletonPose {
        names: Arc::from(bones.iter().map(|b| b.0.to_string()).collect::<Vec<_>>()),
        parents: Arc::from(bones.iter().map(|b| b.1).collect::<Vec<_>>()),
        bones: bones
            .iter()
            .map(|b| BonePose {
                translation: b.2,
                ..BonePose::default()
            })
            .collect(),
    }
}

#[test]
fn hair_grown_lying_on_an_arm_slides_off_it_without_sinking_in_or_being_flung() {
    let mut app = app();
    // Locks from the side of the head down onto the arm, then lying along the top of it
    // (the humanoid upper arm is 5.5 cm thick), a little in front of and behind its crest.
    let arm = |x: f32, z: f32| {
        let up = (0.058f32 * 0.058 - z * z).sqrt();
        Vec3::new(x, 1.42 + up, z)
    };
    let guides: Vec<HairGuide> = [-0.02f32, 0.0, 0.02]
        .into_iter()
        .map(|z| {
            let root = Vec3::new(0.08, 1.62, z * 0.5);
            let on = arm(0.2, z);
            let mut points: Vec<Vec3> = (0..5).map(|i| root.lerp(on, i as f32 / 4.0)).collect();
            points.extend((1..8).map(|i| arm(0.2 + 0.03 * i as f32, z)));
            HairGuide {
                bones: [3, 3],
                blend: 0.0,
                points,
                normal: Vec3::X,
                thickness: 0.004,
            }
        })
        .collect();
    let grown = HairGuides {
        version: 1,
        guides,
        grown_on: t_pose().bones,
        stiffness: 0.2,
    };
    let person = app
        .world
        .spawn((Transform::default(), t_pose(), grown.clone()))
        .id();
    let axis = |p: Vec3| Vec3::new(0.0, p.y - 1.42, p.z).length();
    let mut last: Option<Vec<Vec3>> = None;
    for step in 0..120 {
        app.update();
        let Some(posed) = app.world.get::<HairGuidesPosed>(person) else {
            continue;
        };
        for (i, g) in grown.guides.iter().enumerate() {
            let p = posed.guide(&grown, i).unwrap();
            for (k, (q, r)) in p.iter().zip(&g.points).enumerate().skip(5) {
                // Never deeper into the arm than it was grown, beyond the collision slop.
                assert!(
                    axis(*q) > axis(*r) - 0.004,
                    "step {step}: lock {i} point {k} sank to {:.3} m from the arm's axis",
                    axis(*q)
                );
            }
        }
        if let Some(last) = &last {
            let fastest = posed
                .points
                .iter()
                .zip(last)
                .map(|(a, b)| a.distance(*b) * 60.0)
                .fold(0.0, f32::max);
            assert!(fastest < 3.0, "step {step}: hair flung at {fastest:.1} m/s");
        }
        last = Some(posed.points.clone());
    }
}

#[test]
fn far_hair_rests_and_rides_the_head_until_it_is_near_again() {
    let mut app = app();
    let person = app
        .world
        .spawn((Transform::default(), pose(), guides(1, 24)))
        .id();
    // The camera 80 m off: past twice the LOD distance, inside the simulation range.
    #[derive(Resource)]
    struct Camera(DVec3);
    app.insert_resource(Camera(DVec3::new(80.0, 1.7, 0.0)));
    app.add_systems(
        PreUpdate,
        |cam: Res<Camera>, mut focus: ResMut<engine_world::FocusPoints>| {
            focus.points.clear();
            focus.points.push(cam.0);
        },
    );
    steps(&mut app, 30);
    let hair = |app: &mut App| {
        let mut q = app.world.query::<(&GuideHair, &SoftBodySim)>();
        let (_, b) = q.single(&app.world).unwrap();
        (b.frozen, b.center)
    };
    assert!(hair(&mut app).0, "hair 80 m off should rest");
    let posed = |app: &App| {
        app.world
            .get::<HairGuidesPosed>(person)
            .unwrap()
            .points
            .clone()
    };
    let before = posed(&app);

    // The person walks 3 m and turns a quarter: the hair goes along without being stepped.
    for k in 1..=30 {
        let s = k as f64 / 30.0;
        *app.world.get_mut::<Transform>(person).unwrap() = Transform::from_xyz(0.0, 0.0, 3.0 * s)
            .with_rotation(Quat::from_rotation_y(FRAC_PI_2 * s as f32));
        steps(&mut app, 1);
        assert!(hair(&mut app).0, "step {k}: it woke up");
    }
    steps(&mut app, 2);
    let after = posed(&app);
    let worst = before
        .iter()
        .zip(&after)
        .map(|(a, b)| a.distance(*b))
        .fold(0.0, f32::max);
    assert!(worst < 1e-3, "the resting hair slid {worst} m on the head");
    assert!((hair(&mut app).1 - DVec3::new(0.0, 1.6, 3.0)).length() < 0.5);

    // Back near, it is simulated again, still on the head.
    app.insert_resource(Camera(DVec3::new(0.0, 1.7, 5.0)));
    steps(&mut app, 30);
    assert!(!hair(&mut app).0, "near hair should be simulated");
    let roots = guides(1, 24);
    let p = app.world.get::<HairGuidesPosed>(person).unwrap();
    for i in 0..roots.guides.len() {
        let g = p.guide(&roots, i).unwrap();
        assert!(g[0].distance(roots.guides[i].points[0]) < 1e-3);
    }
}
