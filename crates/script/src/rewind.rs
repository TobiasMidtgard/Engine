//! The rewind recorder: what every script did, frame by frame, kept for a while so it can be
//! scrubbed back through (Unreal's Rewind Debugger and Visual Logger, the time travel of Elm's
//! and Redux's debuggers). Each frame it keeps, for each running script:
//!
//! - its values (`self`'s fields, then its props), only when they change;
//! - its state machine's current states (one per parallel region);
//! - where it and the parts it holds (`self.drone`, `self.head`: entities it built or that are
//!   below it) were, only when they move;
//! - what happened: prints, errors, messages sent and received, signals, hot reloads;
//! - the shapes it drew with `draw`.
//!
//! It records while [`ScriptRewind::recording`] is on (the editor turns it on for Play; a game
//! with `script.rewind { "record": true }`), the last [`ScriptRewind::keep_secs`] world seconds
//! or [`ScriptRewind::budget`] bytes, whichever is less. With [`ScriptRewind::shown`] set to a
//! time, [`show_rewind`] draws what the scripts drew then, the path each moving part took
//! around it and where each was; the editor's Rewind tab also puts the entities back where they
//! were while the game is paused. Nothing is replayed: the rest of the world (physics, other
//! modules) isn't recorded, so the game goes on from where it was paused, as in Unreal.
//! engine-design/scripting-research.md §24.

use std::collections::{HashMap, VecDeque};

use bevy_ecs::prelude::*;
use engine_core::prelude::{DVec3, GlobalTransform, Name, Quat, Time, Transform};
use engine_debug::{DebugDraw, DebugShape, DepthMode, ShapeStyle};
use mlua::{Function, Lua, Table, Value};
use serde_json::{Value as Json, json};

use crate::access::shared;
use crate::entity::EntityRef;
use crate::math::{Kind, kind};
use crate::runtime::ScriptRuntime;

/// Values kept per script per frame, at most (its own fields first, then its props).
const MAX_VALUES: usize = 32;
/// Parts kept per script (itself and what it holds).
const MAX_PARTS: usize = 8;
/// Shapes kept per script per frame.
const MAX_SHAPES: usize = 400;
/// Events kept per script per frame.
const MAX_EVENTS: usize = 64;
/// Text kept of a string value or an event.
const MAX_TEXT: usize = 160;
/// Seconds of path drawn either side of the shown time.
pub const TRAIL_SECS: f64 = 4.0;
/// How far back a shape drawn for `seconds` can still show.
const LONGEST_SHAPE: f64 = 30.0;

/// A value of a script's, as recorded.
#[derive(Clone, Debug, PartialEq)]
pub enum Val {
    Bool(bool),
    Num(f64),
    Text(String),
    Vec3(DVec3),
    Quat(Quat),
    /// An entity, with its name when it was recorded.
    Entity(Entity, String),
    /// A table that is not a `vec3` or `quat`: how many items its list holds.
    Table(usize),
}

impl Val {
    /// As the editor shows it (the state log's way: whole numbers plain, others to three
    /// decimals).
    pub fn show(&self) -> String {
        match self {
            Val::Bool(b) => b.to_string(),
            Val::Num(x) => num(*x),
            Val::Text(s) => format!("\"{s}\""),
            Val::Vec3(v) => format!("({}, {}, {})", num(v.x), num(v.y), num(v.z)),
            Val::Quat(q) => {
                let (yaw, pitch, roll) = q.to_euler(engine_core::prelude::EulerRot::YXZ);
                let deg = |r: f32| num((r.to_degrees() as f64 * 10.0).round() / 10.0);
                if pitch.abs() < 1e-3 && roll.abs() < 1e-3 {
                    format!("yaw {}°", deg(yaw))
                } else {
                    format!(
                        "yaw {}° pitch {}° roll {}°",
                        deg(yaw),
                        deg(pitch),
                        deg(roll)
                    )
                }
            }
            Val::Entity(_, name) if name.is_empty() => "an entity".into(),
            Val::Entity(_, name) => name.clone(),
            Val::Table(0) => "{}".into(),
            Val::Table(n) => format!("{{{n} items}}"),
        }
    }

    /// A number to plot: numbers, and booleans as 0 or 1.
    pub fn number(&self) -> Option<f64> {
        match self {
            Val::Num(x) => Some(*x),
            Val::Bool(b) => Some(if *b { 1.0 } else { 0.0 }),
            _ => None,
        }
    }

    fn bytes(&self) -> usize {
        24 + match self {
            Val::Text(s) | Val::Entity(_, s) => s.len(),
            _ => 0,
        }
    }

    fn to_json(&self) -> Json {
        match self {
            Val::Bool(b) => json!(b),
            Val::Num(x) => json!(x),
            Val::Text(s) => json!(s),
            Val::Vec3(v) => json!([v.x, v.y, v.z]),
            Val::Quat(q) => json!([q.x, q.y, q.z, q.w]),
            Val::Entity(e, name) => json!({"entity": e.to_bits(), "name": name}),
            Val::Table(n) => json!({"items": n}),
        }
    }
}

