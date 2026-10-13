//! Renders the fighters headless to PNGs: the four styles side by side (front and back),
//! helmet, back, whole-body and katana close-ups, and a sequence of poses from the
//! procedural animation.
//!
//! ```text
//! cargo run -p engine_movement --release --example fighter_preview -- [out_dir] [quick]
//! ```
//!
//! `quick` stops after the lineup, helmets, backs and whole bodies (for iterating on armor).
//!
//! Each pose comes from the real animator ([`Animator`]) fed a short scripted timeline of
//! movement and combat state, exactly as the demo feeds it from `Mover` and `CombatState`.
//! Needs a GPU or a software Vulkan driver (lavapipe).

use std::path::{Path, PathBuf};

use engine_core::math::{DVec3, Quat, Vec3};
use engine_core::prelude::*;
use engine_core::time::TimeSource;
use engine_movement::MovementPlugin;
use engine_movement::combat::AttackPhase;
use engine_movement::look::fighter::{self, Body, Style};
use engine_movement::look::rig::{self, AnimInput, Animator, AttackAnim, Bone, Pose};
use engine_movement::look::{material, mesh};
use engine_render::prelude::*;
use engine_render::{RenderDisabled, capture_now};
use engine_world::{LocomotionMode, WorldPlugin};

const WIDTH: u32 = 1600;
const HEIGHT: u32 = 900;
/// Frames drawn before each capture (shadows and the environment settle).
const WARMUP_FRAMES: u32 = 3;
/// Animation step for the scripted timelines, s.
const DT: f32 = 1.0 / 120.0;

/// One fighter on a stage.
struct Actor {
    body: Body,
}

fn spawn_actor(world: &mut World, style: Style, pos: DVec3, yaw: f32) -> Actor {
    let owner = world
        .spawn((
            Name::new(style.key()),
            Transform::from_translation(pos).with_rotation(Quat::from_rotation_y(yaw)),
        ))
        .id();
    let mut queue = bevy_ecs::world::CommandQueue::default();
    let mut commands = Commands::new(&mut queue, world);
    let body = fighter::spawn_body(&mut commands, owner, style, 0.0);
    queue.apply(world);
    Actor { body }
}

fn set_pose(world: &mut World, actor: &Actor, pose: &Pose) {
    let solved = rig::solve(pose);
    for (i, &e) in actor.body.bones[..Bone::ANIMATED].iter().enumerate() {
        *world.get_mut::<Transform>(e).unwrap() = fighter::bone_transform(solved[i], 0.0);
    }
}

/// Runs a fresh animator through `script(t)` for `secs` seconds and returns the last pose.
fn simulate(style: Style, secs: f32, script: impl Fn(f32) -> AnimInput) -> Pose {
    let mut anim = Animator::new(style);
    let mut pose = Pose::BIND;
    let mut t = 0.0;
    while t < secs {
        pose = anim.update(&AnimInput {
            dt: DT,
            ..script(t)
        });
        t += DT;
    }
    pose
}

/// Runs a fresh animator until `until(animator)` holds after at least `min_secs`.
fn simulate_until(
    style: Style,
    min_secs: f32,
    input: AnimInput,
    until: impl Fn(&Animator) -> bool,
) -> Pose {
    let mut anim = Animator::new(style);
    let mut pose = Pose::BIND;
    let mut t = 0.0;
    while t < min_secs || !until(&anim) {
        pose = anim.update(&AnimInput { dt: DT, ..input });
        t += DT;
        if t > min_secs + 10.0 {
            break;
        }
    }
    pose
}

fn attack(dash: bool, phase: AttackPhase, progress: f32, secs: f32) -> Option<AttackAnim> {
    Some(AttackAnim {
        dash,
        phase,
        progress,
        secs,
    })
}

/// A melee attack timeline starting at `start` (windup 0.28 s, strike 0.1 s, recovery 0.45 s).
fn swing(t: f32, start: f32) -> Option<AttackAnim> {
    let s = t - start;
    if s < 0.0 {
        None
    } else if s < 0.28 {
        attack(false, AttackPhase::Windup, s / 0.28, s)
    } else if s < 0.38 {
        attack(false, AttackPhase::Strike, (s - 0.28) / 0.1, s)
    } else if s < 0.83 {
        attack(false, AttackPhase::Recovery, (s - 0.38) / 0.45, s)
    } else {
        None
    }
}

fn block(world: &mut World, center: DVec3, size: [f32; 3], hex: u32) {
    world.spawn((
        Transform::from_translation(center),
        MeshRenderer::new(
            Shape::Cuboid { size },
            StandardMaterial::hex(hex).with_roughness(0.75),
        ),
    ));
}

