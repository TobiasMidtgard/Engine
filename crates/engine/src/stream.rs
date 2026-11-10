//! Loads and unloads the world's cells around the streaming sources ([`engine_world::stream`]).
//!
//! A game registers each cell with [`StreamCellsExt::stream_cell`]: where it is, how far its
//! content reaches, when to load, and how to spawn it. Every frame (`Last`) the plugin finds
//! the cells a source wants, spawns the nearest queued ones within a per-frame budget, and
//! despawns cells every source has left by the hysteresis margin, as Unreal's World Partition
//! loads runtime cells around its streaming sources. Entities a cell's spawn creates get
//! [`CellMember`], so its unload takes exactly them (with their children).
//!
//! A cell that is slow to make splits the work the way open-world engines do (Unreal's async
//! level loading, Unity's entity scenes): a world-free **build** step ([`StreamCell::built`])
//! runs on a worker thread while the game plays (meshes, noise, layouts), and the **place**
//! step that touches the world can run in slices, a few a frame ([`StreamCell::in_steps`]), so
//! no single frame hitches. While a cell loads, and for a few frames after (shaders and uploads
//! warm up), its meshes stay hidden; then they fade in with [`DitherFade`] instead of popping.
//! A cell leaving fades out the same way over its far-view proxy ([`crate::hlod`]) and then
//! despawns a slice of roots a frame within the budget.
//!
//! Assets the cell's spawn added are its own ([`crate::cell_assets`]); a cell that opts in with
//! [`StreamCell::release_assets`] frees them when it unloads.
//!
//! A moving source wants the cells along its path (its [`Velocity`], or its motion measured
//! frame to frame for a camera or focus without one; one frame's jump is not motion), each one
//! from the moment it is its lookahead away, or twice the time that cell took to load last time
//! if that is longer ([`StreamLatency`] for a cell that has never loaded), though never from
//! farther off for that than [`StreamingBudget::stretch_loads`] of its load distances. So a
//! light cell far down a fast source's path waits until the source is nearly there, and a turn
//! before then costs nothing; a slow one is asked for earlier, but a source too fast for its
//! cells sees them fade in late over their far-view proxies rather than waking everything for
//! kilometers ahead. A cell once asked for is held by the lead it was asked with, even when
//! it then loads faster than last time. A loaded cell the path has swung away
//! from stays for [`StreamingBudget::linger_s`] before it leaves, so looking around in flight
//! doesn't load and unload the same cells over and over (`stream.unneeded_loads` counts loads
//! that left without a source ever coming within their load distance); a jump (a teleport,
//! fast travel) or a crossed memory cap ends every cell's lingering at once. Cells load in the order
//! the source will need them: those it stands in
//! first, then by when its path gets there. Loading keeps to [`StreamingBudget::frame_ms`]
//! a frame, and to the larger `late_frame_ms` while a source stands in a cell that is not in
//! yet (`stream.late` counts those, the pop-in and hole the player sees).

use std::any::Any;
use std::sync::mpsc::{self, Receiver, TryRecvError};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use crate::cell_assets::{self, CellAssets, Marks};
use crate::cell_memory;
use crate::hlod::{self, ProxyConfig};
use engine_core::diagnostics::{Diagnostics, PROCESS_RAM_MB, sampled_ram_mb};
use engine_core::ecs::resource::IsResource;
use engine_core::math::DQuat;
use engine_core::off_frame;
use engine_core::prelude::*;
use engine_render::{DitherFade, MeshRenderer};
use engine_world::physics::Velocity;
use engine_world::shift::{FollowsWorldShift, WorldShift, WorldShiftAppExt};
use engine_world::stream::CellInfo;
use engine_world::{
    CellId, CellMember, CellState, FocusPoints, SimulationFocus, StreamFrame, StreamedCells,
    StreamingSource,
};

type Hook = Box<dyn FnMut(&mut World) + Send + Sync>;
type Built = Box<dyn Any + Send>;
type BuildFn = Arc<dyn Fn() -> Built + Send + Sync>;
/// One slice of placing a cell: the world, the build's output (cells with a build step) and the
/// slice's index from 0; true once the cell is all in.
type StepFn = Box<dyn FnMut(&mut World, Option<&mut Built>, usize) -> bool + Send + Sync>;

/// A cell to stream: build with [`StreamCell::new`] (or [`StreamCell::in_steps`],
/// [`StreamCell::built`], [`StreamCell::built_in_steps`]), register with
/// [`StreamCellsExt::stream_cell`].
pub struct StreamCell {
    info: CellInfo,
    loader: Loader,
}

struct Loader {
    build: Option<BuildFn>,
    step: StepFn,
    unload: Option<Hook>,
    proxy: ProxyConfig,
    release: bool,
    remember: bool,
}

impl StreamCell {
    fn with(
        key: impl Into<String>,
        center: DVec3,
        radius: f64,
        build: Option<BuildFn>,
        step: StepFn,
    ) -> Self {
        Self {
            info: CellInfo {
                key: key.into(),
                center,
                radius,
                load_m: 150.0,
                keep_loaded: false,
                state: CellState::Unloaded,
                load_secs: 0.0,
                build_secs: 0.0,
                entities: 0,
                ram_mb: None,
                assets: 0,
                assets_mb: 0.0,
                releases_assets: false,
                frame: None,
                retired: false,
            },
            loader: Loader {
                build,
                step,
                unload: None,
                proxy: ProxyConfig::default(),
                release: false,
                remember: false,
            },
        }
    }

    /// A cell named `key` whose content lies within `radius` of `center`, spawned by `spawn`
    /// in one go (anything it spawns, directly or through commands it applies, belongs to the
    /// cell). It loads when a source is within 150 m of its edge.
    pub fn new(
        key: impl Into<String>,
        center: DVec3,
        radius: f64,
        mut spawn: impl FnMut(&mut World) + Send + Sync + 'static,
    ) -> Self {
        Self::with(
            key,
            center,
            radius,
            None,
            Box::new(move |world, _, _| {
                spawn(world);
                true
            }),
        )
    }

    /// A cell spawned in slices: `step(world, i)` runs for i = 0, 1, 2… (one or more a frame,
    /// within [`StreamingBudget::frame_ms`]) until it returns true. Its meshes stay hidden
    /// until the last slice is in.
    pub fn in_steps(
        key: impl Into<String>,
        center: DVec3,
        radius: f64,
        mut step: impl FnMut(&mut World, usize) -> bool + Send + Sync + 'static,
    ) -> Self {
        Self::with(
            key,
            center,
            radius,
            None,
            Box::new(move |world, _, i| step(world, i)),
        )
    }

    /// A cell made in two parts: `build()` runs on a worker thread when the cell is wanted
    /// (it can't see the world: meshes, textures, noise, layouts), then `place(world, built)`
    /// spawns the result on the main thread in its turn.
    pub fn built<T: Send + 'static>(
        key: impl Into<String>,
        center: DVec3,
        radius: f64,
        build: impl Fn() -> T + Send + Sync + 'static,
        mut place: impl FnMut(&mut World, T) + Send + Sync + 'static,
    ) -> Self {
        Self::built_in_steps(
            key,
            center,
            radius,
            move || Some(build()),
            move |world, built: &mut Option<T>, _| {
                if let Some(built) = built.take() {
                    place(world, built);
                }
                true
            },
        )
    }

    /// [`StreamCell::built`] whose place step runs in slices, as [`StreamCell::in_steps`]:
    /// `place(world, &mut built, i)` until it returns true.
    pub fn built_in_steps<T: Send + 'static>(
        key: impl Into<String>,
        center: DVec3,
        radius: f64,
        build: impl Fn() -> T + Send + Sync + 'static,
        mut place: impl FnMut(&mut World, &mut T, usize) -> bool + Send + Sync + 'static,
    ) -> Self {
        let build: BuildFn = Arc::new(move || Box::new(build()) as Built);
        Self::with(
            key,
            center,
            radius,
            Some(build),
            Box::new(
                move |world, built, i| match built.and_then(|b| (**b).downcast_mut::<T>()) {
                    Some(built) => place(world, built, i),
                    None => true,
                },
            ),
        )
    }

    /// Load when a source is within `meters` of the cell's edge.
    pub fn load_within(mut self, meters: f64) -> Self {
        self.info.load_m = meters;
        self
    }

    /// Never unload once loaded: for content whose module keeps state outside its entities
    /// and has no [`StreamCell::on_unload`] yet.
    pub fn keep_loaded(mut self) -> Self {
        self.info.keep_loaded = true;
        self
    }

    /// No far-view proxy for this cell ([`crate::hlod`]): for content that moves, or that is
    /// never seen from afar.
    pub fn without_proxy(mut self) -> Self {
        self.loader.proxy.enabled = false;
        self
    }

    /// Keep the cell's far-view proxy on disk for this content version, so later runs show it
    /// before the cell is ever visited: change `version` whenever what the cell spawns changes
    /// (a hash of the seed and the build, say).
    pub fn proxy_version(mut self, version: u64) -> Self {
        self.loader.proxy.version = Some(version);
        self
    }

    /// Free the meshes, materials and images the cell's spawn added when it unloads, except
    /// keyed (shared) ones and any a [`MeshRenderer`] outside the cell still draws
    /// ([`crate::cell_assets`]). Opt in once the module that spawns the cell keeps no handle
    /// to them past the unload (it would read as missing on the next load; the stream audit
    /// counts those).
    pub fn release_assets(mut self) -> Self {
        self.loader.release = true;
        self.info.releases_assets = true;
        self
    }

    /// The cell rides with the [`StreamFrame`] named `frame` (a planet, a moon, a station):
    /// its center, given in the world as the frame stands when it first shows up, keeps its
    /// place on the frame however the frame moves later, and so does its far-view proxy. Its
    /// entities are the frame owner's to carry (space parks them on their body).
    pub fn on_frame(mut self, frame: impl Into<String>) -> Self {
        self.info.frame = Some(frame.into());
        self
    }

    /// What happens in the cell survives its unload ([`engine_world::CellMemory`]): what was
    /// destroyed stays gone and what moved or changed (`Transform` and the components modules
    /// add to the memory) is set again when it comes back. Its spawn must make the same
    /// entities, with the same names and places, every time (as a scene document does).
    pub fn remember(mut self) -> Self {
        self.loader.remember = true;
        self
    }

    /// Runs after the cell's entities are despawned, to drop what its module keeps elsewhere
    /// (a resource, a cache).
    pub fn on_unload(mut self, unload: impl FnMut(&mut World) + Send + Sync + 'static) -> Self {
        self.loader.unload = Some(Box::new(unload));
        self
    }
}

/// Registers streamed cells.
pub trait StreamCellsExt {
    /// Adds `cell`. A cell already registered under its key (a scene loaded again, a hot
    /// reload) is replaced: it unloads now if it was in, and the new one takes its place.
    fn stream_cell(&mut self, cell: StreamCell) -> &mut Self;
}

impl StreamCellsExt for World {
    fn stream_cell(&mut self, cell: StreamCell) -> &mut Self {
        self.init_resource::<StreamedCells>();
        self.init_resource::<CellHooks>();
        let at = self.resource::<StreamedCells>().find(&cell.info.key);
        // Within a cell's own spawn the slots are lent out; a cell it registers is added.
        let slots = self.resource::<CellHooks>().0.len();
        match at.filter(|id| (id.0 as usize) < slots) {
            Some(id) => {
                let i = id.0 as usize;
                take_out(self, i);
                self.resource_mut::<StreamedCells>().cells[i] = cell.info;
                self.resource_mut::<CellHooks>().0[i] = Slot::new(cell.loader);
            }
            None => {
                // A retired cell's slot takes the new one (procedural grids retire and make
                // cells all trip long).
                let free = self.resource::<StreamedCells>().cells
                    [..slots.min(self.resource::<StreamedCells>().cells.len())]
                    .iter()
                    .position(|c| c.retired);
                match free {
                    Some(i) => {
                        self.resource_mut::<StreamedCells>().cells[i] = cell.info;
                        self.resource_mut::<CellHooks>().0[i] = Slot::new(cell.loader);
                    }
                    None => {
                        self.resource_mut::<StreamedCells>().cells.push(cell.info);
                        self.resource_mut::<CellHooks>()
                            .0
                            .push(Slot::new(cell.loader));
                    }
                }
            }
        }
        self
    }
}

/// Takes the cell named `key` out of streaming: it unloads now if it is in (its entities go,
/// its assets are freed if it releases them, its proxy goes) and never loads again, until a
/// cell is registered under its key again. False if there is no such cell.
pub fn unstream_cell(world: &mut World, key: &str) -> bool {
    let Some(id) = world
        .get_resource::<StreamedCells>()
        .and_then(|c| c.find(key))
    else {
        return false;
    };
    let i = id.0 as usize;
    if world.resource::<CellHooks>().0.len() <= i {
        return false;
    }
    take_out(world, i);
    let mut cells = world.resource_mut::<StreamedCells>();
    cells.cells[i].retired = true;
    cells.pins.remove(key);
    world.resource_mut::<CellHooks>().0[i] = Slot::new(Loader {
        build: None,
        step: Box::new(|_, _, _| true),
        unload: None,
        proxy: ProxyConfig {
            enabled: false,
            version: None,
        },
        release: false,
        remember: false,
    });
    true
}

/// Cell `i` unloads now, whatever it was doing, and its far-view proxy is dropped.
fn take_out(world: &mut World, i: usize) {
    let state = world.resource::<StreamedCells>().cells[i].state;
    if state != CellState::Unloaded && world.contains_resource::<Fades>() {
        unload(world, i);
    }
    world.resource_mut::<CellHooks>().0[i].reset();
    off_frame::drop_scope(world, art_scope(CellId(i as u32)));
    world.resource_mut::<StreamedCells>().cells[i].state = CellState::Unloaded;
    hlod::forget(world, i);
}

impl StreamCellsExt for App {
    fn stream_cell(&mut self, cell: StreamCell) -> &mut Self {
        self.world.stream_cell(cell);
        self
    }
}

/// How much loading one frame may do, the worker builds, the fade-in and the memory caps.
#[derive(Resource, Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(default)]
pub struct StreamingBudget {
    /// Cells that start spawning per frame at most.
    pub loads_per_frame: usize,
    /// No further spawn slice runs once this frame's loading took this long, ms (the first
    /// always runs, so loading never stalls). Cells loaded ahead of need keep to this, so a
    /// streaming frame stays inside a 60 fps frame.
    pub frame_ms: f32,
    /// The same while a cell a source already stands in is still not in (it shows as a hole or
    /// pops in late): catching up beats a smooth frame, as Unreal's World Partition blocks on
    /// slow streaming. Counted in `stream.late`.
    pub late_frame_ms: f32,
    /// Build steps running on worker threads at once at most.
    pub builds: usize,
    /// Seconds a newly loaded cell's meshes take to fade in (0 shows them at once). Only while
    /// something streams: with no source (tools, tours) cells show at once.
    pub fade_s: f32,
    /// Frames a loaded cell stays hidden before it fades in, while its uploads land.
    pub fade_frames: u32,
    /// Seconds a cell stays after the last moving source's path left it (by the hysteresis
    /// margin), in case the source turns back: a load is far dearer than a few seconds of
    /// memory. A cell left by sources that stand still (or jumped away) goes at once, and so
    /// does one the memory caps below pick.
    pub linger_s: f32,
    /// A source asks for a slow cell earlier than its own lookahead (twice the cell's load
    /// time ahead), but looks no farther down its path for it than this many of the cell's
    /// load distances: past that the source is too fast for the cell, and asking earlier
    /// still would load cells far down a path it may never fly.
    pub stretch_loads: f64,
    /// Over this much process RAM (MB, `process.ram_mb`), cells no source needs right now
    /// (loaded, outside their load distance, not kept) unload early, farthest first, one a
    /// frame, instead of waiting for the hysteresis margin.
    pub ram_mb: Option<f64>,
    /// The same for GPU memory (MB, `render.gpu_memory_mb`).
    pub vram_mb: Option<f64>,
}

