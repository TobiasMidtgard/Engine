//! Who fights whom: the contract lives in `engine_world::faction`, so other modules (magic)
//! can tell friends from foes; NPC AI sets its default relations.

pub use engine_world::faction::{Faction, Factions, Squad, default_factions};
