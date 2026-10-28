//! Streams a scene document by itself: its roots are sorted into the cells of runtime grids by
//! where they stand, and each cell loads around the streaming sources ([`crate::stream`]).
//!
//! This is Unreal's World Partition for [`SceneDoc`]s: a level is one big document, no one
//! cuts it into sublevels by hand. Each root goes to the cell of its grid under its origin
//! (x and z, as Unreal's 2D runtime grids); a root that reaches farther than a cell goes to a
//! coarser level of the grid whose cells are 2, 4, 8… times as wide, so a castle loads as one
//! piece from farther away while the pebbles around it come and go in small cells. Roots the
//! document marks [`StreamPlacement`] `spatial: false`, and roots without a `Transform` (sky,
//! settings, global lights), load with the scene and stay, like Unreal's non-spatially loaded
//! actors.
//!
//! Every cell spawns its roots from the document again when it loads, a few a frame, so no
//! handle outlives it: cells free their meshes, materials and images on unload, and their
//! far-view proxies are kept on disk by a hash of their content (`proxy_version`). Streaming a
//! scene again under the same name (the editor saving it, a hot reload) replaces its cells.
//!
//! APIs: `scene.partition` (the cells a document would make, without loading anything),
//! `scene.stream` and `scene.unstream`.

use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;

use engine_core::prelude::*;
use engine_core::registry::TypeRegistry;
use engine_core::scene::{EntityDoc, SceneDoc, spawn_entity_doc};
use engine_world::StreamPlacement;

use crate::stream::{StreamCell, StreamCellsExt, unstream_cell};

/// One runtime grid: how wide its cells are and how far from a cell's edge a source loads it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct RuntimeGrid {
    pub name: String,
    pub cell_m: f64,
    pub load_m: f64,
}

impl Default for RuntimeGrid {
    /// Unreal's default main grid: 128 m cells loaded within 256 m.
    fn default() -> Self {
        Self {
            name: "main".into(),
            cell_m: 128.0,
            load_m: 256.0,
        }
    }
}

/// How a scene is cut into cells.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ScenePartition {
    /// The grids; a root names its own in [`StreamPlacement::grid`], the first is the default.
    pub grids: Vec<RuntimeGrid>,
    /// Roots a cell spawns per slice (several slices a frame, within the streaming budget).
    pub roots_per_step: usize,
    /// Cells free their assets on unload (they spawn from the document each time, so nothing
    /// keeps a handle).
    pub release_assets: bool,
    /// Cells show far-view proxies while out ([`crate::hlod`]).
    pub proxies: bool,
}

impl Default for ScenePartition {
    fn default() -> Self {
        Self {
            grids: vec![RuntimeGrid::default()],
            roots_per_step: 16,
            release_assets: true,
            proxies: true,
        }
    }
}

/// Coarsest grid level: cells 2^16 times as wide as the grid's (8,000 km for 128 m cells).
const MAX_LEVEL: u32 = 16;

/// One cell a scene makes.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PlannedCell {
    /// Its streaming key: `scene:<name>/<grid>/L<level>/<x>,<z>`.
    pub key: String,
    pub grid: String,
    /// 0 for the grid's own cells; level n cells are 2^n times as wide.
    pub level: u32,
    /// The cell's column and row in its level.
    pub x: i64,
    pub z: i64,
    /// Width of the cell, meters.
    pub size: f64,
    /// Middle of its roots' reach and how far it goes, meters (the streamed cell's sphere).
    pub center: DVec3,
    pub radius: f64,
    pub load_m: f64,
    /// Indices of its roots in the document's `entities`.
    pub roots: Vec<usize>,
}

/// What a scene makes: the roots that always load and the cells.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct SceneLayout {
    pub persistent: Vec<usize>,
    pub cells: Vec<PlannedCell>,
}

/// A cell's grid, level and column and row.
type CellAt = (usize, u32, i64, i64);
/// A root in a cell: its index, where it stands and how far it reaches.
type Placed = (usize, DVec3, f64);

