//! The Sequencer tab: cutscenes (`.cutscene` files, [`engine_script::cutscene`]) as tracks on
//! a timeline, made and previewed in the editor (Unreal's Sequencer, Unity's Timeline window,
//! Blender's Dope Sheet).
//!
//! Each object the cutscene moves is a group of rows: its tracks (`position`, `rotation`,
//! `PointLight.intensity_lm`, events) with a diamond at each key, unfolding into a row per
//! axis; the camera cuts are colored sections named after their camera, blends drawn as ramps;
//! events are flags with their names; marks are on the ruler. Click or drag the ruler to
//! scrub: in edit mode the objects are posed as they would be then, and put back when the
//! cutscene closes, before Play and before the scene is saved (as Godot saves a scene in its
//! RESET pose), so a preview never ends up in the scene. While the game plays, the playhead
//! follows the cutscene playing, and ▶ plays it in the game.
//!
//! Keys: click one (Shift adds), drag across empty rows to box select, drag keys to retime
//! them (snapped to frames; Ctrl doesn't snap), Delete removes them, double-click an axis row
//! to edit its curve in the Curves tab. S keys the selected objects' position and rotation at
//! the playhead, making their tracks if they have none (Unreal's S). With Auto key on, moving
//! a bound object keys only the axes that changed, on the tracks it already has (Unreal's Key
//! Changed). ➕ Track adds tracks for the selection: its transform, any number of its
//! components or material, events, or a cut to it if it is a camera. The camera of the cut
//! at the playhead shows its view in the scene view's corner. Each change is saved at once,
//! one undo step a change (Ctrl+Z, Ctrl+Y).

use std::collections::{HashMap, HashSet};

use egui::{
    Align2, Color32, FontId, Key, Modifiers, PointerButton, Pos2, Rect, RichText, Sense, Shape,
    Stroke, StrokeKind, Vec2,
};
use engine::prelude::*;
use engine_script::cutscene::{
    self, Cut, CutTrack, Cutscene, CutsceneFiles, Cutscenes, EventKey, EventTrack, Finished,
    PlayOptions, Saved, Track, ValueTrack,
};

use crate::editor::{self, Editor, Mode, Tab};
use crate::theme;
use crate::ui::EditorDock;

pub struct SequencerTabPlugin;

impl Plugin for SequencerTabPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<SequencerTab>()
            .add_systems(Update, follow_mode.before(UpdateSet::Logic));
    }
}

/// The names column's width; the ruler's and a row's heights.
const NAMES_W: f32 = 270.0;
const RULER_H: f32 = 26.0;
const ROW_H: f32 = 24.0;
/// How near the pointer must be to pick a key.
const HIT_PX: f32 = 7.0;

const PLAYHEAD: Color32 = Color32::from_rgb(0xff, 0x4d, 0x4d);
const KEY: Color32 = Color32::from_rgb(0xd8, 0xdb, 0xe0);
const PICKED: Color32 = Color32::from_rgb(0xff, 0xc8, 0x4a);
const EVENT: Color32 = Color32::from_rgb(0x3f, 0xd8, 0xd0);
const MARK: Color32 = Color32::from_rgb(0xf2, 0xc9, 0x4c);
const ROW_BG: Color32 = Color32::from_rgb(0x19, 0x1a, 0x1d);
const ROW_ALT: Color32 = Color32::from_rgb(0x1c, 0x1d, 0x21);
const GROUP_BG: Color32 = Color32::from_rgb(0x23, 0x25, 0x2a);
const HOLD: Color32 = Color32::from_rgb(0x4a, 0x4e, 0x56);
const SHOTS: [Color32; 5] = [
    Color32::from_rgb(0x2f, 0x6f, 0xb8),
    Color32::from_rgb(0x8a, 0x4f, 0xc0),
    Color32::from_rgb(0x2f, 0x9a, 0x6a),
    Color32::from_rgb(0xb8, 0x6a, 0x2f),
    Color32::from_rgb(0xb0, 0x3f, 0x6a),
];

/// A row of the sheet.
#[derive(Clone, Debug, PartialEq)]
enum Row {
    /// An object's group: its name.
    Group(String),
    Track(usize),
    Axis(usize, String),
}

/// A key on the sheet: its track, axis (none: every axis there) and time.
#[derive(Clone, Debug, PartialEq)]
pub struct Sel {
    pub track: usize,
    pub axis: Option<String>,
    pub t: f64,
}

#[derive(Clone, Debug)]
enum Drag {
    Scrub,
    /// Keys moving: where the pointer went down, the cutscene and selection then.
    Keys {
        from_x: f32,
        base: Box<Cutscene>,
        sel: Vec<Sel>,
    },
    /// A box being drawn from here.
    Box(Pos2),
}

/// The tab's own state.
#[derive(Resource)]
pub struct SequencerTab {
    /// The cutscene open (its path).
    pub open: Option<String>,
    cutscene: Option<Cutscene>,
    committed: Option<Cutscene>,
    seen: u64,
    error: Option<String>,
    /// The playhead, seconds.
    pub time: f64,
    /// Previewing (edit mode): the playhead moves and the objects follow.
    pub playing: bool,
    pub looping: bool,
    /// Moving a bound object keys what changed.
    pub auto_key: bool,
    /// The cut camera's view in the scene view's corner.
    pub camera_view: bool,
    /// Seconds in view (left, right); fit them next frame.
    pub t_view: (f64, f64),
    pub fit: bool,
    pub selected: Vec<Sel>,
    drag: Option<Drag>,
    /// Groups folded, value tracks unfolded into axes.
    pub folded: HashSet<String>,
    pub unfolded: HashSet<usize>,
    undo: Vec<Cutscene>,
    redo: Vec<Cutscene>,
    files: Vec<String>,
    files_next: f64,
    new_name: String,
    event_name: String,
    /// What the preview changed (to put back), and the transforms it last gave each object.
    saved: Saved,
    posed: HashMap<Entity, Transform>,
    /// The playhead as last posed (None: pose again).
    posed_at: Option<f64>,
    /// The camera this tab pinned in the corner.
    pinned: Option<Entity>,
    /// Names it couldn't find, and value tracks' errors.
    pub problems: Vec<String>,
    /// Where things were drawn (shot scripts point at them).
    pub drawn_at: HashMap<String, Rect>,
    last_mode: Option<Mode>,
}

impl Default for SequencerTab {
    fn default() -> Self {
        Self {
            open: None,
            cutscene: None,
            committed: None,
            seen: u64::MAX,
            error: None,
            time: 0.0,
            playing: false,
            looping: true,
            auto_key: false,
            camera_view: true,
            t_view: (-0.2, 5.2),
            fit: true,
            selected: Vec::new(),
            drag: None,
            folded: HashSet::new(),
            unfolded: HashSet::new(),
            undo: Vec::new(),
            redo: Vec::new(),
            files: Vec::new(),
            files_next: 0.0,
            new_name: String::new(),
            event_name: "event".into(),
            saved: Saved::default(),
            posed: HashMap::new(),
            posed_at: None,
            pinned: None,
            problems: Vec::new(),
            drawn_at: HashMap::new(),
            last_mode: None,
        }
    }
}

impl SequencerTab {
    /// Moves the playhead (the objects follow).
    pub fn seek(&mut self, t: f64) {
        self.time = t.max(0.0);
        self.posed_at = None;
    }
}

/// Opens a cutscene in the tab and brings the tab to the front (putting back what the last one
/// posed).
pub fn open(world: &mut World, path: &str) {
    put_back(world);
    let path = cutscene::normalize(path);
    let mut tab = world.resource_mut::<SequencerTab>();
    if tab.open.as_deref() != Some(path.as_str()) {
        tab.open = Some(path);
        tab.cutscene = None;
        tab.committed = None;
        tab.seen = u64::MAX;
        tab.selected.clear();
        tab.undo.clear();
        tab.redo.clear();
        tab.unfolded.clear();
        tab.fit = true;
        tab.time = 0.0;
    }
    to_front(world);
}

fn to_front(world: &mut World) {
    let mut dock = world.resource_mut::<EditorDock>();
    if let Some(at) = dock.0.find_tab(&Tab::Sequencer) {
        let _ = dock.0.set_active_tab(at);
    }
}

/// Puts back everything the preview posed (before Play and before the scene is saved, so a
/// preview never ends up in the scene); it poses again on the next frame in edit mode.
pub fn put_back(world: &mut World) {
    let Some(mut tab) = world.get_resource_mut::<SequencerTab>() else {
        return;
    };
    let mut saved = std::mem::take(&mut tab.saved);
    tab.posed.clear();
    tab.posed_at = None;
    saved.restore(world, true);
}

/// Play started: the preview is put back (the editor's play hook did it before its
/// snapshot); stopped: pose again.
fn follow_mode(world: &mut World) {
    let Some(mode) = world.get_resource::<Editor>().map(|e| e.mode) else {
        return;
    };
    let last = world.resource_mut::<SequencerTab>().last_mode.replace(mode);
    if last != Some(mode) {
        if mode != Mode::Edit {
            put_back(world);
            world.resource_mut::<SequencerTab>().playing = false;
        } else {
            world.resource_mut::<SequencerTab>().posed_at = None;
        }
    }
}

