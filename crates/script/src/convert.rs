//! Lua values to JSON and back, the way the engine's `module.verb` functions and the registry
//! expect them: vectors as `[x, y, z]` arrays, quaternions as `[x, y, z, w]`, entities as their
//! id, integral numbers as integers (so they fill `u32` fields), sequences as arrays and every
//! other table as an object.

use mlua::{IntoLua, Lua, Table, Value};
use serde_json::{Map, Number, Value as Json};

use crate::entity::EntityRef;
use crate::math::{Kind, Quat, Vec3, kind};

/// Nested tables deeper than this are refused (a table that contains itself, most likely).
const MAX_DEPTH: u32 = 64;

/// Merges `patch` into `base` the way `entity:set` patches components: objects field by field,
/// anything else replaced.
pub fn merge(base: &mut Json, patch: Json) {
    match (base, patch) {
        (Json::Object(b), Json::Object(p)) => {
            for (k, v) in p {
                merge(b.entry(k).or_insert(Json::Null), v);
            }
        }
        (b, p) => *b = p,
    }
}

/// A Lua value as JSON. Functions, threads and foreign userdata can't be sent to the engine.
pub fn to_json(value: &Value) -> mlua::Result<Json> {
    to_json_at(value, 0)
}

fn to_json_at(value: &Value, depth: u32) -> mlua::Result<Json> {
    if depth > MAX_DEPTH {
        return Err(mlua::Error::runtime(
            "a table nests more than 64 levels deep (does it contain itself?)",
        ));
    }
    Ok(match value {
        Value::Nil => Json::Null,
        Value::Boolean(b) => Json::Bool(*b),
        Value::Integer(i) => Json::Number((*i).into()),
        Value::Number(n) => number(*n),
        Value::String(s) => Json::String(s.to_str()?.to_string()),
        Value::Table(t) => match kind(t) {
            // As arrays, the way components store them.
            Some(Kind::Vec3) => {
                let get = |k: &str| t.raw_get::<f64>(k).map(number);
                Json::Array(vec![get("x")?, get("y")?, get("z")?])
            }
            Some(Kind::Quat) => {
                let get = |k: &str| t.raw_get::<f64>(k).map(number);
                Json::Array(vec![get("x")?, get("y")?, get("z")?, get("w")?])
            }
            None => table_to_json(t, depth)?,
        },
        Value::UserData(ud) => {
            if let Ok(e) = ud.borrow::<EntityRef>() {
                Json::Number(e.0.to_bits().into())
            } else {
                return Err(mlua::Error::runtime(
                    "this value can't be sent to the engine (only tables, numbers, strings, \
                     booleans, vec3, quat and entities can)",
                ));
            }
        }
        Value::Vector(v) => Json::Array(vec![
            number(v.x() as f64),
            number(v.y() as f64),
            number(v.z() as f64),
        ]),
        Value::Function(_) => {
            return Err(mlua::Error::runtime(
                "a function can't be sent to the engine; pass data, or use task.spawn for code",
            ));
        }
        other => {
            return Err(mlua::Error::runtime(format!(
                "a {} can't be sent to the engine",
                other.type_name()
            )));
        }
    })
}

/// Integral values become JSON integers so they deserialize into integer fields too.
fn number(n: f64) -> Json {
    if n.is_finite() && n.fract() == 0.0 && n.abs() < 9.0e15 {
        if n >= 0.0 {
            Json::Number((n as u64).into())
        } else {
            Json::Number((n as i64).into())
        }
    } else {
        Number::from_f64(n).map(Json::Number).unwrap_or(Json::Null)
    }
}

fn table_to_json(t: &Table, depth: u32) -> mlua::Result<Json> {
    let len = t.raw_len();
    let mut count = 0usize;
    let mut is_array = len > 0;
    for pair in t.pairs::<Value, Value>() {
        let (k, _) = pair?;
        count += 1;
        match k {
            Value::Integer(i) if i >= 1 && (i as usize) <= len => {}
            Value::Number(n) if n >= 1.0 && n.fract() == 0.0 && (n as usize) <= len => {}
            _ => is_array = false,
        }
    }
    if is_array && count == len {
        let mut out = Vec::with_capacity(len);
        for i in 1..=len {
            out.push(to_json_at(&t.raw_get::<Value>(i)?, depth + 1)?);
        }
        return Ok(Json::Array(out));
    }
    let mut map = Map::new();
    for pair in t.pairs::<Value, Value>() {
        let (k, v) = pair?;
        let key = match k {
            Value::String(s) => s.to_str()?.to_string(),
            Value::Integer(i) => i.to_string(),
            Value::Number(n) => n.to_string(),
            Value::Boolean(b) => b.to_string(),
            other => {
                return Err(mlua::Error::runtime(format!(
                    "a table key of type {} can't be sent to the engine; use strings or numbers",
                    other.type_name()
                )));
            }
        };
        map.insert(key, to_json_at(&v, depth + 1)?);
    }
    Ok(Json::Object(map))
}

