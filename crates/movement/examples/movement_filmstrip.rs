//! Renders the key moments of the movement demo to PNGs, headless, plus a contact sheet.
//!
//! ```text
//! cargo run -p engine_movement --release --example movement_filmstrip -- [out_dir]
//! ```
//!
//! The demo is deterministic (fixed 60 Hz steps on a manual clock), so this runs it twice:
//! once without drawing to find the frame of each moment, then again drawing only the few
//! frames before each moment (so exposure settles) and saving the moment itself. Needs a GPU
//! or a software Vulkan driver (lavapipe).

use std::path::{Path, PathBuf};

use engine_core::math::{DVec3, Vec3};
use engine_core::prelude::*;
use engine_core::time::TimeSource;
use engine_movement::combat::{AttackPhase, CombatState};
use engine_movement::demo::{self, Brain, DuelDirector, Ride, Round};
use engine_movement::{MovementPlugin, Mover};
use engine_render::prelude::*;
use engine_render::{RenderDisabled, Renderer, capture_now};
use engine_world::{LocomotionMode, WorldPlugin};

const WIDTH: u32 = 1280;
const HEIGHT: u32 = 720;
const DT: f64 = 1.0 / 60.0;
/// Longest run searched for moments, real seconds.
const MAX_SECS: f64 = 45.0;
/// Frames drawn before each moment so auto exposure settles.
const WARMUP_FRAMES: u64 = 10;

/// The demo's characters.
#[derive(Clone)]
struct Cast {
    runner: Entity,
    player: Entity,
    elite: Entity,
    grunts: Vec<Entity>,
    director: Entity,
    rider: Entity,
    car: Entity,
    climber: Entity,
    grappler: Entity,
    ladder: Entity,
    vaulter: Entity,
}

struct Shot {
    file: &'static str,
    /// The moment: true on the frame to save (the first frame it holds is used).
    when: fn(&World, &Cast) -> bool,
    /// Camera (eye, target, vertical FOV in degrees) for the current state.
    camera: fn(&World, &Cast) -> (DVec3, DVec3, f32),
}

fn pos(w: &World, e: Entity) -> DVec3 {
    w.get::<Transform>(e).unwrap().translation
}

fn mover(w: &World, e: Entity) -> &Mover {
    w.get::<Mover>(e).unwrap()
}

fn combat(w: &World, e: Entity) -> &CombatState {
    w.get::<CombatState>(e).unwrap()
}

fn round(w: &World, c: &Cast) -> Round {
    w.get::<DuelDirector>(c.director).unwrap().round
}

fn ride(w: &World, c: &Cast) -> Ride {
    match w.get::<Brain>(c.rider).unwrap() {
        Brain::Rider { step, .. } => *step,
        _ => Ride::Start,
    }
}

fn runner_step(w: &World, c: &Cast) -> usize {
    match w.get::<Brain>(c.runner).unwrap() {
        Brain::Runner { step, .. } => *step,
        _ => 0,
    }
}

