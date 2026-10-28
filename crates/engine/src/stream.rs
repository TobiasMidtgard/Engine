//! Loads and unloads the world's cells around the streaming sources ([`engine_world::stream`]).
//!
//! A game registers each cell with [`StreamCellsExt::stream_cell`]: where it is, how far its
//! content reaches, when to load, and a spawn function. Every frame (`Last`) the plugin finds
//! the cells a source wants, spawns the nearest queued ones within a per-frame budget, and
//! despawns cells every source has left by the hysteresis margin, as Unreal's World Partition
//! loads runtime cells around its streaming sources. Entities a cell's spawn creates get
//! [`CellMember`], so its unload takes exactly them (with their children).

use std::collections::HashSet;
use std::time::Instant;

use engine_core::diagnostics::{Diagnostics, PROCESS_RAM_MB};
use engine_core::prelude::*;
use engine_world::physics::Velocity;
use engine_world::stream::CellInfo;
use engine_world::{
    CellId, CellMember, CellState, FocusPoints, SimulationFocus, StreamedCells, StreamingSource,
};

type Hook = Box<dyn FnMut(&mut World) + Send + Sync>;

/// A cell to stream: build with [`StreamCell::new`], register with
/// [`StreamCellsExt::stream_cell`].
pub struct StreamCell {
    info: CellInfo,
    spawn: Hook,
    unload: Option<Hook>,
}

impl StreamCell {
    /// A cell named `key` whose content lies within `radius` of `center`, spawned by `spawn`
    /// (anything it spawns, directly or through commands it applies, belongs to the cell). It
    /// loads when a source is within 150 m of its edge.
    pub fn new(
        key: impl Into<String>,
        center: DVec3,
        radius: f64,
        spawn: impl FnMut(&mut World) + Send + Sync + 'static,
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
            },
            spawn: Box::new(spawn),
            unload: None,
        }
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

    /// Runs after the cell's entities are despawned, to drop what its module keeps elsewhere
    /// (a resource, a cache).
    pub fn on_unload(mut self, unload: impl FnMut(&mut World) + Send + Sync + 'static) -> Self {
        self.unload = Some(Box::new(unload));
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
            .push((cell.spawn, cell.unload));
        self
    }
}

impl StreamCellsExt for App {
    fn stream_cell(&mut self, cell: StreamCell) -> &mut Self {
        self.world.stream_cell(cell);
        self
    }
}

/// How much loading one frame may do, and the memory caps.
#[derive(Resource, Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(default)]
pub struct StreamingBudget {
    /// Cells spawned per frame at most.
    pub loads_per_frame: usize,
    /// No further cell starts once this frame's loading took this long, ms.
    pub frame_ms: f32,
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

/// The spawn and unload functions, in the order of [`StreamedCells::cells`].
#[derive(Resource, Default)]
struct CellHooks(Vec<(Hook, Option<Hook>)>);

/// Streams the registered cells (`Last`).
pub struct StreamingPlugin;

impl Plugin for StreamingPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<StreamedCells>()
            .init_resource::<CellHooks>()
            .init_resource::<StreamingBudget>()
            .add_systems(Last, stream_system)
            .register_api(
                "stream.cells",
                "List the streamed cells (key, center, radius, load distance, state, last load \
                 time), what is loaded, and the process RAM and GPU memory in MB.",
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
                "stream.budget",
                "Get or change the streaming budget: loads_per_frame, frame_ms, ram_mb and \
                 vram_mb (caps in MB; over one, cells no source needs unload early; null = no \
                 cap). Send fields to change; returns the budget.",
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

/// `Last`: queue, load and unload cells.
pub fn stream_system(world: &mut World) {
    if world.resource::<StreamedCells>().cells.is_empty() {
        return;
    }
    let sources = source_points(world);
    let budget = *world.resource::<StreamingBudget>();
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
    let mut queued = Vec::new();
    let mut leaving = Vec::new();
    let mut spare: Option<(f64, usize)> = None;
    for (i, c) in cells.cells.iter_mut().enumerate() {
        let r = reach(c);
        match c.state {
            CellState::Unloaded if r <= 1.0 => c.state = CellState::Queued,
            CellState::Queued if r > 1.0 + hysteresis => c.state = CellState::Unloaded,
            CellState::Loaded if r > 1.0 + hysteresis && !c.keep_loaded => leaving.push(i),
            CellState::Loaded if r > 1.0 && !c.keep_loaded && spare.is_none_or(|s| r > s.0) => {
                spare = Some((r, i));
            }
            _ => {}
        }
        if c.state == CellState::Queued {
            queued.push((r, i));
        }
    }
    let pressed = leaving.is_empty() && budget.over(world.resource::<Diagnostics>());
    if let Some((_, i)) = spare.filter(|_| pressed) {
        leaving.push(i);
    }
    for i in leaving {
        unload(world, i);
    }
    queued.sort_by(|a, b| a.0.total_cmp(&b.0));
    let start = Instant::now();
    for (_, i) in queued.into_iter().take(budget.loads_per_frame.max(1)) {
        if start.elapsed().as_secs_f32() * 1000.0 > budget.frame_ms {
            break;
        }
        load(world, i);
    }
}

fn load(world: &mut World, i: usize) {
    let t = Instant::now();
    let before: HashSet<Entity> = world.query::<Entity>().iter(world).collect();
    let mut hooks = std::mem::take(&mut world.resource_mut::<CellHooks>().0);
    (hooks[i].0)(world);
    restore(world, hooks);
    let id = CellId(i as u32);
    let new: Vec<Entity> = world
        .query_filtered::<Entity, Without<ChildOf>>()
        .iter(world)
        .filter(|e| !before.contains(e))
        .collect();
    for e in new {
        world.entity_mut(e).insert(CellMember(id));
    }
    let mut cells = world.resource_mut::<StreamedCells>();
    let c = &mut cells.cells[i];
    c.state = CellState::Loaded;
    c.load_secs = t.elapsed().as_secs_f32();
    engine_core::log::info!("streaming: loaded {} in {:.2} s", c.key, c.load_secs);
}

/// Puts the hooks back, after any cells the hook itself registered.
fn restore(world: &mut World, mut hooks: Vec<(Hook, Option<Hook>)>) {
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
    let mut hooks = std::mem::take(&mut world.resource_mut::<CellHooks>().0);
    if let Some(unload) = hooks[i].1.as_mut() {
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

    fn app() -> App {
        let mut app = App::new();
        app.add_plugins((engine_world::WorldPlugin, StreamingPlugin));
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