/// The entity playing the open cutscene: a [`cutscene::CutscenePlayer`] of it (names are
/// looked up below it first).
fn player_of(world: &mut World, path: &str) -> Option<Entity> {
    let mut q = world.query::<(Entity, &cutscene::CutscenePlayer)>();
    q.iter(world)
        .find(|(_, p)| cutscene::normalize(&p.path) == path)
        .map(|(e, _)| e)
}

/// The tab.
pub fn panel(ui: &mut egui::Ui, world: &mut World, editor: &mut Editor) {
    let Some(mut tab) = world.remove_resource::<SequencerTab>() else {
        return;
    };
    tab.drawn_at.clear();
    let now = ui.input(|i| i.time);
    let root = engine_script::scripts_root(world);
    if now >= tab.files_next {
        tab.files = cutscene::find_files(&root);
        tab.files_next = now + 2.0;
        if tab.open.is_none()
            && let Some(first) = tab.files.first()
        {
            tab.open = Some(first.clone());
        }
    }
    follow_file(world, &mut tab);
    let at = tab.open.clone().and_then(|p| player_of(world, &p));
    let playing_id = tab.open.as_ref().and_then(|path| {
        world.get_resource::<Cutscenes>().and_then(|c| {
            c.playing
                .iter()
                .find(|p| &p.path == path && !p.done)
                .map(|p| (p.id, p.time, p.view))
        })
    });
    if editor.mode == Mode::Edit {
        preview(ui, world, editor, &mut tab, at);
    } else {
        // The game's view of it in the corner (the scene view stays the editor's).
        let view = playing_id.and_then(|p| p.2).filter(|_| tab.camera_view);
        if let Some((_, t, _)) = playing_id {
            tab.time = t;
        }
        pin(world, &mut tab, view);
    }
    toolbar(ui, world, editor, &mut tab, at, playing_id.map(|p| p.0));
    ui.separator();
    sheet(ui, world, editor, &mut tab, at);
    commit(world, editor, &mut tab, ui.input(|i| i.pointer.any_down()));
    world.insert_resource(tab);
}

/// Keeps the open cutscene in step with its file (changed outside, or reloaded).
fn follow_file(world: &mut World, tab: &mut SequencerTab) {
    let Some(path) = tab.open.clone() else {
        return;
    };
    if tab.drag.is_some() {
        return;
    }
    let root = engine_script::scripts_root(world);
    let mut files = world.get_resource_or_insert_with(CutsceneFiles::default);
    files.poll();
    if tab.cutscene.is_some() && files.version == tab.seen {
        return;
    }
    tab.seen = files.version;
    match files.get(&root, &path) {
        Ok(c) => {
            tab.error = None;
            if tab.committed.as_ref() != Some(&*c) {
                tab.cutscene = Some((*c).clone());
                tab.committed = Some((*c).clone());
                tab.posed_at = None;
            }
        }
        Err(e) => {
            tab.error = Some(e);
            tab.cutscene = None;
        }
    }
}

/// Sends a changed cutscene to the players at once; writes its file, as one undo step, once
/// the mouse is let go.
fn commit(world: &mut World, editor: &mut Editor, tab: &mut SequencerTab, pointer_down: bool) {
    let (Some(path), Some(c)) = (tab.open.clone(), tab.cutscene.clone()) else {
        return;
    };
    let root = engine_script::scripts_root(world);
    let mut files = world.get_resource_or_insert_with(CutsceneFiles::default);
    if files.loaded(&path).as_deref() != Some(&c) {
        files.put(&root, &path, c.clone());
        tab.seen = files.version;
        tab.posed_at = None;
    }
    if pointer_down || tab.committed.as_ref() == Some(&c) {
        return;
    }
    if let Some(before) = tab.committed.replace(c.clone()) {
        tab.undo.push(before);
        if tab.undo.len() > 200 {
            tab.undo.remove(0);
        }
    }
    tab.redo.clear();
    save(world, editor, tab);
}

fn save(world: &mut World, editor: &mut Editor, tab: &mut SequencerTab) {
    let (Some(path), Some(c)) = (tab.open.clone(), tab.cutscene.clone()) else {
        return;
    };
    let root = engine_script::scripts_root(world);
    let mut files = world.get_resource_or_insert_with(CutsceneFiles::default);
    match files.set(&root, &path, c) {
        Ok(()) => tab.seen = files.version,
        Err(e) => {
            let t = editor::now(world);
            editor.error(t, e);
        }
    }
}

fn undo(world: &mut World, editor: &mut Editor, tab: &mut SequencerTab, back: bool) {
    let (from, to) = if back {
        (&mut tab.undo, &mut tab.redo)
    } else {
        (&mut tab.redo, &mut tab.undo)
    };
    let Some(c) = from.pop() else {
        return;
    };
    if let Some(now) = tab.cutscene.replace(c.clone()) {
        to.push(now);
    }
    tab.committed = Some(c);
    tab.selected.clear();
    tab.posed_at = None;
    save(world, editor, tab);
}

/// Makes a new cutscene file and opens it.
fn new_file(world: &mut World, editor: &mut Editor, tab: &mut SequencerTab, name: &str) {
    let root = engine_script::scripts_root(world);
    let name = name.trim().trim_start_matches("cutscenes/");
    let path = cutscene::normalize(&format!("{}/{name}", cutscene::FOLDER));
    let t = editor::now(world);
    if root.join(&path).exists() {
        editor.error(t, format!("{path} already exists"));
    } else {
        let mut files = world.get_resource_or_insert_with(CutsceneFiles::default);
        if let Err(e) = files.set(&root, &path, Cutscene::default()) {
            editor.error(t, e);
            return;
        }
        editor.info(t, format!("Made {path}"));
    }
    tab.files_next = 0.0;
    tab.open = Some(path);
    tab.cutscene = None;
    tab.committed = None;
    tab.seen = u64::MAX;
    tab.undo.clear();
    tab.redo.clear();
    tab.selected.clear();
    tab.time = 0.0;
    tab.fit = true;
}

// ----------------------------------------------------------------------------- preview

/// Edit mode: keys what the user moved (auto key), moves the playhead while previewing, poses
/// the objects at it, and shows the cut camera in the corner.
fn preview(
    ui: &egui::Ui,
    world: &mut World,
    editor: &mut Editor,
    tab: &mut SequencerTab,
    at: Option<Entity>,
) {
    let Some(mut c) = tab.cutscene.clone() else {
        pin(world, tab, None);
        return;
    };
    // What the user moved since the last pose.
    let moved: Vec<(Entity, Transform, Transform)> = tab
        .posed
        .iter()
        .filter_map(|(e, was)| {
            let now = *world.get::<Transform>(*e)?;
            (now != *was).then_some((*e, *was, now))
        })
        .collect();
    if !moved.is_empty() {
        if tab.auto_key && !tab.playing {
            let keyed = auto_key(world, &mut c, tab.time, at, &moved);
            if keyed > 0 {
                tab.cutscene = Some(c.clone());
                let t = editor::now(world);
                editor.info(t, format!("Auto key: {keyed} keys at {:.2} s", tab.time));
            }
        }
        for (e, _, now) in moved {
            tab.posed.insert(e, now);
        }
    }
    if tab.playing {
        let dt = ui.input(|i| i.stable_dt).min(0.25) as f64;
        tab.time += dt;
        if tab.time > c.length {
            if tab.looping {
                tab.time = 0.0;
            } else {
                tab.time = c.length;
                tab.playing = false;
            }
        }
        tab.posed_at = None;
    }
    pose_now(world, tab, &c, at);
    trails(world, editor, &c, at);
    let camera = tab
        .camera_view
        .then(|| c.cut_at(tab.time).map(|(_, cut)| cut.camera.clone()))
        .flatten()
        .and_then(|name| cutscene::resolve(world, &name, at, &HashMap::new()));
    pin(world, tab, camera);
}

/// Poses the objects at the playhead unless they are posed there already.
fn pose_now(world: &mut World, tab: &mut SequencerTab, c: &Cutscene, at: Option<Entity>) {
    if tab.posed_at == Some(tab.time) {
        return;
    }
    tab.problems = cutscene::pose(world, c, tab.time, at, &HashMap::new(), &mut tab.saved);
    tab.posed = tab
        .saved
        .entities()
        .into_iter()
        .filter_map(|e| Some((e, *world.get::<Transform>(e)?)))
        .collect();
    tab.posed_at = Some(tab.time);
}

