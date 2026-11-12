//! The rules in a running world, without an animation module: blows written as the animation
//! module writes them are judged, sent on as `BodyHit`s and kept as wounds; a ruined weapon arm
//! empties its hand; the camera's centre picks the part a look-aiming fighter strikes.

use engine_combat::{CombatLog, CombatPlugin, CombatSettings, LookAim};
use engine_core::prelude::*;
use engine_core::time::TimeSource;
use engine_render::Camera;
use engine_world::WorldPlugin;
use engine_world::animation::StrikeTargets;
use engine_world::combat::{BodyHit, BodyInjuries, BodyZone, CombatSkills, ItemClass, Loadout};
use engine_world::{DamageEvent, DamageKind, Interactor};

fn app() -> App {
    let mut app = App::new();
    app.world
        .resource_mut::<Time>()
        .set_source(TimeSource::Manual {
            step_secs: 1.0 / 60.0,
        });
    app.add_plugins((WorldPlugin, CombatPlugin));
    app
}

fn fighter(app: &mut App, x: f64, yaw: f32, main: ItemClass, skill: f32) -> Entity {
    let t = Transform::from_translation(DVec3::new(x, 0.0, 0.0))
        .with_rotation(Quat::from_rotation_y(yaw.to_radians()));
    app.world
        .spawn((
            t,
            GlobalTransform::from(t),
            Loadout {
                main,
                off: ItemClass::None,
                two_handed: main.needs_both_hands(),
            },
            CombatSkills {
                weapons: [(main, skill)].into_iter().collect(),
                off_hand: 0.3,
            },
            StrikeTargets::default(),
        ))
        .id()
}

/// Two fighters 1.5 m apart along X facing each other: `a` at -0.75 faces +X.
fn pair(app: &mut App, skill: f32, away: bool) -> (Entity, Entity) {
    let a = fighter(app, -0.75, -90.0, ItemClass::TwoHandedBlade, skill);
    let b = fighter(
        app,
        0.75,
        if away { -90.0 } else { 90.0 },
        ItemClass::OneHandedBlade,
        0.1,
    );
    app.world.get_mut::<StrikeTargets>(a).unwrap().target = Some(b);
    app.update();
    (a, b)
}

/// A point on `b`'s part, world space (b's model space: it faces -Z, its right is +X).
fn on(app: &App, b: Entity, local: Vec3) -> DVec3 {
    let gt = app.world.get::<GlobalTransform>(b).unwrap();
    gt.translation() + (gt.rotation() * local).as_dvec3()
}

fn blow(app: &mut App, a: Entity, b: Entity, local: Vec3, amount: f32, kind: DamageKind) {
    let point = on(app, b, local);
    app.world.write_message(DamageEvent {
        target: b,
        amount,
        kind,
        point,
        impulse: Vec3::X * 40.0,
        source: Some(a),
    });
    app.update();
}

/// The hits still kept (this update's and the last).
fn hits(app: &App) -> Vec<BodyHit> {
    let m = app.world.resource::<Messages<BodyHit>>();
    m.get_cursor().read(m).cloned().collect()
}

/// The damage amounts still kept, oldest first.
fn judged(app: &App) -> Vec<f32> {
    let m = app.world.resource::<Messages<DamageEvent>>();
    m.get_cursor().read(m).map(|d| d.amount).collect()
}

fn last_judged(app: &App) -> f32 {
    *judged(app).last().expect("a damage event")
}

#[test]
fn a_blow_is_judged_on_the_part_it_crossed() {
    let mut app = app();
    let (a, b) = pair(&mut app, 0.9, false);
    // Across the throat (b's neck, at the front).
    blow(
        &mut app,
        a,
        b,
        Vec3::new(0.0, 1.52, -0.05),
        20.0,
        DamageKind::Slash,
    );
    let h = hits(&app);
    assert_eq!(h.len(), 1, "{h:?}");
    let v = app
        .world
        .resource::<CombatLog>()
        .records
        .back()
        .unwrap()
        .clone();
    assert_eq!(v.struck, BodyZone::Neck);
    assert_eq!(h[0].zone, v.verdict.zone);
    let amount = last_judged(&app);
    assert_eq!(
        amount, v.verdict.amount,
        "the event carries the judged amount"
    );
    let hurt = app.world.get::<BodyInjuries>(b).expect("the wound is kept");
    assert!(hurt.get(v.verdict.zone).wound > 0.0);
    // On the shin the same swing does far less (unless it was covered on a thigh).
    blow(
        &mut app,
        a,
        b,
        Vec3::new(0.1, 0.25, -0.05),
        20.0,
        DamageKind::Slash,
    );
    let shin = last_judged(&app);
    assert!(shin < 20.0, "{shin}");
}

