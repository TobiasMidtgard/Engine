//! The player-facing side of magic: six elements, four classes with a clean and a dark
//! subclass each, their branches, and mastery tiers with their complexity budgets
//! (engine-design/spell-combination/magic-system.md, sections 1 to 5).
//!
//! Elements and effects are presets of the seven [`Dials`](crate::dials::Dials), so a
//! subclass never needs its own code: Fire is Energy turned to heat, Blood is Life withered and
//! drawn near (to the caster), and so on.

use serde::{Deserialize, Serialize};

use crate::dials::{Dials, Pole};

/// The six damage elements: three clean (Evocation) and three dark (Malefice).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Element {
    /// Burns and spreads.
    Fire,
    /// Slows and builds up to a freeze.
    Frost,
    /// Stuns and chains.
    Storm,
    /// Drains and bleeds (dark).
    Blood,
    /// Rots, poisons and blocks healing (dark).
    Blight,
    /// Terrifies, blinds and eats light (dark).
    Shadow,
}

impl Element {
    pub const ALL: [Element; 6] = [
        Element::Fire,
        Element::Frost,
        Element::Storm,
        Element::Blood,
        Element::Blight,
        Element::Shadow,
    ];
    pub const CLEAN: [Element; 3] = [Element::Fire, Element::Frost, Element::Storm];
    pub const DARK: [Element; 3] = [Element::Blood, Element::Blight, Element::Shadow];

    pub fn is_dark(self) -> bool {
        matches!(self, Element::Blood | Element::Blight | Element::Shadow)
    }

    pub fn key(self) -> &'static str {
        match self {
            Element::Fire => "fire",
            Element::Frost => "frost",
            Element::Storm => "storm",
            Element::Blood => "blood",
            Element::Blight => "blight",
            Element::Shadow => "shadow",
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Element::Fire => "Fire",
            Element::Frost => "Frost",
            Element::Storm => "Storm",
            Element::Blood => "Blood",
            Element::Blight => "Blight",
            Element::Shadow => "Shadow",
        }
    }

    pub fn from_key(key: &str) -> Option<Element> {
        Element::ALL.into_iter().find(|e| e.key() == key)
    }

    /// The element as the seven dials, per unit of power.
    pub fn dials(self) -> Dials {
        match self {
            Element::Fire => Dials::pole(Pole::Heat, 1.0),
            Element::Frost => Dials::pole(Pole::Cold, 1.0),
            // Lightning is charged force (the Energy + Force weave).
            Element::Storm => Dials::pole(Pole::Heat, 0.6) + Dials::pole(Pole::Push, 0.6),
            // Life withered and drawn near: it leaves them and comes to you.
            Element::Blood => Dials::pole(Pole::Wither, 0.6) + Dials::pole(Pole::Near, 0.5),
            Element::Blight => Dials::pole(Pole::Wither, 1.0) + Dials::pole(Pole::Unform, 0.4),
            Element::Shadow => Dials::pole(Pole::Veil, 0.9) + Dials::pole(Pole::Cold, 0.3),
        }
    }

    /// What a hit of this element leaves on the victim.
    pub fn status(self) -> Status {
        match self {
            Element::Fire => Status::Burning,
            Element::Frost => Status::Chilled,
            Element::Storm => Status::Shocked,
            Element::Blood => Status::Bleeding,
            Element::Blight => Status::Rot,
            Element::Shadow => Status::Dread,
        }
    }

    /// The signature color, linear RGB (lights and stand-in looks).
    pub fn color(self) -> [f32; 3] {
        match self {
            Element::Fire => [1.0, 0.42, 0.1],
            Element::Frost => [0.45, 0.75, 1.0],
            Element::Storm => [0.62, 0.7, 1.0],
            Element::Blood => [0.75, 0.03, 0.04],
            Element::Blight => [0.5, 0.72, 0.1],
            Element::Shadow => [0.32, 0.1, 0.55],
        }
    }
}