/// Where each object the cutscene moves goes, as Unreal's motion trails show it: a line
/// through where it is each frame and a tick at each key, bright for the selection.
fn trails(world: &mut World, editor: &Editor, c: &Cutscene, at: Option<Entity>) {
    let mut lines: Vec<(DVec3, DVec3, [f32; 4], bool)> = Vec::new();
    let mut ticks: Vec<(DVec3, [f32; 4], bool)> = Vec::new();
    for track in &c.tracks {
        let Track::Value(v) = track else {
            continue;
        };
        if v.prop != "position" || v.mute {
            continue;
        }
        let Some(e) = cutscene::resolve(world, &v.bind, at, &HashMap::new()) else {
            continue;
        };
        let Some(now) = world.get::<Transform>(e).copied() else {
            continue;
        };
        let parent = world
            .get::<ChildOf>(e)
            .and_then(|p| world.get::<GlobalTransform>(p.parent()))
            .map(|g| g.0);
        let at_t = |t: f64| {
            let mut p = now.translation;
            for (axis, x) in v.sample(t) {
                match axis.as_str() {
                    "x" => p.x = x,
                    "y" => p.y = x,
                    "z" => p.z = x,
                    _ => {}
                }
            }
            parent.map_or(p, |g| g.transform_point3(p))
        };
        let picked = editor.is_selected(e);
        let color = if picked {
            [1.0, 0.78, 0.29, 0.95]
        } else {
            [0.78, 0.82, 0.9, 0.4]
        };
        let n = ((c.length * c.fps.max(1.0)).ceil() as usize).clamp(1, 900);
        let mut last = at_t(0.0);
        for i in 1..=n {
            let p = at_t(c.length * i as f64 / n as f64);
            lines.push((last, p, color, picked));
            last = p;
        }
        for t in track.times() {
            ticks.push((at_t(t), color, picked));
        }
    }
    let mut debug = world.resource_mut::<engine::debug::DebugDraw>();
    for (a, b, color, top) in lines {
        let l = debug.line(a, b, color);
        if top {
            l.on_top();
        }
    }
    for (p, color, top) in ticks {
        let c = debug.cross(p, 0.14, color);
        if top {
            c.on_top();
        }
    }
}

/// Poses the cutscene at `path` again at the playhead if it is the one open (the Curves tab
/// changed one of its axes while this tab is hidden behind it).
pub fn repose(world: &mut World, editor: &Editor, path: &str) {
    if editor.mode != Mode::Edit {
        return;
    }
    let Some(mut tab) = world.remove_resource::<SequencerTab>() else {
        return;
    };
    if tab.open.as_deref() == Some(path) {
        follow_file(world, &mut tab);
        let at = player_of(world, path);
        if let Some(c) = tab.cutscene.clone() {
            pose_now(world, &mut tab, &c, at);
        }
    }
    world.insert_resource(tab);
}

/// Shows `camera`'s view in the scene view's corner (only replacing a pin this tab made).
fn pin(world: &mut World, tab: &mut SequencerTab, camera: Option<Entity>) {
    let Some(mut p) = world.get_resource_mut::<crate::camera_preview::CameraPreview>() else {
        return;
    };
    if p.pinned == camera {
        tab.pinned = camera;
        return;
    }
    if p.pinned.is_none() || p.pinned == tab.pinned {
        p.pinned = camera;
        tab.pinned = camera;
    }
}

/// A transform's axes for `prop` (degrees for a rotation), as tracks key them.
fn axes_of(t: &Transform, prop: &str) -> Vec<(String, f64)> {
    match prop {
        "position" => vec![
            ("x".into(), t.translation.x),
            ("y".into(), t.translation.y),
            ("z".into(), t.translation.z),
        ],
        "rotation" => {
            let (yaw, pitch, roll) = t.rotation.to_euler(EulerRot::YXZ);
            vec![
                ("yaw".into(), yaw.to_degrees() as f64),
                ("pitch".into(), pitch.to_degrees() as f64),
                ("roll".into(), roll.to_degrees() as f64),
            ]
        }
        "scale" => vec![
            ("x".into(), t.scale.x as f64),
            ("y".into(), t.scale.y as f64),
            ("z".into(), t.scale.z as f64),
        ],
        _ => Vec::new(),
    }
}

/// Keys the axes the user changed on the transform tracks the moved objects already have.
/// Returns how many keys it set.
fn auto_key(
    world: &mut World,
    c: &mut Cutscene,
    t: f64,
    at: Option<Entity>,
    moved: &[(Entity, Transform, Transform)],
) -> usize {
    let mut keyed = 0;
    let fps = c.fps;
    let binds: Vec<(usize, Option<Entity>)> = c
        .tracks
        .iter()
        .enumerate()
        .map(|(i, tr)| {
            let e = match tr {
                Track::Value(v) => cutscene::resolve(world, &v.bind, at, &HashMap::new()),
                _ => None,
            };
            (i, e)
        })
        .collect();
    for (i, e) in binds {
        let Some((_, was, now)) = moved.iter().find(|m| Some(m.0) == e) else {
            continue;
        };
        let Track::Value(v) = &mut c.tracks[i] else {
            continue;
        };
        let before = axes_of(was, &v.prop);
        for (axis, value) in axes_of(now, &v.prop) {
            let old = before.iter().find(|(a, _)| *a == axis).map(|(_, x)| *x);
            let differs = old.is_none_or(|o| {
                let d = value - o;
                if v.prop == "rotation" {
                    (d - (d / 360.0).round() * 360.0).abs() > 1e-3
                } else {
                    d.abs() > 1e-6
                }
            });
            if differs {
                v.key(&axis, t, value, fps);
                keyed += 1;
            }
        }
    }
    keyed
}

/// Keys the selected objects' position and rotation at the playhead, making their tracks.
fn key_selection(world: &mut World, editor: &mut Editor, tab: &mut SequencerTab) {
    let Some(c) = tab.cutscene.as_mut() else {
        return;
    };
    let mut keyed = 0;
    for &e in &editor.selection {
        let Some(name) = world.get::<Name>(e).map(|n| n.as_str().to_string()) else {
            continue;
        };
        let Some(t) = world.get::<Transform>(e).copied() else {
            continue;
        };
        for prop in ["position", "rotation"] {
            let i =
                match c.tracks.iter().position(
                    |tr| matches!(tr, Track::Value(v) if v.bind == name && v.prop == prop),
                ) {
                    Some(i) => i,
                    None => {
                        c.tracks.push(Track::Value(ValueTrack {
                            bind: name.clone(),
                            prop: prop.into(),
                            ..Default::default()
                        }));
                        c.tracks.len() - 1
                    }
                };
            let fps = c.fps;
            if let Track::Value(v) = &mut c.tracks[i] {
                for (axis, value) in axes_of(&t, prop) {
                    v.key(&axis, tab.time, value, fps);
                    keyed += 1;
                }
            }
        }
        // A scale track the object has gets a key too.
        let fps = c.fps;
        for tr in &mut c.tracks {
            if let Track::Value(v) = tr
                && v.bind == name
                && v.prop == "scale"
            {
                for (axis, value) in axes_of(&t, "scale") {
                    v.key(&axis, tab.time, value, fps);
                    keyed += 1;
                }
            }
        }
    }
    let now = editor::now(world);
    if keyed == 0 {
        editor.error(now, "Select objects to key their position and rotation");
    } else {
        editor.info(now, format!("Keyed {keyed} values at {:.2} s", tab.time));
        // The keyed values are the pose now: no put back over them.
        for &e in &editor.selection {
            tab.saved.forget(e);
        }
    }
}

/// Adds a value track for `e`'s `prop`, keyed with its value at the playhead.
fn add_value_track(world: &mut World, tab: &mut SequencerTab, e: Entity, prop: &str) {
    let Some(name) = world.get::<Name>(e).map(|n| n.as_str().to_string()) else {
        return;
    };
    let axes = cutscene::read_axes(world, e, prop).unwrap_or_default();
    let Some(c) = tab.cutscene.as_mut() else {
        return;
    };
    if c.tracks
        .iter()
        .any(|tr| matches!(tr, Track::Value(v) if v.bind == name && v.prop == prop))
    {
        return;
    }
    let mut v = ValueTrack {
        bind: name,
        prop: prop.into(),
        ..Default::default()
    };
    for (axis, value) in axes {
        v.key(&axis, tab.time, value, c.fps);
    }
    c.tracks.push(Track::Value(v));
    tab.unfolded.insert(c.tracks.len() - 1);
}

// ----------------------------------------------------------------------------- toolbar

