//! Close-up renders of the procedural fighters in posed lineups (guard, block, telegraph, aim,
//! draw, walk, kneel, downed, getting up, wounded), to check the art and the rig. Needs a GPU or a software Vulkan
//! driver (lavapipe); passes with a note when neither is available (`RenderDisabled`).
//!
//! The fighters here have no AI: their combat state is set by hand and the NPC clock stands
//! still at 0. The PNGs land in the test's temp dir; set `NPC_SCREENSHOT_DIR` to also copy
//! them there.

use std::f32::consts::{FRAC_PI_2, PI};

use engine_core::prelude::*;
use engine_core::time::TimeSource;
use engine_npc::*;
use engine_render::prelude::*;
use engine_render::{RenderDisabled, capture_now};
use engine_world::locomotion::{LocomotionMode, LocomotionState};
use engine_world::{AttackEvent, AttackKind, DamageEvent, DamageKind, WorldPlugin};

const W: u32 = 1280;
const H: u32 = 720;
/// The ground of the close-ups.
const EARTH: u32 = 0x8c7a5b;
/// The arena's sand.
const SAND: u32 = 0xf2dfb8;

fn app() -> App {
    app_on(W, H, EARTH)
}

/// An app rendering `w` × `h` pixels, with the sun up and a plane of `ground` color.
fn app_on(w: u32, h: u32, ground: u32) -> App {
    let mut app = App::new();
    app.world.resource_mut::<EngineConfig>().headless = true;
    {
        let mut win = app.world.resource_mut::<Window>();
        win.width = w;
        win.height = h;
    }
    app.world
        .resource_mut::<Time>()
        .set_source(TimeSource::Manual { step_secs: 0.05 });
    app.add_plugins((WorldPlugin, RenderPlugin, NpcPlugin));
    app.world.spawn((
        DirectionalLight {
            illuminance: 90_000.0,
            ..Default::default()
        },
        Transform::default().looking_at(DVec3::new(-0.6, -0.6, 0.55), Vec3::Y),
    ));
    app.world.spawn((
        Transform::default(),
        MeshRenderer::new(
            Shape::Plane {
                size: [80.0, 80.0],
                subdivisions: 1,
            },
            StandardMaterial::hex(ground).with_roughness(0.95),
        ),
    ));
    app
}

/// One posed fighter.
struct Pose {
    at: DVec3,
    yaw: f32,
    faction: u32,
    color: Option<u32>,
    weapon: NpcWeapon,
    skills: NpcSkills,
    alert: AlertLevel,
    phase: AttackPhase,
    defense: Defense,
    loco: LocomotionState,
    gait: f32,
    downed: bool,
    /// Just hit (flashing).
    hurt: bool,
}

impl Pose {
    fn new(x: f64, faction: u32, weapon: NpcWeapon) -> Self {
        Self {
            at: DVec3::new(x, 0.0, 0.0),
            yaw: 0.0,
            faction,
            color: None,
            weapon,
            skills: NpcSkills::veteran(),
            alert: AlertLevel::Combat,
            phase: AttackPhase::Ready,
            defense: Defense::None,
            loco: LocomotionState::default(),
            gait: 0.0,
            downed: false,
            hurt: false,
        }
    }
}

/// A shot aimed (or an arrow drawn) to go off at `hit_at`.
fn windup_at(hit_at: f64) -> AttackPhase {
    windup(hit_at, false)
}

fn windup(hit_at: f64, heavy: bool) -> AttackPhase {
    AttackPhase::Windup {
        target: None,
        hit_at,
        heavy,
        feint: false,
        aim: DVec3::ZERO,
    }
}

fn spawn(app: &mut App, p: Pose) -> Entity {
    let mut mover = MoverState::default();
    mover.phase = p.gait;
    let mut e = app.world.spawn((
        Transform::from_translation(p.at).with_rotation(Quat::from_rotation_y(p.yaw)),
        StandIn {
            color: p.color,
            plain: false,
        },
        Faction(p.faction),
        p.weapon,
        p.skills,
        p.loco,
        mover,
        NpcBrain {
            alert: p.alert,
            ..Default::default()
        },
        NpcCombat {
            phase: p.phase,
            defense: p.defense,
            // The clock stands at 0: a hit "at" 0.05 s is under the flash's 0.12 s.
            last_hurt: if p.hurt { 0.05 } else { 0.0 },
            ..Default::default()
        },
    ));
    if p.downed {
        e.insert((
            NpcVitals {
                downed: true,
                ..Default::default()
            },
            Downed { since: -2.0 },
        ));
    }
    e.id()
}

fn camera(app: &mut App, eye: DVec3, target: DVec3, fov: f32) -> Entity {
    app.world
        .spawn((
            Camera {
                active: false,
                ..Camera::perspective(fov)
            },
            Transform::from_translation(eye).looking_at(target, Vec3::Y),
        ))
        .id()
}

fn shoot(app: &mut App, cam: Entity, name: &str) -> Option<image::RgbaImage> {
    let mut q = app.world.query::<(Entity, &mut Camera)>();
    for (e, mut c) in q.iter_mut(&mut app.world) {
        c.active = e == cam;
    }
    app.update_n(3);
    if let Some(d) = app.world.get_resource::<RenderDisabled>() {
        eprintln!("skipping: no GPU ({})", d.0);
        return None;
    }
    let path = std::path::Path::new(env!("CARGO_TARGET_TMPDIR")).join(name);
    capture_now(&mut app.world, &path).expect("screenshot");
    if let Ok(dir) = std::env::var("NPC_SCREENSHOT_DIR") {
        let _ = std::fs::create_dir_all(&dir);
        std::fs::copy(&path, std::path::Path::new(&dir).join(name)).expect("copy screenshot");
    }
    eprintln!("wrote {}", path.display());
    Some(image::open(&path).expect("png").to_rgba8())
}