/// What a spell leaves on whoever it hits, tracked per victim (status.rs). Each element has
/// one; mixed statuses on one victim react through the combination table.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Status {
    /// Takes fire damage every tick and lights what it touches.
    Burning,
    /// Slowed 10% per stack; at the freeze threshold (5 stacks) becomes [`Status::Frozen`].
    Chilled,
    /// Can't move or act; the next heavy hit shatters it for bonus damage.
    Frozen,
    /// Stunned for a moment; storm chains jump to shocked and wet targets first.
    Shocked,
    /// Loses health every tick, more while moving.
    Bleeding,
    /// Loses health every tick and can't be healed.
    Rot,
    /// Terrified: flees and fights badly.
    Dread,
}

impl Status {
    pub const ALL: [Status; 7] = [
        Status::Burning,
        Status::Chilled,
        Status::Frozen,
        Status::Shocked,
        Status::Bleeding,
        Status::Rot,
        Status::Dread,
    ];

    /// The element whose hits apply this status (Frozen counts as Frost).
    pub fn element(self) -> Element {
        match self {
            Status::Burning => Element::Fire,
            Status::Chilled | Status::Frozen => Element::Frost,
            Status::Shocked => Element::Storm,
            Status::Bleeding => Element::Blood,
            Status::Rot => Element::Blight,
            Status::Dread => Element::Shadow,
        }
    }

    pub fn key(self) -> &'static str {
        match self {
            Status::Burning => "burning",
            Status::Chilled => "chilled",
            Status::Frozen => "frozen",
            Status::Shocked => "shocked",
            Status::Bleeding => "bleeding",
            Status::Rot => "rot",
            Status::Dread => "dread",
        }
    }
}

/// The four classes of magic, each with a clean and a dark subclass.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Class {
    /// The elements.
    Destructor,
    /// Life.
    Channeler,
    /// Matter, force and space.
    Transmuter,
    /// The mind.
    Mesmer,
}

/// A school of magic: a skill levelled by use to 100, then ascended.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Subclass {
    Evocation,
    Malefice,
    Vitalism,
    Necromancy,
    Shaping,
    Pactbinding,
    Illusion,
    Domination,
}

impl Subclass {
    pub const ALL: [Subclass; 8] = [
        Subclass::Evocation,
        Subclass::Malefice,
        Subclass::Vitalism,
        Subclass::Necromancy,
        Subclass::Shaping,
        Subclass::Pactbinding,
        Subclass::Illusion,
        Subclass::Domination,
    ];

    pub fn class(self) -> Class {
        match self {
            Subclass::Evocation | Subclass::Malefice => Class::Destructor,
            Subclass::Vitalism | Subclass::Necromancy => Class::Channeler,
            Subclass::Shaping | Subclass::Pactbinding => Class::Transmuter,
            Subclass::Illusion | Subclass::Domination => Class::Mesmer,
        }
    }

    pub fn is_dark(self) -> bool {
        matches!(
            self,
            Subclass::Malefice
                | Subclass::Necromancy
                | Subclass::Pactbinding
                | Subclass::Domination
        )
    }

    pub fn name(self) -> &'static str {
        match self {
            Subclass::Evocation => "Evocation",
            Subclass::Malefice => "Malefice",
            Subclass::Vitalism => "Vitalism",
            Subclass::Necromancy => "Necromancy",
            Subclass::Shaping => "Shaping",
            Subclass::Pactbinding => "Pactbinding",
            Subclass::Illusion => "Illusion",
            Subclass::Domination => "Domination",
        }
    }

    /// Its four branches (mastery trees).
    pub fn branches(self) -> [&'static str; 4] {
        match self {
            Subclass::Evocation => ["Fire", "Frost", "Storm", "Spellblade"],
            Subclass::Malefice => ["Blood", "Blight", "Shadow", "Hexblade"],
            Subclass::Vitalism => ["Restoration", "Warding", "Wilds", "Herbalism"],
            Subclass::Necromancy => ["Undeath", "Afflictions", "Soul", "Sacrifice"],
            Subclass::Shaping => ["Kinesis", "Passage", "Conjuration", "Artifice"],
            Subclass::Pactbinding => ["Infernal", "Chains", "Pact Arms", "Gates"],
            Subclass::Illusion => ["Veil", "Phantasm", "Insight", "Glamour"],
            Subclass::Domination => ["Fear", "Wrath", "Command", "Memory"],
        }
    }

    /// The subclass that owns an element.
    pub fn of_element(e: Element) -> Subclass {
        if e.is_dark() {
            Subclass::Malefice
        } else {
            Subclass::Evocation
        }
    }
}

