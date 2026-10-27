#![doc = include_str!("../README.md")]

pub mod demo;
pub mod embers;
pub mod field;
pub mod flipbook;
pub mod fuel;
pub mod grass;
pub mod ground;
pub mod objects;
pub mod props;
pub mod render;
pub mod settings;
mod shift;
pub mod sim;
pub mod textures;

use engine_core::math::DVec3;
use engine_core::prelude::*;
use engine_render::RenderAppExt;
use engine_world::{WorldPlugin, WorldShiftSet};
use serde_json::{Value, json};

pub use demo::FireDemo;
pub use embers::{Ember, Embers};
pub use field::{Cell, CellState, FireField, SpreadParams};
pub use fuel::{FuelKind, FuelPatch, PatchShape};
pub use grass::{GrassBurn, GrassMeadow};
pub use ground::{GroundKind, GroundPaint, PaintedGround};
pub use objects::{Burning, Charring, ContainedFire, FlameShape};
pub use settings::FireSettings;
pub use sim::{FireCommand, FireCommands, FireLight, FireVulnerable};

/// Fire: ground fires that spread through fuel with the wind, burning objects, embers and spot
/// fires, heat, scorch, smoke, light and damage. See the crate docs.
pub struct FirePlugin;

impl Plugin for FirePlugin {
    fn build(&self, app: &mut App) {
        if !app.has_plugin::<WorldPlugin>() {
            app.add_plugins(WorldPlugin);
        }
        let settings = app
            .world
            .get_resource::<FireSettings>()
            .cloned()
            .unwrap_or_default();
        let mut field = FireField::new(settings.cell_m);
        field.max_tiles = settings.max_tiles as usize;
        let fire = render::FireFeature::default();
        let haze = fire.haze_feature();
        app.insert_resource(settings)
            .insert_resource(field)
            .init_resource::<Embers>()
            .init_resource::<sim::FirePatches>()
            .init_resource::<sim::FireCommands>()
            .init_resource::<sim::FireClock>()
            .init_resource::<sim::FireLights>()
            .register_resource::<FireSettings>(
                "Fire tuning: sim_hz, spread (transfer, reach_m, wind_reach, wind_boost, \
                 upwind_damping, cooling), embers and spotting, damage_per_second, lights, \
                 smoke_yield, ground_fuel (ground materials burn), sim_radius_m.",
            )
            .register_component::<FuelPatch>(
                "Fuel on the ground in an area: shape (Disc{radius} | Rect{half_extents}), kind \
                 (None/Grass/Litter/Brush/Wood), load_kg_m2, moisture (kg/kg; >0.25 grass won't \
                 burn), ignition_c, regrow_secs, priority. Kind None makes a firebreak.",
            )
            .register_component::<FlameShape>(
                "Where a burning Combustible's flames are: cylinder radius/height (m) with its \
                 base at the entity plus offset.",
            )
            .register_component::<Charring>(
                "Simple props char as they burn: material darkens to char_color, glows while \
                 smoldering, and `shrink` of the size burns away.",
            )
            .register_component::<Burning>(
                "Fire's state on a Combustible: intensity, glow, heat release (W), char.",
            )
            .register_component::<ContainedFire>(
                "A campfire, brazier, fireplace or torch: never lights the ground and its sparks \
                 can't start fires. `perpetual` keeps it fed and relights it.",
            )
            .register_component::<FireDemo>(
                "The burning-meadow demo: warmup_secs (how far along the first fire is), \
                 cycle_secs (time between fires), bounds (fires are kept inside), line (where \
                 fires start).",
            )
            .register_component::<GrassMeadow>(
                "A meadow of grass blades over a rectangle (half_extents, m) that burn with the \
                 fire field: density (blades/m²), height [min, max] (m), width (m), seed_heads \
                 (share), sway, seed. Grows only where the fuel is grass.",
            )
            .register_component::<PaintedGround>(
                "Flat painted ground over a rectangle (half_extents, m): base layer (Litter, \
                 Dirt, Gravel, Mud, Ash) and paints [{kind, center [x, z], shape, blend_m, \
                 ragged_m, strength}] blended with soft, ragged, height-aware edges.",
            )
            .register_component::<FireVulnerable>(
                "Takes Fire damage (DamageEvent) in flames; `scale` multiplies \
                 FireSettings.damage_per_second.",
            )
            .add_systems(
                FixedUpdate,
                (
                    sim::index_patches,
                    sim::handle_ignition,
                    sim::step_field,
                    sim::step_objects,
                    sim::emit_outputs,
                    sim::step_embers,
                    sim::apply_damage,
                )
                    .chain()
                    .in_set(SimSet::Environment),
            )
            // Unordered where nothing needs an order, so each runs beside other modules'
            // systems (AGENTS.md "Let systems run side by side").
            .add_systems(FixedUpdate, sim::add_burning.in_set(SimSet::Environment))
            .add_systems(
                Update,
                (
                    sim::sync_lights,
                    sim::apply_charring,
                    props::burn_foliage,
                    // Nothing waits on the whole-world demo runner, and it runs only when a
                    // demo has work: a `&mut World` system waits for every other one to
                    // finish, even when its run condition then says no.
                    (demo::tick_demos, demo::run_demos.run_if(demo::demos_due)).chain(),
                    demo::contain_demos,
                    grass::update_grass,
                ),
            )
            .add_systems(PostUpdate, shift::shift_fire.in_set(WorldShiftSet::Apply))
            .add_render_feature(fire)
            .add_render_feature(haze)
            .add_render_feature(grass::GrassFeature::default())
            .add_render_feature(ground::GroundFeature::default());
        register_apis(app);
    }
}

