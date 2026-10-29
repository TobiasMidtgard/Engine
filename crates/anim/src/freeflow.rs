//! Freeflow targeting: whom each press goes at, so kills chain (Batman Arkham, Spider-Man,
//! Sifu).
//!
//! A [`Freeflow`] on an [`Attacker`] picks its target again on every press, from the foes in
//! range: the one closest to the way the stick points (inside [`Freeflow::cone_deg`]), with
//! distance counting less ([`Freeflow::angle_weight`]) and the current target kept unless the
//! stick points clearly elsewhere ([`Freeflow::keep_bonus`]). With no stick, it stays on the
//! current target while that one is still up, else takes the nearest foe in front, else the
//! nearest. A target that goes down (knocked down, on a knee, despawned) is handed over at
//! once, keeping the press waiting in the buffer: a kill never eats the next press, and the
//! finisher's recovery gives way to it ([`Attacker::struck`]). A foe past the strike's range
//! is closed on with the attack set's approaches (a lunge or a leap).
//!
//! The reach grows with the combo, as in Arkham: [`Freeflow::range`] plus
//! [`Freeflow::range_per_hit`] for each blow landed in a row, up to [`Freeflow::max_range`].
//! The combo ends after [`Freeflow::combo_secs`] without landing one, or when the fighter is
//! hit. It is published as [`Freeflow::combo`] and, where the fighter has them, as the
//! `combo` animation parameter.
//!
//! Gameplay drives it with the stick (world, flat: [`Freeflow::stick`], or the `stick_x` and
//! `stick_z` animation parameters) and the usual `attack_light` / `attack_heavy` triggers;
//! [`Freeflow::foes`] lists whom it may pick (empty: anyone with a [`HitReaction`] but
//! itself). [`pick`] is the choice on its own, for a gameplay module that keeps its own
//! list of foes.

use std::collections::HashMap;

use engine_core::prelude::*;
use engine_world::DamageEvent;
use engine_world::animation::AnimationParams;

use crate::Animator;
use crate::combat::Attacker;
use crate::hit::HitReaction;
use crate::hurt::Hurt;
use crate::knockdown::Knockdown;

/// Picks an [`Attacker`]'s target on each press (see the module docs).
#[derive(Component, Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Freeflow {
    pub enabled: bool,
    /// Where the stick points, world (flat; under 0.2 long counts as no stick).
    pub stick: Vec3,
    /// How far a press reaches for a foe, m, growing by `range_per_hit` for each blow of the
    /// combo up to `max_range`.
    pub range: f32,
    pub range_per_hit: f32,
    pub max_range: f32,
    /// Widest angle off the stick a foe is picked at, degrees.
    pub cone_deg: f32,
    /// Weight of the angle off the stick against the distance (0..1).
    pub angle_weight: f32,
    /// Score taken off the current target (it stays unless the stick points clearly
    /// elsewhere).
    pub keep_bonus: f32,
    /// Seconds without landing a blow that end the combo.
    pub combo_secs: f32,
    /// Whom it may pick (empty: anyone with a `HitReaction` but itself).
    pub foes: Vec<Entity>,
    /// Blows landed in a row (published).
    pub combo: u32,
    /// Seconds since the last blow landed (published).
    pub since_hit: f32,
    /// Times the target changed to another foe (published).
    pub switches: u32,
}

impl Default for Freeflow {
    fn default() -> Self {
        Self {
            enabled: true,
            stick: Vec3::ZERO,
            range: 6.0,
            range_per_hit: 0.25,
            max_range: 7.5,
            cone_deg: 60.0,
            angle_weight: 0.7,
            keep_bonus: 0.15,
            combo_secs: 1.5,
            foes: Vec::new(),
            combo: 0,
            since_hit: 0.0,
            switches: 0,
        }
    }
}

impl Freeflow {
    /// How far a press reaches now, m.
    pub fn reach(&self) -> f32 {
        (self.range + self.range_per_hit * self.combo as f32).min(self.max_range.max(self.range))
    }
}