#[test]
fn fighter_closeups() {
    let mut app = app();
    let walk = LocomotionState {
        mode: LocomotionMode::Walk,
        velocity: Vec3::new(0.0, 0.0, -1.4),
        grounded: true,
        ..Default::default()
    };
    let run = LocomotionState {
        mode: LocomotionMode::Run,
        velocity: Vec3::new(0.0, 0.0, -4.6),
        grounded: true,
        ..Default::default()
    };
    let kneel = LocomotionState {
        mode: LocomotionMode::Crouch,
        grounded: true,
        ..Default::default()
    };
    let poses = vec![
        Pose {
            skills: NpcSkills::master(),
            ..Pose::new(-6.0, 1, NpcWeapon::sword())
        },
        Pose {
            defense: Defense::Block { until: 5.0 },
            ..Pose::new(-4.5, 2, NpcWeapon::sword_and_shield())
        },
        Pose {
            phase: windup(0.2, false),
            skills: NpcSkills::elite(),
            ..Pose::new(-3.0, 1, NpcWeapon::rifle())
        },
        Pose {
            phase: windup(0.05, false),
            ..Pose::new(-1.5, 2, NpcWeapon::bow())
        },
        Pose {
            color: Some(0x6b3fa0),
            phase: windup(0.1, true),
            skills: NpcSkills::master(),
            ..Pose::new(0.0, 21, NpcWeapon::sword_and_shield())
        },
        Pose {
            color: Some(0x2f8f86),
            alert: AlertLevel::Relaxed,
            ..Pose::new(1.5, 22, NpcWeapon::sword())
        },
        Pose {
            alert: AlertLevel::Suspicious,
            loco: walk,
            gait: 0.8,
            skills: NpcSkills::rookie(),
            ..Pose::new(3.0, 23, NpcWeapon::sword_and_shield())
        },
        Pose {
            loco: kneel,
            ..Pose::new(4.5, 1, NpcWeapon::rifle())
        },
        Pose {
            alert: AlertLevel::Searching,
            loco: run,
            gait: 2.4,
            ..Pose::new(6.0, 2, NpcWeapon::bow())
        },
        Pose {
            downed: true,
            ..Pose::new(7.8, 1, NpcWeapon::sword())
        },
        Pose {
            phase: AttackPhase::Recovery { until: 0.35 },
            ..Pose::new(-6.0, 2, NpcWeapon::sword())
        },
    ];
    let mut poses = poses;
    // The last one stands behind the first, striking.
    poses.last_mut().unwrap().at = DVec3::new(-6.0, 0.0, 2.5);
    for p in poses {
        spawn(&mut app, p);
    }
    let lineup = camera(
        &mut app,
        DVec3::new(0.8, 1.6, -9.5),
        DVec3::new(0.8, 0.9, 0.0),
        60.0,
    );
    let left = camera(
        &mut app,
        DVec3::new(-4.6, 1.5, -3.6),
        DVec3::new(-4.6, 1.0, 0.0),
        50.0,
    );
    let right = camera(
        &mut app,
        DVec3::new(3.2, 1.3, -3.4),
        DVec3::new(3.2, 0.9, 0.0),
        55.0,
    );
    let duelist = camera(
        &mut app,
        DVec3::new(-0.9, 1.65, -2.1),
        DVec3::new(0.05, 1.25, 0.0),
        40.0,
    );
    let face = camera(
        &mut app,
        DVec3::new(-5.55, 1.7, -0.95),
        DVec3::new(-6.0, 1.55, 0.0),
        35.0,
    );
    let portrait = camera(
        &mut app,
        DVec3::new(-5.72, 1.6, -0.78),
        DVec3::new(-5.98, 1.55, 0.0),
        26.0,
    );
    let downed = camera(
        &mut app,
        DVec3::new(9.6, 1.6, -1.2),
        DVec3::new(7.9, 0.2, 0.8),
        50.0,
    );
    let back = camera(
        &mut app,
        DVec3::new(-2.0, 1.5, 3.2),
        DVec3::new(-2.6, 1.0, 0.0),
        55.0,
    );
    app.update_n(2);
    if app.world.get_resource::<RenderDisabled>().is_some() {
        eprintln!("skipping: no GPU");
        return;
    }
    let Some(img) = shoot(&mut app, lineup, "npc_fighters_lineup.png") else {
        return;
    };
    let red = img
        .pixels()
        .filter(|p| p[0] as f32 > 70.0 && p[0] as f32 > p[1] as f32 * 1.7)
        .count();
    assert!(red > 500, "red fighters visible: {red} px");
    shoot(&mut app, left, "npc_fighters_melee_ranged.png");
    shoot(&mut app, right, "npc_fighters_moving.png");
    shoot(&mut app, duelist, "npc_fighters_duelist.png");
    shoot(&mut app, face, "npc_fighters_face.png");
    shoot(&mut app, portrait, "npc_fighters_portrait.png");
    shoot(&mut app, downed, "npc_fighters_downed.png");
    shoot(&mut app, back, "npc_fighters_back.png");
}

/// One squad side by side, close: each fighter's face (skin tone, shape and eye color) comes
/// from its own seed, so squad members look like different people.
#[test]
fn fighter_faces() {
    let mut app = app();
    let squad = [
        (NpcWeapon::claws(), NpcSkills::master()),
        (NpcWeapon::sword(), NpcSkills::rookie()),
        (NpcWeapon::sword(), NpcSkills::veteran()),
        (NpcWeapon::sword_and_shield(), NpcSkills::elite()),
    ];
    for (i, (weapon, skills)) in squad.into_iter().enumerate() {
        spawn(
            &mut app,
            Pose {
                skills,
                alert: AlertLevel::Relaxed,
                ..Pose::new(-0.9 + 0.6 * i as f64, 1, weapon)
            },
        );
    }
    let cam = camera(
        &mut app,
        DVec3::new(0.0, 1.7, -3.6),
        DVec3::new(0.0, 1.5, 0.0),
        22.0,
    );
    app.update_n(2);
    if app.world.get_resource::<RenderDisabled>().is_some() {
        eprintln!("skipping: no GPU");
        return;
    }
    shoot(&mut app, cam, "npc_fighters_squad_faces.png");
}

/// Fresh, worn and battered paint side by side: a rookie, a soldier and a master. Their plates
/// chip to bare metal along the rims and ridges, scratch, dent and gather dirt with skill.
#[test]
fn fighter_armor_wear() {
    let mut app = app();
    let squad = [
        (NpcWeapon::sword_and_shield(), NpcSkills::rookie()),
        (NpcWeapon::sword(), NpcSkills::soldier()),
        (NpcWeapon::sword_and_shield(), NpcSkills::master()),
    ];
    for (i, (weapon, skills)) in squad.into_iter().enumerate() {
        spawn(
            &mut app,
            Pose {
                skills,
                alert: AlertLevel::Relaxed,
                ..Pose::new(-0.7 + 0.7 * i as f64, 1, weapon)
            },
        );
    }
    let squad = camera(
        &mut app,
        DVec3::new(0.0, 1.45, -2.6),
        DVec3::new(0.0, 1.2, 0.0),
        30.0,
    );
    let close = camera(
        &mut app,
        DVec3::new(0.95, 1.45, -0.85),
        DVec3::new(0.7, 1.3, 0.0),
        32.0,
    );
    app.update_n(2);
    if app.world.get_resource::<RenderDisabled>().is_some() {
        eprintln!("skipping: no GPU");
        return;
    }
    shoot(&mut app, squad, "npc_fighters_armor_wear.png");
    shoot(&mut app, close, "npc_fighters_armor_closeup.png");
}

/// Pixels that read as the red or the blue team (as the arena screenshots count them).
fn is_red(p: &image::Rgba<u8>) -> bool {
    let [r, g, b] = [p[0], p[1], p[2]].map(f32::from);
    r > 70.0 && r > g * 1.7 && r > b * 1.7
}

fn is_blue(p: &image::Rgba<u8>) -> bool {
    let [r, g, b] = [p[0], p[1], p[2]].map(f32::from);
    b > 70.0 && b > r * 1.5 && b > g * 1.2
}

/// Where `p` lands on a `w` × `h` image seen from `eye` looking at `target` with a vertical
/// field of view of `fov` degrees.
fn project(eye: DVec3, target: DVec3, fov: f64, w: u32, h: u32, p: DVec3) -> (f64, f64) {
    let f = (target - eye).normalize();
    let right = f.cross(DVec3::Y).normalize();
    let up = right.cross(f);
    let d = p - eye;
    let k = f64::from(h) * 0.5 / (fov.to_radians() * 0.5).tan() / d.dot(f);
    (
        f64::from(w) * 0.5 + d.dot(right) * k,
        f64::from(h) * 0.5 - d.dot(up) * k,
    )
}

