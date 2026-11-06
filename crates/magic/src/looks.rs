//! How spells look. Every look command is written twice: into the engine's spell contracts
//! (`engine_world::magic`: a `SpellLook` on each carrier, `SpellCue`s for one-shots, see
//! [`write_contracts`]), which vfx and audio read, and, until vfx says it draws from those
//! (its `vfx.spell_contracts` API answers), as the stand-in particle calls below.
//!
//! Like Unreal's GameplayCues (research-architecture.md), the simulation never draws: it
//! queues [`LookCmd`]s, and this module turns them into particle effects through the
//! particles module's public API (`vfx.spawn`, `vfx.stop`, `vfx.beam`, `vfx.mark`), so the
//! crates stay apart. Every look is asked for by name, `spell.<look>.<role>`
//! (`spell.steam.field`, `spell.fire.bolt`, `spell.status.frozen`), so the Particles thread can
//! make each one properly by defining that name. Until it does, the first time a name is
//! needed this module makes a stand-in: the nearest existing effect (the fireball for fire,
//! the frost bolt for frost, the orb for the rest), recoloured to the weave's color through
//! `vfx.describe` and `vfx.define`, with a light added to bright bolts. A name the library
//! already has is never replaced.

use std::collections::HashMap;

use engine_core::math::{DVec3, Vec3};
use engine_core::prelude::*;
use engine_core::registry::call_api;
use engine_render::{AlphaMode, MaterialRef, MeshRef, MeshRenderer, Shape, StandardMaterial};
use engine_world::magic::{SpellCue, SpellLook, SpellLookDrawer, SpellMark, SpellRole};
use serde_json::{Value, json};

use crate::schools::Status;

/// What part of a spell a look is for.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Role {
    /// A projectile in flight.
    Bolt,
    /// Where it lands: a one-shot burst.
    Impact,
    /// A lasting area on the ground.
    Field,
    /// A gout from the hand.
    Cone,
    /// An area that rides on its caster.
    Aura,
    /// Gathering in the hand before release.
    Charge,
    /// A status riding on a victim.
    Status,
    /// A cast that failed.
    Fizzle,
    /// Standing along a wall, one per piece (about 1.5 m of wall).
    Wall,
}

impl Role {
    pub fn key(self) -> &'static str {
        match self {
            Role::Bolt => "bolt",
            Role::Impact => "impact",
            Role::Field => "field",
            Role::Cone => "cone",
            Role::Aura => "aura",
            Role::Charge => "charge",
            Role::Status => "status",
            Role::Fizzle => "fizzle",
            Role::Wall => "wall",
        }
    }
}

/// A look to show, queued by the simulation.
#[derive(Debug, Clone)]
pub enum LookCmd {
    /// Play a look on a carrier entity, following it.
    Attach {
        entity: Entity,
        look: String,
        role: Role,
        scale: f32,
        color: [f32; 3],
        /// The spell's energy, J (0 for statuses).
        power_j: f32,
    },
    /// A one-shot look at a point.
    Burst {
        at: DVec3,
        up: Vec3,
        look: String,
        role: Role,
        scale: f32,
        color: [f32; 3],
    },
    /// Stop an entity's look (its particles fade).
    Stop { entity: Entity },
    /// A beam or arc between two points.
    Beam {
        from: DVec3,
        to: DVec3,
        look: String,
        color: [f32; 3],
        width: f32,
        secs: f32,
    },
    /// Gathering in a caster's hand for `secs`.
    Charge {
        caster: Entity,
        look: String,
        secs: f32,
        scale: f32,
    },
    /// The charge is over (released, fizzled or backfired).
    EndCharge { caster: Entity },
    /// The cast is over: its `Casting` comes off (handled by the flush, never drawn).
    RemoveCasting { caster: Entity },
    /// A scorch, frost or wet mark on a surface.
    Mark {
        at: DVec3,
        normal: Vec3,
        kind: &'static str,
        radius: f32,
    },
}

