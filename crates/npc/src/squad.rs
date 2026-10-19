//! Squads: NPCs that share a [`Squad`](crate::faction::Squad) tell each other what they see
//! (a shouted callout, which enemies can hear too), take turns attacking in melee so they don't
//! crowd one opponent, and split into suppressors and a flanker against the same target. When
//! one of them kneels to revive a downed squad mate, the other shooters cover it.

use std::collections::HashMap;

use engine_core::prelude::*;
use engine_world::NoiseEvent;

use crate::agent::{Npc, NpcAction, NpcBrain, NpcClock, NpcVitals, NpcWeapon, WeaponStyle};
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

#[derive(Debug, Clone, Copy)]
struct ReviveClaim {
    reviver: Entity,
    /// How much the reviver wants it (its utility score).
    score: f32,
    /// World time the claim lapses unless renewed.
    until: f64,
    /// The reviver is there (or nearly): nobody takes it over now.
    locked: bool,
}

/// Who is going to revive which downed squad mate, so that one goes, the best placed: each
/// squad mate that wants to bids its revive score every time it thinks, the highest bid holds
/// the claim, and a reviver that is there keeps it. A claim lapses a second after its holder
/// stops bidding (it was downed, gave up, or found something better to do).
#[derive(Resource, Debug, Default)]
pub struct ReviveClaims {
    claims: HashMap<Entity, ReviveClaim>,
}

impl ReviveClaims {
    /// How long a claim lasts without being renewed (world seconds).
    const SECS: f64 = 1.0;

    /// How much better placed a mate must be to take a claim over (revive score).
    const MARGIN: f32 = 0.05;

    /// Whether `me` bidding `score` to revive `downed` at world time `now` would hold the
    /// claim (see [`ReviveClaims::bid`]): nobody else holds it, or they are not there yet
    /// and `me` is clearly better placed.
    pub fn open_to(&self, downed: Entity, me: Entity, score: f32, now: f64) -> bool {
        self.claims.get(&downed).is_none_or(|c| {
            c.until <= now || c.reviver == me || (!c.locked && score > c.score + 0.05)
        })
    }

    /// `me` bids `score` to revive `downed` at world time `now` (`locked`: it is there and
    /// nobody should take over). Returns true if `me` holds the claim now.
    pub fn bid(&mut self, downed: Entity, me: Entity, score: f32, now: f64, locked: bool) -> bool {
        let mine = ReviveClaim {
            reviver: me,
            score,
            until: now + Self::SECS,
            locked,
        };
        let c = self.claims.entry(downed).or_insert(mine);
        if c.until <= now || c.reviver == me || (!c.locked && score > c.score + Self::MARGIN) {
            *c = mine;
        }
        c.reviver == me
    }

    /// `me`, on its way or kneeling, keeps its claim on `downed` alive (between its thinks).
    /// Returns false if someone else holds it.
    pub fn renew(&mut self, downed: Entity, me: Entity, now: f64) -> bool {
        match self.claims.get_mut(&downed) {
            Some(c) if c.reviver == me => {
                c.until = c.until.max(now + Self::SECS);
                true
            }
            Some(c) if c.until > now => false,
            _ => {
                self.claims.insert(
                    downed,
                    ReviveClaim {
                        reviver: me,
                        score: 0.0,
                        until: now + Self::SECS,
                        locked: false,
                    },
                );
                true
            }
        }
    }

    /// Give up every claim `me` holds.
    pub fn release(&mut self, me: Entity) {
        self.claims.retain(|_, c| c.reviver != me);
    }

    /// Who holds the claim on `downed` at world time `now`.
    pub fn reviver(&self, downed: Entity, now: f64) -> Option<Entity> {
        self.claims
            .get(&downed)
            .filter(|c| c.until > now)
            .map(|c| c.reviver)
    }

    /// Forget lapsed claims.
    pub(crate) fn expire(&mut self, now: f64) {
        self.claims.retain(|_, c| c.until > now);
    }
}

type MemberItem<'a> = (
    Entity,
    &'a Squad,
    &'a Faction,
    &'a Transform,
    &'a NpcSkills,
    &'a NpcWeapon,
    (&'a NpcClock, &'a Npc),
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
    // Squads with a member kneeling over (or on its way to) a downed mate.
    let mut reviving: Vec<Squad> = Vec::new();
    for (e, squad, _, t, skills, weapon, (clock, npc), vitals, brain, memory) in q.iter() {
        if vitals.downed {
            continue;
        }
        // (One handed to game code or stopped in time revives nobody.)
        let acting = npc.active && clock.dt > 0.0;
        if brain.action == NpcAction::Revive && acting && !reviving.contains(squad) {
            reviving.push(*squad);
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
    for (e, squad, _, t, skills, _, (clock, _), vitals, mut brain, mut memory) in q.iter_mut() {
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
    // While a mate revives someone, the other shooters cover it: they hold where they are and
    // keep the enemy's heads down (fire at where it was last seen), instead of flanking.
    for m in &members {
        if m.ranged && m.skills.teamwork >= 0.3 && reviving.contains(&m.squad) {
            roles.insert(m.e, "cover");
        }
    }
    for (e, _, _, _, _, _, _, _, mut brain, _) in q.iter_mut() {
        let role = roles.get(&e).copied().unwrap_or("");
        brain.role = if brain.action == NpcAction::Revive {
            ""
        } else {
            role
        };
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

    #[test]
    fn the_best_placed_squad_mate_holds_a_revive() {
        let mut c = ReviveClaims::default();
        let downed = Entity::from_raw_u32(1).unwrap();
        let far = Entity::from_raw_u32(2).unwrap();
        let near = Entity::from_raw_u32(3).unwrap();
        let late = Entity::from_raw_u32(4).unwrap();
        assert!(c.bid(downed, far, 0.5, 0.0, false));
        assert!(
            c.bid(downed, near, 0.8, 0.1, false),
            "a better bid takes it over"
        );
        assert!(!c.bid(downed, far, 0.5, 0.2, false));
        assert_eq!(c.reviver(downed, 0.2), Some(near));
        // There now: kept against a better bid.
        assert!(c.bid(downed, near, 0.8, 0.3, true));
        assert!(!c.bid(downed, late, 0.95, 0.4, false));
        // Kept alive between thinks while it is at it; nobody else can renew it.
        assert!(c.renew(downed, near, 1.2));
        assert!(!c.renew(downed, far, 1.2));
        assert_eq!(c.reviver(downed, 2.0), Some(near));
        // Lapsed once its holder stops.
        assert_eq!(c.reviver(downed, 2.3), None);
        assert!(c.bid(downed, late, 0.3, 2.3, false));
        c.release(late);
        assert_eq!(c.reviver(downed, 2.4), None);
    }
}