/// How far a caster has come in one subclass. Sets the complexity budget: how much a spell of
/// that subclass may hold (magic-system.md section 5).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Mastery {
    /// The ordinary person: one small effect, slow, fizzles a third of the time.
    Novice,
    Apprentice,
    Adept,
    Expert,
    /// Skill 100.
    Master,
    Ascended1,
    Ascended2,
    Ascended3,
    /// Every skill ascended.
    God,
}

impl Mastery {
    pub const ALL: [Mastery; 9] = [
        Mastery::Novice,
        Mastery::Apprentice,
        Mastery::Adept,
        Mastery::Expert,
        Mastery::Master,
        Mastery::Ascended1,
        Mastery::Ascended2,
        Mastery::Ascended3,
        Mastery::God,
    ];

    /// Complexity a spell of this tier may hold.
    pub fn budget(self) -> u32 {
        match self {
            Mastery::Novice => 1,
            Mastery::Apprentice => 2,
            Mastery::Adept => 5,
            Mastery::Expert => 8,
            Mastery::Master => 12,
            Mastery::Ascended1 => 16,
            Mastery::Ascended2 => 20,
            Mastery::Ascended3 => 25,
            Mastery::God => 40,
        }
    }

    /// The tier for a skill level (0..100, Skyrim's 25-point steps) and ascensions (0..3).
    pub fn from_skill(skill: f32, ascensions: u32, god: bool) -> Mastery {
        if god {
            return Mastery::God;
        }
        match ascensions {
            0 => match skill {
                s if s >= 100.0 => Mastery::Master,
                s if s >= 75.0 => Mastery::Expert,
                s if s >= 50.0 => Mastery::Adept,
                s if s >= 25.0 => Mastery::Apprentice,
                _ => Mastery::Novice,
            },
            1 => Mastery::Ascended1,
            2 => Mastery::Ascended2,
            _ => Mastery::Ascended3,
        }
    }

    /// The skill level a tier starts at (for demos and tests).
    pub fn skill(self) -> f32 {
        match self {
            Mastery::Novice => 5.0,
            Mastery::Apprentice => 25.0,
            Mastery::Adept => 50.0,
            Mastery::Expert => 75.0,
            _ => 100.0,
        }
    }

    pub fn ascensions(self) -> u32 {
        match self {
            Mastery::Ascended1 => 1,
            Mastery::Ascended2 => 2,
            Mastery::Ascended3 | Mastery::God => 3,
            _ => 0,
        }
    }

    /// Chance a spell well inside the budget fizzles (Novice: a third of the time).
    pub fn base_fizzle(self) -> f32 {
        match self {
            Mastery::Novice => 0.33,
            Mastery::Apprentice => 0.15,
            Mastery::Adept => 0.06,
            Mastery::Expert => 0.03,
            Mastery::Master => 0.01,
            _ => 0.0,
        }
    }

    /// Seconds to gather a spell before release; faster with mastery.
    pub fn cast_secs(self) -> f32 {
        match self {
            Mastery::Novice => 0.9,
            Mastery::Apprentice => 0.7,
            Mastery::Adept => 0.55,
            Mastery::Expert => 0.45,
            Mastery::Master => 0.38,
            Mastery::Ascended1 | Mastery::Ascended2 | Mastery::Ascended3 => 0.32,
            Mastery::God => 0.25,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Mastery::Novice => "Novice",
            Mastery::Apprentice => "Apprentice",
            Mastery::Adept => "Adept",
            Mastery::Expert => "Expert",
            Mastery::Master => "Master",
            Mastery::Ascended1 => "Ascended I",
            Mastery::Ascended2 => "Ascended II",
            Mastery::Ascended3 => "Ascended III",
            Mastery::God => "God",
        }
    }
}

