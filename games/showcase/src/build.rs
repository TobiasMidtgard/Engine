//! Building the valley, a step at a time. Tours, films and headless runs build it all at
//! startup. The exe draws a loading screen at once and builds one step a frame behind it, so
//! the window shows something straight away and the progress bar moves, then opens the start
//! screen.

use crate::landscape::{self, Landscape};
use crate::{camera, placeholders, planet, scenery, zones};
use engine::core::ecs::system::SystemState;
use engine::prelude::*;
use engine_ui::color::Color;
use engine_ui::el::El;
use engine_ui::node::Screen;
use engine_ui::screens;
use engine_ui::style::{Align, Corners, Justify, Position, pct};
use engine_ui::theme::Theme;
use std::sync::Arc;

/// Build the world over frames behind a loading screen (the exe) instead of all at startup.
#[derive(Resource)]
pub struct Staged;

/// The world while it is being built: what the steps share, and how long each took.
#[derive(Resource)]
pub struct WorldBuild {
    next: usize,
    land: Arc<Landscape>,
    rng: Rng,
    lib: Option<scenery::Library>,
    hedged: Vec<(DVec2, DVec2)>,
    started: std::time::Instant,
    laps: Vec<(&'static str, f32)>,
    /// The next zone whose demo wakes (the exe wakes one a frame).
    zone: usize,
    /// What the loading screen names after the step's text (the zone waking).
    detail: Option<&'static str>,
    /// What ran last frame, and the frames of the staged build that took long, so a slow
    /// load says where its time went.
    last: &'static str,
    slow: Vec<(&'static str, f32)>,
}

const LIB: &str = "the library is the first step";

/// One step: what the loading screen says meanwhile, and the work. A step returns false to
/// be tried again next frame (it waits for something).
type Step = (
    &'static str,
    &'static str,
    fn(&mut World, &mut WorldBuild) -> bool,
);

/// The steps, in order (the scenery draws from one random sequence, so the order is the
/// layout).
const STEPS: [Step; 11] = [
    (
        "library",
        "Carving the stones and weaving the grass",
        |world, b| {
            let mut state = SystemState::<(
                ResMut<Assets<Mesh>>,
                ResMut<Assets<StandardMaterial>>,
                ResMut<Assets<Image>>,
                Res<engine_foliage::PlantLibrary>,
            )>::new(world);
            let (mut meshes, mut materials, mut images, plants) =
                state.get_mut(world).expect("resources the showcase adds");
            b.lib = Some(scenery::build_library(
                &mut meshes,
                &mut materials,
                &mut images,
                &plants,
            ));
            true
        },
    ),
    ("terrain", "Laying out the valley", |world, b| {
        if world.contains_resource::<planet::UsePlanet>() {
            return true;
        }
        let mut state = SystemState::<(Commands, ResMut<Assets<Mesh>>)>::new(world);
        let (mut commands, mut meshes) = state.get_mut(world).expect("resources the showcase adds");
        scenery::spawn_terrain(
            &mut commands,
            &mut meshes,
            b.lib.as_ref().expect(LIB),
            &b.land,
        );
        state.apply(world);
        true
    }),
    ("woods", "Planting the woods", |world, b| {
        with_commands(world, |c| {
            scenery::spawn_woods(c, b.lib.as_ref().expect(LIB), &b.land, &mut b.rng)
        });
        true
    }),
    ("meadows", "Growing the meadows", |world, b| {
        with_commands(world, |c| {
            scenery::spawn_meadows(c, b.lib.as_ref().expect(LIB), &b.land, &mut b.rng);
            scenery::spawn_meadow_grass(c, &b.land);
        });
        true
    }),
    (
        "farms and walls",
        "Ploughing the fields and walling them",
        |world, b| {
            with_commands(world, |c| {
                b.hedged = scenery::spawn_farms(c, b.lib.as_ref().expect(LIB), &b.land, &mut b.rng);
                scenery::spawn_walls(c, b.lib.as_ref().expect(LIB), &b.land, &b.hedged);
            });
            true
        },
    ),
    ("rocks", "Scattering the boulders", |world, b| {
        with_commands(world, |c| {
            scenery::spawn_rocks(c, b.lib.as_ref().expect(LIB), &b.land, &mut b.rng)
        });
        true
    }),
    ("village", "Raising the village", |world, b| {
        with_commands(world, |c| {
            scenery::spawn_village(c, b.lib.as_ref().expect(LIB), &b.land, &mut b.rng)
        });
        true
    }),
    (
        "cliffs and stream",
        "Running the stream over the cliffs",
        |world, b| {
            let mut state = SystemState::<(Commands, ResMut<Assets<Mesh>>)>::new(world);
            let (mut commands, mut meshes) =
                state.get_mut(world).expect("resources the showcase adds");
            let lib = b.lib.as_ref().expect(LIB);
            scenery::spawn_cliffs_and_stream(&mut commands, &mut meshes, lib, &b.land, &mut b.rng);
            placeholders::cave_shell(&mut commands, &mut meshes, lib, &b.land, &mut b.rng);
            state.apply(world);
            true
        },
    ),
    ("zone demos", "Waking the zones", |world, b| {
        // Movement's look bakes in the background in the exe; its fighters need it.
        let staged = world.contains_resource::<Staged>();
        if staged && !world.contains_resource::<engine_movement::look::LookReady>() {
            return false;
        }
        let only = world.get_resource::<zones::DemoZones>().cloned();
        if let Some(only) = only.as_ref().filter(|_| b.zone == 0) {
            engine::core::log::info!(
                "only the zone demos of {:?} (`--zones all` spawns every one)",
                only.0
            );
        }
        // Zone by zone in order (they draw from one random sequence), one a frame in the exe.
        while let Some(z) = zones::ZONES.get(b.zone) {
            b.zone += 1;
            if only
                .as_ref()
                .is_some_and(|o| !o.0.iter().any(|k| k == z.key))
            {
                continue;
            }
            let t = std::time::Instant::now();
            let one = zones::DemoZones(vec![z.key.to_string()]);
            with_commands(world, |c| {
                zones::spawn_demos(
                    c,
                    b.lib.as_ref().expect(LIB),
                    &b.land,
                    &mut b.rng,
                    Some(&one),
                )
            });
            b.laps.push((z.key, t.elapsed().as_secs_f32()));
            b.last = z.key;
            if staged {
                b.detail = zones::ZONES.get(b.zone).map(|z| z.name);
                return b.zone >= zones::ZONES.len();
            }
        }
        b.detail = None;
        true
    }),
    ("moon", "Hanging the Moon", |world, _| {
        if !world.contains_resource::<planet::UsePlanet>() {
            with_commands(world, zones::moon);
        }
        true
    }),
    ("tower textures", "Finishing the tower", |world, b| {
        // The tower's textures baked on other threads while the world spawned.
        let mut images = world.resource_mut::<Assets<Image>>();
        b.lib
            .as_ref()
            .expect(LIB)
            .tower
            .finish_textures(&mut images);
        true
    }),
];

fn with_commands(world: &mut World, f: impl FnOnce(&mut Commands)) {
    let mut state = SystemState::<Commands>::new(world);
    let mut commands = state.get_mut(world).expect("resources the showcase adds");
    f(&mut commands);
    state.apply(world);
}

/// The world's settings, ground, planet, camera and sky, then the rest: all of it now, or
/// (staged) a step a frame from the next frame on.
pub fn start(world: &mut World) {
    let started = std::time::Instant::now();
    // The planet beyond the terrain is grassland too, so the far edge blends into it.
    world.resource_mut::<Atmosphere>().ground_color = [0.07, 0.13, 0.04];
    {
        let mut settings = world.resource_mut::<RenderSettings>();
        // Mountains a few km away should still be in the shadow maps' reach for the valley.
        settings.shadow_distance = settings.shadow_distance.max(400.0);
        // Every image in the showcase and its module demos is built once and only sampled on
        // the GPU (audited for round 11), so the CPU copies can go once they are uploaded.
        settings.release_image_pixels = true;
    }
    // Clear mountain air: a faint valley haze, about 130 km visibility (3.9 / density), so the
    // atmosphere gives the distant ranges their blue and the far shadow map their shade.
    world.resource_mut::<FogSettings>().density = 0.00003;

    let land = landscape::shared().clone();
    let ground = Arc::new(landscape::LandscapeGround::new(
        land.clone(),
        &mut world.resource_mut::<engine::world::SurfaceMaterials>(),
    ));
    // On the planet the terrain answers `Ground` (edits included) and asks this for surfaces.
    world.insert_resource(engine::world::Ground(ground.clone()));
    let planet = world.contains_resource::<planet::UsePlanet>();
    with_commands(world, |c| {
        if planet {
            planet::spawn(c, land.clone(), ground);
        }
        camera::spawn_camera(c);
        // The weather module drives the sun, the moon and the clock (`engine_weather::Sky`).
        engine_weather::spawn_sky_lights(c);
    });
    let seed = world.resource::<EngineConfig>().seed;
    let mut build = WorldBuild {
        next: 0,
        land,
        rng: Rng::new(seed),
        lib: None,
        hedged: Vec::new(),
        started,
        laps: vec![("ground and sky", started.elapsed().as_secs_f32())],
        zone: 0,
        detail: None,
        last: "ground and sky",
        slow: Vec::new(),
    };
    if world.contains_resource::<Staged>() {
        show_loading(world, &build);
        world.insert_resource(build);
        return;
    }
    while build.next < STEPS.len() {
        run_step(world, &mut build);
    }
    finish(world, &build);
}

/// Next step of a staged build, one a frame.
pub fn step(world: &mut World) {
    let Some(mut build) = world.remove_resource::<WorldBuild>() else {
        return;
    };
    // The frame before this one: what it ran (the step, the systems answering what it
    // spawned, and the drawing) and how long it took.
    let dt = world.resource::<Time>().real_delta_secs();
    if dt > SLOW_FRAME_S {
        build.slow.push((build.last, dt));
    }
    run_step(world, &mut build);
    if build.next < STEPS.len() {
        show_loading(world, &build);
        world.insert_resource(build);
    } else {
        screens::hide(world, "world_loading", false);
        finish(world, &build);
    }
}

/// Whether the world is still being built (the start screen waits).
pub fn building(world: &World) -> bool {
    world.contains_resource::<WorldBuild>()
}

/// Loading frames longer than this (s) are listed when the build is done.
const SLOW_FRAME_S: f32 = 0.1;

fn run_step(world: &mut World, build: &mut WorldBuild) {
    let (name, _, run) = STEPS[build.next];
    let t = std::time::Instant::now();
    build.last = "waiting";
    let laps = build.laps.len();
    if run(world, build) {
        // The zone demos lap each zone themselves.
        if build.laps.len() == laps {
            build.laps.push((name, t.elapsed().as_secs_f32()));
            build.last = name;
        }
        build.next += 1;
    }
}

fn finish(_world: &mut World, build: &WorldBuild) {
    let laps: Vec<String> = build
        .laps
        .iter()
        .map(|(w, s)| format!("{w} {s:.2}"))
        .collect();
    engine::core::log::info!(
        "showcase world built in {:.1} s ({})",
        build.started.elapsed().as_secs_f32(),
        laps.join(", ")
    );
    if !build.slow.is_empty() {
        let slow: Vec<String> = build
            .slow
            .iter()
            .map(|(w, s)| format!("after {w} {s:.2}"))
            .collect();
        engine::core::log::info!("slow loading frames (s): {}", slow.join(", "));
    }
}

/// The loading screen: the name, what is being built and a bar.
fn show_loading(world: &mut World, build: &WorldBuild) {
    screens::hide(world, "world_loading", true);
    let p = world.resource::<Theme>().palette.clone();
    // The zone demos' share of the bar fills zone by zone.
    let waking = STEPS.get(build.next).is_some_and(|s| s.0 == "zone demos");
    let part = if waking {
        build.zone as f32 / zones::ZONES.len() as f32
    } else {
        0.0
    };
    let done = (build.next as f32 + part) / STEPS.len() as f32;
    let what = STEPS
        .get(build.next)
        .map_or(String::new(), |s| match build.detail {
            Some(d) => format!("{}: {d}", s.1),
            None => s.1.to_string(),
        });
    let el = El::new()
        .with_style(|s| {
            s.position(Position::Absolute)
                .left(0.0)
                .right(0.0)
                .top(0.0)
                .bottom(0.0)
                .background(Color::hex(0x0b0f0d))
        })
        .child(
            El::new()
                .class("col")
                .with_style(|s| {
                    s.position(Position::Absolute)
                        .left(140.0)
                        .right(140.0)
                        .top(0.0)
                        .bottom(0.0)
                        .justify(Justify::Center)
                        .gap(14.0)
                })
                .child(
                    El::text("THE VALLEY")
                        .text_style(|t| t.bold().nowrap())
                        .with_style(|s| s.font_size(64.0).color(Color::WHITE)),
                )
                .child(
                    El::text(format!("{what}…"))
                        .text_style(|t| t.nowrap())
                        .with_style(|s| s.font_size(18.0).color(p.text_dim)),
                )
                .child(
                    El::new()
                        .with_style(|s| {
                            s.width(pct(100.0))
                                .max_width(520.0)
                                .height(4.0)
                                .radius(Corners::all(2.0))
                                .background(p.well)
                        })
                        .child(El::new().with_style(|s| {
                            s.width(pct(100.0 * done))
                                .height(4.0)
                                .radius(Corners::all(2.0))
                                .background(p.accent)
                        })),
                ),
        );
    if let Err(e) = screens::spawn_screen(
        world,
        Screen {
            key: "world_loading".into(),
            modal: true,
            layer: 30,
            closable: false,
            presence: 1.0,
            ..Default::default()
        },
        &el,
    ) {
        engine::core::log::warn!("loading screen: {e}");
    }
}

/// A bookmark jump (keys 1-9, 0, O, M) the exe holds back until the cover is drawn:
/// `camera::fly` leaves the eye and the point it looks at here instead of moving.
#[derive(Resource, Default)]
pub struct HeldJump(pub Option<(DVec3, DVec3)>);

/// While the exe's view jumps across the map (a bookmark key, the walk's start): the jump
/// held until the cover is up, and when it started, to cover the ground popping in until the
/// terrain there has its patches.
#[derive(Default)]
pub struct Jump {
    last: Option<DVec3>,
    since: Option<std::time::Instant>,
    held: Option<(DVec3, DVec3)>,
    /// Frames since the cover went up (while held) or since the view moved.
    frames: u32,
}

/// Other moves this far (m) in a frame are covered once they happen.
const JUMP_M: f64 = 150.0;

/// Frames the cover is drawn before a held jump moves the view, and at least after it.
const COVER_FRAMES: u32 = 2;

/// Cover a jump of the view with a dark screen until the terrain around it is built (at most
/// a few seconds), so the ground never pops in front of the player. Bookmark jumps wait for
/// the cover, so the frames that build the new place are covered too.
pub fn cover_jumps(world: &mut World, mut jump: Local<Jump>) {
    if building(world) {
        return;
    }
    if let Some(to) = world
        .get_resource_mut::<HeldJump>()
        .and_then(|mut h| h.0.take())
    {
        jump.held = Some(to);
        jump.frames = 0;
        if jump.since.is_none() {
            show_cover(world);
        }
        jump.since = Some(std::time::Instant::now());
    }
    jump.frames += 1;
    if let Some((eye, target)) = jump.held {
        if jump.frames <= COVER_FRAMES {
            return;
        }
        let mut q = world.query::<(&mut Transform, &mut camera::ShowcaseCamera)>();
        for (mut t, mut cam) in q.iter_mut(world) {
            camera::look(&mut t, &mut cam, eye, target);
        }
        jump.held = None;
        jump.last = Some(eye);
        jump.frames = 0;
        jump.since = Some(std::time::Instant::now());
        return;
    }
    let mut cameras = world.query::<(&Camera, &GlobalTransform)>();
    let Some(eye) = cameras
        .iter(world)
        .filter(|(c, _)| c.active)
        .max_by_key(|(c, _)| c.order)
        .map(|(_, t)| t.translation())
    else {
        return;
    };
    let jumped = jump.last.is_some_and(|l| l.distance(eye) > JUMP_M);
    jump.last = Some(eye);
    if jumped && jump.since.is_none() {
        jump.since = Some(std::time::Instant::now());
        jump.frames = 0;
        show_cover(world);
    }
    let Some(since) = jump.since else {
        return;
    };
    let pending = world
        .get_resource::<engine::core::diagnostics::Diagnostics>()
        .map_or(0.0, |d| {
            d.get("terrain.patches_building").unwrap_or(0.0)
                + d.get("terrain.patches_missing").unwrap_or(0.0)
        });
    let waited = since.elapsed().as_secs_f32();
    if (pending == 0.0 && waited > 0.3 && jump.frames > COVER_FRAMES) || waited > 4.0 {
        screens::hide(world, "jump_cover", false);
        jump.since = None;
    }
}

fn show_cover(world: &mut World) {
    let p = world.resource::<Theme>().palette.clone();
    let el = El::new()
        .with_style(|s| {
            s.position(Position::Absolute)
                .left(0.0)
                .right(0.0)
                .top(0.0)
                .bottom(0.0)
                .background(Color::hex(0x0b0f0d))
                .justify(Justify::Center)
                .align_items(Align::Center)
        })
        .child(
            El::text("Arriving…")
                .text_style(|t| t.nowrap())
                .with_style(|s| s.font_size(18.0).color(p.text_dim)),
        );
    if let Err(e) = screens::spawn_screen(
        world,
        Screen {
            key: "jump_cover".into(),
            modal: false,
            layer: 29,
            closable: false,
            presence: 1.0,
            ..Default::default()
        },
        &el,
    ) {
        engine::core::log::warn!("jump cover: {e}");
    }
}
