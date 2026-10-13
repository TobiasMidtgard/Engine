#![doc = include_str!("../README.md")]

pub mod builtin;
pub mod demo;
pub mod effect;
pub mod emitter;
pub mod events;
pub mod feature;
pub mod gpu;
pub mod library;
pub mod marks;
pub mod marks_feature;
pub mod shift;
pub mod trail;

use engine_assets::AssetAppExt;
use engine_core::math::{DVec3, Vec3};
use engine_core::prelude::*;
use engine_core::registry::resolve_entity;
use engine_render::RenderAppExt;
use engine_world::{
    FocusPoints, GroundContact, HeatField, ProjectileTrace, SurfaceLayers, SurfaceMaterials,
    WaterSplash, WeatherState, WindField, WorldImpulse,
};
use serde_json::{Value, json};

pub use effect::{
    Burst, Collide, Curve, EffectDesc, EffectLight, EmitShape, EmitterDesc, Facing, Gradient,
    Sprite,
};
pub use emitter::{ParticleEffect, VfxLight, VfxRuntime, VfxSettings, VfxStats};
pub use events::{VfxReactions, spawn_effect_at};
pub use feature::ParticleFeature;
pub use library::{EffectFile, EffectLibrary, EffectLoader};
pub use marks::{Mark, MarkDesc, MarkKind, Marks};
pub use marks_feature::MarksFeature;
pub use trail::{Beam, RibbonStyle, Ribbons, Trail};

/// Everything a game needs to play effects.
pub mod prelude {
    pub use crate::{
        Beam, EffectDesc, EffectLibrary, EmitterDesc, ParticleEffect, RibbonStyle, Trail,
        VfxPlugin, VfxReactions, VfxSettings, spawn_effect_at,
    };
}

/// Removes the entity after this many seconds of world time (beams and effects spawned
/// through the API with a `duration`).
#[derive(Component, Debug, Clone, Copy, Serialize, Deserialize)]
pub struct VfxLifetime(pub f32);

fn tick_lifetimes(
    mut commands: Commands,
    time: Res<Time>,
    mut q: Query<(Entity, &mut VfxLifetime)>,
) {
    let dt = time.delta_secs();
    for (e, mut life) in &mut q {
        life.0 -= dt;
        if life.0 <= 0.0 {
            commands.entity(e).despawn();
        }
    }
}

/// GPU particles, trails, beams and the effect library. See the crate docs.
pub struct VfxPlugin;