/// Sorts `doc`'s roots into cells (nothing is spawned). Fails on a root whose `Transform` or
/// `StreamPlacement` doesn't read, or that names a grid the partition doesn't have.
pub fn partition(name: &str, doc: &SceneDoc, how: &ScenePartition) -> Result<SceneLayout, String> {
    if how.grids.is_empty() {
        return Err("the partition has no grids".into());
    }
    if let Some(g) = how
        .grids
        .iter()
        .find(|g| g.cell_m.is_nan() || g.cell_m <= 0.0)
    {
        return Err(format!("grid {:?}: cell_m must be above 0", g.name));
    }
    let mut layout = SceneLayout::default();
    let mut cells: BTreeMap<CellAt, Vec<Placed>> = BTreeMap::new();
    for (i, root) in doc.entities.iter().enumerate() {
        let label = || root.name.clone().unwrap_or_else(|| format!("root {i}"));
        let placement = match root.components.get("StreamPlacement") {
            Some(v) => serde_json::from_value::<StreamPlacement>(v.clone())
                .map_err(|e| format!("{}: StreamPlacement: {e}", label()))?,
            None => StreamPlacement::default(),
        };
        let at = match root.components.get("Transform") {
            Some(v) => {
                serde_json::from_value::<Transform>(v.clone())
                    .map_err(|e| format!("{}: Transform: {e}", label()))?
                    .translation
            }
            None => {
                layout.persistent.push(i);
                continue;
            }
        };
        if !placement.spatial {
            layout.persistent.push(i);
            continue;
        }
        let grid = if placement.grid.is_empty() {
            0
        } else {
            how.grids
                .iter()
                .position(|g| g.name == placement.grid)
                .ok_or_else(|| format!("{}: no grid {:?}", label(), placement.grid))?
        };
        let reach = f64::from(placement.radius.max(0.0));
        let base = how.grids[grid].cell_m;
        // The first level whose cells are as wide as what the root reaches across.
        let level = (0..MAX_LEVEL)
            .find(|&l| base * f64::from(1u32 << l) >= 2.0 * reach)
            .unwrap_or(MAX_LEVEL);
        let size = base * f64::from(1u32 << level);
        let (x, z) = ((at.x / size).floor() as i64, (at.z / size).floor() as i64);
        cells
            .entry((grid, level, x, z))
            .or_default()
            .push((i, at, reach));
    }
    for ((grid, level, x, z), roots) in cells {
        let g = &how.grids[grid];
        let size = g.cell_m * f64::from(1u32 << level);
        let (lo, hi) = roots.iter().fold(
            (DVec3::splat(f64::INFINITY), DVec3::splat(f64::NEG_INFINITY)),
            |(lo, hi), (_, p, r)| (lo.min(*p - *r), hi.max(*p + *r)),
        );
        let center = (lo + hi) * 0.5;
        let radius = roots
            .iter()
            .map(|(_, p, r)| p.distance(center) + r)
            .fold(0.0, f64::max)
            .max(1.0);
        layout.cells.push(PlannedCell {
            key: format!("scene:{name}/{}/L{level}/{x},{z}", g.name),
            grid: g.name.clone(),
            level,
            x,
            z,
            size,
            center,
            radius,
            load_m: g.load_m,
            roots: roots.iter().map(|r| r.0).collect(),
        });
    }
    Ok(layout)
}

/// The scenes streamed now, by name: their always-loaded roots and their cells' keys.
#[derive(Resource, Default)]
pub struct StreamedScenes(HashMap<String, StreamedScene>);

struct StreamedScene {
    persistent: Vec<Entity>,
    cells: Vec<String>,
}

impl StreamedScenes {
    /// The streamed scenes' names.
    pub fn names(&self) -> impl Iterator<Item = &str> {
        self.0.keys().map(String::as_str)
    }

    /// The keys of `name`'s cells.
    pub fn cells(&self, name: &str) -> Option<&[String]> {
        self.0.get(name).map(|s| s.cells.as_slice())
    }
}

