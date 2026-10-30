//! Tweens, sequences and springs: a value moved to another over time, run by the runtime each
//! frame after the scripts' updates (Godot's Tween, DOTween's tweens and sequences, Unreal's
//! timelines; springs after Daniel Holden's "Spring-It-On").
//!
//! ```lua
//! self:tween(self.lid, { rotation = quat.euler(0, -1.6) }, 0.6, { ease = "out_back" })
//! self:tween(self.lamp, { ["PointLight.intensity_lm"] = 900 }, 1.5, { loops = -1, yoyo = true })
//! self:tween(self, { glow = 1 }, 0.4):wait()          -- a field of self (number, vec3, quat)
//! self:sequence()
//!     :to(self.bridge, { rotation = down }, 2.4, { ease = "curves/drawbridge.curve" })
//!     :join(self.chain, { scale = vec3(1, 3, 1) }, 2.4)
//!     :interval(0.2)
//!     :call(function() self:emit("lowered") end)
//! self:spring(self.camera, { position = goal }, { halflife = 0.15 })   -- call again to retarget
//! ```
//!
//! What a tween moves: an entity's `position`, `rotation` (turned the short way) or `scale`; a
//! component's number or list field (`PointLight.intensity_lm`, `PointLight.color`); a field
//! of its material (`material.emissive`); or a field of a table (`self.glow`). Its start is
//! what the value is when it starts (`from` to give one; `relative` makes the end an offset).
//! It counts world seconds on its script's clock, like `task.wait` (`unscaled` for real time).
//! A newer tween on the same value takes it over, so tweens started on each press don't fight.
//! It stops with its script, and when its entity goes away. engine-design/scripting-research.md
//! §25.

use std::sync::Arc;

use bevy_ecs::prelude::*;
use engine_core::math::Quat as GQuat;
use engine_core::prelude::{DVec3, LocalTimeScale, Time, Transform};
use engine_core::registry::TypeRegistry;
use mlua::{Function, Lua, RegistryKey, Table, UserData, UserDataFields, UserDataMethods, Value};
use serde_json::{Map, Value as Json};

use crate::access::{read_world_as, shared, with_world_as};
use crate::curve::{Curve, Extend, Interp, Key, ScriptCurves, Tangent};
use crate::ease::{self, Named};
use crate::entity::EntityRef;
use crate::math::{Kind, Quat, Vec3, kind};

// ----------------------------------------------------------------------------- values

/// A value a tween moves.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum TVal {
    Num(f64),
    Vec3(DVec3),
    Quat(GQuat),
    /// A list of numbers (a color, a component's array field).
    List(Vec<f64>),
}

impl TVal {
    fn from_lua(v: &Value) -> Option<TVal> {
        match v {
            Value::Integer(i) => Some(TVal::Num(*i as f64)),
            Value::Number(n) => Some(TVal::Num(*n)),
            Value::Table(t) => match kind(t) {
                Some(Kind::Vec3) => {
                    let f = |k: &str| t.raw_get::<f64>(k).unwrap_or(0.0);
                    Some(TVal::Vec3(DVec3::new(f("x"), f("y"), f("z"))))
                }
                Some(Kind::Quat) => {
                    let f = |k: &str| t.raw_get::<f64>(k).unwrap_or(0.0) as f32;
                    Some(TVal::Quat(GQuat::from_xyzw(f("x"), f("y"), f("z"), f("w"))))
                }
                None => {
                    let n = t.raw_len();
                    if n == 0 {
                        return None;
                    }
                    let mut out = Vec::with_capacity(n);
                    for i in 1..=n {
                        out.push(t.raw_get::<f64>(i).ok()?);
                    }
                    Some(TVal::List(out))
                }
            },
            _ => None,
        }
    }

    fn to_lua(&self, lua: &Lua) -> mlua::Result<Value> {
        use mlua::IntoLua;
        match self {
            TVal::Num(x) => Ok(Value::Number(*x)),
            TVal::Vec3(v) => Vec3(*v).into_lua(lua),
            TVal::Quat(q) => Quat(*q).into_lua(lua),
            TVal::List(l) => Ok(Value::Table(lua.create_sequence_from(l.iter().copied())?)),
        }
    }

    fn from_json(j: &Json) -> Option<TVal> {
        match j {
            Json::Number(n) => n.as_f64().map(TVal::Num),
            Json::Array(a) => a
                .iter()
                .map(|x| x.as_f64())
                .collect::<Option<Vec<f64>>>()
                .map(TVal::List),
            _ => None,
        }
    }

    fn to_json(&self) -> Json {
        match self {
            TVal::Num(x) => Json::from(*x),
            TVal::Vec3(v) => Json::from(vec![v.x, v.y, v.z]),
            TVal::Quat(q) => Json::from(vec![q.x as f64, q.y as f64, q.z as f64, q.w as f64]),
            TVal::List(l) => Json::from(l.clone()),
        }
    }

    pub(crate) fn kind_name(&self) -> &'static str {
        match self {
            TVal::Num(_) => "a number",
            TVal::Vec3(_) => "a vec3",
            TVal::Quat(_) => "a quat",
            TVal::List(_) => "a list of numbers",
        }
    }

    fn comps(&self) -> Vec<f64> {
        match self {
            TVal::Num(x) => vec![*x],
            TVal::Vec3(v) => vec![v.x, v.y, v.z],
            TVal::Quat(q) => vec![q.x as f64, q.y as f64, q.z as f64, q.w as f64],
            TVal::List(l) => l.clone(),
        }
    }

    /// The same numbers as a value shaped like `like` (a vec3 for a list of 3, ...).
    fn like(&self, like: &TVal) -> Option<TVal> {
        let c = self.comps();
        match (like, self) {
            (TVal::Num(_), TVal::Num(_))
            | (TVal::Vec3(_), TVal::Vec3(_))
            | (TVal::Quat(_), TVal::Quat(_)) => Some(self.clone()),
            (TVal::Vec3(_), TVal::Num(x)) => Some(TVal::Vec3(DVec3::splat(*x))),
            (TVal::Vec3(_), _) if c.len() == 3 => Some(TVal::Vec3(DVec3::new(c[0], c[1], c[2]))),
            (TVal::List(l), _) if l.len() == c.len() => Some(TVal::List(c)),
            (TVal::List(l), TVal::Num(x)) => Some(TVal::List(vec![*x; l.len()])),
            _ => None,
        }
    }

    /// From `self` toward `to` by `a` (beyond 0..1 for overshooting eases).
    fn lerp(&self, to: &TVal, a: f64) -> TVal {
        match (self, to) {
            (TVal::Num(x), TVal::Num(y)) => TVal::Num(x + (y - x) * a),
            (TVal::Vec3(x), TVal::Vec3(y)) => TVal::Vec3(*x + (*y - *x) * a),
            (TVal::Quat(x), TVal::Quat(y)) => TVal::Quat(slerp(*x, *y, a as f32)),
            (TVal::List(x), TVal::List(y)) => {
                TVal::List(x.iter().zip(y).map(|(x, y)| x + (y - x) * a).collect())
            }
            _ => to.clone(),
        }
    }

    /// `self` moved by `by` (a relative tween's end): added, or for a rotation turned by it.
    fn plus(&self, by: &TVal) -> Option<TVal> {
        Some(match (self, by.like(self)?) {
            (TVal::Num(x), TVal::Num(y)) => TVal::Num(x + y),
            (TVal::Vec3(x), TVal::Vec3(y)) => TVal::Vec3(*x + y),
            (TVal::Quat(x), TVal::Quat(y)) => TVal::Quat((y * *x).normalize()),
            (TVal::List(x), TVal::List(y)) => {
                TVal::List(x.iter().zip(&y).map(|(x, y)| x + y).collect())
            }
            _ => return None,
        })
    }
}

/// The short way round, extrapolating past either end (overshooting eases).
fn slerp(a: GQuat, b: GQuat, t: f32) -> GQuat {
    let b = if a.dot(b) < 0.0 { -b } else { b };
    let d = (b * a.inverse()).normalize();
    let (axis, angle) = d.to_axis_angle();
    if angle.abs() < 1e-6 {
        return a.lerp(b, t).normalize();
    }
    (GQuat::from_axis_angle(axis, angle * t) * a).normalize()
}

