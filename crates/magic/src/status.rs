//! What spells leave on whoever they hit: element gauges that react, and the statuses each
//! element brings (research-games.md, Genshin Impact, Pillars of Eternity II, Dota).
//!
//! - **Gauges.** An element that lands on a body with no other element sits there as an aura
//!   of 0.8 of its units, fading over 2.5·U + 7 seconds. The next different element reacts
//!   with it through the combination table (fire on a frosted body: Steam) and uses it up.
//!   The order matters, as in Genshin: the active element (earlier in [`Element::ALL`]) on a
//!   passive aura gets the recipe's full bonus; the reverse gets a little over half of it. No
//!   reaction is worth more than 2x.
//! - **Internal cooldown.** A multi-hit carrier (a beam, a field's ticks, an orbit, rain)
//!   applies its element on the first hit, then once every three hits or 2.5 seconds, so
//!   fast ticks can't set off a reaction every tenth of a second.
//! - **Statuses.** Burning, Bleeding (worse while moving) and Rot (no healing) hurt over
//!   time; Chilled slows 8% a stack and freezes at the spell's threshold (5 by default), with
//!   each freeze in quick succession half as long as the last; Shocked stuns for a moment and
//!   draws chains; Dread frightens. Frozen bodies shatter under a heavy hit.

use engine_core::math::DVec3;
use engine_core::prelude::*;
use engine_world::{DamageEvent, DamageKind, Statuses};
use serde::Serialize;

use crate::combine::{Recipe, recipe_for};
use crate::runtime::{FollowLook, MagicQueue, Spawn};
use crate::schools::{Element, Status};

/// Seconds a multi-hit carrier's element waits before applying again.
pub const ICD_SECS: f32 = 2.5;
/// ...or every this many hits, whichever comes first.
pub const ICD_HITS: u32 = 3;
/// Share of an element's units that stays as an aura.
pub const AURA_TAX: f32 = 0.8;
/// Seconds between damage-over-time ticks.
pub const DOT_TICK: f32 = 0.5;
/// No reaction multiplies a hit by more than this.
pub const MAX_REACTION: f32 = 2.0;

/// One element sitting on a body.
#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
pub struct Gauge {
    pub element: Element,
    pub units: f32,
    /// Units lost per second.
    pub decay: f32,
}

/// Damage over time.
#[derive(Debug, Clone, Copy, PartialEq, Default, Serialize)]
pub struct Dot {
    /// Damage per second.
    pub dps: f32,
    /// Seconds left.
    pub secs: f32,
}

impl Dot {
    pub fn active(&self) -> bool {
        self.secs > 0.0 && self.dps > 0.0
    }

    fn add(&mut self, dps: f32, secs: f32) {
        self.dps = if self.secs > 0.0 {
            self.dps.max(dps)
        } else {
            dps
        };
        self.secs = self.secs.max(secs);
    }
}

/// Something worth showing that a status change caused.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StatusChange {
    /// Caught fire.
    Ignited,
    /// Chill reached its threshold.
    Froze,
    Shocked,
}

