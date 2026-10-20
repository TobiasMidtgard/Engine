//! `character.*` API functions: what the creator, the console and AI agents call.

use engine_core::prelude::*;
use serde_json::{Value, json};

use crate::appearance::Appearance;
use crate::sliders::SLIDERS;
use crate::{BuiltCharacter, CharacterDebug, HumanAssets, PackStatus};

/// The character an API call means: `entity` (its bits) if given, else the first one.
fn target(world: &mut World, args: &Value) -> Result<Entity, String> {
    let mut q = world.query_filtered::<Entity, With<Appearance>>();
    if let Some(bits) = args.get("entity").and_then(Value::as_u64) {
        return q
            .iter(world)
            .find(|e| e.to_bits() == bits)
            .ok_or_else(|| format!("no character with entity {bits}; character.list shows them."));
    }
    q.iter(world)
        .next()
        .ok_or_else(|| "there is no character yet; spawn an entity with an Appearance.".into())
}

/// Merge `patch` into `base` (objects field by field, anything else replaced).
fn merge(base: &mut Value, patch: &Value) {
    match (base, patch) {
        (Value::Object(b), Value::Object(p)) => {
            for (k, v) in p {
                merge(b.entry(k.clone()).or_insert(Value::Null), v);
            }
        }
        (b, p) => *b = p.clone(),
    }
}

fn set_appearance(world: &mut World, e: Entity, a: Appearance) -> Value {
    let out = serde_json::to_value(&a).unwrap_or(Value::Null);
    world.entity_mut(e).insert(a);
    out
}