/// Two squads 33 m off on the sand, seen from as high as the arena overview: every fighter,
/// whichever way it faces and whatever it carries, reads as its team's color.
#[test]
fn fighter_teams_at_distance() {
    let (w, h) = (960, 540);
    let mut app = app_on(w, h, SAND);
    let kit = [
        (NpcWeapon::sword_and_shield(), NpcSkills::rookie()),
        (NpcWeapon::sword(), NpcSkills::soldier()),
        (NpcWeapon::rifle(), NpcSkills::veteran()),
        (NpcWeapon::bow(), NpcSkills::master()),
    ];
    let yaws = [0.0, FRAC_PI_2, PI, -FRAC_PI_2];
    let mut fighters = Vec::new();
    for (team, side) in [(1, -1.0), (2, 1.0)] {
        for (i, (weapon, skills)) in kit.iter().enumerate() {
            let at = DVec3::new(side * (2.0 + 2.2 * i as f64), 0.0, 2.0);
            fighters.push((team, at));
            spawn(
                &mut app,
                Pose {
                    at,
                    yaw: yaws[(i + team as usize) % 4],
                    skills: *skills,
                    ..Pose::new(0.0, team, *weapon)
                },
            );
        }
    }
    // Behind them, one of each team just hit: they flash, tinted, not blown out to white.
    for (team, x) in [(1, -0.7), (2, 0.7)] {
        spawn(
            &mut app,
            Pose {
                at: DVec3::new(x, 0.0, 8.0),
                hurt: true,
                ..Pose::new(0.0, team, NpcWeapon::sword())
            },
        );
    }
    let (eye, target, fov) = (
        DVec3::new(0.0, 16.0, -31.0),
        DVec3::new(0.0, 0.0, 4.0),
        60.0,
    );
    let cam = camera(&mut app, eye, target, fov as f32);
    let struck = camera(
        &mut app,
        DVec3::new(0.0, 2.2, 3.4),
        DVec3::new(0.0, 1.0, 8.0),
        40.0,
    );
    app.update_n(2);
    if app.world.get_resource::<RenderDisabled>().is_some() {
        eprintln!("skipping: no GPU");
        return;
    }
    let Some(img) = shoot(&mut app, cam, "npc_fighters_teams_far.png") else {
        return;
    };
    shoot(&mut app, struck, "npc_fighters_hit_flash.png");
    // Each fighter, a box round it (60 cm to each side, feet to crest): about 6 × 24 pixels.
    let mut squads = [0; 2];
    for (team, at) in fighters {
        let (x0, y1) = project(eye, target, fov, w, h, at + DVec3::new(0.6, 0.0, 0.0));
        let (x1, y0) = project(eye, target, fov, w, h, at + DVec3::new(-0.6, 2.0, 0.0));
        let (mut red, mut blue) = (0, 0);
        for y in y0.floor() as u32..=(y1.ceil() as u32).min(h - 1) {
            for x in x0.floor() as u32..=(x1.ceil() as u32).min(w - 1) {
                let p = img.get_pixel(x, y);
                red += usize::from(is_red(p));
                blue += usize::from(is_blue(p));
            }
        }
        eprintln!(
            "team {team} at x {:+.1}: red {red} px, blue {blue} px",
            at.x
        );
        let (own, other) = if team == 1 { (red, blue) } else { (blue, red) };
        assert!(
            own >= 12 && other <= 2,
            "the team {team} fighter at x {:+.1} shows {own} px of its color, {other} of the other",
            at.x
        );
        squads[team as usize - 1] += own;
    }
    assert!(
        squads.iter().all(|&n| n >= 90),
        "team colors shown by each squad: {squads:?} px"
    );
}

// ---------------------------------------------------------------------------- motion strips

/// When the scripted action starts, on each fighter's own clock.
const EVENT: f64 = 2.0;
/// How long the scripted fighters live before the strips are shot (world seconds).
const WARM: f64 = 1.2;

/// Where a blow comes at a defender from.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Incoming {
    /// A heavy cut from a fighter in front.
    High,
    /// A light cut from a fighter in front (right-handed: it comes in on the defender's left).
    Left,
    /// A light cut from a fighter standing off to the defender's right.
    Right,
    /// A dash attack from in front.
    Thrust,
}

/// What a scripted fighter does, by its own clock (the action starts at `EVENT`).
#[derive(Clone, Copy, Debug)]
enum Scene {
    /// Standing (relaxed or on guard, as its alert level says).
    Stand,
    /// A swing, its telegraph starting at the event.
    Swing { heavy: bool },
    /// The guard raised at the event against a blow landing a moment later.
    Block { parry: bool },
    /// The guard raised at the event against a blow from an (unseen) attacker, telegraphed
    /// 0.3 s before.
    Defend { parry: bool, from: Incoming },
    /// A dash 1.6 m to the left at the event (across the picture, facing the camera), as the
    /// mover dashes.
    Dodge,
    /// A dash attack: 3 m ahead at the event, as the mover dashes, the blow landing after.
    Lunge,
    /// A light swing telegraphed at the event and pulled back before it lands (a feint).
    Feint,
    /// Knocked off balance at the event, for a second.
    Stagger,
    /// Struck at the event `height` m up, on its right (`side` 1) or left (−1), by a blow of
    /// `kind` from in front.
    Hit {
        height: f32,
        side: f32,
        kind: DamageKind,
    },
    /// Cut down at the event by a blow from in front and to its left.
    Fall,
    /// Moving at `vel` (m/s, in its own frame: −Z ahead) the whole time.
    Move { vel: Vec3 },
    /// Moving at `vel` until the event, then stopping dead.
    Stop { vel: Vec3 },
    /// Turning on the spot as fast as a fighter turns (9 rad/s) from the event, by `angle`
    /// (rad, to its left), to end facing its strip's way.
    Turn { angle: f32 },
    /// Shooting as fast as its weapon allows, the `shots`th shot going off at the event.
    Shoot { shots: u32 },
    /// At ease until the event, then a foe in sight (alert `Combat`).
    Alarm,
    /// Lying where it was cut down (long since) until the event, then revived where it lies.
    Revive,
    /// Badly hurt (a quarter of its health left) by a blow long before, to the leg on its
    /// `side` (−1 the left, 1 the right) or, `leg` false, to the body; moving at `vel` (m/s,
    /// own frame: −Z ahead).
    Wounded { vel: Vec3, side: f32, leg: bool },
    /// On guard, its foe straight ahead until the event; then another, `bearing` rad to its
    /// left, becomes its target.
    Look { bearing: f32 },
}

/// A fighter acting out a scene; at its own time `cap` it stands at `at`.
#[derive(Component, Clone, Copy, Debug)]
struct Script {
    scene: Scene,
    weapon: NpcWeapon,
    at: DVec3,
    yaw: f32,
    cap: f64,
    /// The unseen attacker of a `Defend` scene; the foes of a `Look` scene (the first, then the
    /// second).
    foe: Option<Entity>,
    other: Option<Entity>,
    /// The scene's event (attack or blow) was sent.
    fired: bool,
}

/// A dash's progress `t` s in (0..1), as the mover eases it.
fn dash_ease(t: f64) -> f64 {
    let d = (t / 0.22).clamp(0.0, 1.0);
    1.0 - (1.0 - d) * (1.0 - d)
}

type ScriptItem<'a> = (
    Entity,
    &'a mut Script,
    &'a NpcClock,
    &'a mut NpcCombat,
    &'a mut LocomotionState,
    &'a mut MoverState,
    &'a mut Transform,
    Option<&'a mut NpcVitals>,
    &'a mut NpcBrain,
);

