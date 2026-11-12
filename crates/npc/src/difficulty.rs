//! Difficulty changes tactics, not just numbers (engine-design/npc-difficulty.md 4.1, 4.2).
//! The world's [`CombatDifficulty`] tier (per faction, if overridden) decides which named
//! [`Tactic`]s an NPC may use at all; its own skills decide whether it is good enough to (a
//! rookie archer on Hard takes cover but rarely lobs, a master lobs).
//!
//! The archer ladder, against a foe hiding behind cover (`besiege.rs`):
//!
//! | Tactic | Tier | Skill | What it does |
//! |---|---|---|---|
//! | [`Tactic::CloseOnHidden`] | Easy | | walks up to the cover for a clear shot |
//! | [`Tactic::HarassEdge`] | Medium | logic 0.3 | stands in the open and shoots at the cover's edge now and then |
//! | [`Tactic::CoverAndHold`] | Hard | | takes cover with a line to where the foe would show, draws and holds, looses when it shows |
//! | [`Tactic::LobOver`] | Hard | accuracy 0.6 | lobs an arrow over the cover onto the hiding spot |
//! | [`Tactic::CallFlusher`] | Very hard | teamwork 0.5 | calls a hand-to-hand squad mate round the cover to flush the foe, and holds on the side it will break to |
//! | [`Tactic::Volley`] | Very hard | teamwork 0.5 | the squad's archers loose together |
//! | [`Tactic::CoordinatedPush`] | Extreme | logic 0.7 | the flusher waits out of sight until the holders are drawn, then goes, and pulls back to try again if it fails |
//!
//! [`Tokens`] cap how many of a squad act on one foe at once, by tier (Doom's attack tokens):
//! hand-to-hand attackers 1 / 2 / 2 / 3 / 3, shooters loosing at it 2 / 3 / 4 / 5 / 6 and
//! flushers sent round its cover 0 / 0 / 0 / 1 / 2.

use engine_world::{CombatDifficulty, DifficultyTier};

use crate::faction::Faction;
use crate::skills::NpcSkills;

/// A named tactic the difficulty tier gates (see the module docs).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Tactic {
    CloseOnHidden,
    HarassEdge,
    CoverAndHold,
    LobOver,
    CallFlusher,
    Volley,
    CoordinatedPush,
}

impl Tactic {
    pub const ALL: [Tactic; 7] = [
        Tactic::CloseOnHidden,
        Tactic::HarassEdge,
        Tactic::CoverAndHold,
        Tactic::LobOver,
        Tactic::CallFlusher,
        Tactic::Volley,
        Tactic::CoordinatedPush,
    ];

    pub fn name(self) -> &'static str {
        match self {
            Tactic::CloseOnHidden => "close_on_hidden",
            Tactic::HarassEdge => "harass_edge",
            Tactic::CoverAndHold => "cover_and_hold",
            Tactic::LobOver => "lob_over",
            Tactic::CallFlusher => "call_flusher",
            Tactic::Volley => "volley",
            Tactic::CoordinatedPush => "coordinated_push",
        }
    }

    /// The lowest tier that has it.
    pub fn min_tier(self) -> DifficultyTier {
        match self {
            Tactic::CloseOnHidden => DifficultyTier::Easy,
            Tactic::HarassEdge => DifficultyTier::Medium,
            Tactic::CoverAndHold | Tactic::LobOver => DifficultyTier::Hard,
            Tactic::CallFlusher | Tactic::Volley => DifficultyTier::VeryHard,
            Tactic::CoordinatedPush => DifficultyTier::Extreme,
        }
    }

    /// Whether `skills` are good enough for it.
    pub fn skilled(self, skills: &NpcSkills) -> bool {
        match self {
            // Too dull to stand off and time its shots, a shooter rushes in (`brain`'s
            // `RUSH_LOGIC`).
            Tactic::HarassEdge => skills.logic >= 0.3,
            Tactic::LobOver => skills.accuracy >= 0.6,
            Tactic::CallFlusher | Tactic::Volley => skills.teamwork >= 0.5,
            Tactic::CoordinatedPush => skills.logic >= 0.7,
            _ => true,
        }
    }

    /// Whether an NPC with `skills` may use it at `tier`.
    pub fn allowed(self, tier: DifficultyTier, skills: &NpcSkills) -> bool {
        tier >= self.min_tier() && self.skilled(skills)
    }
}

/// How many of a squad may act on one foe at once (see the module docs).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Tokens {
    /// Hand-to-hand attackers at a time.
    pub melee: u32,
    /// Shooters loosing at it at a time.
    pub shooters: u32,
    /// Hand-to-hand mates sent round its cover to flush it out.
    pub flushers: u32,
}

impl Tokens {
    pub fn for_tier(tier: DifficultyTier) -> Self {
        let (melee, shooters, flushers) = match tier {
            DifficultyTier::Easy => (1, 2, 0),
            DifficultyTier::Medium => (2, 3, 0),
            DifficultyTier::Hard => (2, 4, 0),
            DifficultyTier::VeryHard => (3, 5, 1),
            DifficultyTier::Extreme => (3, 6, 2),
        };
        Self {
            melee,
            shooters,
            flushers,
        }
    }
}

/// The tier `faction` fights at.
pub fn tier_of(difficulty: &CombatDifficulty, faction: Faction) -> DifficultyTier {
    difficulty.for_faction(faction.0).0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_tier_adds_tactics_and_skill_gates_the_hard_ones() {
        let master = NpcSkills::master();
        let rookie = NpcSkills::rookie();
        let count = |tier, s: &NpcSkills| Tactic::ALL.iter().filter(|t| t.allowed(tier, s)).count();
        use DifficultyTier::*;
        let ladder: Vec<usize> = [Easy, Medium, Hard, VeryHard, Extreme]
            .iter()
            .map(|&t| count(t, &master))
            .collect();
        assert_eq!(ladder, [1, 2, 4, 6, 7]);
        assert!(
            !Tactic::LobOver.allowed(Hard, &rookie),
            "a rookie doesn't lob"
        );
        assert!(
            Tactic::CoverAndHold.allowed(Hard, &rookie),
            "but it takes cover"
        );
        assert!(
            !Tactic::HarassEdge.allowed(Medium, &rookie),
            "and rushes in on Medium"
        );
        let mut d = CombatDifficulty::default();
        d.factions.insert(2, (Extreme, None));
        assert_eq!(tier_of(&d, Faction(2)), Extreme);
        assert_eq!(tier_of(&d, Faction(1)), Medium);
    }
}
