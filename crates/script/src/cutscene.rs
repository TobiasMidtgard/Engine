//! Cutscenes: several objects animated together on one clock, cuts between cameras and events
//! at times, made in the editor's Sequencer tab and played from a script or a
//! [`CutscenePlayer`] (Unreal's Sequencer and Level Sequences, Unity's Timeline and Playable
//! Director). A `.cutscene` file in the scripts folder (`cutscenes/`) is JSON, one key a line:
//!
//! ```json
//! {
//!   "length": 8,
//!   "aspect": 2.39,
//!   "tracks": [
//!     { "kind": "value", "bind": "Herald", "prop": "position",
//!       "channels": { "z": { "keys": [ { "t": 0, "v": 0 }, { "t": 4, "v": 6 } ] } } },
//!     { "kind": "events", "bind": "Drawbridge", "events": [ { "t": 2, "event": "toggle" } ] },
//!     { "kind": "cuts", "cuts": [ { "t": 0, "camera": "Cam wide" }, { "t": 5, "camera": "Cam close", "blend": 1 } ] }
//!   ]
//! }
//! ```
//!
//! A track names the object it changes (`bind`: looked up under the player's entity first,
//! then in the whole scene, unless the player binds the name to an entity of its own). A
//! value track changes a `prop` as a tween does (`position`, `rotation`, `scale`,
//! `"Component.field"`, `"material.field"`), each axis a [`Curve`]: `x`, `y`, `z` (and `w`)
//! for vectors and lists, `yaw`, `pitch`, `roll` in degrees for a rotation, `v` for a number.
//! Axes without a curve keep their value. An events track sends each event, as a message, to
//! the bound object's script (or the player's, when it binds none) as playback passes it. A
//! cuts track says which camera the picture comes from: while a cutscene plays, a view camera
//! of its own ([`CutsceneView`], in front of the game's) follows that camera, blending into
//! the next over a cut's `blend` seconds, from the game's camera over `blend_in` and back to
//! it over `blend_out`; `aspect` letterboxes it. engine-design/scripting-research.md §26.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::SystemTime;

use bevy_ecs::prelude::*;
use engine_core::math::{EulerRot, Quat, UVec2};
use engine_core::prelude::{ChildOf, GlobalTransform, Name, Time, Transform, Window};
use engine_render::{Camera, ExtraView, Projection, Viewport};
use serde::{Deserialize, Serialize};
use serde_json::{Value as Json, json};

use crate::curve::{Curve, Key};
use crate::tween::{Prop, TVal, read_entity, write_entity};

/// The folder cutscenes go in, under the scripts folder.
pub const FOLDER: &str = "cutscenes";
/// The view camera's `order`: in front of a game's cameras, behind an editor's.
pub const VIEW_ORDER: i32 = 100;

fn is_default<T: Default + PartialEq>(v: &T) -> bool {
    *v == T::default()
}

fn is_false(v: &bool) -> bool {
    !*v
}

// ----------------------------------------------------------------------------- the file

/// A cutscene file (see the module docs).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Cutscene {
    /// Seconds; it ends here even when its last key is earlier.
    pub length: f64,
    /// Frames a second the editor snaps keys to.
    pub fps: f64,
    /// The picture's shape while it plays through its cameras (2.39 puts cinema bars above
    /// and below); 0: the window's own.
    pub aspect: f64,
    /// Seconds to ease from the game's camera into the first cut, and back to it after.
    pub blend_in: f64,
    pub blend_out: f64,
    /// Named times (`cs:seek("landing")`), shown on the Sequencer's ruler.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub marks: Vec<Mark>,
    pub tracks: Vec<Track>,
}

impl Default for Cutscene {
    fn default() -> Self {
        Self {
            length: 5.0,
            fps: 30.0,
            aspect: 0.0,
            blend_in: 0.0,
            blend_out: 0.0,
            marks: Vec::new(),
            tracks: Vec::new(),
        }
    }
}

/// A named time.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Mark {
    pub t: f64,
    pub name: String,
}

/// A track: values of one object over time, its events, or the camera cuts.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Track {
    Value(ValueTrack),
    Events(EventTrack),
    Cuts(CutTrack),
}

/// What a value track leaves when the cutscene ends (Unreal's When Finished).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Finished {
    /// Its values stay as the cutscene left them (Keep State).
    #[default]
    Keep,
    /// Its values go back to what they were before it played (Restore State).
    Restore,
}

/// One value of an object, an axis a curve.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ValueTrack {
    /// The object's name.
    pub bind: String,
    /// What changes: `position`, `rotation`, `scale`, `"Component.field"`, `"material.field"`.
    pub prop: String,
    /// Axis name to curve (see [`axes_for`]).
    pub channels: BTreeMap<String, Curve>,
    #[serde(skip_serializing_if = "is_default")]
    pub finished: Finished,
    #[serde(skip_serializing_if = "is_false")]
    pub mute: bool,
}

/// Messages to one object's script at times.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct EventTrack {
    /// The object's name; empty: the player's own script.
    pub bind: String,
    pub events: Vec<EventKey>,
    #[serde(skip_serializing_if = "is_false")]
    pub mute: bool,
}

/// An event: `event` sent at `t` (a method of that name is called, as `e:send` does).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct EventKey {
    pub t: f64,
    pub event: String,
    #[serde(skip_serializing_if = "Json::is_null")]
    pub data: Json,
    /// Sent also when a skip or seek jumps past it (Unity's retroactive signals), so what it
    /// does still happens when the player skips.
    #[serde(skip_serializing_if = "is_false")]
    pub retro: bool,
    /// Only the first time round when the cutscene loops (Unity's emitOnce).
    #[serde(skip_serializing_if = "is_false")]
    pub once: bool,
}

/// Which camera the picture comes from, from when.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct CutTrack {
    pub cuts: Vec<Cut>,
    #[serde(skip_serializing_if = "is_false")]
    pub mute: bool,
}

/// From `t`, the picture comes from `camera` (a scene camera's name), blending from the
/// previous cut's over `blend` seconds (0: a hard cut).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Cut {
    pub t: f64,
    pub camera: String,
    #[serde(skip_serializing_if = "is_default")]
    pub blend: f64,
}

/// The axes a value of `prop` has: `yaw`, `pitch`, `roll` (degrees) for a rotation; `x`, `y`,
/// `z` for a position, a scale or a vec3; `v` for a number; `x` to `w` for a list.
pub fn axes_for(prop: &str, value: Option<&TValView>) -> Vec<&'static str> {
    match prop {
        "rotation" => vec!["yaw", "pitch", "roll"],
        "position" | "scale" => vec!["x", "y", "z"],
        _ => match value {
            Some(TValView::Number) | None => vec!["v"],
            Some(TValView::List(n)) => ["x", "y", "z", "w"][..(*n).clamp(1, 4)].to_vec(),
        },
    }
}

/// The shape of a value (for [`axes_for`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TValView {
    Number,
    List(usize),
}

/// Where an axis comes in a track's display order.
pub fn axis_rank(axis: &str) -> usize {
    match axis {
        "x" | "yaw" | "v" => 0,
        "y" | "pitch" => 1,
        "z" | "roll" => 2,
        "w" => 3,
        _ => 4,
    }
}

impl ValueTrack {
    /// Its axes with curves, in display order.
    pub fn axes(&self) -> Vec<String> {
        let mut out: Vec<String> = self.channels.keys().cloned().collect();
        out.sort_by_key(|a| (axis_rank(a), a.clone()));
        out
    }

    /// Each axis's value at `t`.
    pub fn sample(&self, t: f64) -> Vec<(String, f64)> {
        self.axes()
            .into_iter()
            .filter_map(|a| {
                let c = self.channels.get(&a)?;
                (!c.keys.is_empty()).then(|| (a.clone(), c.eval(t)))
            })
            .collect()
    }

    /// Sets the value of `axis` at `t`: the key there (within half a frame at `fps`) takes
    /// it, else a new key does.
    pub fn key(&mut self, axis: &str, t: f64, v: f64, fps: f64) {
        let c = self.channels.entry(axis.to_string()).or_default();
        // An angle the way round nearest the curve there, so keys don't spin the long way.
        let mut v = v;
        if matches!(axis, "yaw" | "pitch" | "roll") && !c.keys.is_empty() {
            let there = c.eval(t);
            v += ((there - v) / 360.0).round() * 360.0;
        }
        let near = 0.5 / fps.max(1.0);
        if let Some(k) = c.keys.iter_mut().find(|k| (k.t - t).abs() < near) {
            k.v = v;
        } else {
            c.keys.push(Key::new(t, v));
            c.sort();
        }
    }
}

impl Track {
    /// The object it is bound to (cameras for a cuts track: none).
    pub fn bind(&self) -> &str {
        match self {
            Track::Value(v) => &v.bind,
            Track::Events(e) => &e.bind,
            Track::Cuts(_) => "",
        }
    }

