//! Casters: mana, mastery per subclass (levelled by use, Skyrim style), corruption, the grimoire,
//! and the cast in progress.
//!
//! Anyone with a [`MagicCaster`] can cast: the player, an NPC mage, a turret. A cast is asked for
//! with a [`CastRequest`] (or `magic.cast`): the caster gathers it for the spell's cast time
//! (longer while charging), pays, and releases it, or fizzles. Each cast trains the subclass it
//! belongs to; [`MasteryGained`] tells UI and AI when a tier opens.

use std::collections::BTreeMap;
use std::sync::Arc;

use engine_core::math::{DVec3, Vec3};
use engine_core::prelude::*;

use crate::compile::{CastContext, CompileError, Spell, compile};
use crate::design::SpellDesign;
use crate::effects::effect;
use crate::growth::{Growth, grow};
use crate::schools::{Mastery, Power, Subclass};

/// Skill in one subclass.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Skill {
    /// 0..100 (Skyrim's scale).
    pub level: f32,
    /// Ascensions past 100 (0..3).
    pub ascensions: u32,
    /// Experience toward the next level.
    pub xp: f32,
}

impl Default for Skill {
    fn default() -> Self {
        Self {
            level: 5.0,
            ascensions: 0,
            xp: 0.0,
        }
    }
}

impl Skill {
    pub fn at(m: Mastery) -> Skill {
        Skill {
            level: m.skill(),
            ascensions: m.ascensions(),
            xp: 0.0,
        }
    }
}

/// A spell in the grimoire. With a growth path, it grows with the caster's mastery.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct KnownSpell {
    pub design: SpellDesign,
    pub growth: Option<Growth>,
}

/// Someone who can cast. Tunable and inspectable as JSON (`magic.caster`).
#[derive(Component, Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct MagicCaster {
    pub mana: f32,
    pub max_mana: f32,
    /// Mana regained per second.
    pub mana_regen: f32,
    /// Character level (1..100): sets power, from 1 kJ at 1 to 100 MJ at 100.
    pub level: f32,
    /// Multiplier on power (gear, buffs, a god's extra orders of magnitude).
    pub power_scale: f64,
    /// Skill per subclass (missing: Novice).
    pub skills: BTreeMap<Subclass, Skill>,
    /// Every skill ascended: God tier.
    pub god: bool,
    /// Built by dark magic; shows on the body and changes how others treat you. 0..100.
    pub corruption: f32,
    pub grimoire: Vec<KnownSpell>,
    /// Which grimoire spell a plain cast uses.
    pub selected: usize,
    /// Which side the caster is on: safeguard and the enemies/allies filters spare its own side.
    pub side: u32,
    /// Where the hand is, in the caster's local frame (x right, y up, −z forward), m.
    pub hand: Vec3,
    /// Gains skill from casting.
    pub learns: bool,
    /// Seeds the fizzle and backfire rolls.
    pub seed: u64,
}

impl Default for MagicCaster {
    fn default() -> Self {
        Self {
            mana: 100.0,
            max_mana: 100.0,
            mana_regen: 4.0,
            level: 10.0,
            power_scale: 1.0,
            skills: BTreeMap::new(),
            god: false,
            corruption: 0.0,
            grimoire: Vec::new(),
            selected: 0,
            side: 0,
            hand: Vec3::new(0.25, 1.35, -0.45),
            learns: true,
            seed: 1,
        }
    }
}

impl MagicCaster {
    /// A caster of `level` with every subclass at `mastery` and a mana pool to match.
    pub fn new(level: f32, mastery: Mastery) -> Self {
        let mut c = MagicCaster {
            level,
            max_mana: 80.0 + 2.2 * level,
            mana_regen: 3.0 + 0.08 * level,
            ..Default::default()
        };
        c.mana = c.max_mana;
        for s in Subclass::ALL {
            c.skills.insert(s, Skill::at(mastery));
        }
        c.god = mastery == Mastery::God;
        c
    }