const SHOTS: &[Shot] = &[
    Shot {
        file: "01_slide_under_the_bar.png",
        when: |w, c| {
            let x = pos(w, c.runner).x;
            mover(w, c.runner).mode == LocomotionMode::Slide && (-14.4..-13.9).contains(&x)
        },
        camera: |_, _| {
            (
                DVec3::new(-10.8, 0.9, 7.6),
                DVec3::new(-14.6, 0.75, 12.4),
                50.0,
            )
        },
    },
    Shot {
        file: "02_wall_run.png",
        when: |w, c| {
            let x = pos(w, c.runner).x;
            mover(w, c.runner).mode == LocomotionMode::WallRun && (-1.8..-1.0).contains(&x)
        },
        camera: |w, c| {
            let p = pos(w, c.runner);
            (
                p + DVec3::new(5.5, -0.3, -7.0),
                p + DVec3::new(-2.0, 0.0, 0.6),
                50.0,
            )
        },
    },
    Shot {
        file: "03_wall_jump_between_walls.png",
        when: |w, c| runner_step(w, c) == 6 && (4.5..5.5).contains(&pos(w, c.runner).x),
        camera: |w, c| {
            let p = pos(w, c.runner);
            (
                p + DVec3::new(-7.0, 0.6, 1.2),
                p + DVec3::new(2.0, -0.4, -1.2),
                55.0,
            )
        },
    },
    Shot {
        file: "04_mantle_onto_the_tower.png",
        when: |w, c| mover(w, c.runner).mode == LocomotionMode::Mantle,
        camera: |w, c| {
            let p = pos(w, c.runner);
            (
                p + DVec3::new(-4.5, 2.2, 4.5),
                p + DVec3::new(1.5, -0.2, -0.4),
                50.0,
            )
        },
    },
    Shot {
        file: "05_double_jump.png",
        when: |w, c| {
            let m = mover(w, c.runner);
            !m.grounded && m.air_jumps_left == 0 && pos(w, c.runner).x > 27.5
        },
        camera: |w, c| {
            let p = pos(w, c.runner);
            (
                p + DVec3::new(-6.0, -0.8, -7.5),
                p + DVec3::new(0.5, -0.6, 0.0),
                50.0,
            )
        },
    },
    Shot {
        file: "06_blink_untrained_player_is_cut_down.png",
        when: |w, c| round(w, c) == Round::Blink && combat(w, c.player).hits_taken >= 1,
        camera: |w, c| {
            let mid = (pos(w, c.player) + pos(w, c.elite)) * 0.5;
            (
                mid + DVec3::new(-5.0, 1.3, 1.5),
                mid + DVec3::new(0.0, 0.1, 0.0),
                45.0,
            )
        },
    },
    Shot {
        file: "07_focus_trained_player_sidesteps_in_slow_motion.png",
        when: |w, c| {
            round(w, c) == Round::Focus
                && mover(w, c.player).mode == LocomotionMode::Dash
                && combat(w, c.player).attack.is_none()
                && pos(w, c.player).x > 2.5
        },
        camera: |w, c| {
            let e = pos(w, c.elite);
            (
                e + DVec3::new(-3.5, 1.4, -4.0),
                e + DVec3::new(2.5, 0.0, 1.0),
                50.0,
            )
        },
    },
    Shot {
        file: "08_focus_counter_strike.png",
        when: |w, c| {
            round(w, c) == Round::Focus
                && combat(w, c.player)
                    .attack
                    .is_some_and(|a| a.phase == AttackPhase::Strike)
        },
        camera: |w, c| {
            let mid = (pos(w, c.player) + pos(w, c.elite)) * 0.5;
            (
                mid + DVec3::new(3.0, 1.0, 4.0),
                mid + DVec3::new(0.0, 0.0, 0.0),
                45.0,
            )
        },
    },
    Shot {
        file: "09_pack_grunts_fall_one_by_one.png",
        when: |w, c| {
            round(w, c) == Round::Pack
                && c.grunts.iter().filter(|g| combat(w, **g).defeated).count() >= 2
                && combat(w, c.player)
                    .attack
                    .is_some_and(|a| a.phase == AttackPhase::Strike)
        },
        camera: |w, c| {
            let p = pos(w, c.player);
            (
                p + DVec3::new(-4.5, 2.0, 5.5),
                p + DVec3::new(0.5, -0.2, -1.5),
                50.0,
            )
        },
    },
    // The freight station, framed as the showcase tour stop frames it.
    Shot {
        file: "10_leap_onto_the_moving_freight_car.png",
        when: |w, c| mover(w, c.rider).mode == LocomotionMode::WallRun,
        camera: |_, _| {
            (
                DVec3::new(-20.0, 3.6, 21.0),
                DVec3::new(-15.5, 3.0, 28.5),
                55.0,
            )
        },
    },
    Shot {
        file: "11_climb_aboard_the_freight_car.png",
        when: |w, c| {
            let m = mover(w, c.rider);
            ride(w, c) == Ride::Climb && m.mode == LocomotionMode::Mantle && m.mode_secs > 0.12
        },
        camera: |w, c| {
            let p = pos(w, c.rider);
            (
                p + DVec3::new(-2.6, 0.3, -3.4),
                p + DVec3::new(0.6, 0.0, 0.6),
                55.0,
            )
        },
    },
    Shot {
        file: "12_ride_the_freight_car.png",
        when: |w, c| ride(w, c) == Ride::Aboard && pos(w, c.car).x > -12.0,
        camera: |w, c| {
            let p = pos(w, c.rider);
            (
                p + DVec3::new(-6.0, -2.0, -6.5),
                p + DVec3::new(1.5, -1.2, 0.5),
                50.0,
            )
        },
    },
    Shot {
        file: "13_shimmy_around_the_ledge_corner.png",
        when: |w, c| {
            mover(w, c.climber)
                .hang
                .and_then(|h| h.corner)
                .is_some_and(|k| k.t > 0.5)
        },
        camera: |w, c| {
            let p = pos(w, c.climber);
            (
                p + DVec3::new(3.6, 0.4, 3.4),
                p + DVec3::new(-0.4, 0.3, -0.4),
                50.0,
            )
        },
    },
    // Past the middle of the zip line, from ahead and below on the sunny side: the rider
    // coming at the camera, the cable running back up to the gantry on the tower.
    Shot {
        file: "14_ride_the_zip_line.png",
        when: |w, c| mover(w, c.grappler).zip.is_some_and(|z| z.s > 0.5),
        camera: |w, c| {
            let p = pos(w, c.grappler);
            let (along, across) = demo::layout::zip_axes();
            let (along, across) = (DVec3::from(along), DVec3::from(across));
            (
                p + along * 4.0 - across * 3.2 - DVec3::Y * 1.2,
                p - along * 1.0 + DVec3::Y * 0.4,
                50.0,
            )
        },
    },
    // Halfway up the tower's caged ladder, from behind and to the side, looking up past the
    // climber at the roof.
    Shot {
        file: "15_climb_the_tower_ladder.png",
        when: |w, c| {
            mover(w, c.ladder)
                .ladder
                .is_some_and(|l| l.climbed > 2.4 && l.speed > 2.0)
        },
        camera: |w, c| {
            let p = pos(w, c.ladder);
            (
                p + DVec3::new(2.4, -0.6, 3.0),
                p + DVec3::new(-0.15, 0.5, -0.3),
                50.0,
            )
        },
    },
    // Over the concrete barrier at a sprint, the left hand planted on its top and the legs
    // swung over, from ahead on the vaulter's right.
    Shot {
        file: "16_vault_the_barrier.png",
        when: |w, c| {
            let x = pos(w, c.vaulter).x;
            mover(w, c.vaulter)
                .vault
                .is_some_and(|v| !v.rising && (v.plant.x + 16.1).abs() < 0.2 && x > v.plant.x)
        },
        camera: |w, c| {
            let p = pos(w, c.vaulter);
            (
                p + DVec3::new(2.6, 0.1, 3.3),
                p + DVec3::new(-0.3, -0.15, 0.0),
                50.0,
            )
        },
    },
];