/// A number as the editor shows it: whole numbers plain, others to three decimals.
pub fn num(x: f64) -> String {
    if x == x.floor() && x.abs() < 1e15 {
        format!("{}", x as i64)
    } else {
        let s = format!("{x:.3}");
        s.trim_end_matches('0').trim_end_matches('.').to_string()
    }
}

/// What kind of thing happened.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum EventKind {
    Print,
    Error,
    /// A message it sent (`entity:send`).
    Sent,
    /// A message it got.
    Got,
    /// A signal it emitted (`self:emit`).
    Signal,
    /// Its file was hot reloaded.
    Reload,
}

impl EventKind {
    pub const ALL: [EventKind; 6] = [
        EventKind::Print,
        EventKind::Error,
        EventKind::Sent,
        EventKind::Got,
        EventKind::Signal,
        EventKind::Reload,
    ];

    pub fn name(self) -> &'static str {
        match self {
            EventKind::Print => "print",
            EventKind::Error => "error",
            EventKind::Sent => "sent",
            EventKind::Got => "got",
            EventKind::Signal => "signal",
            EventKind::Reload => "reload",
        }
    }

    fn named(name: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|k| k.name() == name)
    }
}

/// Something a script did in a frame.
#[derive(Clone, Debug, PartialEq)]
pub struct Event {
    pub frame: u64,
    pub kind: EventKind,
    pub text: String,
}

/// What the runtime notes while the recorder records ([`crate::access::Shared::trace`]).
pub(crate) struct Traced {
    pub owner: Entity,
    pub kind: EventKind,
    pub text: String,
    /// The other end of a message.
    pub other: Option<Entity>,
}

impl Traced {
    pub fn new(owner: Entity, kind: EventKind, text: &str) -> Self {
        Self {
            owner,
            kind,
            text: text.to_string(),
            other: None,
        }
    }
}

/// Where a part was.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Pose {
    /// Its `Transform` (in its parent's space).
    pub local: Transform,
    /// Where it was in the world, and which way it faced.
    pub position: DVec3,
    pub rotation: Quat,
}

/// The script's entity, or an entity it holds in a field (`self.drone`).
#[derive(Clone, Debug)]
pub struct Part {
    /// The field (empty: the script's own entity).
    pub field: String,
    /// The entity it was last.
    pub entity: Entity,
    /// Its poses when they changed (None: it wasn't held then, or was gone).
    pub poses: Vec<(u64, Option<Pose>)>,
}

impl Part {
    /// Its pose at `frame`.
    pub fn pose_at(&self, frame: u64) -> Option<&Pose> {
        at(&self.poses, frame).and_then(Option::as_ref)
    }
}

/// One value of a script's over time.
#[derive(Clone, Debug)]
pub struct Series {
    /// Its name: a field (`flown`), or a prop (`props.speed`).
    pub name: String,
    /// The value from each frame on, when it changed (None: unset).
    pub points: Vec<(u64, Option<Val>)>,
}

impl Series {
    pub fn at(&self, frame: u64) -> Option<&Val> {
        at(&self.points, frame).and_then(Option::as_ref)
    }
}

/// One script's recording.
#[derive(Clone, Debug)]
pub struct Track {
    pub entity: Entity,
    pub name: String,
    pub path: String,
    /// The frames it was first and last recorded in.
    pub first: u64,
    pub last: u64,
    /// Itself first, then the entities it holds.
    pub parts: Vec<Part>,
    pub values: Vec<Series>,
    /// Its state machine's current states (one per parallel region), when they changed.
    pub states: Vec<(u64, Vec<String>)>,
    pub events: Vec<Event>,
    /// The shapes it drew, by frame (placed in the world).
    pub shapes: Vec<(u64, Vec<(DebugShape, ShapeStyle)>)>,
    /// Its color in paths and on a timeline.
    pub color: [f32; 4],
}

impl Track {
    /// Whether it was recorded at `frame`.
    pub fn alive_at(&self, frame: u64) -> bool {
        (self.first..=self.last).contains(&frame)
    }

    /// Its current states at `frame` (empty: it has no state machine, or it hadn't started).
    pub fn states_at(&self, frame: u64) -> &[String] {
        at(&self.states, frame).map_or(&[], Vec::as_slice)
    }

    /// The value named `name` at `frame`.
    pub fn value_at(&self, name: &str, frame: u64) -> Option<&Val> {
        self.values.iter().find(|s| s.name == name)?.at(frame)
    }

    /// Its values at `frame`, in order, each with whether it changed right then.
    pub fn values_at(&self, frame: u64) -> Vec<(&str, &Val, bool)> {
        self.values
            .iter()
            .filter_map(|s| {
                let i = s.points.partition_point(|(f, _)| *f <= frame);
                let (f, v) = s.points.get(i.checked_sub(1)?)?;
                let v = v.as_ref()?;
                Some((s.name.as_str(), v, *f == frame && *f != self.first))
            })
            .collect()
    }

