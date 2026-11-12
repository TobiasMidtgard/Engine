//! What spells do to minds: [`engine_world::MindEffect`] (magic sends it) taken in as a
//! [`Spellbound`] on the NPC or animal it targets.
//!
//! - **Fear**: a fighter drops everything and runs from the source ([`NpcAction::Flee`]); an
//!   animal bolts from it as from a shot.
//! - **Calm**: no aggression. A fighter forgets its enemies and goes about its business, an
//!   animal settles; being hurt ends it at once.
//! - **Veil**: unseen. Nothing sees the body (it can still be heard, felt and, by animals,
//!   smelled); [`engine_world::Statuses::veil`] does the same for anyone magic veils,
//!   the player included.
//! - **Command { side }**: a fighter fights for that side. Its faction is that side's until the
//!   effect ends (it leaves its squad meanwhile), so the side's foes are its foes and the
//!   side's members its friends, for everyone (magic and other modules read [`Faction`] too).
//! - **Frenzy**: a fighter attacks whoever is nearest, friend or foe: it gets a faction of its
//!   own for the time, hostile to all, and stops taking cover or falling back.
//!
//! Durations run on world time (a spell lasts as long whoever it is cast on).

use engine_core::prelude::*;
use engine_world::{DamageEvent, DamageKind, MindEffect, MindKind};

use crate::agent::{Npc, NpcBrain, NpcClock};
use crate::faction::{Faction, Squad};
use crate::wildlife::{AnimalMind, AnimalState};

/// First faction id given to frenzied fighters, each its own (hostile to every other); also
/// the first squad id given to fighters a spell takes from their squad.
pub const FRENZY_FACTIONS: u32 = 0xF2E0_0000;

/// A blow this soon after a calm takes hold was already on its way: it doesn't break it (s).
const CALM_GRACE: f64 = 1.0;

/// Veil at or above this hides a body from sight; below it, it is spotted that much slower.
pub const VEIL_UNSEEN: f32 = 0.5;

/// What spells are doing to this mind. Times are world seconds the effect lasts until.
#[derive(Component, Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Spellbound {
    pub fear_until: f64,
    /// What it fears: who cast it (followed while it can be seen), else where it was cast.
    #[serde(skip)]
    pub fear_of: Option<Entity>,
    pub fear_at: DVec3,
    pub calm_until: f64,
    /// When the calm took hold (a blow already falling then doesn't break it).
    pub calm_from: f64,
    pub veil_until: f64,
    pub command: Option<(u32, f64)>,
    pub frenzy_until: f64,
    /// The faction and squad it had before Command or Frenzy took it over.
    own: Option<(Faction, Option<Squad>)>,
    /// Its faction while frenzied.
    frenzy_faction: Option<Faction>,
}

impl Spellbound {
    pub fn afraid(&self, now: f64) -> bool {
        now < self.fear_until
    }
    pub fn calm(&self, now: f64) -> bool {
        now < self.calm_until
    }
    pub fn veiled(&self, now: f64) -> bool {
        now < self.veil_until
    }
    pub fn frenzied(&self, now: f64) -> bool {
        now < self.frenzy_until
    }
    /// The side it fights for while commanded.
    pub fn commanded(&self, now: f64) -> Option<u32> {
        self.command.filter(|c| now < c.1).map(|c| c.0)
    }
    /// The faction it had of its own, if a spell has taken it over.
    pub fn own_faction(&self) -> Option<Faction> {
        self.own.map(|o| o.0)
    }
    /// Anything still running.
    fn active(&self, now: f64) -> bool {
        self.afraid(now)
            || self.calm(now)
            || self.veiled(now)
            || self.frenzied(now)
            || self.commanded(now).is_some()
            || self.own.is_some()
    }
}

/// Where the fear comes from now: the caster where it stands, else where it was cast.
pub fn fear_source(mind: &Spellbound, positions: impl Fn(Entity) -> Option<DVec3>) -> DVec3 {
    mind.fear_of.and_then(positions).unwrap_or(mind.fear_at)
}

type MindItem<'a> = (
    &'a mut Spellbound,
    Option<&'a mut NpcBrain>,
    &'a NpcClock,
    Option<&'a mut Faction>,
    Option<&'a mut Squad>,
    Has<Npc>,
    Option<&'a mut AnimalMind>,
);

