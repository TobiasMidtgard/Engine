//! Hair grown by another module through the `HairGuides` contract: softbody simulates it on
//! a child of the person, doesn't draw it, and hands the posed points back as
//! `HairGuidesPosed` in the person's model space.

use std::f32::consts::{FRAC_PI_2, TAU};
use std::sync::Arc;

use engine_core::prelude::*;
use engine_core::time::{FixedTime, TimeSource};
use engine_softbody::guides::{GroomedRest, GuideHair};
use engine_softbody::{BoneCapsule, BoneColliders, SoftBodyPlugin, SoftBodySim};
use engine_world::animation::{BonePose, SkeletonPose};
use engine_world::{HairBody, HairBodyPart, HairGuide, HairGuides, HairGuidesPosed, WeatherState};

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
                tie: 0,
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

/// Steps until every hair body's rest shape, solved on a worker, is in and its motion shown
/// (at most 60 s of wall time: how many frames that takes depends on the machine's cores).
fn until_rested(app: &mut App) {
    let started = std::time::Instant::now();
    loop {
        let mut q = app.world.query::<&GroomedRest>();
        if q.iter(&app.world).all(GroomedRest::shown) {
            return;
        }
        assert!(
            started.elapsed() < std::time::Duration::from_secs(60),
            "the hair's rest shape never came in"
        );
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
    until_rested(&mut app);

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
        // Still hair is handed back as it was grown.
        for (q, r) in p.iter().zip(&g.points) {
            assert!(q.distance(*r) < 3e-3, "lock {i}: {q} was grown at {r}");
        }
    }
    // The simulated hair itself hangs outside the head it slides off (no deeper in its
    // capsules than it was grown).
    let mut q = app.world.query::<(&GuideHair, &SoftBodySim)>();
    let (_, body) = q.single(&app.world).unwrap();
    let head = engine_softbody::hair_body::capsules(&rest, &pose(), 0.5);
    assert!(!head.is_empty(), "a head is fitted to the roots");
    let to_model = app
        .world
        .get::<GlobalTransform>(person)
        .unwrap()
        .affine()
        .inverse();
    let tress = body.sim.tress.as_ref().expect("the strand solver");
    for (i, w) in body.sim.inv_mass.iter().enumerate() {
        let at = to_model.transform_point3(body.world_position(i)).as_vec3();
        let outside = engine_softbody::hair_body::distance(&head, at, false);
        assert!(
            *w == 0.0 || outside > -tress.verts[i].sunk[0] - 1e-3,
            "particle {i} at {at} is {outside} m outside the head"
        );
    }

    // The head moves 30 cm to the side: the roots go with it, the tips trail behind.
    app.world.get_mut::<SkeletonPose>(person).unwrap().bones[1].translation += Vec3::X * 0.3;
    steps(&mut app, 2);
    let posed = app.world.get::<HairGuidesPosed>(person).unwrap().clone();
    let p = posed.guide(&rest, 0).unwrap();
    let moved = rest.guides[0].points[0] + Vec3::X * 0.3;
    assert!(p[0].distance(moved) < 2e-3, "root {} vs {moved}", p[0]);
    assert!(
        p[8].x < rest.guides[0].points[8].x + 0.29,
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
                tie: 0,
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
                tie: 0,
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

/// `n` locks of `len` points, grown straight out sideways from the skull (a styled shape
/// on a stand pose, far from where it hangs).
fn long_out(n: usize, len: usize) -> HairGuides {
    let center = HEAD + SKULL;
    let guides = (0..n)
        .map(|k| {
            let a = k as f32 / n as f32 * TAU;
            let out = Vec3::new(a.cos(), 0.0, a.sin());
            let d = (out + Vec3::Y * 0.3).normalize();
            let root = center + d * 0.1;
            HairGuide {
                bones: [1, 1],
                blend: 0.0,
                points: (0..len).map(|i| root + out * (0.035 * i as f32)).collect(),
                normal: d,
                thickness: 0.004,
                tie: 0,
            }
        })
        .collect();
    HairGuides {
        version: 1,
        guides,
        grown_on: pose().bones,
        stiffness: 0.05,
    }
}

#[test]
fn a_long_groom_built_near_too_big_to_settle_ahead_sags_into_place_without_flailing() {
    let mut app = app();
    // 400 locks of 20 points: past what a frame may spend settling a body ahead of time,
    // so it settles in its own steps, in view.
    let grown = long_out(400, 20);
    let person = app
        .world
        .spawn((Transform::default(), pose(), grown.clone()))
        .id();
    let mut q = app.world.query::<(&GuideHair, &SoftBodySim)>();
    let mut fastest_seen = 0.0f32;
    let mut stepped = false;
    for step in 0..80 {
        app.update();
        let Ok((_, b)) = q.single(&app.world) else {
            continue;
        };
        stepped = true;
        for (i, g) in grown.guides.iter().enumerate() {
            let tip = b.world_position(i * 20 + 19);
            assert!(
                tip.y < f64::from(g.points[19].y) + 0.05,
                "step {step}: lock {i}'s tip rose to {tip}"
            );
        }
        let fastest = b.sim.vel.iter().map(|v| v.length()).fold(0.0, f32::max);
        fastest_seen = fastest_seen.max(fastest);
        // 70 cm locks let go from level swing through at 3.7 m/s with nothing holding them
        // back; settling, their tips stay under that as they come straight.
        assert!(fastest < 3.5, "step {step}: hair swung at {fastest:.1} m/s");
    }
    assert!(stepped, "never simulated");
    until_rested(&mut app);
    // It did fall (it is not just frozen in the grown shape), and it is settled by now.
    let (_, b) = q.single(&app.world).unwrap();
    let tip = b.world_position(19);
    assert!(
        tip.y < f64::from(grown.guides[0].points[19].y) - 0.2,
        "the tip didn't sag: {tip} (fastest {fastest_seen:.2} m/s)"
    );
    // What is handed back is the grown shape all the while: where the hair rests is its
    // own business, and only its motion from there is drawn.
    let posed = app.world.get::<HairGuidesPosed>(person).expect("posed");
    for (i, g) in grown.guides.iter().enumerate() {
        let tip = posed.guide(&grown, i).unwrap()[19];
        assert!(
            tip.distance(g.points[19]) < 0.01,
            "lock {i}'s tip was handed back at {tip}, grown at {}",
            g.points[19]
        );
    }
    let mut q = app.world.query::<(&GuideHair, &SoftBodySim)>();
    let (_, b) = q.single(&app.world).unwrap();
    assert_eq!(b.pending_prewarm, 0, "still settling");
}

#[test]
fn still_hair_is_handed_back_as_grown_and_its_motion_on_top() {
    let mut app = app();
    let grown = guides(1, 48);
    let person = app
        .world
        .spawn((
            Transform::from_xyz(-40.0, 3.0, 12.0).with_rotation(Quat::from_rotation_y(1.1)),
            pose(),
            BoneColliders {
                capsules: vec![BoneCapsule::sphere("head", 0.1, SKULL.to_array())],
                friction: 0.3,
            },
            grown.clone(),
        ))
        .id();
    steps(&mut app, 90);
    let worst = |app: &App| {
        let posed = app.world.get::<HairGuidesPosed>(person).unwrap();
        posed
            .points
            .iter()
            .zip(grown.guides.iter().flat_map(|g| &g.points))
            .map(|(a, b)| a.distance(*b))
            .fold(0.0, f32::max)
    };
    // Left alone the simulation rests centimetres off the grown shape (it sags, and the
    // skull's sphere pushes it out); what it hands back is the grown shape to the millimetre.
    let mut q = app.world.query::<(&GuideHair, &SoftBodySim)>();
    let (_, body) = q.single(&app.world).unwrap();
    let t = *app.world.get::<GlobalTransform>(person).unwrap();
    let off = grown
        .guides
        .iter()
        .flat_map(|g| &g.points)
        .enumerate()
        .map(|(i, p)| {
            body.world_position(i)
                .distance(t.transform_point(p.as_dvec3()))
        })
        .fold(0.0, f64::max);
    assert!(
        off > 0.01,
        "the simulation rests on the grown shape ({off} m)"
    );
    let still = worst(&app);
    assert!(
        still < 1.5e-3,
        "still hair is {still} m off its grown shape"
    );

    // The head tips 60° to the side: the hair hangs away from the grown shape, and shows it.
    app.world.get_mut::<SkeletonPose>(person).unwrap().bones[1].rotation =
        Quat::from_rotation_z(60f32.to_radians());
    steps(&mut app, 120);
    let tipped = worst(&app);
    assert!(tipped > 0.05, "tipped hair moved only {tipped} m");
}

/// The skull and a neck under it as a look module would measure them: one box on the head
/// bone, 8 mm samples of the distance to a 10 cm ball and a 5 cm column under it.
fn measured(version: u32) -> HairBody {
    let center = HEAD + SKULL;
    let (lo, cell) = (center - Vec3::new(0.16, 0.3, 0.16), 0.008);
    let size = [41u16, 58, 41];
    let mut distance = Vec::new();
    for z in 0..size[2] {
        for y in 0..size[1] {
            for x in 0..size[0] {
                let p = lo + Vec3::new(x as f32, y as f32, z as f32) * cell - center;
                let ball = p.length() - 0.1;
                let column = Vec2::new(p.x, p.z).length() - 0.05;
                let d = if p.y < 0.0 { ball.min(column) } else { ball };
                distance.push((d / HairBody::UNIT) as i16);
            }
        }
    }
    HairBody {
        version,
        parts: vec![HairBodyPart {
            bone: 1,
            min: lo,
            cell,
            size,
            weight: vec![255u8; distance.len()].into(),
            distance: distance.into(),
        }],
    }
}

#[test]
fn hair_is_held_out_of_the_measured_body_and_its_bones_are_handed_on() {
    let mut app = app();
    let grown = guides(1, 24);
    let person = app
        .world
        .spawn((Transform::default(), pose(), grown.clone(), measured(1)))
        .id();
    steps(&mut app, 60);
    until_rested(&mut app);
    let hair = {
        let mut q = app.world.query::<&GuideHair>();
        *q.single(&app.world).unwrap()
    };
    assert_eq!(hair.surface, Some(1), "built on the measured body");
    {
        let mut q = app.world.query::<(&GuideHair, &SoftBodySim)>();
        let (_, body) = q.single(&app.world).unwrap();
        let tress = body.sim.tress.as_ref().unwrap();
        assert_eq!((tress.boxes.len(), tress.capsules.len()), (1, 0));
    }
    // The head nods and turns; the hair stays out of ball and column wherever they go (where
    // a lock was grown through the ball, no deeper in it than it was grown).
    let center = HEAD + SKULL;
    let depth = |q: Vec3| {
        let ball = q.length() - 0.1;
        let column = Vec2::new(q.x, q.z).length() - 0.05;
        if q.y < 0.0 { ball.min(column) } else { ball }
    };
    let mut deepest = f32::MAX;
    for step in 0..180 {
        let t = step as f32 / 60.0;
        let turn = Quat::from_euler(
            EulerRot::YXZ,
            0.7 * (3.0 * t).sin(),
            0.4 * (2.0 * t).sin(),
            0.0,
        );
        let mut now = pose();
        now.bones[1].rotation = turn;
        *app.world.get_mut::<SkeletonPose>(person).unwrap() = now;
        steps(&mut app, 1);
        let posed = app.world.get::<HairGuidesPosed>(person).unwrap();
        assert_eq!(posed.body.len(), 1);
        // The bone as the step posed it carries the box.
        let bone = posed.body[0];
        let to_grown = |p: Vec3| HEAD + bone.rotation.conjugate() * (p - bone.translation);
        for g in 0..grown.guides.len() {
            let now = posed.guide(&grown, g).unwrap();
            for (p, was) in now.iter().zip(&grown.guides[g].points).skip(2) {
                let sunk = depth(*was - center).min(0.0);
                deepest = deepest.min(depth(to_grown(*p) - center) - sunk);
            }
        }
    }
    assert!(deepest > -0.002, "{deepest} m inside the measured body");
    // A body measured for other guides is left aside: capsules again.
    app.world.entity_mut(person).insert(measured(7));
    steps(&mut app, 3);
    let mut q = app.world.query::<&GuideHair>();
    assert_eq!(q.single(&app.world).unwrap().surface, None);
}

/// A blade 60 cm long lying along z at `y`, its edge facing -x, with the motion it is given
/// each step by hand.
fn blade(app: &mut App, sharpness: f32, at: Vec3) -> Entity {
    app.world
        .spawn((
            Transform::from_translation(at.as_dvec3()),
            engine_world::CuttingEdge {
                heel: Vec3::new(0.0, 0.0, -0.3),
                tip: Vec3::new(0.0, 0.0, 0.3),
                facing: Vec3::NEG_X,
                double: false,
                sharpness,
            },
        ))
        .id()
}

/// Sweeps `blade` level through the hair from x = 0.4 to -0.4 at 8 m/s, then lets `after`
/// steps pass. Returns the farthest any guide point was carried toward -x of where it hung.
fn swing(app: &mut App, person: Entity, blade: Entity, after: usize) -> f32 {
    let hung = app
        .world
        .get::<HairGuidesPosed>(person)
        .unwrap()
        .points
        .clone();
    let mut carried = 0.0f32;
    for step in 0..6 + after {
        let x = (0.4 - 8.0 / 60.0 * (step + 1) as f32).max(-0.4);
        app.world.get_mut::<Transform>(blade).unwrap().translation.x = f64::from(x);
        steps(app, 1);
        let now = &app.world.get::<HairGuidesPosed>(person).unwrap().points;
        for (a, b) in hung.iter().zip(now) {
            carried = carried.max(a.x - b.x);
        }
    }
    carried
}

#[test]
fn a_sharp_blade_cuts_the_hair_and_a_blunt_one_sweeps_it_aside() {
    // A honed sword, swung level through the locks 13 cm and more below their roots.
    let mut app = app();
    let grown = guides(1, 24);
    let person = app
        .world
        .spawn((Transform::default(), pose(), grown.clone()))
        .id();
    steps(&mut app, 60);
    until_rested(&mut app);
    let sword = blade(&mut app, 0.8, Vec3::new(0.4, 1.6, 0.0));
    steps(&mut app, 2);
    assert!(
        app.world
            .get::<HairGuidesPosed>(person)
            .unwrap()
            .cut
            .is_empty()
    );
    swing(&mut app, person, sword, 20);
    let posed = app.world.get::<HairGuidesPosed>(person).unwrap().clone();
    assert_eq!((posed.cut.len(), posed.shed.len()), (24, 24));
    for g in 0..24 {
        let (left, shed) = (posed.cut[g], posed.shed[g]);
        assert!((0.2..0.8).contains(&left), "lock {g}: {left} of it left");
        assert!(shed > 0.05 && shed < 0.2, "lock {g}: piece {shed} gone");
        let points = posed.guide(&grown, g).unwrap();
        // What is left hangs from the head down to the blade's height; the piece has come
        // away from it and is falling.
        let end = ((points.len() - 1) as f32 * left) as usize;
        assert!(
            points[end].y > 1.6 - 0.03,
            "lock {g} ends at {}",
            points[end].y
        );
        assert!(
            points[end + 1].y < points[end].y - 0.05,
            "lock {g}: its piece is at {} under an end at {}",
            points[end + 1].y,
            points[end].y
        );
    }
    // Four seconds on the pieces are gone, and the hair is as short as it was cut.
    steps(&mut app, 240);
    let later = app.world.get::<HairGuidesPosed>(person).unwrap();
    assert!(later.shed.iter().all(|s| *s >= 1.0));
    assert_eq!(later.cut, posed.cut);

    // A blunt training blade through the same hair at the same speed: nothing is cut, and
    // the locks it met were carried along with it.
    let mut app = self::app();
    let person = app
        .world
        .spawn((Transform::default(), pose(), grown.clone()))
        .id();
    steps(&mut app, 60);
    until_rested(&mut app);
    let blunt = blade(&mut app, 0.0, Vec3::new(0.4, 1.6, 0.0));
    steps(&mut app, 2);
    let carried = swing(&mut app, person, blunt, 4);
    let posed = app.world.get::<HairGuidesPosed>(person).unwrap();
    assert!(posed.cut.is_empty(), "{:?}", posed.cut);
    assert!(carried > 0.04, "carried {carried} m");
    // And it hangs whole again once the blade is through.
    until_rested(&mut app);
    steps(&mut app, 120);
    let posed = app.world.get::<HairGuidesPosed>(person).unwrap();
    for g in 0..24 {
        let points = posed.guide(&grown, g).unwrap();
        let len: f32 = points.windows(2).map(|w| w[0].distance(w[1])).sum();
        let was: f32 = grown.guides[g]
            .points
            .windows(2)
            .map(|w| w[0].distance(w[1]))
            .sum();
        assert!((len / was - 1.0).abs() < 0.08, "lock {g}: {len} m of {was}");
    }
}

#[test]
fn hair_is_as_wet_as_its_person_and_wetter_in_the_rain() {
    let mut app = app();
    let grown = guides(1, 24);
    let person = app
        .world
        .spawn((
            Transform::default(),
            pose(),
            grown.clone(),
            engine_world::character::Appearance::default(),
        ))
        .id();
    steps(&mut app, 60);
    until_rested(&mut app);
    steps(&mut app, 5);
    assert!(
        app.world
            .get::<HairGuidesPosed>(person)
            .unwrap()
            .wet
            .is_empty()
    );
    // Out of the water: soaked at once, every lock.
    app.world
        .get_mut::<engine_world::character::Appearance>(person)
        .unwrap()
        .wear
        .wet = 1.0;
    steps(&mut app, 120);
    let posed = app.world.get::<HairGuidesPosed>(person).unwrap();
    assert_eq!(posed.wet.len(), 24);
    assert!(posed.wet.iter().all(|w| *w > 0.99), "{:?}", posed.wet);
    // Dry skin again: the hair dries by itself, slowly; rain wets it again.
    app.world
        .get_mut::<engine_world::character::Appearance>(person)
        .unwrap()
        .wear
        .wet = 0.0;
    steps(&mut app, 120);
    let drying = app.world.get::<HairGuidesPosed>(person).unwrap().wet[0];
    assert!((0.5..1.0).contains(&drying), "{drying}");
    {
        let mut weather = app.world.resource_mut::<WeatherState>();
        weather.precipitation_mm_h = 40.0;
        weather.precipitation_kind = engine_world::PrecipitationKind::Rain;
    }
    steps(&mut app, 120);
    let rained = app.world.get::<HairGuidesPosed>(person).unwrap().wet[0];
    assert!(rained > drying, "{rained} after rain, {drying} before");

    // Wet hair is heavy: a wind that lifts dry locks moves soaked ones much less.
    let blown = |wet: f32| {
        let mut app = self::app();
        let mut look = engine_world::character::Appearance::default();
        look.wear.wet = wet;
        let person = app
            .world
            .spawn((Transform::default(), pose(), grown.clone(), look))
            .id();
        steps(&mut app, 60);
        until_rested(&mut app);
        steps(&mut app, 120);
        let tips = |app: &App| -> Vec<Vec3> {
            let posed = app.world.get::<HairGuidesPosed>(person).unwrap();
            (0..24)
                .map(|g| *posed.guide(&grown, g).unwrap().last().unwrap())
                .collect()
        };
        let hung = tips(&app);
        app.world.resource_mut::<WeatherState>().wind = Vec3::new(8.0, 0.0, 0.0);
        steps(&mut app, 90);
        let moved: f32 = hung
            .iter()
            .zip(tips(&app))
            .map(|(a, b)| a.distance(b))
            .sum();
        moved / 24.0
    };
    let (dry, soaked) = (blown(0.0), blown(1.0));
    assert!(
        dry > 0.01 && soaked < 0.7 * dry,
        "an 8 m/s wind moves dry tips {dry} m and soaked ones {soaked} m"
    );
}