fn point_camera(world: &mut World, camera: Entity, eye: DVec3, target: DVec3, fov: f32) {
    *world.get_mut::<Transform>(camera).unwrap() =
        Transform::from_translation(eye).looking_at(target, Vec3::Y);
    world.get_mut::<Camera>(camera).unwrap().projection = Projection::Perspective {
        fov_y_deg: fov,
        near: 0.02,
    };
}

fn shoot(app: &mut App, camera: Entity, view: (DVec3, DVec3, f32), path: &Path) -> bool {
    point_camera(&mut app.world, camera, view.0, view.1, view.2);
    for _ in 0..WARMUP_FRAMES {
        app.update();
        if let Some(d) = app.world.get_resource::<RenderDisabled>() {
            eprintln!("cannot render: {}", d.0);
            std::process::exit(1);
        }
    }
    match capture_now(&mut app.world, path) {
        Ok(_) => {
            println!("saved {}", path.display());
            true
        }
        Err(e) => {
            eprintln!("{}: {e}", path.display());
            false
        }
    }
}

/// Puts images on one sheet, `cols` across, each scaled to `tw`×`th`.
fn sheet(paths: &[PathBuf], cols: u32, tw: u32, th: u32, out: &Path) {
    let rows = (paths.len() as u32).div_ceil(cols);
    let mut img = image::RgbaImage::from_pixel(cols * tw, rows * th, image::Rgba([0, 0, 0, 255]));
    for (i, p) in paths.iter().enumerate() {
        let Ok(src) = image::open(p) else {
            continue;
        };
        let small = image::imageops::resize(
            &src.to_rgba8(),
            tw,
            th,
            image::imageops::FilterType::Lanczos3,
        );
        let (c, r) = (i as u32 % cols, i as u32 / cols);
        image::imageops::overlay(&mut img, &small, (c * tw) as i64, (r * th) as i64);
    }
    img.save(out).expect("sheet");
    println!("saved {}", out.display());
}

