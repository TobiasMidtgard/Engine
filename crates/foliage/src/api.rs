//! `foliage.*` API functions for the AI bridge, console and scripts.

use std::collections::BTreeMap;

use engine_core::prelude::*;
use engine_world::{DamageKind, Ground};
use serde_json::{Value, json};

use crate::ecology::{InRegion, RegionState};
use crate::felling::{ChopResult, chop, fell_tree};
use crate::plant::{EcologyRegion, EcologySettings, Genes, Plant, PlantStage, ResourceSource};
use crate::sim::SimSource;
use crate::species::{PlantLibrary, PlantSpecies};

/// Spawn a plant of `species` at `at` (snapped to the ground when the world has one).
/// `size` is 0..1 of mature size; `genes` default to random ones seeded from the position.
pub fn spawn_plant(
    world: &mut World,
    species: &str,
    at: DVec3,
    size: f32,
    genes: Option<Genes>,
) -> Result<Entity, String> {
    let library = world
        .get_resource::<PlantLibrary>()
        .ok_or("The foliage plugin isn't installed (add FoliagePlugin).")?;
    let id = library.id_or_err(species)?;
    let sp = library.get(id).clone();
    let y = world
        .get_resource::<Ground>()
        .and_then(|g| g.height(at + DVec3::Y * 2.0))
        .unwrap_or(at.y);
    let genes = genes.unwrap_or_else(|| {
        let seed =
            (at.x * 92821.0) as i64 as u64 ^ ((at.z * 68917.0) as i64 as u64).rotate_left(21);
        Genes::random(&mut Rng::new(seed))
    });
    let mut plant = Plant::new(id, size.clamp(0.02, 1.0), genes);
    plant.age_years = plant.size * sp.years_to_mature;
    let combustible = engine_world::Combustible {
        fuel_kg: sp.fuel_kg * plant.size * plant.size,
        moisture: 0.3,
        ignition_c: sp.ignition_c,
        temperature_c: 15.0,
        state: engine_world::BurnState::Intact,
    };
    let mut e = world.spawn((
        Transform::from_translation(DVec3::new(at.x, y, at.z)),
        plant,
    ));
    if sp.is_woody() {
        e.insert(combustible);
    }
    Ok(e.id())
}

fn vec3_arg(args: &Value, key: &str) -> Result<Option<DVec3>, String> {
    match args.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(v) => {
            let a: [f64; 3] = serde_json::from_value(v.clone())
                .map_err(|_| format!("\"{key}\" must be [x, y, z] in meters."))?;
            Ok(Some(DVec3::from_array(a)))
        }
    }
}

fn entity_arg(args: &Value) -> Result<Option<Entity>, String> {
    match args.get("entity").and_then(|v| v.as_u64()) {
        Some(bits) => Entity::try_from_bits(bits)
            .map(Some)
            .ok_or_else(|| format!("{bits} is not a valid entity id")),
        None => Ok(None),
    }
}

fn f32_arg(args: &Value, key: &str, default: f32) -> f32 {
    args.get(key)
        .and_then(|v| v.as_f64())
        .map(|v| v as f32)
        .unwrap_or(default)
}

/// The plant named by `entity`, or the nearest one to `at` (trees first when `trees_only`).
fn target_plant(world: &mut World, args: &Value, trees_only: bool) -> Result<Entity, String> {
    if let Some(e) = entity_arg(args)? {
        return crate::felling::plant_of(world, e)
            .ok_or_else(|| format!("Entity {} is not a plant.", e.to_bits()));
    }
    let at = vec3_arg(args, "at")?.ok_or("Give \"entity\" (a plant id) or \"at\": [x, y, z].")?;
    let radius = f32_arg(args, "radius", 5.0) as f64;
    let library = world.resource::<PlantLibrary>().clone();
    let mut q = world.query::<(Entity, &Plant, &Transform)>();
    q.iter(world)
        .filter(|(_, p, _)| {
            !trees_only
                || (library.get(p.species).is_tree()
                    && p.stage != (PlantStage::Stump { years: 0.0 }))
        })
        .filter(|(_, p, _)| !matches!(p.stage, PlantStage::Stump { .. }))
        .map(|(e, _, t)| (e, t.translation.distance(at)))
        .filter(|(_, d)| *d <= radius)
        .min_by(|a, b| a.1.total_cmp(&b.1))
        .map(|(e, _)| e)
        .ok_or_else(|| format!("No plant within {radius} m of {at:?}."))
}