// ----------------------------------------------------------------------------- what moves

/// What a tween moves on its target.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub(crate) enum Prop {
    Position,
    Rotation,
    Scale,
    /// A component's field: its name, then the field's path.
    Component(String, Vec<String>),
    /// A field of the entity's material.
    Material(Vec<String>),
    /// A field of a table.
    Field(String),
}

impl Prop {
    pub(crate) fn parse(name: &str, entity: bool) -> Result<Prop, String> {
        if !entity {
            return Ok(Prop::Field(name.to_string()));
        }
        match name {
            "position" => return Ok(Prop::Position),
            "rotation" => return Ok(Prop::Rotation),
            "scale" => return Ok(Prop::Scale),
            _ => {}
        }
        let mut parts = name.split('.').map(str::to_string);
        let first = parts.next().unwrap_or_default();
        let rest: Vec<String> = parts.collect();
        if first == "material" && !rest.is_empty() {
            return Ok(Prop::Material(rest));
        }
        if first.starts_with(|c: char| c.is_ascii_uppercase()) && !rest.is_empty() {
            return Ok(Prop::Component(first, rest));
        }
        Err(format!(
            "can't tween \"{name}\" on an entity: use position, rotation, scale, a component's \
             field (\"PointLight.intensity_lm\") or the material's (\"material.emissive\")"
        ))
    }

    pub(crate) fn label(&self) -> String {
        match self {
            Prop::Position => "position".into(),
            Prop::Rotation => "rotation".into(),
            Prop::Scale => "scale".into(),
            Prop::Component(c, path) => format!("{c}.{}", path.join(".")),
            Prop::Material(path) => format!("material.{}", path.join(".")),
            Prop::Field(f) => f.clone(),
        }
    }
}

/// What a tween changes: an entity, or a table (by its registry key; `usize` tells tables
/// apart).
pub(crate) enum Target {
    Entity(Entity),
    Table(RegistryKey, usize),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum TargetId {
    Entity(Entity),
    Table(usize),
}

impl Target {
    fn from_lua(lua: &Lua, v: &Value) -> mlua::Result<Target> {
        match v {
            Value::UserData(ud) => {
                let e = ud.borrow::<EntityRef>().map_err(|_| {
                    mlua::Error::runtime("a tween's target is an entity or a table (like self)")
                })?;
                Ok(Target::Entity(e.0))
            }
            Value::Table(t) if kind(t).is_none() => Ok(Target::Table(
                lua.create_registry_value(t.clone())?,
                t.to_pointer() as usize,
            )),
            _ => Err(mlua::Error::runtime(
                "a tween's target is an entity or a table (like self)",
            )),
        }
    }

    fn id(&self) -> TargetId {
        match self {
            Target::Entity(e) => TargetId::Entity(*e),
            Target::Table(_, p) => TargetId::Table(*p),
        }
    }
}

fn read(lua: &Lua, world: &mut World, target: &Target, prop: &Prop) -> Result<TVal, String> {
    match (target, prop) {
        (Target::Entity(e), _) => read_entity(world, *e, prop),
        (Target::Table(key, _), Prop::Field(f)) => {
            let t: Table = lua.registry_value(key).map_err(|e| e.to_string())?;
            let v: Value = t.get(f.as_str()).map_err(|e| e.to_string())?;
            TVal::from_lua(&v).ok_or_else(|| {
                format!(
                    "{f} is {} (a tween moves numbers, vec3s, quats and lists)",
                    type_name(&v)
                )
            })
        }
        _ => Err("can't tween that".into()),
    }
}

/// An entity's value of `prop` (its transform's, a component's field, its material's field).
pub(crate) fn read_entity(world: &mut World, e: Entity, prop: &Prop) -> Result<TVal, String> {
    let value = match prop {
        Prop::Position | Prop::Rotation | Prop::Scale => {
            let t = world.get::<Transform>(e).ok_or("gone")?;
            match prop {
                Prop::Position => TVal::Vec3(t.translation),
                Prop::Rotation => TVal::Quat(t.rotation),
                _ => TVal::Vec3(t.scale.as_dvec3()),
            }
        }
        Prop::Component(c, path) => {
            if world.get_entity(e).is_err() {
                return Err("gone".into());
            }
            let get = world
                .resource::<TypeRegistry>()
                .component_or_err(c)
                .map_err(|e| e.to_string())?
                .get;
            let json = get(world, e).ok_or_else(|| format!("the entity has no {c}"))?;
            let at = path
                .iter()
                .try_fold(&json, |j, k| j.get(k))
                .ok_or_else(|| format!("{c} has no field {}", path.join(".")))?;
            TVal::from_json(at).ok_or_else(|| {
                format!(
                    "{c}.{} is not a number or a list of numbers",
                    path.join(".")
                )
            })?
        }
        Prop::Material(path) => {
            if world.get_entity(e).is_err() {
                return Err("gone".into());
            }
            let json = crate::entity::material(world, e, None).map_err(|e| e.to_string())?;
            let at = path
                .iter()
                .try_fold(&json, |j, k| j.get(k))
                .ok_or_else(|| format!("the material has no field {}", path.join(".")))?;
            TVal::from_json(at).ok_or_else(|| {
                format!(
                    "material.{} is not a number or a list of numbers",
                    path.join(".")
                )
            })?
        }
        Prop::Field(_) => return Err("can't tween that".into()),
    };
    Ok(value)
}

fn type_name(v: &Value) -> &'static str {
    match v {
        Value::Nil => "nil",
        Value::Boolean(_) => "a boolean",
        Value::String(_) => "a string",
        Value::Table(_) => "a table",
        Value::Function(_) => "a function",
        _ => "not a number",
    }
}

fn write(
    lua: &Lua,
    world: &mut World,
    target: &Target,
    prop: &Prop,
    v: &TVal,
) -> Result<(), String> {
    match (target, prop) {
        (Target::Entity(e), _) => write_entity(world, *e, prop, v),
        (Target::Table(key, _), Prop::Field(f)) => {
            let t: Table = lua.registry_value(key).map_err(|e| e.to_string())?;
            t.set(f.as_str(), v.to_lua(lua).map_err(|e| e.to_string())?)
                .map_err(|e| e.to_string())
        }
        _ => Err("can't tween that".into()),
    }
}

/// Sets an entity's value of `prop`.
pub(crate) fn write_entity(
    world: &mut World,
    e: Entity,
    prop: &Prop,
    v: &TVal,
) -> Result<(), String> {
    match prop {
        Prop::Position | Prop::Rotation | Prop::Scale => {
            let Some(mut t) = world.get_mut::<Transform>(e) else {
                return Err("gone".into());
            };
            match (prop, v) {
                (Prop::Position, TVal::Vec3(p)) => t.translation = *p,
                (Prop::Rotation, TVal::Quat(q)) => t.rotation = *q,
                (Prop::Scale, TVal::Vec3(s)) => t.scale = s.as_vec3(),
                _ => return Err(format!("{} can't be {}", prop.label(), v.kind_name())),
            }
        }
        Prop::Component(c, path) => {
            if world.get_entity(e).is_err() {
                return Err("gone".into());
            }
            let patch = world
                .resource::<TypeRegistry>()
                .component_or_err(c)
                .map_err(|e| e.to_string())?
                .patch;
            patch(world, e, nest(path, v.to_json())).map_err(|e| e.to_string())?;
        }
        Prop::Material(path) => {
            if world.get_entity(e).is_err() {
                return Err("gone".into());
            }
            crate::entity::material(world, e, Some(nest(path, v.to_json())))
                .map_err(|e| e.to_string())?;
        }
        Prop::Field(_) => return Err("can't tween that".into()),
    }
    Ok(())
}

/// `{"a": {"b": value}}` for the path `a.b`.
fn nest(path: &[String], value: Json) -> Json {
    path.iter().rev().fold(value, |inner, k| {
        let mut m = Map::new();
        m.insert(k.clone(), inner);
        Json::Object(m)
    })
}

// ----------------------------------------------------------------------------- easing

/// A curve by file (followed as it is edited) or made in a script.
#[derive(Clone, Debug)]
pub enum CurveSrc {
    File(String),
    Inline(Arc<Curve>),
}

