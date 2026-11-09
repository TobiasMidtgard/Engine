//! The magic module's JSON API (AGENTS.md: everything a player, designer or AI can do is
//! reachable by name): design and price spells, combine effects, cast, inspect casters.

use engine_core::math::{DVec3, Vec3};
use engine_core::prelude::*;
use engine_core::registry::resolve_entity;
use serde_json::{Value, json};

use crate::caster::{Aim, CastRequest, KnownSpell, MagicCaster, SpellChoice};
use crate::combine::{RECIPES, weave};
use crate::compile::{CastContext, compile};
use crate::design::SpellDesign;
use crate::effects::EFFECTS;
use crate::growth::{Growth, grow};
use crate::schools::{Mastery, Power};

fn arg<T: serde::de::DeserializeOwned>(args: &Value, key: &str) -> Result<Option<T>, String> {
    match args.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(v) => serde_json::from_value(v.clone())
            .map(Some)
            .map_err(|e| format!("\"{key}\": {e}")),
    }
}

fn design_arg(args: &Value) -> Result<SpellDesign, String> {
    // A design object, or just a list of effects.
    if let Some(fx) = args.get("effects").and_then(Value::as_array)
        && args.get("design").is_none()
    {
        let keys: Vec<&str> = fx.iter().filter_map(Value::as_str).collect();
        let cast = arg(args, "cast")?.unwrap_or_default();
        let mut d = SpellDesign::new(&keys, cast);
        if let Some(m) = arg(args, "modifiers")? {
            d.modifiers = m;
        }
        return Ok(d);
    }
    arg::<SpellDesign>(args, "design")?
        .ok_or_else(|| "\"design\" (or \"effects\") is required".to_string())
}

fn spell_json(s: &crate::compile::Spell) -> Value {
    json!({
        "name": s.name,
        "summary": s.summary(),
        "weave": s.weave,
        "cast": s.cast,
        "complexity": s.complexity,
        "budget": s.budget,
        "cost": s.cost,
        "fizzle": s.fizzle,
        "backfire": s.backfire,
        "cast_secs": s.cast_secs,
        "power_j": s.power.energy_j,
        "carrier": s.carrier,
        "payload": s.payload,
        "stages": s.stages.iter().map(|(t, st)| json!({"trigger": t, "spell": st.name, "summary": st.summary()})).collect::<Vec<_>>(),
    })
}

