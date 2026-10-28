//! Building the valley, a step at a time. Tours, films and headless runs build it all at
//! startup. The exe draws a loading screen at once and builds one step a frame behind it, so
//! the window shows something straight away and the progress bar moves, then opens the start
//! screen.

use crate::landscape::{self, Landscape};
use crate::{camera, placeholders, planet, scenery, zones};
use engine::core::ecs::system::SystemState;
use engine::prelude::*;
use engine::stream::{StreamCell, StreamCellsExt, StreamingBudget};
use engine::world::{CellState, StreamedCells};
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
    lib: Option<Arc<scenery::Library>>,
    hedged: Vec<(DVec2, DVec2)>,
    started: std::time::Instant,
    laps: Vec<(&'static str, f32)>,
    /// What the loading screen names after the step's text (the zone waking).
    detail: Option<&'static str>,
    /// What ran last frame and when that frame began (wall clock: the engine's frame time
    /// is capped), and the frames of the staged build that took long, so a slow load says
    /// where its time went.
    ran: Vec<&'static str>,
    frame_at: Option<std::time::Instant>,
    slow: Vec<(String, f32)>,
    /// When the wait for the modules' art began.
    waiting: Option<std::time::Instant>,
    /// The library's mesh and texture work, on worker threads from the start.
    lib_job: Option<scenery::LibraryJob>,
    /// The profiler's window when the last frame began, and the slow frames whose window
    /// hasn't been published yet (with the window before each).
    window: Option<Window>,
    unprofiled: Vec<(String, f32, Option<Window>)>,
    /// The GPU meadow's field, mapped on a worker from the start.
    meadow_job: Option<std::thread::JoinHandle<GrassField>>,
    /// The meadow's grass, mapped, waiting for the village's gardens to clear theirs.
    meadow_field: Option<GrassField>,
}

/// A published profile window, told apart by its frame and system times.
type Window = (u64, u64);

/// The profiler's window as published ([`engine::core::profile::ProfileSnapshot`]): it
/// changes about once a second, at the end of a frame.
fn profile_window(world: &World) -> Option<Window> {
    world
        .get_resource::<engine::core::profile::ProfileSnapshot>()
        .filter(|p| !p.systems.is_empty())
        .map(|p| (p.frame_ms.to_bits(), p.busy_ms.to_bits()))
}

/// With `--profile`, the systems of each slow loading frame: the first window published
/// after the frame began is mostly that frame (it ends one when it runs a second or more,
/// and a slow frame that didn't is in the next one, with the frames it shares it with).
fn log_slow_systems(world: &World, build: &mut WorldBuild) {
    let Some(now) = profile_window(world) else {
        return;
    };
    let (ready, waiting): (Vec<_>, Vec<_>) = std::mem::take(&mut build.unprofiled)
        .into_iter()
        .partition(|(_, _, before)| *before != Some(now));
    build.unprofiled = waiting;
    if ready.is_empty() {
        return;
    }
    let snap = world.resource::<engine::core::profile::ProfileSnapshot>();
    let frames: Vec<String> = ready
        .iter()
        .map(|(label, dt, _)| format!("after {label} took {dt:.2} s"))
        .collect();
    let top: Vec<String> = snap
        .systems
        .iter()
        .take(8)
        .map(|(name, ms)| format!("{name} {ms:.0}"))
        .collect();
    engine::core::log::info!(
        "loading frame {}; its profile window: {:.0} ms a frame, systems {:.0} ms, slowest (ms a frame): {}",
        frames.join(", "),
        snap.frame_ms,
        snap.busy_ms,
        top.join(", ")
    );
}