impl CurveSrc {
    /// The curve as it is now (a file's, as last loaded).
    pub fn get(&self, world: &World) -> Option<Arc<Curve>> {
        match self {
            CurveSrc::File(path) => world.get_resource::<ScriptCurves>()?.loaded(path),
            CurveSrc::Inline(c) => Some(c.clone()),
        }
    }
}

/// How a tween eases.
pub(crate) enum Ease {
    Named(Named),
    Bezier([f64; 4]),
    Curve(CurveSrc),
    Lua(RegistryKey),
}

impl Ease {
    fn parse(lua: &Lua, v: &Value) -> mlua::Result<Ease> {
        match v {
            // DOTween's default.
            Value::Nil => Ok(Ease::Named(Named::parse("out_quad").expect("a name"))),
            Value::String(s) => {
                let s = s.to_str()?.to_string();
                if s.ends_with(".curve") {
                    load_curve(lua, &s)?;
                    return Ok(Ease::Curve(CurveSrc::File(crate::curve::normalize(&s))));
                }
                Named::parse(&s).map(Ease::Named).ok_or_else(|| {
                    mlua::Error::runtime(format!(
                        "no ease \"{s}\": linear, smooth, smoother, spring, or in_/out_/in_out_/\
                         out_in_ with sine, quad, cubic, quart, quint, expo, circ, back, \
                         elastic or bounce (in_out_cubic); a .curve file; a function; or \
                         {{x1, y1, x2, y2}} as CSS's cubic-bezier"
                    ))
                })
            }
            Value::Function(f) => {
                // The engine's own eases go by name.
                let names: Option<Table> = lua.named_registry_value(EASE_NAMES).ok();
                if let Some(name) = names
                    .and_then(|t| t.raw_get::<Option<String>>(f.clone()).ok().flatten())
                    .and_then(|n| Named::parse(&n))
                {
                    return Ok(Ease::Named(name));
                }
                Ok(Ease::Lua(lua.create_registry_value(f.clone())?))
            }
            Value::UserData(ud) => {
                let c = ud.borrow::<CurveRef>().map_err(|_| {
                    mlua::Error::runtime("an ease is a name, a curve or a function")
                })?;
                Ok(Ease::Curve(c.0.clone()))
            }
            Value::Table(t) => {
                let mut b = [0.0; 4];
                for (i, x) in b.iter_mut().enumerate() {
                    *x = t.raw_get::<f64>(i + 1).map_err(|_| {
                        mlua::Error::runtime("a cubic-bezier ease is {x1, y1, x2, y2}")
                    })?;
                }
                Ok(Ease::Bezier(b))
            }
            _ => Err(mlua::Error::runtime(
                "an ease is a name (\"out_back\"), a .curve file, a curve, a function or \
                 {x1, y1, x2, y2}",
            )),
        }
    }

    fn eval(&self, lua: &Lua, world: &World, p: f64) -> Result<f64, String> {
        Ok(match self {
            Ease::Named(n) => n.eval(p),
            Ease::Bezier(b) => ease::bezier(*b, p),
            Ease::Curve(src) => match src.get(world) {
                Some(c) => {
                    let (a, b) = c.span();
                    c.eval(a + (b - a) * p)
                }
                None => p,
            },
            Ease::Lua(key) => {
                let f: Function = lua.registry_value(key).map_err(|e| e.to_string())?;
                f.call::<f64>(p).map_err(|e| e.to_string())?
            }
        })
    }

    pub(crate) fn label(&self) -> String {
        match self {
            Ease::Named(n) => n.name(),
            Ease::Bezier(b) => format!("bezier({}, {}, {}, {})", b[0], b[1], b[2], b[3]),
            Ease::Curve(CurveSrc::File(p)) => p.clone(),
            Ease::Curve(CurveSrc::Inline(_)) => "a curve".into(),
            Ease::Lua(_) => "a function".into(),
        }
    }
}

/// Registry name of the table of the engine's ease functions by function (to their names).
const EASE_NAMES: &str = "engine_script.ease_names";

// ----------------------------------------------------------------------------- tweens

struct Channel {
    prop: Prop,
    to: TVal,
    relative: bool,
    from_given: Option<TVal>,
    /// Where it starts and ends, once started.
    from: Option<TVal>,
    end: Option<TVal>,
    dead: bool,
}

/// Where the play position is relative to a step.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Zone {
    Unknown,
    Before,
    Inside,
    After,
}

struct Step {
    at: f64,
    secs: f64,
    ease: Ease,
    target: Option<Target>,
    channels: Vec<Channel>,
    /// A callback step.
    call: Option<RegistryKey>,
    started: bool,
    zone: Zone,
    /// A callback fired this pass.
    fired: bool,
}

struct SpringPart {
    prop: Prop,
    goal: TVal,
    /// Its value and velocity, by component (a rotation: its angle-axis offset from the goal).
    x: Option<TVal>,
    v: Vec<f64>,
    dead: bool,
}

struct Spring {
    target: Target,
    parts: Vec<SpringPart>,
    omega: f64,
    zeta: f64,
}

enum Body {
    Steps(Vec<Step>),
    Spring(Spring),
}

/// A tween, a sequence or a spring.
pub(crate) struct Tween {
    pub id: u64,
    pub owner: Option<Entity>,
    pub label: Option<String>,
    body: Body,
    /// The end of the last step added, and where it started (for `join`).
    cursor: f64,
    last_at: f64,
    /// Seconds into this pass.
    pub time: f64,
    delay: f64,
    /// Passes in all (-1: forever) and the ones done.
    pub loops: i32,
    pub pass: u32,
    yoyo: bool,
    backward: bool,
    pub speed: f64,
    pub paused: bool,
    unscaled: bool,
    on_done: Option<RegistryKey>,
    finished: bool,
    complete: bool,
    /// Killed: gone without calling `done`.
    killed: bool,
    /// Made this frame: it starts (at 0) without moving on.
    fresh: bool,
}

impl Tween {
    fn new(id: u64, owner: Option<Entity>, body: Body) -> Self {
        Self {
            id,
            owner,
            label: None,
            body,
            cursor: 0.0,
            last_at: 0.0,
            time: 0.0,
            delay: 0.0,
            loops: 1,
            pass: 0,
            yoyo: false,
            backward: false,
            speed: 1.0,
            paused: false,
            unscaled: false,
            on_done: None,
            finished: false,
            complete: false,
            killed: false,
            fresh: true,
        }
    }

    /// One pass's length.
    pub fn length(&self) -> f64 {
        match &self.body {
            Body::Steps(steps) => steps.iter().map(|s| s.at + s.secs).fold(0.0, f64::max),
            Body::Spring(_) => 0.0,
        }
    }

    /// The play position in the pass (backward passes count down).
    fn u(&self) -> f64 {
        if self.backward {
            self.length() - self.time
        } else {
            self.time
        }
    }

    /// 0..1 over every pass (a forever tween: over this pass).
    pub fn progress(&self) -> f64 {
        let len = self.length();
        if self.finished {
            return 1.0;
        }
        if len <= 0.0 {
            return 0.0;
        }
        let pass = (self.time / len).clamp(0.0, 1.0);
        if self.loops <= 0 {
            pass
        } else {
            ((self.pass as f64 + pass) / self.loops as f64).clamp(0.0, 1.0)
        }
    }

    pub fn is_spring(&self) -> bool {
        matches!(self.body, Body::Spring(_))
    }

    /// The entities and values it moves (for the editor's list).
    pub fn moves(&self) -> Vec<(Option<Entity>, String)> {
        match &self.body {
            Body::Steps(steps) => steps
                .iter()
                .flat_map(|s| {
                    let e = match &s.target {
                        Some(Target::Entity(e)) => Some(*e),
                        _ => None,
                    };
                    s.channels
                        .iter()
                        .filter(|c| !c.dead)
                        .map(move |c| (e, c.prop.label()))
                })
                .collect(),
            Body::Spring(sp) => {
                let e = match &sp.target {
                    Target::Entity(e) => Some(*e),
                    _ => None,
                };
                sp.parts
                    .iter()
                    .filter(|p| !p.dead)
                    .map(|p| (e, p.prop.label()))
                    .collect()
            }
        }
    }

