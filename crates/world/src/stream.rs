//! Streaming: which parts of the world exist around the player.
//!
//! A big world is split into cells (a showcase zone, a terrain tile, a station in orbit). Each
//! cell loads when a [`StreamingSource`] comes within its load distance and unloads when every
//! source is past it by a margin, so memory and frame time go to what the player can reach,
//! as Unreal's World Partition does with runtime cells and streaming sources, and Unity's
//! entity scenes with sections. The engine's `stream` plugin runs the loading; this module is
//! the part everyone may read: [`StreamedCells`] (each cell's place and state, for loading
//! screens, teleports and tests) and [`CellMember`] on what a cell spawned.
//!
//! Cells keep world points, so they follow [`crate::WorldShift`] and [`crate::SurfaceMoved`].
//! A cell on something that moves through the world (a planet's surface while the world is
//! anchored to its moon, a station in orbit) names that thing's [`StreamFrame`] and rides with
//! it, as Star Citizen's object containers nest content in the frame it moves with.

use std::collections::BTreeMap;

use engine_core::math::DVec3;
use engine_core::prelude::*;

use crate::shift::{FollowsWorldShift, WorldShift};

/// Something that pulls cells in around itself: the player, the active camera, a fast ship.
/// Without any, every [`crate::SimulationFocus`] is a source with the default settings.
#[derive(Component, Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct StreamingSource {
    /// Scales every cell's load distance for this source (a ship's sensors reach farther).
    pub range_scale: f64,
    /// Seconds ahead along its velocity it also loads around, so a fast source finds cells
    /// ready when it arrives.
    pub lookahead_s: f64,
}

impl Default for StreamingSource {
    fn default() -> Self {
        Self {
            range_scale: 1.0,
            lookahead_s: 2.0,
        }
    }
}

/// On an entity whose `Transform` is a frame streamed cells can live in: a planet or a moon
/// (the space module puts one on each body, named after it), a station, a carrier. A cell
/// registered on the frame's `name` keeps its place relative to it however the frame moves
/// and turns in the world, and its proxy goes along.
#[derive(Component, Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StreamFrame(pub String);

/// How an entity of a scene is streamed when the scene is partitioned into cells (the engine's
/// `scene_stream`), as Unreal's actors carry "Is Spatially Loaded" and a runtime grid. Without
/// one, a root with a `Transform` goes to the default grid's cell under it, and a root without
/// one (sky, settings, global lights) is always loaded.
#[derive(Component, Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct StreamPlacement {
    /// Loads with the cell it stands in (false: always loaded, with the scene).
    pub spatial: bool,
    /// The runtime grid it belongs to (a grid has its own cell size and load distance:
    /// small props in a fine near grid, landmarks in a coarse far one); empty: the first grid.
    pub grid: String,
    /// How far it reaches from its origin, meters. Something bigger than a cell goes to a
    /// coarser level of its grid, whose cells are 2, 4, 8… times as wide.
    pub radius: f32,
}

impl Default for StreamPlacement {
    fn default() -> Self {
        Self {
            spatial: true,
            grid: String::new(),
            radius: 0.0,
        }
    }
}

/// Index of a cell in [`StreamedCells`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct CellId(pub u32);

/// On every entity a cell spawned (roots; their children go with them). Unloading the cell
/// despawns them.
#[derive(Component, Debug, Clone, Copy, PartialEq, Eq)]
pub struct CellMember(pub CellId);

/// Where a cell is in its life: `Unloaded`, then (wanted) `Building` off the main thread when it
/// has a build step, `Queued` for its turn in the frame budget, `Loading` while it spawns over
/// several frames, and `Loaded`; on the way out, `Unloading` while it fades and despawns.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum CellState {
    #[default]
    Unloaded,
    /// Wanted; its world-free build step runs on a worker thread.
    Building,
    /// Wanted; waits for its turn in the load budget.
    Queued,
    /// Spawning, a step a frame (or more, within the budget).
    Loading,
    Loaded,
    /// Leaving: its meshes fade out (its far-view proxy in), then it despawns over a few
    /// frames within the budget.
    Unloading,
}

impl CellState {
    /// Wanted but not in yet: building, queued or loading. Loading screens wait for these.
    pub fn is_waking(self) -> bool {
        matches!(self, Self::Building | Self::Queued | Self::Loading)
    }
}