/// The particle names resolved so far, and the charges playing.
#[derive(Resource, Debug, Default)]
pub struct LookLibrary {
    resolved: HashMap<(String, Role), Option<String>>,
    charges: HashMap<Entity, Entity>,
    /// The particles module isn't there (headless tests): looks are skipped.
    pub no_particles: bool,
    /// vfx draws spells from the contracts itself (`vfx.spell_contracts`), so the stand-ins
    /// stay off: asked once, on the first look.
    pub vfx_reads_contracts: Option<bool>,
    /// Stand-ins this module defined, for the Particles thread to replace.
    pub stand_ins: Vec<String>,
}

/// Which existing effects a look borrows from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Family {
    Fire,
    Frost,
    Storm,
    Life,
    Mist,
    Arcane,
}

fn family(look: &str) -> Family {
    match look {
        "fire" | "plasma" | "hellfire" | "boiling_blood" | "rocket_leap" | "radiance"
        | "black_flame" | "energy" | "burning" => Family::Fire,
        "frost" | "frozen_veins" | "frostrot" | "black_ice" | "shardstorm" | "deep_freeze"
        | "chilled" | "frozen" => Family::Frost,
        "storm" | "tempest" | "aether" | "blood_lightning" | "acid_storm" | "dark_thunder"
        | "thunderclap" | "shocked" => Family::Storm,
        "life" | "heal" | "transfer" => Family::Life,
        "steam" | "plague" | "wither" | "doom" | "nightfeed" | "shadow" | "blight" | "blood"
        | "night_terror" | "ghost_step" | "mind" | "dread" | "rot" | "bleeding" => Family::Mist,
        _ => Family::Arcane,
    }
}

/// The families a wall's body is made by ([`crate::walls`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WallFamily {
    Fire,
    Frost,
    Storm,
    Other,
}

pub fn family_of(look: &str) -> WallFamily {
    match family(look) {
        Family::Fire => WallFamily::Fire,
        Family::Frost => WallFamily::Frost,
        Family::Storm => WallFamily::Storm,
        _ => WallFamily::Other,
    }
}

/// Dark looks get no light of their own.
fn bright(look: &str) -> bool {
    !matches!(
        look,
        "shadow"
            | "blood"
            | "blight"
            | "plague"
            | "nightfeed"
            | "wither"
            | "doom"
            | "black_flame"
            | "black_ice"
            | "dark_thunder"
            | "night_terror"
            | "ghost_step"
            | "dread"
            | "rot"
            | "bleeding"
            | "steam"
    )
}

/// The existing effects a stand-in is made from (their emitters merged), first one leading.
fn bases(look: &str, role: Role) -> &'static [&'static str] {
    use Family::*;
    let fam = family(look);
    match role {
        Role::Bolt => match fam {
            Fire => &["spell.fireball"],
            Frost => &["spell.frost_bolt"],
            _ => &["magic.orb"],
        },
        Role::Impact | Role::Cone => match fam {
            Fire => &["spell.fireball_impact"],
            Frost => &["spell.frost_impact"],
            Storm => &["spell.lightning_hit"],
            _ => &["spell.starburst"],
        },
        Role::Field | Role::Aura => match fam {
            Fire => &["spell.heal_aura", "fire.torch"],
            Frost => &["spell.heal_aura", "water.mist"],
            Storm => &["spell.heal_aura", "magic.orb"],
            Mist => &["water.mist", "spell.heal_aura"],
            _ => &["spell.heal_aura"],
        },
        Role::Wall => match fam {
            Fire => &["fire.torch"],
            Frost | Mist => &["water.mist"],
            Storm => &["magic.orb"],
            _ => &["spell.heal_aura"],
        },
        Role::Charge => &["spell.charge"],
        Role::Fizzle => &["spell.starburst"],
        Role::Status => match look {
            "burning" => &["fire.torch"],
            "frozen" | "dread" => &["water.mist"],
            "shocked" => &["magic.orb"],
            _ => &["ambient.fireflies"],
        },
    }
}

/// The name a look is asked for by.
pub fn look_name(look: &str, role: Role) -> String {
    if role == Role::Status {
        format!("spell.status.{look}")
    } else {
        format!("spell.{look}.{}", role.key())
    }
}