    /// Its eases, by step (for the editor's list).
    pub fn eases(&self) -> Vec<String> {
        match &self.body {
            Body::Steps(steps) => steps
                .iter()
                .filter(|s| s.call.is_none())
                .map(|s| s.ease.label())
                .collect(),
            Body::Spring(sp) => vec![format!(
                "spring {:.2} s{}",
                std::f64::consts::LN_2 * 2.0 / sp.omega,
                if sp.zeta < 0.99 { " bouncy" } else { "" }
            )],
        }
    }

    /// A newer tween took `prop` over: this one stops moving it where it already started.
    fn drop_channel(&mut self, target: TargetId, prop: &Prop) {
        self.stop_moving(target, Some(prop), true);
    }

    /// Stops moving `prop` (every value: None) of `target`. Returns how many it stopped.
    fn stop_moving(&mut self, target: TargetId, prop: Option<&Prop>, started_only: bool) -> usize {
        let mut n = 0;
        match &mut self.body {
            Body::Steps(steps) => {
                for s in steps.iter_mut().filter(|s| s.started || !started_only) {
                    if s.target.as_ref().map(Target::id) == Some(target) {
                        for c in s.channels.iter_mut() {
                            if !c.dead && prop.is_none_or(|p| c.prop == *p) {
                                c.dead = true;
                                n += 1;
                            }
                        }
                    }
                }
            }
            Body::Spring(sp) => {
                if sp.target.id() == target {
                    for p in sp.parts.iter_mut() {
                        if !p.dead && prop.is_none_or(|q| p.prop == *q) {
                            p.dead = true;
                            n += 1;
                        }
                    }
                }
            }
        }
        n
    }

    /// Nothing left to move or call.
    fn empty(&self) -> bool {
        match &self.body {
            Body::Steps(steps) => {
                !steps.is_empty()
                    && steps
                        .iter()
                        .all(|s| s.call.is_none() && s.channels.iter().all(|c| c.dead))
            }
            Body::Spring(sp) => sp.parts.iter().all(|p| p.dead),
        }
    }
}

/// A VM's tweens ([`crate::access::Shared::tweens`]).
#[derive(Default)]
pub(crate) struct Tweens {
    next: u64,
    pub list: Vec<Tween>,
}

impl Tweens {
    fn add(&mut self, mut t: Tween) -> u64 {
        self.next += 1;
        t.id = self.next;
        self.list.push(t);
        self.next
    }

    fn get_mut(&mut self, id: u64) -> Option<&mut Tween> {
        self.list.iter_mut().find(|t| t.id == id)
    }
}

/// A row of the editor's list of running tweens.
#[derive(Clone, Debug)]
pub struct TweenRow {
    pub id: u64,
    pub owner: Option<Entity>,
    pub label: Option<String>,
    pub moves: Vec<(Option<Entity>, String)>,
    pub eases: Vec<String>,
    pub progress: f64,
    pub time: f64,
    pub length: f64,
    pub loops: i32,
    pub pass: u32,
    pub paused: bool,
    pub spring: bool,
}

pub(crate) fn count(lua: &Lua) -> usize {
    shared(lua).tweens.list.len()
}

pub(crate) fn rows(lua: &Lua) -> Vec<TweenRow> {
    shared(lua)
        .tweens
        .list
        .iter()
        .filter(|t| !t.finished)
        .map(|t| TweenRow {
            id: t.id,
            owner: t.owner,
            label: t.label.clone(),
            moves: t.moves(),
            eases: t.eases(),
            progress: t.progress(),
            time: t.time,
            length: t.length(),
            loops: t.loops,
            pass: t.pass,
            paused: t.paused,
            spring: t.is_spring(),
        })
        .collect()
}

/// Stops the tweens `owner`'s script started (it stopped).
pub(crate) fn forget_owner(lua: &Lua, owner: Entity) {
    shared(lua).tweens.list.retain(|t| t.owner != Some(owner));
}

/// What a frame of tweens asks of the runtime: callbacks to run (as tasks of their scripts) and
/// errors to report.
#[derive(Default)]
pub(crate) struct Ran {
    pub calls: Vec<(Option<Entity>, Function)>,
    pub errors: Vec<(Option<Entity>, String)>,
}

/// Runs a frame of `lua`'s tweens.
pub(crate) fn run(lua: &Lua, world: &mut World) -> Ran {
    let mut ran = Ran::default();
    let mut list = std::mem::take(&mut shared(lua).tweens.list);
    if list.is_empty() {
        return ran;
    }
    for i in 0..list.len() {
        let dt = {
            let t = &list[i];
            let time = world.resource::<Time>();
            if t.unscaled {
                time.real_delta_secs() as f64
            } else {
                time.delta_for(t.owner.and_then(|e| world.get::<LocalTimeScale>(e))) as f64
            }
        };
        let starts = advance(&mut list[i], lua, world, dt, &mut ran);
        for (target, prop) in starts {
            for (j, other) in list.iter_mut().enumerate() {
                if j != i {
                    other.drop_channel(target, &prop);
                }
            }
        }
    }
    let mut keep = Vec::with_capacity(list.len());
    for mut t in list {
        if !t.killed && !t.finished && t.empty() {
            // Everything it moved went away, or was taken over.
            t.killed = true;
        }
        if t.finished
            && !t.killed
            && let Some(f) = t
                .on_done
                .as_ref()
                .and_then(|k| lua.registry_value::<Function>(k).ok())
        {
            ran.calls.push((t.owner, f));
        }
        if t.finished || t.killed {
            release(lua, t);
        } else {
            keep.push(t);
        }
    }
    let mut s = shared(lua);
    let added = std::mem::take(&mut s.tweens.list);
    keep.extend(added);
    s.tweens.list = keep;
    ran
}

/// Frees a finished tween's registry keys.
fn release(lua: &Lua, t: Tween) {
    let mut keys = Vec::new();
    keys.extend(t.on_done);
    match t.body {
        Body::Steps(steps) => {
            for s in steps {
                keys.extend(s.call);
                if let Some(Target::Table(k, _)) = s.target {
                    keys.push(k);
                }
                if let Ease::Lua(k) = s.ease {
                    keys.push(k);
                }
            }
        }
        Body::Spring(sp) => {
            if let Target::Table(k, _) = sp.target {
                keys.push(k);
            }
        }
    }
    for k in keys {
        let _ = lua.remove_registry_value(k);
    }
}

/// Moves `t` on by `dt`. Returns the values it started moving (which it takes over).
fn advance(
    t: &mut Tween,
    lua: &Lua,
    world: &mut World,
    dt: f64,
    ran: &mut Ran,
) -> Vec<(TargetId, Prop)> {
    let mut starts = Vec::new();
    if t.finished || t.killed || (t.paused && !t.complete) {
        return starts;
    }
    if let Body::Spring(_) = t.body {
        spring_step(t, lua, world, dt * t.speed, ran, &mut starts);
        return starts;
    }
    let mut dt = (dt * t.speed).max(0.0);
    if t.fresh {
        t.fresh = false;
        dt = 0.0;
    }
    if t.delay > 0.0 && !t.complete {
        let used = dt.min(t.delay);
        t.delay -= used;
        dt -= used;
        if t.delay > 0.0 {
            return starts;
        }
    }
    let len = t.length();
    if t.complete {
        // Kill with completion: straight to the end of the last pass.
        if t.loops > 0 && t.yoyo {
            t.backward = t.loops % 2 == 0;
        }
        t.time = len;
        let u = t.u();
        apply(t, lua, world, u, ran, &mut starts);
        fire(t, u, ran, lua);
        t.finished = true;
        return starts;
    }
    t.time += dt;
    let mut passes = 0;
    while t.time >= len {
        // The end of this pass; what is left over goes into the next.
        let over = t.time - len;
        t.time = len;
        let u = t.u();
        fire(t, u, ran, lua);
        apply(t, lua, world, u, ran, &mut starts);
        t.pass += 1;
        passes += 1;
        if t.loops >= 0 && t.pass >= t.loops.max(1) as u32 {
            t.finished = true;
            return starts;
        }
        // The next pass: back to the start (or turned around).
        if t.yoyo {
            t.backward = !t.backward;
        }
        t.time = over;
        if let Body::Steps(steps) = &mut t.body {
            for s in steps.iter_mut() {
                s.zone = Zone::Unknown;
                s.fired = false;
            }
        }
        if len <= 1e-9 || passes > 64 {
            t.time = 0.0;
            break;
        }
    }
    let u = t.u();
    fire(t, u, ran, lua);
    apply(t, lua, world, u, ran, &mut starts);
    starts
}

