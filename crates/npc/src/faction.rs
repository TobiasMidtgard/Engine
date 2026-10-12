//! Who fights whom. Put a [`Faction`] on NPCs, animals and the player; [`Factions`] says which
//! factions are hostile. Entities without a `Faction` are ignored by NPC targeting.

use engine_core::prelude::*;

/// Team membership. Put it on the player too, so NPCs know who to fight.
#[derive(Component, Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Faction(pub u32);

impl Faction {
    /// The player's side.
    pub const PLAYER: Faction = Faction(0);
    /// Generic hostile NPCs.
    pub const ENEMY: Faction = Faction(1);
    /// Second hostile side (for NPC-vs-NPC battles).
    pub const RIVAL: Faction = Faction(2);
    /// Grazing animals: flee from everyone, attack no one.
    pub const PREY: Faction = Faction(10);
    /// Hunting animals: hunt prey, avoid armed groups.
    pub const PREDATOR: Faction = Faction(11);
}

/// Squad membership. NPCs in the same squad share what they see, take turns attacking and
/// split into suppressors and flankers.
#[derive(Component, Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Squad(pub u32);

/// Relations between factions. Default: every pair of different factions is hostile, except
/// that prey are hostile to no one (they flee instead of fighting).
#[derive(Resource, Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Factions {
    /// Pairs set to friendly (order doesn't matter).
    pub allied: Vec<[u32; 2]>,
    /// Factions that never attack anyone (prey animals, civilians).
    pub passive: Vec<u32>,
}

impl Factions {
    /// Would `a` attack `b`?
    pub fn hostile(&self, a: Faction, b: Faction) -> bool {
        if a == b || self.passive.contains(&a.0) {
            return false;
        }
        !self
            .allied
            .iter()
            .any(|p| (p[0] == a.0 && p[1] == b.0) || (p[0] == b.0 && p[1] == a.0))
    }

    /// Make two factions friends (or enemies again with `allied = false`).
    pub fn set_allied(&mut self, a: Faction, b: Faction, allied: bool) {
        self.allied
            .retain(|p| !((p[0] == a.0 && p[1] == b.0) || (p[0] == b.0 && p[1] == a.0)));
        if allied {
            self.allied.push([a.0, b.0]);
        }
    }
}

pub(crate) fn default_factions() -> Factions {
    Factions {
        allied: Vec::new(),
        passive: vec![Faction::PREY.0],
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hostility_rules() {
        let mut f = default_factions();
        assert!(f.hostile(Faction::ENEMY, Faction::PLAYER));
        assert!(!f.hostile(Faction::ENEMY, Faction::ENEMY));
        assert!(!f.hostile(Faction::PREY, Faction::PLAYER));
        assert!(f.hostile(Faction::PREDATOR, Faction::PREY));
        f.set_allied(Faction::ENEMY, Faction::RIVAL, true);
        assert!(!f.hostile(Faction::RIVAL, Faction::ENEMY));
        f.set_allied(Faction::RIVAL, Faction::ENEMY, false);
        assert!(f.hostile(Faction::RIVAL, Faction::ENEMY));
    }
}