fn toolbar(
    ui: &mut egui::Ui,
    world: &mut World,
    editor: &mut Editor,
    tab: &mut SequencerTab,
    at: Option<Entity>,
    playing_id: Option<u64>,
) {
    ui.horizontal(|ui| {
        let label = tab.open.clone().unwrap_or_else(|| "No cutscene".into());
        let r = egui::ComboBox::from_id_salt("sequencer file")
            .selected_text(RichText::new(&label).strong().color(theme::TEXT))
            .width(230.0)
            .show_ui(ui, |ui| {
                let files = tab.files.clone();
                for f in files {
                    if ui
                        .selectable_label(tab.open.as_deref() == Some(f.as_str()), &f)
                        .clicked()
                        && tab.open.as_deref() != Some(f.as_str())
                    {
                        let mut saved = std::mem::take(&mut tab.saved);
                        saved.restore(world, true);
                        tab.posed.clear();
                        tab.open = Some(f.clone());
                        tab.cutscene = None;
                        tab.committed = None;
                        tab.seen = u64::MAX;
                        tab.selected.clear();
                        tab.undo.clear();
                        tab.redo.clear();
                        tab.time = 0.0;
                        tab.fit = true;
                    }
                }
            });
        tab.drawn_at.insert("file".into(), r.response.rect);
        ui.menu_button("➕ New", |ui| {
            ui.label(theme::dim("A new cutscene in cutscenes/"));
            ui.text_edit_singleline(&mut tab.new_name);
            if ui
                .add_enabled(
                    !tab.new_name.trim().is_empty(),
                    egui::Button::new("Make it"),
                )
                .clicked()
            {
                let name = tab.new_name.clone();
                new_file(world, editor, tab, &name);
                tab.new_name.clear();
                ui.close();
            }
        });
        ui.separator();
        let length = tab.cutscene.as_ref().map_or(0.0, |c| c.length);
        let fps = tab.cutscene.as_ref().map_or(30.0, |c| c.fps);
        if ui
            .button("⏮")
            .on_hover_text("To the start (Home)")
            .clicked()
        {
            tab.seek(0.0);
        }
        let in_game = editor.mode != Mode::Edit;
        let playing = if in_game {
            playing_id.is_some()
        } else {
            tab.playing
        };
        let play_label = if playing { "⏸" } else { "▶" };
        let tip = if in_game {
            "Play it in the game, from the playhead (its events go out, its cameras cut)"
        } else {
            "Preview it here (Space): the objects move, events wait for the game"
        };
        let r = ui.button(play_label).on_hover_text(tip);
        tab.drawn_at.insert("play".into(), r.rect);
        if r.clicked() {
            if in_game {
                match (playing_id, tab.open.clone()) {
                    (Some(id), _) => cutscene::stop(world, id),
                    (None, Some(path)) => {
                        let opts = PlayOptions {
                            from: if tab.time >= length - 1e-6 {
                                0.0
                            } else {
                                tab.time
                            },
                            at,
                            ..Default::default()
                        };
                        if let Err(e) = cutscene::play(world, &path, opts) {
                            let t = editor::now(world);
                            editor.error(t, e);
                        }
                    }
                    _ => {}
                }
            } else {
                if !tab.playing && tab.time >= length - 1e-6 {
                    tab.time = 0.0;
                }
                tab.playing = !tab.playing;
            }
        }
        if ui.button("⏭").on_hover_text("To the end (End)").clicked() {
            tab.seek(length);
            tab.playing = false;
        }
        ui.toggle_value(&mut tab.looping, "↺")
            .on_hover_text("Loop the preview");
        let frame = (tab.time * fps).round() as i64;
        ui.label(
            RichText::new(format!(
                "{:.2} / {:.2} s  ·  frame {frame}",
                tab.time, length
            ))
            .monospace()
            .color(theme::TEXT),
        );
        ui.separator();
        let r = ui
            .button("Key (S)")
            .on_hover_text("Key the selected objects' position and rotation at the playhead");
        tab.drawn_at.insert("key".into(), r.rect);
        if r.clicked() {
            key_selection(world, editor, tab);
        }
        let color = if tab.auto_key { PLAYHEAD } else { theme::TEXT };
        let r = ui
            .toggle_value(&mut tab.auto_key, RichText::new("⏺ Auto key").color(color))
            .on_hover_text(
                "Moving an object keys the axes that changed, on the tracks it already has",
            );
        tab.drawn_at.insert("auto key".into(), r.rect);
        add_menu(ui, world, editor, tab);
        ui.separator();
        if let Some(c) = tab.cutscene.as_mut() {
            ui.label(theme::dim("Length"));
            ui.add(
                egui::DragValue::new(&mut c.length)
                    .range(0.1..=600.0)
                    .speed(0.05)
                    .suffix(" s"),
            );
            ui.label(theme::dim("Bars"));
            let mut bars = c.aspect > 0.0;
            if ui
                .checkbox(&mut bars, "")
                .on_hover_text("Letterbox to 2.39:1 while it looks through its cameras")
                .changed()
            {
                c.aspect = if bars { 2.39 } else { 0.0 };
            }
        }
        ui.toggle_value(&mut tab.camera_view, "🎥")
            .on_hover_text("Show the cut camera's view in the scene view's corner");
        if ui
            .button("⛶ Fit")
            .on_hover_text("F: all of it in view")
            .clicked()
        {
            tab.fit = true;
        }
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if let Some(err) = &tab.error {
                ui.colored_label(theme::WARN, err);
            } else if !tab.problems.is_empty() {
                ui.colored_label(
                    theme::WARN,
                    format!("Not found: {}", tab.problems.join(", ")),
                )
                .on_hover_text("No object of that name in the scene (or it can't take the value)");
            } else if tab.open.is_some() {
                ui.label(theme::dim("Saved as you edit"));
            }
        });
    });
}

/// ➕ Track: tracks for the selected object.
fn add_menu(ui: &mut egui::Ui, world: &mut World, editor: &mut Editor, tab: &mut SequencerTab) {
    let r = ui.menu_button("➕ Track", |ui| {
        let Some(e) = editor.primary() else {
            ui.label(theme::dim("Select an object to add its tracks"));
            if ui.button("Camera cuts").clicked() {
                if let Some(c) = tab.cutscene.as_mut()
                    && !c.tracks.iter().any(|t| matches!(t, Track::Cuts(_)))
                {
                    c.tracks.insert(0, Track::Cuts(CutTrack::default()));
                }
                ui.close();
            }
            return;
        };
        let name = crate::scene_ops::label(world, e);
        ui.label(RichText::new(&name).strong());
        for prop in ["position", "rotation", "scale"] {
            if ui.button(prop).clicked() {
                add_value_track(world, tab, e, prop);
                ui.close();
            }
        }
        let props: Vec<String> = cutscene::keyable(world, e).into_iter().skip(3).collect();
        ui.add_enabled_ui(!props.is_empty(), |ui| {
            ui.menu_button("A value", |ui| {
                egui::ScrollArea::vertical()
                    .max_height(320.0)
                    .show(ui, |ui| {
                        for prop in &props {
                            if ui.button(prop).clicked() {
                                add_value_track(world, tab, e, prop);
                                ui.close();
                            }
                        }
                    });
            });
        });
        ui.separator();
        ui.horizontal(|ui| {
            ui.label("Event");
            ui.text_edit_singleline(&mut tab.event_name);
        });
        if ui
            .add_enabled(
                !tab.event_name.trim().is_empty(),
                egui::Button::new("Send it here, at the playhead"),
            )
            .on_hover_text("A message to its script: a method of that name is called")
            .clicked()
        {
            let event = tab.event_name.trim().to_string();
            let t = tab.time;
            if let Some(c) = tab.cutscene.as_mut() {
                let i = c
                    .tracks
                    .iter()
                    .position(|tr| matches!(tr, Track::Events(ev) if ev.bind == name));
                let key = EventKey {
                    t,
                    event,
                    ..Default::default()
                };
                match i {
                    Some(i) => {
                        if let Track::Events(ev) = &mut c.tracks[i] {
                            ev.events.push(key);
                            ev.events.sort_by(|a, b| a.t.total_cmp(&b.t));
                        }
                    }
                    None => c.tracks.push(Track::Events(EventTrack {
                        bind: name.clone(),
                        events: vec![key],
                        ..Default::default()
                    })),
                }
            }
            ui.close();
        }
        if world.get::<Camera>(e).is_some() {
            ui.separator();
            if ui.button("Cut to this camera here").clicked() {
                let t = tab.time;
                if let Some(c) = tab.cutscene.as_mut() {
                    let i = match c.tracks.iter().position(|t| matches!(t, Track::Cuts(_))) {
                        Some(i) => i,
                        None => {
                            c.tracks.insert(0, Track::Cuts(CutTrack::default()));
                            0
                        }
                    };
                    if let Track::Cuts(cuts) = &mut c.tracks[i] {
                        cuts.cuts.retain(|k| (k.t - t).abs() > 1e-6);
                        cuts.cuts.push(Cut {
                            t,
                            camera: name.clone(),
                            blend: 0.0,
                        });
                        cuts.cuts.sort_by(|a, b| a.t.total_cmp(&b.t));
                    }
                }
                ui.close();
            }
        }
    });
    tab.drawn_at.insert("add track".into(), r.response.rect);
}

// ----------------------------------------------------------------------------- the sheet

/// The rows: the camera cuts first, then each object's group with its tracks (and their
/// axes when unfolded).
fn rows(c: &Cutscene, tab: &SequencerTab) -> Vec<Row> {
    let mut out = Vec::new();
    for (i, t) in c.tracks.iter().enumerate() {
        if matches!(t, Track::Cuts(_)) {
            out.push(Row::Track(i));
        }
    }
    let mut groups: Vec<String> = Vec::new();
    for t in &c.tracks {
        if !matches!(t, Track::Cuts(_)) && !groups.iter().any(|g| g == t.bind()) {
            groups.push(t.bind().to_string());
        }
    }
    for g in groups {
        out.push(Row::Group(g.clone()));
        if tab.folded.contains(&g) {
            continue;
        }
        for (i, t) in c.tracks.iter().enumerate() {
            if matches!(t, Track::Cuts(_)) || t.bind() != g {
                continue;
            }
            out.push(Row::Track(i));
            if let Track::Value(v) = t
                && tab.unfolded.contains(&i)
            {
                for axis in v.axes() {
                    out.push(Row::Axis(i, axis));
                }
            }
        }
    }
    out
}

