//! Procedural grids: cells made on demand around the streaming sources, for content that is
//! generated rather than authored and too large to register up front (a planet's surface, an
//! orbit's debris field, a star system's belt).
//!
//! A [`StreamGrid`] is a runtime grid with a maker: as a source comes within a cell's load
//! distance, the grid asks the maker for that cell (`None`: nothing there) and registers it
//! as a [`StreamCell`] under the key `grid:<name>/<x>,<z>` (`<x>,<y>,<z>` for a vertical grid);
//! once every source is beyond the grid's retire distance and the cell is out, it is retired
//! and its slot reused, so a long trip keeps only the cells around it. A cell's key is the same
//! whenever it is made again, so what [`StreamGrid::remember`] keeps of it (destroyed rocks,
//! a moved crate) survives the trip.
//!
//! Unreal's World Partition grids are 2D and authored; space needs cubes in a frame that moves.
//! So a grid can be vertical (cube cells, an orbit's 10 km debris cells) and ride a
//! [`StreamFrame`](engine_world::StreamFrame): its coordinates are then the frame's, and its
//! cells keep their place on the frame however it moves (Star Citizen's object containers and
//! zones that move with their body). Spawns get a [`CellPlace`], where the cell stands in the
//! world now and how the grid is turned, so they never hold world points that a
//! [`WorldShift`](engine_world::shift::WorldShift) or the frame's motion made stale.
//!
//! API `stream.grids`.

use std::collections::HashMap;
use std::sync::Arc;

use engine_core::prelude::*;
use engine_world::shift::{FollowsWorldShift, WorldShift, WorldShiftAppExt};
use engine_world::{CellState, StreamFrame, StreamedCells};
use serde::Serialize;

use crate::stream::{StreamCell, StreamCellsExt, unstream_cell};

/// A grid's cells' spawn: `spawn(world, place, i)` for i = 0, 1, 2… (slices within the
/// streaming budget) until it returns true. [`once`] and [`in_steps`] make one.
pub type GridSpawn = Box<dyn FnMut(&mut World, &CellPlace, usize) -> bool + Send + Sync>;

/// What a grid asks for each cell a source comes near: its spawn, or `None` when the cell is
/// empty. Called on the main thread when the cell is first wanted, so keep it to a decision
/// (a hash of the cell); the spawn does the work.
pub type GridMaker = Arc<dyn Fn(&GridCell) -> Option<GridSpawn> + Send + Sync>;

/// A spawn that puts everything in at once.
pub fn once(mut spawn: impl FnMut(&mut World, &CellPlace) + Send + Sync + 'static) -> GridSpawn {
    Box::new(move |world, place, _| {
        spawn(world, place);
        true
    })
}

/// A spawn in slices: `step(world, place, i)` until it returns true.
pub fn in_steps(
    step: impl FnMut(&mut World, &CellPlace, usize) -> bool + Send + Sync + 'static,
) -> GridSpawn {
    Box::new(step)
}

/// A runtime grid whose cells a maker generates as sources come near.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct StreamGrid {
    pub name: String,
    /// Width of a cell, meters.
    pub cell_m: f64,
    /// A cell loads when a source is within this distance of its edge, meters.
    pub load_m: f64,
    /// Cube cells (x, y, z) instead of columns (x, z): space.
    pub vertical: bool,
    /// The [`StreamFrame`] whose space the grid's coordinates are in; `None` is the world's
    /// (following world shifts).
    pub frame: Option<String>,
    /// Cells retire once every source is this far from their edge (and they are out);
    /// 0 means `2 × load_m + cell_m`.
    pub retire_m: f64,
    /// Cells remember what happened in them ([`StreamCell::remember`]).
    pub remember: bool,
    /// Cells free their assets on unload ([`StreamCell::release_assets`]).
    pub release_assets: bool,
    /// Cells show far-view proxies while out (off by default: a generated cell has no disk
    /// cache, and its proxy is dropped when it retires).
    pub proxies: bool,
    /// Cells one source may reach at most; a grid finer than this for its load distance is
    /// skipped with a warning.
    pub max_cells_per_source: usize,
}

