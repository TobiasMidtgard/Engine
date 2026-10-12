//! [`UiData`]: a JSON value tree the UI binds to, and the `{path}` template syntax.
//!
//! Games (or an AI agent) write values (`ui.set_data {"path": "player.gold", "value": 120}`);
//! any [`Text`](crate::Text) containing `{player.gold}` updates by itself. Templates can also
//! read ECS components directly: `{@Player.Health.current}` is field `current` of the
//! registered component `Health` on the entity named `Player`.

use bevy_ecs::prelude::*;
use engine_core::registry::TypeRegistry;
use serde_json::Value;

/// The UI's data model. Edit through [`UiData::set`] so bound text refreshes.
#[derive(Resource, Debug, Clone, Default)]
pub struct UiData {
    root: Value,
    /// Bumps on every change.
    pub version: u64,
}

impl UiData {
    /// Value at a dotted path (`"player.stats.speed"`, `"party[0].name"` or `"party.0.name"`).
    pub fn get(&self, path: &str) -> Option<&Value> {
        let mut v = &self.root;
        for key in split_path(path) {
            v = match v {
                Value::Object(m) => m.get(key)?,
                Value::Array(a) => a.get(key.parse::<usize>().ok()?)?,
                _ => return None,
            };
        }
        Some(v)
    }

    pub fn get_f64(&self, path: &str) -> Option<f64> {
        self.get(path).and_then(Value::as_f64)
    }

    pub fn get_str(&self, path: &str) -> Option<&str> {
        self.get(path).and_then(Value::as_str)
    }

    /// Set the value at a dotted path, creating objects on the way.
    pub fn set(&mut self, path: &str, value: impl Into<Value>) {
        let value = value.into();
        let keys: Vec<&str> = split_path(path).collect();
        if keys.is_empty() {
            self.root = value;
            self.version += 1;
            return;
        }
        let mut v = &mut self.root;
        for key in &keys[..keys.len() - 1] {
            if let Some(i) = array_index(v, key) {
                v = &mut v.as_array_mut().expect("array")[i];
                continue;
            }
            if !v.is_object() {
                *v = Value::Object(Default::default());
            }
            v = v
                .as_object_mut()
                .expect("object")
                .entry(key.to_string())
                .or_insert(Value::Null);
        }
        let last = keys[keys.len() - 1];
        if let Some(i) = array_index(v, last) {
            let a = v.as_array_mut().expect("array");
            if a[i] != value {
                a[i] = value;
                self.version += 1;
            }
            return;
        }
        if !v.is_object() {
            *v = Value::Object(Default::default());
        }
        let m = v.as_object_mut().expect("object");
        if m.get(last) != Some(&value) {
            m.insert(last.to_string(), value);
            self.version += 1;
        }
    }

    /// The whole tree.
    pub fn root(&self) -> &Value {
        &self.root
    }
}

/// `key` as an index into `v` when `v` is an array that long.
fn array_index(v: &Value, key: &str) -> Option<usize> {
    let a = v.as_array()?;
    key.parse::<usize>().ok().filter(|i| *i < a.len())
}

fn split_path(path: &str) -> impl Iterator<Item = &str> {
    path.split(['.', '[', ']']).filter(|s| !s.is_empty())
}

/// Whether `text` contains a `{...}` binding.
pub fn has_bindings(text: &str) -> bool {
    text.contains('{')
}

/// Replace every `{path}` / `{path:N}` (N decimals) / `{@Entity.Component.field}` /
/// `{$Resource.field}` in `template`. Unknown paths become `?`. `{{` and `}}` are literal braces.
pub fn resolve(template: &str, data: &UiData, world: Option<&World>) -> String {
    let mut out = String::with_capacity(template.len());
    let mut rest = template;
    while let Some(i) = rest.find('{') {
        out.push_str(&rest[..i].replace("}}", "}"));
        rest = &rest[i..];
        if rest.starts_with("{{") {
            out.push('{');
            rest = &rest[2..];
            continue;
        }
        let Some(end) = rest.find('}') else {
            break;
        };
        let inner = &rest[1..end];
        let (path, decimals) = match inner.rsplit_once(':') {
            Some((p, d)) if d.chars().all(|c| c.is_ascii_digit()) && !d.is_empty() => {
                (p, d.parse::<usize>().ok())
            }
            _ => (inner, None),
        };
        let value = lookup(path, data, world);
        out.push_str(&format_value(value.as_ref(), decimals));
        rest = &rest[end + 1..];
    }
    out.push_str(&rest.replace("}}", "}"));
    out
}

/// The value a binding path names: a [`UiData`] path, `@Entity.Component.field` or
/// `$Resource.field`.
pub fn lookup(path: &str, data: &UiData, world: Option<&World>) -> Option<Value> {
    if let Some(ecs) = path.strip_prefix('@') {
        world.and_then(|w| ecs_value(w, ecs))
    } else if let Some(res) = path.strip_prefix('$') {
        world.and_then(|w| resource_value(w, res))
    } else {
        data.get(path).cloned()
    }
}

/// A binding path as a number (missing or non-numeric = None).
pub fn lookup_f64(world: &World, path: &str) -> Option<f64> {
    let data = world.get_resource::<UiData>()?;
    lookup(path, data, Some(world)).and_then(|v| match v {
        Value::Number(n) => n.as_f64(),
        Value::Bool(b) => Some(b as u8 as f64),
        Value::String(s) => s.parse().ok(),
        _ => None,
    })
}

