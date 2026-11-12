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
//! The higher tiers work as a squad ([`plan_push`], run by the squad coordinator; a squad's
//! own suppress and flank roles keep to their plan):
//!
//! * [`Tactic::CallFlusher`] (Very hard, teamwork 0.5): the archer calls a hand-to-hand mate
//!   round the cover ("Flush them out!", [`BarkLine::Flush`]). The mate goes round the edge
//!   nearest it ([`Order::Flush`]), and the archers hold their draw on the *other* edge, the
//!   way the foe will break.
//! * [`Tactic::Volley`] (Very hard, teamwork 0.5): when one archer looses at a show, the others
//!   holding on that foe loose with it at the same spot ([`Volleys`]), so dodging one arrow
//!   isn't enough.
//! * [`Tactic::CoordinatedPush`] (Extreme, logic 0.7): two mates, one round each edge. They
//!   wait out of sight ([`Order::Wait`]) while the archers draw and lob to soften the foe up,
//!   then go together. A push that fails (a flusher badly hurt, or no contact in
//!   [`PUSH_SECS`]) falls back to the archers ([`Order::FallBack`], [`BarkLine::FallBack`]) and
//!   tries again.
//!
//! How many go round is the tier's [`Tokens::flushers`](crate::difficulty::Tokens).

use engine_core::prelude::*;
use engine_world::DifficultyTier;

use bevy_platform::collections::HashMap;

use crate::agent::NpcWeapon;
use crate::bark::BarkLine;
use crate::difficulty::{Tactic, Tokens};
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
    let ours = siege.as_ref().is_some_and(|s| s.target == k.entity);
    let drawn = ours && siege.as_ref().is_some_and(|s| s.drawn_since.is_some());
    if k.visible {
        // Showing itself where it hides: still the siege (it looses at it); and breaking out
        // somewhere else with a draw held on it, until that is loosed.
        return same || drawn;
    }
    if lost > BESIEGE_SECS {
        *siege = None;
        return false;
    }
    // (Moved behind its cover, as a mate saw: the siege goes on from where it is now.)
    if lost < BESIEGE_AFTER && !same && !ours {
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
        // (A draw held on it is still held.)
        drawn_since: keep.and_then(|s| s.drawn_since),
        next_shot: keep.map_or(now, |s| s.next_shot),
        next_lob: keep.map_or(now + LOB_AFTER, |s| s.next_lob),
    });
    true
}

/// Waiting for the go, a push softens the foe up at least this long (s), and at most this
/// long while some archer isn't drawn yet.
pub const READY_SECS: f64 = 3.0;
const READY_MAX: f64 = 6.0;
/// A push that hasn't reached the foe in this long has failed (s); so has one whose flusher is
/// down to this share of its health.
pub const PUSH_SECS: f64 = 12.0;
const FALL_BACK_HEALTH: f32 = 0.5;
/// Mates not sent round stand this far in front of the archers, toward the foe (m).
const GUARD_AHEAD: f64 = 3.0;
/// Falling back lasts this long before the next try (s).
pub const FALL_BACK_SECS: f64 = 5.0;
/// Archers holding on a foe loose with a mate's shot at it this soon after (s): the call.
pub const VOLLEY_CALL: f64 = 0.12;
/// A volley called longer ago than this is over (s).
const VOLLEY_SECS: f64 = 0.5;

/// What a squad's plan against a foe behind cover asks of one member.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Order {
    /// Go round the cover's edge to `goal` and take the foe on.
    Flush { foe: Entity, goal: DVec3 },
    /// Keep out of the foe's sight until the push goes.
    Wait { foe: Entity },
    /// The push failed: back to `to` (the archers).
    FallBack { foe: Entity, to: DVec3 },
    /// Not sent: stand at `at`, in front of the archers, and take on the foe only if it comes
    /// out at them.
    Guard { foe: Entity, at: DVec3 },
    /// An archer holding on the foe: watch `watch` (where the flushers drive it out), lob now
    /// to soften it up (`soften`), or don't lob at all while mates are going in
    /// (`flushing`: an arrow could come down on them, and the draw is wanted for the break).
    Hold {
        foe: Entity,
        watch: Option<DVec3>,
        soften: bool,
        flushing: bool,
    },
}