    pub fn learn(mut self, design: SpellDesign, growth: Option<Growth>) -> Self {
        self.grimoire.push(KnownSpell { design, growth });
        self
    }

    pub fn with_seed(mut self, seed: u64) -> Self {
        self.seed = seed;
        self
    }

    pub fn power(&self) -> Power {
        Power::joules(Power::at_level(self.level).energy_j * self.power_scale.max(1e-6))
    }

    pub fn mastery(&self, s: Subclass) -> Mastery {
        let skill = self.skills.get(&s).copied().unwrap_or_default();
        Mastery::from_skill(skill.level, skill.ascensions, self.god)
    }

    /// The subclass a design trains and draws its budget from: its first effect's.
    pub fn subclass_of(design: &SpellDesign) -> Subclass {
        design
            .effects
            .first()
            .and_then(|k| effect(k))
            .map(|e| e.subclass)
            .unwrap_or(Subclass::Evocation)
    }

    /// The design of grimoire spell `i` as it stands at the caster's current mastery.
    pub fn grown(&self, i: usize) -> Option<SpellDesign> {
        let known = self.grimoire.get(i)?;
        let m = self.mastery(Self::subclass_of(&known.design));
        Some(match known.growth {
            Some(path) => grow(&known.design, path, m),
            None => known.design.clone(),
        })
    }

    /// Compile a design for this caster at `charge` (0..1: half power uncharged, double at a full
    /// charge for spells with a Charge modifier).
    pub fn compile(&self, design: &SpellDesign, charge: f32) -> Result<Arc<Spell>, CompileError> {
        let sub = Self::subclass_of(design);
        let mastery = self.mastery(sub);
        // Every other subclass in the spell must be at least Apprentice.
        for k in &design.all_effects() {
            if let Some(e) = effect(k)
                && e.subclass != sub
                && self.mastery(e.subclass) < Mastery::Apprentice
                && k != "push"
            {
                return Err(CompileError(vec![format!(
                    "{} needs Apprentice in {} for its {} part.",
                    if design.name.is_empty() {
                        "This spell"
                    } else {
                        &design.name
                    },
                    e.subclass.name(),
                    e.name
                )]));
            }
        }
        let own = self.power();
        let power = Power::joules(own.energy_j * (1.0 + charge.clamp(0.0, 1.0)) as f64);
        compile(
            design,
            CastContext {
                mastery,
                power,
                own_power: own,
            },
        )
    }

    /// Train the subclass after a cast: more for complex spells and for hits. Returns the new
    /// tier when one opens.
    pub fn train(&mut self, sub: Subclass, complexity: u32, hit: bool) -> Option<Mastery> {
        if !self.learns {
            return None;
        }
        let before = self.mastery(sub);
        let skill = self.skills.entry(sub).or_default();
        if skill.level >= 100.0 {
            return None;
        }
        // Skyrim-like: each use gives experience; levels need more as they rise.
        skill.xp += (1.0 + complexity as f32) * if hit { 1.0 } else { 0.4 };
        loop {
            let need = 3.0 + skill.level * 0.35;
            if skill.xp < need || skill.level >= 100.0 {
                break;
            }
            skill.xp -= need;
            skill.level = (skill.level + 1.0).min(100.0);
        }
        let after = self.mastery(sub);
        (after > before).then_some(after)
    }
}

/// Health for things magic damages that no other module keeps health for (practice dummies,
/// casters paying a blood price, lifesteal). A combat module's own health wins where it exists;
/// this reads the same `DamageEvent`s.
#[derive(Component, Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Vitality {
    pub health: f32,
    pub max: f32,
    /// Health regained per second.
    pub regen: f32,
    /// Seconds after reaching 0 before it is back at full (practice dummies); 0 = stays down.
    pub revive_secs: f32,
    /// Seconds spent down so far.
    pub down_secs: f32,
}

impl Default for Vitality {
    fn default() -> Self {
        Self {
            health: 100.0,
            max: 100.0,
            regen: 0.0,
            revive_secs: 0.0,
            down_secs: 0.0,
        }
    }
}