fn plant_json(world: &World, e: Entity) -> Value {
    let Some(plant) = world.get::<Plant>(e) else {
        return Value::Null;
    };
    let library = world.resource::<PlantLibrary>();
    let sp = library.get(plant.species);
    let pos = world
        .get::<Transform>(e)
        .map(|t| t.translation)
        .unwrap_or_default();
    let mut v = json!({
        "id": e.to_bits(),
        "species": sp.name,
        "position": pos.to_array(),
        "stage": serde_json::to_value(plant.stage).unwrap_or_default(),
        "size": plant.size,
        "height_m": plant.height(sp),
        "age_years": plant.age_years,
        "health": plant.health,
        "vigor": plant.vigor,
        "limited_by": serde_json::to_value(plant.limiting).unwrap_or_default(),
        "genes": serde_json::to_value(plant.genes).unwrap_or_default(),
        "chop_progress": plant.chop_damage / crate::felling::fell_threshold(sp, plant.size),
    });
    if let Some(region) = world.get::<InRegion>(e).and_then(|r| r.0)
        && let Some(state) = world.get::<RegionState>(region)
    {
        let [water, nutrients, minerals] = state.sim.soil.sample(pos);
        v["region"] = json!(region.to_bits());
        v["soil"] = json!({"water": water, "nutrients": nutrients, "minerals": minerals});
    }
    v
}

fn chop_json(r: ChopResult) -> Value {
    match r {
        ChopResult::Missed => json!({"result": "missed"}),
        ChopResult::Damaged { progress } => json!({"result": "damaged", "progress": progress}),
        ChopResult::Felled { log } => json!({"result": "felled", "log": log.to_bits()}),
        ChopResult::Cut => json!({"result": "cut"}),
    }
}