/// Every element and status on one body. Added by the first spell that touches it.
#[derive(Component, Debug, Clone, Default, Serialize)]
pub struct Afflictions {
    pub gauges: Vec<Gauge>,
    /// Internal cooldowns: element, seconds left, hits counted.
    #[serde(skip)]
    icd: Vec<(Element, f32, u32)>,
    /// Chill stacks.
    pub chill: f32,
    /// Seconds before chill starts to wear off.
    pub chill_hold: f32,
    /// Seconds left frozen.
    pub frozen: f32,
    /// Recent freezes (diminishing returns), wears off one per 10 s.
    pub freezes: f32,
    /// Seconds left stunned by a shock.
    pub shocked: f32,
    pub burning: Dot,
    pub bleeding: Dot,
    pub rot: Dot,
    /// Seconds left terrified.
    pub dread: f32,
    /// Where the terror came from (its caster, else where it struck): the terrified run from
    /// it (minds.rs).
    #[serde(skip)]
    pub dread_from: Option<DVec3>,
    /// Damage a ward will still absorb.
    pub ward: f32,
    pub ward_secs: f32,
    /// The ward's pool when it went up (its cracks show the share left).
    pub ward_full: f32,
    /// The element the ward is woven with (it soaks it, takes double from its opposite).
    pub ward_element: Option<Element>,
    /// Seconds before a broken ward may go up again.
    pub ward_lock: f32,
    /// Damage took the last of the ward this step: its shell shatters (wards.rs clears it).
    pub ward_broke: bool,
    /// Seconds left staggering (a held shield broke in its hands).
    pub staggered: f32,
    /// Holding a shield up: [`crate::wards::hold_shields`] keeps this a moment ahead while it
    /// rides before the body, so it lapses a step or two after the shield goes.
    pub shielding: f32,
    /// Haste or slow: time factor and seconds left.
    pub tempo: Option<(f32, f32)>,
    /// Seconds left unseen.
    pub veiled: f32,
    /// Seconds left calmed.
    pub calmed: f32,
    /// Seconds left held by a stone grip (fire cracks it early).
    pub rooted: f32,
    /// Who last hurt it (damage over time is theirs).
    #[serde(skip)]
    pub source: Option<Entity>,
    /// The time scale this module set on the body, if any.
    #[serde(skip)]
    pub applied_scale: Option<f32>,
    #[serde(skip)]
    pub last_pos: Option<DVec3>,
    /// Ground speed, m/s (bleeding is worse moving).
    pub speed: f32,
    #[serde(skip)]
    dot_timer: f32,
    /// Statuses with a look following the body (bit per [`Status`]).
    #[serde(skip)]
    pub shown: u8,
}

/// The element order that decides which side of a reaction is the stronger.
fn rank(e: Element) -> usize {
    Element::ALL.iter().position(|x| *x == e).unwrap_or(0)
}

/// The recipe two elements make.
pub fn pair_recipe(a: Element, b: Element) -> Option<&'static Recipe> {
    recipe_for(&[a.key().to_string(), b.key().to_string()])
}

fn bit(s: Status) -> u8 {
    1 << (s as u8)
}

impl Afflictions {
    /// Whether an element from this hit applies now (the internal cooldown for multi-hit
    /// carriers; single hits always apply).
    pub fn gate(&mut self, element: Element, multi: bool) -> bool {
        if !multi {
            return true;
        }
        match self.icd.iter_mut().find(|(e, _, _)| *e == element) {
            None => {
                self.icd.push((element, ICD_SECS, 1));
                true
            }
            Some(entry) if entry.1 <= 0.0 => {
                *entry = (element, ICD_SECS, 1);
                true
            }
            Some(entry) => {
                entry.2 += 1;
                entry.2 % ICD_HITS == 1
            }
        }
    }

    /// An element lands: it reacts with another element already here (returning the recipe
    /// and the hit's multiplier), or stays as an aura.
    pub fn react(&mut self, incoming: Element, units: f32) -> Option<(&'static Recipe, f32)> {
        let other = self
            .gauges
            .iter()
            .enumerate()
            .filter(|(_, g)| g.element != incoming && g.units > 0.0)
            .filter(|(_, g)| pair_recipe(incoming, g.element).is_some())
            .max_by(|a, b| a.1.units.total_cmp(&b.1.units))
            .map(|(i, _)| i);
        let Some(i) = other else {
            self.add_aura(incoming, units);
            return None;
        };
        let aura = self.gauges[i];
        let recipe = pair_recipe(incoming, aura.element)?;
        let strong = rank(incoming) < rank(aura.element);
        let bonus = recipe.reaction.bonus.max(1.0);
        let mult = if strong {
            bonus
        } else {
            1.0 + (bonus - 1.0) * 0.6
        }
        .min(MAX_REACTION);
        self.gauges[i].units -= units * if strong { 2.0 } else { 0.5 };
        self.gauges.retain(|g| g.units > 0.0);
        Some((recipe, mult))
    }