impl Default for StreamingBudget {
    fn default() -> Self {
        Self {
            loads_per_frame: 1,
            frame_ms: 3.0,
            late_frame_ms: 8.0,
            builds: 2,
            fade_s: 0.4,
            fade_frames: 5,
            linger_s: 4.0,
            stretch_loads: 2.0,
            ram_mb: None,
            vram_mb: None,
        }
    }
}

impl StreamingBudget {
    /// Whether memory is over a cap, from the [`Diagnostics`] metrics.
    pub fn over(&self, diagnostics: &Diagnostics) -> bool {
        let over = |cap: Option<f64>, metric: &str| {
            cap.zip(diagnostics.get(metric))
                .is_some_and(|(cap, mb)| mb > cap)
        };
        over(self.ram_mb, PROCESS_RAM_MB) || over(self.vram_mb, "render.gpu_memory_mb")
    }
}

/// A cell's loader and where its current load is.
struct Slot {
    loader: Loader,
    /// The worker's answer: the build's output and its seconds, or a dropped sender if it
    /// panicked.
    /// (Behind locks only so the resource is `Sync`; the plugin reaches them through `&mut`.)
    building: Option<Mutex<Receiver<(Built, f32)>>>,
    built: Option<Mutex<Built>>,
    /// A build step panicked: the cell stays out.
    failed: bool,
    step: usize,
    /// The loader's last slice ran; the load waits only for its off-frame art.
    spawned: bool,
    secs: f32,
    ram: Option<f64>,
    /// What the spawn added since the cell last unloaded.
    assets: CellAssets,
    /// The center in its frame's space, once the frame has shown up.
    local: Option<DVec3>,
    /// Twins (same name, same place) given ids so far in this load.
    ids_seen: std::collections::HashMap<u64, u32>,
    /// A remembering cell's spawned values, from its last load.
    baseline: Option<cell_memory::Baseline>,
    /// When a source first wanted this load (for [`before_per_cell_leads`]).
    wanted_at: Option<Instant>,
    /// When this load's work began: its build went to a worker, or its first slice ran.
    started_at: Option<Instant>,
    /// When this load finished.
    loaded_at: Option<Instant>,
    /// Seconds the last load took from `started_at` to loaded: what a source's lookahead for
    /// this cell goes by.
    took: Option<f64>,
    /// A source has been within the cell's load distance during this load.
    needed: bool,
    /// Seconds every source's path has been away from it (beyond the hysteresis margin).
    unwanted_s: f32,
    /// The load time this load was asked for by (the cell's last, or what cells take lately
    /// for one never loaded): it is held by the lead that gave, even once its own shrinks.
    asked_secs: f64,
}

impl Slot {
    fn new(loader: Loader) -> Self {
        Self {
            loader,
            building: None,
            built: None,
            failed: false,
            step: 0,
            spawned: false,
            secs: 0.0,
            ram: None,
            assets: CellAssets::default(),
            local: None,
            ids_seen: Default::default(),
            baseline: None,
            wanted_at: None,
            started_at: None,
            loaded_at: None,
            took: None,
            needed: false,
            unwanted_s: 0.0,
            asked_secs: 0.0,
        }
    }

    /// Forget a load in progress (an unwanted build's answer is dropped when it comes).
    fn reset(&mut self) {
        self.building = None;
        self.built = None;
        self.step = 0;
        self.spawned = false;
        self.secs = 0.0;
        self.ram = None;
        self.ids_seen.clear();
        self.wanted_at = None;
        self.started_at = None;
        self.loaded_at = None;
        self.needed = false;
        self.unwanted_s = 0.0;
        self.asked_secs = 0.0;
    }
}

/// Loads finished so far, and how many of them left again without a source ever coming within
/// their load distance (`stream.loads`, `stream.unneeded_loads`).
#[derive(Resource, Default)]
struct LoadCounts {
    loads: u64,
    unneeded: u64,
    /// Smoothed seconds from wanted to loaded (for [`before_per_cell_leads`]).
    wanted_lag: f64,
}

/// `ENGINE_STREAM_LEADS=old`: the lookahead as it was before cells had leads of their own, for
/// before/after runs of one build (`showcase --route lap` on the PC). Every cell on a moving
/// source's path is asked for at once, as far ahead as twice the time cells have been taking
/// from wanted to loaded (up to [`MAX_LOOKAHEAD_S`]), and a cell the path leaves goes at once.
fn before_per_cell_leads() -> bool {
    static OLD: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *OLD.get_or_init(|| std::env::var("ENGINE_STREAM_LEADS").is_ok_and(|v| v == "old"))
}

/// What cells take to load lately, as the lookahead counts it.
fn load_lag(world: &World) -> f64 {
    if before_per_cell_leads() {
        world
            .get_resource::<LoadCounts>()
            .map_or(0.0, |c| c.wanted_lag)
    } else {
        world
            .get_resource::<StreamLatency>()
            .map_or(0.0, |l| l.seconds)
    }
}

/// The cells' loaders, in the order of [`StreamedCells::cells`].
#[derive(Resource, Default)]
struct CellHooks(Vec<Slot>);

/// Cells whose meshes are hidden or fading in: the cell, the meshes this plugin faded, the
/// frames left hidden, and when the fade started.
#[derive(Resource, Default)]
struct Fades(Vec<Fade>);

struct Fade {
    cell: CellId,
    meshes: Vec<Entity>,
    hidden_frames: u32,
    started: Option<Instant>,
    loaded: bool,
}

/// Streams the registered cells (`Last`).
pub struct StreamingPlugin;

impl Plugin for StreamingPlugin {
    fn build(&self, app: &mut App) {
        hlod::register(app);
        crate::scene_stream::register(app);
        crate::stream_grid::register_plugin(app);
        cell_assets::install_sizers(&mut app.world);
        app.init_resource::<StreamedCells>()
            .init_resource::<CellHooks>()
            .init_resource::<StreamingBudget>()
            .init_resource::<engine_world::CellMemory>()
            .init_resource::<Fades>()
            .init_resource::<Leaves>()
            .init_resource::<SourceMotion>()
            .init_resource::<StreamLatency>()
            .init_resource::<LoadCounts>()
            .shift_resource::<SourceMotion>()
            .add_systems(Last, stream_system)
            .register_api(
                "stream.cells",
                "List the streamed cells (key, center, radius, load distance, state; from the \
                 last load: seconds spawning, seconds building on a worker, entities spawned and \
                 the RAM it added in MB, the assets it made and their MB, whether it frees them \
                 on unload), how many are loaded, the process RAM and GPU memory in MB, and \
                 every asset type's count and heap MB.",
                "{}",
                |world, _| {
                    let cells = world.resource::<StreamedCells>();
                    let diag = world.resource::<Diagnostics>();
                    let assets: serde_json::Map<String, serde_json::Value> = world
                        .get_resource::<engine_assets::AssetTypes>()
                        .map(|t| t.report(world))
                        .unwrap_or_default()
                        .into_iter()
                        .map(|(name, s)| {
                            let mb = s.bytes as f64 / (1 << 20) as f64;
                            (name.to_string(), serde_json::json!({"count": s.count, "mb": mb}))
                        })
                        .collect();
                    Ok(serde_json::json!({
                        "cells": cells.cells,
                        "loaded": cells.cells.iter().filter(|c| c.state == CellState::Loaded).count(),
                        "ram_mb": diag.get(PROCESS_RAM_MB),
                        "vram_mb": diag.get("render.gpu_memory_mb"),
                        "assets": assets,
                    }))
                },
            )
            .register_api(
                "stream.memory",
                "What remembering cells keep while out (destroyed ones, moved and changed \
                 components, per cell key), for save games: no args returns it; {\"set\": \
                 memory} replaces it (load a save; cells in now pick it up on their next \
                 load); {\"forget\": key} resets one cell; {\"remember\": name} adds a \
                 registered component to what is kept.",
                r#"{"forget": "scene:valley/main/L0/0,0"}"#,
                |world, args| {
                    let mut memory = world.resource_mut::<engine_world::CellMemory>();
                    if let Some(set) = args.get("set") {
                        *memory = serde_json::from_value(set.clone())
                            .map_err(|e| format!("bad memory: {e}"))?;
                    }
                    if let Some(key) = args["forget"].as_str() {
                        memory.cells.remove(key);
                    }
                    if let Some(name) = args["remember"].as_str() {
                        memory.remember(name);
                    }
                    serde_json::to_value(&*memory).map_err(|e| e.to_string())
                },
            )
            .register_api(
                "stream.pin",
                "Hold a streamed cell loaded (true) or unloaded (false) whatever the sources \
                 say, or let the sources decide again (null). Returns the pins.",
                r#"{"key": "forest", "loaded": true}"#,
                |world, args| {
                    let key = args["key"].as_str().ok_or("key: a cell's key")?.to_string();
                    let mut cells = world.resource_mut::<StreamedCells>();
                    if cells.find(&key).is_none() {
                        return Err(format!("no cell {key:?}"));
                    }
                    match args["loaded"].as_bool() {
                        Some(v) => cells.pins.insert(key, v),
                        None => cells.pins.remove(&key),
                    };
                    serde_json::to_value(&cells.pins).map_err(|e| e.to_string())
                },
            )
            .register_api(
                "stream.budget",
                "Get or change the streaming budget: loads_per_frame, frame_ms, builds (worker \
                 builds at once), fade_s and fade_frames (the fade-in), ram_mb and vram_mb \
                 (caps in MB; over one, cells no source needs unload early; null = no cap). \
                 Send fields to change; returns the budget.",
                r#"{"ram_mb": 12000, "loads_per_frame": 1}"#,
                |world, args| {
                    let mut budget = world.resource_mut::<StreamingBudget>();
                    if args.as_object().is_some_and(|a| !a.is_empty()) {
                        let mut current =
                            serde_json::to_value(*budget).map_err(|e| e.to_string())?;
                        if let (Some(into), Some(from)) = (current.as_object_mut(), args.as_object()) {
                            for (k, v) in from {
                                into.insert(k.clone(), v.clone());
                            }
                        }
                        *budget = serde_json::from_value(current).map_err(|e| e.to_string())?;
                    }
                    serde_json::to_value(*budget).map_err(|e| e.to_string())
                },
            );
    }
}

/// A streaming source this frame: where it is, where its lookahead reaches along its path, and
/// how it scales load distances.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct Source {
    pub at: DVec3,
    /// Where it will be at the end of its lookahead (stretched to the time cells have been
    /// taking to load) at its speed (`at` when still): for the far views and the grids.
    pub ahead: DVec3,
    /// Its velocity, m/s (zero when it stands still or has no lookahead).
    pub v: DVec3,
    /// Its speed, m/s.
    pub speed: f64,
    pub scale: f64,
    /// Its own lookahead, seconds (0: it wants cells only where it is).
    pub lookahead: f64,
}

impl Source {
    fn still(at: DVec3, scale: f64) -> Self {
        Self {
            at,
            ahead: at,
            v: DVec3::ZERO,
            speed: 0.0,
            scale,
            lookahead: 0.0,
        }
    }

    /// Whether it has a path: it moves and looks ahead.
    fn flies(&self) -> bool {
        self.lookahead > 0.0 && self.speed > 0.0
    }

    /// How many seconds ahead it asks for a cell that takes `load_secs` to load: its
    /// lookahead, or twice the load if that is longer (0 when it stands still). The stretch
    /// past its own lookahead ends `reach_m` ahead of it, where there is such a limit.
    pub(crate) fn lead(&self, load_secs: f64, reach_m: Option<f64>) -> f64 {
        if !self.flies() {
            return 0.0;
        }
        let stretch = (2.0 * load_secs).min(reach_m.map_or(f64::INFINITY, |m| m / self.speed));
        self.lookahead.max(stretch).min(MAX_LOOKAHEAD_S)
    }

    /// Distance from its path over the next `secs` to `c`'s edge, in units of `c`'s load
    /// distance: at most 1 means the cell is wanted, now or before the source gets there.
    pub(crate) fn reach(&self, c: &CellInfo, secs: f64) -> f64 {
        let ahead = self.at + self.v * secs;
        let edge =
            (closest_on_path(self.at, ahead, c.center).distance(c.center) - c.radius).max(0.0);
        edge / (c.load_m * self.scale).max(1e-6)
    }

    /// When it will need `c` (seconds; 0: it needs it now), or `None` if its path over the
    /// next `secs` doesn't come within `c`'s load distance.
    fn need_in(&self, c: &CellInfo, secs: f64) -> Option<f64> {
        let load = (c.load_m * self.scale).max(1e-6);
        if c.distance(self.at) <= load {
            return Some(0.0);
        }
        if self.speed <= 0.0 || secs <= 0.0 || self.reach(c, secs) > 1.0 {
            return None;
        }
        // Where the path first comes within the load distance of the cell's edge.
        let r = c.radius + load;
        let d = self.v * secs;
        let len = d.length();
        let dir = d / len;
        let to = c.center - self.at;
        let along = to.dot(dir);
        let off2 = (to.length_squared() - along * along).max(0.0);
        let enter = (along - (r * r - off2).max(0.0).sqrt()).clamp(0.0, len);
        Some(enter / self.speed)
    }
}

/// The point of segment `a`..`b` nearest `p`.
fn closest_on_path(a: DVec3, b: DVec3, p: DVec3) -> DVec3 {
    let d = b - a;
    let len2 = d.length_squared();
    if len2 <= 1e-12 {
        return a;
    }
    a + d * ((p - a).dot(d) / len2).clamp(0.0, 1.0)
}

/// How fast each source has been moving, measured from its position frame to frame, so a
/// camera or focus with no [`Velocity`] still streams ahead of itself (Flight Simulator and
/// Unreal's World Partition both load along the predicted path of the view).
#[derive(Resource, Debug, Default, Clone)]
pub struct SourceMotion {
    /// Per source entity.
    sources: std::collections::HashMap<Entity, Track>,
    /// The same for the focus points, by index, when no source exists.
    foci: Vec<Track>,
}

/// One source's motion.
#[derive(Debug, Default, Clone, Copy, PartialEq)]
struct Track {
    at: DVec3,
    /// Smoothed velocity, m/s.
    v: DVec3,
    /// Last frame's raw velocity, m/s.
    raw: DVec3,
    /// This frame's move was a jump (a teleport, a tour stop, fast travel), not flight: see
    /// [`JUMP_M`].
    jumped: bool,
}

impl SourceMotion {
    /// Whether a source or focus jumped this frame.
    fn jumped(&self) -> bool {
        self.sources.values().chain(&self.foci).any(|t| t.jumped)
    }
}