    /// Its events from frame `from` to `to`.
    pub fn events_in(&self, from: u64, to: u64) -> &[Event] {
        let a = self.events.partition_point(|e| e.frame < from);
        let b = self.events.partition_point(|e| e.frame <= to);
        &self.events[a..b.max(a)]
    }

    /// The shapes showing at `frame` (time `t`): those it drew then, and those drawn before for
    /// `seconds` that still last.
    pub fn shapes_at<'a>(
        &'a self,
        rewind: &'a ScriptRewind,
        frame: u64,
        t: f64,
    ) -> impl Iterator<Item = &'a (DebugShape, ShapeStyle)> + 'a {
        let end = self.shapes.partition_point(|(f, _)| *f <= frame);
        self.shapes[..end]
            .iter()
            .rev()
            .take_while(move |(f, _)| rewind.time_of(*f).is_some_and(|ft| ft >= t - LONGEST_SHAPE))
            .flat_map(move |(f, list)| {
                let ft = rewind.time_of(*f).unwrap_or(t);
                let now = *f == frame;
                list.iter()
                    .filter(move |(_, style)| now || ft + style.duration as f64 >= t)
            })
    }
}

/// The value at `frame` of points kept when they changed: the last at or before it.
fn at<T>(points: &[(u64, T)], frame: u64) -> Option<&T> {
    let i = points.partition_point(|(f, _)| *f <= frame);
    points.get(i.checked_sub(1)?).map(|(_, v)| v)
}

#[derive(Clone, Copy, Debug)]
struct FrameInfo {
    n: u64,
    t: f64,
    bytes: usize,
}

/// Track colors, in order: bright on dark, told apart at a glance.
pub const COLORS: [[f32; 4]; 8] = [
    [0.27, 0.78, 1.0, 1.0],
    [1.0, 0.62, 0.2, 1.0],
    [0.55, 0.9, 0.35, 1.0],
    [0.95, 0.4, 0.75, 1.0],
    [1.0, 0.88, 0.3, 1.0],
    [0.62, 0.55, 1.0, 1.0],
    [0.3, 0.92, 0.75, 1.0],
    [1.0, 0.45, 0.4, 1.0],
];

/// The recorder (a resource; see the module docs). The editor inserts it; a game can with
/// `script.rewind`.
#[derive(Resource)]
pub struct ScriptRewind {
    pub recording: bool,
    /// World seconds kept.
    pub keep_secs: f64,
    /// Bytes kept, about.
    pub budget: usize,
    /// The time to show (world seconds); None: now, nothing drawn.
    pub shown: Option<f64>,
    /// Show the moment this many seconds before the newest frame, following it (a game's
    /// trailing paths: `script.rewind {show_ago}`).
    pub follow: Option<f64>,
    /// Draw each moving part's path around the shown time.
    pub trails: bool,
    /// Draw the shapes scripts drew at the shown time.
    pub shapes: bool,
    /// The scripts whose paths and shapes show (empty: all).
    pub focus: Vec<Entity>,
    frames: VecDeque<FrameInfo>,
    next: u64,
    tracks: Vec<Track>,
    index: HashMap<Entity, usize>,
    bytes: usize,
    colors: usize,
    /// Changes with every frame recorded and every clear (for UIs to cache by).
    pub version: u64,
}

impl Default for ScriptRewind {
    fn default() -> Self {
        Self {
            recording: false,
            keep_secs: 120.0,
            budget: 96 << 20,
            shown: None,
            follow: None,
            trails: true,
            shapes: true,
            focus: Vec::new(),
            frames: VecDeque::new(),
            next: 0,
            tracks: Vec::new(),
            index: HashMap::new(),
            bytes: 0,
            colors: 0,
            version: 0,
        }
    }
}

impl ScriptRewind {
    /// A recorder that records.
    pub fn recording() -> Self {
        Self {
            recording: true,
            ..Self::default()
        }
    }

    /// Forgets everything recorded.
    pub fn clear(&mut self) {
        self.frames.clear();
        self.tracks.clear();
        self.index.clear();
        self.bytes = 0;
        self.colors = 0;
        self.shown = None;
        self.version += 1;
    }

    /// The first and last recorded times (world seconds).
    pub fn span(&self) -> Option<(f64, f64)> {
        Some((self.frames.front()?.t, self.frames.back()?.t))
    }

    /// The first and last recorded frames.
    pub fn frame_span(&self) -> Option<(u64, u64)> {
        Some((self.frames.front()?.n, self.frames.back()?.n))
    }

    pub fn frame_count(&self) -> usize {
        self.frames.len()
    }