/// The color a status is drawn in.
pub fn status_color(s: Status) -> [f32; 3] {
    match s {
        Status::Burning => [1.0, 0.45, 0.12],
        Status::Chilled => [0.6, 0.85, 1.0],
        Status::Frozen => [0.75, 0.92, 1.0],
        Status::Shocked => [0.65, 0.75, 1.0],
        Status::Bleeding => [0.6, 0.02, 0.03],
        Status::Rot => [0.45, 0.6, 0.08],
        Status::Dread => [0.25, 0.08, 0.4],
    }
}

/// The ice a frozen body is shut in: a translucent capsule.
pub fn ice_shell() -> MeshRenderer {
    MeshRenderer::new(
        MeshRef::Shape(Shape::Capsule {
            radius: 0.5,
            height: 1.95,
            segments: 16,
        }),
        MaterialRef::Inline(StandardMaterial {
            base_color: [0.72, 0.88, 1.0, 0.5],
            emissive: [0.35, 0.6, 1.0],
            emissive_strength: 0.6,
            perceptual_roughness: 0.12,
            reflectance: 0.6,
            alpha_mode: AlphaMode::Blend,
            ..Default::default()
        }),
    )
    .without_shadows()
}

// ------------------------------------------------------------------------------ recolor

fn recolor_rgb(c: [f32; 3], target: [f32; 3]) -> [f32; 3] {
    let max = c[0].max(c[1]).max(c[2]);
    if max <= 1e-6 {
        return c;
    }
    let min = c[0].min(c[1]).min(c[2]);
    let sat = (max - min) / max;
    let tmax = target[0].max(target[1]).max(target[2]).max(1e-3);
    // The target's hue at the old brightness, keeping a little of a white-hot core.
    let keep = (1.0 - sat).powi(2) * 0.5;
    std::array::from_fn(|i| target[i] / tmax * max * (1.0 - keep) + max * keep)
}

fn as_rgb(v: &Value) -> Option<[f32; 3]> {
    let a = v.as_array()?;
    if a.len() < 3 || a.len() > 4 || !a.iter().all(Value::is_number) {
        return None;
    }
    Some(std::array::from_fn(|i| a[i].as_f64().unwrap_or(0.0) as f32))
}

fn set_rgb(v: &mut Value, rgb: [f32; 3]) {
    if let Some(a) = v.as_array_mut() {
        for (i, c) in rgb.iter().enumerate() {
            a[i] = json!(c);
        }
    }
}

fn recolor_value(v: &mut Value, target: [f32; 3]) {
    if let Some(rgb) = as_rgb(v) {
        set_rgb(v, recolor_rgb(rgb, target));
        return;
    }
    // A gradient: [[t, [r, g, b, a]], ...].
    if let Some(keys) = v.as_array_mut() {
        for k in keys {
            if let Some(pair) = k.as_array_mut()
                && pair.len() == 2
                && let Some(rgb) = as_rgb(&pair[1])
            {
                set_rgb(&mut pair[1], recolor_rgb(rgb, target));
            }
        }
    }
}

/// Recolour every `color` in an effect definition (emitters, light, trail) toward `target`.
pub fn recolor(v: &mut Value, target: [f32; 3]) {
    match v {
        Value::Object(map) => {
            for (k, val) in map.iter_mut() {
                if k == "color" {
                    recolor_value(val, target);
                } else {
                    recolor(val, target);
                }
            }
        }
        Value::Array(a) => {
            for x in a {
                recolor(x, target);
            }
        }
        _ => {}
    }
}

/// Rename an effect's emitters (and the layers they spawn into) with a prefix, so merged
/// effects keep their sub-emitters apart.
fn prefix_emitters(desc: &mut Value, prefix: &str) {
    let Some(ems) = desc.get_mut("emitters").and_then(Value::as_array_mut) else {
        return;
    };
    for em in ems {
        if let Some(name) = em.get("name").and_then(Value::as_str) {
            em["name"] = json!(format!("{prefix}{name}"));
        }
        for sub in ["on_hit", "on_death", "trailing"] {
            if let Some(layer) = em
                .get(sub)
                .and_then(|s| s.get("layer"))
                .and_then(Value::as_str)
                .map(str::to_string)
            {
                em[sub]["layer"] = json!(format!("{prefix}{layer}"));
            }
        }
    }
}