impl FollowsWorldShift for SourceMotion {
    fn follow(&mut self, s: &WorldShift) {
        for t in self.sources.values_mut().chain(self.foci.iter_mut()) {
            t.at = s.point(t.at);
            t.v = s.direction(t.v.as_vec3()).as_dvec3();
            t.raw = s.direction(t.raw.as_vec3()).as_dvec3();
        }
    }
}

/// Two frames' velocities further apart than this are a jump (a teleport, a tour stop), not
/// flight, m/s² (500 g: no flight turns that hard; a jump is one frame's spike).
const JUMP_ACCEL: f64 = 5_000.0;
/// A source that moved farther than this in one frame, at over four times the speed it had,
/// left where it was for good (a teleport, a tour stop, fast travel): the cells it left do
/// not linger. (Not the acceleration test above: a free camera that starts to sprint, or a
/// route's next leg, is one frame's spike too, and is still on its way through its cells.)
const JUMP_M: f64 = 250.0;

impl Track {
    /// Feeds this frame's position. A frame's raw velocity counts only when the frame before
    /// agrees with it, so one frame's spike (a jump) never steers the lookahead, while a
    /// steady flight shows within a few frames.
    fn update(slot: Option<Track>, at: DVec3, dt: f64) -> Track {
        let Some(t) = slot else {
            return Track {
                at,
                ..Default::default()
            };
        };
        if dt <= 0.0 {
            return Track {
                at,
                jumped: false,
                ..t
            };
        }
        let raw = (at - t.at) / dt;
        let steady = (raw - t.raw).length() / dt <= JUMP_ACCEL;
        let v = if steady {
            // About a quarter second of smoothing: frame-time noise out, turns in.
            t.v + (raw - t.v) * (dt / 0.25).min(1.0)
        } else {
            t.v
        };
        let had = t.raw.length().max(t.v.length());
        let jumped = at.distance(t.at) > JUMP_M && raw.length() > 4.0 * had;
        Track { at, v, raw, jumped }
    }
}

/// How long cells take to load, from when a load's work starts (its build goes to a worker or
/// its first slice runs) to all in (seconds, smoothed over the last loads): a source asks for
/// a cell it has no time of its own for this long ahead, twice over, so a source moving at any
/// speed finds its cells in, as Planets' terrain look-ahead does from its request-to-upload
/// time. Time a cell spent waiting its turn after being asked for early doesn't count: it
/// would stretch the lookahead, which asks for cells earlier still.
#[derive(Resource, Debug, Default, Clone, Copy, Serialize)]
pub struct StreamLatency {
    /// Smoothed seconds from started to loaded.
    pub seconds: f64,
    /// The last load's.
    pub last: f64,
    /// Loads measured.
    pub loads: u64,
}

impl StreamLatency {
    fn add(&mut self, secs: f64) {
        self.last = secs;
        self.loads += 1;
        self.seconds = if self.loads == 1 {
            secs
        } else {
            self.seconds + (secs - self.seconds) * 0.2
        };
    }
}

/// Measures the sources' motion (once a frame, first thing in [`stream_system`]).
fn measure_sources(world: &mut World) {
    let dt = world.resource::<Time>().delta_secs() as f64;
    let at: Vec<(Entity, DVec3)> = world
        .query_filtered::<(Entity, &GlobalTransform), With<StreamingSource>>()
        .iter(world)
        .map(|(e, g)| (e, g.translation()))
        .collect();
    let foci: Vec<DVec3> = if at.is_empty() {
        let mut q = world.query_filtered::<&GlobalTransform, With<SimulationFocus>>();
        let mut f: Vec<DVec3> = q.iter(world).map(|g| g.translation()).collect();
        if f.is_empty() {
            f = world.resource::<FocusPoints>().points.clone();
        }
        f
    } else {
        Vec::new()
    };
    let mut motion = world.resource_mut::<SourceMotion>();
    motion.sources.retain(|e, _| at.iter().any(|(a, _)| a == e));
    for (e, p) in at {
        let t = Track::update(motion.sources.get(&e).copied(), p, dt);
        motion.sources.insert(e, t);
    }
    let old = std::mem::take(&mut motion.foci);
    motion.foci = foci
        .into_iter()
        .enumerate()
        .map(|(k, p)| Track::update(old.get(k).copied(), p, dt))
        .collect();
}

/// The sources this frame. None when there is no source and no focus (tools and tests:
/// everything loads). Each looks ahead along its velocity (its [`Velocity`], else its measured
/// motion) by its `lookahead_s`, or by twice the time cells have been taking to load if that
/// is longer ([`Source::lead`]); focus points look ahead 2 s the same way.
pub(crate) fn sources(world: &mut World) -> Option<Vec<Source>> {
    // Nothing streams around a ship in cruise: at its speed every cell would be passed before
    // it was in. Sources riding one (its pilot, its chase camera) count as cruising.
    let cruising: Vec<DVec3> = world
        .query_filtered::<&GlobalTransform, With<engine_world::physics::Cruising>>()
        .iter(world)
        .map(|g| g.translation())
        .collect();
    let calm = |p: DVec3| cruising.iter().all(|c| c.distance(p) > CRUISE_REACH_M);
    let lag = load_lag(world);
    let motion = world
        .get_resource::<SourceMotion>()
        .cloned()
        .unwrap_or_default();
    let ahead = |at: DVec3, v: DVec3, lookahead: f64, scale: f64| {
        if lookahead <= 0.0 {
            return Source::still(at, scale);
        }
        let mut s = Source {
            at,
            ahead: at,
            v,
            speed: v.length(),
            scale,
            lookahead,
        };
        s.ahead = at + v * s.lead(lag, None);
        s
    };
    let mut out = Vec::new();
    let mut any = false;
    let mut q = world.query::<(
        Entity,
        &GlobalTransform,
        &StreamingSource,
        Option<&Velocity>,
    )>();
    for (e, g, s, v) in q.iter(world) {
        any = true;
        let p = g.translation();
        if !calm(p) {
            continue;
        }
        let v = match v {
            Some(v) => v.linear.as_dvec3(),
            None => motion.sources.get(&e).map_or(DVec3::ZERO, |m| m.v),
        };
        out.push(ahead(p, v, s.lookahead_s, s.range_scale));
    }
    if !any {
        let mut foci = world.query_filtered::<&GlobalTransform, With<SimulationFocus>>();
        let mut at: Vec<DVec3> = foci.iter(world).map(|g| g.translation()).collect();
        if at.is_empty() {
            at = world.resource::<FocusPoints>().points.clone();
        }
        any = !at.is_empty();
        for (k, p) in at.into_iter().enumerate() {
            if calm(p) {
                let v = motion.foci.get(k).map_or(DVec3::ZERO, |m| m.v);
                out.push(ahead(p, v, FOCUS_LOOKAHEAD_S, 1.0));
            }
        }
    }
    // Sources all cruising: none wants anything (not "no sources", which loads everything).
    any.then_some(out)
}

/// Where the sources are and where their lookahead reaches: (point, range scale), for the far
/// views and the procedural grids.
pub(crate) fn source_points(world: &mut World) -> Option<Vec<(DVec3, f64)>> {
    sources(world).map(|s| {
        s.iter()
            .flat_map(|s| {
                let ahead = (s.ahead != s.at).then_some((s.ahead, s.scale));
                std::iter::once((s.at, s.scale)).chain(ahead)
            })
            .collect()
    })
}

/// Lookahead of a focus point (a source without [`StreamingSource`]), seconds.
const FOCUS_LOOKAHEAD_S: f64 = 2.0;
/// The lookahead never reaches farther ahead than this, seconds.
const MAX_LOOKAHEAD_S: f64 = 20.0;

/// Sources within this distance of a [`Cruising`](engine_world::physics::Cruising) ship are
/// riding it, meters.
const CRUISE_REACH_M: f64 = 1000.0;

/// `Last`: queue, build, load, fade and unload cells.
pub fn stream_system(world: &mut World) {
    let span = tracing::info_span!("stream: sources and grid").entered();
    measure_sources(world);
    crate::stream_grid::update(world);
    if world.resource::<StreamedCells>().cells.is_empty() {
        return;
    }
    drop(span);
    let span = tracing::info_span!("stream: decide").entered();
    let poses = follow_frames(world);
    let sources = sources(world);
    let budget = *world.resource::<StreamingBudget>();
    // Each slot's: has a build step, failed, what its last load took, how long it has been
    // unwanted, the load time it was asked for by.
    let slots: Vec<(bool, bool, Option<f64>, f32, f64)> = world
        .resource::<CellHooks>()
        .0
        .iter()
        .map(|s| {
            let build = s.loader.build.is_some();
            (build, s.failed, s.took, s.unwanted_s, s.asked_secs)
        })
        .collect();
    let lag = load_lag(world);
    let old = before_per_cell_leads();
    let linger_s = if old { 0.0 } else { budget.linger_s };
    let dt = world.resource::<Time>().delta_secs();
    let over_cap = budget.over(world.resource::<Diagnostics>());
    // Nothing lingers over a memory cap, or once a source jumped (a teleport, a tour stop,
    // fast travel: where it left is not on its way back).
    let cut_linger = over_cap
        || world
            .get_resource::<SourceMotion>()
            .is_some_and(SourceMotion::jumped);
    let mut cells = world.resource_mut::<StreamedCells>();
    let hysteresis = cells.hysteresis;
    // How far ahead a source asks for a cell: by what the cell took to load last time.
    let lead = |s: &Source, c: &CellInfo, took: Option<f64>| {
        if old {
            return s.lead(lag, None);
        }
        let reach_m = budget.stretch_loads * c.load_m * s.scale;
        s.lead(took.unwrap_or(lag), Some(reach_m))
    };
    // Each cell's distance from the nearest source's path, in units of its load distance.
    let reach = |c: &CellInfo, took: Option<f64>| match &sources {
        None => 0.0,
        Some(sources) => sources
            .iter()
            .map(|s| s.reach(c, lead(s, c, took)))
            .fold(f64::INFINITY, f64::min),
    };
    // Load order: cells a source stands in first (nearest first), then the rest by when a
    // source will get there, so the cell ahead on a fast path beats a nearer one behind.
    let priority = |c: &CellInfo, r: f64, took: Option<f64>| match &sources {
        None => r,
        Some(sources) => match sources
            .iter()
            .filter_map(|s| s.need_in(c, lead(s, c, took)))
            .fold(None, |a: Option<f64>, t| Some(a.map_or(t, |a| a.min(t))))
        {
            Some(t) if t <= 0.0 => r.min(1.0),
            Some(t) => 2.0 + t,
            None => 2.0 + MAX_LOOKAHEAD_S + r,
        },
    };
    let now_in = |c: &CellInfo| {
        sources
            .as_ref()
            .is_some_and(|s| s.iter().any(|s| c.distance(s.at) <= c.load_m * s.scale))
    };
    // A source in flight may turn back: the cells its path leaves linger. Sources that stand
    // leave cells behind for good.
    let flying = sources
        .as_ref()
        .is_some_and(|s| s.iter().any(Source::flies));
    let mut late = 0usize;
    let mut woke = Vec::new();
    let mut want = Vec::new();
    let mut dropped = Vec::new();
    let mut leaving = Vec::new();
    let mut spare: Option<(f64, usize)> = None;
    // Per cell: a source is within its load distance now, and its seconds unwanted.
    let mut notes = Vec::with_capacity(cells.cells.len());
    let pins = std::mem::take(&mut cells.pins);
    for (i, c) in cells.cells.iter_mut().enumerate() {
        let (build, failed, took, unwanted, asked) = slots.get(i).copied().unwrap_or_default();
        if c.retired {
            notes.push((false, 0.0));
            continue;
        }
        let r = match pins.get(&c.key) {
            Some(true) => 0.0,
            Some(false) => f64::INFINITY,
            // A cell already asked for is held by the lead it was asked with, not the one
            // it would be asked with now: once it has loaded quickly, or other cells have,
            // its lead shrinks, and it would be dropped and asked for again a moment later.
            None if c.state != CellState::Unloaded && !old => {
                reach(c, Some(took.unwrap_or(lag).max(asked)))
            }
            None => reach(c, took),
        };
        let kept = c.keep_loaded && pins.get(&c.key) != Some(&false);
        let loaded = matches!(c.state, CellState::Loading | CellState::Loaded);
        let unwanted = match loaded && r > 1.0 + hysteresis {
            true if cut_linger => linger_s.max(unwanted + dt),
            true => unwanted + dt,
            false => 0.0,
        };
        let lingers = flying && !pins.contains_key(&c.key) && unwanted < linger_s;
        // With no source at all, or pinned in, it is there because it was asked for.
        let needed = sources.is_none() || pins.get(&c.key) == Some(&true) || now_in(c);
        notes.push((needed, unwanted));
        match c.state {
            CellState::Unloaded if r <= 1.0 && !failed => {
                c.state = if build {
                    CellState::Building
                } else {
                    CellState::Queued
                };
                woke.push(i);
            }
            CellState::Building | CellState::Queued if r > 1.0 + hysteresis => {
                c.state = CellState::Unloaded;
                dropped.push(i);
            }
            CellState::Loading | CellState::Loaded if r > 1.0 + hysteresis && !kept && !lingers => {
                leaving.push(i)
            }
            CellState::Loaded if r > 1.0 && !kept && spare.is_none_or(|s| r > s.0) => {
                spare = Some((r, i));
            }
            _ => {}
        }
        if c.state.is_waking() {
            if now_in(c) {
                late += 1;
            }
            want.push((priority(c, r, took), i, c.state));
        }
    }
    cells.pins = pins;
    {
        let now = Instant::now();
        let mut hooks = world.resource_mut::<CellHooks>();
        for (slot, (needed, unwanted)) in hooks.0.iter_mut().zip(notes) {
            slot.needed |= needed;
            slot.unwanted_s = unwanted;
        }
        for i in woke {
            if let Some(slot) = hooks.0.get_mut(i) {
                slot.wanted_at.get_or_insert(now);
                slot.asked_secs = slot.took.unwrap_or(lag);
            }
        }
    }
    if let Some(mut d) = world.get_resource_mut::<Diagnostics>() {
        d.set("stream.late", late as f64);
    }
    // With no source (tools, tours) everything is wanted now: catch up the same way.
    let frame_ms = if late > 0 || sources.is_none() {
        budget.late_frame_ms.max(budget.frame_ms)
    } else {
        budget.frame_ms
    };
    let pressed = leaving.is_empty() && over_cap;
    if let Some((_, i)) = spare.filter(|_| pressed) {
        leaving.push(i);
    }
    for i in dropped {
        world.resource_mut::<CellHooks>().0[i].reset();
        off_frame::drop_scope(world, art_scope(CellId(i as u32)));
    }
    let fading = sources.is_some() && budget.fade_s > 0.0;
    for &i in &leaving {
        let loaded = world.resource::<StreamedCells>().cells[i].state == CellState::Loaded;
        if fading && loaded {
            begin_leave(world, i);
        } else {
            unload(world, i);
        }
    }
    drop(span);
    let span = tracing::info_span!("stream: load and leave").entered();
    tracing::info_span!("stream: leave").in_scope(|| leave(world, &budget));
    want.retain(|w| !leaving.contains(&w.1));
    want.sort_by(|a, b| a.0.total_cmp(&b.0));
    tracing::info_span!("stream: builds").in_scope(|| builds(world, &want, budget.builds));
    // Cells already loading go first, nearest first, then new ones in their turn.
    let start = Instant::now();
    let over = |start: Instant| start.elapsed().as_secs_f32() * 1000.0 > frame_ms;
    let mut stepped = false;
    let mut started = 0;
    let order = want
        .iter()
        .filter(|w| w.2 == CellState::Loading)
        .chain(want.iter().filter(|w| w.2 == CellState::Queued));
    'cells: for &(_, i, state) in order {
        let id = CellId(i as u32);
        if state == CellState::Queued {
            // A build may have finished this frame; a cell still building waits.
            if world.resource::<StreamedCells>().cells[i].state != CellState::Queued
                || started >= budget.loads_per_frame.max(1)
            {
                continue;
            }
            started += 1;
            let fades = sources.is_some() && budget.fade_s > 0.0;
            tracing::info_span!("stream: begin").in_scope(|| begin(world, i, fades));
        }
        loop {
            if stepped && over(start) {
                break 'cells;
            }
            stepped = true;
            match tracing::info_span!("stream: step").in_scope(|| step(world, id)) {
                Step::More => {}
                Step::Done => {
                    tracing::info_span!("stream: finish").in_scope(|| finish(world, i));
                    break;
                }
                // Its art is still building: the next cell gets the time.
                Step::Waiting => break,
            }
        }
    }
    tracing::info_span!("stream: fade").in_scope(|| fade(world, &budget));
    drop(span);
    let _span = tracing::info_span!("stream: hlod").entered();
    let configs: Vec<ProxyConfig> = world
        .resource::<CellHooks>()
        .0
        .iter()
        .map(|s| s.loader.proxy)
        .collect();
    let fading: Vec<CellId> = world.resource::<Fades>().0.iter().map(|f| f.cell).collect();
    let points: Option<Vec<(DVec3, f64)>> = sources.map(|s| {
        s.iter()
            .flat_map(|s| {
                let ahead = (s.ahead != s.at).then_some((s.ahead, s.scale));
                std::iter::once((s.at, s.scale)).chain(ahead)
            })
            .collect()
    });
    hlod::update(world, &configs, &fading, &poses, points.as_deref());
}