fn build_app() -> (App, Cast, Entity) {
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
        .set_source(TimeSource::Manual { step_secs: DT });
    app.world.resource_mut::<FixedTime>().set_hz(60.0);
    // Draw nothing until a moment is near.
    app.world
        .insert_resource(RenderDisabled("parked between shots".into()));
    {
        let mut fog = app.world.resource_mut::<FogSettings>();
        fog.density = 0.0015;
        fog.height_falloff = 0.05;
    }
    {
        let mut s = app.world.resource_mut::<RenderSettings>();
        s.bloom_intensity = 0.06;
        s.vignette = 0.25;
    }

    // Late-afternoon sun from +X, a little toward −Z.
    let sun = Vec3::new(0.84, 0.32, -0.43).normalize();
    app.world.spawn((
        DirectionalLight {
            color: [1.0, 0.86, 0.7],
            illuminance: 70_000.0,
            ..Default::default()
        },
        Transform::default().looking_at(-sun.as_dvec3(), Vec3::Y),
    ));
    // Ground and a backdrop of tall blocks so the frames have depth.
    app.world.spawn((
        Transform::default(),
        MeshRenderer::new(
            Shape::Plane {
                size: [400.0, 400.0],
                subdivisions: 1,
            },
            StandardMaterial::hex(0x33373d).with_roughness(0.9),
        ),
    ));
    for (i, (x, z, h)) in [
        (-40.0, 34.0, 26.0),
        (-18.0, 40.0, 18.0),
        (8.0, 44.0, 34.0),
        (34.0, 36.0, 22.0),
        (52.0, 10.0, 30.0),
        (-52.0, -8.0, 24.0),
        (-36.0, -42.0, 38.0),
        (6.0, -48.0, 28.0),
        (40.0, -36.0, 20.0),
    ]
    .into_iter()
    .enumerate()
    {
        let shade = [0x5d6470, 0x6f6a64, 0x545b66][i % 3];
        app.world.spawn((
            Transform::from_xyz(x, h * 0.5, z),
            MeshRenderer::new(
                Shape::Cuboid {
                    size: [10.0, h as f32, 10.0],
                },
                StandardMaterial::hex(shade).with_roughness(0.8),
            ),
        ));
    }
    let camera = app
        .world
        .spawn((
            Camera::perspective(50.0).with_exposure(Exposure::Auto {
                min_ev100: 8.0,
                max_ev100: 16.0,
                speed: 40.0,
                compensation: 0.3,
            }),
            Transform::from_xyz(0.0, 5.0, 20.0),
        ))
        .id();

    let mut queue = bevy_ecs::world::CommandQueue::default();
    let mut commands = Commands::new(&mut queue, &app.world);
    demo::spawn_demo(&mut commands, DVec3::ZERO);
    queue.apply(&mut app.world);

    let named = |app: &mut App, name: &str| {
        let mut q = app.world.query::<(Entity, &Name)>();
        q.iter(&app.world)
            .find(|(_, n)| n.as_str() == name)
            .map(|(e, _)| e)
            .unwrap_or_else(|| panic!("no {name}"))
    };
    let runner = named(&mut app, "Runner");
    let director = named(&mut app, "Duel director");
    let d = app.world.get::<DuelDirector>(director).unwrap().clone();
    let cast = Cast {
        runner,
        player: d.player.unwrap(),
        elite: d.elite.unwrap(),
        grunts: d.grunts.clone(),
        director,
        rider: named(&mut app, "Rider"),
        car: named(&mut app, "Freight car"),
        climber: named(&mut app, "Climber"),
        grappler: named(&mut app, "Grappler"),
        ladder: named(&mut app, "Ladder climber"),
        vaulter: named(&mut app, "Vaulter"),
    };
    (app, cast, camera)
}

