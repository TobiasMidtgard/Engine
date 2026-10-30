//! The editor's state and the actions its panels, menus and shortcuts call: selecting,
//! creating, deleting, duplicating, saving, loading and playing.

use std::collections::{HashMap, HashSet};

use engine::core::scene::SceneDoc;
use engine::prelude::*;
use serde_json::{Value, json};

use crate::camera::EditorCamera;
use crate::demos::{DemoPart, ModuleDemo};
use crate::scene_ops::{self, EditorOnly, InScene, Remap, SceneOrder};
use crate::undo::{Command, History};
use crate::view::ViewInfo;

/// Where scene files live (relative to the working directory).
pub const SCENES_DIR: &str = "scenes";

/// Editing, or running the game inside the editor.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    /// The world is paused and edits are recorded.
    Edit,
    /// The world runs from a snapshot of the scene; Stop puts the snapshot back.
    Play,
    /// Play, paused (step one frame with Advance).
    Paused,
}

/// The active scene view tool.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tool {
    Select,
    Move,
    Rotate,
    Scale,
    /// Landscape brush (see `landscape`).
    Sculpt,
    /// Paint copies of models onto surfaces (see `scatter`).
    Scatter,
    /// Measure distances (see `measure`).
    Measure,
}

/// A panel in the dock.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tab {
    Viewport,
    Outliner,
    Inspector,
    World,
    Content,
    Console,
    History,
    Partition,
    Matrix,
    Scripts,
    Graphs,
    Rewind,
    Curves,
    Sequencer,
    Story,
}

/// A message in the console.
#[derive(Debug, Clone)]
pub struct LogLine {
    pub at: f64,
    pub text: String,
    pub error: bool,
}

/// What Play saved, so Stop can put the scene back exactly.
pub struct PlaySnapshot {
    roots: Vec<scene_ops::SavedEntity>,
    existing: HashSet<Entity>,
    /// Registered resources as they were (weather, sky, time of day, settings).
    resources: Vec<(String, Value)>,
    was_dirty: bool,
}

/// Resources Stop leaves alone: input, the window, measurements and debug views describe the
/// editor session, not the world, and the space frame follows the camera.
const LIVE_RESOURCES: &[&str] = &[
    "Mouse",
    "Window",
    "InputMap",
    "UiInputCapture",
    "BridgeInfo",
    "EngineConfig",
    "PerfOverlay",
    "DebugViews",
    "PhysicsDebug",
    "CharacterDebug",
    "PhysicsStats",
    "DestructionStats",
    "PerceptionStatus",
    "SpaceFrame",
];

/// The registered resources Stop puts back, with their values now.
fn world_resources(world: &World) -> Vec<(String, Value)> {
    world
        .resource::<TypeRegistry>()
        .resources()
        .filter(|r| !LIVE_RESOURCES.contains(&r.name.as_str()))
        .filter_map(|r| Some((r.name.clone(), (r.get)(world)?)))
        .collect()
}

/// How the scene tab is split into views (Unreal's viewport layouts, 3ds Max's quad view).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Layout {
    One,
    /// Side by side: perspective and top.
    Two,
    /// Top, front, right and perspective.
    Four,
}

/// A scene view's own camera and pointer state. The active view's lives in [`Editor`]'s
/// `camera`, `gizmo`, `view`, `box_start` and `viewport_hovered`, so everything that works
/// on "the view" works on the one the pointer is in.
pub struct Pane {
    pub camera: Entity,
    pub gizmo: transform_gizmo_egui::Gizmo,
    pub view: Option<ViewInfo>,
    pub box_start: Option<egui::Pos2>,
    pub viewport_hovered: bool,
}

impl Pane {
    pub fn new(camera: Entity) -> Self {
        Self {
            camera,
            gizmo: Default::default(),
            view: None,
            box_start: None,
            viewport_hovered: false,
        }
    }
}