    pub fn mute(&self) -> bool {
        match self {
            Track::Value(v) => v.mute,
            Track::Events(e) => e.mute,
            Track::Cuts(c) => c.mute,
        }
    }

    pub fn set_mute(&mut self, on: bool) {
        match self {
            Track::Value(v) => v.mute = on,
            Track::Events(e) => e.mute = on,
            Track::Cuts(c) => c.mute = on,
        }
    }

    /// Its name in the Sequencer: the prop, "Events" or "Camera cuts".
    pub fn label(&self) -> String {
        match self {
            Track::Value(v) => v.prop.clone(),
            Track::Events(_) => "Events".into(),
            Track::Cuts(_) => "Camera cuts".into(),
        }
    }

    /// Every time it has a key, event or cut at, in order, once each.
    pub fn times(&self) -> Vec<f64> {
        let mut out: Vec<f64> = match self {
            Track::Value(v) => v
                .channels
                .values()
                .flat_map(|c| c.keys.iter().map(|k| k.t))
                .collect(),
            Track::Events(e) => e.events.iter().map(|k| k.t).collect(),
            Track::Cuts(c) => c.cuts.iter().map(|k| k.t).collect(),
        };
        out.sort_by(f64::total_cmp);
        out.dedup_by(|a, b| (*a - *b).abs() < 1e-6);
        out
    }

    /// Moves what is at `from` (every axis's key, the event, the cut) to `to`.
    pub fn retime(&mut self, from: f64, to: f64) {
        let near = |t: f64| (t - from).abs() < 1e-6;
        match self {
            Track::Value(v) => {
                for c in v.channels.values_mut() {
                    for k in c.keys.iter_mut().filter(|k| near(k.t)) {
                        k.t = to;
                    }
                    c.sort();
                }
            }
            Track::Events(e) => {
                for k in e.events.iter_mut().filter(|k| near(k.t)) {
                    k.t = to;
                }
                e.events.sort_by(|a, b| a.t.total_cmp(&b.t));
            }
            Track::Cuts(c) => {
                for k in c.cuts.iter_mut().filter(|k| near(k.t)) {
                    k.t = to;
                }
                c.cuts.sort_by(|a, b| a.t.total_cmp(&b.t));
            }
        }
    }

    /// Removes what is at `t`. Returns whether there was anything.
    pub fn remove_at(&mut self, t: f64) -> bool {
        let near = |x: f64| (x - t).abs() < 1e-6;
        match self {
            Track::Value(v) => {
                let mut any = false;
                for c in v.channels.values_mut() {
                    let n = c.keys.len();
                    c.keys.retain(|k| !near(k.t));
                    any |= c.keys.len() != n;
                }
                v.channels.retain(|_, c| !c.keys.is_empty());
                any
            }
            Track::Events(e) => {
                let n = e.events.len();
                e.events.retain(|k| !near(k.t));
                e.events.len() != n
            }
            Track::Cuts(c) => {
                let n = c.cuts.len();
                c.cuts.retain(|k| !near(k.t));
                c.cuts.len() != n
            }
        }
    }
}

impl Cutscene {
    /// From a `.cutscene` file's text.
    pub fn parse(text: &str) -> Result<Self, String> {
        let mut c: Cutscene = serde_json::from_str(text).map_err(|e| e.to_string())?;
        let finite = |x: f64| x.is_finite();
        if !finite(c.length) || c.length < 0.0 {
            return Err("length is not a number of seconds".into());
        }
        for t in &mut c.tracks {
            match t {
                Track::Value(v) => {
                    if v.prop.is_empty() {
                        return Err(format!("a value track for \"{}\" has no prop", v.bind));
                    }
                    for (axis, curve) in v.channels.iter_mut() {
                        if curve.keys.iter().any(|k| !finite(k.t) || !finite(k.v)) {
                            return Err(format!(
                                "{}.{} {axis}: a key's t or v is not a number",
                                v.bind, v.prop
                            ));
                        }
                        curve.sort();
                    }
                }
                Track::Events(e) => {
                    if e.events.iter().any(|k| !finite(k.t) || k.event.is_empty()) {
                        return Err(format!("an event of \"{}\" has no time or name", e.bind));
                    }
                    e.events.sort_by(|a, b| a.t.total_cmp(&b.t));
                }
                Track::Cuts(cuts) => {
                    if cuts
                        .cuts
                        .iter()
                        .any(|k| !finite(k.t) || k.camera.is_empty())
                    {
                        return Err("a camera cut has no time or camera".into());
                    }
                    cuts.cuts.sort_by(|a, b| a.t.total_cmp(&b.t));
                }
            }
        }
        c.marks.sort_by(|a, b| a.t.total_cmp(&b.t));
        if c.fps <= 0.0 || !finite(c.fps) {
            c.fps = 30.0;
        }
        Ok(c)
    }

    /// As a `.cutscene` file: a key, event, cut or mark a line, so changes diff well.
    pub fn to_text(&self) -> String {
        let num = |x: f64| serde_json::to_string(&x).unwrap_or_else(|_| "0".into());
        let mut head = vec![format!("\"length\": {}", num(self.length))];
        if self.fps != 30.0 {
            head.push(format!("\"fps\": {}", num(self.fps)));
        }
        if self.aspect > 0.0 {
            head.push(format!("\"aspect\": {}", num(self.aspect)));
        }
        if self.blend_in > 0.0 {
            head.push(format!("\"blend_in\": {}", num(self.blend_in)));
        }
        if self.blend_out > 0.0 {
            head.push(format!("\"blend_out\": {}", num(self.blend_out)));
        }
        let mut out = format!("{{\n  {}", head.join(",\n  "));
        if !self.marks.is_empty() {
            out.push_str(",\n  \"marks\": [\n");
            out.push_str(&lines(&self.marks, "    "));
            out.push_str("\n  ]");
        }
        out.push_str(",\n  \"tracks\": [");
        let tracks: Vec<String> = self.tracks.iter().map(track_text).collect();
        if !tracks.is_empty() {
            out.push('\n');
            out.push_str(&tracks.join(",\n"));
            out.push_str("\n  ");
        }
        out.push_str("]\n}\n");
        out
    }

    /// The time of the mark named `name`.
    pub fn mark(&self, name: &str) -> Option<f64> {
        self.marks.iter().find(|m| m.name == name).map(|m| m.t)
    }

    /// The first cuts track that isn't muted.
    pub fn cuts(&self) -> Option<&CutTrack> {
        self.tracks.iter().find_map(|t| match t {
            Track::Cuts(c) if !c.mute && !c.cuts.is_empty() => Some(c),
            _ => None,
        })
    }

    /// The cut the picture comes from at `t` (the first before it starts), and its index.
    pub fn cut_at(&self, t: f64) -> Option<(usize, &Cut)> {
        let cuts = &self.cuts()?.cuts;
        let i = cuts.iter().rposition(|c| c.t <= t + 1e-9).unwrap_or(0);
        Some((i, &cuts[i]))
    }

    /// The names its tracks and cuts bind, in the order they first come.
    pub fn names(&self) -> Vec<String> {
        let mut out: Vec<String> = Vec::new();
        let mut add = |n: &str| {
            if !n.is_empty() && !out.iter().any(|o| o == n) {
                out.push(n.to_string());
            }
        };
        for t in &self.tracks {
            match t {
                Track::Cuts(c) => c.cuts.iter().for_each(|c| add(&c.camera)),
                other => add(other.bind()),
            }
        }
        out
    }

    /// The last time anything happens.
    pub fn last_key(&self) -> f64 {
        self.tracks
            .iter()
            .flat_map(|t| t.times())
            .fold(0.0, f64::max)
    }
}

/// One small value on a line: `{ "t": 1, "v": 2 }`.
fn inline<T: Serialize>(v: &T) -> String {
    spaced(&serde_json::to_string(v).unwrap_or_default())
}

fn lines<T: Serialize>(items: &[T], indent: &str) -> String {
    items
        .iter()
        .map(|i| format!("{indent}{}", inline(i)))
        .collect::<Vec<_>>()
        .join(",\n")
}

/// Compact JSON with a space after each `:` and `,` and inside braces (strings untouched).
fn spaced(json: &str) -> String {
    let chars: Vec<char> = json.chars().collect();
    let mut out = String::with_capacity(json.len() + 16);
    let (mut in_str, mut esc) = (false, false);
    for (i, &c) in chars.iter().enumerate() {
        if in_str {
            out.push(c);
            if esc {
                esc = false;
            } else if c == '\\' {
                esc = true;
            } else if c == '"' {
                in_str = false;
            }
            continue;
        }
        match c {
            '"' => {
                in_str = true;
                out.push(c);
            }
            '{' => {
                out.push('{');
                if chars.get(i + 1) != Some(&'}') {
                    out.push(' ');
                }
            }
            '}' => {
                if i > 0 && chars[i - 1] != '{' {
                    out.push(' ');
                }
                out.push('}');
            }
            ':' => out.push_str(": "),
            ',' => out.push_str(", "),
            _ => out.push(c),
        }
    }
    out
}