impl Plugin for VfxPlugin {
    fn build(&self, app: &mut App) {
        // The environment the effects read (idempotent with `WorldPlugin`).
        app.init_resource::<WeatherState>()
            .init_resource::<WindField>()
            .init_resource::<SurfaceLayers>()
            .init_resource::<HeatField>()
            .init_resource::<engine_world::Gravity>()
            .init_resource::<engine_world::Atmospheres>()
            .init_resource::<engine_world::Ground>()
            .init_resource::<engine_world::Water>()
            .init_resource::<engine_world::Obscurance>()
            .init_resource::<SurfaceMaterials>()
            .init_resource::<FocusPoints>()
            .add_message::<GroundContact>()
            .add_message::<ProjectileTrace>()
            .add_message::<WorldImpulse>()
            .add_message::<WaterSplash>()
            .add_message::<engine_world::LightningStrike>()
            .add_message::<engine_world::WorldShift>();

        app.insert_resource(EffectLibrary::with_builtins())
            .init_resource::<VfxSettings>()
            .init_resource::<VfxReactions>()
            .init_resource::<emitter::VfxRuntime>()
            .init_resource::<emitter::ParticleFrame>()
            .init_resource::<Ribbons>()
            .init_resource::<shift::PendingShift>()
            .init_resource::<Marks>()
            .init_asset::<EffectFile>()
            .register_asset_loader(EffectLoader)
            .register_component::<ParticleEffect>(
                "Plays a particle effect from the library at this entity and follows it. \
                 Fields: effect (name, see vfx.effects), playing, scale (size), rate \
                 (density), intensity (glow and light), despawn_when_done, seed.",
            )
            .register_component::<Trail>(
                "A glowing ribbon left behind as the entity moves (spell trails, sword arcs). \
                 Width, lifetime, color over its length, glow (cd/m²) and style \
                 (smooth, energy, lightning, smoke).",
            )
            .register_component::<Beam>(
                "A lightning bolt or energy beam from this entity to `target`, re-shaped \
                 `flicker_hz` times a second, with forking branches.",
            )
            .register_component::<VfxLifetime>("Removes the entity after this many seconds.")
            .register_resource::<VfxSettings>(
                "Particle budget (max_particles), density (spawn_scale), draw distance, soft \
                 particles, and an on/off switch.",
            )
            .register_resource::<VfxReactions>(
                "Which world events play effects by themselves: footsteps, projectile hits, \
                 explosions, water splashes and lightning strikes; and a per-frame cap.",
            )
            .add_systems(
                Update,
                (
                    library::sync_effect_files,
                    events::react_to_world,
                    events::react_to_splashes,
                    events::react_to_lightning,
                    marks::age_marks,
                    tick_lifetimes,
                    (
                        demo::run_casters,
                        demo::run_projectiles,
                        demo::run_repeaters,
                        demo::run_orbits,
                        demo::run_lightning,
                        demo::run_walkers,
                        demo::run_droppers,
                    ),
                ),
            )
            .add_systems(
                PostUpdate,
                (
                    emitter::tick_effects,
                    trail::update_trails,
                    trail::update_beams,
                )
                    .in_set(UpdateSet::AfterPropagate),
            )
            .add_systems(
                PostUpdate,
                shift::follow_world_shift
                    .in_set(engine_world::WorldShiftSet::Apply)
                    .before(UpdateSet::Propagate),
            )
            .add_render_feature(ParticleFeature::default())
            .add_render_feature(MarksFeature::default());
        register_apis(app);
    }
}

fn vec3_arg(args: &Value, key: &str) -> Result<Option<DVec3>, String> {
    match args.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(v) => {
            let a: [f64; 3] = serde_json::from_value(v.clone())
                .map_err(|_| format!("\"{key}\" must be [x, y, z]"))?;
            Ok(Some(DVec3::from_array(a)))
        }
    }
}

fn f32_arg(args: &Value, key: &str, default: f32) -> f32 {
    args.get(key)
        .and_then(Value::as_f64)
        .map_or(default, |v| v as f32)
}