/// Play every scripted fighter's scene: combat state, position and motion by its clock.
fn run_scripts(
    mut q: Query<ScriptItem>,
    mut attacks: MessageWriter<AttackEvent>,
    mut blows: MessageWriter<DamageEvent>,
) {
    for (me, mut s, clock, mut combat, mut loco, mut mover, mut t, vitals, mut brain) in &mut q {
        let now = clock.now;
        let since = now - EVENT;
        let turned = match s.scene {
            Scene::Turn { angle } => (9.0 * since.max(0.0) as f32).min(angle) - angle,
            _ => 0.0,
        };
        let rot = Quat::from_rotation_y(s.yaw + turned);
        let ahead = rot * Vec3::NEG_Z;
        let mut at = s.at;
        let mut vel = Vec3::ZERO;
        let mut mode = LocomotionMode::Idle;
        match s.scene {
            Scene::Stand => {}
            Scene::Swing { heavy } => {
                let w = f64::from(s.weapon.windup) * if heavy { 1.6 } else { 1.0 };
                let r = f64::from(s.weapon.recovery);
                combat.phase = if !(0.0..w + r).contains(&since) {
                    AttackPhase::Ready
                } else if since < w {
                    windup(EVENT + w, heavy)
                } else {
                    AttackPhase::Recovery {
                        until: EVENT + w + r,
                    }
                };
            }
            Scene::Block { parry } | Scene::Defend { parry, .. } => {
                let (len, lands) = if parry { (0.28, 0.2) } else { (0.6, 0.25) };
                combat.defense = if !(0.0..len).contains(&since) {
                    Defense::None
                } else if parry {
                    Defense::Parry { until: EVENT + len }
                } else {
                    Defense::Block { until: EVENT + len }
                };
                combat.incoming.clear();
                let attacker = s.foe.unwrap_or(Entity::PLACEHOLDER);
                if (0.0..lands).contains(&since) {
                    combat.incoming.push(engine_npc::agent::IncomingAttack {
                        attacker,
                        lands_at: EVENT + lands,
                        react_at: EVENT,
                        ranged: false,
                        reach: 1.9,
                        handled: true,
                    });
                }
                if let Scene::Defend { from, .. } = s.scene
                    && since >= -0.3
                    && !s.fired
                {
                    s.fired = true;
                    attacks.write(AttackEvent {
                        attacker,
                        target: Some(me),
                        windup_secs: (0.3 + lands) as f32,
                        reach: 1.9,
                        kind: if from == Incoming::Thrust {
                            AttackKind::Dash
                        } else {
                            AttackKind::Melee
                        },
                    });
                }
            }
            Scene::Dodge => {
                let left = rot * Vec3::NEG_X;
                at +=
                    (left * 1.6 * (dash_ease(since) - dash_ease(s.cap - EVENT)) as f32).as_dvec3();
                if (0.0..0.22).contains(&since) {
                    let d = (since / 0.22) as f32;
                    vel = left * 1.6 * 2.0 * (1.0 - d) / 0.22;
                    mode = LocomotionMode::Dash;
                }
                combat.defense = if (0.0..0.45).contains(&since) {
                    Defense::Dodge {
                        until: EVENT + 0.45,
                    }
                } else {
                    Defense::None
                };
            }
            Scene::Lunge => {
                let dist = 3.0;
                at += (ahead * dist * (dash_ease(since) - dash_ease(s.cap - EVENT)) as f32)
                    .as_dvec3();
                if (0.0..0.22).contains(&since) {
                    let d = (since / 0.22) as f32;
                    vel = ahead * dist * 2.0 * (1.0 - d) / 0.22;
                    mode = LocomotionMode::Dash;
                }
                let (w, r) = (0.45, 0.45);
                combat.phase = if !(0.0..w + r).contains(&since) {
                    AttackPhase::Ready
                } else if since < w {
                    windup(EVENT + w, false)
                } else {
                    AttackPhase::Recovery {
                        until: EVENT + w + r,
                    }
                };
            }
            Scene::Feint => {
                // Called off 0.22 s before it would land, as combat does.
                let w = f64::from(s.weapon.windup);
                combat.phase = if (0.0..w - 0.22).contains(&since) {
                    AttackPhase::Windup {
                        target: None,
                        hit_at: EVENT + w,
                        heavy: false,
                        feint: true,
                        aim: DVec3::ZERO,
                    }
                } else {
                    AttackPhase::Ready
                };
            }
            Scene::Stagger => {
                combat.phase = if (0.0..1.0).contains(&since) {
                    AttackPhase::Staggered { until: EVENT + 1.0 }
                } else {
                    AttackPhase::Ready
                };
            }
            Scene::Hit { height, side, kind } => {
                if since >= 0.0 && !s.fired {
                    s.fired = true;
                    combat.last_hurt = now;
                    let right = rot * Vec3::X;
                    blows.write(DamageEvent {
                        target: me,
                        amount: 25.0,
                        kind,
                        point: at
                            + (right * 0.12 * side + ahead * 0.12 + Vec3::Y * height).as_dvec3(),
                        impulse: (-ahead - right * 0.3 * side).normalize() * 75.0,
                        source: None,
                    });
                }
            }
            Scene::Fall => {
                if let Some(mut v) = vitals {
                    v.downed = since >= 0.0;
                }
                if since >= 0.0 && !s.fired {
                    s.fired = true;
                    combat.last_hurt = now;
                    let right = rot * Vec3::X;
                    blows.write(DamageEvent {
                        target: me,
                        amount: 40.0,
                        kind: DamageKind::Slash,
                        point: at + (ahead * 0.12 + Vec3::Y * 1.1).as_dvec3(),
                        impulse: (-ahead + right * 0.35).normalize() * 120.0,
                        source: None,
                    });
                }
            }
            Scene::Turn { .. } => {}
            Scene::Shoot { shots } => {
                let w = &s.weapon;
                let (windup, recovery, cooldown) = (
                    f64::from(w.windup),
                    f64::from(w.recovery),
                    f64::from(w.cooldown),
                );
                let every = windup + cooldown;
                let first = EVENT - f64::from(shots.max(1) - 1) * every;
                let last = first + ((now - first) / every).floor() * every;
                let next = if now < first { first } else { last + every };
                combat.phase = if now >= first && now < last + recovery {
                    AttackPhase::Recovery {
                        until: last + recovery,
                    }
                } else if now >= next - windup {
                    windup_at(next)
                } else {
                    AttackPhase::Ready
                };
                combat.ready_at = if now < first { 0.0 } else { last + cooldown };
            }
            Scene::Stop { vel: v } => {
                if since < 0.0 {
                    vel = rot * v;
                    mode = LocomotionMode::Run;
                }
                let till = |t: f64| t.min(EVENT);
                at += (rot * v * (till(now) - till(s.cap)) as f32).as_dvec3();
                mover.phase = ((f64::from(v.length()) * till(now) / 0.75 * std::f64::consts::PI)
                    % std::f64::consts::TAU) as f32;
            }
            Scene::Alarm => {
                brain.alert = if since < 0.0 {
                    AlertLevel::Relaxed
                } else {
                    AlertLevel::Combat
                };
            }
            Scene::Revive => {
                if let Some(mut v) = vitals {
                    v.downed = since < 0.0;
                }
            }
            Scene::Look { .. } => {
                brain.target = if since < 0.0 { s.foe } else { s.other };
            }
            Scene::Move { vel: v } | Scene::Wounded { vel: v, .. } => {
                if let Scene::Wounded { side, leg, .. } = s.scene
                    && !s.fired
                {
                    // A light blow, long before the frames (it shows only in the wound).
                    s.fired = true;
                    let right = rot * Vec3::X;
                    let height = if leg { 0.5 } else { 1.1 };
                    blows.write(DamageEvent {
                        target: me,
                        amount: 1.0,
                        kind: DamageKind::Slash,
                        point: at
                            + (right * 0.12 * side + ahead * 0.12 + Vec3::Y * height).as_dvec3(),
                        impulse: (-ahead - right * 0.3 * side).normalize() * 20.0,
                        source: None,
                    });
                }
                vel = rot * v;
                at += (vel * (now - s.cap) as f32).as_dvec3();
                let speed = vel.length();
                mode = if speed < 0.1 {
                    LocomotionMode::Idle
                } else if speed < 2.3 {
                    LocomotionMode::Walk
                } else {
                    LocomotionMode::Run
                };
                // As the mover turns distance into the gait phase (its default 0.75 m stride).
                mover.phase = ((f64::from(speed) * now / 0.75 * std::f64::consts::PI)
                    % std::f64::consts::TAU) as f32;
            }
        }
        t.translation = at;
        t.rotation = rot;
        *loco = LocomotionState {
            mode,
            velocity: vel,
            grounded: true,
            ..Default::default()
        };
    }
}

/// One strip: a row of fighters `gap` meters apart, each a frame: a scene some seconds after
/// its action starts.
struct Strip {
    name: &'static str,
    frames: Vec<(Scene, f64)>,
    weapon: NpcWeapon,
    skills: NpcSkills,
    alert: AlertLevel,
    yaw: f32,
    gap: f64,
    /// How high the camera is, and how high it looks (m).
    eye: (f64, f64),
}

impl Strip {
    fn new(name: &'static str, scene: Scene, weapon: NpcWeapon, times: Vec<f64>) -> Self {
        Self::mixed(
            name,
            weapon,
            times.into_iter().map(|t| (scene, t)).collect(),
        )
    }