/// Build a stand-in definition for `name` from its bases, recoloured to `color`.
pub fn stand_in(
    world: &mut World,
    name: &str,
    look: &str,
    role: Role,
    color: [f32; 3],
) -> Option<Value> {
    let mut merged: Option<Value> = None;
    for (i, base) in bases(look, role).iter().enumerate() {
        let Ok(mut desc) = call_api(world, "vfx.describe", json!({ "effect": base })) else {
            continue;
        };
        match merged.as_mut() {
            None => merged = Some(desc),
            Some(m) => {
                prefix_emitters(&mut desc, &format!("m{i}_"));
                if let (Some(dst), Some(src)) = (
                    m.get_mut("emitters").and_then(Value::as_array_mut),
                    desc.get("emitters").and_then(Value::as_array),
                ) {
                    dst.extend(src.iter().cloned());
                }
            }
        }
    }
    let mut desc = merged?;
    // Keep the base's own colors where the look is the base's own element.
    let own = matches!(
        (look, role),
        ("fire", Role::Bolt | Role::Impact | Role::Cone)
            | ("frost", Role::Bolt | Role::Impact | Role::Cone)
            | ("burning", _)
    );
    if !own {
        recolor(&mut desc, color);
    }
    desc["name"] = json!(name);
    desc["description"] = json!(format!(
        "Stand-in for the magic module's {look} {} look, recoloured from {}; define \"{name}\" \
         to replace it.",
        role.key(),
        bases(look, role).join(" + ")
    ));
    // Bright bolts light their surroundings.
    if role == Role::Bolt && bright(look) && desc.get("light").is_none_or(Value::is_null) {
        desc["light"] = json!({
            "color": color,
            "intensity_lm": 3000.0,
            "range": 9.0,
            "flicker": 0.25,
            "fade_secs": 0.3,
        });
    }
    if role == Role::Charge && bright(look) {
        desc["light"] = json!({
            "color": color,
            "intensity_lm": 1200.0,
            "range": 5.0,
            "flicker": 0.3,
            "fade_secs": 0.2,
        });
    }
    Some(desc)
}

impl LookLibrary {
    /// The particle effect to play for a look: its own name if the library has it, else a
    /// stand-in made now, else None.
    fn resolve(world: &mut World, look: &str, role: Role, color: [f32; 3]) -> Option<String> {
        let key = (look.to_string(), role);
        if let Some(hit) = world.resource::<LookLibrary>().resolved.get(&key) {
            return hit.clone();
        }
        let name = look_name(look, role);
        let found = if call_api(world, "vfx.describe", json!({ "effect": name })).is_ok() {
            Some(name)
        } else if let Some(desc) = stand_in(world, &name, look, role, color) {
            match call_api(world, "vfx.define", json!({ "effect": desc })) {
                Ok(_) => {
                    world
                        .resource_mut::<LookLibrary>()
                        .stand_ins
                        .push(name.clone());
                    Some(name)
                }
                Err(e) => {
                    tracing::warn!("magic: stand-in {name} refused: {e}");
                    bases(look, role).first().map(|b| b.to_string())
                }
            }
        } else {
            None
        };
        world
            .resource_mut::<LookLibrary>()
            .resolved
            .insert(key, found.clone());
        found
    }
}

fn arr(p: DVec3) -> [f64; 3] {
    p.to_array()
}