    fn add_aura(&mut self, element: Element, units: f32) {
        let u = units * AURA_TAX;
        let decay = u / (2.5 * units + 7.0);
        match self.gauges.iter_mut().find(|g| g.element == element) {
            Some(g) => {
                g.units = g.units.max(u);
                g.decay = decay;
            }
            None => self.gauges.push(Gauge {
                element,
                units: u,
                decay,
            }),
        }
    }

    /// Whether an element sits on the body.
    pub fn has_aura(&self, e: Element) -> bool {
        self.gauges.iter().any(|g| g.element == e && g.units > 0.0)
    }

    /// Add a status. `dps` is the damage the hit did (damage over time is a share of it);
    /// `threshold` the chill stacks that freeze.
    pub fn add(
        &mut self,
        s: Status,
        stacks: f32,
        dps: f32,
        threshold: u32,
    ) -> Option<StatusChange> {
        let stacks = stacks.max(0.0);
        match s {
            Status::Burning => {
                let was = self.burning.active();
                self.burning
                    .add((dps * 0.25 * stacks.min(3.0)).max(1.0), 4.0);
                (!was).then_some(StatusChange::Ignited)
            }
            Status::Chilled => {
                if self.frozen > 0.0 {
                    return None;
                }
                self.chill += stacks;
                self.chill_hold = 3.0;
                if self.chill >= threshold as f32 {
                    self.freeze();
                    return Some(StatusChange::Froze);
                }
                None
            }
            Status::Frozen => {
                if self.frozen > 0.0 {
                    return None;
                }
                self.freeze();
                Some(StatusChange::Froze)
            }
            Status::Shocked => {
                let was = self.shocked > 0.0;
                self.shocked = self.shocked.max(0.35 + 0.15 * stacks.min(3.0));
                (!was).then_some(StatusChange::Shocked)
            }
            Status::Bleeding => {
                self.bleeding
                    .add((dps * 0.2 * stacks.min(3.0)).max(1.0), 5.0);
                None
            }
            Status::Rot => {
                self.rot.add((dps * 0.15 * stacks.min(3.0)).max(1.0), 6.0);
                None
            }
            Status::Dread => {
                self.dread = self.dread.max(3.0 * stacks.min(2.0));
                None
            }
        }
    }

    fn freeze(&mut self) {
        // Diminishing returns: each freeze soon after another lasts half as long.
        self.frozen = (3.0 * 0.5f32.powf(self.freezes)).max(0.6);
        self.freezes += 1.0;
        self.chill = 0.0;
        self.burning = Dot::default();
    }

    /// A heavy blow on a frozen body shatters the ice; true if it did.
    pub fn shatter(&mut self) -> bool {
        if self.frozen > 0.0 {
            self.frozen = 0.0;
            self.gauges.retain(|g| g.element != Element::Frost);
            true
        } else {
            false
        }
    }

    /// Steam: puts out fire and thaws chill.
    pub fn quench(&mut self) {
        self.burning = Dot::default();
        self.chill = 0.0;
        self.gauges
            .retain(|g| g.element != Element::Fire && g.element != Element::Frost);
    }

    /// Whether a status is on the body.
    pub fn has(&self, s: Status) -> bool {
        match s {
            Status::Burning => self.burning.active(),
            Status::Chilled => self.chill >= 0.5 && self.frozen <= 0.0,
            Status::Frozen => self.frozen > 0.0,
            Status::Shocked => self.shocked > 0.0,
            Status::Bleeding => self.bleeding.active(),
            Status::Rot => self.rot.active(),
            Status::Dread => self.dread > 0.0,
        }
    }

    /// Chains leap to shocked and bleeding bodies, and to bodies with storm or blood on them,
    /// before the nearest.
    pub fn primed_for_chain(&self) -> bool {
        self.shocked > 0.0
            || self.bleeding.active()
            || self.has_aura(Element::Storm)
            || self.has_aura(Element::Blood)
    }