fn track_text(t: &Track) -> String {
    let s = |x: &str| serde_json::to_string(x).unwrap_or_default();
    match t {
        Track::Value(v) => {
            let mut head = format!(
                "\"kind\": \"value\", \"bind\": {}, \"prop\": {}",
                s(&v.bind),
                s(&v.prop)
            );
            if v.finished == Finished::Restore {
                head.push_str(", \"finished\": \"restore\"");
            }
            if v.mute {
                head.push_str(", \"mute\": true");
            }
            let channels: Vec<String> = v
                .axes()
                .iter()
                .map(|axis| {
                    let c = &v.channels[axis];
                    let mut out = format!("        {}: {{ \"keys\": [\n", s(axis));
                    out.push_str(&lines(&c.keys, "          "));
                    out.push_str("\n        ]");
                    if c.before != Default::default() {
                        out.push_str(&format!(", \"before\": {}", inline(&c.before)));
                    }
                    if c.after != Default::default() {
                        out.push_str(&format!(", \"after\": {}", inline(&c.after)));
                    }
                    out.push_str(" }");
                    out
                })
                .collect();
            format!(
                "    {{\n      {head},\n      \"channels\": {{\n{}\n      }}\n    }}",
                channels.join(",\n")
            )
        }
        Track::Events(e) => {
            let mut head = format!("\"kind\": \"events\", \"bind\": {}", s(&e.bind));
            if e.mute {
                head.push_str(", \"mute\": true");
            }
            format!(
                "    {{\n      {head},\n      \"events\": [\n{}\n      ]\n    }}",
                lines(&e.events, "        ")
            )
        }
        Track::Cuts(c) => {
            let mut head = "\"kind\": \"cuts\"".to_string();
            if c.mute {
                head.push_str(", \"mute\": true");
            }
            format!(
                "    {{\n      {head},\n      \"cuts\": [\n{}\n      ]\n    }}",
                lines(&c.cuts, "        ")
            )
        }
    }
}

// ----------------------------------------------------------------------------- the files

/// The loaded `.cutscene` files (a resource players and the editor share): read once,
/// reloaded when the file changes ([`CutsceneFiles::poll`]), replaced at once when the editor
/// changes one ([`CutsceneFiles::set`]).
#[derive(Resource, Default)]
pub struct CutsceneFiles {
    map: HashMap<String, Loaded>,
    /// Changes whenever a cutscene does.
    pub version: u64,
}

struct Loaded {
    cutscene: Result<Arc<Cutscene>, String>,
    full: PathBuf,
    stamp: Option<(SystemTime, u64)>,
}

fn stamp(path: &Path) -> Option<(SystemTime, u64)> {
    let m = std::fs::metadata(path).ok()?;
    Some((m.modified().ok()?, m.len()))
}

fn read_file(full: &Path, path: &str) -> Result<Arc<Cutscene>, String> {
    std::fs::read_to_string(full)
        .map_err(|e| format!("can't read {path}: {e}"))
        .and_then(|text| Cutscene::parse(&text).map_err(|e| format!("{path}: {e}")))
        .map(Arc::new)
}

/// A cutscene's path as used to name it: forward slashes, no leading `./`, `.cutscene` added.
pub fn normalize(path: &str) -> String {
    let p = path.replace('\\', "/");
    let p = p.trim_start_matches("./").trim_start_matches('/');
    if p.ends_with(".cutscene") {
        p.to_string()
    } else {
        format!("{p}.cutscene")
    }
}

impl CutsceneFiles {
    /// The cutscene at `path` (in `root`, the scripts folder), loading it the first time.
    pub fn get(&mut self, root: &Path, path: &str) -> Result<Arc<Cutscene>, String> {
        let path = normalize(path);
        if let Some(l) = self.map.get(&path) {
            return l.cutscene.clone();
        }
        let full = root.join(&path);
        let cutscene = read_file(&full, &path);
        let stamp = stamp(&full);
        self.map.insert(
            path,
            Loaded {
                cutscene: cutscene.clone(),
                full,
                stamp,
            },
        );
        cutscene
    }

    /// The cutscene at `path` if it is loaded (no file access).
    pub fn loaded(&self, path: &str) -> Option<Arc<Cutscene>> {
        self.map
            .get(&normalize(path))
            .and_then(|l| l.cutscene.as_ref().ok().cloned())
    }

    /// Replaces a cutscene and writes its file (the editor, as it edits).
    pub fn set(&mut self, root: &Path, path: &str, cutscene: Cutscene) -> Result<(), String> {
        let path = normalize(path);
        let full = root.join(&path);
        if let Some(dir) = full.parent() {
            std::fs::create_dir_all(dir)
                .map_err(|e| format!("can't make {}: {e}", dir.display()))?;
        }
        std::fs::write(&full, cutscene.to_text())
            .map_err(|e| format!("can't write {path}: {e}"))?;
        let stamp = stamp(&full);
        self.map.insert(
            path,
            Loaded {
                cutscene: Ok(Arc::new(cutscene)),
                full,
                stamp,
            },
        );
        self.version += 1;
        Ok(())
    }

    /// Replaces a cutscene without writing its file (the editor, while a key is dragged).
    pub fn put(&mut self, root: &Path, path: &str, cutscene: Cutscene) {
        let path = normalize(path);
        let full = root.join(&path);
        let stamp = self
            .map
            .get(&path)
            .map(|l| l.stamp)
            .unwrap_or_else(|| stamp(&full));
        self.map.insert(
            path,
            Loaded {
                cutscene: Ok(Arc::new(cutscene)),
                full,
                stamp,
            },
        );
        self.version += 1;
    }

    /// Reloads the cutscenes whose files changed. Returns the paths reloaded.
    pub fn poll(&mut self) -> Vec<String> {
        let mut changed = Vec::new();
        for (path, l) in self.map.iter_mut() {
            let now = stamp(&l.full);
            if now == l.stamp {
                continue;
            }
            l.stamp = now;
            l.cutscene = read_file(&l.full, path);
            changed.push(path.clone());
        }
        if !changed.is_empty() {
            self.version += 1;
        }
        changed
    }
}

/// Every `.cutscene` file under `root`, as paths relative to it.
pub fn find_files(root: &Path) -> Vec<String> {
    fn walk(dir: &Path, root: &Path, out: &mut Vec<String>, depth: u32) {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return;
        };
        for e in entries.flatten() {
            let p = e.path();
            if p.is_dir() && depth < 6 {
                walk(&p, root, out, depth + 1);
            } else if p.extension().is_some_and(|x| x == "cutscene")
                && let Ok(rel) = p.strip_prefix(root)
            {
                out.push(rel.to_string_lossy().replace('\\', "/"));
            }
        }
    }
    let mut out = Vec::new();
    walk(root, root, &mut out, 0);
    out.sort();
    out
}

/// Loads `path` (relative to the scripts folder) into [`CutsceneFiles`].
pub fn load(world: &mut World, path: &str) -> Result<Arc<Cutscene>, String> {
    let root = crate::scripts_root(world);
    world
        .get_resource_or_insert_with(CutsceneFiles::default)
        .get(&root, path)
}

// ----------------------------------------------------------------------------- posing

/// Values a cutscene changed, with what they were before (to put back).
#[derive(Default)]
pub struct Saved {
    order: Vec<(Entity, Prop)>,
    before: HashMap<(Entity, Prop), (TVal, bool)>,
}

impl Saved {
    fn remember(&mut self, e: Entity, prop: &Prop, value: &TVal, restore: bool) {
        let key = (e, prop.clone());
        if let Some(b) = self.before.get_mut(&key) {
            b.1 |= restore;
            return;
        }
        self.order.push(key.clone());
        self.before.insert(key, (value.clone(), restore));
    }

    pub fn is_empty(&self) -> bool {
        self.order.is_empty()
    }

    /// The entities it changed.
    pub fn entities(&self) -> Vec<Entity> {
        let mut out: Vec<Entity> = self.order.iter().map(|(e, _)| *e).collect();
        out.dedup();
        out
    }

    /// Puts back every value it changed (`all`), or only those of tracks set to restore.
    pub fn restore(&mut self, world: &mut World, all: bool) {
        for key in self.order.iter().rev() {
            let Some((value, restore)) = self.before.get(key) else {
                continue;
            };
            if (all || *restore) && world.get_entity(key.0).is_ok() {
                let _ = write_entity(world, key.0, &key.1, value);
            }
        }
        self.order.clear();
        self.before.clear();
    }