    /// Frames of different scenes.
    fn mixed(name: &'static str, weapon: NpcWeapon, frames: Vec<(Scene, f64)>) -> Self {
        Self {
            name,
            frames,
            weapon,
            skills: NpcSkills::veteran(),
            alert: AlertLevel::Combat,
            // Three quarters on.
            yaw: 0.6,
            gap: 1.3,
            eye: (1.35, 1.0),
        }
    }
}

/// A scripted fighter: at its own time `cap` it stands at `at` (a `Defend` scene's attacker,
/// unseen, stands in front of it or off to its right).
#[allow(clippy::too_many_arguments)]
fn actor(
    app: &mut App,
    at: DVec3,
    yaw: f32,
    team: u32,
    weapon: NpcWeapon,
    skills: NpcSkills,
    alert: AlertLevel,
    scene: Scene,
    cap: f64,
    warm: f64,
) -> Entity {
    let e = spawn(
        app,
        Pose {
            at,
            yaw,
            skills,
            alert,
            ..Pose::new(0.0, team, weapon)
        },
    );
    let rot = Quat::from_rotation_y(yaw);
    let foe = match scene {
        Scene::Defend { parry, from } => {
            let lands = if parry { 0.2 } else { 0.25 };
            let off = match from {
                Incoming::Right => Vec3::new(1.25, 0.0, -1.25),
                _ => Vec3::new(0.0, 0.0, -1.7),
            };
            let pos = at + (rot * off).as_dvec3();
            let to = (at - pos).as_vec3();
            Some(
                app.world
                    .spawn((
                        Transform::from_translation(pos)
                            .with_rotation(Quat::from_rotation_y((-to.x).atan2(-to.z))),
                        NpcWeapon::sword(),
                        LocomotionState::default(),
                        NpcCombat {
                            phase: AttackPhase::Windup {
                                target: Some(e),
                                hit_at: EVENT + lands,
                                heavy: from == Incoming::High,
                                feint: false,
                                aim: DVec3::ZERO,
                            },
                            ..Default::default()
                        },
                    ))
                    .id(),
            )
        }
        _ => None,
    };
    // A `Look` scene's foes: one straight ahead, the other off to the left.
    let mut other = None;
    let foe = if let Scene::Look { bearing } = scene {
        let mut mark = |off: Vec3| {
            let pos = at + (rot * off).as_dvec3();
            app.world.spawn(Transform::from_translation(pos)).id()
        };
        let ahead = mark(Vec3::new(0.0, 0.0, -6.0));
        other = Some(mark(
            Quat::from_rotation_y(bearing) * Vec3::new(0.0, 0.0, -3.0),
        ));
        app.world
            .entity_mut(e)
            .get_mut::<NpcBrain>()
            .unwrap()
            .target = Some(ahead);
        Some(ahead)
    } else {
        foe
    };
    let mut ent = app.world.entity_mut(e);
    ent.insert((
        NpcClock {
            now: cap - warm,
            dt: 0.0,
            rate: 1.0,
        },
        Script {
            scene,
            weapon,
            at,
            yaw,
            cap,
            foe,
            other,
            fired: false,
        },
    ));
    match scene {
        Scene::Fall => {
            ent.insert((NpcVitals::default(), Downed { since: EVENT }));
        }
        Scene::Revive => {
            ent.insert((
                NpcVitals {
                    downed: true,
                    ..Default::default()
                },
                Downed { since: EVENT - 5.0 },
            ));
        }
        Scene::Wounded { .. } => {
            ent.insert(NpcVitals {
                health: 25.0,
                ..Default::default()
            });
        }
        _ => {}
    }
    e
}

/// An app for rows of scripted fighters on a wide floor.
fn strips_app(w: u32, h: u32) -> App {
    let mut app = app_on(w, h, EARTH);
    app.world.spawn((
        Transform::from_translation(DVec3::new(0.0, -0.002, 0.0)),
        MeshRenderer::new(
            Shape::Plane {
                size: [340.0, 220.0],
                subdivisions: 1,
            },
            StandardMaterial::hex(EARTH).with_roughness(0.95),
        ),
    ));
    app.add_systems(FixedUpdate, run_scripts.in_set(NpcSet::Act));
    app
}

/// Set out the strips side by side along X, each with its camera: the shots to take.
fn stage(app: &mut App, strips: &[Strip], w: u32, h: u32) -> Vec<(Entity, String)> {
    stage_warm(app, strips, w, h, WARM)
}

/// [`stage`] with the fighters living `warm` s before the shots.
fn stage_warm(app: &mut App, strips: &[Strip], w: u32, h: u32, warm: f64) -> Vec<(Entity, String)> {
    let center = |k: usize| (k as f64 - (strips.len() - 1) as f64 * 0.5) * 26.0;
    let mut shots = Vec::new();
    for (k, strip) in strips.iter().enumerate() {
        let width = strip.gap * strip.frames.len() as f64;
        for (i, &(scene, time)) in strip.frames.iter().enumerate() {
            // The cameras look along +Z, so the frames run from +X (on the left of the
            // picture) to −X.
            let at = DVec3::new(
                center(k) + width * 0.5 - (i as f64 + 0.5) * strip.gap,
                0.0,
                0.0,
            );
            actor(
                app,
                at,
                strip.yaw,
                1 + (k % 2) as u32,
                strip.weapon,
                strip.skills,
                strip.alert,
                scene,
                EVENT + time,
                warm,
            );
        }
        // Far off with a long lens: the frames side by side at nearly one scale.
        let dist = 26.0;
        let half_w = width * 0.5 + 0.3;
        let half_h = half_w * f64::from(h) / f64::from(w);
        let fov = (2.0 * (half_h / dist).atan()).to_degrees() as f32;
        let cam = camera(
            app,
            DVec3::new(center(k), strip.eye.0, -dist),
            DVec3::new(center(k), strip.eye.1, 0.0),
            fov,
        );
        shots.push((cam, format!("npc_motion_{}.png", strip.name)));
    }
    shots
}

/// Live through the moments before the frames looking at nothing (cheap frames), then stop
/// the clocks and shoot.
fn roll(app: &mut App, shots: Vec<(Entity, String)>) {
    roll_warm(app, shots, WARM);
}

/// [`roll`] through `warm` s.
fn roll_warm(app: &mut App, shots: Vec<(Entity, String)>, warm: f64) {
    let away = camera(
        app,
        DVec3::new(0.0, 2.0, 200.0),
        DVec3::new(0.0, 2.0, 400.0),
        30.0,
    );
    let mut cams = app.world.query::<(Entity, &mut Camera)>();
    for (e, mut c) in cams.iter_mut(&mut app.world) {
        c.active = e == away;
    }
    app.world
        .resource_mut::<Time>()
        .set_source(TimeSource::Manual { step_secs: 0.1 });
    app.update_n((warm / 0.1).round() as usize);
    if app.world.get_resource::<RenderDisabled>().is_some() {
        eprintln!("skipping: no GPU");
        return;
    }
    app.world
        .resource_mut::<Time>()
        .set_source(TimeSource::Manual { step_secs: 0.0 });
    let mut q = app.world.query::<(&Script, &NpcClock)>();
    let lag = q
        .iter(&app.world)
        .map(|(s, c)| (c.now - s.cap).abs())
        .fold(0.0, f64::max);
    eprintln!("the frames are within {lag:.3} s of their moments");
    for (cam, name) in shots {
        shoot(app, cam, &name);
    }
}