    /// The frame showing at `t`: the last at or before it (the first, before the recording).
    pub fn frame_at(&self, t: f64) -> Option<u64> {
        let i = self.frames.partition_point(|f| f.t <= t);
        self.frames.get(i.saturating_sub(1)).map(|f| f.n)
    }

    /// When `frame` was (world seconds).
    pub fn time_of(&self, frame: u64) -> Option<f64> {
        let first = self.frames.front()?.n;
        self.frames
            .get(usize::try_from(frame.checked_sub(first)?).ok()?)
            .map(|f| f.t)
    }

    pub fn tracks(&self) -> &[Track] {
        &self.tracks
    }

    pub fn track(&self, e: Entity) -> Option<&Track> {
        self.index.get(&e).map(|&i| &self.tracks[i])
    }

    /// Bytes kept, about.
    pub fn bytes(&self) -> usize {
        self.bytes
    }

    /// The track of `e` (made the first time).
    fn track_mut(&mut self, e: Entity, frame: u64) -> &mut Track {
        let i = match self.index.get(&e) {
            Some(&i) => i,
            None => {
                let color = COLORS[self.colors % COLORS.len()];
                self.colors += 1;
                self.tracks.push(Track {
                    entity: e,
                    name: String::new(),
                    path: String::new(),
                    first: frame,
                    last: frame,
                    parts: Vec::new(),
                    values: Vec::new(),
                    states: Vec::new(),
                    events: Vec::new(),
                    shapes: Vec::new(),
                    color,
                });
                self.index.insert(e, self.tracks.len() - 1);
                self.tracks.len() - 1
            }
        };
        &mut self.tracks[i]
    }

    /// Adds one frame of what the scripts did.
    fn push(&mut self, world: &World, t: f64, frame: Frame) {
        if self.frames.back().is_some_and(|f| t < f.t - 1e-9) {
            // The clock went back: another run.
            self.clear();
        }
        let advanced = self.frames.back().is_none_or(|f| t > f.t);
        if !advanced {
            // Paused: nothing moves, but what happened (console code) goes on the last frame.
            if let Some(last) = self.frames.back().map(|f| f.n) {
                let bytes = self.add_events(world, last, frame.traced);
                self.bytes += bytes;
                if let Some(f) = self.frames.back_mut() {
                    f.bytes += bytes;
                }
            }
            return;
        }
        let n = self.next;
        self.next += 1;
        let mut bytes = 16;
        for seen in frame.seen {
            let track = self.track_mut(seen.entity, n);
            bytes += record_track(track, world, n, seen);
        }
        bytes += self.add_events(world, n, frame.traced);
        let mut by_owner: HashMap<Entity, Vec<(DebugShape, ShapeStyle)>> = HashMap::new();
        for (e, shape, style) in frame.drawn {
            let list = by_owner.entry(e).or_default();
            if list.len() < MAX_SHAPES {
                list.push((shape, style));
            }
        }
        for (e, list) in by_owner {
            if let Some(&i) = self.index.get(&e) {
                bytes += list.len() * 96;
                self.tracks[i].shapes.push((n, list));
            }
        }
        self.frames.push_back(FrameInfo { n, t, bytes });
        self.bytes += bytes;
        self.version += 1;
        if let Some(lag) = self.follow {
            self.shown = Some(t - lag);
        }
        self.trim();
    }

    fn add_events(&mut self, world: &World, n: u64, traced: Vec<Traced>) -> usize {
        let mut bytes = 0;
        let mut counts: HashMap<Entity, usize> = HashMap::new();
        for tr in traced {
            let Some(&i) = self.index.get(&tr.owner) else {
                continue;
            };
            let count = counts.entry(tr.owner).or_default();
            *count += 1;
            if *count > MAX_EVENTS {
                continue;
            }
            let other = tr.other.map(|o| name_of(world, o));
            let text = match (tr.kind, other) {
                (EventKind::Sent, Some(to)) => format!("{} to {to}", tr.text),
                (EventKind::Got, Some(from)) => format!("{} from {from}", tr.text),
                _ => tr.text,
            };
            let text = clip(&text);
            bytes += 32 + text.len();
            self.tracks[i].events.push(Event {
                frame: n,
                kind: tr.kind,
                text,
            });
        }
        bytes
    }