fn vec3_arg(args: &Value, key: &str) -> Result<DVec3, String> {
    let v = args
        .get(key)
        .and_then(|v| v.as_array())
        .ok_or_else(|| format!("missing '{key}': a world position like [10, 0, -5]"))?;
    let f = |i: usize| v.get(i).and_then(|x| x.as_f64());
    match (f(0), f(1), f(2)) {
        (Some(x), Some(y), Some(z)) => Ok(DVec3::new(x, y, z)),
        _ => Err(format!("'{key}' must be three numbers, like [10, 0, -5]")),
    }
}

fn f32_arg(args: &Value, key: &str, default: f32) -> f32 {
    args.get(key)
        .and_then(|v| v.as_f64())
        .map(|v| v as f32)
        .unwrap_or(default)
}

fn register_apis(app: &mut App) {
    app.register_api(
        "fire.ignite",
        "Start a fire: ignites dry fuel within radius (m) of a point, and heats objects there. \
         Wet fuel won't light.",
        r#"{"at": [130, 0, 150], "radius": 1.5, "temperature_c": 900}"#,
        |world, args| {
            let at = vec3_arg(&args, "at")?;
            let radius = f32_arg(&args, "radius", 1.0).max(0.1);
            let temperature_c = f32_arg(&args, "temperature_c", 900.0);
            world
                .resource_mut::<FireCommands>()
                .0
                .push(FireCommand::Ignite {
                    at,
                    radius,
                    temperature_c,
                });
            Ok(json!({"queued": true, "at": at.to_array(), "radius": radius}))
        },
    )
    .register_api(
        "fire.extinguish",
        "Douse fire within radius (m) with water (kg/m²; 2 is a bucket per square meter).",
        r#"{"at": [130, 0, 150], "radius": 3, "water_kg_m2": 2}"#,
        |world, args| {
            let at = vec3_arg(&args, "at")?;
            let radius = f32_arg(&args, "radius", 2.0).max(0.1);
            let water_kg_m2 = f32_arg(&args, "water_kg_m2", 2.0).max(0.0);
            world
                .resource_mut::<FireCommands>()
                .0
                .push(FireCommand::Extinguish {
                    at,
                    radius,
                    water_kg_m2,
                });
            Ok(json!({"queued": true}))
        },
    )
    .register_api(
        "fire.clear",
        "Remove every fire, coal and scorch mark (the fuel comes back).",
        "{}",
        |world, _| {
            world
                .resource_mut::<FireCommands>()
                .0
                .push(FireCommand::Clear);
            Ok(json!({"queued": true}))
        },
    )
    .register_api(
        "fire.demo",
        "Start the fire demo's next fire now, already `at_secs` along (0 = just lit); each of \
         its views (demo::VIEWS) jumps there and watches its smoke rise, like the plume at 25 for 20 s.",
        r#"{"at_secs": 54}"#,
        |world, args| {
            let at = f32_arg(&args, "at_secs", 0.0);
            if !at.is_finite() || at < 0.0 {
                return Err("'at_secs' must be 0 or more seconds".into());
            }
            match demo::jump_to(world, at) {
                0 => Err("there is no fire demo".into()),
                n => Ok(json!({"demos": n, "at_secs": at})),
            }
        },
    )
    .register_api(
        "fire.status",
        "What is burning: area in flames (m²), heat release (MW), tiles, embers, burning objects.",
        "{}",
        |world, _| {
            let field = world.resource::<FireField>();
            let stats = field.stats;
            let time = field.time;
            let embers = world.resource::<Embers>().list.len();
            let mut q = world.query::<(Entity, &engine_world::Combustible, Option<&Name>)>();
            let objects: Vec<Value> = q
                .iter(world)
                .filter(|(_, c, _)| c.state != engine_world::BurnState::Intact)
                .take(64)
                .map(|(e, c, n)| {
                    json!({
                        "id": e.to_bits(),
                        "name": n.map(|n| n.as_str().to_string()),
                        "state": format!("{:?}", c.state),
                        "fuel_kg": c.fuel_kg,
                        "temperature_c": c.temperature_c,
                    })
                })
                .collect();
            Ok(json!({
                "burning_m2": stats.burning_m2,
                "burning_cells": stats.burning_cells,
                "hrr_mw": stats.hrr_mw,
                "tiles": stats.tiles,
                "active_tiles": stats.active_tiles,
                "embers": embers,
                "sim_seconds": time,
                "objects": objects,
            }))
        },
    )
    .register_api(
        "fire.sample",
        "The fire cell at a point: state, fuel left (kg/m²), moisture, temperature, scorch.",
        r#"{"at": [130, 0, 150]}"#,
        |world, args| {
            let at = vec3_arg(&args, "at")?;
            let field = world.resource::<FireField>();
            Ok(match field.cell_at(at) {
                Some(c) => json!({
                    "state": format!("{:?}", c.state()),
                    "fuel": format!("{:?}", c.kind),
                    "fuel_kg_m2": c.fuel,
                    "moisture": c.effective_moisture(),
                    "temperature_c": c.temp,
                    "flame_m": c.flame_length(),
                    "scorch": c.scorch,
                }),
                None => json!({"state": "untouched"}),
            })
        },
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use engine_world::{BurnState, Combustible, IgniteRequest};

    fn app() -> App {
        let mut app = App::new();
        app.world
            .resource_mut::<Time>()
            .set_source(engine_core::time::TimeSource::Manual {
                step_secs: 1.0 / 30.0,
            });
        app.add_plugins(FirePlugin);
        app
    }

    #[test]
    fn ignite_request_starts_a_spreading_fire_that_writes_heat_smoke_and_scorch() {
        let mut app = app();
        app.world.write_message(IgniteRequest {
            position: DVec3::new(5.0, 0.0, 5.0),
            radius: 1.5,
            temperature_c: 900.0,
            source: None,
        });
        let mut smoke = 0usize;
        for _ in 0..300 {
            app.update();
            smoke += app
                .world
                .resource::<Messages<engine_world::SmokeEmission>>()
                .iter_current_update_messages()
                .count();
        }
        let field = app.world.resource::<FireField>();
        assert!(field.stats.burning_m2 > 5.0, "{:?}", field.stats);
        assert!(smoke > 0, "no smoke emitted");
        let heat = app.world.resource::<engine_world::HeatField>();
        assert!(heat.grid.data.iter().any(|h| *h > 300.0));
        let surface = app.world.resource::<engine_world::SurfaceLayers>();
        assert!(surface.grid.data.iter().any(|c| c.scorch > 0.5));
        assert!(!app.world.resource::<sim::FireLights>().targets.is_empty());
    }

    #[test]
    fn grass_fire_lights_a_tree_that_burns() {
        let mut app = app();
        let tree = app
            .world
            .spawn((
                Transform::from_xyz(8.0, 0.0, 0.0),
                Combustible {
                    fuel_kg: 60.0,
                    moisture: 0.2,
                    ..Default::default()
                },
                FlameShape {
                    radius: 1.5,
                    height: 4.0,
                    offset: [0.0, 0.5, 0.0],
                },
            ))
            .id();
        app.world.resource_mut::<engine_world::WeatherState>().wind =
            engine_core::math::Vec3::new(4.0, 0.0, 0.0);
        app.world
            .resource_mut::<FireCommands>()
            .0
            .push(FireCommand::Ignite {
                at: DVec3::new(0.0, 0.0, 0.0),
                radius: 3.0,
                temperature_c: 900.0,
            });
        let mut burned = false;
        for _ in 0..(90 * 30) {
            app.update();
            if app.world.get::<Combustible>(tree).unwrap().state == BurnState::Burning {
                burned = true;
                break;
            }
        }
        assert!(burned, "{:?}", app.world.get::<Combustible>(tree));
    }

    #[test]
    fn apis_respond() {
        let mut app = app();
        let r = engine_core::registry::call_api(
            &mut app.world,
            "fire.ignite",
            json!({"at": [0, 0, 0], "radius": 2}),
        )
        .unwrap();
        assert_eq!(r["queued"], true);
        app.update_n(30);
        let s = engine_core::registry::call_api(&mut app.world, "fire.status", json!({})).unwrap();
        assert!(s["burning_m2"].as_f64().unwrap() > 1.0, "{s}");
        let c = engine_core::registry::call_api(
            &mut app.world,
            "fire.sample",
            json!({"at": [0, 0, 0]}),
        )
        .unwrap();
        assert_eq!(c["state"], "Burning", "{c}");
        assert!(engine_core::registry::call_api(&mut app.world, "fire.ignite", json!({})).is_err());
    }
}