/// Pose sequences as rows of frames, each fighter one moment of the same action (light and
/// heavy swings, block, parry, dodge, stagger, idle, guard, walk, run and a strafe), to judge
/// the motion as an animator would a flipbook. Each fighter lives on its own clock, offset so
/// that at the shot it stands at its frame's moment, having lived through the moments before.
#[test]
fn fighter_motion_strips() {
    let (w, h) = (1920, 540);
    let mut app = strips_app(w, h);
    let frames = |dt: f64| (0..8).map(|i| f64::from(i) * dt).collect::<Vec<_>>();
    let light = Scene::Swing { heavy: false };
    let strips = vec![
        Strip::new(
            "swing_light",
            light,
            NpcWeapon::sword(),
            vec![-0.15, 0.12, 0.28, 0.42, 0.47, 0.53, 0.65, 0.85],
        ),
        Strip {
            skills: NpcSkills::master(),
            ..Strip::new(
                "swing_heavy",
                Scene::Swing { heavy: true },
                NpcWeapon::sword(),
                vec![0.1, 0.3, 0.5, 0.7, 0.75, 0.8, 0.95, 1.2],
            )
        },
        Strip {
            skills: NpcSkills::soldier(),
            ..Strip::new(
                "swing_shield",
                light,
                NpcWeapon::sword_and_shield(),
                vec![-0.15, 0.15, 0.32, 0.47, 0.52, 0.58, 0.72, 0.92],
            )
        },
        Strip::new(
            "block",
            Scene::Block { parry: false },
            NpcWeapon::sword_and_shield(),
            vec![-0.05, 0.03, 0.08, 0.16, 0.25, 0.3, 0.45, 0.62],
        ),
        Strip {
            skills: NpcSkills::master(),
            ..Strip::new(
                "parry",
                Scene::Block { parry: true },
                NpcWeapon::sword(),
                vec![-0.05, 0.03, 0.08, 0.15, 0.2, 0.24, 0.3, 0.4],
            )
        },
        Strip {
            skills: NpcSkills::elite(),
            yaw: 0.25,
            gap: 2.0,
            ..Strip::new(
                "dodge",
                Scene::Dodge,
                NpcWeapon::sword(),
                vec![-0.05, 0.05, 0.12, 0.22, 0.35, 0.55, 0.8],
            )
        },
        Strip {
            skills: NpcSkills::soldier(),
            // Three quarters on from its left, so that the knock back shows.
            yaw: -1.0,
            ..Strip::new(
                "stagger",
                Scene::Stagger,
                NpcWeapon::sword(),
                vec![0.0, 0.08, 0.2, 0.4, 0.6, 0.8, 0.95, 1.15],
            )
        },
        Strip {
            skills: NpcSkills::rookie(),
            alert: AlertLevel::Relaxed,
            ..Strip::new("idle", Scene::Stand, NpcWeapon::sword(), frames(0.8))
        },
        Strip::new(
            "guard",
            Scene::Stand,
            NpcWeapon::sword_and_shield(),
            frames(0.55),
        ),
        Strip {
            skills: NpcSkills::soldier(),
            alert: AlertLevel::Relaxed,
            yaw: FRAC_PI_2,
            ..Strip::new(
                "walk",
                Scene::Move {
                    vel: Vec3::new(0.0, 0.0, -1.6),
                },
                NpcWeapon::sword(),
                frames(0.12),
            )
        },
        Strip {
            skills: NpcSkills::soldier(),
            alert: AlertLevel::Searching,
            yaw: FRAC_PI_2,
            ..Strip::new(
                "run",
                Scene::Move {
                    vel: Vec3::new(0.0, 0.0, -4.5),
                },
                NpcWeapon::sword_and_shield(),
                frames(0.075),
            )
        },
        Strip {
            yaw: 0.3,
            ..Strip::new(
                "strafe",
                Scene::Move {
                    vel: Vec3::new(-1.8, 0.0, 0.0),
                },
                NpcWeapon::sword(),
                frames(0.1),
            )
        },
    ];
    let mut shots = stage(&mut app, &strips, w, h);
    // The telegraphs as a player 10 m off sees them: on guard, a light swing and a heavy one
    // most of the way through their windups, side by side away from the strips.
    let sword = NpcWeapon::sword();
    let row = DVec3::new(0.0, 0.0, -80.0);
    let telegraphs = [
        (Scene::Stand, 0.0),
        (
            Scene::Swing { heavy: false },
            0.85 * f64::from(sword.windup),
        ),
        (
            Scene::Swing { heavy: true },
            0.85 * 1.6 * f64::from(sword.windup),
        ),
    ];
    for (i, (scene, time)) in telegraphs.into_iter().enumerate() {
        let at = row + DVec3::new(1.7 - 1.7 * i as f64, 0.0, 0.0);
        actor(
            &mut app,
            at,
            0.5,
            2,
            sword,
            NpcSkills::master(),
            AlertLevel::Combat,
            scene,
            EVENT + time,
            WARM,
        );
    }
    let far = camera(
        &mut app,
        row + DVec3::new(-2.0, 1.7, -10.0),
        row + DVec3::new(0.0, 1.1, 0.0),
        40.0,
    );
    shots.push((far, "npc_motion_heavy_at_10m.png".into()));
    roll(&mut app, shots);
}

/// The combat reactions as rows of frames: parries and blocks angled to the line of the blow
/// (from above, from either side, a thrust), a dash attack pushing off, a feint pulled back,
/// blows landing on the head, the body and the legs, and a fighter cut down.
#[test]
fn fighter_combat_strips() {
    let (w, h) = (1920, 540);
    let mut app = strips_app(w, h);
    let lines = [
        Incoming::High,
        Incoming::Left,
        Incoming::Right,
        Incoming::Thrust,
    ];
    // Each line: the guard meeting the blow, then the blow landing on it.
    let defend = |parry: bool, meet: f64, hit: f64| {
        lines
            .iter()
            .flat_map(|&from| [meet, hit].map(|t| (Scene::Defend { parry, from }, t)))
            .collect::<Vec<_>>()
    };
    let hit = |height: f32, side: f32, kind: DamageKind| {
        [0.04, 0.12, 0.35].map(|t| (Scene::Hit { height, side, kind }, t))
    };
    let strips = vec![
        Strip {
            skills: NpcSkills::master(),
            yaw: 0.35,
            ..Strip::mixed("parry_lines", NpcWeapon::sword(), defend(true, 0.15, 0.24))
        },
        Strip {
            yaw: 0.35,
            ..Strip::mixed(
                "block_lines",
                NpcWeapon::sword_and_shield(),
                defend(false, 0.18, 0.3),
            )
        },
        Strip {
            skills: NpcSkills::elite(),
            yaw: FRAC_PI_2,
            ..Strip::new(
                "dash_attack",
                Scene::Lunge,
                NpcWeapon::sword(),
                vec![-0.05, 0.03, 0.07, 0.12, 0.18, 0.25, 0.45, 0.6],
            )
        },
        Strip::new(
            "feint",
            Scene::Feint,
            NpcWeapon::sword(),
            vec![0.1, 0.2, 0.25, 0.28, 0.33, 0.4, 0.5, 0.65],
        ),
        Strip {
            skills: NpcSkills::soldier(),
            yaw: 0.45,
            ..Strip::mixed(
                "hit_reactions",
                NpcWeapon::sword(),
                [
                    hit(1.6, -1.0, DamageKind::Ballistic),
                    hit(1.08, 0.0, DamageKind::Slash),
                    hit(0.5, 1.0, DamageKind::Ballistic),
                ]
                .concat(),
            )
        },
        Strip {
            skills: NpcSkills::soldier(),
            yaw: -1.0,
            gap: 1.8,
            eye: (7.0, 0.3),
            ..Strip::new(
                "downed",
                Scene::Fall,
                NpcWeapon::sword(),
                vec![0.0, 0.12, 0.25, 0.4, 0.55, 0.7, 0.9, 1.4],
            )
        },
    ];
    let shots = stage(&mut app, &strips, w, h);
    roll(&mut app, shots);
}

