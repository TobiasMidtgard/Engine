//! Squads: NPCs that share a [`Squad`](crate::faction::Squad) tell each other what they see
//! (a shouted callout, which enemies can hear too), take turns attacking in melee so they don't
//! crowd one opponent, and suppress and flank the same target: one shooter pins it with steady
//! fire while one or two mates (two in a squad of three or more) go round to its flank out of
//! its sight, bounding from cover to cover in turns, one moving while the other holds and
//! covers it. Whether they do depends on the squad's smartest shooter's logic and the squad's
//! teamwork; a squad without (rookies) rushes straight in. When one of them kneels to revive a
//! downed squad mate, the other shooters cover it. While the squad's medic kneels, they fire
//! at whoever can see it, and its melee mates screen it.

use std::collections::HashMap;

use engine_core::prelude::*;
use engine_world::NoiseEvent;

use crate::agent::{
    Npc, NpcAction, NpcBrain, NpcClock, NpcRole, NpcVitals, NpcWeapon, WeaponStyle,
};
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
    medic: bool,
    target: Option<Entity>,
    /// Flanking: holding at a cover on the way, or at its flank spot.
    holding: bool,
    in_position: bool,
}

/// A squad's suppress-and-flank against one enemy: who pins it, who goes round to its flank,
/// and which of those has the turn to move.
#[derive(Debug, Clone, Default)]
struct Maneuver {
    suppressor: Option<Entity>,
    flankers: Vec<Entity>,
    mover: usize,
}

/// The maneuvers under way, by squad and enemy (kept between planning passes).
#[derive(Default)]
pub(crate) struct Plans {
    maneuvers: HashMap<(Squad, Entity), Maneuver>,
}

/// The least logic a squad's smartest shooter needs, and the least teamwork the squad needs
/// on average, to suppress and flank (a squad of rookies rushes straight in); and what each
/// flanker needs.
const PLAN_LOGIC: f32 = 0.35;
const PLAN_TEAMWORK: f32 = 0.3;