/// All editor state other than the dock layout.
#[derive(Resource)]
pub struct Editor {
    pub mode: Mode,
    /// Selected entities; the last one is the primary (inspector, gizmo pivot).
    pub selection: Vec<Entity>,
    pub tool: Tool,
    pub local_space: bool,
    pub snap: bool,
    pub snap_move: f32,
    pub snap_angle_deg: f32,
    pub snap_scale: f32,
    /// Hide every editor helper to see the scene as a player will (the Show menu).
    pub game_view: bool,
    /// Draw the selection's outline and its colliders (the Show menu).
    pub show_outline: bool,
    pub show_colliders: bool,
    /// Where the toolbar's View and Show buttons were drawn (for scripted clicks).
    pub view_button: Option<egui::Rect>,
    pub show_button: Option<egui::Rect>,
    /// The Measure tool's ruler.
    pub measure: crate::measure::Measure,
    /// Prefabs open for editing, outermost first (empty: editing the scene).
    pub prefab_mode: Vec<crate::prefab::Focus>,
    /// Prefab changes asked for this frame, done after the UI pass.
    pub prefab_actions: Vec<crate::prefab_ui::Action>,
    /// When prefab overrides are next refreshed (seconds, [`now`]).
    pub prefab_track_next: f64,
    /// Where the Details panel's Overrides button was drawn (for scripted clicks).
    pub overrides_button: Option<egui::Rect>,
    /// The transform gizmo was being dragged last frame (Alt at the start of a drag duplicates).
    pub gizmo_dragging: bool,
    /// A view change for the editor camera (the axis widget's clicks).
    pub view_request: Option<crate::camera::ViewRequest>,
    /// A V or Ctrl+Shift snap drag in progress.
    pub snap_drag: Option<crate::snap::Drag>,
    /// Surface snaps tilt the selection's up axis to the surface (Unreal's Rotate to Surface
    /// Normal).
    pub surface_align: bool,
    /// Surface snaps hold the selection this far off the surface, meters.
    pub surface_offset: f32,
    pub history: History,
    pub scene_path: String,
    pub log: Vec<LogLine>,
    pub gizmo: transform_gizmo_egui::Gizmo,
    pub camera: Entity,
    /// The scene view this frame (rect, camera).
    pub view: Option<ViewInfo>,
    pub viewport_hovered: bool,
    pub outliner_filter: String,
    pub show_runtime: bool,
    pub add_filter: String,
    /// Variant names of enum fields, found by asking serde (see `inspector::enum_variants`).
    pub enum_cache: HashMap<String, Option<Vec<String>>>,
    pub play: Option<PlaySnapshot>,
    /// Advance one frame while paused.
    pub step_once: bool,
    /// Frame time smoothing for the stats overlay, seconds.
    pub frame_secs: f32,
    /// Draw the ground grid.
    pub grid: bool,
    /// Draw icons for lights, cameras and demos.
    pub icons: bool,
    /// Pending "frame the selection" for the camera.
    pub focus: Option<(DVec3, f64)>,
    /// Where a box selection drag started in the scene view (points).
    pub box_start: Option<egui::Pos2>,
    /// Copied scene entities, as saved documents with the parent they had.
    pub clipboard: Vec<(scene_ops::SavedEntity, Option<Entity>)>,
    /// The landscape brush.
    pub brush: crate::landscape::Brush,
    /// Model files under the asset root (scanned when the Content panel first shows them).
    pub models: Option<Vec<String>>,
    /// A file from the desktop being dragged over the window.
    pub file_hover: Option<std::path::PathBuf>,
    /// Files dropped on the window, imported by the next UI pass.
    pub file_drops: Vec<std::path::PathBuf>,
    /// The selected model's import settings while they're edited (asset key, settings).
    pub import_edit: Option<(String, crate::import::ImportSettings)>,
    /// The materials of the file `import_edit` is for, for its remap list.
    pub import_materials: Vec<crate::remap::FileMaterial>,
    /// The scatter brush's settings and stroke.
    pub scatter: crate::scatter::Scatter,
    /// What each model file holds (the Content panel's hover preview), by asset key.
    pub model_info: HashMap<String, String>,
    /// The Content panel's import path.
    pub import_path: String,
    /// The Content Browser.
    pub browser: crate::browser::Browser,
    /// The World Partition panel and streaming in Play.
    pub partition: crate::partition::Partition,
    /// The Property Matrix.
    pub matrix: crate::matrix::Matrix,
    /// Scene views: the layout, each view's state (see [`Pane`]), the one being drawn, the
    /// active one, and the one maximized (when it fills the tab alone).
    pub layout: Layout,
    pub panes: Vec<Pane>,
    pub pane: usize,
    pub active_pane: usize,
    pub maximized: Option<usize>,
    /// A view's Maximize or Restore button was clicked.
    pub maximize_request: Option<usize>,
    /// The whole scene tab (toolbars and bars go there, not in one view).
    pub tab_rect: Option<egui::Rect>,
    /// What is hidden and locked in the scene view.
    pub hide: crate::hide::Hide,
    /// The outliner's type filter (names from `ui::OUTLINER_TYPES`).
    pub outliner_types: Vec<&'static str>,
    /// The outliner row being renamed, its text, and whether to focus it.
    pub outliner_rename: Option<(Entity, String, bool)>,
    /// When placed models' files last changed (for reimport).
    pub model_watch: crate::models::Watch,
}

impl Editor {
    pub fn new(camera: Entity) -> Self {
        Self {
            mode: Mode::Edit,
            selection: Vec::new(),
            tool: Tool::Move,
            local_space: false,
            snap: false,
            snap_move: 0.25,
            snap_angle_deg: 15.0,
            snap_scale: 0.1,
            prefab_mode: Vec::new(),
            prefab_actions: Vec::new(),
            prefab_track_next: 0.0,
            overrides_button: None,
            view_request: None,
            measure: Default::default(),
            game_view: false,
            show_outline: true,
            show_colliders: true,
            view_button: None,
            show_button: None,
            gizmo_dragging: false,
            snap_drag: None,
            surface_align: false,
            surface_offset: 0.0,
            history: History::default(),
            scene_path: format!("{SCENES_DIR}/untitled.scene.json"),
            log: Vec::new(),
            gizmo: Default::default(),
            camera,
            view: None,
            viewport_hovered: false,
            outliner_filter: String::new(),
            show_runtime: false,
            add_filter: String::new(),
            enum_cache: HashMap::new(),
            play: None,
            step_once: false,
            frame_secs: 1.0 / 60.0,
            grid: true,
            icons: true,
            focus: None,
            box_start: None,
            clipboard: Vec::new(),
            brush: Default::default(),
            models: None,
            model_watch: Default::default(),
            import_path: String::new(),
            browser: Default::default(),
            partition: Default::default(),
            matrix: Default::default(),
            layout: Layout::One,
            panes: vec![Pane::new(camera)],
            pane: 0,
            active_pane: 0,
            maximized: None,
            maximize_request: None,
            tab_rect: None,
            hide: Default::default(),
            outliner_types: Vec::new(),
            outliner_rename: None,
            file_hover: None,
            import_edit: None,
            import_materials: Vec::new(),
            scatter: crate::scatter::Scatter::default(),
            model_info: HashMap::new(),
            file_drops: Vec::new(),
        }
    }

