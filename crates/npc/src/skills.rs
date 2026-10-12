//! What an NPC is good at, and what it can sense. Every number here is meant to be tuned by
//! designers, data files and AI agents (`npc.set_skill`), per NPC.

use engine_core::prelude::*;

/// How capable an NPC is. All values are 0..1 unless a unit is given; 0.5 is an average
/// soldier. Presets: [`NpcSkills::rookie`], [`soldier`](NpcSkills::soldier),
/// [`veteran`](NpcSkills::veteran), [`elite`](NpcSkills::elite), [`master`](NpcSkills::master),
/// or [`NpcSkills::at_level`] for anything in between.
#[derive(Component, Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct NpcSkills {
    /// Seconds between something happening (an attack starting, an enemy stepping out) and
    /// the NPC acting on it. Counted on the NPC's own clock, so slowing the NPC with
    /// `LocalTimeScale` or world slow motion slows its reactions too. Humans ≈ 0.25.
    pub reaction_time: f32,
    /// Aim and strike precision: spread of shots, how often a swing finds its mark.
    pub accuracy: f32,
    /// Tactical reasoning: how many positions it weighs, whether it flanks, predicts where
    /// targets are going, uses high ground and explosives, and shares plans with its squad.
    pub logic: f32,
    /// How readily it attacks instead of waiting for an opening.
    pub aggression: f32,
    /// How long it keeps fighting when hurt, outnumbered or under fire.
    pub courage: f32,
    /// Chance to block, parry or dodge an attack it reacted to in time.
    pub defense: f32,
    /// Multiplies sight and hearing range and how fast it notices things.
    pub awareness: f32,
    /// Follows squad plans: attack turns, suppress-and-flank, callouts.
    pub teamwork: f32,
    /// How fast it adapts to an opponent's habits (always blocks, always dodges left,
    /// attacks on a rhythm).
    pub learning: f32,
    /// Multiplies movement speed and how quickly it changes direction.
    pub agility: f32,
}

impl Default for NpcSkills {
    fn default() -> Self {
        Self::soldier()
    }
}

impl NpcSkills {
    /// Slow to react, rushes in, ignores tactics.
    pub fn rookie() -> Self {
        Self::at_level(0.1)
    }

    /// An average fighter.
    pub fn soldier() -> Self {
        Self::at_level(0.45)
    }

    /// Uses cover and flanks, blocks most attacks.
    pub fn veteran() -> Self {
        Self::at_level(0.7)
    }

    /// Fast, precise, reads you and punishes mistakes.
    pub fn elite() -> Self {
        Self::at_level(0.88)
    }

    /// Near-perfect reactions and tactics. Only beatable with an edge (slow motion, numbers).
    pub fn master() -> Self {
        Self::at_level(1.0)
    }

    /// Interpolate every skill for an overall level 0..1 (0 = rookie, 1 = master).
    pub fn at_level(level: f32) -> Self {
        let l = level.clamp(0.0, 1.0);
        let mix = |a: f32, b: f32| a + (b - a) * l;
        Self {
            reaction_time: mix(0.65, 0.07),
            accuracy: mix(0.2, 0.97),
            logic: mix(0.05, 1.0),
            aggression: mix(0.75, 0.6),
            courage: mix(0.35, 0.95),
            defense: mix(0.1, 0.97),
            awareness: mix(0.55, 1.0),
            teamwork: mix(0.1, 1.0),
            learning: mix(0.0, 1.0),
            agility: mix(0.85, 1.25),
        }
    }

    /// Named preset: `rookie`, `soldier`, `veteran`, `elite`, `master`.
    pub fn preset(name: &str) -> Option<Self> {
        Some(match name {
            "rookie" => Self::rookie(),
            "soldier" => Self::soldier(),
            "veteran" => Self::veteran(),
            "elite" => Self::elite(),
            "master" => Self::master(),
            _ => return None,
        })
    }

    /// Names accepted by [`NpcSkills::preset`].
    pub const PRESETS: [&'static str; 5] = ["rookie", "soldier", "veteran", "elite", "master"];

    /// Overall level 0..1 (average of the 0..1 skills), for UI and difficulty scaling.
    pub fn level(&self) -> f32 {
        let reaction = 1.0 - ((self.reaction_time - 0.07) / 0.58).clamp(0.0, 1.0);
        (reaction
            + self.accuracy
            + self.logic
            + self.courage
            + self.defense
            + self.awareness
            + self.teamwork
            + self.learning)
            / 8.0
    }

    /// Clamp every value into its valid range (after edits from data or the API).
    pub fn clamped(mut self) -> Self {
        self.reaction_time = self.reaction_time.clamp(0.0, 5.0);
        for v in [
            &mut self.accuracy,
            &mut self.logic,
            &mut self.aggression,
            &mut self.courage,
            &mut self.defense,
            &mut self.awareness,
            &mut self.teamwork,
            &mut self.learning,
        ] {
            *v = v.clamp(0.0, 1.0);
        }
        self.agility = self.agility.clamp(0.1, 3.0);
        self
    }
}

/// What an NPC (or animal) can sense.
#[derive(Component, Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Perception {
    /// Clear-day sight range, meters (scaled by awareness, darkness and fog).
    pub sight_range: f32,
    /// Full field of view, degrees. Things in it are noticed at full speed.
    pub fov_deg: f32,
    /// Wider peripheral field, degrees: noticed at a third of the speed.
    pub peripheral_deg: f32,
    /// Multiplies the audible radius of every `NoiseEvent`.
    pub hearing: f32,
    /// Eye height above the feet, meters.
    pub eye_height: f32,
    /// Seconds of clear, close view needed to go from unaware to certain.
    pub detection_secs: f32,
    /// Seconds a lost target is remembered before it is forgotten.
    pub memory_secs: f32,
}

impl Default for Perception {
    fn default() -> Self {
        Self {
            sight_range: 60.0,
            fov_deg: 120.0,
            peripheral_deg: 200.0,
            hearing: 1.0,
            eye_height: 1.65,
            detection_secs: 0.6,
            memory_secs: 20.0,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn presets_rank_in_order() {
        let levels: Vec<f32> = NpcSkills::PRESETS
            .iter()
            .map(|p| NpcSkills::preset(p).unwrap().level())
            .collect();
        assert!(levels.windows(2).all(|w| w[0] < w[1]), "{levels:?}");
        assert!(NpcSkills::master().reaction_time < 0.1);
        assert!(NpcSkills::rookie().reaction_time > 0.5);
    }

    #[test]
    fn clamped_fixes_out_of_range_values() {
        let s = NpcSkills {
            accuracy: 3.0,
            logic: -1.0,
            reaction_time: -0.2,
            ..NpcSkills::soldier()
        }
        .clamped();
        assert_eq!(s.accuracy, 1.0);
        assert_eq!(s.logic, 0.0);
        assert_eq!(s.reaction_time, 0.0);
    }
}
