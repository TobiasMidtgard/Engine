//! From a [`SpellDesign`] and who casts it to a runtime [`Spell`]: every number the carriers,
//! hits and statuses need, the cost, and the odds of a fizzle or backfire.
//!
//! Balance lives in a few formulas here, not in each spell, so a strange or generated weave can
//! surprise but never be free or broken (spell-combination.md section 5):
//! - damage = power's damage × the cast type's strength × 1.5 if dark × 1.25 if unstable,
//!   shared out over split projectiles;
//! - mana = the cast type's base × (1 + 0.3 × complexity) × √(power ÷ the caster's own power)
//!   × √(duration), ÷ 1.5 if dark (dark magic is paid in health and corruption too);
//! - a spell past its budget only casts with Overcharge, and then risks backfiring.

use std::sync::Arc;

use engine_world::DamageKind;
use serde::Serialize;

use crate::combine::{Reaction, Weave, weave};
use crate::design::{CastType, Modifier, SpellDesign, TargetFilter, Trigger};
use crate::effects::{EffectKind, ForceDir, effect};
use crate::schools::{Element, Mastery, Power, Status};

/// Who is casting, for compiling: their mastery in the spell's subclass and their power.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CastContext {
    pub mastery: Mastery,
    /// Energy put into this cast.
    pub power: Power,
    /// The caster's own power (what their mana pool is sized for).
    pub own_power: Power,
}

impl CastContext {
    pub fn new(mastery: Mastery, power: Power) -> Self {
        Self {
            mastery,
            power,
            own_power: power,
        }
    }
}

/// How the spell travels: every shape and motion number, already scaled by power and
/// modifiers.
#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
pub struct Carrier {
    /// Flight speed, m/s (bolt, lob, wave, rain drops).
    pub speed: f32,
    /// How far it goes, or how far away an area may be placed, m.
    pub range: f32,
    /// Projectile radius, or the area's radius, m.
    pub radius: f32,
    /// Share of gravity a projectile feels (0 bolt, 1 lob).
    pub gravity: f32,
    /// Projectiles per cast (split), orbs (orbit) or drops (rain).
    pub count: u32,
    /// Fan width of a split, degrees.
    pub spread_deg: f32,
    /// Homing turn rate, rad/s (0: none).
    pub seek_turn: f32,
    pub pierce: u32,
    pub bounce: u32,
    /// Projectiles it splits into on a hit.
    pub fork: u32,
    /// Jumps to further targets.
    pub chain: u32,
    /// How far each chain jump may reach, m.
    pub chain_radius: f32,
    pub boomerang: bool,
    pub rend: bool,
    /// How long a field, aura, orbit, rune or beam lasts, s.
    pub duration: f32,
    /// Seconds between a field's or beam's hits.
    pub tick: f32,
    /// Fuse before it fires, s.
    pub delay: f32,
    /// Cone half-width, degrees.
    pub cone_deg: f32,
    /// Stacks that make a status burst (5: frost freezes).
    pub stack_threshold: u32,
}

/// What the spell does to what it reaches, already scaled by power.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Payload {
    /// Health damage per hit.
    pub damage: f32,
    pub damage_kind: DamageKind,
    /// Statuses each hit adds, in stacks.
    pub statuses: Vec<(Status, f32)>,
    /// A force vector and its impulse on a person-sized body, N·s.
    pub force: Option<(ForceDir, f32)>,
    pub heal: f32,
    /// Shield that absorbs this much damage.
    pub ward: f32,
    /// Time scale factor and seconds.
    pub time: Option<(f32, f32)>,
    /// Blink distance, m.
    pub blink_m: f32,
    pub veil_secs: f32,
    pub fear_secs: f32,
    pub calm_secs: f32,
    /// A light it leaves, lumens.
    pub light_lm: f32,
    /// Flame temperature for igniting fuel, °C (0: doesn't ignite).
    pub ignite_c: f32,
    /// Share of damage dealt that heals the caster.
    pub lifesteal: f32,
    /// Knocks out instead of killing.
    pub subdue: bool,
    /// Height of the stone it raises, m (0: none): a wall piece, a pillar, a step (stone.rs).
    pub stone_m: f32,
    /// Seconds the stone stands before it sinks (a wall's stone goes with the wall).
    pub stone_secs: f32,
    /// The weave's reaction (a woven spell's bonus, clouds, chains).
    #[serde(skip)]
    pub reaction: Reaction,
}