impl StreamGrid {
    pub fn new(name: impl Into<String>, cell_m: f64, load_m: f64) -> Self {
        Self {
            name: name.into(),
            cell_m: cell_m.max(1e-3),
            load_m: load_m.max(0.0),
            vertical: false,
            frame: None,
            retire_m: 0.0,
            remember: false,
            release_assets: false,
            proxies: false,
            max_cells_per_source: 4096,
        }
    }

    /// Cube cells, for space.
    pub fn vertical(mut self) -> Self {
        self.vertical = true;
        self
    }

    /// Coordinates in the [`StreamFrame`] named `frame`; cells ride it.
    pub fn on_frame(mut self, frame: impl Into<String>) -> Self {
        self.frame = Some(frame.into());
        self
    }

    pub fn retire_beyond(mut self, meters: f64) -> Self {
        self.retire_m = meters;
        self
    }

    pub fn remember(mut self) -> Self {
        self.remember = true;
        self
    }

    pub fn release_assets(mut self) -> Self {
        self.release_assets = true;
        self
    }

    pub fn with_proxies(mut self) -> Self {
        self.proxies = true;
        self
    }

    /// Distance from a cell's center to its corners.
    pub fn cell_radius(&self) -> f64 {
        self.cell_m
            * if self.vertical {
                0.75f64.sqrt()
            } else {
                0.5f64.sqrt()
            }
    }

    fn retire_distance(&self) -> f64 {
        if self.retire_m > 0.0 {
            self.retire_m
        } else {
            2.0 * self.load_m + self.cell_m
        }
    }

    /// The streaming key of `cell`.
    pub fn key(&self, cell: &GridCell) -> String {
        if self.vertical {
            format!("grid:{}/{},{},{}", self.name, cell.x, cell.y, cell.z)
        } else {
            format!("grid:{}/{},{}", self.name, cell.x, cell.z)
        }
    }

    /// The center of `cell` in the grid's coordinates.
    pub fn local_center(&self, cell: &GridCell) -> DVec3 {
        let y = if self.vertical {
            (cell.y as f64 + 0.5) * self.cell_m
        } else {
            0.0
        };
        DVec3::new(
            (cell.x as f64 + 0.5) * self.cell_m,
            y,
            (cell.z as f64 + 0.5) * self.cell_m,
        )
    }
}

/// One cell of a grid: its column, layer (0 on a flat grid) and row.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
pub struct GridCell {
    pub x: i64,
    pub y: i64,
    pub z: i64,
}

impl GridCell {
    /// A stable number for this cell and `salt`, to seed what it generates.
    pub fn seed(&self, salt: u64) -> u64 {
        let mut h = 0xcbf2_9ce4_8422_2325u64 ^ salt;
        for v in [self.x, self.y, self.z] {
            for b in v.to_le_bytes() {
                h = (h ^ u64::from(b)).wrapping_mul(0x0100_0000_01b3);
            }
        }
        // FNV alone leaves the low bits of near cells alike; finish with a mixer.
        h ^= h >> 33;
        h = h.wrapping_mul(0xff51_afd7_ed55_8ccd);
        h ^ (h >> 33)
    }
}

/// Where a grid cell stands in the world as it spawns.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CellPlace {
    pub cell: GridCell,
    /// The cell's center, world meters.
    pub center: DVec3,
    /// The grid's axes in the world (its frame's turn, or the world shifts' turn).
    pub rotation: DQuat,
    /// Width of the cell, meters.
    pub size: f64,
    /// The entity of the grid's [`StreamFrame`], for a framed grid.
    pub frame: Option<Entity>,
    /// The cell's center in the grid's coordinates (the frame's space, for a framed grid).
    pub local_center: DVec3,
}

impl CellPlace {
    /// The world point `local` meters from the cell's center along the grid's axes.
    pub fn point(&self, local: DVec3) -> DVec3 {
        self.center + self.rotation * local
    }

    /// `local`, placed from the cell's center in the grid's axes, into the world.
    pub fn transform(&self, local: Transform) -> Transform {
        Transform {
            translation: self.point(local.translation),
            rotation: self.rotation.as_quat() * local.rotation,
            scale: local.scale,
        }
    }

    /// `local`, placed from the cell's center, in the frame's space: the transform of an
    /// entity made a child of [`CellPlace::frame`], so it rides the frame by itself.
    pub fn in_frame(&self, local: Transform) -> Transform {
        Transform {
            translation: self.local_center + local.translation,
            ..local
        }
    }
}