/// Module art made on workers ahead of a zone's demo: the zone, how to start it (at the
/// start of the build) and whether it is in (the exe wakes the zone then; sooner, its spawn
/// would make what is left on the frame).
type Prefetch = (&'static str, fn(&mut World), fn(&World) -> bool);

const PREFETCH: [Prefetch; 3] = [
    ("water", water_prefetch, water_ready),
    ("lake", water_prefetch, water_ready),
    (
        "field",
        |_| engine_fire::props::prefetch_textures(),
        |_| engine_fire::props::textures_ready(),
    ),
];

fn water_prefetch(world: &mut World) {
    engine_water::demo::art::WaterDemoArt::prefetch(world);
}

fn water_ready(world: &World) -> bool {
    world.resource::<engine_water::WaterLoading>().is_ready()
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
const STEPS: [Step; 13] = [
    (
        "library",
        "Carving the stones and weaving the grass",
        |world, b| {
            let job = b.lib_job.take().unwrap_or_else(scenery::LibraryJob::start);
            // The exe keeps drawing its loading screen while the workers build.
            if world.contains_resource::<Staged>() && !job.is_ready() {
                b.lib_job = Some(job);
                return false;
            }
            let mut state = SystemState::<(
                ResMut<Assets<Mesh>>,
                ResMut<Assets<StandardMaterial>>,
                ResMut<Assets<Image>>,
                Res<engine_foliage::PlantLibrary>,
            )>::new(world);
            let (mut meshes, mut materials, mut images, plants) =
                state.get_mut(world).expect("resources the showcase adds");
            b.lib = Some(Arc::new(job.finish(
                &mut meshes,
                &mut materials,
                &mut images,
                &plants,
            )));
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
        let reach = world
            .get_resource::<scenery::TreeReach>()
            .copied()
            .unwrap_or_default();
        with_commands(world, |c| {
            scenery::spawn_woods(c, b.lib.as_ref().expect(LIB), &b.land, &mut b.rng, reach)
        });
        true
    }),
    ("meadows", "Growing the meadows", |world, b| {
        let field = match b.meadow_job.take() {
            // The exe keeps drawing its loading screen while the worker maps it.
            Some(job) if world.contains_resource::<Staged>() && !job.is_finished() => {
                b.meadow_job = Some(job);
                return false;
            }
            Some(job) => job.join().expect("meadow thread"),
            None => scenery::meadow_grass(&b.land),
        };
        with_commands(world, |c| {
            scenery::spawn_meadows(c, b.lib.as_ref().expect(LIB), &b.land, &mut b.rng);
        });
        // Spawned with the village, once the gardens have cleared their grass.
        b.meadow_field = Some(field);
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
        let gardens = !world.contains_resource::<crate::garden::NoGardens>();
        let seed = world.resource::<EngineConfig>().seed;
        let mut state = SystemState::<(Commands, ResMut<Assets<Mesh>>)>::new(world);
        let (mut commands, mut meshes) = state.get_mut(world).expect("resources the showcase adds");
        let lib = b.lib.as_ref().expect(LIB);
        let plan = scenery::spawn_village(&mut commands, lib, &b.land, &mut b.rng);
        let gardens = if gardens {
            scenery::spawn_gardens(&mut commands, &mut meshes, lib, &b.land, &plan, seed)
        } else {
            Vec::new()
        };
        if let Some(mut field) = b.meadow_field.take() {
            scenery::clear_gardens(&mut field, &gardens);
            commands.spawn((Transform::default(), field));
        }
        state.apply(world);
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
        if let Some(only) = only.as_ref() {
            engine::core::log::info!(
                "only the zone demos of {:?} (`--zones all` spawns every one)",
                only.0
            );
        }
        // The zones whose art bakes on workers wait for it, so their spawn is quick.
        if staged
            && PREFETCH.iter().any(|(key, _, ready)| {
                only.as_ref().is_none_or(|o| o.0.iter().any(|k| k == key)) && !ready(world)
            })
        {
            return false;
        }
        // Each zone is a streamed cell: in the exe it wakes when the player comes near (and
        // the placeholders' zones sleep again behind them), as Unreal's World Partition
        // streams runtime cells; tours, films and tools load every one at once, as before.
        let seed = world.resource::<EngineConfig>().seed;
        let lib = b.lib.clone().expect(LIB);
        for cell in zones::zone_cells(&b.land, only.as_ref()) {
            let (lib, land, key) = (lib.clone(), b.land.clone(), cell.key);
            let mut streamed = StreamCell::new(key, cell.center, cell.radius, move |world| {
                with_commands(world, |c| zones::spawn_cell(c, &lib, &land, seed, key));
            })
            .load_within(if staged { ZONE_LOAD_M } else { f64::INFINITY });
            if cell.keep_loaded {
                streamed = streamed.keep_loaded();
            }
            world.stream_cell(streamed);
        }
        if !staged {
            world.insert_resource(StreamingBudget {
                loads_per_frame: usize::MAX,
                frame_ms: f32::INFINITY,
                ..Default::default()
            });
        }
        true
    }),
    ("nearby zones", "Waking the zones nearby", |world, b| {
        // The exe opens once the zones around the camera are in (the streaming plugin wakes
        // one a frame); the rest wake as the player gets near them.
        if !world.contains_resource::<Staged>() {
            return true;
        }
        let cells = world.resource::<StreamedCells>();
        let waking = cells.waking().next();
        b.detail = waking
            .and_then(|c| zones::ZONES.iter().find(|z| z.key == c.key))
            .map(|z| z.name);
        waking.is_none()
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
    (
        "module art",
        "Planting the woods, filling the streams, sending it all to the GPU",
        |world, b| {
            // Modules that bake a zone's art on workers say so; the exe waits for them here, so
            // the start screen opens on a finished valley. Builds all at once (tours) go on,
            // and so does a wait that never ends.
            if !world.contains_resource::<Staged>() || !MODULES_LOADING.iter().any(|f| f(world)) {
                return true;
            }
            let since = *b.waiting.get_or_insert_with(std::time::Instant::now);
            if since.elapsed().as_secs_f32() > MODULE_WAIT_S {
                engine::core::log::warn!("modules still loading after {MODULE_WAIT_S} s; going on");
                return true;
            }
            false
        },
    ),
];

/// In the exe a zone wakes when the camera or the player comes this close to its edge, m:
/// a sprinting walker takes over 10 s to cover it, and the start screen then waits for about
/// half the zones around the plaza instead of twelve (at 150 m).
const ZONE_LOAD_M: f64 = 100.0;

/// Longest wait (s) for the modules' art before the start screen opens anyway.
const MODULE_WAIT_S: f32 = 60.0;

/// Modules still baking art or settling their zones on other threads.
const MODULES_LOADING: [fn(&World) -> bool; 8] = [
    engine_smoke::demo::loading,
    engine_fire::demo::loading,
    |world| {
        world
            .get_resource::<engine_water::WaterLoading>()
            .is_some_and(|w| !w.is_ready())
    },
    |world| !world.contains_resource::<engine_destruction::assets::DestructionAssets>(),
    // Its prop looks and voxel volumes follow on bake threads a few frames later.
    engine_destruction::is_loading,
    |world| {
        world
            .get_resource::<engine_foliage::FoliageLoading>()
            .is_some_and(|f| !f.is_ready())
    },
    uploading,
    // Mesh, impostor and material pipelines compiling on workers, so the walk doesn't hitch
    // on first sight.
    |world| {
        world
            .get_resource::<engine::core::diagnostics::Diagnostics>()
            .is_some_and(|d| {
                [
                    "render.meshes.pipelines_warming",
                    "render.materials.pipelines_warming",
                ]
                .iter()
                .any(|k| d.get(k).is_some_and(|n| n > 0.0))
            })
    },
];

/// The renderer still has meshes or images to put on the GPU (it uploads within the
/// [`engine::world::LoadingBudget`] while one is up).
fn uploading(world: &World) -> bool {
    world
        .get_resource::<Uploads>()
        .is_some_and(Uploads::loading)
}

/// What the renderer still has to upload, and when that last reached a new low: a count that
/// stops going down (some of it may never be asked for) stops counting as loading.
#[derive(Resource)]
pub struct Uploads {
    pending: f64,
    low: f64,
    low_at: std::time::Instant,
}

impl Default for Uploads {
    fn default() -> Self {
        Self {
            pending: 0.0,
            low: 0.0,
            low_at: std::time::Instant::now(),
        }
    }
}

impl Uploads {
    /// Uploads are pending and still going down.
    fn loading(&self) -> bool {
        self.pending > 0.0 && self.low_at.elapsed().as_secs_f32() < UPLOAD_STALL_S
    }
}

/// Seconds without the pending uploads going down before the wait for them ends.
const UPLOAD_STALL_S: f32 = 2.0;

/// Follow `render.pending_upload_mb` for [`uploading`].
pub fn watch_uploads(
    diagnostics: Option<Res<engine::core::diagnostics::Diagnostics>>,
    mut u: ResMut<Uploads>,
) {
    let mb = diagnostics
        .and_then(|d| d.get("render.pending_upload_mb"))
        .unwrap_or(0.0);
    u.pending = mb;
    // Rising (new work asked for) or falling by a megabyte counts as movement.
    if mb < u.low - 1.0 || mb > u.low + 1.0 {
        u.low = mb;
        u.low_at = std::time::Instant::now();
    }
}

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
    // Module art for the build's zones starts on workers now.
    let only = world.get_resource::<zones::DemoZones>().cloned();
    for (key, prefetch, _) in PREFETCH {
        if only.as_ref().is_none_or(|o| o.0.iter().any(|k| k == key)) {
            prefetch(world);
        }
    }
    let meadow_job = {
        let land = land.clone();
        std::thread::spawn(move || scenery::meadow_grass(&land))
    };
    let mut build = WorldBuild {
        next: 0,
        land,
        rng: Rng::new(seed),
        lib: None,
        hedged: Vec::new(),
        started,
        laps: vec![("ground and sky", started.elapsed().as_secs_f32())],
        detail: None,
        ran: Vec::new(),
        frame_at: None,
        waiting: None,
        // Started before anything else, so the workers overlap the rest of startup.
        lib_job: Some(scenery::LibraryJob::start()),
        meadow_job: Some(meadow_job),
        meadow_field: None,
        window: None,
        unprofiled: Vec::new(),
        slow: Vec::new(),
    };
    if world.contains_resource::<Staged>() {
        show_loading(world, &build);
        // Modules may spend more of each frame on their loading work while this is up.
        world.insert_resource(engine::world::LoadingBudget::default());
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
    // The frame before this one: what it ran (the steps, the systems answering what they
    // spawned, and the drawing) and how long it took.
    let now = std::time::Instant::now();
    if let Some(at) = build.frame_at.replace(now) {
        let dt = now.duration_since(at).as_secs_f32();
        if dt > SLOW_FRAME_S {
            let ran = if build.ran.is_empty() {
                "waiting".to_string()
            } else {
                build.ran.join("+")
            };
            if dt > PROFILE_FRAME_S {
                build.unprofiled.push((ran.clone(), dt, build.window));
            }
            build.slow.push((ran, dt));
        }
    }
    log_slow_systems(world, &mut build);
    build.window = profile_window(world);
    build.ran.clear();
    // Steps run until this frame's budget is spent, as Unity's High background loading
    // priority (50 ms a frame behind a loading screen) and Unreal's streaming time limits do:
    // cheap steps share a frame, a heavy one has a frame to itself. Zones wake one a frame
    // in the streaming plugin (their modules' systems build what a demo needs later in the
    // frame: the PC showed seconds of that behind a spawn of a few milliseconds).
    loop {
        let at = build.next;
        run_step(world, &mut build);
        let moved = build.next != at;
        if !moved || build.next >= STEPS.len() || now.elapsed().as_secs_f32() > FRAME_BUDGET_S {
            break;
        }
    }
    if build.next < STEPS.len() {
        show_loading(world, &build);
        world.insert_resource(build);
    } else {
        screens::hide(world, "world_loading", false);
        world.remove_resource::<engine::world::LoadingBudget>();
        finish(world, &build);
    }
}

/// Whether the world is still being built (the start screen waits).
pub fn building(world: &World) -> bool {
    world.contains_resource::<WorldBuild>()
}

/// Loading frames longer than this (s) are listed when the build is done.
const SLOW_FRAME_S: f32 = 0.1;

/// Loading frames longer than this (s) get the profile's slowest systems logged.
const PROFILE_FRAME_S: f32 = 1.0;

/// How long (s) a loading frame keeps starting steps.
const FRAME_BUDGET_S: f32 = 0.05;

fn run_step(world: &mut World, build: &mut WorldBuild) {
    let (name, _, run) = STEPS[build.next];
    let t = std::time::Instant::now();
    let laps = build.laps.len();
    if run(world, build) {
        // The zone demos lap each zone themselves.
        if build.laps.len() == laps {
            build.laps.push((name, t.elapsed().as_secs_f32()));
            build.ran.push(name);
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
    // The nearby zones' share of the bar fills zone by zone.
    let waking = STEPS.get(build.next).is_some_and(|s| s.0 == "nearby zones");
    let part = match world.get_resource::<StreamedCells>().filter(|_| waking) {
        Some(cells) => {
            let loaded = cells
                .cells
                .iter()
                .filter(|c| c.state == CellState::Loaded)
                .count() as f32;
            let waking = cells.waking().count() as f32;
            loaded / (loaded + waking).max(1.0)
        }
        None => 0.0,
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
/// `camera::fly` keeps the eye and the point it looks at here, and moves the view
/// [`COVER_FRAMES`] frames later.
#[derive(Resource, Default)]
pub struct HeldJump {
    pub to: Option<(DVec3, DVec3)>,
    pub frames: u32,
    /// Set by `camera::fly` the frame it moved the view.
    pub moved: bool,
}

/// While the exe's view jumps across the map (a bookmark key, the walk's start): where it
/// was last frame, when the cover went up and frames since the view moved, to cover the
/// ground popping in until the terrain there has its patches.
#[derive(Default)]
pub struct Jump {
    last: Option<DVec3>,
    since: Option<std::time::Instant>,
    frames: u32,
    /// When this system last ran (wall clock), and frames in a row since the move that took
    /// less than [`CALM_FRAME_S`].
    tick: Option<std::time::Instant>,
    calm: u32,
}

/// A frame this short (s) counts as settled; the cover lifts after [`CALM_FRAMES`] of them,
/// so the hitch of drawing a new place for the first time (uploads, first-sight builds)
/// stays behind it.
const CALM_FRAME_S: f32 = 0.25;
const CALM_FRAMES: u32 = 3;

/// Longest a jump stays covered (s).
const COVER_MAX_S: f32 = 6.0;

/// Other moves this far (m) in a frame are covered once they happen.
const JUMP_M: f64 = 150.0;

/// Frames the cover is drawn before a held jump moves the view, and at least after it.
pub const COVER_FRAMES: u32 = 2;

/// What the jump cover does this frame: show (true) or lift (false). [`track_jumps`] decides
/// and [`apply_cover`] does it, so only a frame that changes the cover holds the whole world.
#[derive(Resource, Default)]
pub struct CoverOrder(Option<bool>);

/// Whether [`apply_cover`] has something to do.
pub fn cover_ordered(order: Res<CoverOrder>) -> bool {
    order.0.is_some()
}

/// Cover a jump of the view with a dark screen until the terrain around it is built and the
/// frames have settled (at most a few seconds), so neither the ground popping in nor the
/// hitch of a place's first frames shows. Bookmark jumps wait for
/// the cover, so the frames that build the new place are covered too.
#[allow(clippy::too_many_arguments)]
pub fn track_jumps(
    mut jump: Local<Jump>,
    mut held: ResMut<HeldJump>,
    mut order: ResMut<CoverOrder>,
    build: Option<Res<WorldBuild>>,
    uploads: Res<Uploads>,
    diagnostics: Option<Res<engine::core::diagnostics::Diagnostics>>,
    cameras: Query<(&Camera, &GlobalTransform)>,
) {
    if build.is_some() {
        return;
    }
    let now = std::time::Instant::now();
    let dt = jump
        .tick
        .replace(now)
        .map_or(0.0, |t| now.duration_since(t).as_secs_f32());
    jump.calm = if dt < CALM_FRAME_S { jump.calm + 1 } else { 0 };
    let (holding, moved) = (held.to.is_some(), std::mem::take(&mut held.moved));
    if holding {
        if jump.since.is_none() {
            order.0 = Some(true);
            jump.since = Some(now);
        }
        return;
    }
    if moved {
        jump.since = Some(now);
        jump.frames = 0;
        jump.calm = 0;
    }
    jump.frames += 1;
    let Some(eye) = cameras
        .iter()
        .filter(|(c, _)| c.active)
        .max_by_key(|(c, _)| c.order)
        .map(|(_, t)| t.translation())
    else {
        return;
    };
    let jumped = jump.last.is_some_and(|l| l.distance(eye) > JUMP_M);
    jump.last = Some(eye);
    if jumped && jump.since.is_none() {
        jump.since = Some(now);
        jump.frames = 0;
        jump.calm = 0;
        order.0 = Some(true);
    }
    let Some(since) = jump.since else {
        return;
    };
    let pending = diagnostics.map_or(0.0, |d| {
        d.get("terrain.patches_building").unwrap_or(0.0)
            + d.get("terrain.patches_missing").unwrap_or(0.0)
    });
    let waited = since.elapsed().as_secs_f32();
    let settled = pending == 0.0
        && !uploads.loading()
        && jump.frames > COVER_FRAMES
        && jump.calm >= CALM_FRAMES;
    if (settled && waited > 0.3) || waited > COVER_MAX_S {
        order.0 = Some(false);
        jump.since = None;
    }
}

/// Put the jump cover up or take it down, as [`track_jumps`] ordered.
pub fn apply_cover(world: &mut World) {
    match world.resource_mut::<CoverOrder>().0.take() {
        Some(true) => show_cover(world),
        Some(false) => {
            screens::hide(world, "jump_cover", false);
            world.remove_resource::<engine::world::LoadingBudget>();
        }
        None => {}
    }
}

fn show_cover(world: &mut World) {
    // Behind the cover, as behind the loading screen, modules may load faster.
    world.insert_resource(engine::world::LoadingBudget::default());
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