fn group_label(name: &str) -> String {
    if name.is_empty() {
        "The player's script".into()
    } else {
        name.to_string()
    }
}

/// The times a row has keys at.
fn row_times(c: &Cutscene, row: &Row) -> Vec<f64> {
    match row {
        Row::Group(g) => {
            let mut out: Vec<f64> = c
                .tracks
                .iter()
                .filter(|t| !matches!(t, Track::Cuts(_)) && t.bind() == g)
                .flat_map(|t| t.times())
                .collect();
            out.sort_by(f64::total_cmp);
            out.dedup_by(|a, b| (*a - *b).abs() < 1e-6);
            out
        }
        Row::Track(i) => c.tracks[*i].times(),
        Row::Axis(i, axis) => match &c.tracks[*i] {
            Track::Value(v) => v
                .channels
                .get(axis)
                .map(|ch| ch.keys.iter().map(|k| k.t).collect())
                .unwrap_or_default(),
            _ => Vec::new(),
        },
    }
}

/// The keys a click on a row's diamond at `t` picks.
fn sels_for(c: &Cutscene, row: &Row, t: f64) -> Vec<Sel> {
    match row {
        Row::Group(g) => c
            .tracks
            .iter()
            .enumerate()
            .filter(|(_, tr)| !matches!(tr, Track::Cuts(_)) && tr.bind() == g)
            .filter(|(_, tr)| tr.times().iter().any(|x| (x - t).abs() < 1e-6))
            .map(|(i, _)| Sel {
                track: i,
                axis: None,
                t,
            })
            .collect(),
        Row::Track(i) => vec![Sel {
            track: *i,
            axis: None,
            t,
        }],
        Row::Axis(i, a) => vec![Sel {
            track: *i,
            axis: Some(a.clone()),
            t,
        }],
    }
}

fn is_selected(tab: &SequencerTab, row: &Row, t: f64) -> bool {
    let near = |s: &Sel| (s.t - t).abs() < 1e-6;
    match row {
        Row::Group(_) => false,
        Row::Track(i) => tab.selected.iter().any(|s| s.track == *i && near(s)),
        Row::Axis(i, a) => tab
            .selected
            .iter()
            .any(|s| s.track == *i && near(s) && s.axis.as_ref().is_none_or(|x| x == a)),
    }
}

/// Moves the selected keys by `dt` (from `base`).
fn retime(base: &Cutscene, sel: &[Sel], dt: f64) -> Cutscene {
    let mut c = base.clone();
    // Each key once, in an order that never lands one on another before it moves.
    let mut order: Vec<&Sel> = sel.iter().collect();
    order.sort_by(|a, b| {
        if dt > 0.0 {
            b.t.total_cmp(&a.t)
        } else {
            a.t.total_cmp(&b.t)
        }
    });
    for s in order {
        let to = (s.t + dt).max(0.0);
        match (&mut c.tracks[s.track], &s.axis) {
            (Track::Value(v), Some(axis)) => {
                if let Some(ch) = v.channels.get_mut(axis) {
                    for k in ch.keys.iter_mut().filter(|k| (k.t - s.t).abs() < 1e-6) {
                        k.t = to;
                    }
                    ch.sort();
                }
            }
            (tr, _) => tr.retime(s.t, to),
        }
    }
    c
}

fn delete_selected(tab: &mut SequencerTab) {
    let Some(c) = tab.cutscene.as_mut() else {
        return;
    };
    for s in std::mem::take(&mut tab.selected) {
        let Some(tr) = c.tracks.get_mut(s.track) else {
            continue;
        };
        match (tr, &s.axis) {
            (Track::Value(v), Some(axis)) => {
                if let Some(ch) = v.channels.get_mut(axis) {
                    ch.keys.retain(|k| (k.t - s.t).abs() > 1e-6);
                }
                v.channels.retain(|_, ch| !ch.keys.is_empty());
            }
            (tr, _) => {
                tr.remove_at(s.t);
            }
        }
    }
    tab.posed_at = None;
}

fn nice_step(px_per_sec: f64) -> f64 {
    let min = 70.0 / px_per_sec.max(1e-9);
    [0.05, 0.1, 0.25, 0.5, 1.0, 2.0, 5.0, 10.0, 30.0, 60.0]
        .into_iter()
        .find(|s| *s >= min)
        .unwrap_or(120.0)
}

fn shot_color(camera: &str) -> Color32 {
    let h = camera
        .bytes()
        .fold(7u32, |h, b| h.wrapping_mul(31).wrapping_add(b as u32));
    SHOTS[(h % SHOTS.len() as u32) as usize]
}

fn diamond(p: &egui::Painter, at: Pos2, r: f32, fill: Color32, square: bool) {
    let pts = if square {
        vec![
            Pos2::new(at.x - r * 0.8, at.y - r * 0.8),
            Pos2::new(at.x + r * 0.8, at.y - r * 0.8),
            Pos2::new(at.x + r * 0.8, at.y + r * 0.8),
            Pos2::new(at.x - r * 0.8, at.y + r * 0.8),
        ]
    } else {
        vec![
            Pos2::new(at.x, at.y - r),
            Pos2::new(at.x + r, at.y),
            Pos2::new(at.x, at.y + r),
            Pos2::new(at.x - r, at.y),
        ]
    };
    p.add(Shape::convex_polygon(
        pts,
        fill,
        Stroke::new(1.0, Color32::from_black_alpha(160)),
    ));
}

/// What a value track's object shows now, by axis.
fn value_text(world: &mut World, v: &ValueTrack, at: Option<Entity>) -> String {
    let Some(e) = cutscene::resolve(world, &v.bind, at, &HashMap::new()) else {
        return "not found".into();
    };
    match cutscene::read_axes(world, e, &v.prop) {
        Ok(axes) if axes.len() == 1 => format!("{:.2}", axes[0].1),
        Ok(axes) => axes
            .iter()
            .map(|(_, x)| format!("{x:.1}"))
            .collect::<Vec<_>>()
            .join(", "),
        Err(e) => e,
    }
}

fn axis_color(axis: &str) -> Color32 {
    match axis {
        "x" | "yaw" => theme::AXIS_COLORS[0],
        "y" | "pitch" => theme::AXIS_COLORS[1],
        "z" | "roll" => theme::AXIS_COLORS[2],
        _ => theme::TEXT,
    }
}

