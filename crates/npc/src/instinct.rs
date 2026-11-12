//! Animal instincts by difficulty (engine-design/npc-difficulty.md 4.7): what wildlife knows to
//! do, tier by tier, read from each animal's faction's tier (`difficulty::tier_of`), so prey
//! and predators can be set apart.
//!
//! | Tier    | Prey                                                       | Predators              |
//! |---------|------------------------------------------------------------|------------------------|
//! | Easy    | bolt late (flight distance x0.6), look up only briefly, run straight (no jinking) | |
//! | Medium  | as wildlife always has: bolt at the flight distance, rabbits zigzag, deer cut from a wolf | packs test, flank, lie in wait |
//! | Hard    | run for cover: rabbits into tall grass and freeze there until you are almost on them, deer into the woods and stop there to watch; an alert herd bunches up, the young inside | |
//! | Extreme | boars charge whatever is nearest their young               | wolves ring a quarry at bay (Muro's two rules); an ambusher waits in tall grass |
//!
//! Cover is read from the world: tall grass and leaves from the [`Obscurance`] sources (a
//! foliage module registers its own), woods from the tree trunks in the [`TacticalWorld`].
//! With neither about, an animal on Hard runs as it would on Medium.

use engine_core::prelude::*;
use engine_world::{DifficultyTier, Obscurance};

use crate::world_model::{ObstacleShape, TacticalWorld};

/// One instinct, and the lowest tier that has it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Instinct {
    /// Rabbits zigzag, a deer cuts from side to side from a wolf at its heels.
    Jink,
    /// Run for cover, and keep still in it.
    Refuge,
    /// An alert herd bunches up, each toward its nearest neighbour, the young to the middle.
    SelfishHerd,
    /// Boars charge whatever is nearest their young, wherever they themselves are.
    GuardYoung,
    /// Wolves ring a quarry at bay: in to a safe distance, then apart from each other.
    Encircle,
    /// A wolf lying in wait picks tall grass to lie in.
    GrassAmbush,
}

impl Instinct {
    pub const ALL: [Instinct; 6] = [
        Instinct::Jink,
        Instinct::Refuge,
        Instinct::SelfishHerd,
        Instinct::GuardYoung,
        Instinct::Encircle,
        Instinct::GrassAmbush,
    ];

    pub fn name(self) -> &'static str {
        match self {
            Instinct::Jink => "jink",
            Instinct::Refuge => "refuge",
            Instinct::SelfishHerd => "selfish_herd",
            Instinct::GuardYoung => "guard_young",
            Instinct::Encircle => "encircle",
            Instinct::GrassAmbush => "grass_ambush",
        }
    }

    pub fn min_tier(self) -> DifficultyTier {
        match self {
            Instinct::Jink => DifficultyTier::Medium,
            Instinct::Refuge | Instinct::SelfishHerd => DifficultyTier::Hard,
            Instinct::GuardYoung | Instinct::Encircle | Instinct::GrassAmbush => {
                DifficultyTier::Extreme
            }
        }
    }

    pub fn allowed(self, tier: DifficultyTier) -> bool {
        tier >= self.min_tier()
    }
}

/// Multiplies an animal's flight distance: on Easy it lets you much closer.
pub fn flight_scale(tier: DifficultyTier) -> f32 {
    if tier == DifficultyTier::Easy {
        0.6
    } else {
        1.0
    }
}

/// Multiplies how long an animal stays alert after a scare: on Easy it soon goes back to
/// grazing.
pub fn alert_scale(tier: DifficultyTier) -> f64 {
    if tier == DifficultyTier::Easy {
        0.5
    } else {
        1.0
    }
}

/// Multiplies the flight distance of an animal in its cover: it feels safe there, and lets a
/// threat come closer before it breaks.
pub const REFUGE_CALM: f32 = 0.45;
/// A hiding rabbit bolts from a threat this close (m).
pub const HIDE_BOLT: f32 = 3.0;
/// Cover at least this good (0..1) counts as a refuge.
pub const REFUGE_COVER: f32 = 0.6;