/// Ranged motion, turning, follow-through and blows to the head as rows of frames: a carbine
/// fired and its magazine changed in the cooldown, a bow drawn, held at the anchor, loosed and
/// the next arrow nocked, a carbine carried at port arms on the run, an about-face on the spot,
/// a stop from a run (the tabards, the pouches and the crest swinging on), a blow to the head
/// seen from the front, and the roof guard from a low camera; and blows to the head, the body
/// and the legs side by side as a player 10 m off sees them. The fighters live long enough
/// before the shots for the carbine to have fired a magazine.
#[test]
fn fighter_ranged_turn_and_sway_strips() {
    let (w, h) = (1920, 540);
    let mut app = strips_app(w, h);
    let frames = |dt: f64| (0..8).map(|i| f64::from(i) * dt).collect::<Vec<_>>();
    let high = |parry: bool, times: [f64; 4]| {
        times.map(|t| {
            (
                Scene::Defend {
                    parry,
                    from: Incoming::High,
                },
                t,
            )
        })
    };
    let strips = vec![
        Strip {
            skills: NpcSkills::soldier(),
            yaw: -0.7,
            ..Strip::new(
                "rifle_reload",
                Scene::Shoot { shots: 6 },
                NpcWeapon::rifle(),
                vec![-0.05, 0.03, 0.18, 0.3, 0.38, 0.5, 0.58, 0.68, 0.85],
            )
        },
        Strip {
            skills: NpcSkills::veteran(),
            yaw: 1.1,
            ..Strip::new(
                "bow_cycle",
                Scene::Shoot { shots: 1 },
                NpcWeapon::bow(),
                vec![-0.6, -0.3, -0.08, 0.03, 0.42, 0.62, 0.8, 1.0, 1.12],
            )
        },
        Strip {
            skills: NpcSkills::soldier(),
            yaw: FRAC_PI_2,
            ..Strip::new(
                "rifle_run",
                Scene::Move {
                    vel: Vec3::new(0.0, 0.0, -4.5),
                },
                NpcWeapon::rifle(),
                frames(0.075),
            )
        },
        Strip {
            yaw: 0.3,
            gap: 1.5,
            ..Strip::new(
                "turn",
                Scene::Turn { angle: PI },
                NpcWeapon::sword(),
                vec![0.0, 0.08, 0.16, 0.24, 0.32, 0.42, 0.55, 0.8],
            )
        },
        Strip {
            skills: NpcSkills::master(),
            yaw: FRAC_PI_2,
            gap: 1.5,
            ..Strip::new(
                "stop",
                Scene::Stop {
                    vel: Vec3::new(0.0, 0.0, -4.5),
                },
                NpcWeapon::sword_and_shield(),
                vec![-0.1, 0.05, 0.12, 0.2, 0.3, 0.42, 0.6, 0.9],
            )
        },
        Strip {
            skills: NpcSkills::soldier(),
            yaw: 0.0,
            ..Strip::new(
                "head_hit_front",
                Scene::Hit {
                    height: 1.62,
                    side: -1.0,
                    kind: DamageKind::Slash,
                },
                NpcWeapon::sword(),
                vec![0.0, 0.04, 0.08, 0.14, 0.22, 0.35, 0.55, 0.9],
            )
        },
        Strip {
            skills: NpcSkills::master(),
            yaw: 0.2,
            eye: (0.9, 1.45),
            ..Strip::mixed(
                "roof_guard",
                NpcWeapon::sword(),
                [
                    high(false, [0.06, 0.15, 0.27, 0.4]),
                    high(true, [0.05, 0.12, 0.2, 0.26]),
                ]
                .concat(),
            )
        },
    ];
    let warm = 6.0;
    let mut shots = stage_warm(&mut app, &strips, w, h, warm);
    // Where the blows land, as a player 10 m off sees them: on guard, then 0.1 s after a blow
    // to the head, the body and the legs.
    let row = DVec3::new(0.0, 0.0, -80.0);
    let blow = |height: f32| Scene::Hit {
        height,
        side: -1.0,
        kind: DamageKind::Slash,
    };
    let zones = [
        (Scene::Stand, 0.0),
        (blow(1.62), 0.1),
        (blow(1.08), 0.1),
        (blow(0.5), 0.1),
    ];
    for (i, (scene, time)) in zones.into_iter().enumerate() {
        actor(
            &mut app,
            row + DVec3::new(2.55 - 1.7 * i as f64, 0.0, 0.0),
            0.0,
            2,
            NpcWeapon::sword(),
            NpcSkills::soldier(),
            AlertLevel::Combat,
            scene,
            EVENT + time,
            warm,
        );
    }
    let far = camera(
        &mut app,
        row + DVec3::new(0.0, 1.6, -10.0),
        row + DVec3::new(0.0, 1.0, 0.0),
        40.0,
    );
    shots.push((far, "npc_motion_hit_zones_at_10m.png".into()));
    roll_warm(&mut app, shots, warm);
}

/// Close-ups of single fighters, each from its own camera, side by side in one picture: each
/// a scene at a moment, the camera `dist` m off its head, `view` rad round to its left from
/// straight in front.
struct Tiles {
    name: &'static str,
    frames: Vec<(Scene, f64)>,
    weapon: NpcWeapon,
    alert: AlertLevel,
    yaw: f32,
    view: f32,
    dist: f64,
    /// How much of the fighter shows: half the picture's height (m), and where its middle is
    /// (m up).
    half: f64,
    mid: f64,
}

impl Tiles {
    fn new(name: &'static str, weapon: NpcWeapon, frames: Vec<(Scene, f64)>) -> Self {
        Self {
            name,
            frames,
            weapon,
            alert: AlertLevel::Combat,
            yaw: 0.0,
            view: 0.45,
            dist: 1.3,
            half: 0.32,
            mid: 1.5,
        }
    }
}

/// Set out the tiles' fighters far apart (each alone in its picture), each with its camera:
/// the shots to take, by row.
fn stage_tiles(app: &mut App, rows: &[Tiles], warm: f64) -> Vec<(Vec<Entity>, String)> {
    let mut out = Vec::new();
    for (k, row) in rows.iter().enumerate() {
        let mut cams = Vec::new();
        for (i, &(scene, time)) in row.frames.iter().enumerate() {
            // The rows side by side, all on one line across the cameras' view.
            let at = DVec3::new((k * 60 + i * 6) as f64, 0.0, 40.0);
            actor(
                app,
                at,
                row.yaw,
                1 + (k % 2) as u32,
                row.weapon,
                NpcSkills::veteran(),
                row.alert,
                scene,
                EVENT + time,
                warm,
            );
            let look = Quat::from_rotation_y(row.yaw + row.view) * Vec3::NEG_Z;
            let mid = at + DVec3::Y * row.mid;
            let eye = mid + (look * row.dist as f32).as_dvec3() + DVec3::Y * 0.05;
            let fov = (2.0 * (row.half / row.dist).atan()).to_degrees() as f32;
            cams.push(camera(app, eye, mid, fov));
        }
        out.push((cams, format!("npc_motion_{}.png", row.name)));
    }
    out
}

/// [`roll_warm`] for tiles: each row's pictures side by side in one.
fn roll_tiles(app: &mut App, rows: Vec<(Vec<Entity>, String)>, warm: f64) {
    roll_warm(app, Vec::new(), warm);
    if app.world.get_resource::<RenderDisabled>().is_some() {
        return;
    }
    for (cams, name) in rows {
        let shots: Vec<_> = cams
            .iter()
            .enumerate()
            .filter_map(|(i, &cam)| shoot_quiet(app, cam, &format!("tile_{i}_{name}")))
            .collect();
        let (w, h) = shots[0].dimensions();
        let mut sheet = image::RgbaImage::new(w * shots.len() as u32, h);
        for (i, img) in shots.iter().enumerate() {
            image::imageops::replace(&mut sheet, img, i64::from(w) * i as i64, 0);
        }
        let path = std::path::Path::new(env!("CARGO_TARGET_TMPDIR")).join(&name);
        sheet.save(&path).expect("save tiles");
        if let Ok(dir) = std::env::var("NPC_SCREENSHOT_DIR") {
            let _ = std::fs::create_dir_all(&dir);
            std::fs::copy(&path, std::path::Path::new(&dir).join(&name)).expect("copy tiles");
        }
        eprintln!("wrote {}", path.display());
    }
}