    /// Drops what is older than `keep_secs` or over the budget (in steps, so the lists aren't
    /// shifted every frame).
    fn trim(&mut self) {
        let (Some(first), Some(last)) = (self.frames.front(), self.frames.back()) else {
            return;
        };
        let too_old = last.t - first.t > self.keep_secs * 1.1 + 1.0;
        let too_big = self.bytes > self.budget;
        if !too_old && !too_big {
            return;
        }
        let newest = last.t;
        while self.frames.len() > 1 {
            let f = self.frames[0];
            if newest - f.t <= self.keep_secs && self.bytes <= self.budget * 9 / 10 {
                break;
            }
            self.bytes = self.bytes.saturating_sub(f.bytes);
            self.frames.pop_front();
        }
        let Some(cut) = self.frames.front().map(|f| f.n) else {
            return;
        };
        self.tracks.retain(|t| t.last >= cut);
        for track in &mut self.tracks {
            track.first = track.first.max(cut);
            for s in &mut track.values {
                cut_points(&mut s.points, cut);
            }
            for p in &mut track.parts {
                cut_points(&mut p.poses, cut);
            }
            cut_points(&mut track.states, cut);
            let k = track.events.partition_point(|e| e.frame < cut);
            track.events.drain(..k);
            let k = track.shapes.partition_point(|(f, _)| *f < cut);
            track.shapes.drain(..k);
        }
        self.index = self
            .tracks
            .iter()
            .enumerate()
            .map(|(i, t)| (t.entity, i))
            .collect();
    }

    /// `script.rewind`'s summary.
    pub fn describe(&self) -> Json {
        let (from, to) = self.span().unwrap_or((0.0, 0.0));
        json!({
            "recording": self.recording,
            "from": from,
            "to": to,
            "frames": self.frames.len(),
            "kb": self.bytes / 1024,
            "keep_secs": self.keep_secs,
            "shown": self.shown,
            "scripts": self.tracks.iter().map(|t| json!({
                "entity": t.entity.to_bits(),
                "name": t.name,
                "path": t.path,
                "from": self.time_of(t.first),
                "to": self.time_of(t.last),
                "values": t.values.iter().map(|s| s.name.as_str()).collect::<Vec<_>>(),
                "events": t.events.len(),
                "errors": t.events.iter().filter(|e| e.kind == EventKind::Error).count(),
            })).collect::<Vec<_>>(),
        })
    }

    /// `script.rewind {at}`: every script as it was at `t`, with what happened in the half
    /// second before.
    pub fn describe_at(&self, t: f64) -> Json {
        let Some(frame) = self.frame_at(t) else {
            return json!({"t": t, "scripts": []});
        };
        let since = self.frame_at(t - 0.5).unwrap_or(frame);
        let scripts: Vec<Json> = self
            .tracks
            .iter()
            .filter(|tr| tr.alive_at(frame))
            .map(|tr| {
                let values: serde_json::Map<String, Json> = tr
                    .values_at(frame)
                    .into_iter()
                    .map(|(k, v, _)| (k.to_string(), v.to_json()))
                    .collect();
                json!({
                    "entity": tr.entity.to_bits(),
                    "name": tr.name,
                    "path": tr.path,
                    "position": tr.parts.first().and_then(|p| p.pose_at(frame)).map(|p| [p.position.x, p.position.y, p.position.z]),
                    "states": tr.states_at(frame),
                    "values": values,
                    "events": self.events_json(tr, since, frame),
                })
            })
            .collect();
        json!({"t": self.time_of(frame), "frame": frame, "scripts": scripts})
    }

    fn events_json(&self, tr: &Track, from: u64, to: u64) -> Vec<Json> {
        tr.events_in(from, to)
            .iter()
            .map(|e| json!({"t": self.time_of(e.frame), "kind": e.kind.name(), "text": e.text}))
            .collect()
    }

    /// `script.rewind {entity, value}`: one value over the recording, at most `max` points.
    pub fn series_json(&self, e: Entity, name: &str, max: usize) -> Result<Json, String> {
        let tr = self
            .track(e)
            .ok_or("that entity's script wasn't recorded")?;
        let s = tr.values.iter().find(|s| s.name == name).ok_or_else(|| {
            let names: Vec<&str> = tr.values.iter().map(|s| s.name.as_str()).collect();
            format!("no value named {name} (it has {})", names.join(", "))
        })?;
        let step = s.points.len().div_ceil(max.max(1)).max(1);
        let points: Vec<Json> = s
            .points
            .iter()
            .step_by(step)
            .map(|(f, v)| json!([self.time_of(*f), v.as_ref().map(Val::to_json)]))
            .collect();
        Ok(json!({"name": name, "points": points}))
    }

    /// `script.rewind {events}`: what happened (of `kind`, if given), oldest first.
    pub fn events_list(&self, kind: Option<&str>) -> Result<Json, String> {
        let kind = match kind {
            Some(k) => Some(EventKind::named(k).ok_or_else(|| {
                let names: Vec<&str> = EventKind::ALL.iter().map(|k| k.name()).collect();
                format!("no event kind {k} (one of {})", names.join(", "))
            })?),
            None => None,
        };
        let mut all: Vec<(u64, Json)> = Vec::new();
        for tr in &self.tracks {
            for e in tr
                .events
                .iter()
                .filter(|e| kind.is_none_or(|k| e.kind == k))
            {
                all.push((
                    e.frame,
                    json!({
                        "t": self.time_of(e.frame), "entity": tr.entity.to_bits(), "name": tr.name,
                        "kind": e.kind.name(), "text": e.text,
                    }),
                ));
            }
        }
        all.sort_by_key(|(f, _)| *f);
        Ok(Json::Array(all.into_iter().map(|(_, j)| j).collect()))
    }
}