impl Order {
    pub fn foe(&self) -> Entity {
        match *self {
            Order::Flush { foe, .. }
            | Order::Wait { foe }
            | Order::FallBack { foe, .. }
            | Order::Guard { foe, .. }
            | Order::Hold { foe, .. } => foe,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) enum Phase {
    /// Waiting for the go (a coordinated push).
    #[default]
    Ready,
    Go,
    FallBack,
}

/// A squad's plan against one foe behind cover, kept from one planning pass to the next.
#[derive(Debug, Clone, Default)]
pub(crate) struct Push {
    pub phase: Phase,
    /// World clock (s) the phase began.
    pub since: f64,
    pub flushers: Vec<Entity>,
    pub tries: u32,
}

/// An archer of the squad besieging the foe.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Holder {
    pub e: Entity,
    pub pos: DVec3,
    pub skills: NpcSkills,
    pub tier: DifficultyTier,
    pub siege: Siege,
}

/// A hand-to-hand member of the squad that could be sent round.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Mate {
    pub e: Entity,
    pub pos: DVec3,
    /// Health left, 0..1.
    pub health: f32,
}

/// A squad's push: the plan kept for next time, each member's order, and who shouts what.
pub(crate) type PushPlan = (Push, Vec<(Entity, Order)>, Vec<(Entity, BarkLine)>);