/// What cover an animal looks for.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CoverKind {
    /// Hide its body at this height above the ground (m).
    pub height: f32,
    /// Trees count (woods).
    pub woods: bool,
    /// How far it looks for cover (m).
    pub reach: f32,
}

/// How well grass or leaves at `height` above `p` (feet) hide an animal: 0 open .. 1 hidden
/// (sight falls through 1.5 m of it).
pub fn grass_at(p: DVec3, height: f32, obscurance: &Obscurance) -> f32 {
    let ext = obscurance.extinction(p + DVec3::Y * height as f64);
    1.0 - (-ext * 1.5).exp()
}

/// How much `p` (feet) stands among trees: 0 open .. 1 woods (four or more trunks within
/// 6 m).
pub fn woods_at(p: DVec3, world: &TacticalWorld, scratch: &mut Vec<u32>) -> f32 {
    let r = 6.0;
    scratch.clear();
    world.obstacles_near(
        DVec2::new(p.x - r, p.z - r),
        DVec2::new(p.x + r, p.z + r),
        scratch,
    );
    let trunks = scratch
        .iter()
        .filter(|&&i| {
            let o = &world.obstacles[i as usize];
            let thin = match o.shape {
                ObstacleShape::Cylinder {
                    radius,
                    half_height,
                } => radius < 1.0 && half_height > 1.5,
                ObstacleShape::Box { half } => {
                    half.x.max(half.z) < 0.8 && half.y > 1.5 && o.upright()
                }
                _ => false,
            };
            thin && DVec2::new(o.center.x - p.x, o.center.z - p.z).length() < r
        })
        .count();
    (trunks as f32 / 4.0).min(1.0)
}

/// How well `p` (feet) hides an animal that looks for `kind`: 0 open .. 1 hidden.
pub fn cover_at(
    kind: CoverKind,
    p: DVec3,
    obscurance: &Obscurance,
    world: &TacticalWorld,
    scratch: &mut Vec<u32>,
) -> f32 {
    let grass = grass_at(p, kind.height, obscurance);
    if kind.woods {
        grass.max(woods_at(p, world, scratch))
    } else {
        grass
    }
}

/// The spot nearest `center` (within `reach`) whose `cover` is at least [`REFUGE_COVER`].
pub fn nearest_cover(
    center: DVec3,
    reach: f32,
    mut cover: impl FnMut(DVec3) -> f32,
) -> Option<DVec3> {
    if cover(center) >= REFUGE_COVER {
        return Some(center);
    }
    for r in [1.5f32, 3.0, 4.5, 6.0, 8.0, 10.0, 13.0] {
        if r > reach {
            break;
        }
        for k in 0..16 {
            let a = k as f32 / 16.0 * std::f32::consts::TAU;
            let dir = DVec3::new(a.cos() as f64, 0.0, a.sin() as f64);
            let p = center + dir * r as f64;
            if cover(p) >= REFUGE_COVER {
                // A step further in, if it still hides there (not on the very edge).
                let deeper = p + dir * 1.5;
                return Some(if cover(deeper) >= REFUGE_COVER {
                    deeper
                } else {
                    p
                });
            }
        }
    }
    None
}