/// Power: how much energy goes into a spell, on a log scale from a spark to a sun
/// (spell-combination.md section 7). Separate from mastery: a powerful novice throws one big
/// plain fireball, a weak master five small exploding ones.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Power {
    /// Energy per cast, joules (1 kJ an ordinary person, 1 MJ level 50, 100 MJ level 100,
    /// 10 GJ ascended, 1 TJ a god).
    pub energy_j: f64,
}

impl Default for Power {
    fn default() -> Self {
        Self { energy_j: 2.0e5 }
    }
}

impl Power {
    /// Energy of an ordinary person's cast, joules.
    pub const ORDINARY_J: f64 = 1.0e3;

    pub fn joules(energy_j: f64) -> Power {
        Power {
            energy_j: energy_j.max(1.0),
        }
    }

    /// The power of a caster at character `level` (1..100), then up through ascension:
    /// 1 kJ at level 1, 1 MJ at 50, 100 MJ at 100.
    pub fn at_level(level: f32) -> Power {
        let l = level.clamp(1.0, 100.0) as f64;
        // log10(E) from 3 at level 1, through 6 at 50, to 8 at 100.
        let log = if l <= 50.0 {
            3.0 + 3.0 * (l - 1.0) / 49.0
        } else {
            6.0 + 2.0 * (l - 50.0) / 50.0
        };
        Power::joules(10f64.powf(log))
    }

    /// Orders of magnitude above an ordinary person (0 at 1 kJ, 3 at 1 MJ, 9 at 1 TJ).
    pub fn magnitude(self) -> f32 {
        (self.energy_j / Self::ORDINARY_J).max(1.0).log10() as f32
    }

    /// Health damage one full hit of this power does: 5 at 1 kJ (singes a rat), about 110 at
    /// 1 MJ (drops a soldier), about 900 at 100 MJ. Grows as energy^0.45, so it spans gods
    /// without making a novice useless.
    pub fn damage(self) -> f32 {
        5.0 * ((self.energy_j / Self::ORDINARY_J).max(0.0) as f32).powf(0.45)
    }

    /// Size multiplier on shapes (radius, blast), from the cube root of energy: a 1 MJ fireball
    /// is the reference (1.0); a novice's is about a third of it.
    pub fn size(self) -> f32 {
        ((self.energy_j / 1.0e6) as f32).cbrt().clamp(0.25, 12.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn budgets_follow_the_design() {
        let budgets: Vec<u32> = Mastery::ALL.iter().map(|m| m.budget()).collect();
        assert_eq!(budgets, vec![1, 2, 5, 8, 12, 16, 20, 25, 40]);
        assert_eq!(Mastery::from_skill(10.0, 0, false), Mastery::Novice);
        assert_eq!(Mastery::from_skill(60.0, 0, false), Mastery::Adept);
        assert_eq!(Mastery::from_skill(100.0, 0, false), Mastery::Master);
        assert_eq!(Mastery::from_skill(100.0, 2, false), Mastery::Ascended2);
        for m in Mastery::ALL {
            if m < Mastery::Ascended1 {
                assert_eq!(Mastery::from_skill(m.skill(), 0, false), m);
            }
        }
    }

    #[test]
    fn power_spans_a_spark_to_a_sun() {
        let p1 = Power::at_level(1.0);
        let p50 = Power::at_level(50.0);
        let p100 = Power::at_level(100.0);
        assert!((p1.energy_j - 1.0e3).abs() < 1.0);
        assert!((p50.energy_j / 1.0e6 - 1.0).abs() < 1e-6);
        assert!((p100.energy_j / 1.0e8 - 1.0).abs() < 1e-6);
        // Singes the rat; drops a soldier; a god's spell is in another league.
        assert!((p1.damage() - 5.0).abs() < 0.01);
        assert!(p50.damage() > 90.0 && p50.damage() < 130.0);
        assert!(Power::joules(1e12).damage() > 40_000.0);
        assert!(p1.size() < p50.size() && p50.size() < p100.size());
    }

    #[test]
    fn every_element_has_a_status_that_maps_back() {
        for e in Element::ALL {
            assert_eq!(e.status().element(), e);
            assert_eq!(Element::from_key(e.key()), Some(e));
            assert!(e.dials().magnitude() > 0.5);
            assert_eq!(Subclass::of_element(e).is_dark(), e.is_dark());
        }
    }
}