/// Fires the callback steps the play position passed this pass.
fn fire(t: &mut Tween, u: f64, ran: &mut Ran, lua: &Lua) {
    let backward = t.backward;
    let owner = t.owner;
    let Body::Steps(steps) = &mut t.body else {
        return;
    };
    for s in steps.iter_mut().filter(|s| s.call.is_some() && !s.fired) {
        let reached = if backward { u <= s.at } else { u >= s.at };
        if reached {
            s.fired = true;
            if let Some(f) = s
                .call
                .as_ref()
                .and_then(|k| lua.registry_value::<Function>(k).ok())
            {
                ran.calls.push((owner, f));
            }
        }
    }
}

/// Writes every step's value at play position `u`: the steps it is inside, and the edge value
/// of those it just left.
fn apply(
    t: &mut Tween,
    lua: &Lua,
    world: &mut World,
    u: f64,
    ran: &mut Ran,
    starts: &mut Vec<(TargetId, Prop)>,
) {
    let owner = t.owner;
    let Body::Steps(steps) = &mut t.body else {
        return;
    };
    for s in steps.iter_mut().filter(|s| s.call.is_none()) {
        let zone = if u < s.at {
            Zone::Before
        } else if u > s.at + s.secs || (s.secs <= 0.0 && u >= s.at) {
            Zone::After
        } else {
            Zone::Inside
        };
        let was = s.zone;
        s.zone = zone;
        let p = match zone {
            Zone::Inside => {
                if s.secs > 0.0 {
                    (u - s.at) / s.secs
                } else {
                    1.0
                }
            }
            Zone::After if was != Zone::After => 1.0,
            // Played back past its start (not a new pass beginning before it).
            Zone::Before if matches!(was, Zone::Inside | Zone::After) => 0.0,
            _ => continue,
        };
        let Some(target) = &s.target else {
            continue;
        };
        if !s.started {
            s.started = true;
            for c in s.channels.iter_mut() {
                match read(lua, world, target, &c.prop) {
                    Ok(now) => {
                        let from = match &c.from_given {
                            Some(f) => f.like(&now).unwrap_or(now.clone()),
                            None => now.clone(),
                        };
                        let end = if c.relative {
                            from.plus(&c.to)
                        } else {
                            c.to.like(&from)
                        };
                        match end {
                            Some(end) => {
                                c.from = Some(from);
                                c.end = Some(end);
                                starts.push((target.id(), c.prop.clone()));
                            }
                            None => {
                                c.dead = true;
                                ran.errors.push((
                                    owner,
                                    format!(
                                        "tween: {} is {}, so it can't go to {}",
                                        c.prop.label(),
                                        now.kind_name(),
                                        c.to.kind_name()
                                    ),
                                ));
                            }
                        }
                    }
                    Err(err) => {
                        c.dead = true;
                        if err != "gone" {
                            ran.errors
                                .push((owner, format!("tween {}: {err}", c.prop.label())));
                        }
                    }
                }
            }
        }
        let e = match s.ease.eval(lua, world, p) {
            Ok(e) => e,
            Err(err) => {
                ran.errors
                    .push((owner, format!("tween: its ease failed: {err}")));
                s.ease = Ease::Named(Named::parse("linear").expect("a name"));
                p
            }
        };
        for c in s.channels.iter_mut().filter(|c| !c.dead) {
            let (Some(from), Some(end)) = (&c.from, &c.end) else {
                continue;
            };
            let v = from.lerp(end, e);
            if let Err(err) = write(lua, world, target, &c.prop, &v) {
                c.dead = true;
                if err != "gone" {
                    ran.errors
                        .push((owner, format!("tween {}: {err}", c.prop.label())));
                }
            }
        }
    }
}

/// A spring's frame: each value follows its goal, exactly (no step size limits).
fn spring_step(
    t: &mut Tween,
    lua: &Lua,
    world: &mut World,
    dt: f64,
    ran: &mut Ran,
    starts: &mut Vec<(TargetId, Prop)>,
) {
    let owner = t.owner;
    let Body::Spring(sp) = &mut t.body else {
        return;
    };
    let mut settled = true;
    for p in sp.parts.iter_mut().filter(|p| !p.dead) {
        let current = match &p.x {
            Some(x) => x.clone(),
            None => match read(lua, world, &sp.target, &p.prop) {
                Ok(now) => {
                    starts.push((sp.target.id(), p.prop.clone()));
                    p.v = vec![
                        0.0;
                        if let TVal::Quat(_) = now {
                            3
                        } else {
                            now.comps().len()
                        }
                    ];
                    now
                }
                Err(err) => {
                    p.dead = true;
                    if err != "gone" {
                        ran.errors
                            .push((owner, format!("spring {}: {err}", p.prop.label())));
                    }
                    continue;
                }
            },
        };
        let Some(goal) = p.goal.like(&current) else {
            p.dead = true;
            ran.errors.push((
                owner,
                format!(
                    "spring: {} is {}, so it can't follow {}",
                    p.prop.label(),
                    current.kind_name(),
                    p.goal.kind_name()
                ),
            ));
            continue;
        };
        let next = match (&current, &goal) {
            (TVal::Quat(q), TVal::Quat(g)) => {
                let mut d = (*q * g.inverse()).normalize();
                if d.w < 0.0 {
                    d = -d;
                }
                let off = d.to_scaled_axis().as_dvec3();
                let mut x = [off.x, off.y, off.z];
                for (k, xk) in x.iter_mut().enumerate() {
                    let (nx, nv) = damped(*xk, p.v[k], 0.0, sp.omega, sp.zeta, dt);
                    *xk = nx;
                    p.v[k] = nv;
                }
                let off = DVec3::new(x[0], x[1], x[2]);
                if off.length() > 1e-4 || p.v.iter().any(|v| v.abs() > 1e-3) {
                    settled = false;
                    TVal::Quat((GQuat::from_scaled_axis(off.as_vec3()) * *g).normalize())
                } else {
                    p.v.iter_mut().for_each(|v| *v = 0.0);
                    goal.clone()
                }
            }
            _ => {
                let (xs, gs) = (current.comps(), goal.comps());
                if p.v.len() != xs.len() {
                    p.v = vec![0.0; xs.len()];
                }
                let mut out = Vec::with_capacity(xs.len());
                let mut far = false;
                for k in 0..xs.len() {
                    let (nx, nv) = damped(xs[k], p.v[k], gs[k], sp.omega, sp.zeta, dt);
                    let scale = gs[k].abs().max(1.0);
                    if (nx - gs[k]).abs() > 1e-4 * scale || nv.abs() > 1e-3 * scale {
                        far = true;
                    }
                    out.push(nx);
                    p.v[k] = nv;
                }
                if far {
                    settled = false;
                    from_comps(&current, out)
                } else {
                    p.v.iter_mut().for_each(|v| *v = 0.0);
                    goal.clone()
                }
            }
        };
        if let Err(err) = write(lua, world, &sp.target, &p.prop, &next) {
            p.dead = true;
            if err != "gone" {
                ran.errors
                    .push((owner, format!("spring {}: {err}", p.prop.label())));
            }
        }
        p.x = Some(next);
    }
    if settled {
        t.finished = true;
    }
}

fn from_comps(like: &TVal, c: Vec<f64>) -> TVal {
    match like {
        TVal::Num(_) => TVal::Num(c[0]),
        TVal::Vec3(_) => TVal::Vec3(DVec3::new(c[0], c[1], c[2])),
        TVal::Quat(_) => TVal::Quat(GQuat::from_xyzw(
            c[0] as f32,
            c[1] as f32,
            c[2] as f32,
            c[3] as f32,
        )),
        TVal::List(_) => TVal::List(c),
    }
}

