//! Two hand-to-hand fighters on one foe working as a pair, on the higher tiers (the fighter
//! column of engine-design/npc-difficulty.md 4.3; The Last of Us' and Sifu's mobs, which
//! never queue up in front of you).
//!
//! * [`Tactic::PinAndFlank`] (Very hard, teamwork 0.4): of two squad mates fighting one foe,
//!   the one it faces more **holds** it, just out of its reach, and the other goes round, wider,
//!   to where it isn't looking, and cuts in from behind it, or as the holder's cut comes
//!   ([`Beats`]), or while it swings at the holder. The roles swap as the foe turns: it can
//!   only face one of them. The one in front when they pair up shouts it ("Get behind them!",
//!   [`BarkLine::Flank`]), so the player hears the second one coming.
//! * [`Tactic::Bait`] (Extreme, logic 0.6): once its mate is placed behind the foe, the holder
//!   steps in with a feint to draw the foe's guard, and the mate cuts in on that beat.
//!
//! [`plan_pair`] runs in the squad coordinator (`squad::plan_squads`); the fighters act on
//! their [`PairRole`] in `brain::melee`.

use engine_core::prelude::*;
use engine_world::DifficultyTier;

use crate::bark::BarkLine;
use crate::difficulty::Tactic;
use crate::skills::NpcSkills;

/// How near the foe both must be to pair up (m, flat).
pub const PAIR_RANGE: f64 = 9.0;
/// The one of a pair the foe faces less cuts in once it is this far round (dot of the foe's
/// facing with the way from it to the cutter: -0.5 is 120 degrees round).
pub const CUT_FRONT: f32 = -0.5;
/// A mate's facing note is fresh this long (s).
pub const FRESH_SECS: f64 = 0.25;

/// What a pair's fighters tell each other, by the frame: when each one's swing or feint at its
/// foe lands (world clock, s), by foe, so the other can cut in on the same beat; and how much
/// in front of its foe each stands and whether it could swing (when noted, [`front`]'s dot,
/// ready), by fighter.
#[derive(Resource, Debug, Default)]
pub struct Beats {
    pub(crate) at: bevy_platform::collections::HashMap<Entity, f64>,
    pub(crate) front: bevy_platform::collections::HashMap<Entity, (f64, f32, bool)>,
}

/// A cutter goes in when its mate's swing lands within this (s): its own then lands with it.
pub const BEAT_SECS: f64 = 0.45;

/// What a fighter of a pair does (see the module docs).
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum PairRole {
    /// In front of `foe` when they paired up: it holds it first, with `mate` going round.
    Pin { foe: Entity, mate: Entity },
    /// Goes round `foe` first, with `mate` holding it.
    Flank { foe: Entity, mate: Entity },
}

impl PairRole {
    pub fn foe(&self) -> Entity {
        match *self {
            PairRole::Pin { foe, .. } | PairRole::Flank { foe, .. } => foe,
        }
    }

    pub fn mate(&self) -> Entity {
        match *self {
            PairRole::Pin { mate, .. } | PairRole::Flank { mate, .. } => mate,
        }
    }

    pub fn name(&self) -> &'static str {
        match self {
            PairRole::Pin { .. } => "pin",
            PairRole::Flank { .. } => "behind",
        }
    }
}

/// A squad's hand-to-hand fighter on one foe, for [`plan_pair`].
#[derive(Debug, Clone, Copy)]
pub(crate) struct Duelist {
    pub e: Entity,
    pub pos: DVec3,
    pub skills: NpcSkills,
    pub tier: DifficultyTier,
    /// Where the foe is and which way it faces (flat), as this one last saw it.
    pub foe_pos: DVec3,
    pub foe_forward: Vec3,
}

/// A pair kept between planning passes.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct Pair {
    pub pin: Entity,
    pub flank: Entity,
    /// World clock it was last planned (s): a pair that misses a pass or two (one of them
    /// loses sight a moment) is kept [`PAIR_KEEP`].
    pub at: f64,
}

/// How long a pair is kept through passes it can't be planned (s).
pub const PAIR_KEEP: f64 = 3.0;

/// A pair, each one's role, and what the first one shouts when the pair is new.
pub(crate) type PairPlan = (Pair, [(Entity, PairRole); 2], Option<(Entity, BarkLine)>);

