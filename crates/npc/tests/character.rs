//! Fighters on the character contract, in a headless app (no GPU; stand-ins are capsules): the
//! `SkeletonPose` they write puts the humanoid skeleton's bones on the stand-in's joints; they
//! ask for a look; their own body hides only while a character module draws appearances (what
//! they hold stays); `FaceControls` follow pain and effort; blows and falls wear the look; and
//! each footstep is an `AnimationEvent` as well as a `GroundContact`.

use engine_core::ecs::message::Messages;
use engine_core::prelude::*;
use engine_core::time::TimeSource;
use engine_npc::*;
use engine_render::MeshRenderer;
use engine_world::animation::{AnimationEvent, SkeletonPose};
use engine_world::character::{
    Appearance, AppearanceBodies, AppearanceRequest, FaceControls, draws_appearances,
};
use engine_world::locomotion::MoveIntent;
use engine_world::{DamageEvent, DamageKind, GroundContact, WorldPlugin};

fn app() -> App {
    let mut app = App::new();
    app.world
        .resource_mut::<Time>()
        .set_source(TimeSource::Manual {
            step_secs: 1.0 / 60.0,
        });
    app.add_plugins((WorldPlugin, NpcPlugin));
    app.world.resource_mut::<NpcSettings>().character_looks = true;
    app
}

fn run(app: &mut App, secs: f32) {
    app.update_n((secs * 60.0).ceil() as usize);
}

/// A fighter with a stand-in at `at`, facing `facing`.
fn fighter(app: &mut App, at: DVec3, facing: Vec3, faction: Faction, weapon: NpcWeapon) -> Entity {
    let mut t = Transform::from_translation(at);
    t.look_to(facing, Vec3::Y);
    app.world
        .spawn((
            t,
            Npc::default(),
            faction,
            Squad(faction.0 + 3),
            NpcSkills::veteran(),
            weapon,
            StandIn::default(),
        ))
        .id()
}

/// One handed to game code, standing alone, facing −Z.
fn held_still(app: &mut App) -> Entity {
    let e = fighter(
        app,
        DVec3::ZERO,
        Vec3::NEG_Z,
        Faction::ENEMY,
        NpcWeapon::sword(),
    );
    app.world.get_mut::<Npc>(e).unwrap().active = false;
    e
}

fn hit(app: &mut App, e: Entity, amount: f32, kind: DamageKind) {
    let at = app.world.get::<Transform>(e).unwrap().translation;
    app.world.write_message(DamageEvent {
        target: e,
        amount,
        kind,
        point: at + DVec3::Y * 1.2,
        impulse: Vec3::Z * 10.0,
        source: None,
    });
}

/// The stand-in's parts: which, where (owner frame) and whether shown.
fn parts(app: &App, e: Entity) -> Vec<(Part, Transform, bool)> {
    let Some(children) = app.world.get::<Children>(e) else {
        return Vec::new();
    };
    children
        .iter()
        .filter_map(|c| {
            let part = app.world.get::<StandInPart>(c)?;
            let t = app.world.get::<Transform>(c)?;
            let r = app.world.get::<MeshRenderer>(c)?;
            Some((part.part, *t, r.visible))
        })
        .collect()
}

/// The animation module's humanoid skeleton (its README): each bone and its parent.
const HUMANOID: [(&str, Option<&str>); 25] = [
    ("root", None),
    ("hips", Some("root")),
    ("spine", Some("hips")),
    ("chest", Some("spine")),
    ("neck", Some("chest")),
    ("head", Some("neck")),
    ("ponytail_1", Some("head")),
    ("ponytail_2", Some("ponytail_1")),
    ("ponytail_3", Some("ponytail_2")),
    ("clavicle_l", Some("chest")),
    ("clavicle_r", Some("chest")),
    ("upper_arm_l", Some("clavicle_l")),
    ("upper_arm_r", Some("clavicle_r")),
    ("lower_arm_l", Some("upper_arm_l")),
    ("lower_arm_r", Some("upper_arm_r")),
    ("hand_l", Some("lower_arm_l")),
    ("hand_r", Some("lower_arm_r")),
    ("upper_leg_l", Some("hips")),
    ("upper_leg_r", Some("hips")),
    ("lower_leg_l", Some("upper_leg_l")),
    ("lower_leg_r", Some("upper_leg_r")),
    ("foot_l", Some("lower_leg_l")),
    ("foot_r", Some("lower_leg_r")),
    ("toe_l", Some("foot_l")),
    ("toe_r", Some("foot_r")),
];

