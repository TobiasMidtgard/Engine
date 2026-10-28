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

use std::any::Any;
use std::sync::mpsc::{self, Receiver, TryRecvError};
use std::sync::{Arc, Mutex};
use std::time::Instant;

use crate::hlod::{self, ProxyConfig};
use engine_core::diagnostics::{Diagnostics, PROCESS_RAM_MB, process_ram_mb};
use engine_core::ecs::resource::IsResource;
use engine_core::prelude::*;
use engine_render::{DitherFade, MeshRenderer};
use engine_world::physics::Velocity;
use engine_world::stream::CellInfo;
use engine_world::{
    CellId, CellMember, CellState, FocusPoints, SimulationFocus, StreamedCells, StreamingSource,
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
            },
            loader: Loader {
                build,
                step,
                unload: None,
                proxy: ProxyConfig::default(),
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

    /// Runs after the cell's entities are despawned, to drop what its module keeps elsewhere
    /// (a resource, a cache).
    pub fn on_unload(mut self, unload: impl FnMut(&mut World) + Send + Sync + 'static) -> Self {
        self.loader.unload = Some(Box::new(unload));
        self
    }
}

/// Registers streamed cells.
pub trait StreamCellsExt {
    fn stream_cell(&mut self, cell: StreamCell) -> &mut Self;
}

impl StreamCellsExt for World {
    fn stream_cell(&mut self, cell: StreamCell) -> &mut Self {
        self.init_resource::<StreamedCells>();
        self.init_resource::<CellHooks>();
        self.resource_mut::<StreamedCells>().cells.push(cell.info);
        self.resource_mut::<CellHooks>()
            .0
            .push(Slot::new(cell.loader));
        self
    }
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
    /// always runs, so loading never stalls).
    pub frame_ms: f32,
    /// Build steps running on worker threads at once at most.
    pub builds: usize,
    /// Seconds a newly loaded cell's meshes take to fade in (0 shows them at once). Only while
    /// something streams: with no source (tools, tours) cells show at once.
    pub fade_s: f32,
    /// Frames a loaded cell stays hidden before it fades in, while its uploads land.
    pub fade_frames: u32,
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
            frame_ms: 8.0,
            builds: 2,
            fade_s: 0.4,
            fade_frames: 5,
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
    secs: f32,
    ram: Option<f64>,
}

impl Slot {
    fn new(loader: Loader) -> Self {
        Self {
            loader,
            building: None,
            built: None,
            failed: false,
            step: 0,
            secs: 0.0,
            ram: None,
        }
    }

