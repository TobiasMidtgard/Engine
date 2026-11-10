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
    /// The element a ward is woven with (wards.rs): it soaks that element, takes double from its
    /// opposite and pays back close blows. A ward's element types it rather than hurting.
    pub ward_element: Option<Element>,
    /// A ward cast as a cone is a shield held before the caster (wards.rs): how long an NPC's or
    /// the hall's cast holds it up (the spell bar holds it while the key is down). 0: not one.
    pub shield_secs: f32,
    /// A stone rain is a meteor (cataclysm.rs): the falling rock's size across, m (0: not one).
    pub meteor_m: f32,
    /// A stone wave is a fault line (cataclysm.rs): a crack runs along the ground.
    pub fault: bool,
    /// Time scale factor and seconds.
    pub time: Option<(f32, f32)>,
    /// Blink distance, m (space.rs: to the aim point, never further).
    pub blink_m: f32,
    /// Seconds a portal's ends stay open (0: not a portal; space.rs).
    pub portal_secs: f32,
    /// A portal end's width and height, m.
    pub portal_m: [f32; 2],
    /// Seconds of its path a recall rewinds (0: not a recall; space.rs).
    pub recall_secs: f32,
    /// The heaviest thing a grip lifts, kg (0: not a grip; kinesis.rs): half the cast's energy
    /// lifts it 3 m and throws it at 20 m/s.
    pub grip_kg: f32,
    /// Energy that throws what a grip holds, J: a light thing flies at up to 38 m/s, a heavy
    /// one slower.
    pub throw_j: f32,
    /// How long a grip holds before it throws when nobody holds the spell (an NPC's or the
    /// hall's cast); a ring of torn stones circles this long.
    pub grip_secs: f32,
    /// How far a grip reaches for something to hold, m.
    pub grip_m: f32,
    /// A thrown thing's blow at 20 m/s for a 40 kg crate (the spell's damage scale).
    pub grip_damage: f32,
    pub veil_secs: f32,
    pub fear_secs: f32,
    pub calm_secs: f32,
    /// Copies of the caster it makes (minds.rs): round the caster for a Self cast (Mirror
    /// image), where it aims for a Summon (Decoy). 0: none.
    pub phantasms: u32,
    /// Seconds a copy lasts.
    pub phantasm_secs: f32,
    /// Seconds a creature it hits fights for the caster (minds.rs; 0: none).
    pub command_secs: f32,
    /// Seconds a creature it hits strikes at whatever is near (minds.rs; 0: none).
    pub wrath_secs: f32,
    /// Conjured blades it makes (blades.rs; 0: not a blade spell): a glintblade volley's
    /// count, the phalanx's blades, the bladefall's, the spirit blades', or the blades in the
    /// hand.
    pub blades: u32,
    /// A blade's length, m (a greatblade with Size).
    pub blade_m: f32,
    /// Seconds a phalanx, a spirit blade or a blade in the hand lasts.
    pub blade_secs: f32,
    /// Seconds an Imbue rides on its caster's weapon and its creatures' (blades.rs): each
    /// melee hit lands the spell on what it struck. 0: not an imbue.
    pub imbue_secs: f32,
    /// Seconds chains hold a body they bind (chains.rs; 0: they don't bind).
    pub bind_secs: f32,
    /// A hook on a bolt that drags what it bites to the caster (chains.rs).
    pub hook: bool,
    /// Seconds a chain's bonds hold the bodies it hops to together (chains.rs; 0: none).
    pub bond_secs: f32,
    /// Demons it calls (demons.rs; 0: none): the imps that leap out of a demonic meteor, or the
    /// hounds a hellgate sends first.
    pub demons: u32,
    /// Seconds a hellgate stands and its demons last.
    pub demon_secs: f32,
    /// Seconds roots hold a body they grow round (wilds.rs; 0: none): an entangle's, a seed's,
    /// a treant's slam's.
    pub roots_secs: f32,
    /// A thorn hedge grows along the wall (wilds.rs).
    pub hedge: bool,
    /// Seconds bark covers the caster (wilds.rs; 0: none).
    pub bark_secs: f32,
    /// Seconds a prison of ribs holds a body it closes over (bones.rs; 0: none).
    pub prison_secs: f32,
    /// A spear of bone that pierces every body in its line and sticks where it lands (bones.rs).
    pub spear: bool,
    /// A ribcage stands along the wall (bones.rs).
    pub ribcage: bool,
    /// Seconds bone armour covers the caster (bones.rs; 0: none).
    pub armour_secs: f32,
    /// A summon of it raises a bone dragon (dragon.rs).
    pub bone_dragon: bool,
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
    /// Raises the dead (1: it does; undead.rs): a summon of it makes a thrall of a fallen body,
    /// or a skeleton.
    pub raise: f32,
    /// The weave's reaction (a woven spell's bonus, clouds, chains).
    #[serde(skip)]
    pub reaction: Reaction,
}