/// Write `value` to a binding path: a [`UiData`] path or `$Resource.field` (patched through
/// the type registry). Entity paths (`@...`) are read-only here.
pub fn write(world: &mut World, path: &str, value: Value) -> Result<(), String> {
    if let Some(res) = path.strip_prefix('$') {
        let (name, field) = res
            .split_once('.')
            .ok_or_else(|| format!("\"{path}\" needs a field: $Resource.field."))?;
        let registry = world.resource::<TypeRegistry>().clone();
        let entry = registry.resource_or_err(name)?;
        let mut patch = value;
        for key in field.rsplit('.') {
            let mut m = serde_json::Map::new();
            m.insert(key.to_string(), patch);
            patch = Value::Object(m);
        }
        return (entry.patch)(world, patch);
    }
    if path.starts_with('@') {
        return Err(format!(
            "\"{path}\" is an entity field; change it with entity.patch."
        ));
    }
    world.resource_mut::<UiData>().set(path, value);
    Ok(())
}

fn resource_value(world: &World, path: &str) -> Option<Value> {
    let mut parts = path.splitn(2, '.');
    let name = parts.next()?;
    let entry = world.get_resource::<TypeRegistry>()?.resource(name)?;
    let mut v = (entry.get)(world)?;
    if let Some(field) = parts.next() {
        for key in split_path(field) {
            v = match v {
                Value::Object(mut m) => m.remove(key)?,
                _ => return None,
            };
        }
    }
    Some(v)
}

/// `Name.Component.field.sub` → JSON value through the type registry.
fn ecs_value(world: &World, path: &str) -> Option<Value> {
    let mut parts = path.splitn(3, '.');
    let name = parts.next()?;
    let component = parts.next()?;
    let field = parts.next();
    let registry = world.get_resource::<TypeRegistry>()?;
    let entry = registry.component(component)?;
    let entity = find_named(world, name)?;
    let mut v = (entry.get)(world, entity)?;
    if let Some(field) = field {
        for key in split_path(field) {
            v = match v {
                Value::Object(mut m) => m.remove(key)?,
                Value::Array(mut a) => {
                    let i = key.parse::<usize>().ok()?;
                    if i >= a.len() {
                        return None;
                    }
                    a.swap_remove(i)
                }
                _ => return None,
            };
        }
    }
    Some(v)
}

fn find_named(world: &World, name: &str) -> Option<Entity> {
    let mut q = world.try_query::<(Entity, &Name)>()?;
    q.iter(world)
        .find(|(_, n)| n.as_str() == name)
        .map(|(e, _)| e)
}

/// Numbers without needless decimals, strings as-is, others as JSON.
pub fn format_value(v: Option<&Value>, decimals: Option<usize>) -> String {
    match v {
        None | Some(Value::Null) => "?".into(),
        Some(Value::String(s)) => s.clone(),
        Some(Value::Number(n)) => {
            let f = n.as_f64().unwrap_or(0.0);
            match decimals {
                Some(d) => format!("{f:.d$}"),
                None if n.is_i64() || n.is_u64() => group_thousands(n.to_string()),
                None if f.fract().abs() < 1e-9 && f.abs() < 1e15 => {
                    group_thousands(format!("{}", f as i64))
                }
                None => format!("{f:.2}")
                    .trim_end_matches('0')
                    .trim_end_matches('.')
                    .to_string(),
            }
        }
        Some(Value::Bool(b)) => b.to_string(),
        Some(other) => other.to_string(),
    }
}

/// `12345` → `12,345`.
pub fn group_thousands(s: String) -> String {
    let (sign, digits) = match s.strip_prefix('-') {
        Some(d) => ("-", d),
        None => ("", s.as_str()),
    };
    if digits.len() <= 3 || !digits.chars().all(|c| c.is_ascii_digit()) {
        return s;
    }
    let mut out = String::with_capacity(digits.len() + digits.len() / 3);
    for (i, c) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i) % 3 == 0 {
            out.push(',');
        }
        out.push(c);
    }
    format!("{sign}{out}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn set_get_and_version() {
        let mut d = UiData::default();
        d.set("player.gold", 120);
        d.set("player.name", "Mara");
        d.set("party", json!([{"name": "Quill"}]));
        d.set("party[0].name", "Quill the Bold");
        assert_eq!(d.get_f64("player.gold"), Some(120.0));
        assert_eq!(d.get_str("party.0.name"), Some("Quill the Bold"));
        let v = d.version;
        d.set("player.gold", 120);
        assert_eq!(d.version, v, "same value: no change");
    }

    #[test]
    fn templates() {
        let mut d = UiData::default();
        d.set("player.gold", 12500);
        d.set("player.speed", 4.26);
        d.set("player.name", "Mara");
        assert_eq!(
            resolve(
                "{player.name}: {player.gold} gold, {player.speed:1} m/s {{x}} {nope}",
                &d,
                None
            ),
            "Mara: 12,500 gold, 4.3 m/s {x} ?"
        );
        assert_eq!(resolve("no bindings", &d, None), "no bindings");
    }

    #[test]
    fn ecs_bindings_read_registered_components() {
        use engine_core::prelude::*;
        #[derive(Component, Serialize, Deserialize, Default)]
        #[serde(default)]
        struct Health {
            current: f32,
            max: f32,
        }
        let mut app = App::new();
        app.register_component::<Health>("hp");
        app.world.spawn((
            Name::new("Player"),
            Health {
                current: 75.0,
                max: 100.0,
            },
        ));
        let d = UiData::default();
        assert_eq!(
            resolve(
                "{@Player.Health.current} / {@Player.Health.max}",
                &d,
                Some(&app.world)
            ),
            "75 / 100"
        );
        assert_eq!(
            resolve("{@Nobody.Health.current}", &d, Some(&app.world)),
            "?"
        );
    }
}