struct Grid {
    spec: StreamGrid,
    make: GridMaker,
    /// Unframed grids: where the grid's origin and axes are in the world now (they follow
    /// world shifts). Framed: the frame's pose when last seen.
    origin: DVec3,
    turn: DQuat,
    frame: Option<Entity>,
    /// Cells asked for so far and whether they made a cell.
    known: HashMap<GridCell, bool>,
    frames: u64,
    made: u64,
    retired: u64,
    warned: bool,
}

/// The procedural grids ([`StreamGridsExt::stream_grid`]).
#[derive(Resource, Default)]
pub struct StreamGrids {
    grids: Vec<Grid>,
}

impl FollowsWorldShift for StreamGrids {
    fn follow(&mut self, s: &WorldShift) {
        for g in self.grids.iter_mut().filter(|g| g.spec.frame.is_none()) {
            g.origin = s.point(g.origin);
            g.turn = s.rotation * g.turn;
        }
    }
}

/// One grid's numbers, for `stream.grids`.
#[derive(Debug, Clone, Serialize)]
pub struct GridReport {
    #[serde(flatten)]
    pub spec: StreamGrid,
    /// Cells asked for and still known (with and without content).
    pub known: usize,
    /// Of those, cells with content (registered for streaming).
    pub cells: usize,
    /// Cells made and retired since the grid was added.
    pub made: u64,
    pub retired: u64,
}

impl StreamGrids {
    pub fn report(&self) -> Vec<GridReport> {
        self.grids
            .iter()
            .map(|g| GridReport {
                spec: g.spec.clone(),
                known: g.known.len(),
                cells: g.known.values().filter(|&&has| has).count(),
                made: g.made,
                retired: g.retired,
            })
            .collect()
    }
}

/// Adds procedural grids.
pub trait StreamGridsExt {
    /// Stream `grid`, asking `make` for each cell a source comes near. A grid of the same name
    /// is replaced (its cells unload now).
    fn stream_grid(
        &mut self,
        grid: StreamGrid,
        make: impl Fn(&GridCell) -> Option<GridSpawn> + Send + Sync + 'static,
    ) -> &mut Self;
}

impl StreamGridsExt for World {
    fn stream_grid(
        &mut self,
        grid: StreamGrid,
        make: impl Fn(&GridCell) -> Option<GridSpawn> + Send + Sync + 'static,
    ) -> &mut Self {
        self.init_resource::<StreamGrids>();
        unstream_grid(self, &grid.name);
        self.resource_mut::<StreamGrids>().grids.push(Grid {
            spec: grid,
            make: Arc::new(make),
            origin: DVec3::ZERO,
            turn: DQuat::IDENTITY,
            frame: None,
            known: HashMap::new(),
            frames: 0,
            made: 0,
            retired: 0,
            warned: false,
        });
        self
    }
}

impl StreamGridsExt for App {
    fn stream_grid(
        &mut self,
        grid: StreamGrid,
        make: impl Fn(&GridCell) -> Option<GridSpawn> + Send + Sync + 'static,
    ) -> &mut Self {
        self.world.stream_grid(grid, make);
        self
    }
}

/// Takes the grid `name` down: its cells unload now and retire. False if there is none.
pub fn unstream_grid(world: &mut World, name: &str) -> bool {
    let Some(mut grids) = world.get_resource_mut::<StreamGrids>() else {
        return false;
    };
    let Some(i) = grids.grids.iter().position(|g| g.spec.name == name) else {
        return false;
    };
    let grid = grids.grids.remove(i);
    for (cell, has) in &grid.known {
        if *has {
            unstream_cell(world, &grid.spec.key(cell));
        }
    }
    true
}

/// Where the cell `key` of grid `name` stands now.
fn place_of(world: &World, name: &str, key: &str, cell: GridCell, size: f64) -> CellPlace {
    let center = world
        .get_resource::<StreamedCells>()
        .and_then(|c| c.find(key).and_then(|id| c.get(id)).map(|c| c.center))
        .unwrap_or_default();
    let grid = world
        .get_resource::<StreamGrids>()
        .and_then(|g| g.grids.iter().find(|g| g.spec.name == name));
    CellPlace {
        cell,
        center,
        rotation: grid.map_or(DQuat::IDENTITY, |g| g.turn),
        size,
        frame: grid.and_then(|g| g.frame),
        local_center: grid.map_or(DVec3::ZERO, |g| g.spec.local_center(&cell)),
    }
}