/// Where each cell's frame stands now (`None`: no frame, or it isn't there); moves framed
/// cells' centers with their frames.
fn follow_frames(world: &mut World) -> Vec<Option<(DVec3, DQuat)>> {
    let framed: Vec<(usize, String)> = world
        .resource::<StreamedCells>()
        .cells
        .iter()
        .enumerate()
        .filter_map(|(i, c)| Some((i, c.frame.clone()?)))
        .collect();
    let n = world.resource::<StreamedCells>().cells.len();
    let mut poses = vec![None; n];
    if framed.is_empty() {
        return poses;
    }
    let frames: Vec<(String, DVec3, DQuat)> = world
        .query::<(&StreamFrame, &GlobalTransform)>()
        .iter(world)
        .map(|(f, g)| {
            let (_, rotation, translation) = g.0.to_scale_rotation_translation();
            (f.0.clone(), translation, rotation)
        })
        .collect();
    for (i, name) in framed {
        let Some((_, at, turn)) = frames.iter().find(|f| f.0 == name) else {
            continue;
        };
        poses[i] = Some((*at, *turn));
        let center = world.resource::<StreamedCells>().cells[i].center;
        let mut hooks = world.resource_mut::<CellHooks>();
        let Some(slot) = hooks.0.get_mut(i) else {
            continue;
        };
        let local = *slot
            .local
            .get_or_insert_with(|| turn.inverse() * (center - *at));
        world.resource_mut::<StreamedCells>().cells[i].center = *at + *turn * local;
    }
    poses
}

/// Starts the wanted cells' builds on workers, nearest first, and collects finished ones.
fn builds(world: &mut World, want: &[(f64, usize, CellState)], at_once: usize) {
    let building: Vec<(usize, String)> = {
        let cells = world.resource::<StreamedCells>();
        want.iter()
            .filter(|w| w.2 == CellState::Building)
            .map(|w| (w.1, cells.cells[w.1].key.clone()))
            .collect()
    };
    if building.is_empty() {
        return;
    }
    let mut hooks = world.resource_mut::<CellHooks>();
    let mut done = Vec::new();
    let mut failed = Vec::new();
    let mut running = 0;
    for (i, _) in &building {
        let slot = &mut hooks.0[*i];
        let Some(rx) = slot.building.as_mut() else {
            continue;
        };
        match rx
            .get_mut()
            .map_or(Err(TryRecvError::Disconnected), |rx| rx.try_recv())
        {
            Ok((built, secs)) => {
                slot.built = Some(Mutex::new(built));
                slot.building = None;
                done.push((*i, secs));
            }
            Err(TryRecvError::Empty) => running += 1,
            Err(TryRecvError::Disconnected) => {
                slot.building = None;
                slot.failed = true;
                failed.push(*i);
            }
        }
    }
    for (i, key) in &building {
        let slot = &mut hooks.0[*i];
        if slot.building.is_some() || slot.built.is_some() || slot.failed {
            continue;
        }
        if running >= at_once.max(1) {
            break;
        }
        let Some(build) = slot.loader.build.clone() else {
            continue;
        };
        let (tx, rx) = mpsc::channel();
        // A thread of its own: a build may open pool scopes without waiting on the pool it
        // runs in. Named "async ..." so its `par_chunks_mut` splits run on the async pool, as
        // an off-frame build's do: on the compute pool a frame thread waiting in any scope
        // (a system's `par_iter`) picks up a build's chunk and the frame waits for it: on the
        // session VM's route engine_anim::gait_system took 918 ms in one frame while the
        // plaza built, and showcase::update_lods 1.2 s in another.
        let spawned = std::thread::Builder::new()
            .name(format!("async cell build {key}"))
            .spawn(move || {
                let t = Instant::now();
                let built = build();
                let _ = tx.send((built, t.elapsed().as_secs_f32()));
            });
        match spawned {
            Ok(_) => {
                slot.building = Some(Mutex::new(rx));
                slot.started_at.get_or_insert_with(Instant::now);
                running += 1;
            }
            Err(e) => {
                engine_core::log::error!("streaming: can't start {key}'s build: {e}");
                slot.failed = true;
                failed.push(*i);
            }
        }
    }
    let mut cells = world.resource_mut::<StreamedCells>();
    for (i, secs) in done {
        cells.cells[i].state = CellState::Queued;
        cells.cells[i].build_secs = secs;
    }
    for i in failed {
        cells.cells[i].state = CellState::Unloaded;
        engine_core::log::error!(
            "streaming: {}'s build step failed; it stays out",
            cells.cells[i].key
        );
    }
}

/// The cell goes from queued to loading.
fn begin(world: &mut World, i: usize, fade: bool) {
    // Asset stores a game installs after the plugin count their bytes too.
    cell_assets::install_sizers(world);
    let slot = &mut world.resource_mut::<CellHooks>().0[i];
    slot.step = 0;
    slot.spawned = false;
    slot.secs = 0.0;
    slot.started_at.get_or_insert_with(Instant::now);
    // The sampled reading: reading it here took the address space's lock behind the workers
    // building art, 240 ms a cell start on the session VM's route (and as long again in
    // `finish`). A cell's RAM is the difference of two samples up to 100 ms old.
    slot.ram = sampled_ram_mb();
    world.resource_mut::<StreamedCells>().cells[i].state = CellState::Loading;
    if fade {
        world.resource_mut::<Fades>().0.push(Fade {
            cell: CellId(i as u32),
            meshes: Vec::new(),
            hidden_frames: 0,
            started: None,
            loaded: false,
        });
    }
}

/// Where a cell's load is after one [`step`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Step {
    /// More to run now.
    More,
    /// All in.
    Done,
    /// Only off-frame art is left, and none of it has finished building.
    Waiting,
}

/// The [`off_frame`] scope of a cell's art: what its spawn queues there is applied by the
/// cell's own steps, so it is counted in the cell and leaves with it.
fn art_scope(id: CellId) -> off_frame::Scope {
    (1 << 32) | id.0 as u64
}

/// A single step slower than this is logged with its cell, so a profile's tall frame can be
/// traced to the slice or art that made it.
const SLOW_STEP_MS: f64 = 50.0;

/// Runs one slice of the cell's spawn, or applies one piece of its off-frame art once the
/// spawn has run, and tags what it spawned.
fn step(world: &mut World, id: CellId) -> Step {
    let i = id.0 as usize;
    let scope = art_scope(id);
    let spawned = world.resource::<CellHooks>().0[i].spawned;
    if spawned && !off_frame::ready(world, Some(scope)) {
        return if off_frame::pending(world, Some(scope)) > 0 {
            Step::Waiting
        } else {
            Step::Done
        };
    }
    let t = Instant::now();
    let before = Snapshot::of(world);
    let marks = Marks::of(world);
    let outer = off_frame::set_scope(world, Some(scope));
    if spawned {
        // One piece a step, so the frame budget slices the art as it slices the spawn.
        off_frame::apply_finished(world, Some(scope), Some(Duration::ZERO));
    } else {
        let mut hooks = std::mem::take(&mut world.resource_mut::<CellHooks>().0);
        let slot = &mut hooks[i];
        let built = slot.built.as_mut().and_then(|b| b.get_mut().ok());
        slot.spawned = (slot.loader.step)(world, built, slot.step);
        slot.step += 1;
        restore(world, hooks);
    }
    // Commands the slice queued run now, so what they queue off-frame is the cell's too.
    world.flush();
    off_frame::set_scope(world, outer);
    let pending = off_frame::pending(world, Some(scope));
    let spawned = world.resource::<CellHooks>().0[i].spawned;
    let mut hooks = std::mem::take(&mut world.resource_mut::<CellHooks>().0);
    hooks[i].assets.note(world, marks, Marks::of(world));
    restore(world, hooks);
    // Resources are entities too (bevy 0.20); one a spawn inserts stays when the cell goes.
    // The tops of what it spawned: new entities with no parent, or a parent from before (a
    // planet's frame, a station the content hangs on).
    let new: Vec<Entity> = world
        .query_filtered::<(Entity, Option<&ChildOf>), Without<IsResource>>()
        .iter(world)
        .filter(|(e, parent)| !before.has(*e) && parent.is_none_or(|p| before.has(p.parent())))
        .map(|(e, _)| e)
        .collect();
    for &e in &new {
        world.entity_mut(e).insert(CellMember(id));
    }
    if world.resource::<CellHooks>().0[i].loader.remember {
        let (key, center) = {
            let c = &world.resource::<StreamedCells>().cells[i];
            (c.key.clone(), c.center)
        };
        let mut seen = std::mem::take(&mut world.resource_mut::<CellHooks>().0[i].ids_seen);
        cell_memory::assign_ids(world, &key, center, &new, &mut seen);
        world.resource_mut::<CellHooks>().0[i].ids_seen = seen;
    }
    world.resource_mut::<CellHooks>().0[i].secs += t.elapsed().as_secs_f32();
    let ms = t.elapsed().as_secs_f64() * 1000.0;
    if ms > SLOW_STEP_MS {
        let key = &world.resource::<StreamedCells>().cells[i].key;
        let what = if spawned { "art or slice" } else { "slice" };
        tracing::info!("streaming: a {what} of {key} took {ms:.1} ms");
    }
    if !spawned || pending > 0 {
        Step::More
    } else {
        Step::Done
    }
}

/// Every entity alive, by index: cheaper to build and ask than a hash set over a big world.
struct Snapshot(Vec<Option<Entity>>);

impl Snapshot {
    fn of(world: &mut World) -> Self {
        let mut alive = Vec::new();
        for e in world.query::<Entity>().iter(world) {
            let i = e.index_u32() as usize;
            if alive.len() <= i {
                alive.resize(i + 1, None);
            }
            alive[i] = Some(e);
        }
        Self(alive)
    }

    fn has(&self, e: Entity) -> bool {
        self.0.get(e.index_u32() as usize) == Some(&Some(e))
    }
}

/// The cell is all in: record what it cost.
fn finish(world: &mut World, i: usize) {
    let id = CellId(i as u32);
    let (secs, ram0, steps) = {
        let slot = &mut world.resource_mut::<CellHooks>().0[i];
        let out = (slot.secs, slot.ram, slot.step);
        slot.built = None;
        out
    };
    let roots: Vec<Entity> = world
        .query::<(Entity, &CellMember)>()
        .iter(world)
        .filter(|(_, m)| m.0 == id)
        .map(|(e, _)| e)
        .collect();
    if world.resource::<CellHooks>().0[i].loader.remember {
        let (key, center) = {
            let c = &world.resource::<StreamedCells>().cells[i];
            (c.key.clone(), c.center)
        };
        let baseline = cell_memory::loaded(world, &key, center, &roots);
        world.resource_mut::<CellHooks>().0[i].baseline = Some(baseline);
    }
    let roots: Vec<Entity> = roots
        .into_iter()
        .filter(|e| world.get_entity(*e).is_ok())
        .collect();
    let entities = tree(world, &roots).len() as u32;
    let ram = sampled_ram_mb().zip(ram0).map(|(b, a)| (b - a) as f32);
    let (assets, asset_bytes) = world.resource::<CellHooks>().0[i].assets.footprint(world);
    let assets_mb = asset_bytes as f32 / (1 << 20) as f32;
    if let Some(fade) = world
        .resource_mut::<Fades>()
        .0
        .iter_mut()
        .find(|f| f.cell == id)
    {
        fade.loaded = true;
    }
    let slot = &mut world.resource_mut::<CellHooks>().0[i];
    slot.loaded_at = Some(Instant::now());
    let took = slot.started_at.take().map(|t| t.elapsed().as_secs_f64());
    let waited = slot.wanted_at.take().map(|t| t.elapsed().as_secs_f64());
    if let Some(took) = took {
        slot.took = Some(took);
        world.resource_mut::<StreamLatency>().add(took);
    }
    let loads = {
        let mut counts = world.resource_mut::<LoadCounts>();
        counts.loads += 1;
        if let Some(waited) = waited {
            counts.wanted_lag = if counts.wanted_lag == 0.0 {
                waited
            } else {
                counts.wanted_lag + (waited - counts.wanted_lag) * 0.2
            };
        }
        counts.loads
    };
    if let Some(mut d) = world.get_resource_mut::<Diagnostics>() {
        if let Some(took) = took {
            d.set("stream.latency_s", took);
        }
        d.set("stream.loads", loads as f64);
        if d.get("stream.unneeded_loads").is_none() {
            d.set("stream.unneeded_loads", 0.0);
        }
    }
    let mut cells = world.resource_mut::<StreamedCells>();
    let c = &mut cells.cells[i];
    c.state = CellState::Loaded;
    c.load_secs = secs;
    c.entities = entities;
    c.ram_mb = ram;
    c.assets = assets;
    c.assets_mb = assets_mb;
    let built = if c.build_secs > 0.0 {
        format!(", built in {:.2} s on a worker", c.build_secs)
    } else {
        String::new()
    };
    let ram = ram.map_or("n/a".to_string(), |mb| format!("{mb:+.0} MB"));
    let took = took.map_or(String::new(), |t| format!(", {t:.1} s from its start"));
    engine_core::log::info!(
        "streaming: loaded {} in {secs:.2} s ({steps} steps{built}{took}), {entities} entities, \
         {assets} assets ({assets_mb:.0} MB), RAM {ram}",
        c.key
    );
}