#[test]
fn cuts_behind_the_knees_take_the_legs_and_hard_cuts_take_the_head() {
    let mut app = app();
    let (a, b) = pair(&mut app, 1.0, true);
    // b faces away from a: its back is toward -X; a cut at its knee from behind.
    blow(
        &mut app,
        a,
        b,
        Vec3::new(-0.1, 0.5, 0.06),
        18.0,
        DamageKind::Slash,
    );
    let r = app
        .world
        .resource::<CombatLog>()
        .records
        .back()
        .unwrap()
        .clone();
    assert_eq!(r.approach, engine_world::combat::Approach::Back);
    assert!(r.verdict.hamstring, "{r:?}");
    let knee = *app
        .world
        .get::<BodyInjuries>(b)
        .unwrap()
        .get(BodyZone::KneeL);
    assert!(knee.wound >= engine_combat::rules::LEG_GIVES_OUT);
    // A master's heavy two-handed cut through the neck kills.
    let mut hard = DamageEvent {
        target: b,
        amount: 32.0,
        kind: DamageKind::Slash,
        point: on(&app, b, Vec3::new(0.0, 1.52, 0.05)),
        impulse: Vec3::X * 90.0,
        source: Some(a),
    };
    app.world.write_message(hard.clone());
    app.update();
    let neck = *app
        .world
        .get::<BodyInjuries>(b)
        .unwrap()
        .get(BodyZone::Neck);
    assert!(neck.severed, "{neck:?}");
    assert!(last_judged(&app) >= engine_combat::rules::KILLING_BLOW);
    // With gore limited to limbs, heads stay on.
    let (a2, b2) = pair(&mut app, 1.0, true);
    app.world.resource_mut::<CombatSettings>().gore = engine_combat::Gore::Limbs;
    hard.target = b2;
    hard.source = Some(a2);
    hard.point = on(&app, b2, Vec3::new(0.0, 1.52, 0.05));
    app.world.write_message(hard);
    app.update();
    assert!(
        !app.world
            .get::<BodyInjuries>(b2)
            .unwrap()
            .get(BodyZone::Neck)
            .severed
    );
}

#[test]
fn a_ruined_sword_arm_drops_the_sword() {
    let mut app = app();
    let (a, b) = pair(&mut app, 1.0, false);
    for _ in 0..4 {
        blow(
            &mut app,
            a,
            b,
            Vec3::new(0.25, 1.0, -0.03),
            20.0,
            DamageKind::Slash,
        );
    }
    app.update();
    let hurt = *app.world.get::<BodyInjuries>(b).unwrap();
    let arm = hurt.get(BodyZone::ForearmR);
    assert!(arm.wound >= 0.6 || arm.severed, "{arm:?}");
    assert_eq!(app.world.get::<Loadout>(b).unwrap().main, ItemClass::None);
}

#[test]
fn blows_without_an_armed_attacker_or_on_things_are_left_alone() {
    let mut app = app();
    let (_, b) = pair(&mut app, 0.5, false);
    let rock = app
        .world
        .spawn((Transform::default(), GlobalTransform::default()))
        .id();
    app.world.write_message(DamageEvent {
        target: b,
        amount: 15.0,
        kind: DamageKind::Fire,
        point: on(&app, b, Vec3::new(0.0, 1.5, 0.0)),
        impulse: Vec3::ZERO,
        source: None,
    });
    app.world.write_message(DamageEvent {
        target: rock,
        amount: 15.0,
        kind: DamageKind::Slash,
        point: DVec3::ZERO,
        impulse: Vec3::ZERO,
        source: Some(b),
    });
    app.update();
    assert!(hits(&app).is_empty());
    assert_eq!(judged(&app), vec![15.0, 15.0]);
}