/// Bone `name`'s place and turn.
fn bone(pose: &SkeletonPose, name: &str) -> (Vec3, Quat) {
    let i = pose
        .index_of(name)
        .unwrap_or_else(|| panic!("no bone {name}"));
    (pose.bones[i].translation, pose.bones[i].rotation)
}

#[test]
fn the_written_pose_puts_the_bones_on_the_stand_ins_joints() {
    let mut app = app();
    let a = fighter(
        &mut app,
        DVec3::ZERO,
        Vec3::NEG_Z,
        Faction::ENEMY,
        NpcWeapon::default(),
    );
    let b = fighter(
        &mut app,
        DVec3::new(0.5, 0.0, -3.5),
        Vec3::Z,
        Faction::RIVAL,
        NpcWeapon::sword_and_shield(),
    );
    let c = fighter(
        &mut app,
        DVec3::new(-6.0, 0.0, -8.0),
        Vec3::Z,
        Faction::RIVAL,
        NpcWeapon::rifle(),
    );
    app.update();
    app.update();
    // Shared names and parents, the animation module's.
    let pose = app.world.get::<SkeletonPose>(a).expect("a pose");
    assert_eq!(pose.names.len(), HUMANOID.len());
    for (name, parent) in HUMANOID {
        let i = pose
            .index_of(name)
            .unwrap_or_else(|| panic!("no bone {name}"));
        let p = pose.parents[i].map(|p| pose.names[p as usize].as_str());
        assert_eq!(p, parent, "{name}'s parent");
    }
    let other = app.world.get::<SkeletonPose>(b).unwrap();
    assert!(std::sync::Arc::ptr_eq(&pose.names, &other.names));
    assert!(std::sync::Arc::ptr_eq(&pose.parents, &other.parents));
    // Standing, turned as the animation module's rest pose has it: the thighs hang down with
    // their hinge (+X) level, the knees bend back about it and the elbows forward, and the
    // feet are flat (toes along −Z, turned out as the stance has them).
    for e in [a, b, c] {
        let pose = app.world.get::<SkeletonPose>(e).unwrap();
        for side in ["l", "r"] {
            let turn = |name: &str| bone(pose, &format!("{name}_{side}")).1;
            let thigh = turn("upper_leg");
            assert!((thigh * Vec3::NEG_Y).y < -0.7, "{side} thigh {thigh:?}");
            assert!(
                (thigh * Vec3::X).y.abs() < 0.4,
                "{side} thigh hinge {thigh:?}"
            );
            let shin = thigh.inverse() * turn("lower_leg") * Vec3::NEG_Y;
            assert!(shin.z > -0.02 && shin.x.abs() < 0.02, "{side} knee {shin}");
            let fore = turn("upper_arm").inverse() * turn("lower_arm") * Vec3::NEG_Y;
            assert!(fore.z < 0.02 && fore.x.abs() < 0.02, "{side} elbow {fore}");
            let toes = turn("foot") * Vec3::NEG_Z;
            assert!(toes.y.abs() < 0.2, "{side} foot {toes}");
        }
    }
    // Through a fight (the gun fires, blades swing, guards rise, fighters stumble and fall):
    // the limbs, the head and the trunk are where the stand-in has them, within 3 mm, and the
    // chains hold together (each joint at the end of the segment before it).
    let segments = [
        ("upper_leg", 0u8, 0.45f32),
        ("lower_leg", 1, 0.44),
        ("upper_arm", 2, 0.30),
        ("lower_arm", 3, 0.26),
    ];
    let mut worst = 0.0f32;
    let mut checked = 0;
    for step in 0..60 * 8 {
        app.update();
        if step % 5 != 0 {
            continue;
        }
        for e in [a, b, c] {
            let pose = app.world.get::<SkeletonPose>(e).unwrap();
            let posed = parts(&app, e);
            let part = |p: Part| posed.iter().find(|q| q.0 == p).map(|q| q.1).unwrap();
            let mut near = |what: &str, got: Vec3, want: Vec3| {
                let d = got.distance(want);
                worst = worst.max(d);
                assert!(d < 0.003, "{what}: {got} vs {want} ({d} m) at step {step}");
            };
            for (k, side) in ["l", "r"].into_iter().enumerate() {
                for (name, n, len) in segments {
                    let (at, rot) = bone(pose, &format!("{name}_{side}"));
                    let t = part(Part::Fighter(20 + n * 2 + k as u8));
                    near(
                        name,
                        at + rot * Vec3::new(0.0, -len * 0.5, 0.0),
                        t.translation.as_vec3(),
                    );
                    assert!(rot.angle_between(t.rotation) < 0.01, "{name}_{side} turned");
                }
                let end = |name: &str, len: f32| {
                    let (at, rot) = bone(pose, &format!("{name}_{side}"));
                    at + rot * Vec3::new(0.0, -len, 0.0)
                };
                near(
                    "knee",
                    bone(pose, &format!("lower_leg_{side}")).0,
                    end("upper_leg", 0.45),
                );
                near(
                    "ankle",
                    bone(pose, &format!("foot_{side}")).0,
                    end("lower_leg", 0.44),
                );
                near(
                    "elbow",
                    bone(pose, &format!("lower_arm_{side}")).0,
                    end("upper_arm", 0.30),
                );
                near(
                    "wrist",
                    bone(pose, &format!("hand_{side}")).0,
                    end("lower_arm", 0.26),
                );
                let (hips, turn) = bone(pose, "hips");
                let x = if k == 0 { -0.095 } else { 0.095 };
                let hip = hips + turn * Vec3::new(x, -0.07, 0.0);
                near("hip", bone(pose, &format!("upper_leg_{side}")).0, hip);
            }
            let (head, turn) = bone(pose, "head");
            let skull = part(Part::Fighter(19));
            near(
                "head",
                head + turn * Vec3::new(0.0, 0.05, 0.0),
                skull.translation.as_vec3(),
            );
            assert!(turn.angle_between(skull.rotation) < 0.01, "head turned");
            let (chest, turn) = bone(pose, "chest");
            let trunk = part(Part::Fighter(18));
            near(
                "chest",
                chest + turn * Vec3::new(0.0, 0.02, 0.0),
                trunk.translation.as_vec3(),
            );
            checked += 1;
        }
    }
    let fought = [a, b, c]
        .iter()
        .filter(|&&e| {
            let s = app.world.get::<NpcCombat>(e).unwrap().stats;
            s.hits_taken + s.shots + s.attacks > 0
        })
        .count();
    assert!(fought >= 2, "nobody fought");
    assert!(
        checked > 200 && worst < 0.003,
        "{checked} poses, worst {worst} m"
    );
}