/// `roots` and all their descendants.
fn tree(world: &World, roots: &[Entity]) -> Vec<Entity> {
    let mut out = roots.to_vec();
    let mut next = 0;
    while next < out.len() {
        if let Some(children) = world.get::<Children>(out[next]) {
            out.extend(children.iter());
        }
        next += 1;
    }
    out
}

/// Marks the meshes this plugin hid, so their own fades are left alone.
#[derive(Component)]
pub(crate) struct StreamFade;

/// Keeps loading cells' meshes hidden, then fades them in.
fn fade(world: &mut World, budget: &StreamingBudget) {
    let mut fades = std::mem::take(&mut world.resource_mut::<Fades>().0);
    fades.retain_mut(|f| {
        let roots: Vec<Entity> = world
            .query::<(Entity, &CellMember)>()
            .iter(world)
            .filter(|(_, m)| m.0 == f.cell)
            .map(|(e, _)| e)
            .collect();
        // Hide what spawned since (a sliced load, or children added a frame later).
        if f.started.is_none() {
            for e in tree(world, &roots) {
                let fresh = world
                    .get_entity(e)
                    .is_ok_and(|r| r.contains::<MeshRenderer>() && !r.contains::<DitherFade>());
                if fresh {
                    world.entity_mut(e).insert((DitherFade(1.0), StreamFade));
                    f.meshes.push(e);
                }
            }
        }
        if !f.loaded {
            return true;
        }
        let t = match f.started {
            None if f.hidden_frames < budget.fade_frames => {
                f.hidden_frames += 1;
                return true;
            }
            None => {
                f.started = Some(Instant::now());
                0.0
            }
            Some(at) => at.elapsed().as_secs_f32() / budget.fade_s.max(1e-3),
        };
        let done = t >= 1.0;
        // The cell's meshes come in on the complementary pattern of its proxy going out, so
        // together they cover every pixel once (DitherFade::pair).
        hlod::fade_out(world, f.cell, t.min(1.0));
        for &e in &f.meshes {
            let Ok(mut entity) = world.get_entity_mut(e) else {
                continue;
            };
            if !entity.contains::<StreamFade>() {
                continue;
            }
            if done {
                entity.remove::<(DitherFade, StreamFade)>();
            } else if let Some(mut d) = entity.get_mut::<DitherFade>() {
                *d = DitherFade::pair(t).1;
            }
        }
        !done
    });
    // Cells registered while fading kept their place; new fades from this frame stay too.
    let mut res = world.resource_mut::<Fades>();
    fades.append(&mut res.0);
    res.0 = fades;
}

/// Cells on their way out: their meshes fading (their proxies coming in), then their roots
/// despawning a slice a frame.
#[derive(Resource, Default)]
struct Leaves(Vec<Leave>);

struct Leave {
    cell: CellId,
    meshes: Vec<Entity>,
    started: Instant,
    /// Roots still to despawn, once the fade is over.
    roots: Option<Vec<Entity>>,
}

/// The cell starts to leave: its meshes fade out over the budget's fade (its far-view proxy
/// comes in on the complementary pattern), then it despawns within the frame budget.
fn begin_leave(world: &mut World, i: usize) {
    let id = CellId(i as u32);
    // A fade-in still running gives way.
    world.resource_mut::<Fades>().0.retain(|f| f.cell != id);
    let roots: Vec<Entity> = world
        .query::<(Entity, &CellMember)>()
        .iter(world)
        .filter(|(_, m)| m.0 == id)
        .map(|(e, _)| e)
        .collect();
    let mut meshes = Vec::new();
    for e in tree(world, &roots) {
        let Ok(r) = world.get_entity(e) else {
            continue;
        };
        let ours = r.contains::<StreamFade>();
        if r.contains::<MeshRenderer>() && (ours || !r.contains::<DitherFade>()) {
            world.entity_mut(e).insert((DitherFade(0.0), StreamFade));
            meshes.push(e);
        }
    }
    world.resource_mut::<Leaves>().0.push(Leave {
        cell: id,
        meshes,
        started: Instant::now(),
        roots: None,
    });
    world.resource_mut::<StreamedCells>().cells[i].state = CellState::Unloading;
}

/// Fades leaving cells out, then despawns their roots within the frame budget (at least one a
/// frame) and finishes their unload.
fn leave(world: &mut World, budget: &StreamingBudget) {
    let mut leaves = std::mem::take(&mut world.resource_mut::<Leaves>().0);
    let start = Instant::now();
    leaves.retain_mut(|l| {
        let i = l.cell.0 as usize;
        if l.roots.is_none() {
            let t = l.started.elapsed().as_secs_f32() / budget.fade_s.max(1e-3);
            hlod::fade_in(world, l.cell, t.min(1.0));
            if t < 1.0 {
                let (out, _) = DitherFade::pair(t);
                for &e in &l.meshes {
                    if let Ok(mut entity) = world.get_entity_mut(e)
                        && let Some(mut d) = entity.get_mut::<DitherFade>()
                    {
                        *d = out;
                    }
                }
                return true;
            }
            let roots: Vec<Entity> = world
                .query::<(Entity, &CellMember)>()
                .iter(world)
                .filter(|(_, m)| m.0 == l.cell)
                .map(|(e, _)| e)
                .collect();
            tracing::info_span!("stream: leave: note").in_scope(|| {
                note_drawn(world, i, &roots);
                remember_leaving(world, i, &roots);
            });
            l.roots = Some(roots);
        }
        let roots = l.roots.as_mut().expect("set above");
        let span = tracing::info_span!("stream: leave: despawn").entered();
        while let Some(e) = roots.pop() {
            if let Ok(entity) = world.get_entity_mut(e) {
                entity.despawn();
            }
            if start.elapsed().as_secs_f32() * 1000.0 > budget.frame_ms {
                break;
            }
        }
        drop(span);
        if !roots.is_empty() {
            return true;
        }
        tracing::info_span!("stream: leave: unload").in_scope(|| unload(world, i));
        false
    });
    let mut res = world.resource_mut::<Leaves>();
    leaves.append(&mut res.0);
    res.0 = leaves;
}

/// Cell `i` is leaving with `roots` still there: a remembering cell notes what changed.
fn remember_leaving(world: &mut World, i: usize, roots: &[Entity]) {
    let Some(baseline) = world.resource_mut::<CellHooks>().0[i].baseline.take() else {
        return;
    };
    let (key, center) = {
        let c = &world.resource::<StreamedCells>().cells[i];
        (c.key.clone(), c.center)
    };
    cell_memory::leaving(world, &key, center, roots, baseline);
}

/// Notes what cell `i`'s `roots` and their descendants draw, for its release.
fn note_drawn(world: &mut World, i: usize, roots: &[Entity]) {
    if !world.resource::<CellHooks>().0[i].loader.release {
        return;
    }
    let all = tree(world, roots);
    let mut hooks = std::mem::take(&mut world.resource_mut::<CellHooks>().0);
    hooks[i].assets.note_drawn(world, &all);
    restore(world, hooks);
}

/// Puts the slots back, after any cells a hook itself registered.
fn restore(world: &mut World, mut hooks: Vec<Slot>) {
    let mut res = world.resource_mut::<CellHooks>();
    hooks.append(&mut res.0);
    res.0 = hooks;
}