/// Streams `doc` as the scene `name`: its persistent roots spawn now, its cells are registered
/// and load around the sources. A scene already streamed under `name` is replaced. Every
/// component the document names must be registered; if one isn't, nothing changes.
pub fn stream_scene(
    world: &mut World,
    name: &str,
    doc: &SceneDoc,
    how: &ScenePartition,
) -> Result<SceneLayout, String> {
    let layout = partition(name, doc, how)?;
    check_components(world, &doc.entities)?;
    unstream_scene(world, name);
    let mut persistent = Vec::new();
    for &i in &layout.persistent {
        match spawn_entity_doc(world, &doc.entities[i], None) {
            Ok(e) => persistent.push(e),
            Err(e) => {
                for e in persistent {
                    if let Ok(entity) = world.get_entity_mut(e) {
                        entity.despawn();
                    }
                }
                return Err(e);
            }
        }
    }
    let per_step = how.roots_per_step.max(1);
    for planned in &layout.cells {
        let roots: Arc<Vec<EntityDoc>> = Arc::new(
            planned
                .roots
                .iter()
                .map(|&i| doc.entities[i].clone())
                .collect(),
        );
        let version = content_hash(&roots);
        let key = planned.key.clone();
        let mut cell = StreamCell::in_steps(
            planned.key.clone(),
            planned.center,
            planned.radius,
            move |world, step| {
                let from = step * per_step;
                for root in roots.iter().skip(from).take(per_step) {
                    if let Err(e) = spawn_entity_doc(world, root, None) {
                        engine_core::log::error!("scene streaming: {key}: {e}");
                    }
                }
                from + per_step >= roots.len()
            },
        )
        .load_within(planned.load_m);
        cell = if how.proxies {
            cell.proxy_version(version)
        } else {
            cell.without_proxy()
        };
        if how.release_assets {
            cell = cell.release_assets();
        }
        world.stream_cell(cell);
    }
    engine_core::log::info!(
        "scene streaming: {name}: {} roots always in, {} in {} cells",
        layout.persistent.len(),
        layout.cells.iter().map(|c| c.roots.len()).sum::<usize>(),
        layout.cells.len()
    );
    world.init_resource::<StreamedScenes>();
    world.resource_mut::<StreamedScenes>().0.insert(
        name.to_string(),
        StreamedScene {
            persistent,
            cells: layout.cells.iter().map(|c| c.key.clone()).collect(),
        },
    );
    Ok(layout)
}

/// Takes the scene `name` down: its cells unload and retire, its persistent roots despawn.
/// False if no scene streams under that name.
pub fn unstream_scene(world: &mut World, name: &str) -> bool {
    let Some(scene) = world
        .get_resource_mut::<StreamedScenes>()
        .and_then(|mut s| s.0.remove(name))
    else {
        return false;
    };
    for key in &scene.cells {
        unstream_cell(world, key);
    }
    for e in scene.persistent {
        if let Ok(entity) = world.get_entity_mut(e) {
            entity.despawn();
        }
    }
    true
}

/// Every component the roots (and their children) name is registered.
fn check_components(world: &World, docs: &[EntityDoc]) -> Result<(), String> {
    let registry = world.resource::<TypeRegistry>();
    let mut stack: Vec<&EntityDoc> = docs.iter().collect();
    while let Some(doc) = stack.pop() {
        for component in doc.components.keys() {
            registry
                .component_or_err(component)
                .map_err(|e| match &doc.name {
                    Some(n) => format!("entity '{n}': {e}"),
                    None => e,
                })?;
        }
        stack.extend(doc.children.iter());
    }
    Ok(())
}

/// FNV-1a of the roots' JSON: the same content gives the same proxy on disk from run to run.
fn content_hash(roots: &[EntityDoc]) -> u64 {
    let text = serde_json::to_string(roots).unwrap_or_default();
    text.bytes().fold(0xcbf2_9ce4_8422_2325, |h, b| {
        (h ^ u64::from(b)).wrapping_mul(0x0100_0000_01b3)
    })
}

/// The scene from an API call's `scene` (a document) or `path` (a file).
fn scene_arg(args: &serde_json::Value) -> Result<SceneDoc, String> {
    match (args.get("scene"), args["path"].as_str()) {
        (Some(scene), _) => {
            serde_json::from_value(scene.clone()).map_err(|e| format!("bad scene: {e}"))
        }
        (None, Some(path)) => SceneDoc::load(path),
        (None, None) => Err("give 'scene' (a document) or 'path' (a scene file)".into()),
    }
}

fn partition_arg(args: &serde_json::Value) -> Result<ScenePartition, String> {
    match args.get("partition") {
        Some(p) => serde_json::from_value(p.clone()).map_err(|e| format!("bad partition: {e}")),
        None => Ok(ScenePartition::default()),
    }
}