/// Keeps the points from `cut` on, and the one before it (moved to `cut`), so the value at
/// `cut` stays known.
fn cut_points<T>(points: &mut Vec<(u64, T)>, cut: u64) {
    let i = points.partition_point(|(f, _)| *f <= cut);
    if i > 1 {
        points.drain(..i - 1);
    }
    if let Some(p) = points.first_mut()
        && p.0 < cut
    {
        p.0 = cut;
    }
}

fn clip(s: &str) -> String {
    if s.len() <= MAX_TEXT {
        return s.to_string();
    }
    let mut end = MAX_TEXT;
    while !s.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}…", &s[..end])
}

fn name_of(world: &World, e: Entity) -> String {
    world
        .get::<Name>(e)
        .map(|n| n.as_str().to_string())
        .unwrap_or_else(|| format!("entity {}", e.index()))
}

/// One running script as the recorder saw it this frame.
pub(crate) struct Seen {
    entity: Entity,
    path: String,
    values: Vec<(String, Val)>,
    states: Option<Vec<String>>,
    /// The entities it holds that belong to it, by field.
    parts: Vec<(String, Entity)>,
}

/// One frame's worth for [`ScriptRewind::push`].
pub(crate) struct Frame {
    seen: Vec<Seen>,
    traced: Vec<Traced>,
    drawn: Vec<(Entity, DebugShape, ShapeStyle)>,
}

/// Adds what `seen` shows to its track. Returns the bytes added.
fn record_track(track: &mut Track, world: &World, n: u64, seen: Seen) -> usize {
    let mut bytes = 0;
    track.last = n;
    if track.path != seen.path {
        track.path = seen.path;
    }
    let name = name_of(world, seen.entity);
    if track.name != name {
        track.name = name;
    }
    // Values: a point when one changes, and an unset one when it goes.
    let mut present = vec![false; track.values.len()];
    for (name, v) in seen.values {
        let k = match track.values.iter().position(|s| s.name == name) {
            Some(k) => k,
            None => {
                track.values.push(Series {
                    name,
                    points: Vec::new(),
                });
                present.push(false);
                track.values.len() - 1
            }
        };
        present[k] = true;
        let s = &mut track.values[k];
        if s.points
            .last()
            .is_none_or(|(_, last)| last.as_ref() != Some(&v))
        {
            bytes += 8 + v.bytes();
            s.points.push((n, Some(v)));
        }
    }
    for (k, s) in track.values.iter_mut().enumerate() {
        if !present[k] && s.points.last().is_some_and(|(_, v)| v.is_some()) {
            bytes += 16;
            s.points.push((n, None));
        }
    }
    if let Some(states) = seen.states
        && track.states.last().is_none_or(|(_, s)| *s != states)
    {
        bytes += 16 + states.iter().map(|s| s.len() + 24).sum::<usize>();
        track.states.push((n, states));
    }
    // Parts: itself, then what it holds.
    let mut held = vec![(String::new(), seen.entity)];
    held.extend(seen.parts);
    let mut present = vec![false; track.parts.len()];
    for (field, e) in held {
        let k = match track.parts.iter().position(|p| p.field == field) {
            Some(k) => k,
            None if track.parts.len() < MAX_PARTS => {
                track.parts.push(Part {
                    field,
                    entity: e,
                    poses: Vec::new(),
                });
                present.push(false);
                track.parts.len() - 1
            }
            None => continue,
        };
        present[k] = true;
        let part = &mut track.parts[k];
        part.entity = e;
        let pose = pose_of(world, e);
        if part.poses.last().is_none_or(|(_, p)| *p != pose) {
            bytes += 72;
            part.poses.push((n, pose));
        }
    }
    for (k, part) in track.parts.iter_mut().enumerate() {
        if !present[k] && part.poses.last().is_some_and(|(_, p)| p.is_some()) {
            bytes += 16;
            part.poses.push((n, None));
        }
    }
    bytes
}

/// Where `e` is: its own transform, and in the world (its parent's last world transform with
/// its own, so a part moved this frame is where it was put).
fn pose_of(world: &World, e: Entity) -> Option<Pose> {
    let local = *world.get::<Transform>(e)?;
    let parent = world
        .get::<ChildOf>(e)
        .and_then(|c| world.get::<GlobalTransform>(c.parent()));
    let placed = match parent {
        Some(p) => GlobalTransform(p.0 * local.to_affine()),
        None => GlobalTransform(local.to_affine()),
    };
    Some(Pose {
        local,
        position: placed.translation(),
        rotation: placed.rotation(),
    })
}