fn point_camera(world: &mut World, camera: Entity, (eye, target, fov): (DVec3, DVec3, f32)) {
    *world.get_mut::<Transform>(camera).unwrap() =
        Transform::from_translation(eye).looking_at(target, Vec3::Y);
    world.get_mut::<Camera>(camera).unwrap().projection = Projection::Perspective {
        fov_y_deg: fov,
        near: 0.05,
    };
}

fn main() {
    let out = std::env::args()
        .nth(1)
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("target/movement_filmstrip"));
    std::fs::create_dir_all(&out).expect("output directory");

    // Pass 1: find each moment's frame.
    let (mut app, cast, _) = build_app();
    let mut frames: Vec<Option<u64>> = vec![None; SHOTS.len()];
    let total = (MAX_SECS / DT) as u64;
    for frame in 1..=total {
        app.update();
        for (i, shot) in SHOTS.iter().enumerate() {
            if frames[i].is_none() && (shot.when)(&app.world, &cast) {
                frames[i] = Some(frame);
            }
        }
        if frames.iter().all(Option::is_some) {
            break;
        }
    }
    for (shot, f) in SHOTS.iter().zip(&frames) {
        match f {
            Some(f) => println!("{:<55} at {:5.2} s", shot.file, *f as f64 * DT),
            None => println!("{:<55} never happened", shot.file),
        }
    }

    // Pass 2: draw the frames around each moment.
    let (mut app, cast, camera) = build_app();
    let mut parked: Option<Renderer> = None;
    let mut pending: Vec<(usize, u64)> = frames
        .iter()
        .enumerate()
        .filter_map(|(i, f)| f.map(|f| (i, f)))
        .collect();
    pending.sort_by_key(|(_, f)| *f);
    let mut saved = Vec::new();
    let mut frame = 0u64;
    for (i, at) in pending {
        let shot = &SHOTS[i];
        // Catch up without drawing (shots never overlap their warmups by more than a frame).
        while frame + WARMUP_FRAMES < at {
            app.update();
            frame += 1;
        }
        app.world.remove_resource::<RenderDisabled>();
        if let Some(r) = parked.take() {
            app.world.insert_resource(r);
        }
        // Draw up to the moment (and a little past it if pass 2 drifted).
        let mut hit = false;
        while frame < at + 30 {
            let view = (shot.camera)(&app.world, &cast);
            point_camera(&mut app.world, camera, view);
            app.update();
            frame += 1;
            if let Some(d) = app.world.get_resource::<RenderDisabled>() {
                eprintln!("cannot render: {}", d.0);
                std::process::exit(1);
            }
            if frame >= at && (shot.when)(&app.world, &cast) {
                hit = true;
                break;
            }
        }
        if hit {
            let view = (shot.camera)(&app.world, &cast);
            point_camera(&mut app.world, camera, view);
            let path = out.join(shot.file);
            match capture_now(&mut app.world, &path) {
                Ok(_) => {
                    println!("saved {}", path.display());
                    saved.push(path);
                }
                Err(e) => eprintln!("{}: {e}", shot.file),
            }
        } else {
            eprintln!("{}: the moment did not repeat in the second run", shot.file);
        }
        parked = app.world.remove_resource::<Renderer>();
        app.world
            .insert_resource(RenderDisabled("parked between shots".into()));
    }
    saved.sort();
    contact_sheet(&saved, &out.join("00_contact_sheet.png"));
}

/// All shots on one image, three across.
fn contact_sheet(paths: &[PathBuf], out: &Path) {
    if paths.is_empty() {
        return;
    }
    let (tw, th) = (WIDTH / 2, HEIGHT / 2);
    let cols = 3u32;
    let rows = (paths.len() as u32).div_ceil(cols);
    let gap = 6u32;
    let mut sheet = image::RgbaImage::from_pixel(
        cols * tw + (cols + 1) * gap,
        rows * th + (rows + 1) * gap,
        image::Rgba([18, 18, 22, 255]),
    );
    for (i, p) in paths.iter().enumerate() {
        let Ok(img) = image::open(p) else {
            continue;
        };
        let small = image::imageops::resize(
            &img.to_rgba8(),
            tw,
            th,
            image::imageops::FilterType::Triangle,
        );
        let (c, r) = (i as u32 % cols, i as u32 / cols);
        image::imageops::overlay(
            &mut sheet,
            &small,
            (gap + c * (tw + gap)) as i64,
            (gap + r * (th + gap)) as i64,
        );
    }
    sheet.save(out).expect("contact sheet");
    println!("saved {}", out.display());
}