#[test]
fn the_player_aims_where_the_camera_looks() {
    let mut app = app();
    let (a, b) = pair(&mut app, 0.8, false);
    app.world.entity_mut(a).insert(Interactor);
    // A camera over a's shoulder looking at b's left knee (b faces -X: its left is -Z).
    let knee = on(&app, b, Vec3::new(-0.1, 0.5, 0.0));
    let eye = DVec3::new(-2.0, 1.6, 0.3);
    let cam = Transform::from_translation(eye).looking_at(knee, Vec3::Y);
    app.world
        .spawn((Camera::perspective(50.0), cam, GlobalTransform::from(cam)));
    for _ in 0..3 {
        app.update();
    }
    let st = app.world.get::<StrikeTargets>(a).unwrap();
    assert_eq!(st.zone, Some(BodyZone::KneeL), "{st:?}");
    assert!(app.world.get::<LookAim>(a).unwrap().on_body);
    // Now at the neck: the pick waits out its hold, then follows.
    let neck = on(&app, b, Vec3::new(0.0, 1.53, 0.0));
    let cam = Transform::from_translation(eye).looking_at(neck, Vec3::Y);
    let mut q = app.world.query_filtered::<Entity, With<Camera>>();
    let c = q.single(&app.world).unwrap();
    app.world
        .entity_mut(c)
        .insert((cam, GlobalTransform::from(cam)));
    for _ in 0..12 {
        app.update();
    }
    assert_eq!(
        app.world.get::<StrikeTargets>(a).unwrap().zone,
        Some(BodyZone::Neck)
    );
    // Looking at the sky picks the chest, marked off the body.
    let cam = Transform::from_translation(eye).looking_at(eye + DVec3::Y, Vec3::X);
    app.world
        .entity_mut(c)
        .insert((cam, GlobalTransform::from(cam)));
    for _ in 0..12 {
        app.update();
    }
    assert_eq!(
        app.world.get::<StrikeTargets>(a).unwrap().zone,
        Some(BodyZone::Chest)
    );
    assert!(!app.world.get::<LookAim>(a).unwrap().on_body);
}

#[test]
fn a_melee_contact_becomes_one_judged_damage_event() {
    use engine_world::{MeleeContact, MeleeJudge};
    let mut app = app();
    assert_eq!(
        app.world.resource::<MeleeJudge>().judged_by,
        "engine_combat"
    );
    let (a, b) = pair(&mut app, 0.9, false);
    let point = on(&app, b, Vec3::new(0.0, 1.52, -0.05));
    app.world.write_message(MeleeContact {
        blow: DamageEvent {
            target: b,
            amount: 20.0,
            kind: DamageKind::Slash,
            point,
            impulse: Vec3::X * 40.0,
            source: Some(a),
        },
        item: ItemClass::TwoHandedBlade,
        off_hand: false,
        entry: None,
        edge_dir: None,
        bone: None,
    });
    app.update();
    app.update();
    let log = app.world.resource::<CombatLog>();
    assert_eq!(log.total, 1, "judged once, not again as damage");
    let verdict = log.records.back().unwrap().verdict;
    assert_eq!(judged(&app), vec![verdict.amount]);
    assert_eq!(hits(&app).len(), 1);
}

#[test]
fn an_open_wound_bleeds_without_flinching_and_clots() {
    let mut app = app();
    let (a, b) = pair(&mut app, 0.9, true);
    // A cut across the back of the thigh, from behind: it can't be covered.
    blow(
        &mut app,
        a,
        b,
        Vec3::new(0.12, 0.6, 0.08),
        20.0,
        DamageKind::Slash,
    );
    let start: f32 = BodyZone::ALL
        .iter()
        .map(|&z| app.world.get::<BodyInjuries>(b).unwrap().get(z).bleeding)
        .sum();
    assert!(start > 0.0, "the cut bleeds");
    let mut bled = 0.0;
    for _ in 0..(60 * 10) {
        app.update();
        let m = app.world.resource::<Messages<DamageEvent>>();
        bled += m
            .iter_current_update_messages()
            .filter(|d| d.kind == DamageKind::Bleed && d.target == b)
            .inspect(|d| assert!(d.source.is_none() && d.impulse == Vec3::ZERO))
            .map(|d| d.amount)
            .sum::<f32>();
    }
    let now: f32 = BodyZone::ALL
        .iter()
        .map(|&z| app.world.get::<BodyInjuries>(b).unwrap().get(z).bleeding)
        .sum();
    assert!(bled >= 1.0, "ten seconds of bleeding cost health: {bled}");
    assert!(now < start * 0.8, "and it clots: {start} -> {now}");
    // No body part was struck again by the bleeding.
    assert_eq!(app.world.resource::<CombatLog>().total, 1);
}