#[test]
fn fighters_ask_for_a_look_and_hide_their_body_only_while_one_is_drawn() {
    let mut app = app();
    let e = fighter(
        &mut app,
        DVec3::ZERO,
        Vec3::NEG_Z,
        Faction::ENEMY,
        NpcWeapon::sword_and_shield(),
    );
    let mate = fighter(
        &mut app,
        DVec3::new(2.0, 0.0, 0.0),
        Vec3::NEG_Z,
        Faction::ENEMY,
        NpcWeapon::rifle(),
    );
    run(&mut app, 0.5);
    // The look asked for: its own seed, its squad's family, a sex and an age range.
    let ask = app
        .world
        .get::<AppearanceRequest>(e)
        .expect("a request")
        .clone();
    assert_eq!(ask.seed, app.world.get::<Seed>(e).unwrap().0);
    let other = app.world.get::<AppearanceRequest>(mate).unwrap();
    assert!(
        ask.family.is_some() && ask.family == other.family,
        "one family per squad"
    );
    assert_ne!(ask.seed, other.seed);
    assert!(ask.sex.is_some_and(|s| (0.0..=1.0).contains(&s)));
    let [young, old] = ask.age.expect("an age range");
    assert!((16.0..young + 1.0).contains(&young) && old > young && old < 70.0);

    let shown = |app: &App, e: Entity| -> (Vec<Part>, Vec<Part>) {
        let (on, off): (Vec<_>, Vec<_>) = parts(app, e).into_iter().partition(|p| p.2);
        (
            on.into_iter().map(|p| p.0).collect(),
            off.into_iter().map(|p| p.0).collect(),
        )
    };
    let body = [Part::Fighter(18), Part::Fighter(19), Part::Fighter(20)];
    let held = [Part::Fighter(28), Part::Fighter(29)];
    let (before, _) = shown(&app, e);
    for p in body.iter().chain(&held) {
        assert!(before.contains(p), "{p:?} shown");
    }
    // A character module comes in later: the body goes, what it holds stays.
    draws_appearances(&mut app, "test");
    run(&mut app, 0.1);
    let (on, off) = shown(&app, e);
    for p in body {
        assert!(off.contains(&p), "{p:?} still shown with appearances drawn");
    }
    for p in held {
        assert!(on.contains(&p), "{p:?} hidden with the body");
    }
    // Its pose and face are still written.
    assert!(app.world.get::<SkeletonPose>(e).is_some());
    assert!(app.world.get::<FaceControls>(e).is_some());
    // Gone again: back as it was.
    app.world.remove_resource::<AppearanceBodies>();
    run(&mut app, 0.1);
    let (on, _) = shown(&app, e);
    assert_eq!(on.len(), before.len(), "{on:?} vs {before:?}");
}