/// Show queued looks. Runs inside a command at the end of the step.
pub fn apply(world: &mut World, looks: Vec<LookCmd>) {
    if !world.contains_resource::<LookLibrary>() {
        world.init_resource::<LookLibrary>();
    }
    if world.resource::<LookLibrary>().no_particles {
        return;
    }
    if call_api(world, "vfx.stats", json!({})).is_err() {
        world.resource_mut::<LookLibrary>().no_particles = true;
        return;
    }
    // Once vfx draws `SpellLook`s and `SpellCue`s (written by `write_contracts`), every look
    // here would draw twice: the stand-ins go quiet. It says so with the engine's
    // `SpellLookDrawer` or, before that, its `vfx.spell_contracts` API.
    if world.contains_resource::<SpellLookDrawer>() {
        return;
    }
    let reads = match world.resource::<LookLibrary>().vfx_reads_contracts {
        Some(r) => r,
        None => {
            let r = call_api(world, "vfx.spell_contracts", json!({})).is_ok_and(|v| {
                v.get("looks").and_then(Value::as_bool) == Some(true)
                    && v.get("cues").and_then(Value::as_bool) == Some(true)
            });
            if r {
                tracing::info!(
                    "magic: vfx draws spells from SpellLook and SpellCue; stand-ins off"
                );
            }
            world.resource_mut::<LookLibrary>().vfx_reads_contracts = Some(r);
            r
        }
    };
    if reads {
        return;
    }
    for cmd in looks {
        match cmd {
            LookCmd::Attach {
                entity,
                look,
                role,
                scale,
                color,
                ..
            } => {
                if world.get_entity(entity).is_err() {
                    continue;
                }
                let Some(name) = LookLibrary::resolve(world, &look, role, color) else {
                    continue;
                };
                let _ = call_api(
                    world,
                    "vfx.spawn",
                    json!({ "effect": name, "entity": { "id": entity.to_bits() }, "scale": scale, "once": false }),
                );
            }
            LookCmd::Burst {
                at,
                up,
                look,
                role,
                scale,
                color,
            } => {
                let Some(name) = LookLibrary::resolve(world, &look, role, color) else {
                    continue;
                };
                // A cone is a gout of bursts along its axis, growing as they go.
                let shots: Vec<(DVec3, f32)> = if role == Role::Cone {
                    let range = scale * 4.0;
                    (1..=4)
                        .map(|k| {
                            let f = k as f32 / 4.0;
                            (
                                at + (up * range * f * 0.9).as_dvec3(),
                                0.35 + 0.45 * f * scale.max(1.0),
                            )
                        })
                        .collect()
                } else {
                    vec![(at, scale)]
                };
                for (p, s) in shots {
                    let _ = call_api(
                        world,
                        "vfx.spawn",
                        json!({ "effect": name, "position": arr(p), "up": up.to_array(), "scale": s, "once": true, "duration": 8.0 }),
                    );
                }
            }
            LookCmd::Stop { entity } => {
                if world.get_entity(entity).is_ok() {
                    let _ = call_api(
                        world,
                        "vfx.stop",
                        json!({ "entity": { "id": entity.to_bits() } }),
                    );
                }
            }
            LookCmd::Beam {
                from,
                to,
                look,
                color,
                width,
                secs,
            } => {
                let style = match family(&look) {
                    Family::Storm => "lightning",
                    Family::Mist => "smoke",
                    _ => "energy",
                };
                let _ = call_api(
                    world,
                    "vfx.beam",
                    json!({ "from": arr(from), "to": arr(to), "color": color, "width": width, "style": style, "duration": secs }),
                );
            }
            LookCmd::Charge {
                caster,
                look,
                secs,
                scale,
            } => {
                let Some(hand) = hand_position(world, caster) else {
                    continue;
                };
                let color = crate::combine::recipe(&look)
                    .map(|r| r.color)
                    .or_else(|| crate::schools::Element::from_key(&look).map(|e| e.color()))
                    .unwrap_or([0.8, 0.8, 1.0]);
                let Some(name) = LookLibrary::resolve(world, &look, Role::Charge, color) else {
                    continue;
                };
                if let Ok(v) = call_api(
                    world,
                    "vfx.spawn",
                    json!({ "effect": name, "position": arr(hand), "scale": scale, "once": false, "duration": secs + 0.4 }),
                ) && let Some(id) = v["entity"]["id"].as_u64().and_then(Entity::try_from_bits)
                {
                    world
                        .resource_mut::<LookLibrary>()
                        .charges
                        .insert(caster, id);
                }
            }
            LookCmd::EndCharge { caster } => {
                let charge = world.resource_mut::<LookLibrary>().charges.remove(&caster);
                if let Some(c) = charge
                    && world.get_entity(c).is_ok()
                {
                    let _ = call_api(
                        world,
                        "vfx.stop",
                        json!({ "entity": { "id": c.to_bits() } }),
                    );
                }
            }
            LookCmd::RemoveCasting { .. } => {}
            LookCmd::Mark {
                at,
                normal,
                kind,
                radius,
            } => {
                let glow = if matches!(kind, "scorch" | "burn") {
                    4.0
                } else {
                    0.0
                };
                let mut args = json!({ "kind": kind, "position": arr(at), "normal": normal.to_array(), "radius": radius, "life": 90.0, "glow": glow });
                if call_api(world, "vfx.mark", args.clone()).is_err() {
                    let Some(older) = older_mark(kind) else {
                        continue;
                    };
                    args["kind"] = json!(older);
                    let _ = call_api(world, "vfx.mark", args);
                }
            }
        }
    }
}