/// JSON as a Lua value: objects and arrays become tables, `null` becomes `nil`.
pub fn to_lua(lua: &Lua, json: &Json) -> mlua::Result<Value> {
    Ok(match json {
        Json::Null => Value::Nil,
        Json::Bool(b) => Value::Boolean(*b),
        Json::Number(n) => Value::Number(n.as_f64().unwrap_or(0.0)),
        Json::String(s) => Value::String(lua.create_string(s)?),
        Json::Array(items) => {
            let t = lua.create_table_with_capacity(items.len(), 0)?;
            for (i, item) in items.iter().enumerate() {
                t.raw_set(i + 1, to_lua(lua, item)?)?;
            }
            Value::Table(t)
        }
        Json::Object(map) => {
            let t = lua.create_table_with_capacity(0, map.len())?;
            for (k, v) in map {
                t.raw_set(k.as_str(), to_lua(lua, v)?)?;
            }
            Value::Table(t)
        }
    })
}

/// JSON as a Lua value shaped like `like`: where `like` holds a `vec3` or `quat`, a matching
/// array becomes one too, and tables recurse. Used for script props, so `{"color": [0, 0, 1]}`
/// in a scene arrives as a `vec3` when the script's default is one.
pub fn to_lua_like(lua: &Lua, json: &Json, like: &Value) -> mlua::Result<Value> {
    match (json, like) {
        (Json::Array(a), Value::Table(t)) if a.len() == 3 && kind(t) == Some(Kind::Vec3) => {
            let f = |i: usize| a[i].as_f64().unwrap_or(0.0);
            Vec3(engine_core::math::DVec3::new(f(0), f(1), f(2))).into_lua(lua)
        }
        (Json::Array(a), Value::Table(t)) if a.len() == 4 && kind(t) == Some(Kind::Quat) => {
            let f = |i: usize| a[i].as_f64().unwrap_or(0.0) as f32;
            Quat(engine_core::math::Quat::from_xyzw(f(0), f(1), f(2), f(3)).normalize())
                .into_lua(lua)
        }
        (Json::Object(map), Value::Table(defaults)) => {
            let t = lua.create_table()?;
            for (k, v) in map {
                let d: Value = defaults.raw_get(k.as_str())?;
                t.raw_set(k.as_str(), to_lua_like(lua, v, &d)?)?;
            }
            Ok(Value::Table(t))
        }
        _ => to_lua(lua, json),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mlua::FromLua;
    use serde_json::json;

    #[test]
    fn round_trips_engine_shapes() {
        let lua = Lua::new();
        crate::math::install(&lua).unwrap();
        let v: Value = lua
            .load(r#"return { at = vec3(1, 2.5, -3), radius = 2, list = {1, 2, 3}, empty = {}, name = "x" }"#)
            .eval()
            .unwrap();
        let j = to_json(&v).unwrap();
        assert_eq!(j["at"], json!([1, 2.5, -3]));
        assert_eq!(j["radius"], json!(2));
        assert!(j["radius"].is_u64(), "integral numbers fill integer fields");
        assert_eq!(j["list"], json!([1, 2, 3]));
        assert_eq!(j["empty"], json!({}));
        assert_eq!(j["name"], json!("x"));
        let back = to_lua(&lua, &j).unwrap();
        let t = back.as_table().unwrap();
        assert_eq!(t.get::<String>("name").unwrap(), "x");
        assert_eq!(t.get::<Table>("at").unwrap().get::<f64>(2).unwrap(), 2.5);
    }

    #[test]
    fn functions_are_refused_with_a_hint() {
        let lua = Lua::new();
        let v: Value = lua.load("return { f = function() end }").eval().unwrap();
        let err = to_json(&v).unwrap_err().to_string();
        assert!(err.contains("function can't be sent"), "{err}");
    }

    #[test]
    fn props_follow_their_defaults_shape() {
        let lua = Lua::new();
        crate::math::install(&lua).unwrap();
        let defaults: Value = lua
            .load("return { color = vec3(1, 1, 1), speed = 2 }")
            .eval()
            .unwrap();
        let v = to_lua_like(&lua, &json!({"color": [0, 0.5, 1], "speed": 3}), &defaults).unwrap();
        let t = v.as_table().unwrap();
        let c: Value = t.get("color").unwrap();
        assert_eq!(kind(c.as_table().unwrap()), Some(Kind::Vec3));
        assert_eq!(Vec3::from_lua(c, &lua).unwrap().0.y, 0.5);
        assert_eq!(t.get::<f64>("speed").unwrap(), 3.0);
    }
}