fn sheet(
    ui: &mut egui::Ui,
    world: &mut World,
    editor: &mut Editor,
    tab: &mut SequencerTab,
    at: Option<Entity>,
) {
    let outer = ui.available_rect_before_wrap();
    let Some(c) = tab.cutscene.clone() else {
        ui.allocate_rect(outer, Sense::hover());
        let text = match (&tab.error, &tab.open) {
            (Some(e), _) => e.clone(),
            (None, None) => "No cutscenes yet: ➕ New makes one in cutscenes/".into(),
            _ => "Loading".into(),
        };
        ui.painter().text(
            outer.center(),
            Align2::CENTER_CENTER,
            text,
            FontId::proportional(14.0),
            theme::TEXT_DIM,
        );
        return;
    };
    let canvas_x = outer.left() + NAMES_W;
    let width = (outer.right() - canvas_x - 10.0).max(60.0);
    if tab.fit {
        let end = c.length.max(c.last_key()).max(0.5);
        tab.t_view = (-end * 0.02, end * 1.03);
        tab.fit = false;
    }
    let (t0, t1) = tab.t_view;
    let px = width as f64 / (t1 - t0);
    let x_of = |t: f64| canvas_x + ((t - t0) * px) as f32;
    let t_of = |x: f32| t0 + (x - canvas_x) as f64 / px;
    let snap = |t: f64, free: bool| {
        if free {
            t.max(0.0)
        } else {
            ((t * c.fps).round() / c.fps).max(0.0)
        }
    };
    let ruler = Rect::from_min_size(Pos2::new(canvas_x, outer.top()), Vec2::new(width, RULER_H));
    tab.drawn_at.insert("ruler".into(), ruler);
    let free = ui.input(|i| i.modifiers.command);

    // The ruler: seconds, frames, the cutscene's end, the marks, the playhead.
    {
        let p = ui.painter();
        p.rect_filled(ruler, 0.0, theme::PANEL);
        let end = x_of(c.length);
        if end < ruler.right() {
            p.rect_filled(
                Rect::from_x_y_ranges(end.max(ruler.left())..=ruler.right(), ruler.y_range()),
                0.0,
                Color32::from_black_alpha(90),
            );
        }
        let step = nice_step(px);
        let mut t = (t0 / step).ceil() * step;
        while t <= t1 {
            let x = x_of(t);
            p.vline(
                x,
                ruler.bottom() - 8.0..=ruler.bottom(),
                Stroke::new(1.0, theme::TEXT_DIM),
            );
            p.text(
                Pos2::new(x + 3.0, ruler.top() + 2.0),
                Align2::LEFT_TOP,
                format!("{} s", (t * 100.0).round() / 100.0),
                FontId::proportional(11.0),
                theme::TEXT_DIM,
            );
            t += step;
        }
        for m in &c.marks {
            let x = x_of(m.t);
            if x < ruler.left() || x > ruler.right() {
                continue;
            }
            p.add(Shape::convex_polygon(
                vec![
                    Pos2::new(x, ruler.bottom()),
                    Pos2::new(x - 5.0, ruler.bottom() - 7.0),
                    Pos2::new(x + 5.0, ruler.bottom() - 7.0),
                ],
                MARK,
                Stroke::NONE,
            ));
            p.text(
                Pos2::new(x + 6.0, ruler.bottom() - 2.0),
                Align2::LEFT_BOTTOM,
                &m.name,
                FontId::proportional(10.5),
                MARK,
            );
        }
    }
    let ruler_resp = ui.interact(
        ruler,
        ui.id().with("sequencer ruler"),
        Sense::click_and_drag(),
    );
    if (ruler_resp.dragged() || ruler_resp.clicked())
        && let Some(pos) = ruler_resp.interact_pointer_pos()
    {
        tab.seek(snap(t_of(pos.x), free).min(c.length));
        tab.playing = false;
        tab.drag = Some(Drag::Scrub);
    }

    let below = Rect::from_min_max(Pos2::new(outer.left(), ruler.bottom()), outer.max);
    let all = rows(&c, tab);
    let mut rows_ui = ui.new_child(egui::UiBuilder::new().max_rect(below));
    rows_ui.set_clip_rect(below.intersect(ui.clip_rect()));
    let mut clicked_key: Option<(Row, f64, bool)> = None;
    let mut toggle: Option<Row> = None;
    let mut open_curve: Option<(usize, String)> = None;
    let mut menu_track: Option<usize> = None;
    let mut pick_entity: Option<Entity> = None;
    let mut key_rects: Vec<(Row, f64, Pos2)> = Vec::new();
    let mut sheet_resp: Option<egui::Response> = None;
    egui::ScrollArea::vertical()
        .id_salt("sequencer rows")
        .auto_shrink([false, false])
        .show(&mut rows_ui, |ui| {
            let total = all.len() as f32 * ROW_H;
            let (area, resp) = ui.allocate_exact_size(
                Vec2::new(ui.available_width(), total.max(below.height())),
                Sense::click_and_drag(),
            );
            let p = ui.painter().clone();
            let pointer = resp.interact_pointer_pos();
            for (n, row) in all.iter().enumerate() {
                let r = Rect::from_min_size(
                    Pos2::new(area.left(), area.top() + n as f32 * ROW_H),
                    Vec2::new(area.width(), ROW_H),
                );
                if !ui.is_rect_visible(r) {
                    continue;
                }
                let lane = Rect::from_x_y_ranges(canvas_x..=canvas_x + width, r.y_range());
                let names = Rect::from_x_y_ranges(r.left()..=canvas_x - 6.0, r.y_range());
                let bg = match row {
                    Row::Group(_) => GROUP_BG,
                    _ if n % 2 == 0 => ROW_BG,
                    _ => ROW_ALT,
                };
                p.rect_filled(r, 0.0, bg);
                let font = FontId::proportional(12.5);
                let cy = r.center().y;
                match row {
                    Row::Group(g) => {
                        let folded = tab.folded.contains(g);
                        p.text(
                            Pos2::new(names.left() + 6.0, cy),
                            Align2::LEFT_CENTER,
                            if folded { "⏵" } else { "⏷" },
                            font.clone(),
                            theme::TEXT_DIM,
                        );
                        let found = cutscene::resolve(world, g, at, &HashMap::new());
                        p.text(
                            Pos2::new(names.left() + 24.0, cy),
                            Align2::LEFT_CENTER,
                            group_label(g),
                            FontId::proportional(13.0),
                            if found.is_some() {
                                theme::TEXT
                            } else {
                                theme::WARN
                            },
                        );
                        if found.is_none() {
                            p.text(
                                Pos2::new(names.right() - 4.0, cy),
                                Align2::RIGHT_CENTER,
                                "not found",
                                FontId::proportional(11.0),
                                theme::WARN,
                            );
                        }
                        tab.drawn_at.insert(format!("group {g}"), names);
                        if let Some(pos) = pointer.filter(|p| names.contains(*p))
                            && resp.clicked()
                        {
                            if pos.x < names.left() + 22.0 {
                                toggle = Some(row.clone());
                            } else {
                                pick_entity = found;
                            }
                        }
                    }
                    Row::Track(i) => {
                        let tr = &c.tracks[*i];
                        let indent = if matches!(tr, Track::Cuts(_)) {
                            6.0
                        } else {
                            22.0
                        };
                        let fold = match tr {
                            Track::Value(_) if tab.unfolded.contains(i) => "⏷",
                            Track::Value(_) => "⏵",
                            _ => "",
                        };
                        p.text(
                            Pos2::new(names.left() + indent, cy),
                            Align2::LEFT_CENTER,
                            fold,
                            font.clone(),
                            theme::TEXT_DIM,
                        );
                        let color = if tr.mute() {
                            theme::TEXT_DIM
                        } else {
                            theme::TEXT
                        };
                        let mut label = tr.label();
                        if let Track::Value(v) = tr
                            && v.finished == Finished::Restore
                        {
                            label.push_str("  ⟲");
                        }
                        if tr.mute() {
                            label.push_str("  (muted)");
                        }
                        p.text(
                            Pos2::new(names.left() + indent + 16.0, cy),
                            Align2::LEFT_CENTER,
                            label,
                            font.clone(),
                            color,
                        );
                        if let Track::Value(v) = tr {
                            p.text(
                                Pos2::new(names.right() - 4.0, cy),
                                Align2::RIGHT_CENTER,
                                value_text(world, v, at),
                                FontId::monospace(11.0),
                                theme::TEXT_DIM,
                            );
                        }
                        tab.drawn_at
                            .insert(format!("track {} {}", tr.bind(), tr.label()), names);
                        if let Some(pos) = pointer.filter(|p| names.contains(*p)) {
                            if resp.clicked() && pos.x < names.left() + indent + 14.0 {
                                toggle = Some(row.clone());
                            }
                            if resp.secondary_clicked() {
                                menu_track = Some(*i);
                            }
                            if resp.double_clicked()
                                && let Track::Value(v) = tr
                                && let Some(axis) = v.axes().first()
                            {
                                open_curve = Some((*i, axis.clone()));
                            }
                        }
                        draw_track_lane(&p, lane, tr, &c, &x_of, tab, row, &mut key_rects);
                    }
                    Row::Axis(i, axis) => {
                        let Track::Value(v) = &c.tracks[*i] else {
                            continue;
                        };
                        p.text(
                            Pos2::new(names.left() + 56.0, cy),
                            Align2::LEFT_CENTER,
                            axis,
                            font.clone(),
                            axis_color(axis),
                        );
                        if let Some(ch) = v.channels.get(axis) {
                            p.text(
                                Pos2::new(names.right() - 4.0, cy),
                                Align2::RIGHT_CENTER,
                                format!("{:.2}", ch.eval(tab.time)),
                                FontId::monospace(11.0),
                                theme::TEXT,
                            );
                            // Holds: a bar between keys of one value (Blender's).
                            for w in ch.keys.windows(2) {
                                if (w[0].v - w[1].v).abs() < 1e-9 {
                                    let rr = Rect::from_x_y_ranges(
                                        x_of(w[0].t)..=x_of(w[1].t),
                                        cy - 2.5..=cy + 2.5,
                                    );
                                    p.rect_filled(rr.intersect(lane), 1.0, HOLD);
                                }
                            }
                            for k in &ch.keys {
                                let x = x_of(k.t);
                                if x < lane.left() - 6.0 || x > lane.right() + 6.0 {
                                    continue;
                                }
                                let sel = is_selected(tab, row, k.t);
                                let fill = if sel { PICKED } else { axis_color(axis) };
                                diamond(
                                    &p,
                                    Pos2::new(x, cy),
                                    5.0,
                                    fill,
                                    k.interp == engine_script::curve::Interp::Constant,
                                );
                                key_rects.push((row.clone(), k.t, Pos2::new(x, cy)));
                            }
                        }
                        tab.drawn_at
                            .insert(format!("axis {} {} {axis}", v.bind, v.prop), names);
                        if pointer.is_some_and(|p| names.contains(p) || lane.contains(p))
                            && resp.double_clicked()
                        {
                            open_curve = Some((*i, axis.clone()));
                        }
                    }
                }
                if let Row::Group(_) = row {
                    for t in row_times(&c, row) {
                        let x = x_of(t);
                        if x < lane.left() - 6.0 || x > lane.right() + 6.0 {
                            continue;
                        }
                        diamond(&p, Pos2::new(x, cy), 5.5, KEY.gamma_multiply(0.75), false);
                        key_rects.push((row.clone(), t, Pos2::new(x, cy)));
                    }
                }
            }
            // The cutscene's end and the playhead over every row.
            let end = x_of(c.length);
            if end < area.right() {
                p.rect_filled(
                    Rect::from_x_y_ranges(end.max(canvas_x)..=canvas_x + width, area.y_range()),
                    0.0,
                    Color32::from_black_alpha(70),
                );
            }
            sheet_resp = Some(resp);
        });
    let resp = sheet_resp.expect("the rows were drawn");
    let p = ui.painter();
    // The playhead, through the ruler and the rows.
    let x = x_of(tab.time);
    if x >= canvas_x && x <= canvas_x + width {
        p.vline(x, ruler.top()..=outer.bottom(), Stroke::new(1.5, PLAYHEAD));
        p.add(Shape::convex_polygon(
            vec![
                Pos2::new(x - 6.0, ruler.top()),
                Pos2::new(x + 6.0, ruler.top()),
                Pos2::new(x, ruler.top() + 9.0),
            ],
            PLAYHEAD,
            Stroke::NONE,
        ));
        tab.drawn_at.insert(
            "playhead".into(),
            Rect::from_center_size(Pos2::new(x, ruler.center().y), Vec2::splat(12.0)),
        );
    }
    p.vline(
        canvas_x - 3.0,
        ruler.top()..=outer.bottom(),
        Stroke::new(1.0, theme::BORDER),
    );

    // Pointer on the sheet: keys picked, dragged, boxed.
    let hit = |pos: Pos2| {
        key_rects
            .iter()
            .filter(|(_, _, at)| at.distance(pos) <= HIT_PX)
            .min_by(|a, b| a.2.distance(pos).total_cmp(&b.2.distance(pos)))
            .cloned()
    };
    if resp.drag_started_by(PointerButton::Primary)
        && let Some(pos) = resp.interact_pointer_pos()
        && pos.x >= canvas_x
    {
        match hit(pos) {
            Some((row, t, _)) => {
                if !is_selected(tab, &row, t) {
                    if !ui.input(|i| i.modifiers.shift) {
                        tab.selected.clear();
                    }
                    tab.selected.extend(sels_for(&c, &row, t));
                }
                tab.drag = Some(Drag::Keys {
                    from_x: pos.x,
                    base: Box::new(c.clone()),
                    sel: tab.selected.clone(),
                });
            }
            None => tab.drag = Some(Drag::Box(pos)),
        }
    }
    if resp.clicked()
        && let Some(pos) = resp.interact_pointer_pos()
        && pos.x >= canvas_x
    {
        match hit(pos) {
            Some((row, t, _)) => clicked_key = Some((row, t, ui.input(|i| i.modifiers.shift))),
            None => {
                tab.selected.clear();
                // A click on an empty lane moves the playhead there, too.
                tab.seek(snap(t_of(pos.x), free).min(c.length));
            }
        }
    }
    if let Some((row, t, add)) = clicked_key {
        if !add {
            tab.selected.clear();
        }
        tab.selected.extend(sels_for(&c, &row, t));
        if let Row::Track(i) = row
            && let Track::Cuts(cuts) = &c.tracks[i]
            && let Some(cut) = cuts.cuts.iter().find(|k| (k.t - t).abs() < 1e-6)
            && let Some(e) = cutscene::resolve(world, &cut.camera, at, &HashMap::new())
        {
            pick_entity = Some(e);
        }
    }
    match tab.drag.clone() {
        Some(Drag::Keys { from_x, base, sel }) => {
            if let Some(pos) = ui.input(|i| i.pointer.interact_pos()) {
                let dt = (pos.x - from_x) as f64 / px;
                let dt = if free {
                    dt
                } else {
                    (dt * c.fps).round() / c.fps
                };
                let dt = sel.iter().map(|s| -s.t).fold(dt, f64::max);
                if dt != 0.0 {
                    tab.cutscene = Some(retime(&base, &sel, dt));
                    tab.selected = sel
                        .iter()
                        .map(|s| Sel {
                            t: (s.t + dt).max(0.0),
                            ..s.clone()
                        })
                        .collect();
                    tab.posed_at = None;
                }
            }
        }
        Some(Drag::Box(from)) => {
            if let Some(pos) = ui.input(|i| i.pointer.interact_pos()) {
                let r = Rect::from_two_pos(from, pos);
                p.rect(
                    r,
                    0.0,
                    theme::ACCENT.gamma_multiply(0.15),
                    Stroke::new(1.0, theme::ACCENT),
                    StrokeKind::Inside,
                );
                if !ui.input(|i| i.pointer.primary_down()) {
                    if !ui.input(|i| i.modifiers.shift) {
                        tab.selected.clear();
                    }
                    for (row, t, at) in &key_rects {
                        if r.contains(*at) && !matches!(row, Row::Group(_)) {
                            for s in sels_for(&c, row, *t) {
                                if !tab.selected.contains(&s) {
                                    tab.selected.push(s);
                                }
                            }
                        }
                    }
                }
            }
        }
        _ => {}
    }
    if !ui.input(|i| i.pointer.primary_down()) {
        tab.drag = None;
    }
    if let Some(row) = toggle {
        match row {
            Row::Group(g) => {
                if !tab.folded.remove(&g) {
                    tab.folded.insert(g);
                }
            }
            Row::Track(i) => {
                if !tab.unfolded.remove(&i) {
                    tab.unfolded.insert(i);
                }
            }
            Row::Axis(..) => {}
        }
    }
    if let Some(e) = pick_entity {
        editor.select(Some(e));
    }
    if let (Some((i, axis)), Some(path)) = (open_curve, tab.open.clone()) {
        crate::curves::show_channel(world, &path, i, &axis);
    }
    if let Some(i) = menu_track {
        ui.ctx()
            .memory_mut(|m| m.data.insert_temp(ui.id().with("track menu"), i));
    }
    track_menu(ui, tab, &resp);

    // Zoom (Ctrl+wheel or pinch), pan (Shift+wheel), keys: only while the pointer is here.
    if ui.rect_contains_pointer(outer) && !ui.ctx().egui_wants_keyboard_input() {
        let (scroll, zoom, mods) =
            ui.input(|i| (i.smooth_scroll_delta, i.zoom_delta(), i.modifiers));
        let pointer_t = ui
            .input(|i| i.pointer.hover_pos())
            .map(|p| t_of(p.x))
            .unwrap_or(tab.time);
        let mut z = 1.0f64;
        if zoom != 1.0 {
            z = zoom as f64;
        } else if mods.command && scroll.y != 0.0 {
            z = (scroll.y as f64 * 0.003).exp();
        }
        if z != 1.0 {
            let (a, b) = tab.t_view;
            tab.t_view = (
                pointer_t - (pointer_t - a) / z,
                pointer_t + (b - pointer_t) / z,
            );
        }
        if mods.shift && scroll.y != 0.0 {
            let shift = -(scroll.y as f64) / px;
            tab.t_view = (tab.t_view.0 + shift, tab.t_view.1 + shift);
        }
        let key = |m: Modifiers, k: Key| ui.input_mut(|i| i.consume_key(m, k));
        if key(Modifiers::NONE, Key::Delete) || key(Modifiers::NONE, Key::Backspace) {
            delete_selected(tab);
        }
        if key(Modifiers::NONE, Key::S) {
            key_selection(world, editor, tab);
        }
        if key(Modifiers::NONE, Key::F) {
            tab.fit = true;
        }
        if key(Modifiers::NONE, Key::Space) && editor.mode == Mode::Edit {
            tab.playing = !tab.playing;
        }
        if key(Modifiers::NONE, Key::Home) {
            tab.seek(0.0);
        }
        if key(Modifiers::NONE, Key::End) {
            tab.seek(c.length);
        }
        let frame = 1.0 / c.fps;
        if key(Modifiers::NONE, Key::ArrowLeft) {
            tab.seek(snap(tab.time - frame, false));
        }
        if key(Modifiers::NONE, Key::ArrowRight) {
            tab.seek(snap(tab.time + frame, false).min(c.length));
        }
        let redo =
            key(Modifiers::COMMAND, Key::Y) || key(Modifiers::COMMAND | Modifiers::SHIFT, Key::Z);
        let back = key(Modifiers::COMMAND, Key::Z);
        if back || redo {
            undo(world, editor, tab, back);
        }
    }
}