/// The mark a vfx without the newer kinds draws instead (blood, rot, void and the lightning
/// fern arrived after scorch, frost, wet and chip).
fn older_mark(kind: &str) -> Option<&'static str> {
    match kind {
        "blood" | "rot" => Some("wet"),
        "burn" => Some("scorch"),
        _ => None,
    }
}

/// The engine's spell role for a carrier's look.
pub fn spell_role(role: Role) -> SpellRole {
    match role {
        Role::Bolt => SpellRole::Bolt,
        Role::Wall => SpellRole::Wall,
        Role::Aura => SpellRole::Aura,
        Role::Charge => SpellRole::Charge,
        Role::Status => SpellRole::Status,
        Role::Field | Role::Impact | Role::Cone | Role::Fizzle => SpellRole::Field,
    }
}

/// The engine's mark for a mark kind (the newer kinds included).
pub fn spell_mark(kind: &str) -> SpellMark {
    match kind {
        "frost" => SpellMark::Frost,
        "wet" => SpellMark::Wet,
        "blood" => SpellMark::Blood,
        "rot" => SpellMark::Rot,
        "void" => SpellMark::Void,
        "burn" => SpellMark::Burn,
        _ => SpellMark::Scorch,
    }
}

/// What a look command means in the engine's spell contracts (`engine_world::magic`): a
/// [`SpellLook`] put on or taken off an entity, or a one-shot [`SpellCue`]. The flush writes
/// both beside the stand-ins, so vfx and audio can draw and sound spells from the contracts
/// alone.
#[derive(Debug, Clone, PartialEq)]
pub enum Contract {
    Put(Entity, SpellLook),
    Take(Entity),
    Cue(SpellCue),
}

pub fn contract(cmd: &LookCmd) -> Option<Contract> {
    Some(match cmd {
        LookCmd::Attach {
            entity,
            look,
            role,
            scale,
            color,
            power_j,
        } => Contract::Put(
            *entity,
            SpellLook {
                look: look.clone(),
                role: spell_role(*role),
                scale: *scale,
                color: *color,
                power_j: *power_j,
            },
        ),
        LookCmd::Stop { entity } => Contract::Take(*entity),
        LookCmd::Charge {
            caster,
            look,
            scale,
            ..
        } => Contract::Put(
            *caster,
            SpellLook {
                look: look.clone(),
                role: SpellRole::Charge,
                scale: *scale,
                ..Default::default()
            },
        ),
        LookCmd::EndCharge { caster } => Contract::Take(*caster),
        LookCmd::Burst {
            at,
            up,
            look,
            role,
            scale,
            color,
        } => Contract::Cue(match role {
            Role::Cone => SpellCue::Cone {
                at: *at,
                dir: *up,
                look: look.clone(),
                range_m: scale * 4.0,
            },
            Role::Fizzle => SpellCue::Fizzle {
                at: *at,
                look: look.clone(),
            },
            _ => SpellCue::Impact {
                at: *at,
                normal: *up,
                look: look.clone(),
                scale: *scale,
                color: *color,
            },
        }),
        LookCmd::Beam {
            from,
            to,
            look,
            width,
            secs,
            ..
        } => Contract::Cue(SpellCue::Beam {
            from: *from,
            to: *to,
            look: look.clone(),
            width_m: *width,
            secs: *secs,
        }),
        LookCmd::Mark {
            at,
            normal,
            kind,
            radius,
        } => Contract::Cue(SpellCue::Mark {
            at: *at,
            normal: *normal,
            kind: spell_mark(kind),
            radius_m: *radius,
        }),
        LookCmd::RemoveCasting { .. } => return None,
    })
}