/// Where to run for cover from a threat at `threat`: the nearest spot within `kind.reach`
/// whose `cover` is at least [`REFUGE_COVER`], not toward the threat (nor much closer to it)
/// and walkable (`ok`). Looks along 16 bearings at a few distances.
pub fn find_refuge(
    pos: DVec3,
    threat: DVec3,
    kind: CoverKind,
    mut cover: impl FnMut(DVec3) -> f32,
    mut ok: impl FnMut(DVec3) -> bool,
) -> Option<DVec3> {
    let toward = Vec3::new((threat.x - pos.x) as f32, 0.0, (threat.z - pos.z) as f32);
    let gap = toward.length();
    let toward = toward.normalize_or_zero();
    for r in [1.5f32, 3.0, 5.0, 7.5, 10.0, 14.0, 19.0, 25.0, 33.0, 45.0] {
        if r > kind.reach {
            break;
        }
        let mut best: Option<(DVec3, f32)> = None;
        for k in 0..16 {
            let a = k as f32 / 16.0 * std::f32::consts::TAU;
            let dir = Vec3::new(a.cos(), 0.0, a.sin());
            // Not toward the threat: off to the side at most, unless the threat is still far
            // and the spot well short of it.
            let at_threat = dir.dot(toward);
            if at_threat > 0.35 && !(gap > 3.0 * r && at_threat < 0.8) {
                continue;
            }
            let p = pos + (dir * r).as_dvec3();
            let left = DVec2::new(p.x - threat.x, p.z - threat.z).length() as f32;
            if left < (gap * 0.8).min(gap - 1.0) {
                continue;
            }
            let c = cover(p);
            if c < REFUGE_COVER || !ok(p) {
                continue;
            }
            // Better hidden, and further from the threat.
            let score = c + 0.02 * left;
            if best.is_none_or(|b| score > b.1) {
                best = Some((p, score));
            }
        }
        if let Some((p, _)) = best {
            return Some(p);
        }
    }
    None
}

/// Selfish herd (Hamilton): which way an alert herd animal at `pos` steps, given the middle
/// of the rest of its herd and its nearest neighbour. The young go to the middle; the rest
/// close on their nearest neighbour, until within `close`; the badly hurt can't keep up and
/// are left on the edge.
pub fn selfish_step(
    pos: DVec3,
    young: bool,
    hurt: f32,
    center: DVec3,
    nearest: DVec3,
    close: f32,
) -> Vec3 {
    if hurt > 0.4 {
        return Vec3::ZERO;
    }
    let flat = |d: DVec3| Vec3::new(d.x as f32, 0.0, d.z as f32);
    let middle = flat(center - pos);
    if young {
        return if middle.length() > close * 0.5 {
            middle.normalize()
        } else {
            Vec3::ZERO
        };
    }
    let to = flat(nearest - pos);
    if to.length() > close {
        // Toward the neighbour, with a pull to the middle (so two at the edge don't just
        // pair off out there).
        (to.normalize() + middle.normalize_or_zero() * 0.5).normalize_or_zero()
    } else {
        Vec3::ZERO
    }
}