/// The squad's plan against the foe its archers `holders` besiege (all the same foe), with its
/// hand-to-hand mates `mates` (free to go: not guarding a mate): the plan kept for next time,
/// each member's order and who shouts what. `None` while no archer may call a flusher, or
/// there's no one to send (see the module docs).
pub(crate) fn plan_push(
    holders: &[Holder],
    mates: &[Mate],
    had: Option<Push>,
    now: f64,
) -> Option<PushPlan> {
    let caller = holders
        .iter()
        .filter(|h| Tactic::CallFlusher.allowed(h.tier, &h.skills))
        .max_by(|a, b| a.skills.teamwork.total_cmp(&b.skills.teamwork))?;
    let want = Tokens::for_tier(caller.tier).flushers as usize;
    if want == 0 {
        return None;
    }
    let push = Tactic::CoordinatedPush.allowed(caller.tier, &caller.skills);
    let siege = caller.siege;
    let foe = siege.target;
    let flat = |a: DVec3, b: DVec3| DVec2::new(a.x - b.x, a.z - b.z).length();
    // Those already going keep at it; the nearest others join, up to the tier's count.
    let had = had.unwrap_or(Push {
        phase: if push { Phase::Ready } else { Phase::Go },
        since: now,
        flushers: Vec::new(),
        tries: 0,
    });
    let fresh = had.flushers.is_empty();
    let mut flushers: Vec<Entity> = had
        .flushers
        .iter()
        .copied()
        .filter(|e| mates.iter().any(|m| m.e == *e))
        .collect();
    let mut others: Vec<&Mate> = mates.iter().filter(|m| !flushers.contains(&m.e)).collect();
    others.sort_by(|a, b| flat(a.pos, siege.at).total_cmp(&flat(b.pos, siege.at)));
    for m in others {
        if flushers.len() >= want {
            break;
        }
        flushers.push(m.e);
    }
    flushers.truncate(want);
    if flushers.is_empty() {
        return None;
    }
    let mate = |e: Entity| mates.iter().find(|m| m.e == e).copied();
    let mut barks = Vec::new();
    let mut next = Push { flushers, ..had };
    let go = |barks: &mut Vec<(Entity, BarkLine)>, next: &Push| {
        barks.push((caller.e, BarkLine::Flush));
        for &f in &next.flushers {
            barks.push((f, BarkLine::GoingRound));
        }
    };
    if fresh && next.phase == Phase::Go {
        go(&mut barks, &next);
    }
    let held = now - next.since;
    match next.phase {
        Phase::Ready => {
            let drawn = holders
                .iter()
                .all(|h| h.siege.tactic != Tactic::CoverAndHold || h.siege.drawn_since.is_some());
            if (held >= READY_SECS && drawn) || held >= READY_MAX {
                next.phase = Phase::Go;
                next.since = now;
                go(&mut barks, &next);
            }
        }
        Phase::Go => {
            let hurt = next
                .flushers
                .iter()
                .any(|&f| mate(f).is_some_and(|m| m.health < FALL_BACK_HEALTH));
            if push && (hurt || held > PUSH_SECS) {
                next.phase = Phase::FallBack;
                next.since = now;
                barks.push((caller.e, BarkLine::FallBack));
            }
        }
        Phase::FallBack => {
            if held > FALL_BACK_SECS {
                next.phase = Phase::Ready;
                next.since = now;
                next.tries += 1;
            }
        }
    }
    // Which edge each goes round: one each when two go and there are two; else the nearest.
    let edges: Vec<(usize, DVec3)> = (0..2)
        .filter_map(|i| siege.peeks[i].map(|p| (i, p)))
        .collect();
    let mut taken = [false; 2];
    let mut orders = Vec::new();
    for (i, &f) in next.flushers.iter().enumerate() {
        let Some(m) = mate(f) else { continue };
        let edge = if next.flushers.len() >= 2 && edges.len() == 2 {
            Some(edges[i % 2])
        } else {
            edges
                .iter()
                .copied()
                .min_by(|a, b| flat(a.1, m.pos).total_cmp(&flat(b.1, m.pos)))
        };
        if let Some((k, _)) = edge {
            taken[k] = true;
        }
        let goal = edge.map_or(siege.at, |(_, p)| DVec3::new(p.x, siege.at.y, p.z));
        let order = match next.phase {
            Phase::Ready => Order::Wait { foe },
            Phase::Go => Order::Flush { foe, goal },
            Phase::FallBack => Order::FallBack {
                foe,
                to: caller.pos,
            },
        };
        orders.push((f, order));
    }
    // The rest stand guard in front of the archers (only so many go in).
    let to_foe = {
        let d = DVec2::new(siege.at.x - caller.pos.x, siege.at.z - caller.pos.z);
        d.normalize_or_zero() * GUARD_AHEAD
    };
    // (Off to one side, clear of the archers' line.)
    let aside = DVec2::new(-to_foe.y, to_foe.x) * 0.5;
    let guard = caller.pos + DVec3::new(to_foe.x + aside.x, 0.0, to_foe.y + aside.y);
    for m in mates.iter().filter(|m| !next.flushers.contains(&m.e)) {
        orders.push((m.e, Order::Guard { foe, at: guard }));
    }
    // The archers hold on the edge the foe will break to: the one nobody comes round.
    let watch = (next.phase == Phase::Go)
        .then(|| {
            let free: Vec<DVec3> = edges.iter().filter(|e| !taken[e.0]).map(|e| e.1).collect();
            (free.len() == 1 && taken.iter().any(|&t| t)).then(|| free[0])
        })
        .flatten();
    for h in holders {
        orders.push((
            h.e,
            Order::Hold {
                foe,
                watch,
                soften: push && next.phase == Phase::Ready,
                flushing: next.phase == Phase::Go,
            },
        ));
    }
    Some((next, orders, barks))
}

/// Volleys called in the last moment, per foe: (world clock s, faction, where to loose).
#[derive(Resource, Debug, Default, Clone)]
pub struct Volleys {
    pub called: HashMap<Entity, (f64, crate::faction::Faction, DVec3)>,
}

impl Volleys {
    /// Where to loose with a volley `faction` called at `foe`, and when (world clock s).
    pub fn joining(
        &self,
        foe: Entity,
        faction: crate::faction::Faction,
        now: f64,
    ) -> Option<(DVec3, f64)> {
        self.called
            .get(&foe)
            .filter(|v| v.1 == faction && now - v.0 < VOLLEY_SECS)
            .map(|v| (v.2, v.0 + VOLLEY_CALL))
    }
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