fn register(world: &mut World, spec: &StreamGrid, cell: GridCell, center: DVec3, spawn: GridSpawn) {
    let key = spec.key(&cell);
    let name = spec.name.clone();
    let size = spec.cell_m;
    let mut spawn = spawn;
    let own = key.clone();
    let mut stream = StreamCell::in_steps(key, center, spec.cell_radius(), move |world, i| {
        let place = place_of(world, &name, &own, cell, size);
        spawn(world, &place, i)
    })
    .load_within(spec.load_m);
    if let Some(frame) = &spec.frame {
        stream = stream.on_frame(frame.clone());
    }
    if spec.remember {
        stream = stream.remember();
    }
    if spec.release_assets {
        stream = stream.release_assets();
    }
    if !spec.proxies {
        stream = stream.without_proxy();
    }
    world.stream_cell(stream);
}

/// Frames between retire sweeps.
const RETIRE_EVERY: u64 = 16;

/// Makes the cells sources came near and retires those far behind (first thing in
/// [`crate::stream::stream_system`]).
pub(crate) fn update(world: &mut World) {
    let Some(mut res) = world.get_resource_mut::<StreamGrids>() else {
        return;
    };
    if res.grids.is_empty() {
        return;
    }
    let mut grids = std::mem::take(&mut res.grids);
    let points = crate::stream::source_points(world);
    let frames: Vec<(String, DVec3, DQuat, Entity)> =
        if grids.iter().any(|g| g.spec.frame.is_some()) {
            world
                .query::<(Entity, &StreamFrame, &GlobalTransform)>()
                .iter(world)
                .map(|(e, f, g)| {
                    let (_, rotation, translation) = g.0.to_scale_rotation_translation();
                    (f.0.clone(), translation, rotation, e)
                })
                .collect()
        } else {
            Vec::new()
        };
    for g in &mut grids {
        g.frames += 1;
        if let Some(name) = &g.spec.frame {
            let Some((_, at, turn, e)) = frames.iter().find(|f| &f.0 == name) else {
                continue;
            };
            g.origin = *at;
            g.turn = *turn;
            g.frame = Some(*e);
        }
        // No source: nothing to stream around (an endless grid can't load everything).
        let Some(points) = &points else {
            continue;
        };
        let back = g.turn.inverse();
        let local: Vec<(DVec3, f64)> = points
            .iter()
            .map(|(p, scale)| (back * (*p - g.origin), *scale))
            .collect();
        make_near(world, g, &local);
        if g.frames % RETIRE_EVERY == 0 {
            retire_far(world, g, &local);
        }
    }
    let mut res = world.resource_mut::<StreamGrids>();
    grids.append(&mut res.grids);
    res.grids = grids;
}

/// Asks the maker for every cell within load distance of a source not asked yet.
fn make_near(world: &mut World, g: &mut Grid, local: &[(DVec3, f64)]) {
    let s = g.spec.cell_m;
    let r = g.spec.cell_radius();
    for &(q, scale) in local {
        let reach = g.spec.load_m * scale;
        let lo = ((q - DVec3::splat(reach + r)) / s).floor();
        let hi = ((q + DVec3::splat(reach + r)) / s).floor();
        let (y0, y1) = if g.spec.vertical {
            (lo.y as i64, hi.y as i64)
        } else {
            (0, 0)
        };
        let span = |a: f64, b: f64| (b - a + 1.0).max(0.0);
        let count = span(lo.x, hi.x) * span(lo.z, hi.z) * (y1 - y0 + 1) as f64;
        if !count.is_finite() || count > g.spec.max_cells_per_source as f64 {
            if !std::mem::replace(&mut g.warned, true) {
                engine_core::log::warn!(
                    "stream grid {}: a source reaches {count} cells (over {}); raise cell_m or \
                     lower load_m",
                    g.spec.name,
                    g.spec.max_cells_per_source
                );
            }
            continue;
        }
        for x in lo.x as i64..=hi.x as i64 {
            for z in lo.z as i64..=hi.z as i64 {
                for y in y0..=y1 {
                    let cell = GridCell { x, y, z };
                    if g.known.contains_key(&cell) {
                        continue;
                    }
                    let c = g.spec.local_center(&cell);
                    if c.distance(q) - r > reach {
                        continue;
                    }
                    let made = (g.make)(&cell);
                    g.known.insert(cell, made.is_some());
                    if let Some(spawn) = made {
                        g.made += 1;
                        register(world, &g.spec, cell, g.origin + g.turn * c, spawn);
                    }
                }
            }
        }
    }
}