/// Muro's two rules for a wolf ringing a quarry at bay (Muro, Escobedo, Spector and Coppinger,
/// "Wolf-pack hunting strategies emerge from simple rules", 2011): close in on the quarry
/// until at `safe` distance, then move away from the other wolves near it. Where the wolf at
/// `pos` heads (unit, or zero when settled).
pub fn muro_step(
    pos: DVec3,
    quarry: DVec3,
    safe: f32,
    others: impl IntoIterator<Item = DVec3>,
) -> Vec3 {
    let flat = |d: DVec3| Vec3::new(d.x as f32, 0.0, d.z as f32);
    let to = flat(quarry - pos);
    let d = to.length();
    if d < 1e-3 {
        return Vec3::ZERO;
    }
    // Rule 1: in to the safe distance (and back out to it, if inside).
    let mut v = to / d * ((d - safe) / safe).clamp(-1.0, 2.0);
    // Rule 2: away from the other wolves near the quarry, around the ring (tangentially),
    // the nearest the most.
    let ring = Vec3::new(-to.z, 0.0, to.x) / d;
    for o in others {
        let off = flat(pos - o);
        let od = off.length();
        if od < 1e-3 || flat(quarry - o).length() > safe * 2.5 {
            continue;
        }
        let push = (safe * 2.0 / od.max(0.5)).min(3.0);
        let side = if off.dot(ring) >= 0.0 { 1.0 } else { -1.0 };
        v += ring * side * push * 0.6;
    }
    if v.length() < 0.15 {
        Vec3::ZERO
    } else {
        v.normalize()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_tier_adds_instincts() {
        let count = |tier| Instinct::ALL.iter().filter(|i| i.allowed(tier)).count();
        assert_eq!(
            [
                DifficultyTier::Easy,
                DifficultyTier::Medium,
                DifficultyTier::Hard,
                DifficultyTier::VeryHard,
                DifficultyTier::Extreme
            ]
            .map(count),
            [0, 1, 3, 3, 6]
        );
        assert!(flight_scale(DifficultyTier::Easy) < flight_scale(DifficultyTier::Medium));
    }

    #[test]
    fn a_refuge_is_the_nearest_cover_away_from_the_threat() {
        let kind = CoverKind {
            height: 0.25,
            woods: false,
            reach: 30.0,
        };
        // Grass to the north (toward the threat) at 4 m, and to the east at 9 m.
        let grass = |p: DVec3| {
            let north = p.z < -2.5 && p.z > -6.0 && p.x.abs() < 3.0;
            let east = p.x > 8.0 && p.x < 12.0 && p.z.abs() < 3.0;
            if north || east { 1.0 } else { 0.0 }
        };
        let threat = DVec3::new(0.0, 0.0, -15.0);
        let r = find_refuge(DVec3::ZERO, threat, kind, grass, |_| true).unwrap();
        assert!(r.x > 8.0 && r.z.abs() < 3.0, "{r}");
        // None in reach: none.
        let none = find_refuge(DVec3::ZERO, threat, kind, |_| 0.0, |_| true);
        assert!(none.is_none());
    }

    #[test]
    fn the_young_go_to_the_middle_and_adults_close_on_a_neighbour() {
        let center = DVec3::new(4.0, 0.0, 4.0);
        let calf = selfish_step(DVec3::ZERO, true, 0.0, center, DVec3::X * 6.0, 1.8);
        assert!(calf.x > 0.5 && calf.z > 0.5, "{calf}");
        let at = DVec3::new(-4.0, 0.0, 0.0);
        let adult = selfish_step(at, false, 0.0, center, DVec3::ZERO, 1.8);
        assert!(adult.x > 0.8, "{adult}");
        let close = DVec3::new(5.0, 0.0, 0.0);
        assert_eq!(
            selfish_step(close, false, 0.0, center, DVec3::X * 6.0, 1.8),
            Vec3::ZERO
        );
        assert_eq!(
            selfish_step(at, false, 0.9, center, DVec3::ZERO, 1.8),
            Vec3::ZERO
        );
    }

    #[test]
    fn wolves_following_muros_rules_spread_round_the_quarry() {
        // Three wolves bunched on one side of a quarry at bay end up round it.
        let quarry = DVec3::ZERO;
        let mut wolves = [
            DVec3::new(-12.0, 0.0, 1.0),
            DVec3::new(-12.0, 0.0, -1.0),
            DVec3::new(-13.0, 0.0, 0.0),
        ];
        for _ in 0..600 {
            let now = wolves;
            for (k, w) in wolves.iter_mut().enumerate() {
                let others = (0..3).filter(|&j| j != k).map(|j| now[j]);
                *w += (muro_step(*w, quarry, 4.0, others) * 0.05).as_dvec3();
            }
        }
        let mut angles: Vec<f64> = wolves.iter().map(|w| w.z.atan2(w.x)).collect();
        angles.sort_by(f64::total_cmp);
        let gaps = [
            angles[1] - angles[0],
            angles[2] - angles[1],
            angles[0] + std::f64::consts::TAU - angles[2],
        ];
        let widest = gaps.iter().cloned().fold(0.0, f64::max);
        assert!(widest < 3.2, "{angles:?}");
        for w in wolves {
            assert!((w.length() - 4.0).abs() < 1.5, "{w}");
        }
    }
}