    pub fn primary(&self) -> Option<Entity> {
        self.selection.last().copied()
    }

    pub fn is_selected(&self, e: Entity) -> bool {
        self.selection.contains(&e)
    }

    /// Select only `e` (or nothing).
    pub fn select(&mut self, e: Option<Entity>) {
        self.selection.clear();
        self.selection.extend(e);
    }

    /// Add or remove `e` from the selection (Ctrl-click).
    pub fn toggle(&mut self, e: Entity) {
        if let Some(i) = self.selection.iter().position(|&s| s == e) {
            self.selection.remove(i);
        } else {
            self.selection.push(e);
        }
    }

    pub fn info(&mut self, now: f64, text: impl Into<String>) {
        let text = text.into();
        engine::core::log::info!("editor: {text}");
        self.log.push(LogLine {
            at: now,
            text,
            error: false,
        });
    }

    pub fn error(&mut self, now: f64, text: impl Into<String>) {
        let text = text.into();
        engine::core::log::warn!("editor: {text}");
        self.log.push(LogLine {
            at: now,
            text,
            error: true,
        });
    }

    /// Report the result of an action in the console.
    pub fn report(&mut self, now: f64, result: Result<(), String>) {
        if let Err(e) = result {
            self.error(now, e);
        }
    }

    /// Keep the selection pointing at live entities after undo/redo respawned some.
    pub fn remap_selection(&mut self, map: &Remap) {
        for e in &mut self.selection {
            if let Some(new) = map.get(e) {
                *e = *new;
            }
        }
        self.hide.remap(map);
    }

    /// Swap the editor's view state with pane `i`'s (twice puts it back).
    pub fn swap_pane(&mut self, i: usize) {
        let p = &mut self.panes[i];
        std::mem::swap(&mut self.camera, &mut p.camera);
        std::mem::swap(&mut self.gizmo, &mut p.gizmo);
        std::mem::swap(&mut self.view, &mut p.view);
        std::mem::swap(&mut self.box_start, &mut p.box_start);
        std::mem::swap(&mut self.viewport_hovered, &mut p.viewport_hovered);
    }

    /// Make pane `i` the active view.
    pub fn activate_pane(&mut self, i: usize) {
        if i == self.active_pane || i >= self.panes.len() {
            return;
        }
        let a = self.active_pane;
        self.swap_pane(a);
        self.swap_pane(i);
        self.active_pane = i;
    }

    pub fn is_playing(&self) -> bool {
        self.mode != Mode::Edit
    }
}

/// Real seconds since start (the clock edits are timed with; world time is paused in Edit).
pub fn now(world: &World) -> f64 {
    world.resource::<Time>().real_elapsed_secs_f64()
}

/// Apply an edit through the history and report errors.
pub fn apply(world: &mut World, editor: &mut Editor, command: Command) {
    let t = now(world);
    let result = editor.history.apply(world, command, t);
    editor.report(t, result);
}

pub fn undo(world: &mut World, editor: &mut Editor) {
    let t = now(world);
    match editor.history.undo(world) {
        Ok(Some(label)) => editor.info(t, format!("Undo: {label}")),
        Ok(None) => {}
        Err(e) => editor.error(t, format!("Undo failed: {e}")),
    }
    let map = editor.history.last_remap.clone();
    editor.remap_selection(&map);
    editor.selection.retain(|&e| world.get_entity(e).is_ok());
}

pub fn redo(world: &mut World, editor: &mut Editor) {
    let t = now(world);
    match editor.history.redo(world) {
        Ok(Some(label)) => editor.info(t, format!("Redo: {label}")),
        Ok(None) => {}
        Err(e) => editor.error(t, format!("Redo failed: {e}")),
    }
    let map = editor.history.last_remap.clone();
    editor.remap_selection(&map);
    editor.selection.retain(|&e| world.get_entity(e).is_ok());
}