pub(crate) fn register(app: &mut App) {
    app.register_api(
        "character.list",
        "Every character: entity bits, and whether its meshes are built.",
        "{}",
        |world, _| {
            let mut q = world.query::<(Entity, &Appearance, Option<&BuiltCharacter>)>();
            let list: Vec<Value> = q
                .iter(world)
                .map(|(e, a, b)| {
                    json!({
                        "entity": e.to_bits(),
                        "sex": a.sex,
                        "age": a.age,
                        "built": b.is_some_and(|b| b.appearance == *a),
                    })
                })
                .collect();
            let status = match world.get_resource::<HumanAssets>().map(|h| &h.status) {
                Some(PackStatus::Ready) => "ready".to_string(),
                Some(PackStatus::Loading) | None => "loading".to_string(),
                Some(PackStatus::Missing(e)) => e.clone(),
            };
            Ok(json!({ "pack": status, "characters": list }))
        },
    )
    .register_api(
        "character.get",
        "A character's appearance (entity optional: the first character).",
        r#"{"entity": 4294967297}"#,
        |world, args| {
            let e = target(world, &args)?;
            let a = world.get::<Appearance>(e).cloned().unwrap_or_default();
            Ok(serde_json::to_value(a).unwrap_or(Value::Null))
        },
    )
    .register_api(
        "character.set",
        "Change part of a character's appearance; fields not given stay as they are.",
        r#"{"appearance": {"age": 40, "skin": {"melanin": 0.6}, "shape": {"nose-hump-decr|incr": 0.5}}}"#,
        |world, args| {
            let e = target(world, &args)?;
            let current = world.get::<Appearance>(e).cloned().unwrap_or_default();
            let mut value = serde_json::to_value(&current).map_err(|e| e.to_string())?;
            let patch = args
                .get("appearance")
                .ok_or("give the fields to change under \"appearance\".")?;
            merge(&mut value, patch);
            let mut a: Appearance = serde_json::from_value(value).map_err(|e| {
                format!("that appearance doesn't fit ({e}); character.get shows the fields.")
            })?;
            // Clamp sliders into range (unknown keys are kept out).
            let shape = std::mem::take(&mut a.shape);
            for (k, v) in shape {
                if !a.set_slider(&k, v) {
                    return Err(format!(
                        "there is no slider \"{k}\"; character.sliders lists them."
                    ));
                }
            }
            Ok(set_appearance(world, e, a))
        },
    )
    .register_api(
        "character.preset",
        "Load a named preset onto a character: Astrid, Kofi, Mei, Tomas, Ruth, Arjun.",
        r#"{"name": "Kofi"}"#,
        |world, args| {
            let e = target(world, &args)?;
            let name = args.get("name").and_then(Value::as_str).unwrap_or("");
            let a = Appearance::preset(name).ok_or_else(|| {
                let names: Vec<&str> = Appearance::presets().iter().map(|(n, _)| *n).collect();
                format!("no preset \"{name}\"; try one of {}.", names.join(", "))
            })?;
            Ok(set_appearance(world, e, a))
        },
    )
    .register_api(
        "character.random",
        "Give a character a random but plausible appearance (the same for the same seed).",
        r#"{"seed": 42}"#,
        |world, args| {
            let e = target(world, &args)?;
            let seed = args.get("seed").and_then(Value::as_u64).unwrap_or(0);
            Ok(set_appearance(world, e, Appearance::random(seed)))
        },
    )
    .register_api(
        "character.sliders",
        "Every face, neck and torso slider by group: key and range.",
        "{}",
        |_, _| {
            let mut groups: Vec<(String, Vec<Value>)> = Vec::new();
            for s in SLIDERS {
                let entry = json!({
                    "key": s.key(),
                    "min": if s.low.is_empty() { 0.0 } else { -1.0 },
                    "max": 1.0,
                });
                match groups.iter_mut().find(|(g, _)| g == s.group) {
                    Some((_, list)) => list.push(entry),
                    None => groups.push((s.group.to_string(), vec![entry])),
                }
            }
            Ok(Value::Object(
                groups
                    .into_iter()
                    .map(|(g, l)| (g, Value::Array(l)))
                    .collect(),
            ))
        },
    )
    .register_api(
        "character.debug",
        "Shader debug view: 0 off, 1 region masks, 2 normals, 3 curvature.",
        r#"{"view": 1}"#,
        |world, args| {
            let view = args.get("view").and_then(Value::as_u64).unwrap_or(0).min(3) as u8;
            world.insert_resource(CharacterDebug { view });
            Ok(json!({ "view": view }))
        },
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    fn app() -> App {
        let mut app = App::new();
        register(&mut app);
        app.world.spawn(Appearance::default());
        app
    }

    fn call(app: &mut App, name: &str, args: Value) -> Result<Value, String> {
        engine_core::registry::call_api(&mut app.world, name, args)
    }

    #[test]
    fn set_patches_only_what_is_given() {
        let mut app = app();
        let out = call(
            &mut app,
            "character.set",
            json!({"appearance": {"age": 61, "skin": {"melanin": 0.7}, "shape": {"nose-hump-decr|incr": 2.0}}}),
        )
        .unwrap();
        assert_eq!(out["age"], 61.0);
        let a = app
            .world
            .query::<&Appearance>()
            .single(&app.world)
            .unwrap()
            .clone();
        assert_eq!(a.skin.melanin, 0.7);
        assert_eq!(a.skin.redness, Appearance::default().skin.redness);
        assert_eq!(a.slider("nose-hump-decr|incr"), 1.0);
        let err = call(
            &mut app,
            "character.set",
            json!({"appearance": {"shape": {"nose-wings": 1}}}),
        )
        .unwrap_err();
        assert!(err.contains("character.sliders"), "{err}");
    }

    #[test]
    fn presets_and_random_people_load() {
        let mut app = app();
        let out = call(&mut app, "character.preset", json!({"name": "ruth"})).unwrap();
        assert_eq!(out["age"], 72.0);
        assert!(call(&mut app, "character.preset", json!({"name": "x"})).is_err());
        let a = call(&mut app, "character.random", json!({"seed": 9})).unwrap();
        let b = call(&mut app, "character.random", json!({"seed": 9})).unwrap();
        assert_eq!(a, b);
        let sliders = call(&mut app, "character.sliders", json!({})).unwrap();
        assert!(sliders["nose"].as_array().unwrap().len() > 10);
    }
}