/// A damped spring's exact step: position `x` and velocity `v` toward `g` after `dt`, for
/// angular frequency `omega` and damping ratio `zeta` (1: critical, no overshoot).
fn damped(x: f64, v: f64, g: f64, omega: f64, zeta: f64, dt: f64) -> (f64, f64) {
    let x0 = x - g;
    if zeta >= 0.999 {
        let e = (-omega * dt).exp();
        let j = v + omega * x0;
        (g + e * (x0 + j * dt), e * (v - omega * j * dt))
    } else {
        let wd = omega * (1.0 - zeta * zeta).sqrt();
        let e = (-zeta * omega * dt).exp();
        let (s, c) = (wd * dt).sin_cos();
        let b = (v + zeta * omega * x0) / wd;
        (
            g + e * (x0 * c + b * s),
            e * (v * c - (omega * omega * x0 + zeta * omega * v) / wd * s),
        )
    }
}

// ----------------------------------------------------------------------------- Lua side

/// `__tween_add`'s arguments: the sequence, how the step goes in (`to`, `join`, `at`,
/// `interval`, `call`), its time for `at`, and the step (target, values, seconds, options).
type AddArgs = (
    u64,
    String,
    Option<f64>,
    Value,
    Option<Table>,
    Option<f64>,
    Option<Table>,
);

/// A curve for scripts (`curve("curves/door.curve")`, `curve({ {0, 0}, {1, 1} })`).
#[derive(Clone, Debug)]
pub struct CurveRef(pub CurveSrc);

impl UserData for CurveRef {
    fn add_fields<F: UserDataFields<Self>>(fields: &mut F) {
        fields.add_field_method_get("path", |_, c| {
            Ok(match &c.0 {
                CurveSrc::File(p) => Some(p.clone()),
                CurveSrc::Inline(_) => None,
            })
        });
        fields.add_field_method_get("first", |lua, c| Ok(curve_now(lua, c)?.span().0));
        fields.add_field_method_get("last", |lua, c| Ok(curve_now(lua, c)?.span().1));
        fields.add_field_method_get("length", |lua, c| {
            let (a, b) = curve_now(lua, c)?.span();
            Ok(b - a)
        });
    }

    fn add_methods<M: UserDataMethods<Self>>(methods: &mut M) {
        methods.add_method("at", |lua, c, t: f64| Ok(curve_now(lua, c)?.eval(t)));
        methods.add_method("keys", |lua, c, ()| {
            let curve = curve_now(lua, c)?;
            let out = lua.create_table()?;
            for (i, k) in curve.keys.iter().enumerate() {
                out.raw_set(i + 1, lua.create_sequence_from([k.t, k.v])?)?;
            }
            Ok(out)
        });
    }
}

fn curve_now(lua: &Lua, c: &CurveRef) -> mlua::Result<Arc<Curve>> {
    match &c.0 {
        CurveSrc::Inline(c) => Ok(c.clone()),
        CurveSrc::File(path) => read_world_as(lua, "curve", |w| {
            w.get_resource::<ScriptCurves>()
                .and_then(|s| s.loaded(path))
                .ok_or_else(|| mlua::Error::runtime(format!("the curve {path} isn't loaded")))
        }),
    }
}

/// Loads a `.curve` file (the scripts folder's) into [`ScriptCurves`].
fn load_curve(lua: &Lua, path: &str) -> mlua::Result<Arc<Curve>> {
    let root = lua
        .app_data_ref::<crate::runtime::Files>()
        .map(|f| f.root.clone())
        .unwrap_or_default();
    with_world_as(lua, "curve", |w| {
        w.get_resource_or_insert_with(ScriptCurves::default)
            .get(&root, path)
            .map_err(mlua::Error::runtime)
    })
}

/// A curve from keys in a script: `{ {t, v}, {t, v, "linear"}, ... }`, or `{ keys = {...},
/// after = "loop" }` with keys as tables `{ t = .., v = .., interp, tangent, ["in"], out }`.
fn inline_curve(t: &Table) -> mlua::Result<Curve> {
    let (list, after, before) = match t.raw_get::<Value>("keys")? {
        Value::Table(keys) => (
            keys,
            t.raw_get::<Option<String>>("after")?,
            t.raw_get::<Option<String>>("before")?,
        ),
        _ => (t.clone(), None, None),
    };
    let extend = |s: Option<String>| -> mlua::Result<Extend> {
        match s.as_deref() {
            None | Some("hold") => Ok(Extend::Hold),
            Some("loop") => Ok(Extend::Loop),
            Some("pingpong") => Ok(Extend::Pingpong),
            Some(other) => Err(mlua::Error::runtime(format!(
                "a curve's after or before is hold, loop or pingpong, not {other}"
            ))),
        }
    };
    let mut keys = Vec::new();
    for k in list.sequence_values::<Table>() {
        let k = k?;
        let get = |name: &str, i: usize| -> mlua::Result<Value> {
            let v: Value = k.raw_get(name)?;
            if v.is_nil() { k.raw_get(i) } else { Ok(v) }
        };
        let num = |v: Value, what: &str| -> mlua::Result<f64> {
            match v {
                Value::Integer(i) => Ok(i as f64),
                Value::Number(n) => Ok(n),
                _ => Err(mlua::Error::runtime(format!(
                    "a curve key is {{t, v}}: its {what} is missing"
                ))),
            }
        };
        let mut key = Key::new(num(get("t", 1)?, "time")?, num(get("v", 2)?, "value")?);
        if let Value::String(s) = get("interp", 3)? {
            key.interp = match &*s.to_str()? {
                "cubic" => Interp::Cubic,
                "linear" => Interp::Linear,
                "constant" => Interp::Constant,
                other => {
                    return Err(mlua::Error::runtime(format!(
                        "a key's interp is cubic, linear or constant, not {other}"
                    )));
                }
            };
        }
        if let Value::String(s) = k.raw_get::<Value>("tangent")? {
            key.tangent = match &*s.to_str()? {
                "auto" => Tangent::Auto,
                "flat" => Tangent::Flat,
                "user" => Tangent::User,
                "break" => Tangent::Break,
                other => {
                    return Err(mlua::Error::runtime(format!(
                        "a key's tangent is auto, flat, user or break, not {other}"
                    )));
                }
            };
        }
        key.slope_in = k.raw_get("in")?;
        key.slope_out = k.raw_get("out")?;
        if (key.slope_in.is_some() || key.slope_out.is_some()) && key.tangent == Tangent::Auto {
            key.tangent = if key.slope_in.is_some() && key.slope_out.is_some() {
                Tangent::Break
            } else {
                Tangent::User
            };
        }
        keys.push(key);
    }
    if keys.is_empty() {
        return Err(mlua::Error::runtime(
            "a curve needs at least one key {t, v}",
        ));
    }
    let mut c = Curve::new(keys);
    c.after = extend(after)?;
    c.before = extend(before)?;
    Ok(c)
}

/// `curve(src)` for a value: a path, keys, or a curve already.
pub(crate) fn curve_of(lua: &Lua, src: &Value) -> mlua::Result<CurveRef> {
    match src {
        Value::String(s) => {
            let path = crate::curve::normalize(&s.to_str()?);
            load_curve(lua, &path)?;
            Ok(CurveRef(CurveSrc::File(path)))
        }
        Value::Table(t) => Ok(CurveRef(CurveSrc::Inline(Arc::new(inline_curve(t)?)))),
        Value::UserData(ud) => Ok(ud.borrow::<CurveRef>()?.clone()),
        _ => Err(mlua::Error::runtime(
            "curve() takes a .curve file's path or keys { {t, v}, ... }",
        )),
    }
}