fn main() {
    let out = std::env::args()
        .nth(1)
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("target/fighter_preview"));
    // `quick`: only the lineup, helmets, backs and whole bodies (for iterating on armor).
    let quick = std::env::args().nth(2).is_some_and(|a| a == "quick");
    std::fs::create_dir_all(&out).expect("output directory");
    for style in Style::ALL {
        println!(
            "{:<8} {:>6} triangles, {} meshes",
            style.key(),
            fighter::triangle_count(style),
            fighter::parts(style).len()
        );
    }

    let mut app = App::new();
    app.world.resource_mut::<EngineConfig>().headless = true;
    {
        let mut w = app.world.resource_mut::<Window>();
        w.width = WIDTH;
        w.height = HEIGHT;
    }
    app.add_plugins(RenderPlugin)
        .add_plugins(WorldPlugin)
        .add_plugins(MovementPlugin);
    app.world
        .resource_mut::<Time>()
        .set_source(TimeSource::Manual {
            step_secs: 1.0 / 60.0,
        });
    {
        let mut s = app.world.resource_mut::<RenderSettings>();
        s.bloom_intensity = 0.05;
        s.vignette = 0.22;
        s.shadow_distance = 20.0;
        s.shadow_map_size = 4096;
        s.shadow_softness = 1.5;
    }
    {
        let mut fog = app.world.resource_mut::<FogSettings>();
        fog.density = 0.0025;
        fog.height_falloff = 0.04;
    }
    let world = &mut app.world;
    // Low late-afternoon sun, front-right in most shots (the fighters face −Z, the cameras
    // look along +Z and +X).
    let sun = Vec3::new(-0.55, 0.42, -0.72).normalize();
    world.spawn((
        DirectionalLight {
            color: [1.0, 0.86, 0.7],
            illuminance: 85_000.0,
            ..Default::default()
        },
        Transform::default().looking_at(-sun.as_dvec3(), Vec3::Y),
    ));
    world.spawn((
        Transform::default(),
        MeshRenderer::new(
            Shape::Plane {
                size: [3000.0, 3000.0],
                subdivisions: 1,
            },
            StandardMaterial::hex(0x45464a).with_roughness(0.8),
        ),
    ));
    // Stages 25 m apart along (+X, −Z), so no camera sees another stage.
    let stage = |i: u32| DVec3::new(i as f64 * 25.0, 0.0, -(i as f64) * 25.0);
    // A distant skyline all around.
    let middle = stage(8);
    for i in 0..40 {
        let a = i as f64 / 40.0 * std::f64::consts::TAU;
        let r = 420.0 + (i % 5) as f64 * 40.0;
        let h = 30.0 + ((i * 7) % 9) as f32 * 9.0;
        block(
            world,
            middle + DVec3::new(a.cos() * r, h as f64 * 0.5, a.sin() * r),
            [30.0, h, 30.0],
            [0x5d6470, 0x6f6a64, 0x545b66][i % 3],
        );
    }
    let camera = world
        .spawn((
            Camera::perspective(35.0).with_exposure(Exposure::Manual { ev100: 14.3 }),
            Transform::from_xyz(0.0, 1.5, -5.0),
        ))
        .id();

    let idle = AnimInput::default();
    let v = DVec3::new;

    // 0: the lineup.
    let mut lineup = Vec::new();
    for (k, style) in Style::ALL.into_iter().enumerate() {
        let pos = stage(0) + DVec3::new(1.65 - k as f64 * 1.1, 0.0, 0.0);
        let yaw = 0.3;
        let a = spawn_actor(&mut app.world, style, pos, yaw);
        let fighter = style != Style::Pilot;
        let pose = simulate(style, 1.5, |_| AnimInput { fighter, ..idle });
        set_pose(&mut app.world, &a, &pose);
        let head = rig::solve(&pose)[Bone::Head.index()].pos + Vec3::Y * 0.07;
        let head = pos + (Quat::from_rotation_y(yaw) * head).as_dvec3();
        lineup.push((style, head));
    }

    // 1: the katana on display: the duelist's sword above its scabbard, blade to the left.
    let katana_at = stage(1) + v(0.0, 1.2, 0.0);
    {
        let world = &mut app.world;
        let rot = rig::sword_rot(Vec3::new(-1.0, 0.1, 0.0), Vec3::new(0.0, -1.0, 0.0));
        let sword = world
            .spawn(Transform::from_translation(katana_at).with_rotation(rot))
            .id();
        let blade = world
            .spawn((
                Transform::from_translation((Vec3::Z * rig::BLADE_BASE_Z).as_dvec3()),
                ChildOf(sword),
            ))
            .id();
        let srot = rig::sword_rot(Vec3::new(-1.0, 0.08, 0.0), Vec3::new(0.0, 1.0, 0.0));
        let sheath = world
            .spawn(
                Transform::from_translation(katana_at + v(-0.07, -0.15, 0.04)).with_rotation(srot),
            )
            .id();
        for part in fighter::parts(Style::Duelist) {
            let parent = match part.bone {
                Bone::Sword => sword,
                Bone::Blade => blade,
                Bone::Sheath => sheath,
                _ => continue,
            };
            world.spawn((
                Transform::default(),
                MeshRenderer::new(mesh(&part.mesh), material(&part.material)),
                ChildOf(parent),
            ));
        }
    }

    // Poses: one stage each. (file, style, pose, camera eye and target relative to the
    // actor's feet, fov)
    type Shot = (&'static str, Style, Pose, DVec3, DVec3, f32);
    let mut shots: Vec<Shot> = Vec::new();
    let run = AnimInput {
        mode: LocomotionMode::Sprint,
        velocity: Vec3::new(0.0, 0.0, -8.5),
        fighter: false,
        effort: 0.8,
        ..idle
    };
    shots.push((
        "05_run.png",
        Style::Pilot,
        simulate_until(Style::Pilot, 1.5, run, |a| (a.phase - 0.12).abs() < 0.01),
        v(-3.8, 1.05, -0.9),
        v(0.0, 0.95, -0.1),
        32.0,
    ));
    shots.push((
        "06_slide.png",
        Style::Pilot,
        simulate(Style::Pilot, 0.6, |_| AnimInput {
            mode: LocomotionMode::Slide,
            velocity: Vec3::new(0.0, 0.0, -8.0),
            fighter: false,
            ..idle
        }),
        v(-2.6, 0.7, -2.4),
        v(0.0, 0.45, -0.2),
        32.0,
    ));
    let wall_gap = 0.42;
    shots.push((
        "07_wall_run.png",
        Style::Pilot,
        simulate_until(
            Style::Pilot,
            1.0,
            AnimInput {
                mode: LocomotionMode::WallRun,
                velocity: Vec3::new(0.0, 0.3, -8.0),
                grounded: false,
                wall: Some((Vec3::NEG_X, wall_gap)),
                fighter: false,
                ..idle
            },
            |a| (a.phase - 0.2).abs() < 0.01,
        ),
        v(-3.2, 1.4, -2.6),
        v(0.15, 0.95, 0.0),
        38.0,
    ));
    shots.push((
        "08_double_jump.png",
        Style::Pilot,
        simulate(Style::Pilot, 0.64, |t| AnimInput {
            mode: LocomotionMode::Air,
            velocity: Vec3::new(0.0, if t < 0.3 { 3.0 } else { 6.0 }, -6.0),
            grounded: false,
            air_jumps_left: if t < 0.3 { 1 } else { 0 },
            fighter: false,
            ..idle
        }),
        v(-4.2, 0.9, -0.6),
        v(0.0, 1.0, 0.0),
        36.0,
    ));
    shots.push((
        "09_strike.png",
        Style::Duelist,
        simulate(Style::Duelist, 0.5 + 0.28 + 0.03, |t| AnimInput {
            attack: swing(t, 0.5),
            ..idle
        }),
        v(-1.9, 1.45, -3.0),
        v(0.0, 1.2, -0.3),
        38.0,
    ));
    shots.push((
        "10_iaido_draw.png",
        Style::Elite,
        simulate(Style::Elite, 0.42, |t| {
            let (mode, a) = if t < 0.3 {
                (
                    LocomotionMode::Dash,
                    attack(true, AttackPhase::DashIn, 0.0, t),
                )
            } else {
                (
                    LocomotionMode::Idle,
                    attack(true, AttackPhase::Windup, ((t - 0.3) / 0.1).min(1.0), t),
                )
            };
            AnimInput {
                mode,
                velocity: if t < 0.3 {
                    Vec3::new(0.0, 0.0, -20.0)
                } else {
                    Vec3::ZERO
                },
                dash_strike: true,
                attack: a,
                ..idle
            }
        }),
        v(-2.4, 1.25, -2.4),
        v(0.0, 0.95, 0.0),
        36.0,
    ));
    shots.push((
        "11_block.png",
        Style::Duelist,
        simulate(Style::Duelist, 1.0, |t| AnimInput {
            blocking: t > 0.3,
            ..idle
        }),
        v(-2.0, 1.55, -2.6),
        v(0.0, 1.2, 0.0),
        36.0,
    ));
    shots.push((
        "12_defeat.png",
        Style::Grunt,
        simulate(Style::Grunt, 2.5, |t| AnimInput {
            hits_taken: if t > 0.4 { 1 } else { 0 },
            defeated: t > 0.4,
            ..idle
        }),
        v(-2.4, 1.6, -1.9),
        v(0.0, 0.25, -0.4),
        36.0,
    ));
    let mut actors = Vec::new();
    for (i, (file, style, pose, ..)) in shots.iter().enumerate() {
        let mut at = stage(2 + i as u32);
        if file.contains("wall_run") {
            // Running on a wall on the right, a meter up.
            block(
                &mut app.world,
                at + v(wall_gap as f64 + 0.15, 2.0, -1.0),
                [0.3, 4.0, 8.0],
                0x8a8f96,
            );
            at.y = 1.0;
        } else if file.contains("double_jump") {
            at.y = 1.2;
        }
        let a = spawn_actor(&mut app.world, *style, at, 0.0);
        set_pose(&mut app.world, &a, pose);
        actors.push(at);
    }
    // Sequences: one actor re-posed per frame.
    let seq_at = stage(2 + shots.len() as u32);
    let seq_duelist = spawn_actor(&mut app.world, Style::Duelist, seq_at, 0.0);
    let run_at = stage(3 + shots.len() as u32);
    let seq_pilot = spawn_actor(&mut app.world, Style::Pilot, run_at, 0.0);
    // Whole bodies, one stage per style, standing as in the lineup.
    let mut bodies = Vec::new();
    for (k, style) in Style::ALL.into_iter().enumerate() {
        let at = stage(4 + shots.len() as u32 + k as u32);
        let a = spawn_actor(&mut app.world, style, at, 0.0);
        let fighter = style != Style::Pilot;
        set_pose(
            &mut app.world,
            &a,
            &simulate(style, 1.5, |_| AnimInput { fighter, ..idle }),
        );
        bodies.push((style, at));
    }

    // Draw.
    let l = stage(0);
    shoot(
        &mut app,
        camera,
        (l + v(0.45, 1.3, -5.6), l + v(0.0, 0.93, 0.0), 34.0),
        &out.join("01_lineup.png"),
    );
    shoot(
        &mut app,
        camera,
        (l + v(-1.6, 1.5, 5.2), l + v(0.0, 0.93, 0.0), 34.0),
        &out.join("01_lineup_back.png"),
    );
    let mut heads = Vec::new();
    for (style, head) in &lineup {
        let p = out.join(format!("02_helmet_{}.png", style.key()));
        if shoot(
            &mut app,
            camera,
            (
                *head + v(-0.44, 0.06, -0.52),
                *head + v(0.0, -0.03, 0.0),
                30.0,
            ),
            &p,
        ) {
            heads.push(p);
        }
    }
    if !heads.is_empty() {
        sheet(
            &heads,
            2,
            WIDTH / 2,
            HEIGHT / 2,
            &out.join("02_helmets.png"),
        );
    }
    // From behind and to the side: neck guards, shoulder lames, the sash's knot.
    let mut backs = Vec::new();
    for (style, head) in &lineup {
        let p = out.join(format!("02_back_{}.png", style.key()));
        if shoot(
            &mut app,
            camera,
            (
                *head + v(0.62, 0.02, 0.62),
                *head + v(0.0, -0.12, 0.0),
                34.0,
            ),
            &p,
        ) {
            backs.push(p);
        }
    }
    if !backs.is_empty() {
        sheet(&backs, 2, WIDTH / 2, HEIGHT / 2, &out.join("02_backs.png"));
    }
    // Each style alone, from the front left and from behind right.
    let mut views = Vec::new();
    for (style, at) in &bodies {
        for (side, eye) in [
            ("front", v(-1.25, 1.25, -2.35)),
            ("back", v(-1.95, 1.35, 1.75)),
        ] {
            let p = out.join(format!("02_body_{}_{side}.png", style.key()));
            if shoot(
                &mut app,
                camera,
                (*at + eye, *at + v(0.0, 0.92, 0.0), 36.0),
                &p,
            ) {
                views.push(p);
            }
        }
    }
    if !views.is_empty() {
        sheet(&views, 2, WIDTH / 2, HEIGHT / 2, &out.join("02_bodies.png"));
    }
    if quick {
        return;
    }
    let k = katana_at;
    shoot(
        &mut app,
        camera,
        (k + v(-0.4, 0.1, -1.15), k + v(-0.4, -0.07, 0.0), 40.0),
        &out.join("03_katana.png"),
    );
    shoot(
        &mut app,
        camera,
        (k + v(0.05, 0.07, -0.3), k + v(-0.04, 0.0, 0.0), 32.0),
        &out.join("04_katana_detail.png"),
    );
    let mut poses = Vec::new();
    for ((file, _, _, eye, target, fov), at) in shots.iter().zip(&actors) {
        let p = out.join(file);
        if shoot(&mut app, camera, (*at + *eye, *at + *target, *fov), &p) {
            poses.push(p);
        }
    }
    if !poses.is_empty() {
        sheet(&poses, 4, WIDTH / 4, HEIGHT / 4, &out.join("00_poses.png"));
    }

    // A strike, frame by frame: windup, the cut, the follow-through, recovery.
    let frames_dir = out.join("frames");
    std::fs::create_dir_all(&frames_dir).expect("frames directory");
    let mut frames = Vec::new();
    for (i, t) in [0.5, 0.64, 0.78, 0.805, 0.83, 0.855, 0.88, 1.1]
        .into_iter()
        .enumerate()
    {
        let pose = simulate(Style::Duelist, t, |t| AnimInput {
            attack: swing(t, 0.5),
            ..idle
        });
        set_pose(&mut app.world, &seq_duelist, &pose);
        let p = frames_dir.join(format!("strike_{i}.png"));
        if shoot(
            &mut app,
            camera,
            (
                seq_at + v(-1.6, 1.5, -3.2),
                seq_at + v(0.0, 1.15, -0.35),
                40.0,
            ),
            &p,
        ) {
            frames.push(p);
        }
    }
    if !frames.is_empty() {
        sheet(
            &frames,
            4,
            WIDTH / 4,
            HEIGHT / 4,
            &out.join("13_strike_sequence.png"),
        );
    }
    let mut frames = Vec::new();
    for i in 0..8 {
        let target = i as f32 / 8.0;
        let pose = simulate_until(Style::Pilot, 1.5, run, |a| {
            let d = (a.phase - target).rem_euclid(1.0);
            d < 0.012
        });
        set_pose(&mut app.world, &seq_pilot, &pose);
        let p = frames_dir.join(format!("run_{i}.png"));
        if shoot(
            &mut app,
            camera,
            (
                run_at + v(-4.2, 1.0, -0.2),
                run_at + v(0.0, 0.95, -0.2),
                30.0,
            ),
            &p,
        ) {
            frames.push(p);
        }
    }
    if !frames.is_empty() {
        sheet(
            &frames,
            4,
            WIDTH / 4,
            HEIGHT / 4,
            &out.join("14_run_cycle.png"),
        );
    }
    let _ = std::fs::remove_dir_all(&frames_dir);
}