/// Register every `foliage.*` function.
pub fn register(app: &mut App) {
    app.register_api(
        "foliage.plant",
        "Plant a plant: species name (see foliage.species), position (snapped to the ground), \
         size 0..1 of mature size (default 0.1, a sapling). Inside an ecology region it competes \
         and spreads like any other. Returns the plant's id.",
        r#"{"species": "oak", "at": [10, 0, 5], "size": 0.1}"#,
        |world, args| {
            let species = args
                .get("species")
                .and_then(|s| s.as_str())
                .ok_or("Give \"species\": a name from foliage.species.")?;
            let at = vec3_arg(&args, "at")?.ok_or("Give \"at\": [x, y, z].")?;
            let genes = match args.get("genes") {
                Some(g) => Some(serde_json::from_value(g.clone()).map_err(|e| format!("Bad genes: {e}"))?),
                None => None,
            };
            let e = spawn_plant(world, species, at, f32_arg(&args, "size", 0.1), genes)?;
            Ok(json!({"id": e.to_bits()}))
        },
    )
    .register_api(
        "foliage.chop",
        "Hit a plant like an axe would: \"damage\" points (a mature oak takes about 140), \
         \"push\" the direction the blow pushes (the tree falls that way). Name the plant with \
         \"entity\" or the nearest tree to \"at\". Result: damaged (with progress 0..1), felled \
         (with the falling log's id), cut (small plants and bushes) or missed.",
        r#"{"at": [10, 1, 5], "damage": 40, "push": [1, 0, 0]}"#,
        |world, args| {
            let e = target_plant(world, &args, args.get("entity").is_none())?;
            let push = vec3_arg(&args, "push")?.unwrap_or_default().as_vec3();
            Ok(chop_json(chop(world, e, f32_arg(&args, "damage", 40.0), DamageKind::Slash, push)))
        },
    )
    .register_api(
        "foliage.fell",
        "Fell a tree at once, toward a direction (default: the way it leans). It swings down, \
         crashes (shaking foliage, crushing saplings) and settles as a log; its stump may resprout.",
        r#"{"at": [10, 1, 5], "toward": [1, 0, 0]}"#,
        |world, args| {
            let e = target_plant(world, &args, true)?;
            let lean = world.get::<Plant>(e).map(|p| p.genes.lean_dir).unwrap_or(0.0);
            let dir = vec3_arg(&args, "toward")?
                .map(|d| Vec3::new(d.x as f32, 0.0, d.z as f32))
                .filter(|d| d.length_squared() > 1e-6)
                .unwrap_or(Vec3::new(lean.cos(), 0.0, lean.sin()))
                .normalize();
            let log = fell_tree(world, e, dir).ok_or("That plant can't be felled (not a standing tree).")?;
            Ok(json!({"log": log.to_bits()}))
        },
    )
    .register_api(
        "foliage.clear",
        "Clear an area: \"fell\" (trees fall outward from the center, small plants are cut), \
         \"burn\" (everything turns to charred snags and ash that feeds regrowth) or \"remove\" \
         (plants vanish). Returns how many plants were affected.",
        r#"{"at": [0, 0, 0], "radius": 12, "mode": "fell"}"#,
        |world, args| {
            let at = vec3_arg(&args, "at")?.ok_or("Give \"at\": [x, y, z].")?;
            let radius = f32_arg(&args, "radius", 10.0) as f64;
            let mode = args.get("mode").and_then(|m| m.as_str()).unwrap_or("fell");
            let library = world.resource::<PlantLibrary>().clone();
            let mut q = world.query::<(Entity, &Plant, &Transform)>();
            let hits: Vec<(Entity, bool, DVec3)> = q
                .iter(world)
                .filter(|(_, _, t)| DVec2::new(t.translation.x - at.x, t.translation.z - at.z).length() <= radius)
                .map(|(e, p, t)| (e, library.get(p.species).is_tree() && p.size >= 0.2, t.translation))
                .collect();
            for &(e, tree, p) in &hits {
                match mode {
                    "fell" => {
                        let mut out = (p - at).as_vec3();
                        out.y = 0.0;
                        if tree {
                            let dir = out.try_normalize().unwrap_or(Vec3::X);
                            fell_tree(world, e, dir);
                        } else {
                            chop(world, e, 1.0e4, DamageKind::Slash, out);
                        }
                    }
                    "burn" => {
                        if let Some(mut plant) = world.get_mut::<Plant>(e) {
                            plant.stage = PlantStage::Burnt { years: 0.0 };
                        }
                        if let Some(mut c) = world.get_mut::<engine_world::Combustible>(e) {
                            c.state = engine_world::BurnState::Burnt;
                        }
                    }
                    "remove" => {
                        world.entity_mut(e).despawn();
                    }
                    other => return Err(format!("Unknown mode \"{other}\": use fell, burn or remove.")),
                }
            }
            if mode == "burn" {
                let mut rq = world.query::<&mut RegionState>();
                for mut state in rq.iter_mut(world) {
                    state.sim.soil.enrich(at, radius as f32, 0.15);
                }
            }
            Ok(json!({"plants": hits.len()}))
        },
    )
    .register_api(
        "foliage.species",
        "List plant species (name, form, layer, size and what they need), or one species in \
         full with {\"name\": \"oak\"} (tune it with foliage.define_species).",
        r#"{"name": "glowcap"}"#,
        |world, args| {
            let library = world.resource::<PlantLibrary>();
            if let Some(name) = args.get("name").and_then(|n| n.as_str()) {
                let id = library.id_or_err(name)?;
                return serde_json::to_value(library.get(id)).map_err(|e| e.to_string());
            }
            let list: Vec<Value> = library
                .species
                .iter()
                .map(|s| {
                    json!({
                        "name": s.name,
                        "form": serde_json::to_value(s.form).unwrap_or_default(),
                        "layer": serde_json::to_value(s.layer).unwrap_or_default(),
                        "height_m": s.max_height_m,
                        "years_to_mature": s.years_to_mature,
                        "needs": {
                            "water": s.water, "light": s.light, "nutrients": s.nutrients,
                            "minerals": s.minerals, "temperature_c": s.temperature_c,
                        },
                        "glows": s.glow.is_some(),
                    })
                })
                .collect();
            Ok(json!({"species": list}))
        },
    )
    .register_api(
        "foliage.define_species",
        "Add a species, or replace the one with the same name: any PlantSpecies fields (see \
         foliage.species {\"name\": ...}); missing fields take defaults. Start from \"like\" to \
         copy an existing species. Existing plants of a replaced species redraw and follow its \
         new needs.",
        r#"{"like": "glowcap", "species": {"name": "ember cap", "glow": {"color": [1.0, 0.4, 0.1], "nits": 30, "lumens": 20, "pulse_secs": 4}}}"#,
        |world, args| {
            let mut base = serde_json::to_value(PlantSpecies::default()).map_err(|e| e.to_string())?;
            if let Some(like) = args.get("like").and_then(|l| l.as_str()) {
                let library = world.resource::<PlantLibrary>();
                let id = library.id_or_err(like)?;
                base = serde_json::to_value(library.get(id)).map_err(|e| e.to_string())?;
            }
            let patch = args.get("species").cloned().ok_or("Give \"species\": {\"name\": ..., ...}.")?;
            merge(&mut base, patch);
            let sp: PlantSpecies =
                serde_json::from_value(base).map_err(|e| format!("That species doesn't parse: {e}"))?;
            let name = sp.name.clone();
            let id = world.resource_mut::<PlantLibrary>().add(sp);
            Ok(json!({"name": name, "id": id.0}))
        },
    )
    .register_api(
        "foliage.stats",
        "Ecology at a glance: plants per species and stage, and per region the living plants \
         per layer (canopy, understory, ground), births and deaths in the last step and its cost.",
        "{}",
        |world, _| {
            let library = world.resource::<PlantLibrary>().clone();
            let mut per: BTreeMap<String, [u32; 4]> = BTreeMap::new();
            let mut q = world.query::<&Plant>();
            for p in q.iter(world) {
                let k = match p.stage {
                    PlantStage::Alive => 0,
                    PlantStage::Stump { .. } => 1,
                    PlantStage::Dead { .. } => 2,
                    PlantStage::Burnt { .. } => 3,
                };
                per.entry(library.get(p.species).name.clone()).or_default()[k] += 1;
            }
            let species: BTreeMap<String, Value> = per
                .into_iter()
                .map(|(k, [a, s, d, b])| (k, json!({"alive": a, "stumps": s, "dead": d, "burnt": b})))
                .collect();
            let mut rq = world.query::<(Entity, &RegionState)>();
            let regions: Vec<Value> = rq
                .iter(world)
                .map(|(e, s)| json!({"id": e.to_bits(), "center": s.center.to_array(), "last_step": s.stats}))
                .collect();
            let mut lq = world.query::<&crate::plant::FelledLog>();
            let logs = lq.iter(world).count();
            let settings = world.resource::<EcologySettings>();
            Ok(json!({
                "species": species,
                "logs": logs,
                "regions": regions,
                "years_per_second": settings.years_per_second,
            }))
        },
    )
    .register_api(
        "foliage.inspect",
        "Everything about one plant: species, stage, size, age, health, vigor (how good its \
         spot is), what limits it most (water, light, nutrients, minerals, temperature, \
         crowding), genes, chop progress and the soil under it. Name it with \"entity\" or the \
         nearest plant to \"at\".",
        r#"{"at": [10, 0, 5]}"#,
        |world, args| {
            let e = target_plant(world, &args, false)?;
            Ok(plant_json(world, e))
        },
    )
    .register_api(
        "foliage.set_speed",
        "How fast plants grow and spread: \"years_per_second\" for the whole world (default \
         1/600, an oak in about five hours of play), or \"time_scale\" for one region (\"region\" \
         id) as a multiplier. \"paused\" stops growth.",
        r#"{"years_per_second": 0.05}"#,
        |world, args| {
            if let Some(bits) = args.get("region").and_then(|r| r.as_u64()) {
                let e = Entity::try_from_bits(bits).ok_or("Not a valid region id.")?;
                let mut region = world
                    .get_mut::<EcologyRegion>(e)
                    .ok_or("That entity is not an ecology region (see foliage.stats).")?;
                region.time_scale = f32_arg(&args, "time_scale", region.time_scale).max(0.0);
                return Ok(json!({"region": bits, "time_scale": region.time_scale}));
            }
            let mut s = world.resource_mut::<EcologySettings>();
            s.years_per_second = f32_arg(&args, "years_per_second", s.years_per_second).max(0.0);
            if let Some(p) = args.get("paused").and_then(|p| p.as_bool()) {
                s.paused = p;
            }
            Ok(serde_json::to_value(s.clone()).unwrap_or_default())
        },
    )
    .register_api(
        "foliage.add_source",
        "Add a resource source to the soil: a spring (\"water\"), compost or ash (\"nutrients\"), \
         a mineral pool (\"minerals\": cave glowcaps need it), or a dry ridge (negative \
         water), fading out at \"radius\" meters. Plants respond at the next ecology steps.",
        r#"{"at": [0, 0, 0], "radius": 6, "water": 0.3, "minerals": 0.9}"#,
        |world, args| {
            let at = vec3_arg(&args, "at")?.ok_or("Give \"at\": [x, y, z].")?;
            let source = ResourceSource {
                radius_m: f32_arg(&args, "radius", 8.0).max(0.5),
                water: f32_arg(&args, "water", 0.0),
                nutrients: f32_arg(&args, "nutrients", 0.0),
                minerals: f32_arg(&args, "minerals", 0.0),
            };
            let e = world.spawn((Transform::from_translation(at), source)).id();
            Ok(json!({"id": e.to_bits()}))
        },
    );
}

