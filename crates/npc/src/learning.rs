//! Learning an opponent's habits. Every NPC fight feeds a shared model of each opponent (the
//! player, usually): how often they block, which way they dodge, the rhythm of their attacks.
//! NPCs with a high `learning` skill use it: feints against habitual blockers, guard raised just
//! before a rhythmic attacker swings, swings aimed at the side a dodger rolls to, and the parts
//! an attacker keeps going for guarded harder (from the combat module's `BodyHit`s).

use bevy_platform::collections::HashMap;

use engine_core::prelude::*;
use engine_world::combat::{BodyHit, BodyZone};

/// What NPCs have learned about one opponent.
#[derive(Debug, Clone, Copy, PartialEq, Default, Serialize, Deserialize)]
pub struct OpponentModel {
    /// Fraction of NPC attacks this opponent blocked or parried (smoothed).
    pub block_rate: f32,
    /// Fraction of NPC attacks this opponent dodged (smoothed).
    pub dodge_rate: f32,
    /// Which way they dodge relative to the attacker: −1 left .. +1 right (smoothed).
    pub dodge_side: f32,
    /// Seconds between their attacks (smoothed; 0 = unknown).
    pub attack_interval: f32,
    /// World time of their last attack.
    pub last_attack: f64,
    /// Attacks NPCs have thrown at them.
    pub defended_samples: u32,
    /// Attacks they have thrown.
    pub attack_samples: u32,
    /// Where on a body their blows land, by [`BodyZone::ALL`] index (smoothed shares, from the
    /// combat module's `BodyHit`s).
    pub aims: [f32; 16],
    /// Blows of theirs seen landing.
    pub aim_samples: u32,
}

impl OpponentModel {
    /// How much to trust the model (0..1) given the samples seen.
    pub fn confidence(&self) -> f32 {
        (self.defended_samples as f32 / 6.0).min(1.0)
    }

    /// Record the outcome of an NPC attack on this opponent.
    pub fn record_defense(&mut self, blocked: bool, dodged: bool, dodge_side: f32) {
        let a = 0.25;
        self.block_rate += (if blocked { 1.0 } else { 0.0 } - self.block_rate) * a;
        self.dodge_rate += (if dodged { 1.0 } else { 0.0 } - self.dodge_rate) * a;
        if dodged {
            self.dodge_side += (dodge_side.clamp(-1.0, 1.0) - self.dodge_side) * a;
        }
        self.defended_samples += 1;
    }

    /// Record that this opponent started an attack at world time `now`.
    pub fn record_attack(&mut self, now: f64) {
        if self.attack_samples > 0 {
            let gap = (now - self.last_attack) as f32;
            if gap > 0.15 && gap < 6.0 {
                self.attack_interval = if self.attack_interval <= 0.0 {
                    gap
                } else {
                    self.attack_interval + (gap - self.attack_interval) * 0.3
                };
            }
        }
        self.last_attack = now;
        self.attack_samples += 1;
    }

    /// Record a blow of theirs landing on `zone`.
    pub fn record_aim(&mut self, zone: BodyZone) {
        let a = 0.2;
        for (i, share) in self.aims.iter_mut().enumerate() {
            let hit = if i == zone.index() { 1.0 } else { 0.0 };
            *share += (hit - *share) * a;
        }
        self.aim_samples += 1;
    }

    /// How much of what they land goes to `zone` (0..1), as far as it can be trusted: nothing
    /// until a few blows have been seen. A knee counts the thigh and shin beside it at half,
    /// as a guard low covers them all.
    pub fn aim_share(&self, zone: BodyZone) -> f32 {
        let trust = (self.aim_samples as f32 / 4.0).min(1.0);
        let near: f32 = BodyZone::ALL
            .iter()
            .filter(|z| **z != zone && same_region(**z, zone))
            .map(|z| self.aims[z.index()])
            .sum();
        (self.aims[zone.index()] + 0.5 * near).min(1.0) * trust
    }

    /// World time this opponent's next attack is expected, if they attack on a rhythm.
    pub fn next_attack_expected(&self) -> Option<f64> {
        (self.attack_samples >= 3 && self.attack_interval > 0.0)
            .then_some(self.last_attack + self.attack_interval as f64)
    }
}

/// Parts a guard covers together: head and neck; chest and belly; one arm; one leg.
fn same_region(a: BodyZone, b: BodyZone) -> bool {
    use BodyZone::*;
    let region = |z: BodyZone| match z {
        Head | Neck => 0,
        Chest | Belly => 1,
        UpperArmL | ForearmL | HandL => 2,
        UpperArmR | ForearmR | HandR => 3,
        ThighL | KneeL | ShinL => 4,
        ThighR | KneeR | ShinR => 5,
    };
    region(a) == region(b)
}

/// Learn where each body's blows land from the combat module's `BodyHit`s (`learning`
/// skill decides how much a defender makes of it: see `wounds::guard_weight`).
pub(crate) fn learn_aims(mut hits: MessageReader<BodyHit>, mut models: ResMut<OpponentModels>) {
    for h in hits.read() {
        if let Some(a) = h.attacker {
            models.entry(a).record_aim(h.zone);
        }
    }
}

/// Opponent models by entity, shared by every NPC (they compare notes).
#[derive(Resource, Debug, Clone, Default)]
pub struct OpponentModels {
    pub models: HashMap<Entity, OpponentModel>,
}

impl OpponentModels {
    pub fn get(&self, e: Entity) -> OpponentModel {
        self.models.get(&e).copied().unwrap_or_default()
    }

    pub fn entry(&mut self, e: Entity) -> &mut OpponentModel {
        self.models.entry(e).or_default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn learns_block_habit_and_rhythm() {
        let mut m = OpponentModel::default();
        for _ in 0..8 {
            m.record_defense(true, false, 0.0);
        }
        assert!(m.block_rate > 0.85 && m.confidence() == 1.0);
        for k in 0..5 {
            m.record_attack(k as f64 * 1.5);
        }
        assert!((m.attack_interval - 1.5).abs() < 1e-3);
        assert!((m.next_attack_expected().unwrap() - 7.5).abs() < 1e-3);
    }
}