    /// Forgets what it changed on `e` (the editor keeps a key it made there).
    pub fn forget(&mut self, e: Entity) {
        self.order.retain(|(x, _)| *x != e);
        self.before.retain(|(x, _), _| *x != e);
    }
}

/// Finds the object a track binds: an override, then below `at`, then anywhere in the scene.
pub fn resolve(
    world: &mut World,
    name: &str,
    at: Option<Entity>,
    bind: &HashMap<String, Entity>,
) -> Option<Entity> {
    if let Some(e) = bind.get(name).copied()
        && world.get_entity(e).is_ok()
    {
        return Some(e);
    }
    if name.is_empty() {
        return at.filter(|e| world.get_entity(*e).is_ok());
    }
    if let Some(at) = at
        && world.get_entity(at).is_ok()
    {
        if world.get::<Name>(at).is_some_and(|n| n.as_str() == name) {
            return Some(at);
        }
        if let Some(e) = crate::entity::find_descendant(world, at, name) {
            return Some(e);
        }
    }
    let mut q = world.query::<(Entity, &Name)>();
    q.iter(world)
        .find(|(_, n)| n.as_str() == name)
        .map(|(e, _)| e)
}

/// The value a value track would give `current` at `t` (its keyed axes replaced).
fn compose(prop: &Prop, current: &TVal, axes: &[(String, f64)]) -> Result<TVal, String> {
    let get = |name: &str| axes.iter().find(|(a, _)| a == name).map(|(_, v)| *v);
    Ok(match (prop, current) {
        (Prop::Rotation, TVal::Quat(q)) => {
            let (yaw, pitch, roll) = q.to_euler(EulerRot::YXZ);
            let deg =
                |name: &str, now: f32| get(name).map(|d| (d as f32).to_radians()).unwrap_or(now);
            TVal::Quat(Quat::from_euler(
                EulerRot::YXZ,
                deg("yaw", yaw),
                deg("pitch", pitch),
                deg("roll", roll),
            ))
        }
        (_, TVal::Vec3(v)) => {
            let mut v = *v;
            v.x = get("x").unwrap_or(v.x);
            v.y = get("y").unwrap_or(v.y);
            v.z = get("z").unwrap_or(v.z);
            TVal::Vec3(v)
        }
        (_, TVal::Num(x)) => TVal::Num(get("v").or_else(|| get("x")).unwrap_or(*x)),
        (_, TVal::List(l)) => {
            let mut l = l.clone();
            for (i, name) in ["x", "y", "z", "w"].iter().enumerate() {
                if let (Some(slot), Some(v)) = (l.get_mut(i), get(name)) {
                    *slot = v;
                }
            }
            TVal::List(l)
        }
        (_, other) => return Err(format!("{} can't take axes", other.kind_name())),
    })
}

/// An object's value of `prop` by axis, as a value track keys it (degrees for a rotation).
pub fn read_axes(world: &mut World, e: Entity, prop: &str) -> Result<Vec<(String, f64)>, String> {
    let p = Prop::parse(prop, true)?;
    let value = read_entity(world, e, &p)?;
    Ok(match (&p, &value) {
        (Prop::Rotation, TVal::Quat(q)) => {
            let (yaw, pitch, roll) = q.to_euler(EulerRot::YXZ);
            vec![
                ("yaw".into(), yaw.to_degrees() as f64),
                ("pitch".into(), pitch.to_degrees() as f64),
                ("roll".into(), roll.to_degrees() as f64),
            ]
        }
        (_, TVal::Vec3(v)) => vec![("x".into(), v.x), ("y".into(), v.y), ("z".into(), v.z)],
        (_, TVal::Num(x)) => vec![("v".into(), *x)],
        (_, TVal::List(l)) => ["x", "y", "z", "w"]
            .iter()
            .zip(l)
            .map(|(a, v)| (a.to_string(), *v))
            .collect(),
        _ => Vec::new(),
    })
}

/// What an object has that a value track can change: its transform, the number fields of its
/// components (`"PointLight.intensity_lm"`, `"PointLight.color"`) and of its material
/// (`"material.emissive_strength"`).
pub fn keyable(world: &mut World, e: Entity) -> Vec<String> {
    fn walk(j: &Json, at: String, out: &mut Vec<String>, depth: u32) {
        match j {
            Json::Number(_) => out.push(at),
            Json::Array(a) if (1..=4).contains(&a.len()) && a.iter().all(Json::is_number) => {
                out.push(at)
            }
            Json::Object(m) if depth < 3 => {
                for (k, v) in m {
                    walk(v, format!("{at}.{k}"), out, depth + 1);
                }
            }
            _ => {}
        }
    }
    let mut out: Vec<String> = ["position", "rotation", "scale"].map(String::from).to_vec();
    if world.get_entity(e).is_err() {
        return out;
    }
    const SKIP: [&str; 6] = [
        "Transform",
        "GlobalTransform",
        "Name",
        "Script",
        "CutscenePlayer",
        "MeshRenderer",
    ];
    for c in engine_core::registry::component_names(world, e) {
        if SKIP.contains(&c.as_str()) {
            continue;
        }
        let get = world
            .get_resource::<engine_core::registry::TypeRegistry>()
            .and_then(|r| r.component(&c))
            .map(|entry| entry.get);
        if let Some(json) = get.and_then(|get| get(world, e)) {
            walk(&json, c, &mut out, 0);
        }
    }
    if world.get::<engine_render::MeshRenderer>(e).is_some()
        && let Ok(m) = crate::entity::material(world, e, None)
    {
        walk(&m, "material".into(), &mut out, 1);
    }
    out
}

/// Sets every value track of `cutscene` to its value at `t` on the objects it binds,
/// remembering in `saved` what each was before the first change. Returns the names it
/// couldn't find, and value tracks' errors.
pub fn pose(
    world: &mut World,
    cutscene: &Cutscene,
    t: f64,
    at: Option<Entity>,
    bind: &HashMap<String, Entity>,
    saved: &mut Saved,
) -> Vec<String> {
    let mut problems = Vec::new();
    let mut found: HashMap<String, Option<Entity>> = HashMap::new();
    for track in &cutscene.tracks {
        let Track::Value(v) = track else {
            continue;
        };
        if v.mute || v.channels.is_empty() {
            continue;
        }
        let e = *found
            .entry(v.bind.clone())
            .or_insert_with(|| resolve(world, &v.bind, at, bind));
        let Some(e) = e else {
            if !problems.contains(&v.bind) {
                problems.push(v.bind.clone());
            }
            continue;
        };
        if let Err(err) = pose_track(world, e, v, t, saved) {
            let line = format!("{}.{}: {err}", v.bind, v.prop);
            if !problems.contains(&line) {
                problems.push(line);
            }
        }
    }
    problems
}

fn pose_track(
    world: &mut World,
    e: Entity,
    v: &ValueTrack,
    t: f64,
    saved: &mut Saved,
) -> Result<(), String> {
    let prop = Prop::parse(&v.prop, true)?;
    let current = read_entity(world, e, &prop)?;
    saved.remember(e, &prop, &current, v.finished == Finished::Restore);
    let next = compose(&prop, &current, &v.sample(t))?;
    if next != current {
        write_entity(world, e, &prop, &next)?;
    }
    Ok(())
}

/// `e`'s transform in the world: its parent's last world transform with its own (so a camera
/// a cutscene moved this frame is where it was put).
pub fn world_pose(world: &World, e: Entity) -> Option<Transform> {
    let local = *world.get::<Transform>(e)?;
    let parent = world
        .get::<ChildOf>(e)
        .and_then(|c| world.get::<GlobalTransform>(c.parent()));
    Some(match parent {
        Some(p) => GlobalTransform(p.0 * local.to_affine()).to_transform(),
        None => local,
    })
}

// ----------------------------------------------------------------------------- playing

/// Plays a cutscene on this entity (Unreal's Level Sequence Actor, Unity's Playable
/// Director): the objects its tracks name are looked up below it first, and its events with
/// no object go to this entity's script.
#[derive(Component, Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct CutscenePlayer {
    /// The `.cutscene` file, relative to the scripts folder (`"cutscenes/intro"`).
    pub path: String,
    /// Plays when the game starts (Play On Awake).
    pub autoplay: bool,
    /// From the start again at the end.
    #[serde(rename = "loop")]
    pub looping: bool,
    pub rate: f64,
    /// Puts back everything it changed when it ends (Unreal's Restore State).
    pub restore: bool,
    /// Plays its camera cuts (off: only its objects move).
    pub cameras: bool,
}

impl Default for CutscenePlayer {
    fn default() -> Self {
        Self {
            path: String::new(),
            autoplay: true,
            looping: false,
            rate: 1.0,
            restore: false,
            cameras: true,
        }
    }
}

