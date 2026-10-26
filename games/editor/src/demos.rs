//! Module demos as placeable scene objects. A [`ModuleDemo`] entity in the scene stands for one
//! module's `demo::spawn_demo` at its position: the scene file stores only the reference, and
//! the editor spawns (and respawns, when it moves) the demo's entities, which are runtime
//! entities tagged [`DemoPart`] and never saved.

use std::collections::HashSet;

use engine::prelude::*;

/// Spawns one module's showcase demo here. `module` is a key from [`DEMOS`].
#[derive(Component, Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ModuleDemo {
    #[serde(deserialize_with = "known_module")]
    pub module: String,
}

/// The keys of [`DEMOS`], in order.
pub const DEMO_KEYS: &[&str] = &[
    "physics",
    "destruction",
    "softbody",
    "vehicles",
    "fire",
    "smoke",
    "vfx",
    "water",
    "weather",
    "foliage",
    "terrain",
    "anim",
    "npc",
    "movement",
    "audio",
];

/// Only keys from [`DEMO_KEYS`]. The error names them all, which is how the inspector knows to
/// offer a dropdown.
fn known_module<'de, D: serde::Deserializer<'de>>(d: D) -> Result<String, D::Error> {
    let key = String::deserialize(d)?;
    if key.is_empty() || DEMO_KEYS.contains(&key.as_str()) {
        Ok(key)
    } else {
        Err(serde::de::Error::unknown_variant(&key, DEMO_KEYS))
    }
}

/// A runtime entity spawned for a [`ModuleDemo`]; clicking it selects the demo.
#[derive(Component, Debug, Clone, Copy)]
pub struct DemoPart {
    pub spawner: Entity,
}

/// What a [`ModuleDemo`] spawned, and where, so moving it respawns the demo.
#[derive(Component, Debug, Clone, Default)]
pub struct DemoInstance {
    pub module: String,
    pub at: DVec3,
    pub entities: Vec<Entity>,
}

type SpawnFn = fn(&mut Commands, DVec3);

/// Every module demo the editor can place: key, menu label, what it shows.
pub const DEMOS: &[(&str, &str, &str, SpawnFn)] = &[
    (
        "physics",
        "Physics proving ground",
        "Rigid bodies, ragdolls, ropes, stacks",
        engine_physics::demo::spawn_demo,
    ),
    (
        "destruction",
        "Destruction",
        "Fracturing walls, siege fortress, breach house",
        engine_destruction::demo::spawn_demo,
    ),
    (
        "softbody",
        "Cloth and hair",
        "XPBD cloth, sails, flags, hair",
        engine_softbody::demo::spawn_demo,
    ),
    (
        "vehicles",
        "Vehicles",
        "Cars, planes and a spaceport",
        engine_vehicles::demo::spawn_demo,
    ),
    (
        "fire",
        "Fire",
        "Spreading fire on fuel",
        engine_fire::demo::spawn_demo,
    ),
    (
        "smoke",
        "Smoke",
        "Smoke plumes and fog banks",
        engine_smoke::demo::spawn_demo,
    ),
    (
        "vfx",
        "Particles and magic",
        "Spell court: fireballs, frost, lightning",
        engine_vfx::demo::spawn_demo,
    ),
    (
        "water",
        "Water",
        "Pools, waves and a stream",
        engine_water::demo::spawn_demo,
    ),
    (
        "weather",
        "Weather station",
        "Wind vanes, rain gauge, puddles",
        engine_weather::demo::spawn_demo,
    ),
    (
        "foliage",
        "Foliage",
        "A meadow and a grove",
        engine_foliage::demo::spawn_demo,
    ),
    (
        "terrain",
        "Terrain lab",
        "Sculptable terrain patches",
        engine_terrain::demo::spawn_demo,
    ),
    (
        "anim",
        "Animation paddock",
        "Walkers, horses, sparring",
        engine_anim::demo::spawn_demo,
    ),
    (
        "npc",
        "NPC arena",
        "NPCs and wildlife with behaviour",
        engine_npc::demo::spawn_demo,
    ),
    (
        "movement",
        "Movement course",
        "Parkour, ledges, zip lines",
        engine_movement::demo::spawn_demo,
    ),
    (
        "audio",
        "Audio",
        "Footstep surfaces and sound sources",
        engine_audio::demo::spawn_demo,
    ),
];

/// The demo with this key.
pub fn find(key: &str) -> Option<&'static (&'static str, &'static str, &'static str, SpawnFn)> {
    DEMOS.iter().find(|d| d.0 == key)
}

/// Spawn, respawn or remove demos to match their [`ModuleDemo`] entities (Update, exclusive:
/// the demo's entities are told apart by diffing the world around the spawn).
pub fn sync_demos(world: &mut World) {
    let mut q = world.query::<(Entity, &ModuleDemo, &GlobalTransform, Option<&DemoInstance>)>();
    let mut todo = Vec::new();
    for (entity, demo, global, instance) in q.iter(world) {
        let at = global.translation();
        let stale = instance.is_none_or(|i| i.module != demo.module || (i.at - at).length() > 1e-3);
        if stale {
            todo.push((entity, demo.module.clone(), at));
        }
    }
    for (spawner, module, at) in todo {
        if let Some(old) = world.get::<DemoInstance>(spawner).cloned() {
            for e in old.entities {
                if let Ok(e) = world.get_entity_mut(e) {
                    e.despawn();
                }
            }
        }
        let entities = match find(&module) {
            Some(&(_, _, _, spawn)) => spawn_and_collect(world, spawn, at),
            None => {
                if !module.is_empty() {
                    engine::core::log::warn!("no module demo called '{module}'");
                }
                Vec::new()
            }
        };
        for &e in &entities {
            world.entity_mut(e).insert(DemoPart { spawner });
        }
        world.entity_mut(spawner).insert(DemoInstance {
            module,
            at,
            entities,
        });
    }
    // Spawners that were deleted take their demos with them.
    let mut parts = world.query::<(Entity, &DemoPart)>();
    let orphans: Vec<Entity> = parts
        .iter(world)
        .filter(|(_, p)| world.get::<ModuleDemo>(p.spawner).is_none())
        .map(|(e, _)| e)
        .collect();
    for e in orphans {
        if let Ok(e) = world.get_entity_mut(e) {
            e.despawn();
        }
    }
}

/// Run a demo's spawn function and return the root entities it created.
fn spawn_and_collect(world: &mut World, spawn: SpawnFn, at: DVec3) -> Vec<Entity> {
    let before: HashSet<Entity> = world.iter_entities().map(|e| e.id()).collect();
    {
        let mut commands = world.commands();
        spawn(&mut commands, at);
    }
    world.flush();
    let mut roots: Vec<Entity> = world
        .iter_entities()
        // Resources and message queues are entities too; only take things in the world.
        .filter(|e| {
            !before.contains(&e.id())
                && e.get::<ChildOf>().is_none()
                && e.get::<GlobalTransform>().is_some()
        })
        .map(|e| e.id())
        .collect();
    roots.sort();
    roots
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keys_match_the_demo_list() {
        let keys: Vec<&str> = DEMOS.iter().map(|d| d.0).collect();
        assert_eq!(keys, DEMO_KEYS);
    }

    #[test]
    fn unknown_modules_name_the_known_ones() {
        let err = serde_json::from_str::<ModuleDemo>(r#"{"module":"nope"}"#).unwrap_err();
        let names = crate::inspector::parse_variants(&err.to_string()).unwrap();
        assert_eq!(names, DEMO_KEYS);
        assert!(serde_json::from_str::<ModuleDemo>(r#"{"module":"water"}"#).is_ok());
    }
}