/// The foe a press from `from` should go at: `stick` is where the stick points (world; zero
/// for none), `facing` where the fighter faces, `foes` who could be struck (up and about) and
/// where, `current` the target now. `None` when no foe is in reach.
pub fn pick(
    ff: &Freeflow,
    from: DVec3,
    facing: Vec3,
    stick: Vec3,
    foes: &[(Entity, DVec3)],
    current: Option<Entity>,
) -> Option<Entity> {
    let reach = ff.reach() as f64;
    let flat = |v: Vec3| Vec3::new(v.x, 0.0, v.z);
    let stick = flat(stick);
    let in_reach = foes.iter().filter_map(|&(e, at)| {
        let to = (at - from).with_y(0.0);
        let d = to.length();
        (d <= reach).then(|| (e, to.as_vec3(), d as f32))
    });
    if stick.length() >= 0.2 {
        let dir = stick.normalize();
        let cone = ff.cone_deg.to_radians().max(1e-3);
        in_reach
            .filter_map(|(e, to, d)| {
                let angle = if d < 1e-3 {
                    0.0
                } else {
                    (to / d).dot(dir).clamp(-1.0, 1.0).acos()
                };
                (angle <= cone).then(|| {
                    let w = ff.angle_weight.clamp(0.0, 1.0);
                    let mut score = w * angle / cone + (1.0 - w) * d / reach as f32;
                    if Some(e) == current {
                        score -= ff.keep_bonus;
                    }
                    (e, score)
                })
            })
            .min_by(|a, b| a.1.total_cmp(&b.1))
            .map(|(e, _)| e)
    } else {
        let all: Vec<_> = in_reach.collect();
        if let Some(c) = current
            && all.iter().any(|f| f.0 == c)
        {
            return Some(c);
        }
        let facing = flat(facing).normalize_or_zero();
        let nearest = |front: bool| {
            all.iter()
                .filter(|(_, to, d)| !front || *d < 1e-3 || to.dot(facing) > 0.0)
                .min_by(|a, b| a.2.total_cmp(&b.2))
                .map(|f| f.0)
        };
        nearest(true).or_else(|| nearest(false))
    }
}