/// A track row's lane: keys as diamonds, events as flags, cuts as colored shots.
#[allow(clippy::too_many_arguments)]
fn draw_track_lane(
    p: &egui::Painter,
    lane: Rect,
    tr: &Track,
    c: &Cutscene,
    x_of: &dyn Fn(f64) -> f32,
    tab: &SequencerTab,
    row: &Row,
    keys: &mut Vec<(Row, f64, Pos2)>,
) {
    let cy = lane.center().y;
    let dim = if tr.mute() { 0.4 } else { 1.0 };
    match tr {
        Track::Cuts(cuts) => {
            for (i, cut) in cuts.cuts.iter().enumerate() {
                let end = cuts.cuts.get(i + 1).map_or(c.length, |n| n.t);
                let r = Rect::from_x_y_ranges(
                    x_of(cut.t)..=x_of(end),
                    lane.top() + 2.0..=lane.bottom() - 2.0,
                );
                let shown = r.intersect(lane);
                if shown.width() <= 0.0 {
                    continue;
                }
                let color = shot_color(&cut.camera).gamma_multiply(dim);
                p.rect_filled(shown, 3.0, color);
                if cut.blend > 0.0 && i > 0 {
                    // The blend: a ramp up from the previous shot.
                    let bx = x_of(cut.t + cut.blend).min(r.right());
                    let prev = shot_color(&cuts.cuts[i - 1].camera).gamma_multiply(dim);
                    p.add(Shape::convex_polygon(
                        vec![
                            Pos2::new(r.left(), r.top()),
                            Pos2::new(bx, r.top()),
                            Pos2::new(r.left(), r.bottom()),
                        ],
                        prev,
                        Stroke::NONE,
                    ));
                    p.line_segment(
                        [Pos2::new(bx, r.top()), Pos2::new(r.left(), r.bottom())],
                        Stroke::new(1.0, Color32::from_white_alpha(120)),
                    );
                }
                let sel = is_selected(tab, row, cut.t);
                p.rect_stroke(
                    shown,
                    3.0,
                    Stroke::new(
                        if sel { 2.0 } else { 1.0 },
                        if sel {
                            PICKED
                        } else {
                            Color32::from_black_alpha(140)
                        },
                    ),
                    StrokeKind::Inside,
                );
                let label = if cut.blend > 0.0 {
                    format!("🎥 {}  (blend {:.1} s)", cut.camera, cut.blend)
                } else {
                    format!("🎥 {}", cut.camera)
                };
                p.with_clip_rect(shown.shrink(2.0)).text(
                    Pos2::new(shown.left() + 6.0, cy),
                    Align2::LEFT_CENTER,
                    label,
                    FontId::proportional(12.0),
                    Color32::WHITE,
                );
                keys.push((row.clone(), cut.t, Pos2::new(x_of(cut.t), cy)));
            }
        }
        Track::Events(ev) => {
            for k in &ev.events {
                let x = x_of(k.t);
                if x < lane.left() - 6.0 || x > lane.right() + 6.0 {
                    continue;
                }
                let sel = is_selected(tab, row, k.t);
                let color = if sel {
                    PICKED
                } else {
                    EVENT.gamma_multiply(dim)
                };
                p.vline(
                    x,
                    lane.top() + 2.0..=lane.bottom() - 2.0,
                    Stroke::new(1.5, color),
                );
                let mut label = k.event.clone();
                if k.retro {
                    label.push_str(" ⟲");
                }
                let galley = p.layout_no_wrap(label, FontId::proportional(11.5), Color32::BLACK);
                let flag = Rect::from_min_size(
                    Pos2::new(x, lane.top() + 3.0),
                    galley.size() + Vec2::new(8.0, 2.0),
                );
                p.rect_filled(flag, 2.0, color);
                p.galley(flag.min + Vec2::new(4.0, 1.0), galley, Color32::BLACK);
                keys.push((row.clone(), k.t, Pos2::new(x, cy)));
            }
        }
        Track::Value(v) => {
            for t in tr.times() {
                let x = x_of(t);
                if x < lane.left() - 6.0 || x > lane.right() + 6.0 {
                    continue;
                }
                let sel = is_selected(tab, row, t);
                let constant = v.channels.values().any(|ch| {
                    ch.keys.iter().any(|k| {
                        (k.t - t).abs() < 1e-6 && k.interp == engine_script::curve::Interp::Constant
                    })
                });
                diamond(
                    p,
                    Pos2::new(x, cy),
                    5.5,
                    if sel { PICKED } else { KEY.gamma_multiply(dim) },
                    constant,
                );
                keys.push((row.clone(), t, Pos2::new(x, cy)));
            }
        }
    }
}