fn register_apis(app: &mut App) {
    app.register_api(
        "vfx.effects",
        "List every particle effect by name with a one-line description.",
        "{}",
        |world, _| {
            let lib = world.resource::<EffectLibrary>();
            Ok(json!(
                lib.names()
                    .into_iter()
                    .map(|n| json!({
                        "name": n,
                        "description": lib.get(n).map(|d| d.description.as_str()).unwrap_or(""),
                    }))
                    .collect::<Vec<_>>()
            ))
        },
    )
    .register_api(
        "vfx.describe",
        "The full definition of an effect as JSON (start here to make a new one with vfx.define).",
        r#"{"effect": "spell.fireball"}"#,
        |world, args| {
            let name = args.get("effect").and_then(Value::as_str).unwrap_or_default();
            let lib = world.resource::<EffectLibrary>();
            let desc = lib
                .get(name)
                .ok_or_else(|| format!("No effect named \"{name}\". vfx.effects lists them."))?;
            serde_json::to_value(&**desc).map_err(|e| e.to_string())
        },
    )
    .register_api(
        "vfx.define",
        "Add or replace effects (one definition or a list, same format as vfx.describe). \
         Instances playing a replaced effect restart with the new definition.",
        r#"{"effect": {"name": "spell.green_mist", "emitters": [{"rate": 40, "sprite": "smoke", "lit": 1}]}}"#,
        |world, args| {
            let value = args.get("effect").or_else(|| args.get("effects")).cloned().unwrap_or(args);
            let effects = library::parse_effects(&value.to_string())?;
            let mut lib = world.resource_mut::<EffectLibrary>();
            let mut names = Vec::new();
            for e in effects {
                names.push(e.name.clone());
                lib.insert(e)?;
            }
            Ok(json!({ "defined": names }))
        },
    )
    .register_api(
        "vfx.spawn",
        "Play an effect at a position (or on an existing entity with \"entity\"). \"once\" \
         (default true) removes it when done; \"up\" turns its up axis; \"duration\" removes \
         it after that many seconds.",
        r#"{"effect": "spell.fireball_impact", "position": [0, 1, 0], "up": [0, 1, 0], "scale": 1.0, "once": true}"#,
        |world, args| {
            let name = args
                .get("effect")
                .and_then(Value::as_str)
                .ok_or("\"effect\" is required (see vfx.effects)")?
                .to_string();
            if !world.resource::<EffectLibrary>().contains(&name) {
                return Err(format!("No effect named \"{name}\". vfx.effects lists them."));
            }
            let once = args.get("once").and_then(Value::as_bool).unwrap_or(true);
            let mut effect = if once {
                ParticleEffect::once(name)
            } else {
                ParticleEffect::new(name)
            };
            effect.scale = f32_arg(&args, "scale", 1.0);
            effect.rate = f32_arg(&args, "rate", 1.0);
            effect.intensity = f32_arg(&args, "intensity", 1.0);
            let entity = if let Some(sel) = args.get("entity") {
                let e = resolve_entity(world, sel)?;
                effect.despawn_when_done = false;
                world.entity_mut(e).insert(effect);
                e
            } else {
                let pos = vec3_arg(&args, "position")?.unwrap_or(DVec3::ZERO);
                let up = vec3_arg(&args, "up")?.unwrap_or(DVec3::Y).as_vec3();
                let rot = Quat::from_rotation_arc(Vec3::Y, up.normalize_or(Vec3::Y));
                world
                    .spawn((Transform::from_translation(pos).with_rotation(rot), effect))
                    .id()
            };
            if let Some(d) = args.get("duration").and_then(Value::as_f64) {
                world.entity_mut(entity).insert(VfxLifetime(d as f32));
            }
            Ok(json!({ "entity": { "id": entity.to_bits() } }))
        },
    )
    .register_api(
        "vfx.stop",
        "Stop an effect: it stops emitting and its particles fade out. \"remove\": true also \
         removes the ParticleEffect component.",
        r#"{"entity": {"id": 4294967296}, "remove": false}"#,
        |world, args| {
            let e = resolve_entity(world, args.get("entity").unwrap_or(&Value::Null))?;
            if args.get("remove").and_then(Value::as_bool).unwrap_or(false) {
                world.entity_mut(e).remove::<ParticleEffect>();
            } else if let Some(mut fx) = world.get_mut::<ParticleEffect>(e) {
                fx.playing = false;
            } else {
                return Err(format!("entity {e} has no ParticleEffect"));
            }
            Ok(json!({ "stopped": e.to_bits() }))
        },
    )
    .register_api(
        "vfx.clear",
        "Stop every effect. One-shot effects finish fading and remove themselves.",
        "{}",
        |world, _| {
            let mut q = world.query::<&mut ParticleEffect>();
            let mut n = 0;
            for mut fx in q.iter_mut(world) {
                fx.playing = false;
                n += 1;
            }
            Ok(json!({ "stopped": n }))
        },
    )
    .register_api(
        "vfx.beam",
        "Draw a lightning bolt or energy beam between two points, optionally for \"duration\" \
         seconds. Style: lightning, energy, smooth or smoke.",
        r#"{"from": [0, 3, 0], "to": [6, 3, 4], "color": [0.6, 0.75, 1.0], "width": 0.25, "style": "lightning", "duration": 2.0}"#,
        |world, args| {
            let from = vec3_arg(&args, "from")?.ok_or("\"from\" is required")?;
            let to = vec3_arg(&args, "to")?.ok_or("\"to\" is required")?;
            let mut beam = Beam {
                target: to,
                width: f32_arg(&args, "width", 0.25),
                emissive: f32_arg(&args, "emissive", 80_000.0),
                ..Default::default()
            };
            if let Some(c) = args.get("color") {
                beam.color = serde_json::from_value(c.clone())
                    .map_err(|_| "\"color\" must be [r, g, b]".to_string())?;
            }
            if let Some(s) = args.get("style") {
                beam.style = serde_json::from_value(s.clone()).map_err(|_| {
                    "\"style\" must be lightning, energy, smooth or smoke".to_string()
                })?;
            }
            let mut e = world.spawn((Transform::from_translation(from), beam));
            if let Some(d) = args.get("duration").and_then(Value::as_f64) {
                e.insert(VfxLifetime(d as f32));
            }
            Ok(json!({ "entity": { "id": e.id().to_bits() } }))
        },
    )
    .register_api(
        "vfx.mark",
        "Leave a mark on the ground or a wall: scorch, frost, wet or chip. \"normal\" faces the \
         surface (default up), \"radius\" (m) or \"size\" [width, length] with \"forward\" \
         along its length, \"life\" (s), \"glow\" (s of embers, scorch). \"clear\": true \
         removes every mark instead.",
        r#"{"kind": "scorch", "position": [0, 0, 0], "normal": [0, 1, 0], "radius": 1.0, "life": 120, "glow": 6}"#,
        |world, args| {
            let mut marks = world.resource_mut::<Marks>();
            if args.get("clear").and_then(Value::as_bool) == Some(true) {
                let n = marks.len();
                marks.clear();
                return Ok(json!({ "cleared": n }));
            }
            let kind: MarkKind = args
                .get("kind")
                .map(|k| serde_json::from_value(k.clone()))
                .transpose()
                .map_err(|_| {
                    "\"kind\" must be scorch, frost, wet or chip".to_string()
                })?
                .unwrap_or(MarkKind::Scorch);
            let position = vec3_arg(&args, "position")?.ok_or("\"position\" is required")?;
            let normal = vec3_arg(&args, "normal")?.map_or(Vec3::Y, |n| n.as_vec3());
            let radius = f32_arg(&args, "radius", 0.5).max(0.01);
            let mut mark = Mark::round(kind, position, normal, radius);
            if let Some(f) = vec3_arg(&args, "forward")? {
                mark.forward = marks::along_surface(f.as_vec3(), mark.normal);
            }
            if let Some(size) = args.get("size") {
                let [w, l]: [f32; 2] = serde_json::from_value(size.clone())
                    .map_err(|_| "\"size\" must be [width, length]".to_string())?;
                mark.half_size = [w.max(0.02) * 0.5, l.max(0.02) * 0.5];
            }
            mark.life_secs = f32_arg(&args, "life", 60.0).max(0.1);
            mark.glow_secs = f32_arg(&args, "glow", 0.0).max(0.0);
            mark.strength = f32_arg(&args, "strength", 1.0).clamp(0.0, 1.0);
            marks.add(mark);
            Ok(json!({ "marks": marks.len() }))
        },
    )
    .register_api(
        "vfx.stats",
        "Particle system counters: playing and fading effects, emitters, particles spawned \
         this frame, pool use, starved emitters, ribbons, and effects asked for but missing.",
        "{}",
        |world, _| {
            let runtime = world.resource::<VfxRuntime>();
            let mut missing: Vec<&str> = runtime.missing_effects().collect();
            missing.sort_unstable();
            let (trails, beams) = world.resource::<Ribbons>().counts();
            Ok(json!({
                "stats": runtime.stats,
                "pool_used": runtime.pool().used(),
                "pool_high_water": runtime.pool().high_water(),
                "max_particles": world.resource::<VfxSettings>().max_particles,
                "trails": trails,
                "beams": beams,
                "missing_effects": missing,
                "library_size": world.resource::<EffectLibrary>().len(),
                "marks": world.resource::<Marks>().len(),
            }))
        },
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use engine_core::registry::call_api;

    fn app() -> App {
        let mut app = App::new();
        app.add_plugins(VfxPlugin);
        app
    }

    #[test]
    fn apis_spawn_describe_and_define() {
        let mut app = app();
        let list = call_api(&mut app.world, "vfx.effects", json!({})).unwrap();
        assert!(list.as_array().unwrap().len() >= 20);
        let desc = call_api(
            &mut app.world,
            "vfx.describe",
            json!({"effect": "water.splash"}),
        )
        .unwrap();
        assert_eq!(desc["name"], "water.splash");

        let mut custom = desc.clone();
        custom["name"] = json!("water.big_splash");
        let r = call_api(&mut app.world, "vfx.define", json!({ "effect": custom })).unwrap();
        assert_eq!(r["defined"][0], "water.big_splash");

        let r = call_api(
            &mut app.world,
            "vfx.spawn",
            json!({"effect": "water.big_splash", "position": [1, 0, 2], "scale": 2.0}),
        )
        .unwrap();
        let id = r["entity"]["id"].as_u64().unwrap();
        let e = Entity::from_bits(id);
        assert_eq!(app.world.get::<ParticleEffect>(e).unwrap().scale, 2.0);
        assert!(call_api(&mut app.world, "vfx.spawn", json!({"effect": "nope"})).is_err());

        app.update();
        let stats = call_api(&mut app.world, "vfx.stats", json!({})).unwrap();
        assert_eq!(stats["stats"]["effects"], 1);
        assert!(stats["pool_used"].as_u64().unwrap() > 0);
    }

    #[test]
    fn marks_api_leaves_and_clears_marks() {
        let mut app = app();
        let r = call_api(
            &mut app.world,
            "vfx.mark",
            json!({"kind": "wet", "position": [1, 0, 2], "forward": [1, 0, 0],
                   "size": [0.6, 1.4]}),
        )
        .unwrap();
        assert_eq!(r["marks"], 1);
        let mark = *app.world.resource::<Marks>().iter().next().unwrap();
        assert_eq!(mark.kind, MarkKind::Wet);
        assert!(mark.forward.abs_diff_eq(Vec3::X, 1e-5));
        assert!((mark.half_size[1] - 0.7).abs() < 1e-5);
        assert!(
            call_api(
                &mut app.world,
                "vfx.mark",
                json!({"kind": "glitter", "position": [0, 0, 0]})
            )
            .is_err()
        );
        assert!(call_api(&mut app.world, "vfx.mark", json!({"kind": "frost"})).is_err());
        let stats = call_api(&mut app.world, "vfx.stats", json!({})).unwrap();
        assert_eq!(stats["marks"], 1);
        let r = call_api(&mut app.world, "vfx.mark", json!({"clear": true})).unwrap();
        assert_eq!(r["cleared"], 1);
        assert!(app.world.resource::<Marks>().is_empty());
    }

    #[test]
    fn beams_with_a_duration_go_away() {
        let mut app = app();
        app.world
            .resource_mut::<Time>()
            .set_source(engine_core::time::TimeSource::Manual { step_secs: 0.1 });
        call_api(
            &mut app.world,
            "vfx.beam",
            json!({"from": [0, 3, 0], "to": [5, 3, 0], "duration": 0.25}),
        )
        .unwrap();
        app.update();
        assert_eq!(app.world.resource::<Ribbons>().counts().1, 1);
        for _ in 0..4 {
            app.update();
        }
        let mut q = app.world.query::<&Beam>();
        assert_eq!(q.iter(&app.world).count(), 0);
    }
}