/// How much in front of the foe `d` stands (1 face to face, -1 behind it).
pub(crate) fn front(d: &Duelist) -> f32 {
    let to = d.pos - d.foe_pos;
    let to = Vec3::new(to.x as f32, 0.0, to.z as f32).normalize_or_zero();
    d.foe_forward.dot(to)
}

/// The pair against one foe among a squad's `duelists` on it, keeping the one `had` while both
/// are still there; each one's role, and what the pin shouts when the pair is new. `None` when
/// fewer than two may pair (tier, teamwork, range).
pub(crate) fn plan_pair(
    duelists: &[Duelist],
    foe: Entity,
    had: Option<Pair>,
    now: f64,
) -> Option<PairPlan> {
    let flat = |a: DVec3, b: DVec3| DVec2::new(a.x - b.x, a.z - b.z).length();
    let able: Vec<&Duelist> = duelists
        .iter()
        .filter(|d| Tactic::PinAndFlank.allowed(d.tier, &d.skills))
        .filter(|d| flat(d.pos, d.foe_pos) < PAIR_RANGE)
        .collect();
    if able.len() < 2 {
        return None;
    }
    let find = |e: Entity| able.iter().copied().find(|d| d.e == e);
    let kept = had
        .filter(|p| now - p.at < PAIR_KEEP)
        .and_then(|p| Some((find(p.pin)?, find(p.flank)?)));
    let (pin, flank) = match kept {
        Some(p) => p,
        None => {
            // The one most in front of the foe pins it; the nearest of the rest goes round.
            let pin = *able
                .iter()
                .max_by(|a, b| front(a).total_cmp(&front(b)).then(b.e.cmp(&a.e)))
                .expect("two or more");
            let flank = *able
                .iter()
                .filter(|d| d.e != pin.e)
                .min_by(|a, b| {
                    flat(a.pos, a.foe_pos)
                        .total_cmp(&flat(b.pos, b.foe_pos))
                        .then(a.e.cmp(&b.e))
                })
                .expect("one more");
            (pin, flank)
        }
    };
    let pair = Pair {
        pin: pin.e,
        flank: flank.e,
        at: now,
    };
    let roles = [
        (pin.e, PairRole::Pin { foe, mate: flank.e }),
        (flank.e, PairRole::Flank { foe, mate: pin.e }),
    ];
    let bark = kept.is_none().then_some((pin.e, BarkLine::Flank));
    Some((pair, roles, bark))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn duelist(e: u32, pos: DVec3, tier: DifficultyTier) -> Duelist {
        Duelist {
            e: Entity::from_raw_u32(e).unwrap(),
            pos,
            skills: NpcSkills::veteran(),
            tier,
            foe_pos: DVec3::ZERO,
            foe_forward: Vec3::Z,
        }
    }

    #[test]
    fn the_one_in_front_pins_and_the_other_goes_behind_on_very_hard_only() {
        let foe = Entity::from_raw_u32(99).unwrap();
        let a = duelist(1, DVec3::new(0.5, 0.0, 3.0), DifficultyTier::VeryHard);
        let b = duelist(2, DVec3::new(3.0, 0.0, 1.0), DifficultyTier::VeryHard);
        let (pair, roles, bark) = plan_pair(&[b, a], foe, None, 0.0).expect("a pair");
        assert_eq!(pair.pin, a.e);
        assert_eq!(pair.flank, b.e);
        assert!(matches!(roles[0].1, PairRole::Pin { mate, .. } if mate == b.e));
        assert_eq!(bark, Some((a.e, BarkLine::Flank)));
        // Kept next pass, without a second shout, even once the flanker is in front.
        let mut b2 = b;
        b2.pos = DVec3::new(0.0, 0.0, 2.0);
        let (again, _, bark) = plan_pair(&[a, b2], foe, Some(pair), 1.0).unwrap();
        assert_eq!((again.pin, again.flank), (pair.pin, pair.flank));
        assert_eq!(bark, None);
        // Not on Hard, and not with a rookie's teamwork.
        let h = |d: Duelist| Duelist {
            tier: DifficultyTier::Hard,
            ..d
        };
        assert!(plan_pair(&[h(a), h(b)], foe, None, 0.0).is_none());
        let mut r = b;
        r.skills.teamwork = 0.2;
        assert!(plan_pair(&[a, r], foe, None, 0.0).is_none());
    }
}