/// Retires the cells every source left behind by the retire distance, once they are out.
fn retire_far(world: &mut World, g: &mut Grid, local: &[(DVec3, f64)]) {
    let r = g.spec.cell_radius();
    let far = g.spec.retire_distance();
    let behind: Vec<(GridCell, bool)> = g
        .known
        .iter()
        .filter(|(cell, _)| {
            let c = g.spec.local_center(cell);
            local
                .iter()
                .all(|&(q, scale)| c.distance(q) - r > far * scale)
        })
        .map(|(c, has)| (*c, *has))
        .collect();
    for (cell, has) in behind {
        if has {
            let key = g.spec.key(&cell);
            let out = world
                .get_resource::<StreamedCells>()
                .and_then(|c| c.find(&key).and_then(|id| c.get(id)).map(|c| c.state))
                .is_none_or(|s| s == CellState::Unloaded);
            if !out {
                continue;
            }
            unstream_cell(world, &key);
            g.retired += 1;
        }
        g.known.remove(&cell);
    }
}

pub(crate) fn register_plugin(app: &mut App) {
    app.init_resource::<StreamGrids>()
        .shift_resource::<StreamGrids>()
        .register_api(
            "stream.grids",
            "List the procedural streaming grids: name, cell_m, load_m, vertical, frame, \
             retire_m and options, the cells known around the sources (with and without \
             content), and how many were made and retired so far.",
            "{}",
            |world, _| {
                let grids = world.resource::<StreamGrids>().report();
                serde_json::to_value(grids).map_err(|e| e.to_string())
            },
        );
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::stream::{StreamingBudget, StreamingPlugin};
    use engine_world::{CellMember, StreamingSource};

    #[derive(Component)]
    struct Rock(GridCell);

    fn app() -> App {
        let mut app = App::new();
        app.add_plugins((engine_world::WorldPlugin, StreamingPlugin));
        let mut budget = app.world.resource_mut::<StreamingBudget>();
        budget.fade_s = 0.0;
        budget.loads_per_frame = 64;
        app
    }

    /// A rock at the center of every cell but those whose x + z is a multiple of 3.
    fn rocky(grid: StreamGrid) -> impl FnOnce(&mut App) {
        move |app: &mut App| {
            app.world.stream_grid(grid, |cell| {
                ((cell.x + cell.z).rem_euclid(3) != 0).then(|| {
                    once(|world, place| {
                        world.spawn((
                            Name::new("rock"),
                            Rock(place.cell),
                            Transform::from_translation(place.point(DVec3::new(0.0, 1.0, 0.0))),
                        ));
                    })
                })
            });
        }
    }

    fn source(app: &mut App, at: DVec3) -> Entity {
        let t = Transform::from_translation(at);
        app.world
            .spawn((
                t,
                GlobalTransform::from(t),
                StreamingSource {
                    lookahead_s: 0.0,
                    ..Default::default()
                },
            ))
            .id()
    }

    fn go(app: &mut App, e: Entity, at: DVec3) {
        let t = Transform::from_translation(at);
        app.world
            .entity_mut(e)
            .insert((t, GlobalTransform::from(t)));
        settle(app);
    }

    fn settle(app: &mut App) {
        for _ in 0..(RETIRE_EVERY as usize * 2 + 4) {
            app.update();
        }
    }

    fn rocks(app: &mut App) -> Vec<(GridCell, DVec3)> {
        app.world
            .query::<(&Rock, &Transform)>()
            .iter(&app.world)
            .map(|(r, t)| (r.0, t.translation))
            .collect()
    }

    fn report(app: &App) -> GridReport {
        app.world.resource::<StreamGrids>().report().remove(0)
    }

    fn live_slots(app: &App) -> (usize, usize) {
        let cells = &app.world.resource::<StreamedCells>().cells;
        (cells.iter().filter(|c| !c.retired).count(), cells.len())
    }

    #[test]
    fn a_grid_makes_cells_around_the_source_and_retires_them_behind() {
        let mut app = app();
        let player = source(&mut app, DVec3::ZERO);
        rocky(StreamGrid::new("rocks", 100.0, 100.0))(&mut app);
        settle(&mut app);
        let here = rocks(&mut app);
        let r = StreamGrid::new("rocks", 100.0, 100.0).cell_radius();
        assert!(here.len() > 4, "{} rocks", here.len());
        assert!(here.iter().all(|(_, p)| p.length() < 100.0 + 2.0 * r));
        assert!(here.iter().all(|(c, _)| (c.x + c.z).rem_euclid(3) != 0));
        let keys: Vec<String> = app
            .world
            .resource::<StreamedCells>()
            .cells
            .iter()
            .map(|c| c.key.clone())
            .collect();
        assert!(keys.contains(&"grid:rocks/0,1".to_string()), "{keys:?}");
        let around = live_slots(&app).0;
        // A 20 km walk: cells come and go, slots are reused, so the table stays small.
        for step in 1..=100 {
            go(&mut app, player, DVec3::new(step as f64 * 200.0, 0.0, 0.0));
        }
        let now = rocks(&mut app);
        assert!(
            now.iter().all(|(_, p)| (p.x - 20_000.0).abs() < 400.0),
            "{now:?}"
        );
        let (live, slots) = live_slots(&app);
        assert!(
            live <= around * 3,
            "{live} live cells, {around} around one place"
        );
        assert!(
            slots <= around * 4,
            "{slots} slots for {around} cells around one place"
        );
        let g = report(&app);
        assert!(g.made > 100 && g.retired > 80, "{g:?}");
        assert!(g.known < 200, "{g:?}");
    }

    #[test]
    fn a_remembering_grid_keeps_what_was_destroyed_across_a_trip() {
        let mut app = app();
        let player = source(&mut app, DVec3::ZERO);
        rocky(StreamGrid::new("rocks", 100.0, 100.0).remember())(&mut app);
        settle(&mut app);
        let before = rocks(&mut app).len();
        let gone = app
            .world
            .query::<(Entity, &Rock)>()
            .iter(&app.world)
            .find(|(_, r)| r.0 == GridCell { x: 0, y: 0, z: 1 })
            .map(|(e, _)| e)
            .expect("a rock in cell 0,1");
        app.world.entity_mut(gone).despawn();
        go(&mut app, player, DVec3::new(5000.0, 0.0, 0.0));
        assert!(report(&app).retired > 0);
        go(&mut app, player, DVec3::ZERO);
        let after = rocks(&mut app);
        assert_eq!(after.len(), before - 1);
        assert!(
            !after
                .iter()
                .any(|(c, _)| *c == GridCell { x: 0, y: 0, z: 1 })
        );
    }

    #[test]
    fn a_vertical_grid_streams_cubes_around_a_source_in_the_sky() {
        let mut app = app();
        source(&mut app, DVec3::new(0.0, 5000.0, 0.0));
        rocky(StreamGrid::new("debris", 1000.0, 1000.0).vertical())(&mut app);
        settle(&mut app);
        let here = rocks(&mut app);
        assert!(!here.is_empty());
        assert!(
            here.iter()
                .all(|(c, p)| c.y >= 3 && c.y <= 6 && p.y > 3000.0),
            "{here:?}"
        );
        let keys = &app.world.resource::<StreamedCells>().cells;
        assert!(keys.iter().any(|c| c.key == "grid:debris/1,5,0"));
    }

    #[test]
    fn a_framed_grid_rides_its_frame() {
        let mut app = app();
        let moon_at = |x: f64| {
            let t = Transform::from_translation(DVec3::new(x, 0.0, 0.0));
            (t, GlobalTransform::from(t))
        };
        let moon = app
            .world
            .spawn((StreamFrame("moon".into()), moon_at(10_000.0)))
            .id();
        let ship = source(&mut app, DVec3::new(10_000.0, 0.0, 0.0));
        rocky(StreamGrid::new("belt", 100.0, 100.0).on_frame("moon"))(&mut app);
        settle(&mut app);
        let here = rocks(&mut app);
        assert!(!here.is_empty());
        // Cell 0,0 is the moon's, 50 m from its origin on x and z.
        let (_, p) = here
            .iter()
            .find(|(c, _)| *c == GridCell { x: 0, y: 0, z: 1 })
            .expect("cell 0,1");
        assert!(p.distance(DVec3::new(10_050.0, 1.0, 150.0)) < 1e-6, "{p}");
        let made = report(&app).made;
        // The moon and the ship move on together: the same cells, carried along.
        app.world.entity_mut(moon).insert(moon_at(12_000.0));
        go(&mut app, ship, DVec3::new(12_000.0, 0.0, 0.0));
        assert_eq!(report(&app).made, made);
        let cells = &app.world.resource::<StreamedCells>().cells;
        let c = cells.iter().find(|c| c.key == "grid:belt/0,1").unwrap();
        assert!(c.center.distance(DVec3::new(12_050.0, 0.0, 150.0)) < 1e-6);
    }

    #[test]
    fn content_hung_on_the_frame_rides_it_and_goes_with_its_cell() {
        let mut app = app();
        let moon_pose = |x: f64, deg: f32| {
            let t = Transform::from_translation(DVec3::new(x, 0.0, 0.0))
                .with_rotation(Quat::from_rotation_y(deg.to_radians()));
            (t, GlobalTransform::from(t))
        };
        let moon = app
            .world
            .spawn((StreamFrame("moon".into()), moon_pose(5000.0, 0.0)))
            .id();
        // The ship is parked on the moon, as space parks things on their body.
        app.world.spawn((
            Transform::default(),
            GlobalTransform::default(),
            StreamingSource {
                lookahead_s: 0.0,
                ..Default::default()
            },
            ChildOf(moon),
        ));
        app.world.stream_grid(
            StreamGrid::new("belt", 100.0, 100.0)
                .on_frame("moon")
                .remember(),
            |cell| {
                ((cell.x + cell.z).rem_euclid(3) != 0).then(|| {
                    once(|world, place| {
                        let frame = place.frame.expect("a framed grid's place has its frame");
                        world.spawn((
                            Rock(place.cell),
                            place.in_frame(Transform::default()),
                            ChildOf(frame),
                        ));
                    })
                })
            },
        );
        settle(&mut app);
        let local: Vec<(GridCell, DVec3)> = rocks(&mut app);
        assert!(!local.is_empty());
        for (cell, at) in &local {
            let c = StreamGrid::new("belt", 100.0, 100.0).local_center(cell);
            assert!(at.distance(c) < 1e-9, "{cell:?} at {at}");
        }
        let tops = app
            .world
            .query_filtered::<&CellMember, With<Rock>>()
            .iter(&app.world)
            .count();
        assert_eq!(
            tops,
            local.len(),
            "every rock hung on the moon is its cell's"
        );
        let made = report(&app).made;
        // The moon moves 2 km and turns 30°: the ship and the rocks ride it, nothing is remade.
        app.world.entity_mut(moon).insert(moon_pose(7000.0, 30.0));
        settle(&mut app);
        assert_eq!((report(&app).made, report(&app).retired), (made, 0));
        assert_eq!(rocks(&mut app), local);
        unstream_grid(&mut app.world, "belt");
        assert!(rocks(&mut app).is_empty(), "the grid's rocks go with it");
    }

    #[test]
    fn world_shifts_move_the_grid_not_its_cells_keys() {
        let mut app = app();
        let player = source(&mut app, DVec3::ZERO);
        rocky(StreamGrid::new("rocks", 100.0, 100.0))(&mut app);
        settle(&mut app);
        let made = report(&app).made;
        app.world
            .write_message(WorldShift::translate(DVec3::new(-1000.0, 0.0, 0.0)));
        app.update();
        settle(&mut app);
        assert_eq!(report(&app).made, made, "a shift made new cells");
        // The player walks on 300 m in the shifted world; new cells line up with the old.
        let at = app.world.get::<Transform>(player).unwrap().translation;
        assert!(at.distance(DVec3::new(-1000.0, 0.0, 0.0)) < 1e-6, "{at}");
        go(&mut app, player, DVec3::new(-700.0, 0.0, 0.0));
        let (_, p) = rocks(&mut app)
            .into_iter()
            .find(|(c, _)| *c == GridCell { x: 3, y: 0, z: 1 })
            .expect("cell 3,1");
        assert!(p.distance(DVec3::new(-650.0, 1.0, 150.0)) < 1e-6, "{p}");
    }
}