/// The tween-level options: delay, loops, yoyo, speed, unscaled, done, label, paused.
fn tween_opts(lua: &Lua, t: &mut Tween, opts: &Option<Table>) -> mlua::Result<()> {
    let Some(o) = opts else {
        return Ok(());
    };
    t.delay = o.raw_get::<Option<f64>>("delay")?.unwrap_or(0.0).max(0.0);
    if let Some(n) = o.raw_get::<Option<i64>>("loops")? {
        t.loops = if n < 0 { -1 } else { n.max(1) as i32 };
    }
    t.yoyo = o.raw_get::<Option<bool>>("yoyo")?.unwrap_or(false);
    if t.yoyo && t.loops == 1 {
        // There and back.
        t.loops = 2;
    }
    t.speed = o.raw_get::<Option<f64>>("speed")?.unwrap_or(1.0).max(0.0);
    t.unscaled = o.raw_get::<Option<bool>>("unscaled")?.unwrap_or(false);
    t.paused = o.raw_get::<Option<bool>>("paused")?.unwrap_or(false);
    t.label = o.raw_get::<Option<String>>("label")?;
    if let Some(f) = o.raw_get::<Option<Function>>("done")? {
        t.on_done = Some(lua.create_registry_value(f)?);
    }
    Ok(())
}

/// A step moving `target`'s `props` over `secs` from `at`, with its options (ease, relative,
/// from).
fn make_step(
    lua: &Lua,
    at: f64,
    target: Value,
    props: Table,
    secs: f64,
    opts: &Option<Table>,
) -> mlua::Result<Step> {
    let target = Target::from_lua(lua, &target)?;
    let entity = matches!(target, Target::Entity(_));
    let ease = match opts {
        Some(o) => Ease::parse(lua, &o.raw_get::<Value>("ease")?)?,
        None => Ease::parse(lua, &Value::Nil)?,
    };
    let relative = match opts {
        Some(o) => o.raw_get::<Option<bool>>("relative")?.unwrap_or(false),
        None => false,
    };
    let from: Value = match opts {
        Some(o) => o.raw_get("from")?,
        None => Value::Nil,
    };
    let mut names: Vec<(String, Value)> = Vec::new();
    for pair in props.pairs::<Value, Value>() {
        let (k, v) = pair?;
        let Value::String(k) = k else {
            return Err(mlua::Error::runtime(
                "a tween's values are named: { position = vec3(0, 2, 0) }",
            ));
        };
        names.push((k.to_str()?.to_string(), v));
    }
    names.sort_by(|a, b| a.0.cmp(&b.0));
    if names.is_empty() {
        return Err(mlua::Error::runtime(
            "a tween needs something to move: { position = vec3(0, 2, 0) }",
        ));
    }
    let mut channels = Vec::new();
    for (name, v) in names {
        let prop = Prop::parse(&name, entity).map_err(mlua::Error::runtime)?;
        if let (Target::Entity(_), Prop::Component(c, _)) = (&target, &prop) {
            read_world_as(lua, "tween", |w| {
                w.resource::<TypeRegistry>()
                    .component_or_err(c)
                    .map(|_| ())
                    .map_err(mlua::Error::runtime)
            })?;
        }
        let to = TVal::from_lua(&v).ok_or_else(|| {
            mlua::Error::runtime(format!(
                "tween: {name} goes to {} (a number, vec3, quat or list of numbers)",
                type_name(&v)
            ))
        })?;
        if prop == Prop::Rotation && !matches!(to, TVal::Quat(_)) {
            return Err(mlua::Error::runtime(
                "tween: a rotation goes to a quat (quat.euler(yaw, pitch, roll))",
            ));
        }
        let from_given = match &from {
            Value::Nil => None,
            Value::Table(t) if kind(t).is_none() && t.raw_len() == 0 => {
                TVal::from_lua(&t.raw_get::<Value>(name.as_str())?)
            }
            other => TVal::from_lua(other),
        };
        channels.push(Channel {
            prop,
            to,
            relative,
            from_given,
            from: None,
            end: None,
            dead: false,
        });
    }
    Ok(Step {
        at,
        secs: secs.max(0.0),
        ease,
        target: Some(target),
        channels,
        call: None,
        started: false,
        zone: Zone::Unknown,
        fired: false,
    })
}

fn insert_step(t: &mut Tween, s: Step) {
    if let Body::Steps(steps) = &mut t.body {
        let i = steps.partition_point(|x| x.at <= s.at);
        steps.insert(i, s);
    }
}