impl Payload {
    /// Hurts what it lands on.
    pub fn harmful(&self) -> bool {
        self.damage > 0.0
            || self.force.is_some()
            || self.fear_secs > 0.0
            || self.command_secs > 0.0
            || self.wrath_secs > 0.0
            || self.roots_secs > 0.0
            || self.hedge
            || self.prison_secs > 0.0
            || self.ribcage
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
    /// The cast type that runs.
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
    let cast = design.cast;
    let size = ctx.power.size();
    // Stone falling from the sky or splitting the ground (Cataclysm, cataclysm.rs).
    let stone = w
        .ingredients
        .iter()
        .any(|k| crate::effects::effect(k).is_some_and(|d| matches!(d.kind, EffectKind::Stone)));
    let meteor = stone && cast == CastType::Rain;
    let fault = stone && cast == CastType::Wave;
    // A conjured blade (Shaping, blades.rs): the cast type picks its shape.
    let blade = w
        .ingredients
        .iter()
        .any(|k| crate::effects::effect(k).is_some_and(|d| matches!(d.kind, EffectKind::Blade)));
    // Pact chains (chains.rs) and demons (demons.rs).
    let chained = w.ingredients.iter().any(|k| k == "chains");
    let demon = w
        .ingredients
        .iter()
        .any(|k| crate::effects::effect(k).is_some_and(|d| matches!(d.kind, EffectKind::Demon)));
    // Living wood (Vitalism's Wilds, wilds.rs).
    let wild = w
        .ingredients
        .iter()
        .any(|k| crate::effects::effect(k).is_some_and(|d| matches!(d.kind, EffectKind::Wilds)));
    // Bone out of the ground (Necromancy's Undeath, bones.rs).
    let bone = w
        .ingredients
        .iter()
        .any(|k| crate::effects::effect(k).is_some_and(|d| matches!(d.kind, EffectKind::Bone)));
    let spear = bone && matches!(cast, CastType::Bolt | CastType::Lob);
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
        // A fault line's crack runs at 14 m/s and throws what stands within 1.4 m of it.
        CastType::Wave if fault => (14.0, 18.0, 1.4, 0.0, 0.0, 0.0),
        CastType::Wave => (11.0, 20.0, 1.6 * shape, 0.0, 0.0, 0.0),
        CastType::Wall => (0.0, 20.0, 4.0 * shape, 0.0, 8.0, 0.5),
        CastType::Area => (0.0, 30.0, 3.0 * shape, 0.0, 0.0, 0.5),
        CastType::Aura => (0.0, 0.0, 2.5 * shape, 0.0, 10.0, 0.5),
        CastType::Rune => (0.0, 20.0, 1.6 * shape, 0.0, 60.0, 0.0),
        CastType::Orbit => (2.6, 0.0, 0.14 * shape, 0.0, 10.0, 0.0),
        CastType::Chain => (0.0, 20.0, 0.0, 0.0, 0.0, 0.0),
        // Meteors come down at 42 m/s, one per split, scattered over a few metres.
        CastType::Rain if meteor => (42.0, 40.0, 1.5 + 1.5 * shape, 0.0, 0.0, 0.0),
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
        // A phalanx: two blades over an Adept's head, five over an Ascended's (Elden Ring's
        // glintblade phalanx), one more per Split.
        CastType::Orbit if blade => {
            (match ctx.mastery {
                Mastery::Novice | Mastery::Apprentice => 1,
                Mastery::Adept => 2,
                Mastery::Expert => 3,
                Mastery::Master => 4,
                _ => 5,
            }) + split.max(1)
                - 1
        }
        CastType::Orbit => 3 * split.max(1),
        CastType::Rain if meteor => split.max(1),
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
        // A glintblade turns a little toward what it was thrown at, Seek or not.
        seek_turn: if seek {
            2.6
        } else if blade && cast == CastType::Bolt {
            1.4
        } else {
            0.0
        },
        // A bone spear goes through every body in its line.
        pierce: count_of(&|m| match m {
            Modifier::Pierce { count } => Some(*count),
            _ => None,
        })
        .unwrap_or(0)
        .max(if spear { crate::bones::SPEAR_PIERCE } else { 0 }),
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
        } else if meteor {
            // A swarm's rocks come down one after another, 0.45 s apart.
            0.45 * (count.max(1) - 1) as f32 + 0.01
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
    // A ward woven with an element is a ward of that element, not a blow (wards.rs).
    let warding = defs.iter().any(|d| matches!(d.kind, EffectKind::Ward));
    let harm = !warding
        && defs
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
    // Rock from the sky or the ground's split strikes as a blow on top of any element: more
    // per mana than Evocation's (the design's Cataclysm), shared between a swarm's rocks.
    let damage = damage
        + if meteor {
            ctx.power.damage() * 1.8 * spread * if dark { 1.5 } else { 1.0 }
        } else if fault {
            ctx.power.damage() * 0.9 * if dark { 1.5 } else { 1.0 }
        } else if blade && cast != CastType::Imbue {
            // A conjured blade cuts, element or not, shared between a volley's blades.
            ctx.power.damage() * 0.8 * cast.strength() * strength * spread
        } else if demon && matches!(cast, CastType::Bolt | CastType::Lob) && !harm {
            // A demonic meteor burns where it lands, fire or not.
            ctx.power.damage() * 0.9 * cast.strength() * strength * spread
        } else if wild && !harm && crate::wilds::roots_hold(cast) {
            // Roots crush what they coil round (Diablo IV's Poison Creeper strangles); the hold
            // is the point, so it is light.
            ctx.power.damage() * 0.15 * cast.strength() * strength * spread
        } else if wild && !harm && cast == CastType::Wall {
            // A hedge's thorns scratch on each tick (its damage is per tick, two a second).
            ctx.power.damage() * 0.035 * strength
        } else if spear && !harm {
            // A bone spear runs through whatever stands in its line, each body in turn: a
            // little less than a bolt that stops at the first.
            ctx.power.damage() * 0.75 * cast.strength() * strength * spread
        } else if bone && !harm && crate::bones::prison_holds(cast) {
            // The ribs' points bite as they close; the hold is the point.
            ctx.power.damage() * 0.2 * cast.strength() * strength * spread
        } else if bone && !harm && cast == CastType::Wall {
            // A ribcage's points snag and scratch what pushes through (per tick).
            ctx.power.damage() * 0.03 * strength
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
    // A hook of chains on a bolt drags what it bites back to the caster.
    let hook = chained && matches!(cast, CastType::Bolt | CastType::Lob);
    let force = if hook {
        force.map(|(_, i)| (ForceDir::Toward, i * 1.6))
    } else {
        force
    };
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
    // A grip's energy: what it can lift and how hard it throws (kinesis.rs).
    let gripping = has(&|k| matches!(k, EffectKind::Grip));
    let grip_j = ctx.power.energy_j as f32 * strength;
    let payload = Payload {
        damage,
        damage_kind: if blade && w.elements.is_empty() {
            DamageKind::Slash
        } else if demon && w.elements.is_empty() {
            DamageKind::Fire
        } else if (wild || bone) && w.elements.is_empty() {
            DamageKind::Pierce
        } else {
            damage_kind(&w)
        },
        statuses: if harm { statuses } else { Vec::new() },
        force,
        heal: if has(&|k| matches!(k, EffectKind::Heal)) {
            base * 1.2
        } else {
            0.0
        },
        ward: if warding { base * 2.0 } else { 0.0 },
        ward_element: if warding {
            w.elements.first().copied()
        } else {
            None
        },
        shield_secs: if warding && cast == CastType::Cone {
            (4.0 * duration_k).clamp(0.5, 30.0)
        } else {
            0.0
        },
        // Near 0.8 m across for an Adept's rock, 1 m for an Expert's, 1.5 m for an Ascended's.
        meteor_m: if meteor {
            (0.6 + 0.5 * size).clamp(0.7, 3.0) * size_k.sqrt()
        } else {
            0.0
        },
        fault,
        time,
        blink_m: if has(&|k| matches!(k, EffectKind::Blink { .. })) {
            (8.0 * size.sqrt() * range_k).clamp(3.0, 60.0)
        } else {
            0.0
        },
        portal_secs: if has(&|k| matches!(k, EffectKind::Portal)) {
            (20.0 * duration_k).clamp(2.0, 120.0)
        } else {
            0.0
        },
        portal_m: {
            // Size scales the opening, within reason: a door for a crate up to a gate for a cart.
            let k = size_k.sqrt().clamp(0.7, 2.0);
            [1.4 * k, 2.4 * k]
        },
        recall_secs: if has(&|k| matches!(k, EffectKind::Recall)) {
            (3.0 * duration_k).clamp(1.0, 6.0)
        } else {
            0.0
        },
        grip_kg: if gripping {
            crate::kinesis::heaviest_kg(grip_j)
        } else {
            0.0
        },
        throw_j: if gripping { 0.5 * grip_j } else { 0.0 },
        grip_secs: if !gripping {
            0.0
        } else if cast == CastType::Orbit {
            (8.0 * duration_k).clamp(2.0, 40.0)
        } else {
            (2.2 * duration_k).clamp(0.6, 12.0)
        },
        grip_m: if gripping {
            (12.0 * range_k).clamp(4.0, 40.0)
        } else {
            0.0
        },
        grip_damage: if gripping {
            ctx.power.damage() * strength
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
        // Mirror image: two copies for an Adept, three from Expert (the design's three),
        // four once ascended, one more per Split; a Decoy one per Split.
        phantasms: if !has(&|k| matches!(k, EffectKind::Phantasm)) {
            0
        } else {
            match cast {
                CastType::OnSelf => {
                    let base = match ctx.mastery {
                        Mastery::Novice | Mastery::Apprentice => 1,
                        Mastery::Adept => 2,
                        Mastery::Expert | Mastery::Master => 3,
                        _ => 4,
                    };
                    base + split.max(1) - 1
                }
                CastType::Summon => split.max(1),
                _ => 0,
            }
            .min(8)
        },
        phantasm_secs: if has(&|k| matches!(k, EffectKind::Phantasm)) {
            (if cast == CastType::Summon { 15.0 } else { 12.0 } * duration_k).clamp(3.0, 60.0)
        } else {
            0.0
        },
        // Command holds 8 s (12 from Master), Wrath 6 s.
        command_secs: if has(&|k| matches!(k, EffectKind::Command)) {
            (if ctx.mastery >= Mastery::Master {
                12.0
            } else {
                8.0
            } * duration_k)
                .clamp(2.0, 40.0)
        } else {
            0.0
        },
        wrath_secs: if has(&|k| matches!(k, EffectKind::Wrath)) {
            (6.0 * duration_k).clamp(2.0, 30.0)
        } else {
            0.0
        },
        blades: if blade
            && matches!(
                cast,
                CastType::Bolt
                    | CastType::Orbit
                    | CastType::Rain
                    | CastType::Summon
                    | CastType::OnSelf
            ) {
            match cast {
                // Two hands at most.
                CastType::OnSelf => split.clamp(1, 2),
                CastType::Summon => split.clamp(1, 6),
                _ => count.max(1),
            }
        } else {
            0
        },
        // About a metre for an Adept, a greatblade with Size.
        blade_m: if blade {
            ((0.95 + 0.06 * size) * size_k.sqrt()).clamp(0.8, 2.4)
        } else {
            0.0
        },
        blade_secs: if blade {
            match cast {
                CastType::Orbit | CastType::OnSelf => (30.0 * duration_k).clamp(3.0, 120.0),
                CastType::Summon => (20.0 * duration_k).clamp(3.0, 120.0),
                _ => 0.0,
            }
        } else {
            0.0
        },
        imbue_secs: if cast == CastType::Imbue {
            (30.0 * duration_k).clamp(5.0, 120.0)
        } else {
            0.0
        },
        // Chains out of the floor hold 3 s (a little more with power), more with Duration.
        bind_secs: if chained
            && matches!(
                cast,
                CastType::Area
                    | CastType::Rain
                    | CastType::Rune
                    | CastType::Wave
                    | CastType::Wall
                    | CastType::Cone
                    | CastType::Touch
                    | CastType::Summon
            ) {
            (3.0 * duration_k * (0.85 + 0.15 * size)).clamp(1.5, 12.0)
        } else {
            0.0
        },
        hook,
        // Fatal Bonds hold their bodies 12 s.
        bond_secs: if chained && cast == CastType::Chain {
            (12.0 * duration_k).clamp(3.0, 40.0)
        } else {
            0.0
        },
        // Three imps out of a meteor (Hand of Gul'dan's); a hellgate's two hounds, one more
        // per Split, before its imps.
        demons: if !demon {
            0
        } else {
            match cast {
                CastType::Bolt | CastType::Lob => 3,
                CastType::Summon => 1 + split.clamp(1, 4),
                _ => 0,
            }
        },
        demon_secs: if demon {
            (20.0 * duration_k).clamp(4.0, 90.0)
        } else {
            0.0
        },
        // Roots hold 3 s (a little more with power), more with Duration; a treant's slam
        // holds as long.
        roots_secs: if wild && (crate::wilds::roots_hold(cast) || cast == CastType::Summon) {
            (3.0 * duration_k * (0.85 + 0.15 * size)).clamp(1.5, 12.0)
        } else {
            0.0
        },
        hedge: wild && cast == CastType::Wall,
        bark_secs: if wild && cast == CastType::OnSelf {
            (20.0 * duration_k).clamp(4.0, 90.0)
        } else {
            0.0
        },
        // A prison holds 4 s (a little more with power), more with Duration.
        prison_secs: if bone && crate::bones::prison_holds(cast) {
            (4.0 * duration_k * (0.85 + 0.15 * size)).clamp(1.5, 15.0)
        } else {
            0.0
        },
        spear,
        ribcage: bone && cast == CastType::Wall,
        armour_secs: if bone && cast == CastType::OnSelf {
            (20.0 * duration_k).clamp(4.0, 90.0)
        } else {
            0.0
        },
        bone_dragon: bone && cast == CastType::Summon,
        light_lm: if has(&|k| matches!(k, EffectKind::Light)) {
            4000.0 * size
        } else {
            0.0
        },
        ignite_c: if fire && !warding {
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
        raise: if has(&|k| matches!(k, EffectKind::Raise)) {
            1.0
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
    // A living-wood bolt or lob is a seed: where it lands it bursts into roots that hold what
    // stands within SEED_M (wilds.rs).
    if wild && matches!(cast, CastType::Bolt | CastType::Lob) {
        let seed = SpellDesign {
            name: format!("{} roots", w.name),
            effects: design.effects.clone(),
            cast: CastType::Area,
            modifiers: vec![Modifier::Size {
                factor: crate::wilds::SEED_M / 3.0,
            }],
            stages: Vec::new(),
        };
        let mut s = compile_inner(&seed, ctx, complexity, budget, 0.6 * strength);
        s.carrier.duration = 0.0;
        stages.push((Trigger::Impact, Arc::new(s)));
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
        // A conjured blade is named for its shape (blades.rs): a fire glintblade, a frost
        // phalanx, a bladefall, a spirit blade, a bound blade.
        let made_of = w
            .name
            .strip_suffix(" blade")
            .filter(|_| !w.elements.is_empty());
        let shape = match cast {
            CastType::Bolt => Some("glintblade"),
            CastType::Orbit => Some("phalanx"),
            CastType::Rain => Some("bladefall"),
            CastType::Summon => Some("spirit blade"),
            CastType::OnSelf => Some("blade"),
            _ => None,
        };
        match (blade, shape, made_of) {
            (true, Some(sh), Some(m)) if cast == CastType::OnSelf => {
                format!("Bound {} {sh}", m.to_lowercase())
            }
            (true, Some(sh), Some(m)) => format!("{m} {sh}"),
            (true, Some(_), None) if cast == CastType::OnSelf => "Bound blade".to_string(),
            (true, Some(sh), None) if cast == CastType::Orbit => format!("Glintblade {sh}"),
            (true, Some(sh), None) => {
                let mut c = sh.chars();
                c.next()
                    .map(|f| f.to_uppercase().collect::<String>() + c.as_str())
                    .unwrap_or_default()
            }
            _ => format!("{} {}", w.name, cast.name().to_lowercase()),
        }
    } else {
        design.name.clone()
    };
    let shield = payload.shield_secs > 0.0;
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
        // A shield comes up in 40% of the time: a block is a reaction.
        cast_secs: ctx.mastery.cast_secs()
            * (1.0 + 0.04 * complexity as f32)
            * if shield { 0.4 } else { 1.0 },
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