/// Takes in this step's [`MindEffect`]s, ends calm on a blow, and gives commanded and
/// frenzied fighters their faction for the time (and back after). (Every fighter and animal
/// carries a [`Spellbound`], so nothing is inserted or removed: no command sync point.)
pub(crate) fn take_minds(
    mut effects: MessageReader<MindEffect>,
    mut hurts: MessageReader<DamageEvent>,
    mut q: Query<MindItem>,
    positions: Query<&GlobalTransform>,
    mut frenzies: Local<u32>,
    time: Res<Time>,
) {
    let now = time.elapsed_secs_f64();
    let at = |e: Entity| positions.get(e).ok().map(|g| g.translation());
    for ev in effects.read() {
        let Ok((mut m, brain, clock, ..)) = q.get_mut(ev.target) else {
            continue;
        };
        // A fighter takes it in at once, not at its next think.
        if let Some(mut brain) = brain {
            brain.next_think = brain.next_think.min(clock.now);
        }
        let until = now + ev.secs.max(0.0) as f64;
        match ev.kind {
            MindKind::Fear => {
                m.fear_until = m.fear_until.max(until);
                m.fear_of = ev.source.filter(|&s| s != ev.target);
                m.fear_at = ev.source.and_then(at).unwrap_or(m.fear_at);
            }
            MindKind::Calm => {
                if !m.calm(now) {
                    m.calm_from = now;
                }
                m.calm_until = m.calm_until.max(until);
            }
            MindKind::Veil => m.veil_until = m.veil_until.max(until),
            // A later command takes over from an earlier one.
            MindKind::Command { side } => m.command = Some((side, until)),
            MindKind::Frenzy => m.frenzy_until = m.frenzy_until.max(until),
        }
    }
    // A blow wakes the calmed (a hit from anyone but itself, swung after the calm fell).
    for h in hurts.read() {
        if h.amount > 0.0
            && h.kind != DamageKind::Bleed
            && h.source != Some(h.target)
            && let Ok((mut m, ..)) = q.get_mut(h.target)
            && m.calm(now)
            && now > m.calm_from + CALM_GRACE
        {
            m.calm_until = now;
        }
    }
    for (mut m, _, clock, faction, squad, npc, animal) in &mut q {
        if !m.active(now) {
            continue;
        }
        // Command and Frenzy: the faction a fighter fights under, alone (a squad of its own
        // while it lasts).
        if let (true, Some(mut faction)) = (npc, faction) {
            let mut squad = squad;
            let want = if m.frenzied(now) {
                Some(*m.frenzy_faction.get_or_insert_with(|| {
                    *frenzies = frenzies.wrapping_add(1);
                    Faction(FRENZY_FACTIONS + (*frenzies & 0xFFFF))
                }))
            } else {
                m.commanded(now).map(Faction)
            };
            if !m.frenzied(now) {
                m.frenzy_faction = None;
            }
            match (want, m.own) {
                (Some(f), own) => {
                    if own.is_none() {
                        m.own = Some((*faction, squad.as_deref().copied()));
                        if let Some(sq) = squad.as_deref_mut() {
                            *frenzies = frenzies.wrapping_add(1);
                            *sq = Squad(FRENZY_FACTIONS + (*frenzies & 0xFFFF));
                        }
                    }
                    if *faction != f {
                        *faction = f;
                    }
                }
                (None, Some((f, own_squad))) => {
                    *faction = f;
                    if let (Some(sq), Some(own)) = (squad.as_deref_mut(), own_squad) {
                        *sq = own;
                    }
                    m.own = None;
                }
                (None, None) => {}
            }
        }
        // Animals: fear is a fright that keeps coming from the source; calm settles them.
        if let Some(mut mind) = animal
            && mind.state != AnimalState::Dead
        {
            if m.afraid(now) {
                let from = fear_source(&m, at);
                if mind.heard.is_none_or(|(_, l)| l < 3.0) {
                    mind.heard = Some((from, 3.0));
                }
            } else if m.calm(now) {
                mind.heard = None;
                mind.hurt_from = None;
                mind.danger = 0.0;
                mind.threat = None;
                mind.threat_pos = None;
                mind.contagion_at = None;
                mind.flee_until = mind.flee_until.min(clock.now);
                if matches!(
                    mind.state,
                    AnimalState::Flee
                        | AnimalState::Alert
                        | AnimalState::Chase
                        | AnimalState::Charge
                        | AnimalState::Stalk
                ) {
                    mind.set(AnimalState::Graze, clock.now);
                }
            }
        }
    }
}