#[test]
fn face_controls_follow_pain_and_effort() {
    let mut app = app();
    let e = held_still(&mut app);
    run(&mut app, 1.0);
    let face = |app: &App| app.world.get::<FaceControls>(e).expect("a face").clone();
    let sneer = |app: &App| face(app).get("noseSneerLeft");
    let brow = |app: &App| face(app).get("browDownLeft");
    assert_eq!(face(&app).weights.len(), 52);
    let rest = (sneer(&app), brow(&app));
    assert!(rest.0 < 0.02 && rest.1 < 0.05, "at rest {rest:?}");

    // Pain: a blow screws the face up at once; it eases to the ache of the wound, which is
    // worse badly hurt.
    hit(&mut app, e, 30.0, DamageKind::Slash);
    run(&mut app, 0.1);
    let struck = sneer(&app);
    assert!(struck > 0.5, "struck {struck}");
    assert!(face(&app).get("eyeSquintLeft") > 0.5);
    run(&mut app, 1.5);
    let aching = sneer(&app);
    assert!(
        aching > 0.02 && aching < struck * 0.5,
        "aching {aching} after {struck}"
    );
    hit(&mut app, e, 45.0, DamageKind::Slash);
    run(&mut app, 1.6);
    let badly = sneer(&app);
    assert!(badly > 2.0 * aching, "badly hurt {badly} vs {aching}");
}

#[test]
fn face_controls_follow_effort_and_panting() {
    let mut app = app();
    let e = held_still(&mut app);
    run(&mut app, 1.0);
    let face = |app: &App| app.world.get::<FaceControls>(e).expect("a face").clone();
    let rest = face(&app).get("browDownLeft");
    // Sprinting: brows down, eyes narrowed; afterwards the effort eases off and it pants.
    let mut jaw_rest = 0.0f32;
    for _ in 0..60 {
        app.update();
        jaw_rest = jaw_rest.max(face(&app).get("jawOpen"));
    }
    let mut sprinting = 0.0f32;
    for _ in 0..60 * 4 {
        app.world.entity_mut(e).insert(MoveIntent {
            direction: Vec3::NEG_Z,
            sprint: true,
            ..Default::default()
        });
        app.update();
        sprinting = sprinting.max(face(&app).get("browDownLeft"));
    }
    app.world.entity_mut(e).insert(MoveIntent::default());
    run(&mut app, 1.5);
    let after = face(&app).get("browDownLeft");
    let mut panting = 0.0f32;
    for _ in 0..90 {
        app.update();
        panting = panting.max(face(&app).get("jawOpen"));
    }
    assert!(rest < 0.05, "at rest {rest}");
    assert!(sprinting > 0.3, "sprinting {sprinting}");
    assert!(after < sprinting * 0.6, "after {after} vs {sprinting}");
    assert!(
        panting > jaw_rest + 0.15,
        "panting {panting} vs {jaw_rest} at rest"
    );
}

