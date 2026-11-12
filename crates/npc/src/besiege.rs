//! Getting at a foe gone to ground behind cover, as the difficulty tier allows
//! (`difficulty`, engine-design/npc-difficulty.md 4.2).
//!
//! A shooter whose foe ducked behind cover works out where it would show again: round each of
//! the cover's edges and over its top (its **peek points**, our reading of Killzone's line of
//! fire table), and which one it showed at last. Then, by the highest [`Tactic`] its tier and
//! skill allow:
//!
//! * [`Tactic::CloseOnHidden`] (Easy): it walks round the cover to a clear
//!   shot at where the foe hides.
//! * [`Tactic::HarassEdge`] (Medium, with the logic to stand off; a dull one closes in): it stays in the open with a line to that peek point and
//!   shoots at it now and then; when the foe shows it starts its shot from scratch, which a foe
//!   that only shows a moment is gone before.
//! * [`Tactic::CoverAndHold`] (Hard): it takes cover with a line to that peek point, draws and
//!   holds the draw on it, and looses as soon as it sees the foe show (its reaction time after),
//!   a shot ready before the foe is. Arms tire: it lowers after [`HOLD_MAX`] and draws again.
//! * [`Tactic::LobOver`] (Hard, a bow, accuracy 0.6): held [`LOB_AFTER`] without a show, it
//!   lobs the arrow over the cover onto the hiding spot instead (`lob.rs`), every [`LOB_EVERY`].
//!
//! The squad tactics of the higher tiers (a flusher called round the cover, volleys, timed
//! pushes) come with the squad coordinator (round D2); a squad's own suppress and flank roles
//! keep to their plan meanwhile.

use engine_core::prelude::*;
use engine_world::DifficultyTier;

use crate::agent::NpcWeapon;
use crate::difficulty::Tactic;
use crate::perception::Known;
use crate::skills::NpcSkills;
use crate::tactics::PositionWeights;
use crate::world_model::Sight;

/// How long after losing sight of it a shooter keeps besieging a foe's cover (s); then it
/// goes looking for it (`Search`).
pub const BESIEGE_SECS: f32 = 20.0;
/// A siege goes on while it is at least this sure the foe is there (it fades unseen).
pub const KEEP_AWARENESS: f32 = 0.4;
/// How soon after the foe ducks it starts (s): a foe only stepping behind something for a
/// moment is still fought as one in sight.
pub const BESIEGE_AFTER: f32 = 0.35;
/// The besieging score (the utility brain's), plus a tenth of its aggression: above a
/// lost foe's search (at most 0.62) and a fight with one just gone (at most 0.65).
pub const SCORE: f32 = 0.7;
/// Longest it holds a full draw (s, its clock) before lowering and drawing again.
pub const HOLD_MAX: f64 = 7.0;
/// How long it holds a draw with no show before lobbing (s).
pub const LOB_AFTER: f64 = 3.5;
/// Seconds between lobs.
pub const LOB_EVERY: f64 = 5.0;
/// Seconds between shots at the edge, harassing (the least; up to half again more).
pub const HARASS_EVERY: f64 = 2.6;
/// How long after it sees the foe show a held draw is loosed (s, on top of its reaction).
pub const LOOSE_SECS: f64 = 0.05;
/// Holding: the shot is kept this far ahead (s) so it never lands on its own.
pub const HOLD_SLACK: f64 = 0.1;

/// How far round each side and how high over its top a peek point is looked for (m), and in
/// what steps.
const SIDE_REACH: f64 = 4.8;
const TOP_REACH: f64 = 3.0;
const PEEK_STEP: f64 = 0.3;
/// How far through the cover a peek point's line must reach to be past it (m).
const PAST: f64 = 3.0;
/// A foe that moved more than this (m) from where it was hiding hides somewhere else.
const MOVED: f64 = 0.75;
/// A gap in the sightings longer than this (s) means it went out of sight and showed again.
const SIGHTING_GAP: f64 = 0.45;

/// A shooter's siege of a foe behind cover (kept on its `NpcBrain`).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Siege {
    pub target: Entity,
    /// Where the foe hides (feet), as far as it knows.
    pub at: DVec3,
    /// The way its cover faces from it (XZ).
    pub front: Vec3,
    /// Where it would show: round the cover's left edge, its right edge (looking out from the
    /// foe along `front`), and over its top (at head height).
    pub peeks: [Option<DVec3>; 3],
    /// The top of its cover (world height), where it can be seen over.
    pub top: Option<f64>,
    /// What it does about it.
    pub tactic: Tactic,
    /// And lobs, held long enough.
    pub lobs: bool,
    /// The peek point it watches (where the foe showed last).
    pub watch: DVec3,
    /// Holding a draw since (its clock).
    pub drawn_since: Option<f64>,
    /// Its next shot at the edge, harassing; its next lob (its clock).
    pub next_shot: f64,
    pub next_lob: f64,
}