impl Payload {
    /// Hurts what it lands on.
    pub fn harmful(&self) -> bool {
        self.damage > 0.0 || self.force.is_some() || self.fear_secs > 0.0
    }
}

/// What a cast costs.
#[derive(Debug, Clone, Copy, PartialEq, Default, Serialize)]
pub struct Cost {
    pub mana: f32,
    pub health: f32,
    /// Corruption added (dark magic).
    pub corruption: f32,
}

/// A compiled spell: shared between every carrier it spawns.
#[derive(Debug, Clone, Serialize)]
pub struct Spell {
    pub name: String,
    pub weave: Weave,
    #[serde(skip)]
    pub design: SpellDesign,
    /// The cast type that runs (Imbue casts as Touch until the combat module's weapons exist).
    pub cast: CastType,
    pub carrier: Carrier,
    pub payload: Payload,
    /// Further spells fired by triggers, including the blast and linger stages that modifiers
    /// make.
    #[serde(skip)]
    pub stages: Vec<(Trigger, Arc<Spell>)>,
    pub complexity: u32,
    pub budget: u32,
    pub cost: Cost,
    /// Chance the cast fizzles (nothing happens, mana half spent).
    pub fizzle: f32,
    /// Chance it backfires on the caster (overcharge, unstable weaves).
    pub backfire: f32,
    pub power: Power,
    pub mastery: Mastery,
    pub dark: bool,
    /// Seconds to gather it before release.
    pub cast_secs: f32,
    /// Most seconds it can be charged for (0: no charge).
    pub charge_secs: f32,
    /// Recasts itself once.
    pub echo: bool,
    /// How far its casting is heard, m (0: silent).
    pub noise_m: f32,
    pub filter: TargetFilter,
    pub safeguard: bool,
}

impl Spell {
    /// The first element, for looks and statuses (None for pure force, heal, veil...).
    pub fn element(&self) -> Option<Element> {
        self.weave.elements.first().copied()
    }

    /// Stages fired by a trigger.
    pub fn stages_on(&self, trigger: Trigger) -> impl Iterator<Item = &Arc<Spell>> {
        self.stages
            .iter()
            .filter(move |(t, _)| *t == trigger)
            .map(|(_, s)| s)
    }

    /// One line for a readout: name, form, complexity against budget, mana.
    pub fn summary(&self) -> String {
        let mut parts = vec![self.cast.name().to_string()];
        if self.carrier.count > 1 && !matches!(self.cast, CastType::Rain) {
            parts.push(format!("×{}", self.carrier.count));
        }
        for m in &self.design.modifiers {
            match m {
                Modifier::Split { .. } => {}
                m => parts.push(m.name().to_lowercase()),
            }
        }
        format!(
            "{} · {} · complexity {}/{} · {:.0} mana{}",
            self.name,
            parts.join(", "),
            self.complexity,
            self.budget,
            self.cost.mana,
            if self.cost.health > 0.0 {
                format!(" + {:.0} health", self.cost.health)
            } else {
                String::new()
            }
        )
    }
}

/// Why a design can't be compiled.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct CompileError(pub Vec<String>);

impl std::fmt::Display for CompileError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0.join(" "))
    }
}

/// Compile a design for a caster. Fails with plain sentences when the design is invalid or
/// past the caster's budget without Overcharge.
pub fn compile(design: &SpellDesign, ctx: CastContext) -> Result<Arc<Spell>, CompileError> {
    let problems = design.problems();
    if !problems.is_empty() {
        return Err(CompileError(problems));
    }
    let complexity = design.complexity();
    let budget = ctx.mastery.budget();
    let overcharge = design.has(|m| matches!(m, Modifier::Overcharge));
    if complexity > budget && !overcharge {
        return Err(CompileError(vec![format!(
            "Too complex: {complexity} against a {} budget of {budget}. Remove parts, raise mastery, or add Overcharge to risk it.",
            ctx.mastery.name()
        )]));
    }
    Ok(Arc::new(compile_inner(
        design, ctx, complexity, budget, 1.0,
    )))
}