/// Apply new resource sources to regions that have already grown their soil.
pub fn apply_new_sources(
    mut commands: Commands,
    sources: Query<(Entity, &ResourceSource, &Transform), Without<crate::ecology::SourceApplied>>,
    mut regions: Query<&mut RegionState>,
) {
    for (e, s, t) in &sources {
        let sim = SimSource {
            pos: t.translation,
            radius: s.radius_m,
            water: s.water,
            nutrients: s.nutrients,
            minerals: s.minerals,
        };
        for mut state in &mut regions {
            state.sim.soil.add_source(&sim);
        }
        commands.entity(e).insert(crate::ecology::SourceApplied);
    }
}

fn merge(base: &mut Value, patch: Value) {
    match (base, patch) {
        (Value::Object(b), Value::Object(p)) => {
            for (k, v) in p {
                merge(b.entry(k).or_insert(Value::Null), v);
            }
        }
        (b, p) => *b = p,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn app() -> App {
        let mut app = App::new();
        app.add_plugins(engine_world::WorldPlugin);
        app.add_plugins(crate::FoliagePlugin);
        app.update();
        app
    }

    fn call(app: &mut App, name: &str, args: Value) -> Result<Value, String> {
        engine_core::registry::call_api(&mut app.world, name, args)
    }

    #[test]
    fn plant_inspect_and_chop_through_the_api() {
        let mut app = app();
        let id = call(
            &mut app,
            "foliage.plant",
            json!({"species": "birch", "at": [2, 0, 3], "size": 0.6}),
        )
        .unwrap()["id"]
            .as_u64()
            .unwrap();
        let info = call(&mut app, "foliage.inspect", json!({"entity": id})).unwrap();
        assert_eq!(info["species"], "birch");
        assert!(info["height_m"].as_f64().unwrap() > 5.0);
        let r = call(
            &mut app,
            "foliage.chop",
            json!({"at": [2, 1, 3], "damage": 5}),
        )
        .unwrap();
        assert_eq!(r["result"], "damaged");
        let r = call(
            &mut app,
            "foliage.fell",
            json!({"entity": id, "toward": [0, 0, 1]}),
        )
        .unwrap();
        assert!(r["log"].as_u64().is_some());
        let err = call(
            &mut app,
            "foliage.plant",
            json!({"species": "palm tree", "at": [0, 0, 0]}),
        )
        .unwrap_err();
        assert!(err.contains("palm tree"), "{err}");
    }

    #[test]
    fn define_species_from_an_existing_one() {
        let mut app = app();
        let r = call(
            &mut app,
            "foliage.define_species",
            json!({"like": "glowcap", "species": {"name": "ember cap", "accent_color": [1.0, 0.3, 0.1]}}),
        )
        .unwrap();
        assert_eq!(r["name"], "ember cap");
        let lib = app.world.resource::<PlantLibrary>();
        let ember = lib.get(lib.id("ember cap").unwrap());
        assert!(ember.glow.is_some(), "copied from glowcap");
        assert_eq!(ember.accent_color, [1.0, 0.3, 0.1]);
        let stats = call(&mut app, "foliage.stats", json!({})).unwrap();
        assert!(stats["regions"].is_array());
    }
}