/// The camera a playing cutscene looks through (spawned and removed by it).
#[derive(Component, Debug, Clone, Copy)]
pub struct CutsceneView {
    pub id: u64,
}

/// How to play one ([`play`]).
#[derive(Clone, Debug)]
pub struct PlayOptions {
    pub rate: f64,
    pub looping: bool,
    /// Seconds to start at (or end at, playing backward).
    pub from: f64,
    /// Where it ends (its length when none).
    pub to: Option<f64>,
    /// Put back everything it changed when it ends.
    pub restore: bool,
    /// Look through its camera cuts.
    pub cameras: bool,
    /// Real seconds, not world seconds (slow motion doesn't slow it).
    pub unscaled: bool,
    /// The entity it plays on: names are looked up below it first, and events with no object
    /// go to its script.
    pub at: Option<Entity>,
    /// Names bound to entities of the caller's choosing.
    pub bind: HashMap<String, Entity>,
}

impl Default for PlayOptions {
    fn default() -> Self {
        Self {
            rate: 1.0,
            looping: false,
            from: 0.0,
            to: None,
            restore: false,
            cameras: true,
            unscaled: false,
            at: None,
            bind: HashMap::new(),
        }
    }
}

/// A cutscene playing.
pub struct Playing {
    pub id: u64,
    pub path: String,
    pub time: f64,
    pub opts: PlayOptions,
    pub paused: bool,
    /// Finished, stopped or skipped.
    pub done: bool,
    /// Names it couldn't find (and value tracks' errors).
    pub missing: Vec<String>,
    /// Real seconds since it started (for blending in from the game's camera).
    pub played: f64,
    saved: Saved,
    /// Where the last frame left off (events between it and now fire).
    last: Option<f64>,
    fired_once: HashSet<(usize, usize)>,
    /// The camera showing its cuts (none without cuts or with `cameras` off).
    pub view: Option<Entity>,
    /// Where the view was last put, and its lens (to blend out from).
    view_pose: Option<(Transform, Projection)>,
}

impl Playing {
    fn end(&self, len: f64) -> f64 {
        self.opts.to.unwrap_or(len).min(len).max(0.0)
    }

    fn start(&self) -> f64 {
        self.opts.from.max(0.0)
    }
}

/// The view easing back to the game's camera after a cutscene ended.
struct Outro {
    view: Entity,
    from: (Transform, Projection),
    secs: f64,
    elapsed: f64,
}

/// Cutscenes playing now, and what they do this frame.
#[derive(Resource, Default)]
pub struct Cutscenes {
    pub playing: Vec<Playing>,
    next: u64,
    /// Messages for scripts (to, event, data, from), sent after the frame's cutscenes ran.
    outbox: Vec<(Entity, String, Json, Option<Entity>)>,
    outros: Vec<Outro>,
    /// Players already started this play.
    autoplayed: HashSet<Entity>,
}

impl Cutscenes {
    pub fn get(&self, id: u64) -> Option<&Playing> {
        self.playing.iter().find(|p| p.id == id)
    }

    fn get_mut(&mut self, id: u64) -> Option<&mut Playing> {
        self.playing.iter_mut().find(|p| p.id == id)
    }
}

/// Starts `path` playing. Another play of the same file on the same entity stops first.
/// Poses its objects at the start at once. Returns its id.
pub fn play(world: &mut World, path: &str, opts: PlayOptions) -> Result<u64, String> {
    let cutscene = load(world, path)?;
    let path = normalize(path);
    let again: Vec<u64> = world
        .get_resource::<Cutscenes>()
        .map(|c| {
            c.playing
                .iter()
                .filter(|p| p.path == path && p.opts.at == opts.at && !p.done)
                .map(|p| p.id)
                .collect()
        })
        .unwrap_or_default();
    for id in again {
        stop(world, id);
    }
    let mut cs = world.get_resource_or_insert_with(Cutscenes::default);
    cs.next += 1;
    let id = cs.next;
    let reverse = opts.rate < 0.0;
    let mut p = Playing {
        id,
        path,
        time: 0.0,
        opts,
        paused: false,
        done: false,
        missing: Vec::new(),
        played: 0.0,
        saved: Saved::default(),
        last: None,
        fired_once: HashSet::new(),
        view: None,
        view_pose: None,
    };
    p.time = if reverse {
        p.end(cutscene.length)
    } else {
        p.start().min(p.end(cutscene.length))
    };
    // Starting past retro events sends them (Unity's retroactive signals); one right at the
    // start goes with the first frame's.
    if !reverse && p.time > 0.0 {
        let hi = p.time - 1e-9;
        events(world, &mut p, &cutscene, 0.0, hi, true, true);
    }
    let problems = pose(
        world,
        &cutscene,
        p.time,
        p.opts.at,
        &p.opts.bind,
        &mut p.saved,
    );
    note_missing(&mut p, problems);
    world.resource_mut::<Cutscenes>().playing.push(p);
    Ok(id)
}

fn note_missing(p: &mut Playing, problems: Vec<String>) {
    for m in problems {
        if !p.missing.contains(&m) {
            tracing::warn!(target: "script", "{}: can't play \"{m}\": no object of that name (or it can't take that value)", p.path);
            p.missing.push(m);
        }
    }
}

/// Takes one playing cutscene out, runs `f` on it with the world, and puts it back.
fn with_playing<R>(
    world: &mut World,
    id: u64,
    f: impl FnOnce(&mut World, &mut Playing, &Cutscene) -> R,
) -> Option<R> {
    let (i, mut p) = {
        let mut cs = world.get_resource_mut::<Cutscenes>()?;
        let i = cs.playing.iter().position(|p| p.id == id)?;
        (i, cs.playing.remove(i))
    };
    let cutscene: Arc<Cutscene> = world
        .get_resource::<CutsceneFiles>()
        .and_then(|f| f.loaded(&p.path))
        .unwrap_or_default();
    let out = f(world, &mut p, cutscene.as_ref());
    let mut cs = world.resource_mut::<Cutscenes>();
    let i = i.min(cs.playing.len());
    cs.playing.insert(i, p);
    Some(out)
}

/// Pauses (or goes on with) a playing cutscene.
pub fn set_paused(world: &mut World, id: u64, paused: bool) {
    if let Some(mut cs) = world.get_resource_mut::<Cutscenes>()
        && let Some(p) = cs.get_mut(id)
    {
        p.paused = paused;
    }
}

/// Changes how fast it plays (negative: backward).
pub fn set_rate(world: &mut World, id: u64, rate: f64) {
    if let Some(mut cs) = world.get_resource_mut::<Cutscenes>()
        && let Some(p) = cs.get_mut(id)
    {
        p.opts.rate = rate;
    }
}

/// Jumps to `t`: its objects are posed there at once, and the events it jumps over that are
/// `retro` are sent (forward only).
pub fn seek(world: &mut World, id: u64, t: f64) {
    with_playing(world, id, |world, p, cutscene| jump(world, p, cutscene, t));
}

fn jump(world: &mut World, p: &mut Playing, cutscene: &Cutscene, t: f64) {
    let end = p.end(cutscene.length);
    let to = t.clamp(p.start().min(end), end);
    let from = p.last.unwrap_or(p.time);
    if to > from {
        events(world, p, cutscene, from, to, true, p.last.is_none());
    }
    p.time = to;
    p.last = Some(to);
    let problems = pose(world, cutscene, to, p.opts.at, &p.opts.bind, &mut p.saved);
    note_missing(p, problems);
    if p.opts.cameras {
        view(world, p, cutscene, 0.0);
    }
}

/// Jumps to the end and stops (Unreal's Go to End and Stop): its objects as it leaves them,
/// its `retro` events sent.
pub fn skip(world: &mut World, id: u64) {
    with_playing(world, id, |world, p, cutscene| {
        let end = if p.opts.rate < 0.0 {
            p.start().min(p.end(cutscene.length))
        } else {
            p.end(cutscene.length)
        };
        jump(world, p, cutscene, end);
    });
    finish(world, id);
}

/// Stops it where it is (values put back if it restores).
pub fn stop(world: &mut World, id: u64) {
    finish(world, id);
}

/// Stops every cutscene playing.
pub fn stop_all(world: &mut World) {
    let ids: Vec<u64> = world
        .get_resource::<Cutscenes>()
        .map(|c| c.playing.iter().map(|p| p.id).collect())
        .unwrap_or_default();
    for id in ids {
        finish(world, id);
    }
}