fn compile_inner(
    design: &SpellDesign,
    ctx: CastContext,
    complexity: u32,
    budget: u32,
    strength: f32,
) -> Spell {
    let w = weave(&design.effects).expect("validated designs have known effects");
    let dark = design.is_dark();
    let cast = match design.cast {
        CastType::Imbue => CastType::Touch,
        c => c,
    };
    let size = ctx.power.size();
    let mods = &design.modifiers;
    let factor = |f: &dyn Fn(&Modifier) -> Option<f32>| -> f32 {
        mods.iter().filter_map(f).product::<f32>().max(0.05)
    };
    let speed_k = factor(&|m| match m {
        Modifier::Speed { factor } => Some(*factor),
        _ => None,
    });
    let range_k = factor(&|m| match m {
        Modifier::Range { factor } => Some(*factor),
        _ => None,
    });
    let size_k = factor(&|m| match m {
        Modifier::Size { factor } => Some(*factor),
        _ => None,
    });
    let duration_k = factor(&|m| match m {
        Modifier::Duration { factor } => Some(*factor),
        _ => None,
    });
    let count_of = |f: &dyn Fn(&Modifier) -> Option<u32>| mods.iter().filter_map(f).max();

    // ---- carrier
    let shape = 0.6 + 0.4 * size;
    let (speed, range, radius, gravity, duration, tick) = match cast {
        CastType::Bolt => (26.0, 45.0, 0.16 * shape, 0.0, 0.0, 0.0),
        CastType::Lob => (17.0, 32.0, 0.2 * shape, 1.0, 0.0, 0.0),
        CastType::Beam => (0.0, 22.0, 0.12 * shape, 0.0, 2.5, 0.1),
        CastType::Cone => (0.0, 7.0 * shape.sqrt(), 0.0, 0.0, 0.0, 0.0),
        CastType::Wave => (11.0, 20.0, 1.6 * shape, 0.0, 0.0, 0.0),
        CastType::Wall => (0.0, 20.0, 4.0 * shape, 0.0, 8.0, 0.5),
        CastType::Area => (0.0, 30.0, 3.0 * shape, 0.0, 0.0, 0.5),
        CastType::Aura => (0.0, 0.0, 2.5 * shape, 0.0, 10.0, 0.5),
        CastType::Rune => (0.0, 20.0, 1.6 * shape, 0.0, 60.0, 0.0),
        CastType::Orbit => (2.6, 0.0, 0.14 * shape, 0.0, 10.0, 0.0),
        CastType::Chain => (0.0, 20.0, 0.0, 0.0, 0.0, 0.0),
        CastType::Rain => (22.0, 30.0, 5.0 * shape, 1.0, 2.5, 0.0),
        CastType::Touch => (0.0, 2.2, 0.4, 0.0, 0.0, 0.0),
        // A link that works every quarter second for 4 s (or while held), and snaps when
        // stretched past 1.3 times its range (tether.rs).
        CastType::Tether => (0.0, 14.0, 0.1 * shape, 0.0, 4.0, 0.25),
        // A creature placed within 14 m that stays a minute (Skyrim's atronachs; summon.rs).
        CastType::Summon => (0.0, 14.0, 1.0 * shape, 0.0, 60.0, 0.0),
        _ => (0.0, 0.0, 0.0, 0.0, 0.0, 0.0),
    };
    let split = count_of(&|m| match m {
        Modifier::Split { count } => Some(*count),
        _ => None,
    })
    .unwrap_or(1);
    let count = match cast {
        CastType::Orbit => 3 * split.max(1),
        CastType::Rain => (10.0 * size_k * size_k * split as f32).round() as u32,
        _ => split.max(1),
    };
    let chain = count_of(&|m| match m {
        Modifier::Chain { jumps } => Some(*jumps),
        _ => None,
    })
    .unwrap_or(if cast == CastType::Chain { 3 } else { 0 });
    let stack_threshold = count_of(&|m| match m {
        Modifier::Stacking { threshold } => Some(*threshold),
        _ => None,
    })
    .unwrap_or(5)
    .clamp(2, 10);
    let seek = design.has(|m| matches!(m, Modifier::Seek));
    let carrier = Carrier {
        speed: speed * speed_k,
        range: range * range_k,
        radius: radius * size_k,
        gravity,
        count,
        spread_deg: if count > 1 {
            (8.0 * (count - 1) as f32).min(48.0)
        } else {
            0.0
        },
        seek_turn: if seek { 2.6 } else { 0.0 },
        pierce: count_of(&|m| match m {
            Modifier::Pierce { count } => Some(*count),
            _ => None,
        })
        .unwrap_or(0),
        bounce: count_of(&|m| match m {
            Modifier::Bounce { count } => Some(*count),
            _ => None,
        })
        .unwrap_or(0),
        fork: count_of(&|m| match m {
            Modifier::Fork { count } => Some(*count),
            _ => None,
        })
        .unwrap_or(0),
        chain,
        chain_radius: 8.0 * range_k.sqrt(),
        boomerang: design.has(|m| matches!(m, Modifier::Boomerang)),
        rend: design.has(|m| matches!(m, Modifier::Rend)),
        // An area given a duration lasts (a burning patch, a lifting field) instead of bursting.
        duration: if cast == CastType::Area
            && duration == 0.0
            && design.has(|m| matches!(m, Modifier::Duration { .. }))
        {
            4.0 * duration_k
        } else {
            duration * duration_k
        },
        tick,
        delay: mods
            .iter()
            .find_map(|m| match m {
                Modifier::Delay { secs } => Some(*secs),
                _ => None,
            })
            .unwrap_or(0.0),
        cone_deg: if cast == CastType::Cone { 30.0 } else { 0.0 },
        stack_threshold,
    };

    // ---- payload
    let defs: Vec<_> = w.ingredients.iter().filter_map(|k| effect(k)).collect();
    let unstable = w.unstable > 0.5;
    // Count modifiers divide a spell's energy, never multiply it (Magicka's twelve full-damage
    // shards are the cautionary tale): five fireballs share one fireball's energy, with a
    // small bonus for the ground they cover.
    let spread = if count > 1 {
        (1.0 + 0.1 * (count - 1) as f32) / count as f32
    } else {
        1.0
    };
    let reaction = w.reaction();
    let woven = if w.elements.len() > 1 {
        1.0 + (reaction.bonus - 1.0) * 0.5
    } else {
        1.0
    };
    let base = ctx.power.damage() * cast.strength() * strength;
    let harm = defs
        .iter()
        .any(|d| matches!(d.kind, EffectKind::Harm { .. } | EffectKind::Drain));
    let damage = if harm {
        base * if dark { 1.5 } else { 1.0 }
            * if unstable { 1.25 } else { 1.0 }
            * spread
            * woven
            * if matches!(cast, CastType::Beam | CastType::Tether) {
                carrier.tick * 3.0
            } else {
                1.0
            }
    } else {
        0.0
    };
    let mut statuses: Vec<(Status, f32)> = w.elements.iter().map(|e| (e.status(), 1.0)).collect();
    if w.authored && w.elements.len() > 1 {
        for (s, n) in reaction.statuses {
            match statuses.iter_mut().find(|(t, _)| t == s) {
                Some(entry) => entry.1 = entry.1.max(*n),
                None => statuses.push((*s, *n)),
            }
        }
    }
    let impulse = 60.0 * ((ctx.power.energy_j / Power::ORDINARY_J) as f32).cbrt() * strength;
    let force = defs.iter().find_map(|d| match d.kind {
        EffectKind::Force { dir, strength } => Some((dir, impulse * strength)),
        _ => None,
    });
    // A tether pulls on every tick: a steady haul (about 2 m/s a tick on a person), not a
    // shove each time.
    let force = force.map(|(d, i)| {
        if cast == CastType::Tether {
            (d, (i * tick * 1.5).min(90.0))
        } else {
            (d, i)
        }
    });
    let has = |f: &dyn Fn(&EffectKind) -> bool| defs.iter().any(|d| f(&d.kind));
    let time = defs.iter().find_map(|d| match d.kind {
        EffectKind::TimeScale { factor } => Some((factor, 6.0 * duration_k)),
        _ => None,
    });
    let lifesteal = mods
        .iter()
        .find_map(|m| match m {
            Modifier::Lifesteal { share } => Some(*share),
            _ => None,
        })
        .unwrap_or(0.0)
        + if has(&|k| matches!(k, EffectKind::Drain)) {
            0.5
        } else if w.elements.contains(&Element::Blood) {
            0.15
        } else {
            0.0
        }
        + reaction.drain;
    let fire = w.elements.contains(&Element::Fire);
    let payload = Payload {
        damage,
        damage_kind: damage_kind(&w),
        statuses: if harm { statuses } else { Vec::new() },
        force,
        heal: if has(&|k| matches!(k, EffectKind::Heal)) {
            base * 1.2
        } else {
            0.0
        },
        ward: if has(&|k| matches!(k, EffectKind::Ward)) {
            base * 2.0
        } else {
            0.0
        },
        time,
        blink_m: if has(&|k| matches!(k, EffectKind::Blink { .. })) {
            (8.0 * size.sqrt() * range_k).clamp(3.0, 60.0)
        } else {
            0.0
        },
        veil_secs: if has(&|k| matches!(k, EffectKind::Veil)) {
            12.0 * duration_k
        } else {
            0.0
        },
        fear_secs: if has(&|k| matches!(k, EffectKind::Fear))
            || w.elements.contains(&Element::Shadow)
        {
            4.0 * duration_k
        } else {
            0.0
        },
        calm_secs: if has(&|k| matches!(k, EffectKind::Calm)) {
            8.0 * duration_k
        } else {
            0.0
        },
        light_lm: if has(&|k| matches!(k, EffectKind::Light)) {
            4000.0 * size
        } else {
            0.0
        },
        ignite_c: if fire {
            (500.0 + 300.0 * size).min(1400.0)
        } else {
            0.0
        },
        lifesteal: lifesteal.min(1.0),
        subdue: design.has(|m| matches!(m, Modifier::Subdue)),
        // Head-high (2.2 m) for an Adept, near 3 m for a master, more with more power; Size
        // changes a wall's length or a pillar's girth, not how high the stone stands.
        stone_m: if has(&|k| matches!(k, EffectKind::Stone)) {
            (1.9 + 0.6 * size).clamp(1.2, 5.0)
        } else {
            0.0
        },
        stone_secs: if has(&|k| matches!(k, EffectKind::Stone)) {
            12.0 * duration_k
        } else {
            0.0
        },
        reaction,
    };

    // ---- stages: those the design names, plus the blast and linger its modifiers imply.
    let mut stages: Vec<(Trigger, Arc<Spell>)> = Vec::new();
    let mut element_keys: Vec<String> = w.elements.iter().map(|e| e.key().to_string()).collect();
    if element_keys.is_empty() {
        element_keys = w.ingredients.clone();
    }
    for m in mods {
        match *m {
            Modifier::Blast { radius_m } => {
                let mut fx = element_keys.clone();
                if !fx.iter().any(|k| k == "push") {
                    fx.push("push".into());
                }
                let blast = SpellDesign {
                    name: format!("{} blast", w.name),
                    effects: fx,
                    cast: CastType::Area,
                    modifiers: vec![Modifier::Size {
                        factor: radius_m / 3.0,
                    }],
                    stages: Vec::new(),
                };
                let mut s = compile_inner(&blast, ctx, complexity, budget, 0.8 * strength);
                s.carrier.duration = 0.0;
                stages.push((Trigger::Impact, Arc::new(s)));
            }
            Modifier::Linger { secs, radius_m } => {
                let linger = SpellDesign {
                    name: format!("{} ground", w.name),
                    effects: element_keys.clone(),
                    cast: CastType::Area,
                    modifiers: vec![Modifier::Size {
                        factor: radius_m / 3.0,
                    }],
                    stages: Vec::new(),
                };
                let mut s = compile_inner(&linger, ctx, complexity, budget, 0.2 * strength);
                s.carrier.duration = secs;
                s.carrier.tick = 0.5;
                stages.push((Trigger::Impact, Arc::new(s)));
            }
            _ => {}
        }
    }
    for st in &design.stages {
        stages.push((
            st.trigger,
            Arc::new(compile_inner(
                &st.spell,
                ctx,
                complexity,
                budget,
                0.85 * strength,
            )),
        ));
    }

    // ---- cost, odds, timing
    let ratio = (ctx.power.energy_j / ctx.own_power.energy_j.max(1.0)) as f32;
    // Mastery makes spells cheaper, with diminishing returns (Skyrim: about 40% at 100), but
    // never below 40% of the price.
    let skill = (ctx.mastery.skill() + 100.0 * ctx.mastery.ascensions() as f32).min(400.0);
    let discount = (1.0 - (skill / 400.0).powf(0.65)).max(0.4);
    // Total delivered effect is what's priced: a longer duration costs at least in proportion
    // (Oblivion's exponent 1.28 keeps spreading damage over time from being a discount).
    let mut mana = (cast.base_mana()
        * (1.0 + 0.3 * complexity as f32)
        * ratio.max(0.0).powf(0.75)
        * duration_k.powf(1.25)
        * discount
        * if dark { 1.0 / 1.5 } else { 1.0 })
    .max(1.0);
    let mut health = 0.0;
    if design.has(|m| matches!(m, Modifier::BloodPrice)) {
        health = mana * 0.8;
        mana = 0.0;
    }
    let corruption = if dark {
        0.5 + 0.1 * complexity as f32
    } else {
        0.0
    };
    let edge = complexity as f32 / budget.max(1) as f32;
    let over = complexity.saturating_sub(budget) as f32;
    let fizzle = (ctx.mastery.base_fizzle() + (edge - 0.8).max(0.0) * 0.4).clamp(0.0, 0.9);
    let backfire = (0.15 * over + if unstable { 0.04 * w.unstable } else { 0.0 }).clamp(0.0, 0.9);
    let loud = defs.iter().any(|d| {
        matches!(
            d.kind,
            EffectKind::Harm {
                element: Element::Fire | Element::Storm
            } | EffectKind::Force { .. }
        )
    });
    let noise_m = if design.has(|m| matches!(m, Modifier::Silent)) {
        0.0
    } else if loud {
        (35.0 * size.sqrt()).clamp(15.0, 300.0)
    } else {
        12.0
    };
    let name = if design.name.is_empty() {
        format!("{} {}", w.name, cast.name().to_lowercase())
    } else {
        design.name.clone()
    };
    Spell {
        name,
        dark,
        cast,
        carrier,
        payload,
        stages,
        complexity,
        budget,
        cost: Cost {
            mana,
            health,
            corruption,
        },
        fizzle,
        backfire,
        power: ctx.power,
        mastery: ctx.mastery,
        cast_secs: ctx.mastery.cast_secs() * (1.0 + 0.04 * complexity as f32),
        charge_secs: mods
            .iter()
            .find_map(|m| match m {
                Modifier::Charge { max_secs } => Some(*max_secs),
                _ => None,
            })
            .unwrap_or(0.0),
        echo: design.has(|m| matches!(m, Modifier::Echo)),
        noise_m,
        filter: mods
            .iter()
            .find_map(|m| match m {
                Modifier::Filter { who } => Some(*who),
                _ => None,
            })
            .unwrap_or_default(),
        safeguard: design.has(|m| matches!(m, Modifier::Safeguard)),
        design: design.clone(),
        weave: w,
    }
}