    /// Forget a load in progress (an unwanted build's answer is dropped when it comes).
    fn reset(&mut self) {
        self.building = None;
        self.built = None;
        self.step = 0;
        self.secs = 0.0;
        self.ram = None;
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
        app.init_resource::<StreamedCells>()
            .init_resource::<CellHooks>()
            .init_resource::<StreamingBudget>()
            .init_resource::<Fades>()
            .init_resource::<Leaves>()
            .add_systems(Last, stream_system)
            .register_api(
                "stream.cells",
                "List the streamed cells (key, center, radius, load distance, state; from the \
                 last load: seconds spawning, seconds building on a worker, entities spawned and \
                 the RAM it added in MB), how many are loaded, and the process RAM and GPU \
                 memory in MB.",
                "{}",
                |world, _| {
                    let cells = world.resource::<StreamedCells>();
                    let diag = world.resource::<Diagnostics>();
                    Ok(serde_json::json!({
                        "cells": cells.cells,
                        "loaded": cells.cells.iter().filter(|c| c.state == CellState::Loaded).count(),
                        "ram_mb": diag.get(PROCESS_RAM_MB),
                        "vram_mb": diag.get("render.gpu_memory_mb"),
                    }))
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

/// Where the sources are: (point, range scale) for each, plus its lookahead point. None when
/// there is no source and no focus (tools and tests: everything loads).
fn source_points(world: &mut World) -> Option<Vec<(DVec3, f64)>> {
    let mut points = Vec::new();
    let mut sources = world.query::<(&GlobalTransform, &StreamingSource, Option<&Velocity>)>();
    for (g, s, v) in sources.iter(world) {
        let p = g.translation();
        points.push((p, s.range_scale));
        if let Some(v) = v {
            points.push((p + v.linear.as_dvec3() * s.lookahead_s, s.range_scale));
        }
    }
    if points.is_empty() {
        let mut foci = world.query_filtered::<&GlobalTransform, With<SimulationFocus>>();
        points.extend(foci.iter(world).map(|g| (g.translation(), 1.0)));
        if points.is_empty() {
            let focus = world.resource::<FocusPoints>();
            points.extend(focus.points.iter().map(|p| (*p, 1.0)));
        }
    }
    (!points.is_empty()).then_some(points)
}

/// `Last`: queue, build, load, fade and unload cells.
pub fn stream_system(world: &mut World) {
    if world.resource::<StreamedCells>().cells.is_empty() {
        return;
    }
    let sources = source_points(world);
    let budget = *world.resource::<StreamingBudget>();
    let has_build: Vec<(bool, bool)> = world
        .resource::<CellHooks>()
        .0
        .iter()
        .map(|s| (s.loader.build.is_some(), s.failed))
        .collect();
    let mut cells = world.resource_mut::<StreamedCells>();
    let hysteresis = cells.hysteresis;
    // Each cell's distance from the nearest source, in units of its load distance.
    let reach = |c: &CellInfo| match &sources {
        None => 0.0,
        Some(points) => points
            .iter()
            .map(|(p, scale)| c.distance(*p) / (c.load_m * scale).max(1e-6))
            .fold(f64::INFINITY, f64::min),
    };
    let mut want = Vec::new();
    let mut dropped = Vec::new();
    let mut leaving = Vec::new();
    let mut spare: Option<(f64, usize)> = None;
    let pins = std::mem::take(&mut cells.pins);
    for (i, c) in cells.cells.iter_mut().enumerate() {
        let r = match pins.get(&c.key) {
            Some(true) => 0.0,
            Some(false) => f64::INFINITY,
            None => reach(c),
        };
        let kept = c.keep_loaded && pins.get(&c.key) != Some(&false);
        let (build, failed) = has_build.get(i).copied().unwrap_or_default();
        match c.state {
            CellState::Unloaded if r <= 1.0 && !failed => {
                c.state = if build {
                    CellState::Building
                } else {
                    CellState::Queued
                };
            }
            CellState::Building | CellState::Queued if r > 1.0 + hysteresis => {
                c.state = CellState::Unloaded;
                dropped.push(i);
            }
            CellState::Loading | CellState::Loaded if r > 1.0 + hysteresis && !kept => {
                leaving.push(i)
            }
            CellState::Loaded if r > 1.0 && !kept && spare.is_none_or(|s| r > s.0) => {
                spare = Some((r, i));
            }
            _ => {}
        }
        if c.state.is_waking() {
            want.push((r, i, c.state));
        }
    }
    cells.pins = pins;
    let pressed = leaving.is_empty() && budget.over(world.resource::<Diagnostics>());
    if let Some((_, i)) = spare.filter(|_| pressed) {
        leaving.push(i);
    }
    for i in dropped {
        world.resource_mut::<CellHooks>().0[i].reset();
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
    leave(world, &budget);
    want.retain(|w| !leaving.contains(&w.1));
    want.sort_by(|a, b| a.0.total_cmp(&b.0));
    builds(world, &want, budget.builds);
    // Cells already loading go first, nearest first, then new ones in their turn.
    let start = Instant::now();
    let over = |start: Instant| start.elapsed().as_secs_f32() * 1000.0 > budget.frame_ms;
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
            begin(world, i, sources.is_some() && budget.fade_s > 0.0);
        }
        loop {
            if stepped && over(start) {
                break 'cells;
            }
            stepped = true;
            if step(world, id) {
                finish(world, i);
                break;
            }
        }
    }
    fade(world, &budget);
    let configs: Vec<ProxyConfig> = world
        .resource::<CellHooks>()
        .0
        .iter()
        .map(|s| s.loader.proxy)
        .collect();
    let fading: Vec<CellId> = world.resource::<Fades>().0.iter().map(|f| f.cell).collect();
    hlod::update(world, &configs, &fading, sources.is_some());
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
        // A thread of its own: a build may use the compute pool's scopes without waiting on
        // the pool it runs in.
        let spawned = std::thread::Builder::new()
            .name(format!("cell build {key}"))
            .spawn(move || {
                let t = Instant::now();
                let built = build();
                let _ = tx.send((built, t.elapsed().as_secs_f32()));
            });
        match spawned {
            Ok(_) => {
                slot.building = Some(Mutex::new(rx));
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
    let slot = &mut world.resource_mut::<CellHooks>().0[i];
    slot.step = 0;
    slot.secs = 0.0;
    slot.ram = process_ram_mb();
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

/// Runs one slice of the cell's spawn and tags what it spawned; true when it is all in.
fn step(world: &mut World, id: CellId) -> bool {
    let i = id.0 as usize;
    let t = Instant::now();
    let before = Snapshot::of(world);
    let mut hooks = std::mem::take(&mut world.resource_mut::<CellHooks>().0);
    let slot = &mut hooks[i];
    let built = slot.built.as_mut().and_then(|b| b.get_mut().ok());
    let done = (slot.loader.step)(world, built, slot.step);
    slot.step += 1;
    restore(world, hooks);
    // Resources are entities too (bevy 0.20); one a spawn inserts stays when the cell goes.
    let new: Vec<Entity> = world
        .query_filtered::<Entity, (Without<ChildOf>, Without<IsResource>)>()
        .iter(world)
        .filter(|e| !before.has(*e))
        .collect();
    for e in new {
        world.entity_mut(e).insert(CellMember(id));
    }
    world.resource_mut::<CellHooks>().0[i].secs += t.elapsed().as_secs_f32();
    done
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
    let entities = tree(world, &roots).len() as u32;
    let ram = process_ram_mb().zip(ram0).map(|(b, a)| (b - a) as f32);
    if let Some(fade) = world
        .resource_mut::<Fades>()
        .0
        .iter_mut()
        .find(|f| f.cell == id)
    {
        fade.loaded = true;
    }
    let mut cells = world.resource_mut::<StreamedCells>();
    let c = &mut cells.cells[i];
    c.state = CellState::Loaded;
    c.load_secs = secs;
    c.entities = entities;
    c.ram_mb = ram;
    let built = if c.build_secs > 0.0 {
        format!(", built in {:.2} s on a worker", c.build_secs)
    } else {
        String::new()
    };
    let ram = ram.map_or("n/a".to_string(), |mb| format!("{mb:+.0} MB"));
    engine_core::log::info!(
        "streaming: loaded {} in {secs:.2} s ({steps} steps{built}), {entities} entities, RAM {ram}",
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
            l.roots = Some(
                world
                    .query::<(Entity, &CellMember)>()
                    .iter(world)
                    .filter(|(_, m)| m.0 == l.cell)
                    .map(|(e, _)| e)
                    .collect(),
            );
        }
        let roots = l.roots.as_mut().expect("set above");
        while let Some(e) = roots.pop() {
            if let Ok(entity) = world.get_entity_mut(e) {
                entity.despawn();
            }
            if start.elapsed().as_secs_f32() * 1000.0 > budget.frame_ms {
                break;
            }
        }
        if !roots.is_empty() {
            return true;
        }
        unload(world, i);
        false
    });
    let mut res = world.resource_mut::<Leaves>();
    leaves.append(&mut res.0);
    res.0 = leaves;
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
    for e in members {
        if let Ok(entity) = world.get_entity_mut(e) {
            entity.despawn();
        }
    }
    world.resource_mut::<Fades>().0.retain(|f| f.cell != id);
    world.resource_mut::<Leaves>().0.retain(|l| l.cell != id);
    hlod::fade_in(world, id, 1.0);
    let mut hooks = std::mem::take(&mut world.resource_mut::<CellHooks>().0);
    hooks[i].reset();
    if let Some(unload) = hooks[i].loader.unload.as_mut() {
        unload(world);
    }
    restore(world, hooks);
    let mut cells = world.resource_mut::<StreamedCells>();
    cells.cells[i].state = CellState::Unloaded;
    engine_core::log::info!("streaming: unloaded {}", cells.cells[i].key);
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

    #[test]
    fn a_sliced_cell_spawns_a_slice_a_frame_and_can_leave_half_in() {
        let mut app = app();
        app.world.resource_mut::<StreamingBudget>().frame_ms = 0.0;
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