/// Whether `part` is below `owner` (or was built by it).
fn belongs(world: &World, part: Entity, owner: Entity) -> bool {
    if world
        .get::<crate::Constructed>(part)
        .is_some_and(|c| c.by == owner)
    {
        return true;
    }
    let mut e = part;
    for _ in 0..16 {
        match world.get::<ChildOf>(e) {
            Some(c) if c.parent() == owner => return true,
            Some(c) => e = c.parent(),
            None => return false,
        }
    }
    false
}

/// A Lua value as recorded (None: one it leaves out, like functions).
fn val_of(world: &World, v: &Value) -> Option<Val> {
    match v {
        Value::Boolean(b) => Some(Val::Bool(*b)),
        Value::Integer(i) => Some(Val::Num(*i as f64)),
        Value::Number(n) => Some(Val::Num(*n)),
        Value::String(s) => Some(Val::Text(clip(&s.to_string_lossy()))),
        Value::Table(t) => {
            let f = |k: &str| t.raw_get::<f64>(k).unwrap_or(0.0);
            match kind(t) {
                Some(Kind::Vec3) => Some(Val::Vec3(DVec3::new(f("x"), f("y"), f("z")))),
                Some(Kind::Quat) => Some(Val::Quat(
                    Quat::from_xyzw(f("x") as f32, f("y") as f32, f("z") as f32, f("w") as f32)
                        .normalize(),
                )),
                None => Some(Val::Table(t.raw_len())),
            }
        }
        Value::UserData(ud) => {
            let e = ud.borrow::<EntityRef>().ok()?.0;
            let name = world
                .get::<Name>(e)
                .map(|n| n.as_str().to_string())
                .unwrap_or_default();
            Some(Val::Entity(e, name))
        }
        _ => None,
    }
}

/// What a script's `self` shows: its own fields by name (not `_private` ones, nor `entity`,
/// `props` and the state machine's `state`), then its props; and the entities it holds that
/// belong to it.
fn read_self(world: &World, this: &Table, e: Entity, machine: bool) -> Seen {
    let mut own: Vec<(String, Value)> = Vec::new();
    for (k, v) in this.pairs::<Value, Value>().flatten() {
        let Value::String(k) = k else { continue };
        let Ok(k) = k.to_str().map(|s| s.to_string()) else {
            continue;
        };
        if k.starts_with('_') || k == "entity" || k == "props" || (machine && k == "state") {
            continue;
        }
        own.push((k, v));
    }
    own.sort_by(|a, b| a.0.cmp(&b.0));
    let mut values = Vec::new();
    let mut parts = Vec::new();
    for (k, v) in &own {
        if let Value::UserData(ud) = v
            && let Ok(part) = ud.borrow::<EntityRef>()
            && part.0 != e
            && belongs(world, part.0, e)
        {
            parts.push((k.clone(), part.0));
        }
        if values.len() < MAX_VALUES
            && let Some(val) = val_of(world, v)
        {
            values.push((k.clone(), val));
        }
    }
    if let Ok(Value::Table(props)) = this.raw_get::<Value>("props") {
        let mut list: Vec<(String, Value)> = props
            .pairs::<Value, Value>()
            .flatten()
            .filter_map(|(k, v)| match k {
                Value::String(k) => Some((k.to_string_lossy().to_string(), v)),
                _ => None,
            })
            .collect();
        list.sort_by(|a, b| a.0.cmp(&b.0));
        for (k, v) in list {
            if values.len() >= MAX_VALUES {
                break;
            }
            if let Some(val) = val_of(world, &v) {
                values.push((format!("props.{k}"), val));
            }
        }
    }
    Seen {
        entity: e,
        path: String::new(),
        values,
        states: None,
        parts,
    }
}

/// The current states of a script with a state machine (None: not started).
fn states_now(this: &Table) -> Option<Vec<String>> {
    let f: Function = this.get("__states_now").ok()?;
    let list: Option<Vec<String>> = f.call(this.clone()).ok()?;
    list
}

/// Turns tracing on or off in every VM (what the runtime notes for the recorder).
fn set_tracing(rt: &ScriptRuntime, on: bool) {
    for vm in rt.vms() {
        let mut s = shared(&vm.lua);
        match (on, s.trace.is_some()) {
            (true, false) => s.trace = Some(Vec::new()),
            (false, true) => s.trace = None,
            _ => {}
        }
    }
}

fn take_traced(lua: &Lua) -> Vec<Traced> {
    shared(lua)
        .trace
        .as_mut()
        .map(std::mem::take)
        .unwrap_or_default()
}