fn damage_kind(w: &Weave) -> DamageKind {
    match w.elements.first() {
        Some(Element::Fire) => DamageKind::Fire,
        Some(Element::Frost) => DamageKind::Cold,
        Some(_) => DamageKind::Magic,
        None => DamageKind::Blunt,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::design::Modifier;

    fn ctx(m: Mastery, level: f32) -> CastContext {
        CastContext::new(m, Power::at_level(level))
    }

    #[test]
    fn a_master_volley_splits_seeks_explodes_and_lingers() {
        let d = SpellDesign::new(&["fire"], CastType::Bolt)
            .with(Modifier::Split { count: 5 })
            .with(Modifier::Seek)
            .with(Modifier::Blast { radius_m: 3.0 })
            .with(Modifier::Linger {
                secs: 10.0,
                radius_m: 2.5,
            });
        // Too complex for an Expert.
        assert!(compile(&d, ctx(Mastery::Expert, 70.0)).is_err());
        let s = compile(&d, ctx(Mastery::Master, 100.0)).unwrap();
        assert_eq!(s.carrier.count, 5);
        assert!(s.carrier.seek_turn > 0.0);
        assert_eq!(s.stages_on(Trigger::Impact).count(), 2);
        let blast = s.stages_on(Trigger::Impact).next().unwrap();
        assert!(blast.payload.force.is_some());
        assert!(blast.carrier.radius > 2.0);
        let linger = s.stages_on(Trigger::Impact).nth(1).unwrap();
        assert_eq!(linger.carrier.duration, 10.0);
        assert!(s.payload.ignite_c > 0.0);
        assert_eq!(s.complexity, 11);
        assert!(s.cost.mana > 15.0 && s.cost.mana < 45.0, "{}", s.cost.mana);
        // Overcharge lets the Expert try it, at a risk.
        let over = d.clone().with(Modifier::Overcharge);
        let s = compile(&over, ctx(Mastery::Expert, 70.0)).unwrap();
        assert!(s.backfire > 0.3);
    }

    #[test]
    fn dark_spells_hit_harder_per_mana_and_cost_corruption() {
        let fire = compile(
            &SpellDesign::new(&["fire"], CastType::Bolt),
            ctx(Mastery::Adept, 50.0),
        )
        .unwrap();
        let blood = compile(
            &SpellDesign::new(&["blood"], CastType::Bolt),
            ctx(Mastery::Adept, 50.0),
        )
        .unwrap();
        assert!(blood.payload.damage > fire.payload.damage * 1.4);
        assert!(blood.cost.mana < fire.cost.mana);
        assert!(blood.cost.corruption > 0.0);
        assert_eq!(fire.cost.corruption, 0.0);
        assert!(blood.payload.lifesteal > 0.0);
        let priced = compile(
            &SpellDesign::new(&["blood"], CastType::Bolt).with(Modifier::BloodPrice),
            ctx(Mastery::Adept, 50.0),
        )
        .unwrap();
        assert_eq!(priced.cost.mana, 0.0);
        assert!(priced.cost.health > 0.0);
    }

    #[test]
    fn a_novice_fizzles_often_and_a_master_rarely() {
        let d = SpellDesign::new(&["fire"], CastType::Bolt);
        let novice = compile(&d, ctx(Mastery::Novice, 1.0)).unwrap();
        let master = compile(&d, ctx(Mastery::Master, 100.0)).unwrap();
        assert!(novice.fizzle > 0.3);
        assert!(master.fizzle < 0.05);
        assert!(novice.payload.damage < 10.0);
        assert!(master.payload.damage > 500.0);
        assert!(novice.carrier.radius < master.carrier.radius);
        assert!(novice.cast_secs > master.cast_secs);
    }

    #[test]
    fn woven_elements_carry_their_recipe() {
        let steam = compile(
            &SpellDesign::new(&["fire", "frost"], CastType::Area),
            ctx(Mastery::Master, 100.0),
        )
        .unwrap();
        assert_eq!(steam.weave.key, "steam");
        assert!(steam.payload.reaction.cloud_m > 0.0);
        assert!(steam.name.starts_with("Steam"));
        let heal = compile(
            &SpellDesign::new(&["heal"], CastType::OnSelf),
            ctx(Mastery::Novice, 10.0),
        )
        .unwrap();
        assert!(heal.payload.heal > 0.0);
        assert_eq!(heal.payload.damage, 0.0);
        assert!(!heal.payload.harmful());
        let push = compile(
            &SpellDesign::new(&["push"], CastType::Cone),
            ctx(Mastery::Apprentice, 30.0),
        )
        .unwrap();
        assert!(push.payload.force.is_some());
        assert_eq!(push.payload.damage, 0.0);
    }
}