/// Installs `curve`, `ease` and the runtime half of `tween` (the handles are in
/// `tween.luau`).
pub(crate) fn install(lua: &Lua) -> mlua::Result<()> {
    let g = lua.globals();
    g.set(
        "curve",
        lua.create_function(|lua, src: Value| curve_of(lua, &src))?,
    )?;

    // ease.<name>(x) for every named ease, and makers for the others.
    let eases = lua.create_table()?;
    let by_fn = lua.create_table()?;
    let mut all = ease::names();
    all.push("in_out".into());
    for name in all {
        let n = Named::parse(&name).expect("names parse");
        let f = lua.create_function(move |_, x: f64| Ok(n.eval(x)))?;
        by_fn.raw_set(f.clone(), name.as_str())?;
        eases.raw_set(name.as_str(), f)?;
    }
    lua.set_named_registry_value(EASE_NAMES, by_fn)?;
    eases.raw_set(
        "bezier",
        lua.create_function(|lua, (x1, y1, x2, y2): (f64, f64, f64, f64)| {
            lua.create_function(move |_, x: f64| Ok(ease::bezier([x1, y1, x2, y2], x)))
        })?,
    )?;
    eases.raw_set(
        "steps",
        lua.create_function(|lua, n: u32| {
            lua.create_function(move |_, x: f64| Ok(ease::steps(n, x)))
        })?,
    )?;
    eases.raw_set(
        "curve",
        lua.create_function(|lua, src: Value| {
            let c = curve_of(lua, &src)?;
            lua.create_function(move |lua, x: f64| {
                let curve = curve_now(lua, &c)?;
                let (a, b) = curve.span();
                Ok(curve.eval(a + (b - a) * x))
            })
        })?,
    )?;
    eases.raw_set(
        "names",
        lua.create_function(|lua, ()| lua.create_sequence_from(ease::names()))?,
    )?;
    g.set("ease", eases)?;

    g.set(
        "__tween_to",
        lua.create_function(
            |lua, (target, props, secs, opts): (Value, Table, f64, Option<Table>)| {
                let step = make_step(lua, 0.0, target, props, secs, &opts)?;
                let owner = shared(lua).owner;
                let mut t = Tween::new(0, owner, Body::Steps(Vec::new()));
                t.cursor = step.secs;
                insert_step(&mut t, step);
                tween_opts(lua, &mut t, &opts)?;
                Ok(shared(lua).tweens.add(t))
            },
        )?,
    )?;
    g.set(
        "__tween_seq",
        lua.create_function(|lua, opts: Option<Table>| {
            let owner = shared(lua).owner;
            let mut t = Tween::new(0, owner, Body::Steps(Vec::new()));
            tween_opts(lua, &mut t, &opts)?;
            Ok(shared(lua).tweens.add(t))
        })?,
    )?;
    g.set(
        "__tween_add",
        lua.create_function(|lua, (id, mode, at, target, props, secs, opts): AddArgs| {
            let step = match mode.as_str() {
                "interval" => None,
                "call" => {
                    let Value::Function(f) = target else {
                        return Err(mlua::Error::runtime("call(f) takes a function"));
                    };
                    Some(Step {
                        at: 0.0,
                        secs: 0.0,
                        ease: Ease::Named(Named::parse("linear").expect("a name")),
                        target: None,
                        channels: Vec::new(),
                        call: Some(lua.create_registry_value(f)?),
                        started: false,
                        zone: Zone::Unknown,
                        fired: false,
                    })
                }
                _ => {
                    let props = props.ok_or_else(|| {
                        mlua::Error::runtime("a step needs values to move: {position = ...}")
                    })?;
                    Some(make_step(
                        lua,
                        0.0,
                        target,
                        props,
                        secs.unwrap_or(0.0),
                        &opts,
                    )?)
                }
            };
            let mut s = shared(lua);
            let t = s.tweens.get_mut(id).ok_or_else(|| {
                mlua::Error::runtime("that sequence has finished (add its steps right away)")
            })?;
            if t.is_spring() {
                return Err(mlua::Error::runtime("a spring has no steps"));
            }
            match (mode.as_str(), step) {
                ("interval", _) => t.cursor += secs.unwrap_or(0.0).max(0.0),
                ("join", Some(mut step)) => {
                    step.at = t.last_at;
                    t.cursor = t.cursor.max(step.at + step.secs);
                    insert_step(t, step);
                }
                ("at", Some(mut step)) => {
                    step.at = at.unwrap_or(0.0).max(0.0);
                    t.cursor = t.cursor.max(step.at + step.secs);
                    insert_step(t, step);
                }
                (_, Some(mut step)) => {
                    step.at = t.cursor;
                    t.last_at = t.cursor;
                    t.cursor += step.secs;
                    insert_step(t, step);
                }
                _ => {}
            }
            Ok(())
        })?,
    )?;
    g.set(
        "__tween_spring",
        lua.create_function(|lua, (target, props, opts): (Value, Table, Option<Table>)| {
            let target = Target::from_lua(lua, &target)?;
            let entity = matches!(target, Target::Entity(_));
            let halflife = opts
                .as_ref()
                .map(|o| o.raw_get::<Option<f64>>("halflife"))
                .transpose()?
                .flatten()
                .unwrap_or(0.15)
                .max(0.005);
            let bounce = opts
                .as_ref()
                .map(|o| o.raw_get::<Option<f64>>("bounce"))
                .transpose()?
                .flatten()
                .unwrap_or(0.0)
                .clamp(0.0, 0.9);
            let mut goals = Vec::new();
            for pair in props.pairs::<String, Value>() {
                let (name, v) = pair?;
                let prop = Prop::parse(&name, entity).map_err(mlua::Error::runtime)?;
                let goal = TVal::from_lua(&v).ok_or_else(|| {
                    mlua::Error::runtime(format!("spring: {name} follows {}", type_name(&v)))
                })?;
                goals.push((prop, goal));
            }
            let id = target.id();
            let owner = shared(lua).owner;
            let mut s = shared(lua);
            // The same value's spring: a new goal, keeping its speed.
            let mut rest = Vec::new();
            for (prop, goal) in goals {
                let mut hit = false;
                for t in s.tweens.list.iter_mut().filter(|t| !t.finished && !t.killed) {
                    if let Body::Spring(sp) = &mut t.body
                        && sp.target.id() == id
                        && let Some(p) = sp.parts.iter_mut().find(|p| p.prop == prop && !p.dead)
                    {
                        p.goal = goal.clone();
                        sp.omega = 2.0 * std::f64::consts::LN_2 / halflife;
                        sp.zeta = 1.0 - bounce;
                        hit = true;
                        break;
                    }
                }
                if !hit {
                    rest.push((prop, goal));
                }
            }
            let existing = s
                .tweens
                .list
                .iter()
                .find(|t| matches!(&t.body, Body::Spring(sp) if !t.finished && sp.target.id() == id))
                .map(|t| t.id);
            if rest.is_empty() {
                drop(s);
                if let Target::Table(k, _) = target {
                    let _ = lua.remove_registry_value(k);
                }
                return Ok(existing.unwrap_or(0));
            }
            let parts = rest
                .into_iter()
                .map(|(prop, goal)| SpringPart {
                    prop,
                    goal,
                    x: None,
                    v: Vec::new(),
                    dead: false,
                })
                .collect();
            let t = Tween::new(
                0,
                owner,
                Body::Spring(Spring {
                    target,
                    parts,
                    omega: 2.0 * std::f64::consts::LN_2 / halflife,
                    zeta: 1.0 - bounce,
                }),
            );
            Ok(s.tweens.add(t))
        })?,
    )?;
    g.set(
        "__tween_ctl",
        lua.create_function(|lua, (id, op, arg): (u64, String, Option<f64>)| {
            let mut s = shared(lua);
            let Some(t) = s.tweens.get_mut(id) else {
                return Ok(false);
            };
            match op.as_str() {
                "pause" => t.paused = true,
                "play" => t.paused = false,
                "kill" => t.killed = true,
                "complete" => t.complete = true,
                "speed" => t.speed = arg.unwrap_or(1.0).max(0.0),
                "seek" | "restart" => {
                    let len = t.length();
                    if op == "restart" {
                        t.pass = 0;
                        t.backward = false;
                        t.paused = false;
                    }
                    t.time = if op == "restart" {
                        0.0
                    } else {
                        arg.unwrap_or(0.0).clamp(0.0, len)
                    };
                    if let Body::Steps(steps) = &mut t.body {
                        for s in steps.iter_mut() {
                            s.zone = Zone::Unknown;
                            if op == "restart" {
                                s.fired = false;
                            }
                        }
                    }
                }
                other => {
                    return Err(mlua::Error::runtime(format!("no tween control {other}")));
                }
            }
            Ok(true)
        })?,
    )?;
    g.set(
        "__tween_info",
        lua.create_function(|lua, id: u64| {
            let s = shared(lua);
            Ok(match s.tweens.list.iter().find(|t| t.id == id) {
                Some(t) if !t.finished && !t.killed => {
                    (!t.paused, false, t.time, t.length(), t.progress())
                }
                _ => (false, true, 0.0, 0.0, 1.0),
            })
        })?,
    )?;
    g.set(
        "__tween_kill",
        lua.create_function(|lua, (target, prop): (Value, Option<String>)| {
            let id = match &target {
                Value::UserData(ud) => TargetId::Entity(ud.borrow::<EntityRef>()?.0),
                Value::Table(t) => TargetId::Table(t.to_pointer() as usize),
                _ => return Err(mlua::Error::runtime("tween.kill(target, value?)")),
            };
            let entity = matches!(id, TargetId::Entity(_));
            let prop = prop
                .map(|p| Prop::parse(&p, entity).map_err(mlua::Error::runtime))
                .transpose()?;
            let mut n = 0;
            for t in shared(lua).tweens.list.iter_mut() {
                if t.stop_moving(id, prop.as_ref(), false) > 0 {
                    n += 1;
                    if t.empty() {
                        t.killed = true;
                    }
                }
            }
            Ok(n)
        })?,
    )?;
    Ok(())
}

// ----------------------------------------------------------------------------- timelines

/// A script's timeline as the editor shows it.
#[derive(Clone, Debug)]
pub struct TimelineInfo {
    pub name: String,
    pub time: f64,
    pub length: f64,
    pub playing: bool,
    /// 1 forward, -1 backward.
    pub direction: f64,
    /// Each track: its name, curve and value now.
    pub tracks: Vec<(String, CurveSrc, f64)>,
    /// Each event: its method's name and time.
    pub events: Vec<(String, f64)>,
}

/// The timelines of the script whose `self` is `this` (`timelines.luau`'s `__timelines_now`),
/// with the world lent for the call.
pub(crate) fn timelines_now(lua: &Lua, world: &mut World, this: &RegistryKey) -> Vec<TimelineInfo> {
    let Ok(Value::Table(me)) = lua.registry_value::<Value>(this) else {
        return Vec::new();
    };
    let Ok(f) = me.get::<Function>("__timelines_now") else {
        return Vec::new();
    };
    let _lent = crate::access::lend_world(lua, world);
    let Ok(list) = f.call::<Table>(me.clone()) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for item in list.sequence_values::<Table>().flatten() {
        let mut tracks = Vec::new();
        if let Ok(ts) = item.get::<Table>("tracks") {
            for t in ts.sequence_values::<Table>().flatten() {
                let (Ok(name), Ok(Value::UserData(ud))) =
                    (t.get::<String>("name"), t.get::<Value>("curve"))
                else {
                    continue;
                };
                let Ok(c) = ud.borrow::<CurveRef>() else {
                    continue;
                };
                tracks.push((name, c.0.clone(), t.get::<f64>("value").unwrap_or(0.0)));
            }
        }
        let mut events = Vec::new();
        if let Ok(es) = item.get::<Table>("events") {
            for e in es.sequence_values::<Table>().flatten() {
                if let (Ok(name), Ok(at)) = (e.get::<String>("name"), e.get::<f64>("at")) {
                    events.push((name, at));
                }
            }
        }
        out.push(TimelineInfo {
            name: item.get("name").unwrap_or_default(),
            time: item.get("time").unwrap_or(0.0),
            length: item.get("length").unwrap_or(0.0),
            playing: item.get("playing").unwrap_or(false),
            direction: item.get("direction").unwrap_or(1.0),
            tracks,
            events,
        });
    }
    out
}