/// After the scripts' frame: records it, if a [`ScriptRewind`] records.
pub(crate) fn record(world: &mut World, rt: &mut ScriptRuntime) {
    let on = world
        .get_resource::<ScriptRewind>()
        .is_some_and(|r| r.recording);
    set_tracing(rt, on);
    if !on {
        rt.drawn.clear();
        return;
    }
    let t = world.resource::<Time>().elapsed_secs_f64();
    let mut seen = Vec::new();
    let mut traced = Vec::new();
    for vm in rt.vms() {
        for (&e, i) in &vm.instances {
            if !i.started {
                continue;
            }
            let Ok(Value::Table(this)) = vm.lua.registry_value::<Value>(&i.this) else {
                continue;
            };
            let mut s = read_self(world, &this, e, i.hooks.states);
            s.path = i.path.clone();
            if i.hooks.states {
                s.states = states_now(&this);
            }
            seen.push(s);
        }
        traced.extend(take_traced(&vm.lua));
    }
    let drawn = std::mem::take(&mut rt.drawn);
    world.resource_scope(|world, mut rw: Mut<ScriptRewind>| {
        rw.push(
            world,
            t,
            Frame {
                seen,
                traced,
                drawn,
            },
        );
    });
}

/// The world moved to a new frame of reference: what was recorded is in the old one, so the
/// recording starts again.
pub(crate) fn forget_on_shift(world: &mut World) {
    if let Some(mut rw) = world.get_resource_mut::<ScriptRewind>()
        && rw.frame_count() > 0
    {
        rw.clear();
    }
}

/// Draws the shown time ([`ScriptRewind::shown`]): the shapes scripts drew then, each moving
/// part's path around it (bright behind, faint ahead, a dot each second) and where each part
/// was.
pub fn show_rewind(rewind: Option<Res<ScriptRewind>>, debug: Option<ResMut<DebugDraw>>) {
    let (Some(rw), Some(mut debug)) = (rewind, debug) else {
        return;
    };
    let Some(t) = rw.shown else {
        return;
    };
    let Some(frame) = rw.frame_at(t) else {
        return;
    };
    for track in rw
        .tracks
        .iter()
        .filter(|tr| rw.focus.is_empty() || rw.focus.contains(&tr.entity))
        .filter(|tr| tr.alive_at(frame))
    {
        if rw.shapes {
            for (shape, style) in track.shapes_at(&rw, frame, t) {
                debug.styled(
                    shape,
                    ShapeStyle {
                        duration: 0.0,
                        ..*style
                    },
                );
            }
        }
        if rw.trails {
            for part in &track.parts {
                trail(&rw, &mut debug, track.color, part, frame, t);
            }
        }
    }
}

/// One part's path from `TRAIL_SECS` before `t` to as long after, if it moved.
fn trail(
    rw: &ScriptRewind,
    debug: &mut DebugDraw,
    color: [f32; 4],
    part: &Part,
    frame: u64,
    t: f64,
) {
    let from = rw.frame_at(t - TRAIL_SECS).unwrap_or(frame);
    let to = rw.frame_at(t + TRAIL_SECS).unwrap_or(frame);
    let a = part
        .poses
        .partition_point(|(f, _)| *f < from)
        .saturating_sub(1);
    let b = part.poses.partition_point(|(f, _)| *f <= to);
    let window = &part.poses[a..b.max(a)];
    let points = |range: &[(u64, Option<Pose>)]| -> Vec<DVec3> {
        range
            .iter()
            .filter_map(|(_, p)| p.map(|p| p.position))
            .collect()
    };
    // Only parts that went somewhere (not a barrel's kick or a bob in place).
    let all = points(window);
    let Some(&start) = all.first() else {
        return;
    };
    if all.iter().all(|p| (*p - start).length() < 0.3) {
        return;
    }
    let split = window.partition_point(|(f, _)| *f <= frame);
    let (past, ahead) = window.split_at(split);
    let mut past = points(past);
    let mut ahead_points = points(ahead);
    if let Some(&now) = past.last() {
        ahead_points.insert(0, now);
    }
    if let Some(&first_ahead) = ahead_points.get(1)
        && past.is_empty()
    {
        past.push(first_ahead);
    }
    let style = |alpha: f32| ShapeStyle {
        color: [color[0], color[1], color[2], alpha],
        duration: 0.0,
        depth: DepthMode::OnTop,
    };
    if past.len() > 1 {
        debug.styled(
            &DebugShape::Path {
                points: past.clone(),
                closed: false,
            },
            style(0.95),
        );
    }
    if ahead_points.len() > 1 {
        debug.styled(
            &DebugShape::Path {
                points: ahead_points,
                closed: false,
            },
            style(0.3),
        );
    }
    // A dot each whole second, then where it was.
    let mut next_tick = (t - TRAIL_SECS).ceil();
    for (f, p) in window {
        let (Some(p), Some(ft)) = (p, rw.time_of(*f)) else {
            continue;
        };
        if ft >= next_tick {
            next_tick = ft.floor() + 1.0;
            debug.styled(
                &DebugShape::Cross {
                    at: p.position,
                    size: 0.12,
                },
                style(if *f <= frame { 0.9 } else { 0.35 }),
            );
        }
    }
    if let Some(p) = part.pose_at(frame) {
        debug.styled(
            &DebugShape::Sphere {
                center: p.position,
                radius: 0.09,
            },
            style(1.0),
        );
    }
}