/// Something to create from a menu or the content panel.
#[derive(Debug, Clone, PartialEq)]
pub enum Create {
    Empty,
    Shape(&'static str, Shape),
    PointLight,
    SunLight,
    Camera,
    Demo(&'static str),
}

/// The shapes offered in menus: label and shape.
pub fn shape_presets() -> Vec<(&'static str, Shape)> {
    vec![
        (
            "Cube",
            Shape::RoundedCuboid {
                size: [1.0; 3],
                radius: 0.03,
                segments: 0,
            },
        ),
        (
            "Sphere",
            Shape::Sphere {
                radius: 0.5,
                segments: 0,
            },
        ),
        (
            "Cylinder",
            Shape::Cylinder {
                radius: 0.5,
                height: 1.0,
                segments: 0,
            },
        ),
        (
            "Capsule",
            Shape::Capsule {
                radius: 0.35,
                height: 1.0,
                segments: 0,
            },
        ),
        (
            "Cone",
            Shape::Cone {
                radius: 0.5,
                height: 1.0,
                segments: 0,
            },
        ),
        (
            "Torus",
            Shape::Torus {
                radius: 0.5,
                tube: 0.15,
                segments: 0,
            },
        ),
        (
            "Plane",
            Shape::Plane {
                size: [10.0, 10.0],
                subdivisions: 1,
            },
        ),
        (
            "Rock",
            Shape::Rock {
                size: [1.6, 1.0, 1.3],
                seed: 7,
                detail: 0,
            },
        ),
    ]
}

/// Where new things go: on the ground (y = 0 plane) under the middle of the view, at most
/// 30 m away, else 8 m in front of the camera.
pub fn placement_point(world: &mut World, editor: &Editor) -> DVec3 {
    let Some(view) = editor.view else {
        return DVec3::ZERO;
    };
    // Where the view center meets a mesh or the ground, or 8 m ahead in the air.
    let (o, d) = view.ray(view.screen.center());
    crate::picking::pick_surface(world, o, d, 60.0, &|_| false).unwrap_or(o + d.as_dvec3() * 8.0)
}

/// Put the selected scene entities down on whatever is below them (mesh or ground), bottoms
/// touching, as one undo step.
pub fn drop_to_ground(world: &mut World, editor: &mut Editor) {
    let t = now(world);
    let targets: Vec<Entity> = editor
        .selection
        .iter()
        .copied()
        .filter(|&e| world.get::<InScene>(e).is_some())
        .collect();
    let mut commands = Vec::new();
    for e in targets {
        let Some(global) = world.get::<GlobalTransform>(e).copied() else {
            continue;
        };
        let Some(before) = world.get::<Transform>(e).copied() else {
            continue;
        };
        // How far the entity reaches below its origin (mesh bounds, else nothing).
        let below = world
            .get::<engine::render::ResolvedMesh>(e)
            .and_then(|r| world.resource::<Assets<engine::render::Mesh>>().get(r.mesh))
            .map(|m| {
                let aabb = m.aabb();
                let mut low = f64::INFINITY;
                for i in 0..8 {
                    let corner = Vec3::new(
                        if i & 1 == 0 { aabb.min.x } else { aabb.max.x },
                        if i & 2 == 0 { aabb.min.y } else { aabb.max.y },
                        if i & 4 == 0 { aabb.min.z } else { aabb.max.z },
                    );
                    low = low.min(global.transform_point(corner.as_dvec3()).y);
                }
                global.translation().y - low
            })
            .unwrap_or(0.0);
        let start = global.translation() + DVec3::Y * 0.01;
        let own: HashSet<Entity> = world
            .iter_entities()
            .map(|r| r.id())
            .filter(|&k| scene_ops::is_ancestor(world, e, k))
            .collect();
        let skip = |hit: Entity| own.contains(&hit);
        let Some(floor) =
            crate::picking::pick_surface(world, start - DVec3::Y * below, -Vec3::Y, 500.0, &skip)
        else {
            continue;
        };
        let lift = floor.y + below - global.translation().y;
        if lift.abs() < 1e-4 {
            continue;
        }
        let mut after = before;
        // Moves along world up; for a child, through its parent's scale and rotation.
        let parent_inv = world
            .get::<ChildOf>(e)
            .and_then(|p| world.get::<GlobalTransform>(p.parent()))
            .map(|g| g.inverse());
        let up = match parent_inv {
            Some(inv) => inv.transform_vector3(DVec3::Y),
            None => DVec3::Y,
        };
        after.translation += up * lift;
        let (Ok(b), Ok(a)) = (serde_json::to_value(before), serde_json::to_value(after)) else {
            continue;
        };
        commands.push(Command::Component {
            entity: e,
            name: "Transform".into(),
            before: Some(b),
            after: Some(a),
        });
    }
    match commands.len() {
        0 => editor.info(t, "Nothing to drop: no surface below the selection."),
        1 => apply(world, editor, commands.pop().expect("one")),
        n => apply(
            world,
            editor,
            Command::Group(format!("Drop {n} entities to the ground"), commands),
        ),
    }
}

/// Create something in the scene at `at`, select it, and record it for undo.
pub fn create(world: &mut World, editor: &mut Editor, what: Create, at: DVec3) -> Entity {
    let (name, components): (String, Vec<(&str, Value)>) = match &what {
        Create::Empty => ("Empty".into(), vec![]),
        Create::Shape(label, shape) => {
            let lift = match shape {
                Shape::Plane { .. } => 0.0,
                Shape::Rock { .. } => 0.0,
                _ => 0.5,
            };
            let renderer =
                MeshRenderer::new(*shape, StandardMaterial::hex(0xb8b2a8).with_roughness(0.55));
            return finish_create(
                world,
                editor,
                label.to_string(),
                vec![(
                    "MeshRenderer",
                    serde_json::to_value(renderer).unwrap_or_default(),
                )],
                at + DVec3::Y * lift,
            );
        }
        Create::PointLight => (
            "Point light".into(),
            vec![(
                "PointLight",
                serde_json::to_value(PointLight::default()).unwrap_or_default(),
            )],
        ),
        Create::SunLight => (
            "Directional light".into(),
            vec![(
                "DirectionalLight",
                serde_json::to_value(DirectionalLight::default()).unwrap_or_default(),
            )],
        ),
        Create::Camera => (
            "Camera".into(),
            vec![(
                "Camera",
                serde_json::to_value(Camera {
                    active: false,
                    ..Default::default()
                })
                .unwrap_or_default(),
            )],
        ),
        Create::Demo(key) => {
            let label = crate::demos::find(key).map(|d| d.1).unwrap_or(key);
            (
                label.to_string(),
                vec![("ModuleDemo", json!({ "module": key }))],
            )
        }
    };
    let lift = if matches!(what, Create::PointLight) {
        2.0
    } else {
        0.0
    };
    finish_create(world, editor, name, components, at + DVec3::Y * lift)
}

fn finish_create(
    world: &mut World,
    editor: &mut Editor,
    name: String,
    components: Vec<(&str, Value)>,
    at: DVec3,
) -> Entity {
    let mut transform = Transform::from_translation(at);
    if components.iter().any(|(c, _)| *c == "DirectionalLight") {
        transform = transform.looking_at(at + DVec3::new(-0.5, -0.7, -0.4), Vec3::Y);
    }
    spawn_with(world, editor, name, components, transform)
}

/// Spawn a scene entity with registered components, record it for undo and select it.
pub fn spawn_with(
    world: &mut World,
    editor: &mut Editor,
    name: String,
    components: Vec<(&str, Value)>,
    transform: Transform,
) -> Entity {
    let t = now(world);
    let entity = world
        .spawn((
            InScene,
            SceneOrder::next(),
            Name::new(name.clone()),
            transform,
        ))
        .id();
    for (component, value) in components {
        let entry = world
            .resource::<TypeRegistry>()
            .component_or_err(component)
            .cloned();
        let result = entry.and_then(|e| (e.insert)(world, entity, value));
        editor.report(t, result);
    }
    let saved = scene_ops::save_entity(world, entity);
    editor.history.record(
        world,
        Command::Spawned {
            saved,
            parent: None,
        },
        t,
    );
    editor.select(Some(entity));
    editor.info(t, format!("Created {name}"));
    entity
}

/// Delete the selected scene entities (one undo step).
pub fn delete_selection(world: &mut World, editor: &mut Editor) {
    let t = now(world);
    let mut targets: Vec<Entity> = editor
        .selection
        .iter()
        .copied()
        .filter(|&e| world.get::<InScene>(e).is_some())
        .collect();
    // Children of other selected entities go with their parent.
    let all = targets.clone();
    targets.retain(|&e| {
        !all.iter()
            .any(|&other| other != e && scene_ops::is_ancestor(world, other, e))
    });
    if targets.is_empty() {
        if !editor.selection.is_empty() {
            editor.error(
                t,
                "Only scene entities can be deleted (runtime entities belong to their module).",
            );
        }
        return;
    }
    let commands: Vec<Command> = targets
        .iter()
        .map(|&e| Command::Despawned {
            saved: scene_ops::save_entity(world, e),
            parent: world.get::<ChildOf>(e).map(|p| p.parent()),
        })
        .collect();
    let label = if commands.len() == 1 {
        commands[0].describe(world)
    } else {
        format!("Delete {} entities", commands.len())
    };
    apply(world, editor, Command::Group(label, commands));
    editor.selection.clear();
}

/// Duplicate the selected scene entities next to themselves and select the copies.
pub fn duplicate_selection(world: &mut World, editor: &mut Editor) {
    duplicate_offset(world, editor, DVec3::new(1.0, 0.0, 0.0));
}

/// Duplicate the selected scene entities `offset` from themselves (in their parent's space) and
/// select the copies; returns (original, copy) pairs.
pub fn duplicate_offset(
    world: &mut World,
    editor: &mut Editor,
    offset: DVec3,
) -> Vec<(Entity, Entity)> {
    let t = now(world);
    let targets: Vec<Entity> = editor
        .selection
        .iter()
        .copied()
        .filter(|&e| world.get::<InScene>(e).is_some())
        .collect();
    let mut copies = Vec::new();
    let mut commands = Vec::new();
    for e in targets {
        let saved = scene_ops::save_entity(world, e);
        let parent = world.get::<ChildOf>(e).map(|p| p.parent());
        let mut remap = Remap::new();
        let copy = scene_ops::restore_entity(world, &saved, parent, &mut remap);
        if let Some(mut tr) = world.get_mut::<Transform>(copy) {
            tr.translation += offset;
        }
        if let Some(name) = world.get::<Name>(copy).map(|n| n.as_str().to_string()) {
            world.entity_mut(copy).insert(Name::new(next_name(&name)));
        }
        commands.push(Command::Spawned {
            saved: scene_ops::save_entity(world, copy),
            parent,
        });
        copies.push((e, copy));
    }
    if copies.is_empty() {
        return copies;
    }
    let new: Vec<Entity> = copies.iter().map(|c| c.1).collect();
    let label = count_label(world, "Duplicate", &new);
    editor
        .history
        .record(world, Command::Group(label.clone(), commands), t);
    editor.selection = new;
    editor.info(t, label);
    copies
}

/// Copy the selection (topmost entities, with their children) to the editor's clipboard.
pub fn copy_selection(world: &mut World, editor: &mut Editor) {
    let t = now(world);
    let selected: Vec<Entity> = editor
        .selection
        .iter()
        .copied()
        .filter(|&e| world.get::<InScene>(e).is_some())
        .collect();
    let topmost: Vec<Entity> = selected
        .iter()
        .copied()
        .filter(|&e| {
            !selected
                .iter()
                .any(|&o| o != e && scene_ops::is_ancestor(world, o, e))
        })
        .collect();
    if topmost.is_empty() {
        return;
    }
    editor.clipboard = topmost
        .iter()
        .map(|&e| {
            let mut saved = scene_ops::save_entity(world, e);
            saved.order = None;
            (saved, world.get::<ChildOf>(e).map(|p| p.parent()))
        })
        .collect();
    editor.info(t, format!("Copied {} entities", topmost.len()));
}

/// Paste the clipboard in place (under the same parent while it exists) and select the copies.
pub fn paste(world: &mut World, editor: &mut Editor) {
    let t = now(world);
    if editor.clipboard.is_empty() {
        return;
    }
    let mut copies = Vec::new();
    let mut commands = Vec::new();
    for (saved, parent) in editor.clipboard.clone() {
        let parent = parent.filter(|&p| world.get::<InScene>(p).is_some());
        let copy = scene_ops::restore_entity(world, &saved, parent, &mut Remap::new());
        if let Some(name) = world.get::<Name>(copy).map(|n| n.as_str().to_string()) {
            world.entity_mut(copy).insert(Name::new(next_name(&name)));
        }
        commands.push(Command::Spawned {
            saved: scene_ops::save_entity(world, copy),
            parent,
        });
        copies.push(copy);
    }
    let label = count_label(world, "Paste", &copies);
    editor
        .history
        .record(world, Command::Group(label.clone(), commands), t);
    editor.selection = copies;
    editor.info(t, label);
}

/// "Paste Barrel 2" for one entity, "Paste 3 entities" for more.
fn count_label(world: &World, verb: &str, entities: &[Entity]) -> String {
    match entities {
        [one] => format!("{verb} {}", scene_ops::label(world, *one)),
        many => format!("{verb} {} entities", many.len()),
    }
}

/// "Crate" → "Crate 2", "Crate 2" → "Crate 3".
pub fn next_name(name: &str) -> String {
    match name.rsplit_once(' ') {
        Some((base, n)) if n.parse::<u32>().is_ok() => {
            format!("{base} {}", n.parse::<u32>().unwrap_or(1) + 1)
        }
        _ => format!("{name} 2"),
    }
}

/// Move `entity` under `parent` (or to the root), keeping where it is in the world.
pub fn reparent(world: &mut World, editor: &mut Editor, entity: Entity, parent: Option<Entity>) {
    let t = now(world);
    if parent.is_some_and(|p| scene_ops::is_ancestor(world, entity, p)) {
        editor.error(t, "Can't put an entity under itself or its own children.");
        return;
    }
    let before = world.get::<ChildOf>(entity).map(|p| p.parent());
    if before == parent {
        return;
    }
    let Some(before_local) = world.get::<Transform>(entity).copied() else {
        return;
    };
    let world_affine = world
        .get::<GlobalTransform>(entity)
        .map(|g| g.affine())
        .unwrap_or_else(|| before_local.to_affine());
    let parent_inv = parent
        .and_then(|p| world.get::<GlobalTransform>(p))
        .map(|g| g.inverse())
        .unwrap_or(glam::DAffine3::IDENTITY);
    let after_local = transform_from_affine(parent_inv * world_affine);
    apply(
        world,
        editor,
        Command::Reparent {
            entity,
            before,
            after: parent,
            before_local,
            after_local,
        },
    );
}

/// Decompose an affine into a [`Transform`] (f64 translation, f32 rotation and scale).
pub fn transform_from_affine(a: glam::DAffine3) -> Transform {
    let (scale, rotation, translation) = a.to_scale_rotation_translation();
    Transform {
        translation,
        rotation: rotation.as_quat().normalize(),
        scale: scale.as_vec3(),
    }
}

/// Frame the selection: point the camera at its bounds.
pub fn focus_selection(world: &mut World, editor: &mut Editor) {
    let mut center = DVec3::ZERO;
    let mut radius: f64 = 0.0;
    let mut n = 0.0;
    let points: Vec<(DVec3, f64)> = editor
        .selection
        .iter()
        .filter_map(|&e| bounds(world, e))
        .collect();
    for (c, _) in &points {
        center += *c;
        n += 1.0;
    }
    if n == 0.0 {
        return;
    }
    center /= n;
    for (c, r) in &points {
        radius = radius.max((*c - center).length() + r);
    }
    editor.focus = Some((center, radius.max(0.5)));
}

/// World-space bounding sphere of an entity's mesh (or a small one around its position).
pub fn bounds(world: &World, e: Entity) -> Option<(DVec3, f64)> {
    let global = world.get::<GlobalTransform>(e)?;
    let mesh = world
        .get::<engine::render::ResolvedMesh>(e)
        .and_then(|r| world.resource::<Assets<engine::render::Mesh>>().get(r.mesh));
    match mesh {
        Some(mesh) => {
            let aabb = mesh.aabb();
            let c = global.transform_point(((aabb.min + aabb.max) * 0.5).as_dvec3());
            let r = ((aabb.max - aabb.min) * 0.5 * global.scale().abs()).length() as f64;
            Some((c, r))
        }
        None => {
            if let Some(instance) = world.get::<crate::demos::DemoInstance>(e)
                && !instance.entities.is_empty()
            {
                return Some((global.translation(), 12.0));
            }
            Some((global.translation(), 0.5))
        }
    }
}

/// What a click on `hit` selects: the demo it belongs to, else the entity itself.
pub fn selectable(world: &World, hit: Entity) -> Entity {
    // A part a script's `construct` built selects the scripted entity (Unity picks the object
    // whose gizmo or child was clicked the same way), through parts of parts.
    let mut hit = hit;
    for _ in 0..16 {
        let built =
            std::iter::successors(Some(hit), |&e| world.get::<ChildOf>(e).map(|p| p.parent()))
                .find_map(|e| world.get::<engine_script::Constructed>(e).map(|c| c.by));
        match built {
            Some(by) if by != hit => hit = by,
            _ => break,
        }
    }
    let root = scene_ops::root_of(world, hit);
    if let Some(part) = world.get::<DemoPart>(root) {
        return part.spawner;
    }
    hit
}

// ----------------------------------------------------------------------------- files

/// Scene files in [`SCENES_DIR`], sorted.
pub fn scene_files() -> Vec<String> {
    let mut files: Vec<String> = std::fs::read_dir(SCENES_DIR)
        .map(|dir| {
            dir.filter_map(|e| e.ok())
                .map(|e| e.path())
                .filter(|p| p.to_string_lossy().ends_with(".scene.json"))
                .map(|p| p.to_string_lossy().replace('\\', "/"))
                .collect()
        })
        .unwrap_or_default();
    files.sort();
    files
}

pub fn save_scene(world: &mut World, editor: &mut Editor, path: &str) {
    let t = now(world);
    if editor.is_playing() {
        editor.error(
            t,
            "Stop playing before saving, so the scene is saved as it was edited.",
        );
        return;
    }
    if !editor.prefab_mode.is_empty() {
        // Ctrl+S while a prefab is open saves the prefab (as in Unity).
        crate::prefab::save_focus(world, editor);
        return;
    }
    crate::prefab::refresh_all(world);
    // A pose the Sequencer is previewing is not part of the scene.
    crate::sequencer::put_back(world);
    let scene = scene_ops::snapshot_scene(world);
    match scene.save(path) {
        Ok(()) => {
            editor.scene_path = path.to_string();
            editor.history.dirty = false;
            crate::hide::save(world, editor);
            editor.info(
                t,
                format!("Saved {path} ({} root entities)", scene.entities.len()),
            );
        }
        Err(e) => editor.error(t, format!("Could not save: {e}")),
    }
}

pub fn load_scene(world: &mut World, editor: &mut Editor, path: &str) {
    let t = now(world);
    if editor.is_playing() {
        stop(world, editor);
    }
    match SceneDoc::load(path) {
        Ok(scene) => {
            crate::prefab::exit_all(world, editor);
            scene_ops::clear_scene(world);
            let roots = scene_ops::load_scene(world, &scene);
            crate::prefab::after_load(world);
            editor.history.clear();
            editor.selection.clear();
            editor.scene_path = path.to_string();
            crate::hide::load(world, editor);
            editor.info(t, format!("Opened {path} ({} root entities)", roots.len()));
        }
        Err(e) => editor.error(t, format!("Could not open {path}: {e}")),
    }
}

pub fn new_scene(world: &mut World, editor: &mut Editor) {
    let t = now(world);
    if editor.is_playing() {
        stop(world, editor);
    }
    crate::prefab::exit_all(world, editor);
    scene_ops::clear_scene(world);
    scene_ops::load_scene(world, &crate::sample::starter_scene());
    editor.history.clear();
    editor.selection.clear();
    editor.scene_path = format!("{SCENES_DIR}/untitled.scene.json");
    editor.hide.clear();
    editor.info(t, "New scene");
}

// ----------------------------------------------------------------------------- play

/// Start playing: snapshot the scene and let the world run.
pub fn play(world: &mut World, editor: &mut Editor) {
    let t = now(world);
    match editor.mode {
        Mode::Paused => {
            editor.mode = Mode::Play;
            world.resource_mut::<Time>().resume();
            return;
        }
        Mode::Play => return,
        Mode::Edit => {}
    }
    crate::prefab::exit_all(world, editor);
    crate::sequencer::put_back(world);
    let roots = scene_ops::scene_roots(world);
    let saved = roots
        .iter()
        .map(|&e| scene_ops::save_entity(world, e))
        .collect();
    let existing = world.iter_entities().map(|e| e.id()).collect();
    editor.play = Some(PlaySnapshot {
        roots: saved,
        existing,
        resources: world_resources(world),
        was_dirty: editor.history.dirty,
    });
    if editor.partition.stream_play {
        crate::partition::start_streaming(world, editor);
    }
    editor.mode = Mode::Play;
    world.resource_mut::<Time>().resume();
    editor.info(
        t,
        "Play: the world runs; Stop puts the scene back as it was.",
    );
}

pub fn pause(world: &mut World, editor: &mut Editor) {
    if editor.mode == Mode::Play {
        editor.mode = Mode::Paused;
        world.resource_mut::<Time>().pause();
    }
}

/// Stop playing: remove what play created and put the scene back from the snapshot.
pub fn stop(world: &mut World, editor: &mut Editor) {
    let t = now(world);
    let Some(snapshot) = editor.play.take() else {
        editor.mode = Mode::Edit;
        world.resource_mut::<Time>().pause();
        return;
    };
    world.resource_mut::<Time>().pause();
    // A streamed level's cells go first, so none loads again.
    crate::partition::stop_streaming(world, editor);
    // Everything spatial that appeared while playing (projectiles, debris, effects).
    let mut q = world.query_filtered::<Entity, (With<GlobalTransform>, Without<EditorOnly>)>();
    let spawned: Vec<Entity> = q
        .iter(world)
        .filter(|e| !snapshot.existing.contains(e))
        .filter(|&e| !crate::partition::is_streamed(world, e))
        .collect();
    for e in spawned {
        if let Ok(e) = world.get_entity_mut(e) {
            e.despawn();
        }
    }
    scene_ops::clear_scene(world);
    let mut remap = Remap::new();
    for saved in &snapshot.roots {
        scene_ops::restore_entity(world, saved, None, &mut remap);
    }
    editor.history.remap_all(&remap);
    editor.remap_selection(&remap);
    editor.selection.retain(|&e| world.get_entity(e).is_ok());
    let now_values: HashMap<String, Value> = world_resources(world).into_iter().collect();
    for (name, before) in snapshot.resources {
        if now_values.get(&name) == Some(&before) {
            continue;
        }
        let entry = world.resource::<TypeRegistry>().resource(&name).cloned();
        if let Some(Err(e)) = entry.map(|entry| (entry.patch)(world, before)) {
            editor.error(t, format!("Stop could not restore {name}: {e}"));
        }
    }
    editor.history.dirty = snapshot.was_dirty;
    editor.mode = Mode::Edit;
    editor.info(t, "Stopped: scene restored.");
}

/// Spawn the editor camera (not part of the scene).
pub fn spawn_camera(world: &mut World, at: DVec3, target: DVec3) -> Entity {
    let transform = Transform::from_translation(at).looking_at(target, Vec3::Y);
    let fwd = transform.forward();
    world
        .spawn((
            EditorOnly,
            Name::new("Editor camera"),
            Camera {
                order: 1000,
                ..Camera::perspective(60.0)
            },
            EditorCamera::looking(fwd),
            // Modules simulate in full around what the editor looks at.
            engine::world::SimulationFocus::default(),
            transform,
        ))
        .id()
}

/// Mark components a module needs for picking ([`ModuleDemo`] has no mesh, so it gets an icon).
pub fn has_icon(world: &World, e: Entity) -> Option<&'static str> {
    if crate::scatter::painted(world, e) {
        // Hundreds of painted rocks would bury the view in icons; the outliner lists them.
        None
    } else if world.get::<ModuleDemo>(e).is_some() {
        Some("demo")
    } else if world.get::<DirectionalLight>(e).is_some() {
        Some("sun")
    } else if world.get::<PointLight>(e).is_some() {
        Some("light")
    } else if world.get::<Camera>(e).is_some() {
        Some("camera")
    } else if world.get::<crate::models::Model>(e).is_some() {
        Some("model")
    } else if world.get::<MeshRenderer>(e).is_none()
        && world.get::<InScene>(e).is_some()
        && !crate::models::inside_model(world, e)
    {
        Some("empty")
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_count_up() {
        assert_eq!(next_name("Crate"), "Crate 2");
        assert_eq!(next_name("Crate 2"), "Crate 3");
        assert_eq!(next_name("Big crate"), "Big crate 2");
    }

    #[test]
    fn drop_to_ground_lands_on_the_ground_and_undoes() {
        let mut app = App::new();
        app.update();
        let world = &mut app.world;
        world.init_resource::<engine::world::Ground>();
        let e = world
            .spawn((
                InScene,
                Name::new("Crate"),
                Transform::from_xyz(1.0, 5.0, 2.0),
            ))
            .id();
        app.update();
        let world = &mut app.world;
        let camera = world.spawn(EditorOnly).id();
        let mut editor = Editor::new(camera);
        editor.select(Some(e));
        drop_to_ground(world, &mut editor);
        let y = world.get::<Transform>(e).unwrap().translation.y;
        assert!(y.abs() < 1e-3, "{y}");
        undo(world, &mut editor);
        assert_eq!(world.get::<Transform>(e).unwrap().translation.y, 5.0);
    }

    #[derive(Resource, Serialize, Deserialize, Default, Debug, PartialEq)]
    struct Rain {
        amount: f32,
    }

    #[test]
    fn stop_puts_world_settings_back() {
        let mut world = World::new();
        world.insert_resource(Time::default());
        let mut registry = TypeRegistry::default();
        registry.register_resource::<Rain>("test");
        registry.register_resource::<Mouse>("live input");
        world.insert_resource(registry);
        world.insert_resource(Rain { amount: 0.2 });
        world.insert_resource(Mouse::default());
        let camera = world.spawn(EditorOnly).id();
        let mut editor = Editor::new(camera);

        play(&mut world, &mut editor);
        world.resource_mut::<Rain>().amount = 0.9;
        world.resource_mut::<Mouse>().position = Some(Vec2::new(5.0, 6.0));
        stop(&mut world, &mut editor);

        assert_eq!(world.resource::<Rain>().amount, 0.2);
        assert_eq!(
            world.resource::<Mouse>().position,
            Some(Vec2::new(5.0, 6.0))
        );
        assert_eq!(editor.mode, Mode::Edit);
    }
}
