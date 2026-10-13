//! Close-up renders of the procedural fighters in posed lineups (guard, block, telegraph, aim,
//! draw, walk, kneel, downed), to check the art and the rig. Needs a GPU or a software Vulkan
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
use engine_world::WorldPlugin;
use engine_world::locomotion::{LocomotionMode, LocomotionState};

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

/// What a scripted fighter does, by its own clock (the action starts at `EVENT`).
#[derive(Clone, Copy, Debug)]
enum Scene {
    /// Standing (relaxed or on guard, as its alert level says).
    Stand,
    /// A swing, its telegraph starting at the event.
    Swing { heavy: bool },
    /// The guard raised at the event against a blow landing a moment later.
    Block { parry: bool },
    /// A dash 1.6 m to the left at the event (across the picture, facing the camera), as the
    /// mover dashes.
    Dodge,
    /// Knocked off balance at the event, for a second.
    Stagger,
    /// Moving at `vel` (m/s, in its own frame: −Z ahead) the whole time.
    Move { vel: Vec3 },
}

/// A fighter acting out a scene; at its own time `cap` it stands at `at`.
#[derive(Component, Clone, Copy, Debug)]
struct Script {
    scene: Scene,
    weapon: NpcWeapon,
    at: DVec3,
    yaw: f32,
    cap: f64,
}

/// Play every scripted fighter's scene: combat state, position and motion by its clock.
fn run_scripts(
    mut q: Query<(
        &Script,
        &NpcClock,
        &mut NpcCombat,
        &mut LocomotionState,
        &mut MoverState,
        &mut Transform,
    )>,
) {
    for (s, clock, mut combat, mut loco, mut mover, mut t) in &mut q {
        let now = clock.now;
        let since = now - EVENT;
        let rot = Quat::from_rotation_y(s.yaw);
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
            Scene::Block { parry } => {
                let (len, lands) = if parry { (0.28, 0.2) } else { (0.6, 0.25) };
                combat.defense = if !(0.0..len).contains(&since) {
                    Defense::None
                } else if parry {
                    Defense::Parry { until: EVENT + len }
                } else {
                    Defense::Block { until: EVENT + len }
                };
                combat.incoming.clear();
                if (0.0..lands).contains(&since) {
                    combat.incoming.push(engine_npc::agent::IncomingAttack {
                        attacker: Entity::PLACEHOLDER,
                        lands_at: EVENT + lands,
                        react_at: EVENT,
                        ranged: false,
                        reach: 1.9,
                        handled: true,
                    });
                }
            }
            Scene::Dodge => {
                let ease = |t: f64| {
                    let d = ((t - EVENT) / 0.22).clamp(0.0, 1.0);
                    1.0 - (1.0 - d) * (1.0 - d)
                };
                let left = rot * Vec3::NEG_X;
                at += (left * 1.6 * (ease(now) - ease(s.cap)) as f32).as_dvec3();
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
            Scene::Stagger => {
                combat.phase = if (0.0..1.0).contains(&since) {
                    AttackPhase::Staggered { until: EVENT + 1.0 }
                } else {
                    AttackPhase::Ready
                };
            }
            Scene::Move { vel: v } => {
                vel = rot * v;
                at += (vel * (now - s.cap) as f32).as_dvec3();
                let speed = vel.length();
                mode = if speed < 2.3 {
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

/// One strip: a row of fighters `gap` meters apart, each a frame of the same scene, `times`
/// seconds after the action starts.
struct Strip {
    name: &'static str,
    scene: Scene,
    weapon: NpcWeapon,
    skills: NpcSkills,
    alert: AlertLevel,
    yaw: f32,
    gap: f64,
    times: Vec<f64>,
}

impl Strip {
    fn new(name: &'static str, scene: Scene, weapon: NpcWeapon, times: Vec<f64>) -> Self {
        Self {
            name,
            scene,
            weapon,
            skills: NpcSkills::veteran(),
            alert: AlertLevel::Combat,
            // Three quarters on.
            yaw: 0.6,
            gap: 1.3,
            times,
        }
    }
}

/// Pose sequences as rows of frames, each fighter one moment of the same action (light and
/// heavy swings, block, parry, dodge, stagger, idle, guard, walk, run and a strafe), to judge
/// the motion as an animator would a flipbook. Each fighter lives on its own clock, offset so
/// that at the shot it stands at its frame's moment, having lived through the moments before.
#[test]
fn fighter_motion_strips() {
    let (w, h) = (1920, 540);
    let mut app = app_on(w, h, EARTH);
    // A floor for the long row of strips and the telegraphs.
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
    let center = |k: usize| (k as f64 - (strips.len() - 1) as f64 * 0.5) * 26.0;
    // The cameras look along +Z, so the frames run from +X (on the left of the picture) to −X.
    let slot = |strip: &Strip, k: usize, i: usize| {
        let width = strip.gap * strip.times.len() as f64;
        DVec3::new(
            center(k) + width * 0.5 - (i as f64 + 0.5) * strip.gap,
            0.0,
            0.0,
        )
    };
    let mut shots = Vec::new();
    for (k, strip) in strips.iter().enumerate() {
        for (i, &time) in strip.times.iter().enumerate() {
            let at = slot(strip, k, i);
            let cap = EVENT + time;
            let e = spawn(
                &mut app,
                Pose {
                    at,
                    yaw: strip.yaw,
                    skills: strip.skills,
                    alert: strip.alert,
                    ..Pose::new(0.0, 1 + (k % 2) as u32, strip.weapon)
                },
            );
            app.world.entity_mut(e).insert((
                NpcClock {
                    now: cap - WARM,
                    dt: 0.0,
                    rate: 1.0,
                },
                Script {
                    scene: strip.scene,
                    weapon: strip.weapon,
                    at,
                    yaw: strip.yaw,
                    cap,
                },
            ));
        }
        // Far off with a long lens: the frames side by side at nearly one scale.
        let dist = 26.0;
        let half_w = strip.gap * strip.times.len() as f64 * 0.5 + 0.3;
        let half_h = half_w * f64::from(h) / f64::from(w);
        let fov = (2.0 * (half_h / dist).atan()).to_degrees() as f32;
        let cam = camera(
            &mut app,
            DVec3::new(center(k), 1.35, -dist),
            DVec3::new(center(k), 1.0, 0.0),
            fov,
        );
        shots.push((cam, format!("npc_motion_{}.png", strip.name)));
    }
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
        let cap = EVENT + time;
        let e = spawn(
            &mut app,
            Pose {
                at,
                yaw: 0.5,
                skills: NpcSkills::master(),
                ..Pose::new(0.0, 2, sword)
            },
        );
        app.world.entity_mut(e).insert((
            NpcClock {
                now: cap - WARM,
                dt: 0.0,
                rate: 1.0,
            },
            Script {
                scene,
                weapon: sword,
                at,
                yaw: 0.5,
                cap,
            },
        ));
    }
    let far = camera(
        &mut app,
        row + DVec3::new(-2.0, 1.7, -10.0),
        row + DVec3::new(0.0, 1.1, 0.0),
        40.0,
    );
    shots.push((far, "npc_motion_heavy_at_10m.png".into()));
    // Live through the moments before the frames looking at nothing (cheap frames), then stop
    // the clocks and shoot.
    let away = camera(
        &mut app,
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
    app.update_n((WARM / 0.1).round() as usize);
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
        shoot(&mut app, cam, &name);
    }
}