/// Pick freeflow fighters' targets on their presses and hand a downed target over (in
/// `AnimSet::Setup`, after triggers are queued and before the attacks run).
#[allow(clippy::type_complexity)]
pub(crate) fn pick_targets(
    time: Res<Time>,
    mut damage: MessageReader<DamageEvent>,
    mut fighters: Query<(
        Entity,
        &mut Freeflow,
        &mut Attacker,
        &Animator,
        &GlobalTransform,
        Option<&mut AnimationParams>,
        Option<&LocalTimeScale>,
    )>,
    bodies: Query<(Entity, &GlobalTransform, Option<&Knockdown>, Option<&Hurt>), With<HitReaction>>,
) {
    // Blows landed by each fighter, and fighters hit, this frame.
    let mut landed: HashMap<Entity, u32> = HashMap::new();
    let mut hurt: Vec<Entity> = Vec::new();
    for d in damage.read() {
        if d.amount <= 0.0 {
            continue;
        }
        if let Some(s) = d.source {
            *landed.entry(s).or_default() += 1;
        }
        hurt.push(d.target);
    }
    // Who is up and about, and where.
    let up: Vec<(Entity, DVec3)> = bodies
        .iter()
        .filter(|(_, _, down, h)| down.is_none() && !h.is_some_and(|h| h.is_down()))
        .map(|(e, gt, ..)| (e, gt.translation()))
        .collect();
    for (me, mut ff, mut attacker, animator, gt, params, local) in &mut fighters {
        let ff = &mut *ff;
        // The combo.
        ff.since_hit += time.delta_for(local);
        if let Some(n) = landed.get(&me) {
            ff.combo += n;
            ff.since_hit = 0.0;
        }
        if hurt.contains(&me) || ff.since_hit > ff.combo_secs {
            ff.combo = 0;
        }
        let stick = params
            .as_deref()
            .filter(|p| p.values.contains_key("stick_x") || p.values.contains_key("stick_z"))
            .map_or(ff.stick, |p| {
                Vec3::new(p.get("stick_x"), 0.0, p.get("stick_z"))
            });
        if let Some(mut p) = params
            && p.get("combo") != ff.combo as f32
        {
            p.set("combo", ff.combo as f32);
        }
        if !ff.enabled {
            continue;
        }
        let foes: Vec<(Entity, DVec3)> = up
            .iter()
            .copied()
            .filter(|(e, _)| *e != me && (ff.foes.is_empty() || ff.foes.contains(e)))
            .collect();
        let pressed = animator
            .triggers
            .iter()
            .any(|(name, _)| name.starts_with("attack_"));
        let lost = attacker
            .target
            .is_none_or(|t| !foes.iter().any(|f| f.0 == t));
        // A fresh press picks again; a press waiting on a target that went down is handed
        // over.
        if !(pressed || (lost && attacker.buffered.is_some())) {
            continue;
        }
        let facing = gt.rotation() * Vec3::NEG_Z;
        let current = attacker.target.filter(|_| !lost);
        let next = pick(ff, gt.translation(), facing, stick, &foes, current);
        if next.is_some() && next != attacker.target {
            if attacker.target.is_some() {
                ff.switches += 1;
            }
            attacker.target = next;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn foes() -> Vec<(Entity, DVec3)> {
        // Near and ahead, farther off to the left, far to the right, close behind, out of
        // reach ahead.
        [
            (0.0, -1.2),
            (-3.0, -0.5),
            (5.0, 0.0),
            (0.0, 1.5),
            (0.0, -9.0),
        ]
        .iter()
        .enumerate()
        .map(|(i, &(x, z))| {
            (
                Entity::from_raw_u32(i as u32 + 1).unwrap(),
                DVec3::new(x, 0.0, z),
            )
        })
        .collect()
    }

    fn e(i: u32) -> Entity {
        Entity::from_raw_u32(i).unwrap()
    }

    #[test]
    fn the_stick_picks_the_foe_it_points_at_over_a_nearer_one() {
        let ff = Freeflow::default();
        let pick =
            |stick: Vec3, current| pick(&ff, DVec3::ZERO, Vec3::NEG_Z, stick, &foes(), current);
        assert_eq!(pick(Vec3::NEG_X, None), Some(e(2)));
        assert_eq!(pick(Vec3::X, Some(e(1))), Some(e(3)));
        assert_eq!(pick(Vec3::Z, Some(e(1))), Some(e(4)));
        // Ahead: the near one; the one 9 m ahead is out of reach.
        assert_eq!(pick(Vec3::NEG_Z, None), Some(e(1)));
        // Nobody within 60° of the stick.
        let ahead = &foes()[..1];
        assert_eq!(
            super::pick(&ff, DVec3::ZERO, Vec3::NEG_Z, Vec3::X, ahead, None),
            None
        );
    }

    #[test]
    fn the_current_target_is_kept_unless_the_stick_points_clearly_elsewhere() {
        let ff = Freeflow::default();
        // Between the one ahead and the one to the left, a little nearer the left one: the
        // current target (ahead) stays.
        let between = Vec3::new(-1.0, 0.0, -0.9);
        let foes = foes();
        assert_eq!(
            pick(&ff, DVec3::ZERO, Vec3::NEG_Z, between, &foes, Some(e(1))),
            Some(e(1))
        );
        assert_eq!(
            pick(&ff, DVec3::ZERO, Vec3::NEG_Z, between, &foes, Some(e(2))),
            Some(e(2))
        );
    }

    #[test]
    fn with_no_stick_it_stays_on_or_takes_the_nearest_in_front() {
        let ff = Freeflow::default();
        let foes = foes();
        let none = Vec3::ZERO;
        assert_eq!(
            pick(&ff, DVec3::ZERO, Vec3::NEG_Z, none, &foes, Some(e(3))),
            Some(e(3))
        );
        // Facing backwards: the one behind, though the one ahead is nearer.
        assert_eq!(
            pick(&ff, DVec3::ZERO, Vec3::Z, none, &foes, None),
            Some(e(4))
        );
        // Facing right with nobody up ahead of it but the far one: that one, though nearer
        // ones stand behind.
        assert_eq!(
            pick(&ff, DVec3::ZERO, Vec3::X, none, &foes, None),
            Some(e(3))
        );
    }

    #[test]
    fn reach_grows_with_the_combo_up_to_its_limit() {
        let mut ff = Freeflow::default();
        let far = [(e(9), DVec3::new(0.0, 0.0, -7.0))];
        assert_eq!(
            pick(&ff, DVec3::ZERO, Vec3::NEG_Z, Vec3::NEG_Z, &far, None),
            None
        );
        ff.combo = 4;
        assert_eq!(
            pick(&ff, DVec3::ZERO, Vec3::NEG_Z, Vec3::NEG_Z, &far, None),
            Some(e(9))
        );
        ff.combo = 100;
        assert_eq!(ff.reach(), ff.max_range);
    }
}