impl Vitality {
    pub fn new(max: f32) -> Self {
        Self {
            health: max,
            max,
            ..Default::default()
        }
    }

    pub fn down(&self) -> bool {
        self.health <= 0.0
    }
}

/// Where a cast is aimed.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Aim {
    /// Along a direction from the hand (unit vector, world).
    Direction(Vec3),
    /// At a point (world).
    Point(DVec3),
    /// At an entity (followed while the cast gathers).
    Entity(Entity),
}

/// Which spell to cast.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SpellChoice {
    /// The caster's selected grimoire spell.
    Selected,
    /// Grimoire slot.
    Slot(usize),
    /// A design given outright (the spellcrafter's preview, a scroll, a script).
    Design(SpellDesign),
}

/// Ask a caster to cast. Gathering takes the spell's cast time, then it releases (or fizzles).
#[derive(Message, Debug, Clone)]
pub struct CastRequest {
    pub caster: Entity,
    pub spell: SpellChoice,
    pub aim: Aim,
    /// Charge 0..1 (spells with a Charge modifier); a held button would raise it.
    pub charge: f32,
}

/// A subclass reached a new mastery tier.
#[derive(Message, Debug, Clone, Copy)]
pub struct MasteryGained {
    pub caster: Entity,
    pub subclass: Subclass,
    pub mastery: Mastery,
}

/// A cast was released, fizzled or backfired: for UI, audio and AI.
#[derive(Message, Debug, Clone)]
pub struct SpellCast {
    pub caster: Entity,
    pub spell: Arc<Spell>,
    pub outcome: CastOutcome,
    pub at: DVec3,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CastOutcome {
    Released,
    Fizzled,
    Backfired,
    /// Not enough mana (or health for a blood price).
    Spent,
}

/// The cast in progress on a caster.
#[derive(Component, Debug, Clone)]
pub struct Casting {
    pub spell: Arc<Spell>,
    pub aim: Aim,
    /// Seconds gathered so far (caster time).
    pub t: f32,
    /// Seconds it takes to release.
    pub release_at: f32,
    /// Recast pending from Echo (seconds until it fires).
    pub echo_in: Option<f32>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::design::{CastType, Modifier};

    #[test]
    fn casting_trains_until_a_tier_opens() {
        let mut c = MagicCaster::new(10.0, Mastery::Novice);
        assert_eq!(c.mastery(Subclass::Evocation), Mastery::Novice);
        let mut opened = None;
        for _ in 0..200 {
            if let Some(m) = c.train(Subclass::Evocation, 1, true) {
                opened = Some(m);
                break;
            }
        }
        assert_eq!(opened, Some(Mastery::Apprentice));
        assert_eq!(c.mastery(Subclass::Malefice), Mastery::Novice);
    }

    #[test]
    fn known_spells_grow_with_mastery_and_need_their_parts() {
        let bolt = SpellDesign::new(&["fire"], CastType::Bolt).named("Fire bolt");
        let novice =
            MagicCaster::new(5.0, Mastery::Novice).learn(bolt.clone(), Some(Growth::Volley));
        let master = MagicCaster::new(100.0, Mastery::Master).learn(bolt, Some(Growth::Volley));
        assert!(novice.grown(0).unwrap().modifiers.is_empty());
        let m = master.grown(0).unwrap();
        assert!(m.has(|x| matches!(x, Modifier::Split { count: 5 })));
        assert!(master.compile(&m, 0.0).is_ok());
        assert!(novice.compile(&m, 0.0).is_err());
        // A storm part needs Apprentice in Evocation for a blood caster.
        let mut c = MagicCaster::new(40.0, Mastery::Novice);
        c.skills
            .insert(Subclass::Malefice, Skill::at(Mastery::Adept));
        let mixed = SpellDesign::new(&["blood", "storm"], CastType::Bolt);
        assert!(c.compile(&mixed, 0.0).is_err());
        c.skills
            .insert(Subclass::Evocation, Skill::at(Mastery::Apprentice));
        assert!(c.compile(&mixed, 0.0).is_ok());
    }
}