/// Ends a cutscene: values put back where its tracks (or its options) say, the view eased back
/// to the game's camera or removed.
fn finish(world: &mut World, id: u64) {
    let blend_out = with_playing(world, id, |world, p, cutscene| {
        if p.done {
            return None;
        }
        p.done = true;
        p.saved.restore(world, p.opts.restore);
        let view = p.view.take();
        match (view, p.view_pose) {
            (Some(v), Some(from)) if cutscene.blend_out > 0.0 && world.get_entity(v).is_ok() => {
                Some(Outro {
                    view: v,
                    from,
                    secs: cutscene.blend_out,
                    elapsed: 0.0,
                })
            }
            (Some(v), _) => {
                if world.get_entity(v).is_ok() {
                    world.despawn(v);
                }
                None
            }
            _ => None,
        }
    })
    .flatten();
    let mut cs = world.resource_mut::<Cutscenes>();
    if let Some(outro) = blend_out {
        cs.outros.push(outro);
    }
    cs.playing.retain(|p| p.id != id);
}

/// Sends the events between `from` and `to` (in the direction played). `jumped`: only the
/// `retro` ones. `inclusive`: one exactly at `from` too (the first frame).
fn events(
    world: &mut World,
    p: &mut Playing,
    cutscene: &Cutscene,
    from: f64,
    to: f64,
    jumped: bool,
    inclusive: bool,
) {
    let forward = to >= from;
    let (lo, hi) = if forward { (from, to) } else { (to, from) };
    let mut due: Vec<(f64, usize, usize)> = Vec::new();
    for (ti, track) in cutscene.tracks.iter().enumerate() {
        let Track::Events(e) = track else {
            continue;
        };
        if e.mute {
            continue;
        }
        for (ki, k) in e.events.iter().enumerate() {
            let inside = if forward {
                (k.t > lo || (inclusive && k.t >= lo)) && k.t <= hi
            } else {
                k.t >= lo && (k.t < hi || (inclusive && k.t <= hi))
            };
            if !inside || (jumped && !k.retro) {
                continue;
            }
            if k.once && !p.fired_once.insert((ti, ki)) {
                continue;
            }
            due.push((k.t, ti, ki));
        }
    }
    due.sort_by(|a, b| {
        if forward {
            a.0.total_cmp(&b.0)
        } else {
            b.0.total_cmp(&a.0)
        }
    });
    for (_, ti, ki) in due {
        let Track::Events(e) = &cutscene.tracks[ti] else {
            continue;
        };
        let k = &e.events[ki];
        match resolve(world, &e.bind, p.opts.at, &p.opts.bind) {
            Some(to) => world.resource_mut::<Cutscenes>().outbox.push((
                to,
                k.event.clone(),
                k.data.clone(),
                p.opts.at,
            )),
            None => {
                let name = if e.bind.is_empty() {
                    "the player"
                } else {
                    e.bind.as_str()
                };
                note_missing(p, vec![name.to_string()]);
            }
        }
    }
}

fn smooth(x: f64) -> f64 {
    let x = x.clamp(0.0, 1.0);
    x * x * (3.0 - 2.0 * x)
}

/// From `a` toward `b` by `k`: position, rotation and (both perspective) field of view.
fn blend(
    a: &(Transform, Projection),
    b: &(Transform, Projection),
    k: f64,
) -> (Transform, Projection) {
    let mut t = a.0.lerp(&b.0, k as f32);
    t.scale = engine_core::math::Vec3::ONE;
    let lens = match (a.1, b.1) {
        (
            Projection::Perspective {
                fov_y_deg: fa,
                near,
            },
            Projection::Perspective { fov_y_deg: fb, .. },
        ) => Projection::Perspective {
            fov_y_deg: fa + (fb - fa) * k as f32,
            near,
        },
        _ if k < 0.5 => a.1,
        _ => b.1,
    };
    (t, lens)
}

/// The game's camera: the active one in front that isn't a cutscene's or an extra view, below
/// [`VIEW_ORDER`].
fn game_camera(world: &mut World) -> Option<(Transform, Projection)> {
    let mut q =
        world.query_filtered::<(Entity, &Camera), (Without<CutsceneView>, Without<ExtraView>)>();
    let best = q
        .iter(world)
        .filter(|(_, c)| c.active && c.order < VIEW_ORDER)
        .max_by_key(|(_, c)| c.order)
        .map(|(e, c)| (e, c.projection))?;
    Some((world_pose(world, best.0)?, best.1))
}

/// Where the picture comes from at `t`: the cut's camera, blending from the previous cut's.
pub fn shot_at(
    world: &mut World,
    cutscene: &Cutscene,
    t: f64,
    at: Option<Entity>,
    bind: &HashMap<String, Entity>,
) -> Option<(Entity, Transform, Projection)> {
    let (i, cut) = cutscene.cut_at(t)?;
    let lens_of = |world: &mut World, name: &str| {
        let e = resolve(world, name, at, bind)?;
        let lens = world
            .get::<Camera>(e)
            .map(|c| c.projection)
            .unwrap_or_default();
        Some((e, (world_pose(world, e)?, lens)))
    };
    let (e, now) = lens_of(world, &cut.camera)?;
    let cuts = &cutscene.cuts()?.cuts;
    if cut.blend > 0.0
        && i > 0
        && t < cut.t + cut.blend
        && let Some((_, before)) = lens_of(world, &cuts[i - 1].camera)
    {
        let (pose, lens) = blend(&before, &now, smooth((t - cut.t) / cut.blend));
        return Some((e, pose, lens));
    }
    Some((e, now.0, now.1))
}

/// Puts the view camera where the picture comes from (spawning it the first time).
fn view(world: &mut World, p: &mut Playing, cutscene: &Cutscene, _dt: f64) {
    let Some((cut_cam, mut pose, mut lens)) =
        shot_at(world, cutscene, p.time, p.opts.at, &p.opts.bind)
    else {
        if cutscene.cuts().is_some()
            && let Some((_, cut)) = cutscene.cut_at(p.time)
        {
            note_missing(p, vec![cut.camera.clone()]);
        }
        return;
    };
    if cutscene.blend_in > 0.0
        && p.played < cutscene.blend_in
        && let Some(game) = game_camera(world)
    {
        (pose, lens) = blend(&game, &(pose, lens), smooth(p.played / cutscene.blend_in));
    }
    pose.scale = engine_core::math::Vec3::ONE;
    let exposure = world
        .get::<Camera>(cut_cam)
        .map(|c| c.exposure)
        .unwrap_or_default();
    let camera = Camera {
        projection: lens,
        exposure,
        active: true,
        order: VIEW_ORDER,
    };
    let view = match p.view.filter(|v| world.get_entity(*v).is_ok()) {
        Some(v) => {
            if let Some(mut t) = world.get_mut::<Transform>(v) {
                *t = pose;
            }
            if let Some(mut c) = world.get_mut::<Camera>(v) {
                *c = camera;
            }
            v
        }
        None => world
            .spawn((
                Name::new("Cutscene view"),
                CutsceneView { id: p.id },
                pose,
                camera,
            ))
            .id(),
    };
    p.view = Some(view);
    p.view_pose = Some((pose, lens));
    letterbox(world, view, cutscene.aspect);
}

/// Black bars: the view's viewport the widest (or tallest) `aspect` the window holds.
fn letterbox(world: &mut World, view: Entity, aspect: f64) {
    let window = world
        .get_resource::<Window>()
        .map(|w| UVec2::new(w.width, w.height));
    match window {
        Some(w) if aspect > 0.0 && w.x > 0 && w.y > 0 => {
            let (ww, wh) = (w.x as f64, w.y as f64);
            let size = if ww / wh > aspect {
                UVec2::new((wh * aspect).round() as u32, w.y)
            } else {
                UVec2::new(w.x, (ww / aspect).round() as u32)
            };
            let position = (w - size) / 2;
            let vp = Viewport::new(position, size);
            if world.get::<Viewport>(view) != Some(&vp) {
                world.entity_mut(view).insert(vp);
            }
        }
        _ => {
            if world.get::<Viewport>(view).is_some() {
                world.entity_mut(view).remove::<Viewport>();
            }
        }
    }
}

/// One frame of every cutscene playing: time moves on, events between are sent, objects are
/// posed, the view follows the cuts; at the end they finish (or loop). Not while scripts are
/// held (an editor's edit mode): then nothing plays.
pub(crate) fn run_cutscenes(world: &mut World) {
    if world.contains_resource::<crate::ScriptsHeld>() {
        if let Some(mut cs) = world.get_resource_mut::<Cutscenes>() {
            let cs = &mut *cs;
            let views: Vec<Entity> = cs
                .playing
                .drain(..)
                .filter_map(|p| p.view)
                .chain(cs.outros.drain(..).map(|o| o.view))
                .collect();
            cs.autoplayed.clear();
            cs.outbox.clear();
            for v in views {
                if world.get_entity(v).is_ok() {
                    world.despawn(v);
                }
            }
        }
        return;
    }
    autoplay(world);
    let Some(cs) = world.get_resource::<Cutscenes>() else {
        return;
    };
    if cs.playing.is_empty() && cs.outros.is_empty() && cs.outbox.is_empty() {
        return;
    }
    if let Some(mut files) = world.get_resource_mut::<CutsceneFiles>() {
        files.poll();
    }
    let (world_dt, real_dt) = world
        .get_resource::<Time>()
        .map(|t| (t.delta_secs_f64(), t.real_delta_secs() as f64))
        .unwrap_or((0.0, 0.0));
    let ids: Vec<u64> = world
        .resource::<Cutscenes>()
        .playing
        .iter()
        .map(|p| p.id)
        .collect();
    let mut ended = Vec::new();
    for id in ids {
        let over = with_playing(world, id, |world, p, cutscene| {
            step(world, p, cutscene, world_dt, real_dt)
        });
        if over == Some(true) {
            ended.push(id);
        }
    }
    for id in ended {
        finish(world, id);
    }
    outros(world, real_dt);
    send(world);
}