impl Siege {
    /// Where its position search aims (as feet: it looks for a shot 1.2 m over them): low on
    /// the foe crouching where it hides when closing in (a line there clears the whole of it,
    /// with room to stop short of the spot), otherwise the watched peek point.
    pub fn aim_feet(&self) -> DVec3 {
        match self.tactic {
            Tactic::CloseOnHidden => self.at - DVec3::Y * 0.85,
            _ => self.watch - DVec3::Y * 1.2,
        }
    }
}

/// How it weighs spots for `tactic` (its aim is [`Siege::aim_feet`]): closing in, a clear
/// shot is all; harassing, a line to the edge from about where it stands, in the open or not;
/// holding, cover from where the foe would show with a line to it (standing).
pub fn weights(tactic: Tactic, logic: f32) -> PositionWeights {
    let l = logic.clamp(0.0, 1.0);
    match tactic {
        Tactic::CloseOnHidden => PositionWeights {
            range: 0.6,
            can_shoot: 2.6,
            travel: 0.8,
            spacing: 0.6,
            hazard: 1.2,
            stay: 0.4,
            ..Default::default()
        },
        Tactic::HarassEdge => PositionWeights {
            range: 0.3,
            can_shoot: 2.2,
            travel: 1.2,
            spacing: 0.6,
            hazard: 1.2,
            stay: 0.9,
            ..Default::default()
        },
        _ => PositionWeights {
            range: 0.4,
            cover: 2.0,
            can_shoot: 2.4,
            exposure: 0.8 * l,
            travel: 1.1,
            spacing: 0.6,
            hazard: 1.5,
            stay: 0.6,
            ..Default::default()
        },
    }
}

/// The range it keeps (m) for `tactic`, `now_at` off and with a weapon that likes `ideal`.
pub fn range_for(tactic: Tactic, now_at: f32, ideal: f32) -> f32 {
    match tactic {
        Tactic::CloseOnHidden => 7.0,
        Tactic::HarassEdge => now_at.clamp(8.0, ideal.max(8.0)),
        _ => ideal,
    }
}

/// The best tactic against a hidden foe that `tier` and `skills` allow, and whether it lobs
/// too (a bow only).
pub fn tactic_for(tier: DifficultyTier, skills: &NpcSkills, weapon: &NpcWeapon) -> (Tactic, bool) {
    let lobs = crate::intent_body::is_bow(weapon) && Tactic::LobOver.allowed(tier, skills);
    let t = if Tactic::CoverAndHold.allowed(tier, skills) {
        Tactic::CoverAndHold
    } else if Tactic::HarassEdge.allowed(tier, skills) {
        Tactic::HarassEdge
    } else {
        Tactic::CloseOnHidden
    };
    (t, lobs)
}

/// Where a foe hiding at `at` behind cover facing `front` would show itself: round each edge of
/// the cover at chest height (where its line along `front` gets past the cover, as long as
/// nothing stands between it and there), and over the cover's top (with the top's height).
pub fn peek_points(
    sight: &Sight,
    at: DVec3,
    front: Vec3,
    ignore: &[Entity],
) -> ([Option<DVec3>; 3], Option<f64>) {
    let f = front.as_dvec3();
    let side = DVec3::new(-f.z, 0.0, f.x);
    let chest = at + DVec3::Y * 1.2;
    let mut peeks = [None; 3];
    for (k, sgn) in [(0, 1.0), (1, -1.0)] {
        let mut x = PEEK_STEP;
        while x <= SIDE_REACH {
            let from = chest + side * (x * sgn);
            if !sight.clear(chest, from, ignore) {
                break;
            }
            if sight.clear(from, from + f * PAST, ignore) {
                // Its shoulder out past the edge.
                peeks[k] = Some(from + side * (0.2 * sgn) + DVec3::Y * 0.2);
                break;
            }
            x += PEEK_STEP;
        }
    }
    let mut top = None;
    let mut h = 1.0;
    while h <= TOP_REACH {
        let p = at + DVec3::Y * h;
        if sight.clear(p, p + f * PAST, ignore) {
            top = Some(p.y);
            peeks[2] = Some(DVec3::new(at.x, p.y + 0.15, at.z));
            break;
        }
        h += 0.2;
    }
    (peeks, top)
}