    #[test]
    fn a_push_waits_goes_round_both_edges_falls_back_and_tries_again() {
        use DifficultyTier::*;
        let e = |i| Entity::from_raw_u32(i).unwrap();
        let siege = Siege {
            target: e(1),
            at: DVec3::ZERO,
            front: Vec3::Z,
            peeks: [
                Some(DVec3::new(-2.6, 1.4, 0.0)),
                Some(DVec3::new(2.6, 1.4, 0.0)),
                Some(DVec3::new(0.0, 1.55, 0.0)),
            ],
            top: Some(1.4),
            tactic: Tactic::CoverAndHold,
            lobs: true,
            watch: DVec3::new(0.0, 1.55, 0.0),
            drawn_since: Some(0.0),
            next_shot: 0.0,
            next_lob: 0.0,
        };
        let holder = |tier| Holder {
            e: e(2),
            pos: DVec3::new(0.0, 0.0, 15.0),
            skills: NpcSkills::master(),
            tier,
            siege,
        };
        let mate = |i, x: f64, health| Mate {
            e: e(i),
            pos: DVec3::new(x, 0.0, 12.0),
            health,
        };
        let mates = [mate(3, -3.0, 1.0), mate(4, 3.0, 1.0), mate(5, 0.0, 1.0)];
        // Hard: nobody is sent.
        assert!(plan_push(&[holder(Hard)], &mates, None, 0.0).is_none());
        // Very hard: the nearest goes at once round its own edge, the others guard, and the
        // archer holds on the other edge.
        let (p, orders, barks) = plan_push(&[holder(VeryHard)], &mates, None, 0.0).unwrap();
        assert_eq!(p.phase, Phase::Go);
        let order = |orders: &[(Entity, Order)], i| orders.iter().find(|o| o.0 == e(i)).unwrap().1;
        let Order::Flush { goal, .. } = order(&orders, 5) else {
            panic!("{orders:?}")
        };
        assert!(matches!(order(&orders, 3), Order::Guard { .. }));
        let Order::Hold { watch: Some(w), .. } = order(&orders, 2) else {
            panic!("{orders:?}")
        };
        assert!(goal.x * w.x < 0.0, "{goal} {w}");
        assert!(barks.contains(&(e(2), BarkLine::Flush)));
        assert!(barks.contains(&(e(5), BarkLine::GoingRound)));
        // Extreme: two wait while the archer softens up, then go round both edges.
        let (p, orders, barks) = plan_push(&[holder(Extreme)], &mates, None, 0.0).unwrap();
        assert_eq!(p.phase, Phase::Ready);
        assert!(barks.is_empty());
        assert_eq!(p.flushers.len(), 2);
        assert!(matches!(
            order(&orders, 2),
            Order::Hold { soften: true, .. }
        ));
        let (p, orders, _) =
            plan_push(&[holder(Extreme)], &mates, Some(p), READY_SECS + 0.1).unwrap();
        assert_eq!(p.phase, Phase::Go);
        let goals: Vec<f64> = p
            .flushers
            .iter()
            .map(|&f| match order(&orders, f.index_u32()) {
                Order::Flush { goal, .. } => goal.x,
                o => panic!("{o:?}"),
            })
            .collect();
        assert!(goals[0] * goals[1] < 0.0, "{goals:?}");
        // A flusher badly hurt: fall back, then try again.
        let hurt: Vec<Mate> = mates
            .iter()
            .map(|m| Mate {
                health: if m.e == p.flushers[0] { 0.3 } else { 1.0 },
                ..*m
            })
            .collect();
        let (p, _, barks) = plan_push(&[holder(Extreme)], &hurt, Some(p), 5.0).unwrap();
        assert_eq!(p.phase, Phase::FallBack);
        assert!(barks.contains(&(e(2), BarkLine::FallBack)));
        let (p, _, _) = plan_push(
            &[holder(Extreme)],
            &mates,
            Some(p),
            5.0 + FALL_BACK_SECS + 0.1,
        )
        .unwrap();
        assert_eq!((p.phase, p.tries), (Phase::Ready, 1));
    }
}