/// Starts the [`CutscenePlayer`]s that autoplay and haven't yet.
fn autoplay(world: &mut World) {
    let mut q = world.query::<(Entity, &CutscenePlayer)>();
    let due: Vec<(Entity, CutscenePlayer)> = q
        .iter(world)
        .filter(|(_, p)| p.autoplay && !p.path.is_empty())
        .map(|(e, p)| (e, p.clone()))
        .collect();
    if due.is_empty() {
        return;
    }
    for (e, player) in due {
        let mut cs = world.get_resource_or_insert_with(Cutscenes::default);
        if !cs.autoplayed.insert(e) {
            continue;
        }
        let opts = PlayOptions {
            rate: player.rate,
            looping: player.looping,
            restore: player.restore,
            cameras: player.cameras,
            at: Some(e),
            ..Default::default()
        };
        if let Err(err) = play(world, &player.path, opts) {
            tracing::warn!(target: "script", "CutscenePlayer on {e}: {err}");
        }
    }
}

/// One cutscene's frame. Returns whether it reached its end.
fn step(
    world: &mut World,
    p: &mut Playing,
    cutscene: &Cutscene,
    world_dt: f64,
    real_dt: f64,
) -> bool {
    if p.opts.at.is_some_and(|a| world.get_entity(a).is_err()) {
        return true;
    }
    let dt = if p.opts.unscaled { real_dt } else { world_dt };
    let (start, end) = (
        p.start().min(p.end(cutscene.length)),
        p.end(cutscene.length),
    );
    let mut over = false;
    if !p.paused {
        p.played += real_dt.max(dt);
        let rate = p.opts.rate;
        let mut next = p.time + dt * rate;
        let from = p.last.unwrap_or(p.time);
        let first = p.last.is_none();
        if rate >= 0.0 && next >= end {
            events(world, p, cutscene, from, end, false, first);
            if p.opts.looping && end > start {
                next = start + (next - end) % (end - start);
                events(world, p, cutscene, start, next, false, true);
            } else {
                next = end;
                over = true;
            }
        } else if rate < 0.0 && next <= start {
            events(world, p, cutscene, from, start, false, first);
            if p.opts.looping && end > start {
                next = end - (start - next) % (end - start);
                events(world, p, cutscene, end, next, false, true);
            } else {
                next = start;
                over = true;
            }
        } else {
            events(world, p, cutscene, from, next, false, first);
        }
        p.time = next;
        p.last = Some(next);
    }
    let problems = pose(
        world,
        cutscene,
        p.time,
        p.opts.at,
        &p.opts.bind,
        &mut p.saved,
    );
    note_missing(p, problems);
    if p.opts.cameras {
        view(world, p, cutscene, dt);
    }
    over
}

/// Views easing back to the game's camera after their cutscene ended.
fn outros(world: &mut World, real_dt: f64) {
    let mut list = std::mem::take(&mut world.resource_mut::<Cutscenes>().outros);
    let game = game_camera(world);
    list.retain_mut(|o| {
        o.elapsed += real_dt;
        if world.get_entity(o.view).is_err() {
            return false;
        }
        let Some(game) = game else {
            world.despawn(o.view);
            return false;
        };
        if o.elapsed >= o.secs {
            world.despawn(o.view);
            return false;
        }
        let (pose, lens) = blend(&o.from, &game, smooth(o.elapsed / o.secs));
        if let Some(mut t) = world.get_mut::<Transform>(o.view) {
            *t = pose;
        }
        if let Some(mut c) = world.get_mut::<Camera>(o.view) {
            c.projection = lens;
        }
        true
    });
    world.resource_mut::<Cutscenes>().outros.extend(list);
}

/// Hands the frame's events to the scripts (they arrive as messages the next frame).
fn send(world: &mut World) {
    let out = std::mem::take(&mut world.resource_mut::<Cutscenes>().outbox);
    if out.is_empty() {
        return;
    }
    let _ = crate::with_runtime(world, |_, rt, _| {
        let mut s = crate::access::shared(&rt.lua);
        for (to, event, data, from) in out {
            s.outbox.push(crate::access::Outgoing {
                to,
                event,
                data,
                from,
            });
        }
    });
}

/// What the API and the editor show of a playing cutscene.
pub fn describe(p: &Playing) -> Json {
    json!({
        "id": p.id,
        "path": p.path,
        "time": p.time,
        "rate": p.opts.rate,
        "paused": p.paused,
        "loop": p.opts.looping,
        "at": p.opts.at.map(|e| e.to_bits()),
        "missing": p.missing,
    })
}

// ----------------------------------------------------------------------------- scripts

/// `__cutscene_*`, the Rust half of `cutscene.play` (`cutscene.luau` makes the handles).
pub(crate) fn install(lua: &mlua::Lua) -> mlua::Result<()> {
    use crate::access::{read_world_as, shared, with_world_as};
    use crate::entity::EntityRef;
    use mlua::{Table, Value};
    let g = lua.globals();
    g.set(
        "__cutscene_play",
        lua.create_function(|lua, (path, opts): (String, Option<Table>)| {
            let mut o = PlayOptions {
                at: shared(lua).owner,
                ..Default::default()
            };
            if let Some(t) = &opts {
                o.rate = t.get::<Option<f64>>("rate")?.unwrap_or(1.0);
                o.looping = t.get::<Option<bool>>("loop")?.unwrap_or(false);
                o.from = t.get::<Option<f64>>("from")?.unwrap_or(0.0);
                o.to = t.get::<Option<f64>>("to")?;
                o.restore = t.get::<Option<bool>>("restore")?.unwrap_or(false)
                    || t.get::<Option<bool>>("keep")? == Some(false);
                o.cameras = t.get::<Option<bool>>("cameras")?.unwrap_or(true);
                o.unscaled = t.get::<Option<bool>>("unscaled")?.unwrap_or(false);
                if let Value::UserData(ud) = t.get::<Value>("at")? {
                    o.at = Some(ud.borrow::<EntityRef>()?.0);
                }
                if let Some(bind) = t.get::<Option<Table>>("bind")? {
                    for pair in bind.pairs::<String, Value>() {
                        let (name, v) = pair?;
                        let Value::UserData(ud) = v else {
                            return Err(mlua::Error::runtime(format!(
                                "bind.{name} is not an entity"
                            )));
                        };
                        o.bind.insert(name, ud.borrow::<EntityRef>()?.0);
                    }
                }
            }
            with_world_as(lua, "cutscene.play", |w| {
                play(w, &path, o).map_err(mlua::Error::runtime)
            })
        })?,
    )?;
    g.set(
        "__cutscene_ctl",
        lua.create_function(|lua, (id, op, arg): (u64, String, Value)| {
            with_world_as(lua, "cutscene", |w| {
                match op.as_str() {
                    "pause" => set_paused(w, id, true),
                    "play" => set_paused(w, id, false),
                    "stop" => stop(w, id),
                    "skip" => skip(w, id),
                    "rate" => set_rate(w, id, arg.as_f64().unwrap_or(1.0)),
                    "seek" => {
                        let t = match &arg {
                            Value::String(s) => {
                                let name = s.to_string_lossy();
                                let path = w
                                    .get_resource::<Cutscenes>()
                                    .and_then(|c| c.get(id))
                                    .map(|p| p.path.clone());
                                let cutscene = path
                                    .and_then(|p| w.get_resource::<CutsceneFiles>()?.loaded(&p));
                                cutscene.and_then(|c| c.mark(&name)).ok_or_else(|| {
                                    mlua::Error::runtime(format!(
                                        "the cutscene has no mark \"{name}\""
                                    ))
                                })?
                            }
                            v => v.as_f64().unwrap_or(0.0),
                        };
                        seek(w, id, t);
                    }
                    other => {
                        return Err(mlua::Error::runtime(format!("no cutscene control {other}")));
                    }
                }
                Ok(())
            })
        })?,
    )?;
    g.set(
        "__cutscene_info",
        lua.create_function(|lua, id: u64| {
            read_world_as(lua, "cutscene", |w| {
                let p = w.get_resource::<Cutscenes>().and_then(|c| c.get(id));
                let length = p
                    .and_then(|p| w.get_resource::<CutsceneFiles>()?.loaded(&p.path))
                    .map(|c| c.length)
                    .unwrap_or(0.0);
                Ok(match p {
                    Some(p) => (!p.paused, false, p.time, length, p.missing.join(", ")),
                    None => (false, true, 0.0, length, String::new()),
                })
            })
        })?,
    )?;
    g.set(
        "__cutscene_stop_all",
        lua.create_function(|lua, ()| {
            with_world_as(lua, "cutscene.stop_all", |w| {
                stop_all(w);
                Ok(())
            })
        })?,
    )?;
    Ok(())
}