    /// Rot stops healing.
    pub fn heal_blocked(&self) -> bool {
        self.rot.active()
    }

    /// Damage left after the ward takes its share.
    pub fn absorb(&mut self, damage: f32) -> f32 {
        self.absorb_hit(damage, None).0
    }

    /// A hit of `damage` of `element` meets the ward: the damage that gets through and the
    /// pool it cost. A ward soaks its own element and takes double from its opposite
    /// (`wards::ward_cost`); a hit that takes the last of it breaks it, and what the pool could
    /// not cover goes through.
    pub fn absorb_hit(&mut self, damage: f32, element: Option<Element>) -> (f32, f32) {
        if self.ward <= 0.0 || damage <= 0.0 {
            return (damage, 0.0);
        }
        let cost = crate::wards::ward_cost(damage, element, self.ward_element);
        if cost < self.ward {
            self.ward -= cost;
            return (0.0, cost);
        }
        let covered = self.ward / cost;
        let taken = self.ward;
        self.ward = 0.0;
        self.ward_secs = 0.0;
        self.ward_broke = true;
        self.ward_lock = crate::wards::LOCK_SECS;
        (damage * (1.0 - covered), taken)
    }

    /// Put up a ward of `pool` woven with `element`, unless a broken one still locks it out.
    pub fn raise_ward(&mut self, pool: f32, element: Option<Element>) -> bool {
        if self.ward_lock > 0.0 || pool <= 0.0 {
            return false;
        }
        if pool >= self.ward || element != self.ward_element {
            self.ward = pool;
            self.ward_full = pool;
            self.ward_element = element;
        }
        self.ward_secs = crate::wards::SHELL_SECS;
        true
    }

    /// The time scale the statuses put on the body: frozen almost still, shocked stunned,
    /// chilled slower, hasted or slowed by Time.
    pub fn time_scale(&self) -> f32 {
        if self.frozen > 0.0 {
            return 0.02;
        }
        if self.shocked > 0.0 {
            return 0.15;
        }
        if self.staggered > 0.0 {
            return crate::wards::STAGGER_SCALE;
        }
        let chill = (1.0 - 0.08 * self.chill).clamp(0.55, 1.0);
        let tempo = self.tempo.map(|(f, _)| f).unwrap_or(1.0);
        chill * tempo
    }

    /// Advance timers by `dt`; returns the damage over time due this step (paid every
    /// [`DOT_TICK`]).
    pub fn tick(&mut self, dt: f32) -> f32 {
        for g in &mut self.gauges {
            g.units -= g.decay * dt;
        }
        self.gauges.retain(|g| g.units > 0.0);
        for c in &mut self.icd {
            c.1 -= dt;
        }
        self.icd.retain(|c| c.1 > -1.0);
        if self.chill_hold > 0.0 {
            self.chill_hold -= dt;
        } else {
            self.chill = (self.chill - dt * 0.5).max(0.0);
        }
        self.frozen = (self.frozen - dt).max(0.0);
        self.freezes = (self.freezes - dt / 10.0).max(0.0);
        self.shocked = (self.shocked - dt).max(0.0);
        self.dread = (self.dread - dt).max(0.0);
        self.veiled = (self.veiled - dt).max(0.0);
        self.calmed = (self.calmed - dt).max(0.0);
        self.rooted = (self.rooted - dt).max(0.0);
        if self.ward_secs > 0.0 {
            self.ward_secs -= dt;
            if self.ward_secs <= 0.0 {
                self.ward = 0.0;
            }
        }
        self.ward_lock = (self.ward_lock - dt).max(0.0);
        self.staggered = (self.staggered - dt).max(0.0);
        self.shielding = (self.shielding - dt).max(0.0);
        if let Some((f, s)) = self.tempo {
            self.tempo = (s - dt > 0.0).then_some((f, s - dt));
        }
        let moving = 1.0 + self.speed.min(6.0) / 3.0;
        let mut due = 0.0;
        self.dot_timer += dt;
        let pay = self.dot_timer >= DOT_TICK;
        if pay {
            self.dot_timer -= DOT_TICK;
        }
        for (dot, k) in [
            (&mut self.burning, 1.0),
            (&mut self.bleeding, moving),
            (&mut self.rot, 1.0),
        ] {
            if dot.secs > 0.0 {
                if pay {
                    due += dot.dps * k * DOT_TICK;
                }
                dot.secs -= dt;
                if dot.secs <= 0.0 {
                    *dot = Dot::default();
                }
            }
        }
        due
    }
}

