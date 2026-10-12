//! Close-up renders of the procedural fighters in posed lineups (guard, block, telegraph, aim,
//! draw, walk, kneel, downed), to check the art and the rig. Needs a GPU or a software Vulkan
//! driver (lavapipe); passes with a note when neither is available (`RenderDisabled`).
//!
//! The fighters here have no AI: their combat state is set by hand and the NPC clock stands
//! still at 0. The PNGs land in the test's temp dir; set `NPC_SCREENSHOT_DIR` to also copy
//! them there.

use engine_core::prelude::*;
use engine_core::time::TimeSource;
use engine_npc::*;
use engine_render::prelude::*;
use engine_render::{RenderDisabled, capture_now};
use engine_world::WorldPlugin;
use engine_world::locomotion::{LocomotionMode, LocomotionState};

const W: u32 = 1280;
const H: u32 = 720;

fn app() -> App {
    let mut app = App::new();
    app.world.resource_mut::<EngineConfig>().headless = true;
    {
        let mut w = app.world.resource_mut::<Window>();
        w.width = W;
        w.height = H;
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
            StandardMaterial::hex(0x8c7a5b).with_roughness(0.95),
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

fn spawn(app: &mut App, p: Pose) {
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
    shoot(&mut app, downed, "npc_fighters_downed.png");
    shoot(&mut app, back, "npc_fighters_back.png");
}