// ----------------------------------------------------------------------------- API

/// `script.cutscene`: read a cutscene, play one, stop or skip, list what plays.
pub(crate) fn register(app: &mut engine_core::App) {
    use engine_core::registry::resolve_entity;
    app.register_api(
        "script.cutscene",
        "Cutscenes (.cutscene files of tracks: values of named objects as curves, events sent to \
         their scripts, camera cuts). {path}: the file's tracks and which names it finds. \
         {path, play: true, at, rate, loop, from, to, restore, cameras, unscaled}: plays it (at: \
         the entity names are looked up below first). {stop: id} or {stop: true} (all), {skip: \
         id} (to the end, retro events sent), {seek: id, t}, {pause: id}, {resume: id}; \
         {playing: true}: what plays now.",
        r#"{"path": "cutscenes/courtyard", "play": true}"#,
        |world, args| {
            if args.get("playing").is_some() {
                let list: Vec<Json> = world
                    .get_resource::<Cutscenes>()
                    .map(|c| c.playing.iter().map(describe).collect())
                    .unwrap_or_default();
                return Ok(json!(list));
            }
            let id_of = |k: &str| args.get(k).and_then(|v| v.as_u64());
            if let Some(stop_arg) = args.get("stop") {
                match stop_arg.as_u64() {
                    Some(id) => stop(world, id),
                    None => stop_all(world),
                }
                return Ok(json!({"stopped": true}));
            }
            if let Some(id) = id_of("skip") {
                skip(world, id);
                return Ok(json!({"skipped": id}));
            }
            if let Some(id) = id_of("seek") {
                seek(
                    world,
                    id,
                    args.get("t").and_then(|v| v.as_f64()).unwrap_or(0.0),
                );
                return Ok(json!({"seeked": id}));
            }
            if let Some(id) = id_of("pause") {
                set_paused(world, id, true);
                return Ok(json!({"paused": id}));
            }
            if let Some(id) = id_of("resume") {
                set_paused(world, id, false);
                return Ok(json!({"resumed": id}));
            }
            let path = args
                .get("path")
                .and_then(|v| v.as_str())
                .ok_or("missing 'path': a .cutscene file, e.g. \"cutscenes/courtyard\"")?
                .to_string();
            if path.split('/').any(|p| p == "..") {
                return Err(format!("'{path}' climbs out of the scripts folder"));
            }
            if args.get("play").and_then(|v| v.as_bool()) == Some(true) {
                let f = |k: &str| args.get(k).and_then(|v| v.as_f64());
                let b = |k: &str| args.get(k).and_then(|v| v.as_bool());
                let at = match args.get("at") {
                    Some(v) if !v.is_null() => Some(resolve_entity(world, v)?),
                    _ => None,
                };
                let opts = PlayOptions {
                    rate: f("rate").unwrap_or(1.0),
                    looping: b("loop").unwrap_or(false),
                    from: f("from").unwrap_or(0.0),
                    to: f("to"),
                    restore: b("restore").unwrap_or(false),
                    cameras: b("cameras").unwrap_or(true),
                    unscaled: b("unscaled").unwrap_or(false),
                    at,
                    bind: HashMap::new(),
                };
                let id = play(world, &path, opts)?;
                return Ok(json!({"id": id}));
            }
            let cutscene = load(world, &path)?;
            let mut found = serde_json::Map::new();
            for name in cutscene.names() {
                let e = resolve(world, &name, None, &HashMap::new());
                found.insert(name, json!(e.map(|e| e.to_bits())));
            }
            Ok(json!({
                "path": normalize(&path),
                "cutscene": serde_json::to_value(&*cutscene).map_err(|e| e.to_string())?,
                "found": found,
            }))
        },
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    const TEXT: &str = r#"{
  "length": 6,
  "aspect": 2.39,
  "marks": [ { "t": 3, "name": "middle" } ],
  "tracks": [
    { "kind": "value", "bind": "Box", "prop": "position",
      "channels": { "y": { "keys": [ { "t": 0, "v": 0, "interp": "linear" }, { "t": 2, "v": 4 } ] } } },
    { "kind": "value", "bind": "Box", "prop": "rotation", "finished": "restore",
      "channels": { "yaw": { "keys": [ { "t": 0, "v": 0, "interp": "linear" }, { "t": 2, "v": 90 } ] } } },
    { "kind": "events", "bind": "", "events": [ { "t": 1, "event": "hello", "data": { "n": 1 } }, { "t": 5, "event": "late", "retro": true } ] },
    { "kind": "cuts", "cuts": [ { "t": 4, "camera": "B", "blend": 1 }, { "t": 0, "camera": "A" } ] }
  ]
}"#;

    #[test]
    fn a_cutscene_reads_writes_and_samples() {
        let c = Cutscene::parse(TEXT).unwrap();
        assert_eq!(c.tracks.len(), 4);
        assert_eq!(c.mark("middle"), Some(3.0));
        // Cuts come sorted; before the first, the first is shown.
        assert_eq!(c.cut_at(0.5).unwrap().1.camera, "A");
        assert_eq!(c.cut_at(4.5).unwrap().1.camera, "B");
        assert_eq!(c.names(), vec!["Box".to_string(), "A".into(), "B".into()]);
        let Track::Value(pos) = &c.tracks[0] else {
            panic!()
        };
        assert_eq!(pos.sample(1.0), vec![("y".to_string(), 2.0)]);
        // Written one key a line, and read back the same.
        let text = c.to_text();
        assert!(
            text.contains("\n          { \"t\": 0.0, \"v\": 0.0, \"interp\": \"linear\" },\n"),
            "{text}"
        );
        assert_eq!(Cutscene::parse(&text).unwrap(), c);
        assert_eq!(c.tracks[0].times(), vec![0.0, 2.0]);
        let mut d = c.clone();
        d.tracks[0].retime(2.0, 2.5);
        assert_eq!(d.tracks[0].times(), vec![0.0, 2.5]);
        assert!(d.tracks[3].remove_at(4.0));
        assert_eq!(d.cut_at(5.0).unwrap().1.camera, "A");
        let Track::Value(v) = &mut d.tracks[0] else {
            panic!()
        };
        v.key("y", 2.51, 9.0, 30.0);
        v.key("x", 1.0, 1.0, 30.0);
        assert_eq!(
            v.channels["y"].keys.len(),
            2,
            "near a key: that key takes it"
        );
        assert_eq!(v.axes(), vec!["x".to_string(), "y".into()]);
        assert!(Cutscene::parse(r#"{ "tracks": [ { "kind": "value", "bind": "A" } ] }"#).is_err());
    }

    #[test]
    fn axes_replace_only_what_they_key() {
        let q = Quat::from_euler(EulerRot::YXZ, 0.3, 0.2, 0.0);
        let r = compose(&Prop::Rotation, &TVal::Quat(q), &[("yaw".into(), 90.0)]).unwrap();
        let TVal::Quat(r) = r else { panic!() };
        let (y, p, _) = r.to_euler(EulerRot::YXZ);
        assert!((y - std::f32::consts::FRAC_PI_2).abs() < 1e-4 && (p - 0.2).abs() < 1e-4);
        let v = compose(
            &Prop::Position,
            &TVal::Vec3(engine_core::math::DVec3::new(1.0, 2.0, 3.0)),
            &[("y".into(), 7.0)],
        )
        .unwrap();
        assert_eq!(v, TVal::Vec3(engine_core::math::DVec3::new(1.0, 7.0, 3.0)));
        let l = compose(
            &Prop::Component("PointLight".into(), vec!["color".into()]),
            &TVal::List(vec![1.0, 1.0, 1.0]),
            &[("y".into(), 0.5)],
        )
        .unwrap();
        assert_eq!(l, TVal::List(vec![1.0, 0.5, 1.0]));
        assert_eq!(axes_for("rotation", None), vec!["yaw", "pitch", "roll"]);
        assert_eq!(
            axes_for("PointLight.color", Some(&TValView::List(3))),
            vec!["x", "y", "z"]
        );
    }
}