pub fn register(app: &mut App) {
    app.register_api(
        "magic.effects",
        "Every effect a spell can be built from: key, name, subclass, what it does.",
        "{}",
        |_, _| Ok(json!(EFFECTS)),
    )
    .register_api(
        "magic.recipes",
        "The combination table: what two or three effects make together (Steam, Plasma, \
         Rocket leap...), what it does, and the reaction it sets off.",
        "{}",
        |_, _| {
            Ok(json!(
                RECIPES
                    .iter()
                    .map(|r| json!({
                        "key": r.key,
                        "name": r.name,
                        "ingredients": r.ingredients,
                        "description": r.description,
                        "reaction": r.reaction,
                        "color": r.color,
                    }))
                    .collect::<Vec<_>>()
            ))
        },
    )
    .register_api(
        "magic.combine",
        "Weave effects (or recipes) together, in any order: the result's name, look, dials \
         and stability. Unknown pairs are named from their dials (\"Steam of Haste\").",
        r#"{"effects": ["fire", "frost"]}"#,
        |_, args| {
            let keys: Vec<String> = arg(&args, "effects")?.ok_or("\"effects\" is required")?;
            let w = weave(&keys).ok_or("Unknown effect; magic.effects lists them.")?;
            Ok(json!(w))
        },
    )
    .register_api(
        "magic.craft",
        "Price a spell design for a mastery tier (default expert) and power (joules, default \
         200 kJ): complexity against budget, mana, fizzle and backfire odds, and every number \
         it will fly with; or the problems that stop it.",
        r#"{"design": {"name": "Fire volley", "effects": ["fire"], "cast": "bolt", "modifiers": [{"kind": "split", "count": 3}, {"kind": "seek"}]}, "mastery": "expert"}"#,
        |_, args| {
            let design = design_arg(&args)?;
            let mastery: Mastery = arg(&args, "mastery")?.unwrap_or(Mastery::Expert);
            let power = Power {
                energy_j: arg(&args, "power_j")?.unwrap_or(Power::default().energy_j),
            };
            match compile(&design, CastContext::new(mastery, power)) {
                Ok(s) => Ok(json!({ "ok": true, "spell": spell_json(&s) })),
                Err(e) => Ok(json!({
                    "ok": false,
                    "problems": e.0,
                    "complexity": design.complexity(),
                    "budget": mastery.budget(),
                })),
            }
        },
    )
    .register_api(
        "magic.grow",
        "What a known spell becomes at a mastery tier along a growth path (volley, blast, \
         burn, lance).",
        r#"{"design": {"effects": ["fire"], "cast": "bolt"}, "growth": "volley", "mastery": "master"}"#,
        |_, args| {
            let design = design_arg(&args)?;
            let path: Growth = arg(&args, "growth")?.unwrap_or(Growth::Volley);
            let mastery: Mastery = arg(&args, "mastery")?.unwrap_or(Mastery::Expert);
            let grown = grow(&design, path, mastery);
            Ok(json!({ "design": grown, "complexity": grown.complexity(), "budget": mastery.budget() }))
        },
    )
    .register_api(
        "magic.caster",
        "A caster's mana, skills, corruption and grimoire. \"set\" changes fields (same names), \
         \"mastery\" puts every subclass at a tier.",
        r#"{"entity": "Mage", "set": {"mana": 200, "level": 40}, "mastery": "master"}"#,
        |world, args| {
            let e = resolve_entity(world, args.get("entity").unwrap_or(&Value::Null))?;
            let mut c = world
                .get::<MagicCaster>(e)
                .cloned()
                .ok_or("That entity can't cast (no MagicCaster).")?;
            if let Some(set) = args.get("set").and_then(Value::as_object) {
                let mut v = serde_json::to_value(&c).map_err(|e| e.to_string())?;
                for (k, val) in set {
                    v[k] = val.clone();
                }
                c = serde_json::from_value(v).map_err(|e| e.to_string())?;
            }
            if let Some(m) = arg::<Mastery>(&args, "mastery")? {
                let fresh = MagicCaster::new(c.level, m);
                c.skills = fresh.skills;
                c.god = fresh.god;
            }
            let out = json!({
                "caster": c,
                "power_j": c.power().energy_j,
                "spells": (0..c.grimoire.len()).filter_map(|i| {
                    let d = c.grown(i)?;
                    Some(match c.compile(&d, 0.0) {
                        Ok(s) => json!({"slot": i, "summary": s.summary()}),
                        Err(e) => json!({"slot": i, "problems": e.0}),
                    })
                }).collect::<Vec<_>>(),
            });
            world.entity_mut(e).insert(c);
            Ok(out)
        },
    )
    .register_api(
        "magic.learn",
        "Add a spell to a caster's grimoire, optionally growing along a path; \"select\" makes \
         it the spell a plain cast uses.",
        r#"{"entity": "Mage", "design": {"name": "Frost lance", "effects": ["frost"], "cast": "bolt"}, "growth": "lance", "select": true}"#,
        |world, args| {
            let e = resolve_entity(world, args.get("entity").unwrap_or(&Value::Null))?;
            let design = design_arg(&args)?;
            let growth: Option<Growth> = arg(&args, "growth")?;
            let select = args.get("select").and_then(Value::as_bool).unwrap_or(false);
            let mut c = world
                .get_mut::<MagicCaster>(e)
                .ok_or("That entity can't cast (no MagicCaster).")?;
            c.grimoire.push(KnownSpell { design, growth });
            let slot = c.grimoire.len() - 1;
            if select {
                c.selected = slot;
            }
            Ok(json!({ "slot": slot }))
        },
    )
    .register_api(
        "magic.cast",
        "Cast: the caster's selected spell, a grimoire \"slot\", or a \"design\" given here. \
         Aim at an entity (\"target\"), a point (\"at\") or along a direction (\"dir\"); \
         \"charge\" 0..1 for chargeable spells; \"hand\": \"off\" casts from the off hand (a weapon in \
         the main one; a heavy attack's charged cast is \"hand\": \"off\" with its \"charge\" and \
         \"gathered\": true, at power × (1 + charge)); \"gathered\": true releases at once \
         after the plain cast time.",
        r#"{"caster": "Mage", "design": {"effects": ["fire", "frost"], "cast": "lob"}, "at": [0, 0, -20]}"#,
        |world, args| {
            let caster = resolve_entity(world, args.get("caster").unwrap_or(&Value::Null))?;
            if world.get::<MagicCaster>(caster).is_none() {
                return Err("That entity can't cast (no MagicCaster).".into());
            }
            let spell = if args.get("design").is_some() || args.get("effects").is_some() {
                SpellChoice::Design(design_arg(&args)?)
            } else if let Some(slot) = arg::<usize>(&args, "slot")? {
                SpellChoice::Slot(slot)
            } else {
                SpellChoice::Selected
            };
            let aim = if let Some(t) = args.get("target") {
                Aim::Entity(resolve_entity(world, t)?)
            } else if let Some(p) = arg::<[f64; 3]>(&args, "at")? {
                Aim::Point(DVec3::from_array(p))
            } else if let Some(d) = arg::<[f32; 3]>(&args, "dir")? {
                Aim::Direction(Vec3::from_array(d).normalize_or(Vec3::NEG_Z))
            } else {
                let rot = world
                    .get::<GlobalTransform>(caster)
                    .map(|g| g.rotation())
                    .unwrap_or_default();
                Aim::Direction(rot * Vec3::NEG_Z)
            };
            let charge = arg::<f32>(&args, "charge")?.unwrap_or(0.0);
            let off_hand = match arg::<String>(&args, "hand")?.as_deref() {
                None | Some("main") => false,
                Some("off") => true,
                Some(h) => return Err(format!("hand is \"main\" or \"off\", not {h:?}")),
            };
            world.write_message(CastRequest {
                caster,
                spell,
                aim,
                charge,
                gathered: arg::<bool>(&args, "gathered")?.unwrap_or(false),
                off_hand,
            });
            Ok(json!({ "queued": true }))
        },
    );
}