/// One cell: a sphere of the world that loads and unloads as a whole.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CellInfo {
    pub key: String,
    /// Middle of what it spawns (world meters; follows shifts).
    pub center: DVec3,
    /// How far its content reaches from `center`, meters.
    pub radius: f64,
    /// It loads when a source is this close to its edge, meters.
    pub load_m: f64,
    /// Never unloads once loaded (content that can't be taken down yet).
    pub keep_loaded: bool,
    pub state: CellState,
    /// Seconds spent spawning it the last time it loaded (main thread; its build step is
    /// `build_secs`).
    pub load_secs: f32,
    /// Seconds its build step took on a worker the last time it loaded.
    pub build_secs: f32,
    /// Entities it spawned the last time it loaded (roots and their descendants).
    pub entities: u32,
    /// How much the process RAM grew while it loaded, MB (other threads' allocations blur it).
    pub ram_mb: Option<f32>,
    /// Assets its last load added (meshes, materials and images it made, not shared keyed
    /// ones): how many, and their heap MB.
    pub assets: u32,
    pub assets_mb: f32,
    /// The cell frees those assets when it unloads (unless something still draws them).
    pub releases_assets: bool,
    /// The [`StreamFrame`] it rides with (`None`: it stays put in the world, following only
    /// shifts). `center` is where the frame puts it now.
    pub frame: Option<String>,
    /// Taken out of streaming (its scene was unloaded or replaced): never loads again, and
    /// readiness checks skip it. Registering a cell with its key brings the slot back.
    #[serde(default)]
    pub retired: bool,
}

impl CellInfo {
    /// Distance from `p` to the cell's edge (0 inside).
    pub fn distance(&self, p: DVec3) -> f64 {
        (self.center.distance(p) - self.radius).max(0.0)
    }
}

/// Every streamed cell and its state, written by the engine's streaming plugin.
#[derive(Resource, Debug, Clone, Default)]
pub struct StreamedCells {
    pub cells: Vec<CellInfo>,
    /// Unload a cell once every source is farther than `load_m × (1 + hysteresis)`.
    pub hysteresis: f64,
    /// Cells held in or out whatever the sources say, by key: `true` keeps one loaded,
    /// `false` keeps it unloaded (even a kept one). For tools, the editor and audits.
    pub pins: BTreeMap<String, bool>,
}

impl StreamedCells {
    /// Every cell that reaches within `radius` of `p` is loaded (true where there are none).
    /// A loading screen or a teleport waits for this before it lets the player in.
    pub fn is_ready_around(&self, p: DVec3, radius: f64) -> bool {
        self.cells
            .iter()
            .filter(|c| !c.retired && c.distance(p) <= radius)
            .all(|c| c.state == CellState::Loaded)
    }

    /// The cells wanted but not in yet ([`CellState::is_waking`]).
    pub fn waking(&self) -> impl Iterator<Item = &CellInfo> {
        self.cells.iter().filter(|c| c.state.is_waking())
    }

    pub fn find(&self, key: &str) -> Option<CellId> {
        self.cells
            .iter()
            .position(|c| c.key == key)
            .map(|i| CellId(i as u32))
    }

    pub fn get(&self, id: CellId) -> Option<&CellInfo> {
        self.cells.get(id.0 as usize)
    }
}

impl FollowsWorldShift for StreamedCells {
    fn follow(&mut self, s: &WorldShift) {
        for c in &mut self.cells {
            c.center = s.point(c.center);
        }
    }
}

pub(crate) fn register(app: &mut App) {
    use crate::shift::WorldShiftAppExt;
    app.insert_resource(StreamedCells {
        hysteresis: 0.25,
        ..Default::default()
    })
    .shift_resource::<StreamedCells>()
    .register_component::<StreamingSource>(
        "Pulls streamed cells in around itself: range_scale (times each cell's load distance), \
         lookahead_s (also loads ahead along its velocity). Without any, the simulation foci are \
         sources.",
    )
    .register_component::<StreamPlacement>(
        "How a scene entity streams when its scene is partitioned into cells: spatial (false: \
         always loaded), grid (runtime grid name, empty = the first), radius (meters it reaches; \
         bigger than a cell goes to a coarser grid level).",
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ready_only_when_every_nearby_cell_is_in() {
        let cell = |key: &str, x: f64, state| CellInfo {
            key: key.into(),
            center: DVec3::new(x, 0.0, 0.0),
            radius: 10.0,
            load_m: 50.0,
            keep_loaded: false,
            state,
            load_secs: 0.0,
            build_secs: 0.0,
            entities: 0,
            ram_mb: None,
            assets: 0,
            assets_mb: 0.0,
            releases_assets: false,
            frame: None,
            retired: false,
        };
        let mut cells = StreamedCells {
            cells: vec![
                cell("near", 0.0, CellState::Loaded),
                cell("far", 500.0, CellState::Unloaded),
            ],
            hysteresis: 0.25,
            ..Default::default()
        };
        assert!(cells.is_ready_around(DVec3::ZERO, 100.0));
        assert!(!cells.is_ready_around(DVec3::new(450.0, 0.0, 0.0), 100.0));
        cells.cells[1].state = CellState::Building;
        assert_eq!(cells.waking().count(), 1);
        cells.cells[1].state = CellState::Loaded;
        assert!(cells.is_ready_around(DVec3::new(450.0, 0.0, 0.0), 100.0));
        cells.cells[1].state = CellState::Unloaded;
        cells.cells[1].retired = true;
        assert!(cells.is_ready_around(DVec3::new(450.0, 0.0, 0.0), 100.0));
        assert_eq!(cells.find("far"), Some(CellId(1)));
        assert_eq!(cells.cells[1].distance(DVec3::new(480.0, 0.0, 0.0)), 10.0);
    }
}