/// The right-click menu of a track: mute, restore at the end, remove.
fn track_menu(ui: &mut egui::Ui, tab: &mut SequencerTab, resp: &egui::Response) {
    let id = ui.id().with("track menu");
    let Some(i) = ui.ctx().memory(|m| m.data.get_temp::<usize>(id)) else {
        return;
    };
    let mut close = false;
    resp.context_menu(|ui| {
        let Some(c) = tab.cutscene.as_mut() else {
            return;
        };
        let Some(tr) = c.tracks.get_mut(i) else {
            return;
        };
        ui.label(RichText::new(format!("{} · {}", group_label(tr.bind()), tr.label())).strong());
        let mut mute = tr.mute();
        if ui.checkbox(&mut mute, "Muted").changed() {
            tr.set_mute(mute);
        }
        if let Track::Value(v) = tr {
            let mut restore = v.finished == Finished::Restore;
            if ui
                .checkbox(&mut restore, "Put back when it ends")
                .on_hover_text("Unreal's Restore State; off keeps what it set (Keep State)")
                .changed()
            {
                v.finished = if restore {
                    Finished::Restore
                } else {
                    Finished::Keep
                };
            }
        }
        if let Track::Cuts(cuts) = tr {
            for cut in cuts.cuts.iter_mut().skip(1) {
                ui.horizontal(|ui| {
                    ui.label(format!("Blend into {}", cut.camera));
                    ui.add(
                        egui::DragValue::new(&mut cut.blend)
                            .range(0.0..=10.0)
                            .speed(0.05)
                            .suffix(" s"),
                    );
                });
            }
        }
        if ui.button("✖ Remove the track").clicked() {
            c.tracks.remove(i);
            tab.selected.clear();
            tab.unfolded.clear();
            close = true;
            ui.close();
        }
    });
    if close {
        ui.ctx().memory_mut(|m| m.data.remove::<usize>(id));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use engine_script::curve::Key as CurveKey;

    fn sample() -> Cutscene {
        let mut c = Cutscene::default();
        let mut v = ValueTrack {
            bind: "Box".into(),
            prop: "position".into(),
            ..Default::default()
        };
        v.channels.insert(
            "x".into(),
            engine_script::curve::Curve::new(vec![
                CurveKey::new(0.0, 0.0),
                CurveKey::new(1.0, 2.0),
            ]),
        );
        v.channels.insert(
            "y".into(),
            engine_script::curve::Curve::new(vec![
                CurveKey::new(1.0, 5.0),
                CurveKey::new(2.0, 5.0),
            ]),
        );
        c.tracks.push(Track::Cuts(CutTrack {
            cuts: vec![Cut {
                t: 0.0,
                camera: "A".into(),
                blend: 0.0,
            }],
            ..Default::default()
        }));
        c.tracks.push(Track::Value(v));
        c
    }

    #[test]
    fn rows_put_cuts_first_then_each_object_with_its_tracks() {
        let c = sample();
        let mut tab = SequencerTab::default();
        assert_eq!(
            rows(&c, &tab),
            vec![Row::Track(0), Row::Group("Box".into()), Row::Track(1)]
        );
        tab.unfolded.insert(1);
        assert_eq!(rows(&c, &tab).len(), 5, "x and y unfolded");
        tab.folded.insert("Box".into());
        assert_eq!(rows(&c, &tab).len(), 2);
        assert_eq!(
            row_times(&c, &Row::Group("Box".into())),
            vec![0.0, 1.0, 2.0]
        );
    }

    #[test]
    fn dragging_keys_retimes_only_what_is_picked() {
        let c = sample();
        // One axis's key: only that axis moves.
        let moved = retime(
            &c,
            &[Sel {
                track: 1,
                axis: Some("x".into()),
                t: 1.0,
            }],
            0.5,
        );
        let Track::Value(v) = &moved.tracks[1] else {
            panic!()
        };
        assert_eq!(v.channels["x"].keys[1].t, 1.5);
        assert_eq!(v.channels["y"].keys[0].t, 1.0);
        // The track's key: every axis there moves.
        let moved = retime(
            &c,
            &[Sel {
                track: 1,
                axis: None,
                t: 1.0,
            }],
            -0.5,
        );
        assert_eq!(moved.tracks[1].times(), vec![0.0, 0.5, 2.0]);
    }

    #[test]
    fn auto_key_keys_only_the_axes_that_changed() {
        let mut world = World::new();
        let e = world
            .spawn((Name::new("Box"), Transform::from_xyz(1.0, 5.0, 0.0)))
            .id();
        let mut c = sample();
        let was = Transform::from_xyz(1.0, 5.0, 0.0);
        let now = Transform::from_xyz(3.0, 5.0, 0.0);
        let n = auto_key(&mut world, &mut c, 0.5, None, &[(e, was, now)]);
        assert_eq!(n, 1);
        let Track::Value(v) = &c.tracks[1] else {
            panic!()
        };
        assert_eq!(v.channels["x"].keys.len(), 3);
        assert!(
            !v.channels.contains_key("z"),
            "no key on an axis that didn't change"
        );
    }
}