/// Share sightings and hand out roles (runs every `plan_secs`).
pub(crate) fn plan_squads(
    mut q: Query<MemberItem>,
    settings: Res<SquadSettings>,
    time: Res<Time>,
    mut noise: MessageWriter<NoiseEvent>,
    mut since: Local<f32>,
    mut plans: Local<Plans>,
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
    // Squads whose medic goes to or kneels over a downed mate, and where it kneels.
    let mut medics: Vec<(Squad, DVec3)> = Vec::new();
    for (e, squad, _, t, skills, weapon, (clock, npc), vitals, brain, memory) in q.iter() {
        if vitals.downed {
            continue;
        }
        // (One handed to game code or stopped in time revives nobody.)
        let acting = npc.active && clock.dt > 0.0;
        if brain.action == NpcAction::Revive && acting && !reviving.contains(squad) {
            reviving.push(*squad);
        }
        let medic = npc.role == NpcRole::Medic;
        if medic && acting && brain.action == NpcAction::Revive {
            // (Its mates get there as it does.)
            let at = if brain.kneeling() {
                t.translation
            } else {
                brain.destination.unwrap_or(t.translation)
            };
            medics.push((*squad, at));
        }
        members.push(Member {
            e,
            squad: *squad,
            skills: *skills,
            ranged: weapon.style == WeaponStyle::Ranged,
            medic,
            target: brain.target,
            holding: brain.holding,
            in_position: brain.in_position,
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
    // Roles: per squad and shared target, suppress and flank (see the module docs).
    let mut by_target: HashMap<(Squad, Entity), Vec<usize>> = HashMap::new();
    for (i, m) in members.iter().enumerate() {
        if let Some(t) = m.target {
            by_target.entry((m.squad, t)).or_default().push(i);
        }
    }
    let mut roles: HashMap<Entity, &'static str> = HashMap::new();
    let mut bound: HashMap<Entity, bool> = HashMap::new();
    let mut groups: Vec<_> = by_target.into_iter().collect();
    // (In a fixed order: what each pass decides doesn't depend on how a map iterates.)
    groups.sort_by_key(|(_, list)| list[0]);
    let mut kept = HashMap::new();
    for (key, list) in groups {
        let ranged: Vec<&Member> = list
            .iter()
            .map(|&i| &members[i])
            .filter(|m| m.ranged)
            .collect();
        if ranged.len() < 2 {
            continue;
        }
        let smartest = ranged.iter().map(|m| m.skills.logic).fold(0.0, f32::max);
        let teamwork = ranged.iter().map(|m| m.skills.teamwork).sum::<f32>() / ranged.len() as f32;
        if smartest < PLAN_LOGIC || teamwork < PLAN_TEAMWORK {
            continue;
        }
        let can_flank =
            |m: &Member| !m.medic && m.skills.teamwork >= 0.3 && m.skills.logic >= PLAN_LOGIC;
        let had = plans.maneuvers.remove(&key).unwrap_or_default();
        // The flankers: those already going round keep at it; the smartest others join them,
        // up to two, one shooter always left to pin the enemy.
        let want = if ranged.len() >= 3 { 2 } else { 1 };
        let mut flankers: Vec<Entity> = had
            .flankers
            .iter()
            .copied()
            .filter(|&e| ranged.iter().any(|m| m.e == e && can_flank(m)))
            .collect();
        let mut others: Vec<&&Member> = ranged
            .iter()
            .filter(|m| can_flank(m) && !flankers.contains(&m.e) && had.suppressor != Some(m.e))
            .collect();
        others.sort_by(|a, b| b.skills.logic.total_cmp(&a.skills.logic));
        for m in others {
            if flankers.len() >= want {
                break;
            }
            flankers.push(m.e);
        }
        flankers.truncate(want);
        // The one that pins it: it keeps the job, else the best shot of the rest.
        let rest = || ranged.iter().filter(|m| !flankers.contains(&m.e));
        let suppressor = had
            .suppressor
            .filter(|&s| rest().any(|m| m.e == s))
            .or_else(|| {
                rest()
                    .filter(|m| !m.medic)
                    .max_by(|a, b| a.skills.accuracy.total_cmp(&b.skills.accuracy))
                    .map(|m| m.e)
            });
        let Some(suppressor) = suppressor else {
            continue;
        };
        // Whose turn it is to move: one at a time; the other holds a cover and covers it. The
        // turn passes on once the mover reaches a cover on the way (or its flank spot).
        let state = |e: Entity| {
            ranged
                .iter()
                .find(|m| m.e == e)
                .map_or((false, false), |m| (m.holding, m.in_position))
        };
        let moving: Vec<usize> = (0..flankers.len())
            .filter(|&i| !state(flankers[i]).1)
            .collect();
        let mut mover = had.mover.min(flankers.len().saturating_sub(1));
        if moving.len() >= 2 {
            if !moving.contains(&mover) || state(flankers[mover]).0 {
                mover = moving
                    .iter()
                    .copied()
                    .find(|&i| i != mover)
                    .unwrap_or(mover);
            }
        } else if let Some(&only) = moving.first() {
            mover = only;
        }
        for m in &ranged {
            let role = if flankers.contains(&m.e) {
                "flank"
            } else if m.e == suppressor || (m.skills.teamwork >= 0.3 && !m.medic) {
                "suppress"
            } else {
                ""
            };
            roles.insert(m.e, role);
        }
        for (i, &f) in flankers.iter().enumerate() {
            bound.insert(f, moving.len() < 2 || i == mover);
        }
        kept.insert(
            key,
            Maneuver {
                suppressor: Some(suppressor),
                flankers,
                mover,
            },
        );
    }
    plans.maneuvers = kept;
    // While a mate revives someone, the other shooters cover it: they hold where they are and
    // keep the enemy's heads down (fire at where it was last seen), instead of flanking.
    // A medic going to kneel or kneeling most of all: the shooters fire at whoever can see
    // its spot, and the melee fighters go stand between it and the enemy.
    let mut guards: HashMap<Entity, DVec3> = HashMap::new();
    for m in &members {
        if m.skills.teamwork < 0.3 {
            continue;
        }
        if m.ranged && reviving.contains(&m.squad) {
            roles.insert(m.e, "cover");
        }
        if let Some(&(_, at)) = medics.iter().find(|(sq, _)| *sq == m.squad) {
            if !m.ranged {
                roles.insert(m.e, "screen");
            }
            guards.insert(m.e, at);
        }
    }
    for (e, _, _, _, _, _, _, _, mut brain, _) in q.iter_mut() {
        let role = roles.get(&e).copied().unwrap_or("");
        let reviving = brain.action == NpcAction::Revive;
        brain.role = if reviving { "" } else { role };
        brain.guard = guards.get(&e).copied().filter(|_| !reviving);
        brain.bound = bound.get(&e).copied().unwrap_or(true);
        if brain.role != "flank" {
            // Not going round any more: what it planned for that goes.
            brain.holding = false;
            brain.in_position = false;
            brain.flank_goal = None;
            brain.flank_route.clear();
        }
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