/// A damage over time's strength for the engine's view, 0..1 (10 a second is full).
fn dot_level(d: &Dot) -> f32 {
    if d.active() {
        (d.dps / 10.0).clamp(0.2, 1.0)
    } else {
        0.0
    }
}

impl Afflictions {
    /// What other modules see of this body's afflictions (`engine_world::Statuses`).
    pub fn statuses(&self) -> Statuses {
        Statuses {
            burning: dot_level(&self.burning),
            chill: self.chill.clamp(0.0, 255.0) as u8,
            frozen: self.frozen > 0.0,
            shocked: self.shocked > 0.0,
            bleeding: dot_level(&self.bleeding),
            rot: dot_level(&self.rot),
            // Three seconds of terror is full fright.
            dread: (self.dread / 3.0).clamp(0.0, 1.0),
            rooted: self.rooted > 0.0,
            staggered: self.staggered > 0.0,
            shielding: self.shielding > 0.0,
            // Magic eases this in and out from its veil affliction.
            veil: 0.0,
        }
    }
}

/// Run statuses: damage over time, time scales, the looks that follow each status, and the
/// engine's `Statuses` for other modules.
#[allow(clippy::type_complexity)]
pub fn tick_afflictions(
    time: Res<Time>,
    mut bodies: Query<(
        Entity,
        &mut Afflictions,
        &GlobalTransform,
        Option<&mut Statuses>,
    )>,
    mut queue: ResMut<MagicQueue>,
    mut damage: MessageWriter<DamageEvent>,
) {
    let dt = time.delta_secs();
    if dt <= 0.0 {
        return;
    }
    for (e, mut a, gt, shown_to_others) in &mut bodies {
        let p = gt.translation();
        if let Some(last) = a.last_pos {
            let d = p - last;
            a.speed = (DVec3::new(d.x, 0.0, d.z).length() as f32 / dt).min(30.0);
        }
        a.last_pos = Some(p);
        let due = a.tick(dt);
        if due > 0.0 {
            let amount = a.absorb(due);
            let kind = if a.burning.active() {
                DamageKind::Fire
            } else {
                DamageKind::Magic
            };
            if amount > 0.0 {
                damage.write(DamageEvent {
                    target: e,
                    amount,
                    kind,
                    point: p + DVec3::Y,
                    impulse: engine_core::math::Vec3::ZERO,
                    source: a.source,
                });
            }
        }
        // Time scale: set when it differs from 1, removed when it is 1 again.
        let want = a.time_scale();
        if (want - 1.0).abs() < 0.01 {
            if a.applied_scale.take().is_some() {
                queue.time_scales.push((e, None));
            }
        } else if a.applied_scale.is_none_or(|s| (s - want).abs() > 0.02) {
            a.applied_scale = Some(want);
            queue.time_scales.push((
                e,
                Some(LocalTimeScale {
                    factor: want,
                    ignore_world_dilation: false,
                }),
            ));
        }
        // Looks that follow the body while a status lasts.
        for s in Status::ALL {
            let on = a.has(s);
            let shown = a.shown & bit(s) != 0;
            if on && !shown {
                a.shown |= bit(s);
                queue.spawns.push(Spawn::Follow(FollowLook::new(e, s), p));
            } else if !on && shown {
                a.shown &= !bit(s);
            }
        }
        let now = a.statuses();
        match shown_to_others {
            Some(mut st) => {
                st.set_if_neq(now);
            }
            None => queue.statuses.push((e, now)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn auras_sit_and_fade() {
        let mut a = Afflictions::default();
        assert!(a.react(Element::Frost, 1.0).is_none());
        assert!(a.has_aura(Element::Frost));
        // 0.8 units fading over 9.5 s.
        for _ in 0..(9.4 * 60.0) as usize {
            a.tick(1.0 / 60.0);
        }
        assert!(a.has_aura(Element::Frost));
        for _ in 0..12 {
            a.tick(1.0 / 60.0);
        }
        assert!(!a.has_aura(Element::Frost));
    }

    #[test]
    fn fire_on_frost_makes_steam_and_order_matters() {
        let mut a = Afflictions::default();
        a.react(Element::Frost, 1.0);
        let (r, strong) = a.react(Element::Fire, 1.0).expect("reacts");
        assert_eq!(r.key, "steam");
        assert!(
            !a.has_aura(Element::Frost),
            "the strong order uses the aura up"
        );
        let mut b = Afflictions::default();
        b.react(Element::Fire, 1.0);
        let (r2, weak) = b.react(Element::Frost, 1.0).expect("reacts");
        assert_eq!(r2.key, "steam");
        assert!(strong > weak && weak > 1.0, "{strong} {weak}");
        assert!(strong <= MAX_REACTION);
    }

    #[test]
    fn every_element_pair_reacts() {
        for a in Element::ALL {
            for b in Element::ALL {
                if a != b {
                    assert!(pair_recipe(a, b).is_some(), "{a:?} + {b:?}");
                }
            }
        }
    }

    #[test]
    fn internal_cooldown_lets_one_in_three_through() {
        let mut a = Afflictions::default();
        let through: Vec<bool> = (0..7).map(|_| a.gate(Element::Fire, true)).collect();
        assert_eq!(through, [true, false, false, true, false, false, true]);
        assert!(a.gate(Element::Fire, false));
        // After the cooldown it applies at once.
        a.tick(ICD_SECS + 0.1);
        assert!(a.gate(Element::Fire, true));
    }

    #[test]
    fn five_chills_freeze_with_diminishing_returns() {
        let mut a = Afflictions::default();
        for i in 0..4 {
            assert_eq!(a.add(Status::Chilled, 1.0, 10.0, 5), None, "stack {i}");
        }
        assert!(a.time_scale() < 0.75);
        assert_eq!(
            a.add(Status::Chilled, 1.0, 10.0, 5),
            Some(StatusChange::Froze)
        );
        assert!(a.time_scale() < 0.05);
        let first = a.frozen;
        a.tick(first + 0.01);
        for _ in 0..5 {
            a.add(Status::Chilled, 1.0, 10.0, 5);
        }
        assert!(a.frozen < first * 0.75, "{} vs {first}", a.frozen);
        assert!(a.shatter());
        assert!(!a.has(Status::Frozen));
    }

    #[test]
    fn bleeding_is_worse_on_the_move_and_rot_blocks_healing() {
        let mut still = Afflictions::default();
        still.add(Status::Bleeding, 1.0, 20.0, 5);
        let mut moving = still.clone();
        moving.speed = 4.0;
        let (mut a, mut b) = (0.0, 0.0);
        for _ in 0..120 {
            a += still.tick(1.0 / 60.0);
            b += moving.tick(1.0 / 60.0);
        }
        assert!(a > 0.0 && b > a * 1.5, "{a} {b}");
        let mut r = Afflictions::default();
        r.add(Status::Rot, 1.0, 20.0, 5);
        assert!(r.heal_blocked());
    }

    #[test]
    fn wards_absorb_and_steam_quenches() {
        let mut a = Afflictions {
            ward: 30.0,
            ..Default::default()
        };
        assert_eq!(a.absorb(20.0), 0.0);
        assert_eq!(a.absorb(20.0), 10.0);
        a.add(Status::Burning, 1.0, 20.0, 5);
        assert!(a.has(Status::Burning));
        a.quench();
        assert!(!a.has(Status::Burning));
    }
}
