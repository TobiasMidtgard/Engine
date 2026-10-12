//! Squads: NPCs that share a [`Squad`](crate::faction::Squad) tell each other what they see
//! (a shouted callout, which enemies can hear too), take turns attacking in melee so they don't
//! crowd one opponent, and split into suppressors and a flanker against the same target.

use std::collections::HashMap;

use engine_core::prelude::*;
use engine_world::NoiseEvent;

use crate::agent::{NpcBrain, NpcClock, NpcVitals, NpcWeapon, WeaponStyle};
use crate::faction::{Faction, Squad};
use crate::perception::NpcMemory;
use crate::skills::NpcSkills;

/// Squad tuning.
#[derive(Resource, Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(default)]
pub struct SquadSettings {
    /// At most this many NPCs swing at one target at a time (teamwork-respecting ones).
    pub max_melee_attackers: u32,
    /// Seconds an attack turn lasts before another NPC gets it.
    pub token_secs: f32,
    /// Callouts reach squad mates within this distance, meters.
    pub callout_range: f32,
    /// Seconds between squad planning passes.
    pub plan_secs: f32,
}

impl Default for SquadSettings {
    fn default() -> Self {
        Self {
            max_melee_attackers: 2,
            token_secs: 2.5,
            callout_range: 45.0,
            plan_secs: 0.5,
        }
    }
}

#[derive(Debug, Clone, Copy)]
struct Token {
    holder: Entity,
    /// World time it expires.
    until: f64,
}

/// Melee attack turns per target ("tokens").
#[derive(Resource, Debug, Default)]
pub struct AttackTokens {
    tokens: HashMap<Entity, Vec<Token>>,
}

impl AttackTokens {
    /// Ask for a turn to attack `target`. Returns true if `me` holds one now.
    pub fn request(&mut self, target: Entity, me: Entity, now: f64, max: u32, secs: f32) -> bool {
        let list = self.tokens.entry(target).or_default();
        list.retain(|t| t.until > now);
        if list.iter().any(|t| t.holder == me) {
            return true;
        }
        if (list.len() as u32) < max.max(1) {
            list.push(Token {
                holder: me,
                until: now + secs as f64,
            });
            return true;
        }
        false
    }

    /// Give the turn back (after an attack, or when leaving the fight).
    pub fn release(&mut self, me: Entity) {
        for list in self.tokens.values_mut() {
            list.retain(|t| t.holder != me);
        }
    }

    /// How many hold a turn on `target`.
    pub fn holders(&self, target: Entity, now: f64) -> usize {
        self.tokens
            .get(&target)
            .map(|l| l.iter().filter(|t| t.until > now).count())
            .unwrap_or(0)
    }
}

type MemberItem<'a> = (
    Entity,
    &'a Squad,
    &'a Faction,
    &'a Transform,
    &'a NpcSkills,
    &'a NpcWeapon,
    &'a NpcClock,
    &'a NpcVitals,
    &'a mut NpcBrain,
    &'a mut NpcMemory,
);

struct Member {
    e: Entity,
    squad: Squad,
    skills: NpcSkills,
    ranged: bool,
    target: Option<Entity>,
}

/// Share sightings and hand out roles (runs every `plan_secs`).
pub(crate) fn plan_squads(
    mut q: Query<MemberItem>,
    settings: Res<SquadSettings>,
    time: Res<Time>,
    mut noise: MessageWriter<NoiseEvent>,
    mut since: Local<f32>,
) {
    *since += time.delta_secs();
    if *since < settings.plan_secs {
        return;
    }
    *since = 0.0;
    let mut members: Vec<Member> = Vec::new();
    // Sightings: (squad, spotter, enemy, faction, position, velocity).
    let mut sightings: Vec<(Squad, Entity, Entity, Faction, DVec3, Vec3, DVec3)> = Vec::new();
    for (e, squad, _, t, skills, weapon, _, vitals, brain, memory) in q.iter() {
        if vitals.downed {
            continue;
        }
        members.push(Member {
            e,
            squad: *squad,
            skills: *skills,
            ranged: weapon.style == WeaponStyle::Ranged,
            target: brain.target,
        });
        for k in &memory.known {
            if k.visible && k.awareness >= 0.9 {
                sightings.push((
                    *squad,
                    e,
                    k.entity,
                    k.faction,
                    k.last_position,
                    k.last_velocity,
                    t.translation,
                ));
            }
        }
    }
    // Callouts.
    let mut shouted: Vec<Entity> = Vec::new();
    for (e, squad, _, t, skills, _, clock, vitals, mut brain, mut memory) in q.iter_mut() {
        if vitals.downed {
            continue;
        }
        for &(sq, spotter, enemy, faction, at, vel, spotter_pos) in &sightings {
            if sq != *squad || spotter == e {
                continue;
            }
            if spotter_pos.distance(t.translation) > settings.callout_range as f64 {
                continue;
            }
            // Low-teamwork NPCs often don't listen.
            if brain.random() > 0.35 + 0.65 * skills.teamwork {
                continue;
            }
            let certainty = 0.6 + 0.3 * skills.teamwork;
            memory.report(enemy, faction, at, vel, clock.now, certainty);
            if !shouted.contains(&spotter) {
                shouted.push(spotter);
                noise.write(NoiseEvent {
                    position: spotter_pos + DVec3::Y * 1.6,
                    radius: 30.0,
                    source: Some(spotter),
                });
            }
        }
    }
    // Roles: per squad and shared target, the smartest ranged member flanks, the rest suppress.
    let mut by_target: HashMap<(Squad, Entity), Vec<usize>> = HashMap::new();
    for (i, m) in members.iter().enumerate() {
        if let Some(t) = m.target {
            by_target.entry((m.squad, t)).or_default().push(i);
        }
    }
    let mut roles: HashMap<Entity, &'static str> = HashMap::new();
    for list in by_target.values() {
        let ranged: Vec<&Member> = list
            .iter()
            .map(|&i| &members[i])
            .filter(|m| m.ranged)
            .collect();
        if ranged.len() >= 2 {
            let flanker = ranged
                .iter()
                .filter(|m| m.skills.teamwork >= 0.3 && m.skills.logic >= 0.35)
                .max_by(|a, b| a.skills.logic.total_cmp(&b.skills.logic));
            for m in &ranged {
                let role = if flanker.is_some_and(|f| f.e == m.e) {
                    "flank"
                } else if m.skills.teamwork >= 0.3 {
                    "suppress"
                } else {
                    ""
                };
                roles.insert(m.e, role);
            }
        }
    }
    for (e, _, _, _, _, _, _, _, mut brain, _) in q.iter_mut() {
        brain.role = roles.get(&e).copied().unwrap_or("");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tokens_limit_attackers_and_expire() {
        let mut t = AttackTokens::default();
        let target = Entity::from_raw_u32(1).unwrap();
        let a = Entity::from_raw_u32(2).unwrap();
        let b = Entity::from_raw_u32(3).unwrap();
        let c = Entity::from_raw_u32(4).unwrap();
        assert!(t.request(target, a, 0.0, 2, 2.0));
        assert!(t.request(target, b, 0.0, 2, 2.0));
        assert!(!t.request(target, c, 0.5, 2, 2.0));
        assert!(t.request(target, a, 0.5, 2, 2.0), "a holder keeps its turn");
        t.release(a);
        assert!(t.request(target, c, 0.6, 2, 2.0));
        assert!(t.request(target, a, 3.0, 2, 2.0), "turns expire");
    }
}