/// Put the step's looks into the contracts: [`SpellLook`] on carriers, [`SpellCue`]s out.
pub fn write_contracts(world: &mut World, looks: &[LookCmd]) {
    let can_cue = world.contains_resource::<Messages<SpellCue>>();
    for l in looks {
        match contract(l) {
            Some(Contract::Put(e, look)) => {
                if let Ok(mut em) = world.get_entity_mut(e) {
                    em.insert(look);
                }
            }
            Some(Contract::Take(e)) => {
                if let Ok(mut em) = world.get_entity_mut(e) {
                    em.remove::<SpellLook>();
                }
            }
            Some(Contract::Cue(c)) if can_cue => {
                world.write_message(c);
            }
            _ => {}
        }
    }
}

fn hand_position(world: &World, caster: Entity) -> Option<DVec3> {
    let g = world.get::<GlobalTransform>(caster)?;
    let c = world.get::<crate::caster::MagicCaster>(caster)?;
    Some(crate::runtime::hand_of(g, c))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recolor_keeps_brightness_and_takes_the_hue() {
        let fire_core = [4.0, 2.4, 0.8];
        let blue = recolor_rgb(fire_core, [0.45, 0.75, 1.0]);
        assert!((blue[0].max(blue[1]).max(blue[2]) - 4.0).abs() < 0.01);
        assert!(blue[2] > blue[0]);
        let white = recolor_rgb([1.0, 1.0, 1.0], [1.0, 0.0, 0.0]);
        assert!(white[1] > 0.3, "white keeps some of its core: {white:?}");
    }

    #[test]
    fn recolor_walks_gradients_lights_and_trails() {
        let mut v = json!({
            "emitters": [{ "name": "core", "color": [[0.0, [1.0, 0.5, 0.1, 1.0]], [1.0, [0.5, 0.2, 0.0, 0.0]]] }],
            "light": { "color": [1.0, 0.5, 0.2], "intensity_lm": 100.0 },
            "trail": { "color": [[0.0, [1.0, 0.6, 0.2, 1.0]]] }
        });
        recolor(&mut v, [0.2, 0.4, 1.0]);
        let c = &v["emitters"][0]["color"][0][1];
        assert!(c[2].as_f64().unwrap() > c[0].as_f64().unwrap());
        assert_eq!(c[3].as_f64().unwrap(), 1.0, "alpha untouched");
        assert!(
            v["light"]["color"][2].as_f64().unwrap() > v["light"]["color"][0].as_f64().unwrap()
        );
        assert!(v["trail"]["color"][0][1][2].as_f64().unwrap() > 0.5);
    }

    #[test]
    fn every_look_has_a_base() {
        for look in [
            "fire", "steam", "doom", "force", "burning", "frozen", "arcane",
        ] {
            for role in [
                Role::Bolt,
                Role::Impact,
                Role::Field,
                Role::Aura,
                Role::Charge,
                Role::Status,
                Role::Fizzle,
            ] {
                assert!(!bases(look, role).is_empty());
            }
        }
        assert_eq!(look_name("steam", Role::Field), "spell.steam.field");
        assert_eq!(look_name("frozen", Role::Status), "spell.status.frozen");
    }
}