/// [`shoot`] to the test's temp dir only.
fn shoot_quiet(app: &mut App, cam: Entity, name: &str) -> Option<image::RgbaImage> {
    let mut q = app.world.query::<(Entity, &mut Camera)>();
    for (e, mut c) in q.iter_mut(&mut app.world) {
        c.active = e == cam;
    }
    app.update_n(3);
    let path = std::path::Path::new(env!("CARGO_TARGET_TMPDIR")).join(name);
    capture_now(&mut app.world, &path).expect("screenshot");
    Some(image::open(&path).expect("png").to_rgba8())
}

/// The face and the look, close: on guard, struck in the body and on the head, through a
/// heavy blow (the jaw set in the telegraph, a shout as it goes in), breathing hard after a
/// sprint and badly hurt; the head and eyes going to a new foe off to the left (the eyes
/// first, the head after them, the shoulders taking what the neck can't); and a searching
/// fighter sweeping the ground ahead.
#[test]
fn fighter_face_strips() {
    let mut app = strips_app(360, 420);
    let sword = NpcWeapon::sword();
    let heavy = 1.6 * f64::from(sword.windup);
    let hit = |height: f32| Scene::Hit {
        height,
        side: -1.0,
        kind: DamageKind::Slash,
    };
    let rows = vec![
        Tiles::new(
            "faces",
            sword,
            vec![
                (Scene::Stand, 0.0),
                (hit(1.1), 0.08),
                (hit(1.62), 0.1),
                (Scene::Swing { heavy: true }, 0.6 * heavy),
                (Scene::Swing { heavy: true }, heavy + 0.08),
                (
                    Scene::Stop {
                        vel: Vec3::new(0.0, 0.0, -6.5),
                    },
                    0.6,
                ),
                (
                    Scene::Wounded {
                        vel: Vec3::ZERO,
                        side: 1.0,
                        leg: false,
                    },
                    0.0,
                ),
            ],
        ),
        Tiles {
            view: 0.0,
            dist: 1.6,
            half: 0.42,
            mid: 1.45,
            ..Tiles::new(
                "look",
                sword,
                [-0.1, 0.03, 0.07, 0.12, 0.2, 0.3, 0.45, 0.8]
                    .map(|t| (Scene::Look { bearing: 1.7 }, t))
                    .to_vec(),
            )
        },
        Tiles {
            alert: AlertLevel::Searching,
            view: 0.0,
            dist: 1.6,
            half: 0.42,
            mid: 1.45,
            ..Tiles::new(
                "search",
                NpcWeapon::sword_and_shield(),
                (0..8).map(|i| (Scene::Stand, 0.5 * f64::from(i))).collect(),
            )
        },
    ];
    let warm = 6.0;
    let shots = stage_tiles(&mut app, &rows, warm);
    roll_tiles(&mut app, shots, warm);
}

/// Getting up and readying as rows of frames: revived fighters of each weapon getting up
/// from where they lay (rolled to the side, onto a knee with the weapon taken up, pushed up
/// onto guard); at ease with the weapon carried, then a foe in sight and the weapon brought up
/// to guard; the badly hurt limping on the leg last struck, or hunched over a wound in the
/// body, a hand pressed to it at ease; and each weapon at ease and at the low ready.
#[test]
fn fighter_recovery_strips() {
    let (w, h) = (1920, 540);
    let mut app = strips_app(w, h);
    // (An archer takes longer, nocking an arrow on the way up.)
    let up = [-0.1, 0.2, 0.4, 0.6, 0.8, 1.0, 1.2, 1.55];
    let up_bow = [-0.1, 0.25, 0.55, 0.85, 1.15, 1.5, 1.85, 2.35];
    let get_up = |name, weapon, times: [f64; 8]| Strip {
        gap: 2.0,
        eye: (2.6, 0.75),
        ..Strip::new(name, Scene::Revive, weapon, times.to_vec())
    };
    let ready = [-0.1, 0.08, 0.16, 0.25, 0.35, 0.5, 0.7, 1.0];
    let ready_bow = [-0.1, 0.12, 0.3, 0.5, 0.7, 0.9, 1.1, 1.4];
    let alarm =
        |name, weapon, times: [f64; 8]| Strip::new(name, Scene::Alarm, weapon, times.to_vec());
    let wounded = |side: f32, leg: bool| Scene::Wounded {
        vel: Vec3::new(0.0, 0.0, -1.4),
        side,
        leg,
    };
    let still = |side: f32, leg: bool| Scene::Wounded {
        vel: Vec3::ZERO,
        side,
        leg,
    };
    let frames = |scene, dt: f64| (0..8).map(|i| (scene, f64::from(i) * dt)).collect();
    let strips = vec![
        get_up("get_up_longsword", NpcWeapon::sword(), up),
        get_up("get_up_shield", NpcWeapon::sword_and_shield(), up),
        get_up("get_up_rifle", NpcWeapon::rifle(), up),
        get_up("get_up_bow", NpcWeapon::bow(), up_bow),
        alarm("ready_longsword", NpcWeapon::sword(), ready),
        alarm("ready_shield", NpcWeapon::sword_and_shield(), ready),
        alarm("ready_rifle", NpcWeapon::rifle(), ready),
        alarm("ready_bow", NpcWeapon::bow(), ready_bow),
        Strip {
            alert: AlertLevel::Relaxed,
            yaw: FRAC_PI_2,
            ..Strip::mixed("limp", NpcWeapon::sword(), frames(wounded(-1.0, true), 0.1))
        },
        Strip {
            alert: AlertLevel::Relaxed,
            yaw: FRAC_PI_2,
            ..Strip::mixed(
                "hunched",
                NpcWeapon::sword_and_shield(),
                frames(wounded(1.0, false), 0.1),
            )
        },
        Strip {
            alert: AlertLevel::Relaxed,
            yaw: 0.3,
            ..Strip::mixed(
                "wounded_at_ease",
                NpcWeapon::sword(),
                [
                    (still(-1.0, true), 0.0),
                    (still(1.0, true), 0.0),
                    (still(-1.0, false), 0.0),
                    (still(1.0, false), 0.0),
                    (still(-1.0, true), 0.7),
                    (still(1.0, true), 0.7),
                    (still(-1.0, false), 0.7),
                    (still(1.0, false), 0.7),
                ]
                .to_vec(),
            )
        },
    ];
    let warm = 2.5;
    let mut shots = stage_warm(&mut app, &strips, w, h, warm);
    // Each weapon carried at ease, then at the low ready of a wary fighter.
    let row = DVec3::new(0.0, 0.0, -80.0);
    let weapons = [
        NpcWeapon::sword(),
        NpcWeapon::sword_and_shield(),
        NpcWeapon::rifle(),
        NpcWeapon::bow(),
    ];
    for (i, alert) in [AlertLevel::Relaxed, AlertLevel::Suspicious]
        .into_iter()
        .enumerate()
    {
        for (j, weapon) in weapons.into_iter().enumerate() {
            let x = 4.55 - 1.3 * (i * 4 + j) as f64;
            actor(
                &mut app,
                row + DVec3::new(x, 0.0, 0.0),
                0.6,
                1 + i as u32,
                weapon,
                NpcSkills::veteran(),
                alert,
                Scene::Stand,
                EVENT,
                warm,
            );
        }
    }
    let half_h = 5.5 * f64::from(h) / f64::from(w);
    let cam = camera(
        &mut app,
        row + DVec3::new(0.0, 1.35, -26.0),
        row + DVec3::new(0.0, 1.0, 0.0),
        (2.0 * (half_h / 26.0).atan()).to_degrees() as f32,
    );
    shots.push((cam, "npc_motion_carry.png".into()));
    roll_warm(&mut app, shots, warm);
}