pub(crate) fn register(app: &mut App) {
    app.init_resource::<StreamedScenes>()
        .register_api(
            "scene.partition",
            "The cells a scene would stream in (nothing loads): give 'scene' (a document) or \
             'path', and optionally 'partition' {grids: [{name, cell_m, load_m}], \
             roots_per_step, release_assets, proxies}. Returns the roots that always load and \
             each cell's key, grid, level, size, center, radius and roots.",
            r#"{"path": "assets/scenes/valley.scene.json", "partition": {"grids": [{"name": "main", "cell_m": 128, "load_m": 256}]}}"#,
            |_, args| {
                let layout = partition("preview", &scene_arg(&args)?, &partition_arg(&args)?)?;
                serde_json::to_value(layout).map_err(|e| e.to_string())
            },
        )
        .register_api(
            "scene.stream",
            "Stream a scene as 'name' (replacing one streamed under it): its roots go to grid \
             cells by position and load around the player; roots with StreamPlacement \
             spatial: false or without a Transform load now and stay. Same 'scene'/'path' and \
             'partition' as scene.partition. Returns the layout.",
            r#"{"name": "valley", "path": "assets/scenes/valley.scene.json"}"#,
            |world, args| {
                let name = args["name"].as_str().ok_or("name: what to call the scene")?;
                let layout = stream_scene(world, name, &scene_arg(&args)?, &partition_arg(&args)?)?;
                serde_json::to_value(layout).map_err(|e| e.to_string())
            },
        )
        .register_api(
            "scene.unstream",
            "Take a streamed scene down: its cells unload for good, its persistent roots go.",
            r#"{"name": "valley"}"#,
            |world, args| {
                let name = args["name"].as_str().ok_or("name: a streamed scene")?;
                if unstream_scene(world, name) {
                    Ok(serde_json::json!({ "unstreamed": name }))
                } else {
                    Err(format!("no streamed scene {name:?}"))
                }
            },
        );
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::stream::{StreamingBudget, StreamingPlugin};
    use engine_world::{CellState, StreamedCells, StreamingSource};

    fn root(name: &str, at: [f64; 3], placement: Option<serde_json::Value>) -> EntityDoc {
        let mut components = serde_json::Map::new();
        components.insert("Transform".into(), serde_json::json!({ "translation": at }));
        if let Some(p) = placement {
            components.insert("StreamPlacement".into(), p);
        }
        EntityDoc {
            name: Some(name.into()),
            components,
            children: vec![EntityDoc {
                name: Some(format!("{name} door")),
                ..Default::default()
            }],
        }
    }

    /// Huts along a road, a sky without a Transform, a keep marked always in, and a castle
    /// that reaches across 300 m.
    fn valley() -> SceneDoc {
        let mut entities: Vec<EntityDoc> = (0..10)
            .map(|i| {
                root(
                    &format!("hut {i}"),
                    [i as f64 * 100.0 + 10.0, 0.0, 5.0],
                    None,
                )
            })
            .collect();
        entities.push(EntityDoc {
            name: Some("sky".into()),
            ..Default::default()
        });
        entities.push(root(
            "keep",
            [500.0, 0.0, 500.0],
            Some(serde_json::json!({ "spatial": false })),
        ));
        entities.push(root(
            "castle",
            [2000.0, 0.0, 0.0],
            Some(serde_json::json!({ "radius": 150.0 })),
        ));
        SceneDoc { entities }
    }

    fn app() -> App {
        let mut app = App::new();
        app.add_plugins((engine_world::WorldPlugin, StreamingPlugin));
        app.world.resource_mut::<StreamingBudget>().fade_s = 0.0;
        app
    }

    fn named(app: &mut App, prefix: &str) -> usize {
        app.world
            .query::<&Name>()
            .iter(&app.world)
            .filter(|n| n.as_str().starts_with(prefix))
            .count()
    }

    fn settle(app: &mut App) {
        for _ in 0..20 {
            app.update();
        }
    }

    #[test]
    fn roots_go_to_the_cell_under_them_and_big_ones_to_a_coarser_level() {
        let layout = partition("valley", &valley(), &ScenePartition::default()).unwrap();
        // The sky (no Transform) and the keep (not spatial) always load.
        assert_eq!(layout.persistent, vec![10, 11]);
        let fine: Vec<_> = layout.cells.iter().filter(|c| c.level == 0).collect();
        // Huts at x = 10, 110, … 910 in 128 m cells: 0,0,1,2,3,3,4,5,6,7.
        assert_eq!(fine.len(), 8);
        let first = fine.iter().find(|c| c.x == 0).unwrap();
        assert_eq!(first.key, "scene:valley/main/L0/0,0");
        assert_eq!(first.roots, vec![0, 1]);
        assert!((first.center.x - 60.0).abs() < 1e-9, "{first:?}");
        assert!((first.radius - 50.0).abs() < 1e-9, "{first:?}");
        assert_eq!(first.load_m, 256.0);
        // 300 m across needs 512 m cells: level 2.
        let castle = layout.cells.iter().find(|c| c.roots == vec![12]).unwrap();
        assert_eq!((castle.level, castle.size, castle.x), (2, 512.0, 3));
        assert!((castle.radius - 150.0).abs() < 1e-9);
        // A grid that isn't there is an error, not a silent default.
        let mut doc = valley();
        doc.entities.push(root(
            "lost",
            [0.0; 3],
            Some(serde_json::json!({ "grid": "props" })),
        ));
        assert!(partition("v", &doc, &ScenePartition::default()).is_err());
    }

    #[test]
    fn a_streamed_scene_loads_what_is_near_and_unloads_for_good() {
        let mut app = app();
        // Source at the road's west end.
        app.world.spawn((
            Transform::from_translation(DVec3::new(0.0, 0.0, 0.0)),
            GlobalTransform::default(),
            StreamingSource::default(),
        ));
        stream_scene(
            &mut app.world,
            "valley",
            &valley(),
            &ScenePartition::default(),
        )
        .unwrap();
        // Always-loaded roots are in before the first frame.
        assert_eq!(named(&mut app, "sky"), 1);
        assert_eq!(named(&mut app, "keep"), 2, "the keep and its door");
        settle(&mut app);
        // Cells whose edge is within 256 m of x = 0: huts 0 and 1 (one cell) and hut 2, not
        // hut 3 at 310 m, nor the castle 2 km away.
        assert_eq!(named(&mut app, "hut "), 6, "three huts and their doors");
        assert_eq!(named(&mut app, "hut 9"), 0);
        assert_eq!(named(&mut app, "castle"), 0);
        assert_eq!(
            named(&mut app, "hut 0 door"),
            1,
            "children come with their roots"
        );
        // Streaming it again replaces it: no doubles.
        stream_scene(
            &mut app.world,
            "valley",
            &valley(),
            &ScenePartition::default(),
        )
        .unwrap();
        settle(&mut app);
        assert_eq!(named(&mut app, "sky"), 1);
        assert_eq!(named(&mut app, "hut 0"), 2);
        let cells = app.world.resource::<StreamedCells>().cells.len();
        assert_eq!(cells, 9, "replaced in place, not added");
        // Taken down: everything goes and nothing comes back.
        assert!(unstream_scene(&mut app.world, "valley"));
        settle(&mut app);
        assert_eq!(named(&mut app, "hut"), 0);
        assert_eq!(named(&mut app, "sky"), 0);
        assert_eq!(named(&mut app, "keep"), 0);
        let streamed = app.world.resource::<StreamedCells>();
        assert!(
            streamed
                .cells
                .iter()
                .all(|c| c.retired && c.state == CellState::Unloaded)
        );
        assert!(streamed.is_ready_around(DVec3::ZERO, 1000.0));
        // And it can come back under the same keys.
        stream_scene(
            &mut app.world,
            "valley",
            &valley(),
            &ScenePartition::default(),
        )
        .unwrap();
        settle(&mut app);
        assert_eq!(named(&mut app, "hut 0"), 2);
        assert_eq!(app.world.resource::<StreamedCells>().cells.len(), 9);
    }

    #[test]
    fn an_unregistered_component_changes_nothing() {
        let mut app = app();
        let mut doc = valley();
        doc.entities[3].children[0]
            .components
            .insert("NoSuchThing".into(), serde_json::json!({}));
        let err =
            stream_scene(&mut app.world, "valley", &doc, &ScenePartition::default()).unwrap_err();
        assert!(err.contains("hut 3 door"), "{err}");
        assert_eq!(named(&mut app, "sky"), 0);
        assert!(app.world.resource::<StreamedCells>().cells.is_empty());
    }
}