/// How long `k` has been in sight this time (s; `None` out of sight): since the first of its
/// latest unbroken run of sightings.
pub fn shown_for(k: &Known, now: f64) -> Option<f64> {
    if !k.visible {
        return None;
    }
    let mut first = k.history.back()?.time;
    for s in k.history.iter().rev().skip(1) {
        if first - s.time > SIGHTING_GAP {
            break;
        }
        first = s.time;
    }
    Some(now - first)
}

/// Keep `siege` up to date for a foe `k` it hasn't seen for `lost` seconds (worked out anew
/// when the foe hides somewhere else); `false` when it isn't besieging it (in sight and not
/// peeking from the cover it besieges, not hidden long enough or too long, or not behind
/// cover). `cover_front` finds the way the foe's cover faces.
#[allow(clippy::too_many_arguments)]
pub(crate) fn update(
    siege: &mut Option<Siege>,
    k: &Known,
    lost: f32,
    now: f64,
    (tier, skills, weapon): (DifficultyTier, &NpcSkills, &NpcWeapon),
    sight: &Sight,
    me: Entity,
    cover_front: impl Fn(&Sight, DVec3, &[Entity]) -> Option<Vec3>,
) -> bool {
    let (tactic, lobs) = tactic_for(tier, skills, weapon);
    let flat = |a: DVec3, b: DVec3| DVec2::new(a.x - b.x, a.z - b.z).length();
    let same = siege
        .as_ref()
        .is_some_and(|s| s.target == k.entity && flat(s.at, k.last_position) < MOVED);
    if k.visible {
        // Showing itself where it hides: still the siege (it looses at it).
        return same;
    }
    if lost > BESIEGE_SECS {
        *siege = None;
        return false;
    }
    if lost < BESIEGE_AFTER && !same {
        return false;
    }
    if same {
        let s = siege.as_mut().expect("same siege");
        s.tactic = tactic;
        s.lobs = lobs;
        return true;
    }
    let ignore = [me, k.entity];
    let Some(front) = cover_front(sight, k.last_position, &ignore) else {
        *siege = None;
        return false;
    };
    let (peeks, top) = peek_points(sight, k.last_position, front, &ignore);
    // Where it showed last: its head then.
    let head = k.history.back().map_or(k.last_position, |s| s.position) + DVec3::Y * 1.5;
    let Some(watch) = peeks
        .iter()
        .flatten()
        .copied()
        .min_by(|a, b| a.distance(head).total_cmp(&b.distance(head)))
    else {
        // Walled in all round: nothing to watch.
        *siege = None;
        return false;
    };
    let keep = siege.filter(|s| s.target == k.entity);
    *siege = Some(Siege {
        target: k.entity,
        at: k.last_position,
        front,
        peeks,
        top,
        tactic,
        lobs,
        watch,
        drawn_since: None,
        next_shot: keep.map_or(now, |s| s.next_shot),
        next_lob: keep.map_or(now + LOB_AFTER, |s| s.next_lob),
    });
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_tier_and_the_shooters_skill_pick_the_tactic() {
        use DifficultyTier::*;
        let bow = NpcWeapon::bow();
        let rifle = NpcWeapon::rifle();
        let master = NpcSkills::master();
        let rookie = NpcSkills::rookie();
        let pick = |t, s: &NpcSkills, w: &NpcWeapon| tactic_for(t, s, w);
        assert_eq!(pick(Easy, &master, &bow), (Tactic::CloseOnHidden, false));
        assert_eq!(pick(Medium, &master, &bow), (Tactic::HarassEdge, false));
        assert_eq!(pick(Hard, &master, &bow), (Tactic::CoverAndHold, true));
        assert_eq!(pick(Extreme, &master, &bow), (Tactic::CoverAndHold, true));
        // A rifle doesn't lob, and a rookie takes cover and holds but doesn't lob.
        assert_eq!(pick(Hard, &master, &rifle), (Tactic::CoverAndHold, false));
        assert_eq!(pick(Hard, &rookie, &bow), (Tactic::CoverAndHold, false));
        assert_eq!(pick(Medium, &rookie, &bow).0, Tactic::CloseOnHidden);
    }
}