fn unload(world: &mut World, i: usize) {
    let id = CellId(i as u32);
    let members: Vec<Entity> = world
        .query::<(Entity, &CellMember)>()
        .iter(world)
        .filter(|(_, m)| m.0 == id)
        .map(|(e, _)| e)
        .collect();
    note_drawn(world, i, &members);
    remember_leaving(world, i, &members);
    for e in members {
        if let Ok(entity) = world.get_entity_mut(e) {
            entity.despawn();
        }
    }
    off_frame::drop_scope(world, art_scope(id));
    world.resource_mut::<Fades>().0.retain(|f| f.cell != id);
    world.resource_mut::<Leaves>().0.retain(|l| l.cell != id);
    hlod::fade_in(world, id, 1.0);
    let mut hooks = std::mem::take(&mut world.resource_mut::<CellHooks>().0);
    // Loaded, and gone again without a source ever within its load distance: the lookahead
    // asked for it and the source turned away.
    let unneeded = hooks[i]
        .loaded_at
        .filter(|_| !hooks[i].needed)
        .map(|t| t.elapsed().as_secs_f32());
    hooks[i].reset();
    if let Some(unload) = hooks[i].loader.unload.as_mut() {
        tracing::info_span!("stream: unload: hook").in_scope(|| unload(world));
    }
    let assets = std::mem::take(&mut hooks[i].assets);
    let release = hooks[i].loader.release;
    restore(world, hooks);
    let freed = if release {
        let (n, bytes) =
            tracing::info_span!("stream: unload: release").in_scope(|| assets.release(world));
        format!(
            ", freed {n} assets ({:.0} MB)",
            bytes as f64 / (1 << 20) as f64
        )
    } else {
        String::new()
    };
    let unneeded = match unneeded {
        Some(secs) => {
            let n = {
                let mut counts = world.resource_mut::<LoadCounts>();
                counts.unneeded += 1;
                counts.unneeded
            };
            if let Some(mut d) = world.get_resource_mut::<Diagnostics>() {
                d.set("stream.unneeded_loads", n as f64);
            }
            format!(" {secs:.1} s after it loaded, never needed")
        }
        None => String::new(),
    };
    let mut cells = world.resource_mut::<StreamedCells>();
    cells.cells[i].state = CellState::Unloaded;
    engine_core::log::info!(
        "streaming: unloaded {}{unneeded}{freed}",
        cells.cells[i].key
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    /// Without fades, so cells come and go at once (the fades have tests of their own).
    fn app() -> App {
        let mut app = App::new();
        app.add_plugins((engine_world::WorldPlugin, StreamingPlugin));
        app.world.resource_mut::<StreamingBudget>().fade_s = 0.0;
        app
    }

    fn place(app: &mut App, e: Entity, x: f64) {
        let t = Transform::from_translation(DVec3::new(x, 0.0, 0.0));
        app.world
            .entity_mut(e)
            .insert((t, GlobalTransform::from(t)));
    }

    fn members(app: &mut App) -> usize {
        app.world.query::<&CellMember>().iter(&app.world).count()
    }

    fn named(app: &mut App, name: &str) -> usize {
        app.world
            .query::<&Name>()
            .iter(&app.world)
            .filter(|n| n.as_str() == name)
            .count()
    }

    #[test]
    fn nothing_streams_around_a_cruising_ship() {
        let mut app = app();
        app.stream_cell(
            StreamCell::new("hut", DVec3::new(50.0, 0.0, 0.0), 20.0, |world| {
                world.spawn((Name::new("hut"), Transform::default()));
            })
            .load_within(100.0),
        );
        // The pilot's camera is the source; the ship it rides cruises.
        let camera = app
            .world
            .spawn(StreamingSource {
                lookahead_s: 0.0,
                ..Default::default()
            })
            .id();
        let ship = app.world.spawn_empty().id();
        place(&mut app, camera, 0.0);
        place(&mut app, ship, 10.0);
        app.update();
        assert_eq!(named(&mut app, "hut"), 1);
        app.world
            .entity_mut(ship)
            .insert(engine_world::physics::Cruising {
                velocity_mps: DVec3::new(3.0e8, 0.0, 0.0),
            });
        app.update();
        assert_eq!(
            named(&mut app, "hut"),
            0,
            "a cruise wants no cells, not all of them"
        );
        // Dropping out: it streams again.
        app.world
            .entity_mut(ship)
            .remove::<engine_world::physics::Cruising>();
        app.update();
        assert_eq!(named(&mut app, "hut"), 1);
    }

    #[test]
    fn a_cell_loads_near_a_source_and_unloads_when_it_leaves() {
        let mut app = app();
        let unloads = Arc::new(AtomicUsize::new(0));
        let seen = unloads.clone();
        app.stream_cell(
            StreamCell::new("hut", DVec3::new(500.0, 0.0, 0.0), 20.0, |world| {
                world
                    .spawn((Name::new("hut"), Transform::default()))
                    .with_children(|c| {
                        c.spawn((Name::new("door"), Transform::default()));
                    });
            })
            .load_within(100.0)
            .on_unload(move |_| {
                seen.fetch_add(1, Ordering::Relaxed);
            }),
        );
        let player = app
            .world
            .spawn((StreamingSource {
                lookahead_s: 0.0,
                ..Default::default()
            },))
            .id();
        place(&mut app, player, 0.0);
        app.update();
        assert_eq!(named(&mut app, "hut"), 0, "too far to load");

        place(&mut app, player, 400.0);
        app.update();
        let cells = app.world.resource::<StreamedCells>();
        assert_eq!(cells.cells[0].state, CellState::Loaded);
        assert!(cells.is_ready_around(DVec3::new(400.0, 0.0, 0.0), 150.0));
        assert_eq!((named(&mut app, "hut"), named(&mut app, "door")), (1, 1));
        assert_eq!(members(&mut app), 1, "the root carries the membership");

        // Within the margin it stays; past it, it goes with its children.
        place(&mut app, player, 360.0);
        app.update();
        assert_eq!(named(&mut app, "hut"), 1);
        place(&mut app, player, 0.0);
        app.update();
        assert_eq!((named(&mut app, "hut"), named(&mut app, "door")), (0, 0));
        assert_eq!(unloads.load(Ordering::Relaxed), 1);

        // And it comes back.
        place(&mut app, player, 480.0);
        app.update();
        assert_eq!(named(&mut app, "hut"), 1);
    }

    #[test]
    fn over_the_memory_cap_a_cell_in_the_margin_goes_early() {
        let mut app = app();
        for (key, x) in [("left", 100.0), ("ahead", 160.0)] {
            app.stream_cell(
                StreamCell::new(key, DVec3::new(x, 0.0, 0.0), 10.0, move |world| {
                    world.spawn(Name::new(key));
                })
                .load_within(100.0),
            );
        }
        let player = app
            .world
            .spawn(StreamingSource {
                lookahead_s: 0.0,
                ..Default::default()
            })
            .id();
        place(&mut app, player, 100.0);
        app.update_n(2);
        assert_eq!((named(&mut app, "left"), named(&mut app, "ahead")), (1, 1));
        // 115 m past "left"'s edge: inside its margin, so it stays while memory is fine.
        place(&mut app, player, 215.0);
        app.update();
        assert_eq!(named(&mut app, "left"), 1);
        app.world.resource_mut::<StreamingBudget>().ram_mb = Some(1.0);
        app.world
            .resource_mut::<Diagnostics>()
            .set(PROCESS_RAM_MB, 150.0);
        app.update();
        assert_eq!((named(&mut app, "left"), named(&mut app, "ahead")), (0, 1));
    }

    #[test]
    fn the_bridge_lists_cells_and_sets_the_budget() {
        use engine_core::registry::call_api;
        let mut app = app();
        app.stream_cell(StreamCell::new("hut", DVec3::ZERO, 5.0, |w| {
            w.spawn(Name::new("hut"));
        }));
        app.update();
        let cells = call_api(&mut app.world, "stream.cells", serde_json::json!({})).unwrap();
        assert_eq!(cells["loaded"], 1);
        assert_eq!(cells["cells"][0]["key"], "hut");
        assert_eq!(cells["cells"][0]["state"], "Loaded");
        let budget = call_api(
            &mut app.world,
            "stream.budget",
            serde_json::json!({"ram_mb": 12000, "loads_per_frame": 2}),
        )
        .unwrap();
        assert_eq!(budget["ram_mb"], 12000.0);
        let set = *app.world.resource::<StreamingBudget>();
        assert_eq!(
            (set.ram_mb, set.loads_per_frame, set.vram_mb),
            (Some(12000.0), 2, None)
        );
    }

    #[test]
    fn pins_hold_a_cell_in_or_out() {
        let mut app = app();
        app.stream_cell(
            StreamCell::new("far", DVec3::new(5000.0, 0.0, 0.0), 5.0, |w| {
                w.spawn(Name::new("far"));
            })
            .keep_loaded(),
        );
        let player = app.world.spawn(StreamingSource::default()).id();
        place(&mut app, player, 0.0);
        app.update();
        assert_eq!(named(&mut app, "far"), 0);
        let pin = |app: &mut App, v| {
            let mut cells = app.world.resource_mut::<StreamedCells>();
            cells.pins.insert("far".into(), v);
        };
        pin(&mut app, true);
        app.update();
        assert_eq!(named(&mut app, "far"), 1, "pinned in");
        pin(&mut app, false);
        app.update();
        assert_eq!(named(&mut app, "far"), 0, "pinned out, though kept");
    }

    #[test]
    fn nearest_first_one_a_frame_and_kept_cells_stay() {
        let mut app = app();
        for (key, x) in [("far", 120.0), ("near", 30.0)] {
            app.stream_cell(
                StreamCell::new(key, DVec3::new(x, 0.0, 0.0), 10.0, move |world| {
                    world.spawn(Name::new(key));
                })
                .keep_loaded(),
            );
        }
        let player = app.world.spawn(StreamingSource::default()).id();
        place(&mut app, player, 0.0);
        app.update();
        assert_eq!((named(&mut app, "near"), named(&mut app, "far")), (1, 0));
        app.update();
        assert_eq!(named(&mut app, "far"), 1);
        place(&mut app, player, 5000.0);
        app.update();
        assert_eq!((named(&mut app, "near"), named(&mut app, "far")), (1, 1));
    }

    /// Runs frames until `done`, a few ms apart (a worker builds meanwhile).
    fn until(app: &mut App, mut done: impl FnMut(&mut App) -> bool) {
        for _ in 0..400 {
            app.update();
            if done(app) {
                return;
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        panic!("never happened");
    }

    fn state(app: &App, key: &str) -> CellState {
        let cells = app.world.resource::<StreamedCells>();
        cells.get(cells.find(key).unwrap()).unwrap().state
    }

    #[derive(Resource, Default)]
    struct HutLedger(u32);

    #[test]
    fn a_resource_a_spawn_inserts_is_not_part_of_the_cell() {
        let mut app = app();
        app.stream_cell(
            StreamCell::new("hut", DVec3::new(100.0, 0.0, 0.0), 10.0, |world| {
                world.init_resource::<HutLedger>();
                world.resource_mut::<HutLedger>().0 += 1;
                world.spawn(Name::new("hut"));
            })
            .load_within(50.0),
        );
        let player = app.world.spawn(StreamingSource::default()).id();
        place(&mut app, player, 100.0);
        app.update();
        assert_eq!(members(&mut app), 1, "the hut, not the ledger");
        place(&mut app, player, 1000.0);
        app.update();
        place(&mut app, player, 100.0);
        app.update();
        assert_eq!(app.world.resource::<HutLedger>().0, 2, "the ledger stayed");
        assert_eq!(named(&mut app, "hut"), 1);
    }

    #[test]
    fn a_built_cell_builds_on_a_worker_then_places() {
        let mut app = app();
        let main = std::thread::current().id();
        app.stream_cell(StreamCell::built(
            "quarry",
            DVec3::ZERO,
            10.0,
            move || {
                assert_ne!(
                    std::thread::current().id(),
                    main,
                    "built off the main thread"
                );
                std::thread::sleep(std::time::Duration::from_millis(20));
                vec!["block"; 3]
            },
            |world, blocks| {
                for b in blocks {
                    world.spawn(Name::new(b));
                }
            },
        ));
        app.update();
        assert_eq!(state(&app, "quarry"), CellState::Building);
        assert!(app.world.resource::<StreamedCells>().waking().count() == 1);
        until(&mut app, |app| state(app, "quarry") == CellState::Loaded);
        assert_eq!(named(&mut app, "block"), 3);
        let cells = app.world.resource::<StreamedCells>();
        assert!(cells.cells[0].build_secs >= 0.02);
        assert_eq!(cells.cells[0].entities, 3);
    }

    #[test]
    fn a_build_nobody_wants_any_more_is_dropped() {
        let mut app = app();
        let (go, wait) = std::sync::mpsc::channel::<()>();
        let wait = std::sync::Mutex::new(wait);
        app.stream_cell(
            StreamCell::built(
                "quarry",
                DVec3::new(100.0, 0.0, 0.0),
                10.0,
                move || {
                    let _ = wait.lock().unwrap().recv();
                },
                |world, ()| {
                    world.spawn(Name::new("block"));
                },
            )
            .load_within(50.0),
        );
        let player = app
            .world
            .spawn(StreamingSource {
                lookahead_s: 0.0,
                ..Default::default()
            })
            .id();
        place(&mut app, player, 100.0);
        app.update();
        assert_eq!(state(&app, "quarry"), CellState::Building);
        place(&mut app, player, 1000.0);
        app.update();
        assert_eq!(state(&app, "quarry"), CellState::Unloaded);
        go.send(()).unwrap();
        app.update_n(5);
        assert_eq!(named(&mut app, "block"), 0, "the late build was dropped");
        // Wanted again, it builds again.
        place(&mut app, player, 100.0);
        go.send(()).unwrap();
        until(&mut app, |app| state(app, "quarry") == CellState::Loaded);
        assert_eq!(named(&mut app, "block"), 1);
    }

    /// A source flying along +x at `speed` m/s (measured from its motion, no `Velocity`), on a
    /// manual 1/60 s clock.
    fn flyer(app: &mut App, lookahead_s: f64) -> Entity {
        app.world
            .resource_mut::<Time>()
            .set_source(engine_core::time::TimeSource::Manual {
                step_secs: 1.0 / 60.0,
            });
        app.world
            .spawn(StreamingSource {
                lookahead_s,
                ..Default::default()
            })
            .id()
    }

    fn fly(app: &mut App, e: Entity, from: f64, speed: f64, frames: usize) -> f64 {
        let mut x = from;
        for _ in 0..frames {
            place(app, e, x);
            app.update();
            x += speed / 60.0;
        }
        x
    }

    #[test]
    fn a_moving_source_loads_along_its_path_by_when_it_gets_there() {
        let mut app = app();
        let hut = |key: &'static str, x: f64| {
            StreamCell::new(key, DVec3::new(x, 0.0, 0.0), 10.0, move |world| {
                world.spawn(Name::new(key));
            })
            .load_within(50.0)
        };
        // Far behind (never wanted), mid-path, near the end of the lookahead.
        app.stream_cell(hut("behind", -2_000.0));
        let x0 = 10_000.0;
        app.stream_cell(hut("near", x0 + 700.0));
        app.stream_cell(hut("far", x0 + 1_600.0));
        let ship = flyer(&mut app, 4.0);
        // 400 m/s with a 4 s lookahead: its path reaches 1.6 km ahead. A teleport to the start
        // is not motion: nothing ahead loads on the jump.
        place(&mut app, ship, x0 - 5_000.0);
        app.update();
        place(&mut app, ship, x0);
        app.update();
        assert_eq!(
            state(&app, "near"),
            CellState::Unloaded,
            "a jump is not a flight"
        );
        // A second of flight: the measured velocity settles and the path reaches both huts,
        // the one it reaches first loading first.
        let x = fly(&mut app, ship, x0, 400.0, 60);
        let cells = app.world.resource::<StreamedCells>();
        assert!(x < x0 + 700.0 - 60.0, "not there yet: {x}");
        assert_eq!(
            cells.get(cells.find("near").unwrap()).unwrap().state,
            CellState::Loaded
        );
        assert_eq!(state(&app, "behind"), CellState::Unloaded);
        let latency = *app.world.resource::<StreamLatency>();
        assert!(latency.loads >= 1, "{latency:?}");
        // With one load a frame, the cell needed first goes first.
        let mut app = app_with_two_ahead();
        let ship = flyer(&mut app, 4.0);
        place(&mut app, ship, 0.0);
        app.update();
        fly(&mut app, ship, 0.0, 400.0, 30);
        let order = app.world.resource::<LoadOrder>().0.clone();
        assert_eq!(
            order,
            vec!["soon", "later"],
            "loaded by when the path gets there"
        );
    }

    /// A hut on the flight line of the tests below.
    fn hut(key: &'static str, at: DVec3) -> StreamCell {
        StreamCell::new(key, at, 10.0, move |world| {
            world.spawn(Name::new(key));
        })
        .load_within(50.0)
    }

    fn place_at(app: &mut App, e: Entity, at: DVec3) {
        let t = Transform::from_translation(at);
        app.world
            .entity_mut(e)
            .insert((t, GlobalTransform::from(t)));
    }

    #[test]
    fn a_quick_cell_far_down_the_path_waits_and_a_slow_one_is_asked_for_early() {
        let mut app = app();
        app.stream_cell(hut("quick", DVec3::new(1_000.0, 0.0, 0.0)));
        app.stream_cell(hut("slow", DVec3::new(1_000.0, 0.0, 40.0)));
        app.stream_cell(hut("new", DVec3::new(1_000.0, 0.0, -40.0)));
        // What their last loads took; a cell never loaded goes by what cells take lately.
        {
            let mut hooks = app.world.resource_mut::<CellHooks>();
            hooks.0[0].took = Some(0.1);
            hooks.0[1].took = Some(6.0);
        }
        // No limit on how far off a slow cell may be asked for (the next test has one).
        app.world.resource_mut::<StreamingBudget>().stretch_loads = f64::INFINITY;
        *app.world.resource_mut::<StreamLatency>() = StreamLatency {
            seconds: 3.0,
            last: 3.0,
            loads: 5,
        };
        let ship = flyer(&mut app, 2.0);
        place(&mut app, ship, 0.0);
        app.update();
        // A second at 100 m/s: 900 m to go. The slow hut is asked for 12 s ahead and the new
        // one about 5 s ahead; the quick one only 2 s ahead, 200 m.
        let x = fly(&mut app, ship, 0.0, 100.0, 60);
        assert_eq!(state(&app, "slow"), CellState::Loaded);
        assert_eq!(state(&app, "quick"), CellState::Unloaded, "at {x}");
        assert_eq!(state(&app, "new"), CellState::Unloaded, "at {x}");
        let x = fly(&mut app, ship, x, 100.0, 240);
        assert_eq!(state(&app, "new"), CellState::Loaded, "at {x}");
        assert_eq!(state(&app, "quick"), CellState::Unloaded, "at {x}");
        // And the quick one is in before the ship is within its 50 m.
        let x = fly(&mut app, ship, x, 100.0, 250);
        assert!(x < 1_000.0 - 60.0, "not there yet: {x}");
        assert_eq!(state(&app, "quick"), CellState::Loaded, "at {x}");
    }

    #[test]
    fn a_cell_asked_for_is_held_by_its_own_lead_not_the_whole_path() {
        let mut app = app();
        app.stream_cell(hut("ahead", DVec3::new(1_000.0, 0.0, 0.0)));
        app.world.resource_mut::<CellHooks>().0[0].took = Some(0.1);
        let ship = flyer(&mut app, 2.0);
        place(&mut app, ship, 700.0);
        app.update();
        // At 100 m/s its 2 s reach the hut from 800 m on: asked for, and loaded.
        let x = fly(&mut app, ship, 700.0, 100.0, 90);
        assert_eq!(state(&app, "ahead"), CellState::Loaded, "at {x}");
        // Slowed to 10 m/s, still heading for it: 20 s of that path would reach the hut, but
        // its 2 s don't, and it goes once its lingering is over.
        let x = fly(&mut app, ship, x, 10.0, 60);
        assert_eq!(state(&app, "ahead"), CellState::Loaded, "lingering at {x}");
        let x = fly(&mut app, ship, x, 10.0, 300);
        assert!(x < 920.0, "{x}");
        assert_eq!(state(&app, "ahead"), CellState::Unloaded, "at {x}");
    }

    #[test]
    fn a_source_too_fast_for_a_slow_cell_asks_for_it_only_a_few_load_distances_off() {
        let mut app = app();
        app.stream_cell(hut("slow", DVec3::new(2_000.0, 0.0, 0.0)));
        app.world.resource_mut::<CellHooks>().0[0].took = Some(6.0);
        let ship = flyer(&mut app, 2.0);
        place(&mut app, ship, 0.0);
        app.update();
        // 100 m/s: 12 s ahead would be 1.2 km off. Two of its 50 m load distances are
        // 100 m, less than the ship's own 2 s (200 m), so the ship looks 200 m ahead for it.
        let x = fly(&mut app, ship, 0.0, 100.0, 600);
        assert_eq!(state(&app, "slow"), CellState::Unloaded, "at {x}");
        let x = fly(&mut app, ship, x, 100.0, 420);
        assert!(x < 2_000.0 - 260.0, "not near yet: {x}");
        assert_eq!(state(&app, "slow"), CellState::Unloaded, "at {x}");
        let x = fly(&mut app, ship, x, 100.0, 60);
        assert_eq!(state(&app, "slow"), CellState::Loaded, "at {x}");
        // A slower ship has the time: at 20 m/s it looks 100 m ahead (5 s), not the 40 m
        // of its own 2 s, and finds the hut's 50 m when it is 150 m from its edge.
        let mut app = self::app();
        app.stream_cell(hut("slow", DVec3::new(2_000.0, 0.0, 0.0)));
        app.world.resource_mut::<CellHooks>().0[0].took = Some(6.0);
        let ship = flyer(&mut app, 2.0);
        place(&mut app, ship, 1_700.0);
        app.update();
        let x = fly(&mut app, ship, 1_700.0, 20.0, 360);
        assert!(x < 1_840.0 - 15.0, "{x}");
        assert_eq!(state(&app, "slow"), CellState::Unloaded, "at {x}");
        let x = fly(&mut app, ship, x, 20.0, 120);
        assert!(x < 2_000.0 - 10.0 - 100.0, "{x}");
        assert_eq!(state(&app, "slow"), CellState::Loaded, "at {x}");
    }

    #[test]
    fn a_cell_the_path_turns_away_from_lingers_then_goes() {
        for linger_s in [4.0, 0.0] {
            let mut app = app();
            app.world.resource_mut::<StreamingBudget>().linger_s = linger_s;
            app.stream_cell(hut("ahead", DVec3::new(300.0, 0.0, 0.0)));
            let ship = flyer(&mut app, 4.0);
            place(&mut app, ship, 0.0);
            app.update();
            // Half a second toward it at 100 m/s: its path reaches the hut, which loads.
            let x = fly(&mut app, ship, 0.0, 100.0, 30);
            assert_eq!(state(&app, "ahead"), CellState::Loaded);
            // A turn to the side, 250 m short of it: the path swings away.
            let fly_z = |app: &mut App, from: f64, frames: usize| {
                let mut z = from;
                for _ in 0..frames {
                    place_at(app, ship, DVec3::new(x, 0.0, z));
                    app.update();
                    z += 100.0 / 60.0;
                }
                z
            };
            let z = fly_z(&mut app, 0.0, 90);
            let lingering = state(&app, "ahead") == CellState::Loaded;
            assert_eq!(lingering, linger_s > 0.0, "1.5 s after the turn");
            fly_z(&mut app, z, 240);
            assert_eq!(state(&app, "ahead"), CellState::Unloaded);
            assert_eq!(named(&mut app, "ahead"), 0);
            let counts = app.world.resource::<LoadCounts>();
            assert_eq!(
                (counts.loads, counts.unneeded),
                (1, 1),
                "it came and went and the ship was never within its 50 m"
            );
        }
    }

    #[test]
    fn a_cell_left_by_a_source_that_stands_goes_at_once() {
        let mut app = app();
        app.stream_cell(hut("hut", DVec3::new(300.0, 0.0, 0.0)));
        let ship = flyer(&mut app, 4.0);
        place(&mut app, ship, 280.0);
        app.update();
        app.update();
        assert_eq!(state(&app, "hut"), CellState::Loaded);
        // A jump away is not a flight: nothing says it will turn back.
        place(&mut app, ship, 5_000.0);
        app.update();
        app.update();
        assert_eq!(state(&app, "hut"), CellState::Unloaded);
        let counts = app.world.resource::<LoadCounts>();
        assert_eq!((counts.loads, counts.unneeded), (1, 0), "it was needed");
    }

    /// A ship that flew toward "ahead" (300 m out) until it loaded, then turned aside 250 m
    /// short of it and flew on for half a second: the hut lingers.
    fn turned_away_from_a_lingering_hut() -> (App, Entity, DVec3) {
        let mut app = app();
        app.stream_cell(hut("ahead", DVec3::new(300.0, 0.0, 0.0)));
        let ship = flyer(&mut app, 4.0);
        place(&mut app, ship, 0.0);
        app.update();
        let x = fly(&mut app, ship, 0.0, 100.0, 30);
        let mut at = DVec3::new(x, 0.0, 0.0);
        for _ in 0..30 {
            at.z += 100.0 / 60.0;
            place_at(&mut app, ship, at);
            app.update();
        }
        assert_eq!(state(&app, "ahead"), CellState::Loaded, "it lingers");
        (app, ship, at)
    }

    #[test]
    fn a_jump_in_flight_leaves_no_cell_lingering() {
        let (mut app, ship, mut at) = turned_away_from_a_lingering_hut();
        // Fast travel 5 km on, still flying the same way there: the hut can't be flown back
        // to in the time it would linger, so it goes at once.
        at.z += 5_000.0;
        for _ in 0..3 {
            at.z += 100.0 / 60.0;
            place_at(&mut app, ship, at);
            app.update();
        }
        assert_eq!(state(&app, "ahead"), CellState::Unloaded);
    }

    #[test]
    fn a_sprint_in_flight_is_not_a_jump() {
        let (mut app, ship, mut at) = turned_away_from_a_lingering_hut();
        // From 100 to 600 m/s in one frame, 10 m a frame: still on its way, and the hut
        // behind it lingers on as it did.
        for _ in 0..30 {
            at.z += 600.0 / 60.0;
            place_at(&mut app, ship, at);
            app.update();
        }
        assert_eq!(state(&app, "ahead"), CellState::Loaded);
    }

    #[test]
    fn over_the_memory_cap_no_cell_lingers() {
        let (mut app, ship, mut at) = turned_away_from_a_lingering_hut();
        app.world.resource_mut::<StreamingBudget>().ram_mb = Some(1.0);
        app.world
            .resource_mut::<Diagnostics>()
            .set(PROCESS_RAM_MB, 150.0);
        for _ in 0..2 {
            at.z += 100.0 / 60.0;
            place_at(&mut app, ship, at);
            app.update();
        }
        assert_eq!(state(&app, "ahead"), CellState::Unloaded);
    }

    #[derive(Resource, Default)]
    struct LoadOrder(Vec<&'static str>);

    /// Two huts ahead of a flight along +x from 0: "soon" on the line 600 m out, "later" 1 km
    /// out and 300 m to the side, so its center is nearer the path's end than "soon" is to the
    /// start, but the flight reaches it later.
    fn app_with_two_ahead() -> App {
        let mut app = app();
        app.init_resource::<LoadOrder>();
        app.world.resource_mut::<StreamingBudget>().loads_per_frame = 1;
        for (key, at) in [
            ("later", DVec3::new(1_000.0, 0.0, 300.0)),
            ("soon", DVec3::new(600.0, 0.0, 0.0)),
        ] {
            app.stream_cell(
                StreamCell::new(key, at, 10.0, move |world| {
                    world.resource_mut::<LoadOrder>().0.push(key);
                    world.spawn(Name::new(key));
                })
                .load_within(300.0),
            );
        }
        app
    }

    #[test]
    fn a_source_standing_in_an_unloaded_cell_counts_as_late_and_gets_the_bigger_budget() {
        let mut app = app();
        {
            let mut budget = app.world.resource_mut::<StreamingBudget>();
            budget.frame_ms = 0.0;
            budget.late_frame_ms = 1_000.0;
        }
        app.stream_cell(
            StreamCell::in_steps("wall", DVec3::new(100.0, 0.0, 0.0), 10.0, |world, i| {
                world.spawn(Name::new("brick"));
                i == 3
            })
            .load_within(50.0),
        );
        let player = app
            .world
            .spawn(StreamingSource {
                lookahead_s: 0.0,
                ..Default::default()
            })
            .id();
        place(&mut app, player, 100.0);
        app.update();
        // Standing in it: every slice in one frame, and the frame counted late.
        assert_eq!(state(&app, "wall"), CellState::Loaded);
        assert_eq!(named(&mut app, "brick"), 4);
        assert_eq!(
            app.world.resource::<Diagnostics>().get("stream.late"),
            Some(1.0)
        );
        app.update();
        assert_eq!(
            app.world.resource::<Diagnostics>().get("stream.late"),
            Some(0.0)
        );
    }

    #[test]
    fn a_sliced_cell_spawns_a_slice_a_frame_and_can_leave_half_in() {
        let mut app = app();
        {
            let mut budget = app.world.resource_mut::<StreamingBudget>();
            budget.frame_ms = 0.0;
            budget.late_frame_ms = 0.0;
        }
        app.stream_cell(
            StreamCell::in_steps("wall", DVec3::new(100.0, 0.0, 0.0), 10.0, |world, i| {
                world.spawn(Name::new("brick")).with_children(|c| {
                    c.spawn(Name::new("mortar"));
                });
                i == 2
            })
            .load_within(50.0),
        );
        let player = app
            .world
            .spawn(StreamingSource {
                lookahead_s: 0.0,
                ..Default::default()
            })
            .id();
        place(&mut app, player, 100.0);
        app.update();
        assert_eq!(state(&app, "wall"), CellState::Loading);
        assert_eq!(named(&mut app, "brick"), 1);
        app.update_n(2);
        assert_eq!(state(&app, "wall"), CellState::Loaded);
        assert_eq!((named(&mut app, "brick"), members(&mut app)), (3, 3));
        assert_eq!(app.world.resource::<StreamedCells>().cells[0].entities, 6);

        // Leaving half way through takes the half that is in.
        place(&mut app, player, 1000.0);
        app.update();
        place(&mut app, player, 100.0);
        app.update();
        assert_eq!(state(&app, "wall"), CellState::Loading);
        place(&mut app, player, 1000.0);
        app.update();
        assert_eq!(state(&app, "wall"), CellState::Unloaded);
        assert_eq!(named(&mut app, "brick"), 0);
        // And the next load starts from the first slice.
        place(&mut app, player, 100.0);
        app.update_n(3);
        assert_eq!(named(&mut app, "brick"), 3);
    }

    #[test]
    fn a_cell_that_releases_its_assets_frees_what_nothing_else_draws() {
        use engine_assets::{AssetAppExt, Assets, Handle};
        use engine_render::{Image, MaterialRef, Mesh, MeshRef, Shape, StandardMaterial};
        use std::sync::Mutex;

        let mut app = app();
        app.init_asset::<Mesh>()
            .init_asset::<StandardMaterial>()
            .init_asset::<Image>();
        let cube = || Shape::Cuboid { size: [1.0; 3] }.mesh();
        // Made before the cell: not its own.
        let shared = app.world.resource_mut::<Assets<Mesh>>().add(cube());
        type Made = (
            Handle<Mesh>,
            Handle<Mesh>,
            Handle<Mesh>,
            Handle<StandardMaterial>,
            Handle<Image>,
        );
        let made: Arc<Mutex<Option<Made>>> = Arc::default();
        let out = made.clone();
        app.stream_cell(
            StreamCell::new("yard", DVec3::ZERO, 10.0, move |world| {
                let mut meshes = world.resource_mut::<Assets<Mesh>>();
                let own = meshes.add(cube());
                let lent = meshes.add(cube());
                let keyed = meshes.get_or_insert_keyed("shape:test", cube);
                let image = world.resource_mut::<Assets<Image>>().add(Image::default());
                let material = world
                    .resource_mut::<Assets<StandardMaterial>>()
                    .add(StandardMaterial::default().with_base_color_texture(image));
                world.spawn(MeshRenderer {
                    mesh: MeshRef::Handle(own),
                    material: MaterialRef::Handle(material),
                    ..Default::default()
                });
                world.spawn(MeshRenderer {
                    mesh: MeshRef::Handle(shared),
                    ..Default::default()
                });
                *out.lock().unwrap() = Some((own, lent, keyed, material, image));
            })
            .release_assets(),
        );
        app.update();
        assert_eq!(state(&app, "yard"), CellState::Loaded);
        let (own, lent, keyed, material, image) = made.lock().unwrap().take().unwrap();
        let info = app.world.resource::<StreamedCells>().cells[0].clone();
        assert_eq!(info.assets, 4, "own, lent, the material and the image");
        assert!(info.assets_mb > 0.0 && info.releases_assets);
        // Something outside the cell draws one of its meshes.
        app.world.spawn(MeshRenderer {
            mesh: MeshRef::Handle(lent),
            ..Default::default()
        });
        app.world
            .resource_mut::<StreamedCells>()
            .pins
            .insert("yard".into(), false);
        app.update();
        assert_eq!(state(&app, "yard"), CellState::Unloaded);
        let meshes = app.world.resource::<Assets<Mesh>>();
        assert!(!meshes.contains(own), "freed");
        assert!(meshes.contains(lent), "still drawn");
        assert!(
            meshes.contains(keyed) && meshes.contains(shared),
            "not the cell's"
        );
        assert!(
            !app.world
                .resource::<Assets<StandardMaterial>>()
                .contains(material)
        );
        assert!(!app.world.resource::<Assets<Image>>().contains(image));
        // The next asset takes the freed slot; the old handle still reads as missing.
        let next = app.world.resource_mut::<Assets<Mesh>>().add(cube());
        assert_eq!(next.index(), own.index());
        assert!(!app.world.resource::<Assets<Mesh>>().contains(own));
    }

    #[test]
    fn a_cells_off_frame_art_lands_in_its_load_and_goes_with_it() {
        use engine_assets::{AssetAppExt, Assets};
        use engine_render::{MeshRef, Shape};

        let mut app = app();
        app.init_asset::<engine_render::Mesh>();
        let sleep_ms = Arc::new(AtomicUsize::new(40));
        let ms = sleep_ms.clone();
        app.stream_cell(
            StreamCell::new("court", DVec3::ZERO, 10.0, move |world| {
                world.spawn((Name::new("court floor"), Transform::default()));
                let ms = ms.load(Ordering::Relaxed) as u64;
                off_frame::off_frame(
                    world,
                    "court statue",
                    move || {
                        std::thread::sleep(std::time::Duration::from_millis(ms));
                        Shape::Cuboid { size: [1.0; 3] }.mesh()
                    },
                    |world, mesh| {
                        let mesh = world
                            .resource_mut::<Assets<engine_render::Mesh>>()
                            .add(mesh);
                        world
                            .spawn((Name::new("statue"), Transform::default()))
                            .with_children(|c| {
                                c.spawn((
                                    Name::new("statue body"),
                                    Transform::default(),
                                    MeshRenderer {
                                        mesh: MeshRef::Handle(mesh),
                                        ..Default::default()
                                    },
                                ));
                            });
                    },
                );
            })
            .release_assets(),
        );
        app.update();
        // The spawn ran; its art is still on the worker, so the cell isn't in yet.
        assert_eq!(state(&app, "court"), CellState::Loading);
        assert!(
            !app.world
                .resource::<StreamedCells>()
                .is_ready_around(DVec3::ZERO, 50.0)
        );
        assert_eq!(
            (named(&mut app, "court floor"), named(&mut app, "statue")),
            (1, 0)
        );
        let t = Instant::now();
        while state(&app, "court") != CellState::Loaded {
            assert!(t.elapsed().as_secs() < 10, "the art never landed");
            std::thread::sleep(std::time::Duration::from_millis(2));
            app.update();
        }
        assert_eq!(named(&mut app, "statue body"), 1);
        let statue = app
            .world
            .query::<(Entity, &Name)>()
            .iter(&app.world)
            .find(|(_, n)| n.as_str() == "statue")
            .map(|(e, _)| e)
            .unwrap();
        assert_eq!(
            app.world.get::<CellMember>(statue).map(|m| m.0),
            Some(CellId(0)),
            "the art is the cell's"
        );
        let info = app.world.resource::<StreamedCells>().cells[0].clone();
        assert_eq!(info.assets, 1, "the statue's mesh is counted in the cell");

        // It goes with the cell, mesh and all.
        let pins = |app: &mut App, on: bool| {
            app.world
                .resource_mut::<StreamedCells>()
                .pins
                .insert("court".into(), on);
        };
        pins(&mut app, false);
        app.update();
        assert_eq!(state(&app, "court"), CellState::Unloaded);
        assert_eq!(
            (named(&mut app, "statue"), named(&mut app, "statue body")),
            (0, 0)
        );
        assert_eq!(app.world.resource::<Assets<engine_render::Mesh>>().len(), 0);

        // Leaving with the art in flight drops it: it never lands later.
        sleep_ms.store(60, Ordering::Relaxed);
        pins(&mut app, true);
        app.update();
        assert_eq!(state(&app, "court"), CellState::Loading);
        pins(&mut app, false);
        app.update();
        assert_eq!(state(&app, "court"), CellState::Unloaded);
        assert!(app.world.resource::<off_frame::OffFrameJobs>().is_empty());
        std::thread::sleep(std::time::Duration::from_millis(100));
        for _ in 0..3 {
            app.update();
        }
        assert_eq!(
            (named(&mut app, "court floor"), named(&mut app, "statue")),
            (0, 0)
        );
        assert_eq!(app.world.resource::<Assets<engine_render::Mesh>>().len(), 0);
    }

    #[test]
    fn shapes_and_inline_materials_go_with_the_last_cell_that_drew_them() {
        use engine_assets::{AssetAppExt, Assets};
        use engine_render::{MaterialRef, Mesh, MeshRef, ResolvedMesh, Shape, StandardMaterial};

        let mut app = app();
        app.init_asset::<Mesh>().init_asset::<StandardMaterial>();
        // Draws a shape with an inline material and resolves it as the renderer would.
        fn draw(world: &mut World, size: f32, color: f32) {
            let shape = Shape::Cuboid { size: [size; 3] };
            let material = StandardMaterial {
                base_color: [color, 0.5, 0.5, 1.0],
                ..Default::default()
            };
            let mesh = world
                .resource_mut::<Assets<Mesh>>()
                .get_or_insert_keyed(&shape.key(), || shape.mesh());
            let mat = world
                .resource_mut::<Assets<StandardMaterial>>()
                .get_or_insert_keyed(&material.key(), || material);
            world.spawn((
                MeshRenderer {
                    mesh: MeshRef::Shape(shape),
                    material: MaterialRef::Inline(material),
                    ..Default::default()
                },
                ResolvedMesh {
                    mesh,
                    material: mat,
                },
            ));
        }
        // Both draw the 1 m cube in red; each has a shape of its own.
        app.stream_cell(
            StreamCell::new("a", DVec3::ZERO, 10.0, |w| {
                draw(w, 1.0, 1.0);
                draw(w, 2.0, 0.2);
            })
            .release_assets(),
        );
        app.stream_cell(
            StreamCell::new("b", DVec3::new(30.0, 0.0, 0.0), 10.0, |w| {
                draw(w, 1.0, 1.0);
                draw(w, 3.0, 0.3);
            })
            .release_assets(),
        );
        app.update_n(2);
        let counts = |app: &App| {
            (
                app.world.resource::<Assets<Mesh>>().len(),
                app.world.resource::<Assets<StandardMaterial>>().len(),
            )
        };
        assert_eq!(counts(&app), (3, 3));
        let pin = |app: &mut App, key: &str| {
            app.world
                .resource_mut::<StreamedCells>()
                .pins
                .insert(key.into(), false);
            app.update();
        };
        pin(&mut app, "a");
        assert_eq!(
            counts(&app),
            (2, 2),
            "a's own shape went, the shared cube stays"
        );
        pin(&mut app, "b");
        assert_eq!(counts(&app), (0, 0));
        // Back in, the renderer makes them again from their keys.
        app.world.resource_mut::<StreamedCells>().pins.clear();
        app.update_n(2);
        assert_eq!(counts(&app), (3, 3));
    }

    #[test]
    fn inline_materials_and_shapes_resolve_again_after_a_release() {
        use engine_assets::{AssetAppExt, Assets};
        use engine_render::{Mesh, ResolvedMesh, Shape, StandardMaterial};

        let mut app = app();
        app.init_asset::<Mesh>()
            .init_asset::<StandardMaterial>()
            .add_systems(Update, engine_render::resolve_mesh_renderers);
        let draw = |x: f64| {
            move |w: &mut World| {
                w.spawn((
                    Transform::from_translation(DVec3::new(x, 0.0, 0.0)),
                    MeshRenderer::new(
                        Shape::Cuboid { size: [1.0; 3] },
                        StandardMaterial::hex(0x6fb3e0),
                    ),
                ));
            }
        };
        // Two cells drawing the same shape and material, as neighbouring zones do.
        app.stream_cell(StreamCell::new("a", DVec3::ZERO, 10.0, draw(0.0)).release_assets());
        app.stream_cell(
            StreamCell::new("b", DVec3::new(30.0, 0.0, 0.0), 10.0, draw(30.0)).release_assets(),
        );
        let live = |app: &mut App| {
            let resolved: Vec<ResolvedMesh> = app
                .world
                .query::<&ResolvedMesh>()
                .iter(&app.world)
                .copied()
                .collect();
            let meshes = app.world.resource::<Assets<Mesh>>();
            let materials = app.world.resource::<Assets<StandardMaterial>>();
            resolved
                .iter()
                .all(|r| meshes.contains(r.mesh) && materials.contains(r.material))
                .then_some(resolved.len())
        };
        app.update_n(3);
        assert_eq!(live(&mut app), Some(2));
        for key in ["a", "b", "a"] {
            app.world
                .resource_mut::<StreamedCells>()
                .pins
                .insert(key.into(), false);
            app.update_n(2);
            app.world.resource_mut::<StreamedCells>().pins.remove(key);
            app.update_n(3);
            assert_eq!(live(&mut app), Some(2), "after {key} went and came back");
        }
    }

    #[test]
    fn a_remembering_cell_keeps_what_was_destroyed_and_moved() {
        use engine_world::CellMemory;
        use engine_world::shift::WorldShift;

        let mut app = app();
        // Spawned where the cell is now (world shifts move it), as streamed content must be.
        let yard = |w: &mut World| {
            let cells = w.resource::<StreamedCells>();
            let at = cells.get(cells.find("yard").unwrap()).unwrap().center - DVec3::X * 5.0;
            for (i, x) in [0.0, 2.0, 4.0].into_iter().enumerate() {
                w.spawn((
                    Name::new(format!("crate {i}")),
                    Transform::from_translation(at + DVec3::new(x, 0.0, 0.0)),
                ))
                .with_children(|c| {
                    c.spawn((Name::new("lid"), Transform::from_xyz(0.0, 1.0, 0.0)));
                });
            }
            // Two alike in one place: still told apart.
            for _ in 0..2 {
                w.spawn((
                    Name::new("barrel"),
                    Transform::from_translation(at + DVec3::new(10.0, 0.0, 0.0)),
                ));
            }
        };
        app.stream_cell(
            StreamCell::new("yard", DVec3::new(505.0, 0.0, 0.0), 20.0, yard)
                .load_within(50.0)
                .remember(),
        );
        // The same yard, forgetful.
        app.stream_cell(
            StreamCell::new("shed", DVec3::new(-505.0, 0.0, 0.0), 20.0, move |w| {
                w.spawn((
                    Name::new("tool"),
                    Transform::from_translation(DVec3::new(-505.0, 0.0, 0.0)),
                ));
            })
            .load_within(50.0),
        );
        let player = app.world.spawn(StreamingSource::default()).id();
        let find = |app: &mut App, name: &str| -> Vec<Entity> {
            app.world
                .query::<(Entity, &Name)>()
                .iter(&app.world)
                .filter(|(_, n)| n.as_str() == name)
                .map(|(e, _)| e)
                .collect()
        };
        place(&mut app, player, 505.0);
        app.update();
        assert_eq!(state(&app, "yard"), CellState::Loaded);
        // Play: crate 1 is blown up, crate 2 is pushed 3 m, one barrel rolls away, a lid tips.
        let gone = find(&mut app, "crate 1")[0];
        app.world.entity_mut(gone).despawn();
        let pushed = find(&mut app, "crate 2")[0];
        app.world
            .get_mut::<Transform>(pushed)
            .unwrap()
            .translation
            .z = 3.0;
        let barrel = find(&mut app, "barrel")[1];
        app.world
            .get_mut::<Transform>(barrel)
            .unwrap()
            .translation
            .x = 520.0;
        let crate0 = find(&mut app, "crate 0")[0];
        let lid = app.world.get::<Children>(crate0).unwrap()[0];
        app.world.get_mut::<Transform>(lid).unwrap().translation.y = 0.2;
        // Away, and the world shifts 1 km while the yard is out.
        place(&mut app, player, 0.0);
        app.update();
        assert_eq!(state(&app, "yard"), CellState::Unloaded);
        let memory = app.world.resource::<CellMemory>().cells["yard"].clone();
        assert_eq!(memory.removed.len(), 2, "crate 1 and its lid");
        assert_eq!(
            memory.changed.len(),
            3,
            "crate 2, a barrel, a lid: {memory:?}"
        );
        app.world
            .write_message(WorldShift::translate(DVec3::new(-1000.0, 0.0, 0.0)));
        app.update();
        // Back: as it was left, at the shifted place.
        place(&mut app, player, -495.0);
        app.update();
        assert_eq!(state(&app, "yard"), CellState::Loaded);
        assert!(find(&mut app, "crate 1").is_empty(), "stays destroyed");
        assert_eq!(find(&mut app, "lid").len(), 2);
        let crate2 = find(&mut app, "crate 2")[0];
        let at = app.world.get::<Transform>(crate2).unwrap().translation;
        assert!(at.distance(DVec3::new(-496.0, 0.0, 3.0)) < 1e-6, "{at:?}");
        let mut barrels: Vec<f64> = find(&mut app, "barrel")
            .into_iter()
            .map(|e| app.world.get::<Transform>(e).unwrap().translation.x)
            .collect();
        barrels.sort_by(f64::total_cmp);
        assert!((barrels[0] + 490.0).abs() < 1e-6 && (barrels[1] + 480.0).abs() < 1e-6);
        let crate0 = find(&mut app, "crate 0")[0];
        let lid = app.world.get::<Children>(crate0).unwrap()[0];
        assert!((app.world.get::<Transform>(lid).unwrap().translation.y - 0.2).abs() < 1e-6);
        // Nothing changed this time: the memory stays as it was.
        place(&mut app, player, 0.0);
        app.update();
        assert_eq!(app.world.resource::<CellMemory>().cells["yard"], memory);
        // A forgetful cell starts over every time.
        place(&mut app, player, -1505.0);
        app.update();
        let tool = find(&mut app, "tool")[0];
        app.world.entity_mut(tool).despawn();
        place(&mut app, player, 0.0);
        app.update();
        place(&mut app, player, -1505.0);
        app.update();
        assert_eq!(find(&mut app, "tool").len(), 1);
        assert!(
            !app.world
                .resource::<CellMemory>()
                .cells
                .contains_key("shed")
        );
        // A save game takes the memory and gives it back; forgetting resets the cell.
        let saved = app.world.resource::<CellMemory>().clone();
        app.world.resource_mut::<CellMemory>().cells.clear();
        *app.world.resource_mut::<CellMemory>() = saved.clone();
        assert_eq!(*app.world.resource::<CellMemory>(), saved);
        app.world.resource_mut::<CellMemory>().cells.remove("yard");
        place(&mut app, player, -495.0);
        app.update();
        assert_eq!(
            find(&mut app, "crate 1").len(),
            1,
            "a reset cell is new again"
        );
    }

    #[test]
    fn a_cell_on_a_moving_frame_rides_with_it() {
        let mut app = app();
        let pose = |app: &mut App, e: Entity, at: DVec3, turn_deg: f64| {
            let t = Transform::from_translation(at)
                .with_rotation(DQuat::from_rotation_y(turn_deg.to_radians()).as_quat());
            app.world
                .entity_mut(e)
                .insert((t, GlobalTransform::from(t)));
        };
        app.stream_cell(
            StreamCell::new("base", DVec3::new(100.0, 0.0, 0.0), 10.0, |w| {
                w.spawn(Name::new("dome"));
            })
            .load_within(50.0)
            .on_frame("moon"),
        );
        let player = app.world.spawn(StreamingSource::default()).id();
        place(&mut app, player, 100.0);
        // Before the frame shows up the cell stays where it was given.
        app.update();
        assert_eq!(state(&app, "base"), CellState::Loaded);
        let moon = app.world.spawn(StreamFrame("moon".into())).id();
        pose(&mut app, moon, DVec3::ZERO, 0.0);
        app.update();
        // The moon moves off and turns a quarter: the base goes with it.
        pose(&mut app, moon, DVec3::new(1000.0, 0.0, 0.0), 90.0);
        app.update();
        let center = app.world.resource::<StreamedCells>().cells[0].center;
        assert!(
            (center - DVec3::new(1000.0, 0.0, -100.0)).length() < 1e-3,
            "{center}"
        );
        assert_eq!(
            state(&app, "base"),
            CellState::Unloaded,
            "the player stayed"
        );
        let there = Transform::from_xyz(1000.0, 0.0, -100.0);
        app.world
            .entity_mut(player)
            .insert((there, GlobalTransform::from(there)));
        app.update();
        assert_eq!(
            state(&app, "base"),
            CellState::Loaded,
            "found where it went"
        );
    }

    #[test]
    fn new_meshes_stay_hidden_then_fade_in() {
        let mut app = app();
        {
            let mut budget = app.world.resource_mut::<StreamingBudget>();
            budget.fade_frames = 2;
            budget.fade_s = 0.5;
        }
        app.stream_cell(StreamCell::new("hut", DVec3::ZERO, 10.0, |world| {
            world
                .spawn((Name::new("hut"), MeshRenderer::default()))
                .with_children(|c| {
                    c.spawn((Name::new("sign"), MeshRenderer::default(), DitherFade(0.3)));
                });
        }));
        let player = app.world.spawn(StreamingSource::default()).id();
        place(&mut app, player, 0.0);
        let fade = |app: &mut App, name: &str| {
            app.world
                .query::<(&Name, Option<&DitherFade>)>()
                .iter(&app.world)
                .find(|(n, _)| n.as_str() == name)
                .and_then(|(_, d)| d.copied())
        };
        app.update();
        assert_eq!(
            fade(&mut app, "hut"),
            Some(DitherFade(1.0)),
            "hidden at first"
        );
        assert_eq!(
            fade(&mut app, "sign"),
            Some(DitherFade(0.3)),
            "its own fade left alone"
        );
        app.update_n(3);
        let mid = fade(&mut app, "hut").expect("fading").0;
        assert!(mid < 1.0, "{mid}");
        std::thread::sleep(std::time::Duration::from_millis(550));
        app.update();
        assert_eq!(fade(&mut app, "hut"), None, "fully in");
        assert_eq!(fade(&mut app, "sign"), Some(DitherFade(0.3)));
        assert_eq!(app.world.query::<&StreamFade>().iter(&app.world).count(), 0);

        // With no source (tools, tours) cells show at once.
        let mut app = self::app();
        app.stream_cell(StreamCell::new("hut", DVec3::ZERO, 10.0, |world| {
            world.spawn((Name::new("hut"), MeshRenderer::default()));
        }));
        app.update();
        assert_eq!(fade(&mut app, "hut"), None);
    }

    #[test]
    fn a_fast_source_loads_ahead_and_no_source_loads_everything() {
        let mut app = app();
        app.stream_cell(StreamCell::new(
            "ahead",
            DVec3::new(2000.0, 0.0, 0.0),
            10.0,
            |w| {
                w.spawn(Name::new("ahead"));
            },
        ));
        // No source and no focus: tools and tests see the whole world.
        app.update();
        assert_eq!(named(&mut app, "ahead"), 1);

        let mut app = self::app();
        app.stream_cell(StreamCell::new(
            "ahead",
            DVec3::new(2000.0, 0.0, 0.0),
            10.0,
            |w| {
                w.spawn(Name::new("ahead"));
            },
        ));
        let ship = app
            .world
            .spawn((
                StreamingSource::default(),
                Velocity {
                    linear: Vec3::new(1000.0, 0.0, 0.0),
                    ..Default::default()
                },
            ))
            .id();
        place(&mut app, ship, 0.0);
        app.update();
        assert_eq!(named(&mut app, "ahead"), 1, "2 s ahead at 1 km/s");
    }
}