#[test]
fn blows_and_falls_wear_the_look() {
    let mut app = app();
    let e = held_still(&mut app);
    app.world.entity_mut(e).insert(Appearance::default());
    run(&mut app, 0.5);
    let wear = |app: &App| app.world.get::<Appearance>(e).unwrap().wear;
    assert_eq!(wear(&app), Default::default());
    hit(&mut app, e, 30.0, DamageKind::Slash);
    run(&mut app, 0.2);
    let cut = wear(&app);
    assert!(cut.blood > 0.25 && cut.bruises < cut.blood, "{cut:?}");
    hit(&mut app, e, 20.0, DamageKind::Blunt);
    run(&mut app, 0.2);
    let bashed = wear(&app);
    assert!(bashed.bruises > cut.bruises + 0.2, "{bashed:?}");
    assert_eq!(bashed.dirt, 0.0);
    hit(&mut app, e, 100.0, DamageKind::Slash);
    run(&mut app, 2.0);
    let down = wear(&app);
    assert!(app.world.get::<NpcVitals>(e).unwrap().downed);
    assert!(down.dirt > 0.3 && down.blood >= bashed.blood, "{down:?}");
    assert_eq!(down.wet, 0.0);
}

#[test]
fn each_footstep_is_an_animation_event_on_its_foot() {
    let mut app = app();
    let e = held_still(&mut app);
    run(&mut app, 0.5);
    let mut steps = app.world.resource::<Messages<GroundContact>>().get_cursor();
    let mut events = app
        .world
        .resource::<Messages<AnimationEvent>>()
        .get_cursor();
    let (mut contacts, mut seen) = (0, Vec::new());
    for _ in 0..60 * 3 {
        app.world.entity_mut(e).insert(MoveIntent {
            direction: Vec3::NEG_Z * 0.5,
            ..Default::default()
        });
        app.update();
        let msgs = app.world.resource::<Messages<GroundContact>>();
        contacts += steps.read(msgs).filter(|c| c.entity == Some(e)).count();
        let msgs = app.world.resource::<Messages<AnimationEvent>>();
        seen.extend(
            events
                .read(msgs)
                .filter(|a| a.entity == e)
                .map(|a| (a.name.clone(), a.bone.clone())),
        );
    }
    assert!(contacts >= 4, "{contacts} steps");
    assert_eq!(seen.len(), contacts);
    for pair in seen.windows(2) {
        assert_ne!(pair[0].0, pair[1].0, "feet alternate");
    }
    for (name, bone) in &seen {
        let want = match name.as_str() {
            "footstep_left" => "foot_l",
            "footstep_right" => "foot_r",
            other => panic!("{other}"),
        };
        assert_eq!(bone.as_deref(), Some(want));
    }
}

#[test]
fn fighters_ask_for_no_looks_by_default() {
    let mut app = App::new();
    app.world
        .resource_mut::<Time>()
        .set_source(TimeSource::Manual {
            step_secs: 1.0 / 60.0,
        });
    app.add_plugins((WorldPlugin, NpcPlugin));
    let e = fighter(
        &mut app,
        DVec3::ZERO,
        Vec3::NEG_Z,
        Faction::ENEMY,
        NpcWeapon::sword_and_shield(),
    );
    draws_appearances(&mut app, "test");
    run(&mut app, 0.5);
    assert!(app.world.get::<AppearanceRequest>(e).is_none());
    // With no look asked for, the fighter keeps its own body even while people are drawn.
    let shown: Vec<Part> = parts(&app, e)
        .into_iter()
        .filter(|p| p.2)
        .map(|p| p.0)
        .collect();
    for p in [Part::Fighter(18), Part::Fighter(19), Part::Fighter(20)] {
        assert!(shown.contains(&p), "{p:?} hidden without a look asked for");
    }
}
