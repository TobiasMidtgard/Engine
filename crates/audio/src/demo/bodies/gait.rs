//! Where a walker's feet are. `walk_trail` writes each footfall where this says the foot
//! lands, and the body animation plants the drawn foot on the same spot, so a contact, its
//! ring and the foot always agree.
//!
//! A walker's gait is driven by distance, not time: each foot touches down once per stride
//! (`feet × step_len` meters of travel), stays planted for `duty` of the stride while the body
//! moves over it, then swings forward to its next footprint one stride further on.

use engine_core::math::{DVec3, Vec3};
use engine_world::ContactKind;

use super::super::{TrailWalker, trail_point};

/// Height of the trail's surface above its origin, meters.
pub const GROUND: f64 = 0.06;

/// How a walker's feet move.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Gait {
    /// Fraction of a stride each foot spends on the ground (below 0.5: a run, with a flight
    /// phase).
    pub duty: f32,
    /// How far ahead of its hip (or shoulder) a foot's footprint centre lands, meters.
    pub ahead: f32,
    /// Feet turn out from the direction of travel by this much, radians.
    pub toe_out: f32,
}

impl Gait {
    pub fn of(w: &TrailWalker) -> Self {
        match w.kind {
            ContactKind::Hoof => Gait {
                duty: 0.56,
                ahead: 0.55,
                toe_out: 0.0,
            },
            ContactKind::Paw => Gait {
                duty: 0.45,
                ahead: 0.2,
                toe_out: 0.0,
            },
            _ if w.feet == 2 && w.speed > 2.5 => Gait {
                duty: 0.33,
                ahead: 0.3,
                toe_out: 0.05,
            },
            _ => Gait {
                duty: 0.62,
                ahead: 0.42,
                toe_out: 0.12,
            },
        }
    }

    /// The runner's gait has both feet off the ground between steps.
    pub fn runs(&self) -> bool {
        self.duty < 0.5
    }
}

/// Which side a foot is on (−1 left, +1 right) and how far ahead of the walker's root its hip
/// or shoulder is, meters. Bipeds: left, right. Quadrupeds in walking order: left fore, right
/// hind, right fore, left hind.
pub fn layout(w: &TrailWalker, foot: u8) -> (f32, f32) {
    let (side, fore) = match (w.feet, foot) {
        (2, 0) => (-1.0, 0.0),
        (2, _) => (1.0, 0.0),
        (_, 0) => (-1.0, 0.5),
        (_, 1) => (1.0, -0.5),
        (_, 2) => (1.0, 0.5),
        _ => (-1.0, -0.5),
    };
    (side, fore * w.length)
}

/// Meters per stride (every foot once).
pub fn stride(w: &TrailWalker) -> f32 {
    w.feet as f32 * w.step_len
}

/// Where the gait is, in steps since foot 0 last touched down: `0..feet`, reaching `f` as foot
/// `f` touches down.
pub fn phase(w: &TrailWalker) -> f32 {
    (w.next_foot as f32 - w.to_next_step / w.step_len).rem_euclid(w.feet as f32)
}

/// For `foot`: how far through its stride it is (0 at touchdown, `duty` at lift-off, up to 1)
/// and where the walker's root was along the trail when it last touched down.
pub fn foot_cycle(w: &TrailWalker, foot: u8) -> (f32, f64) {
    let feet = w.feet as f32;
    let u = ((phase(w) - foot as f32) / feet).rem_euclid(1.0);
    let since = u * stride(w);
    (u, w.along - since as f64)
}

/// The centre of the footprint `foot` makes when it touches down with the walker's root at
/// `root_along` (absolute, on the trail's surface), the trail's heading there and how far
/// along the trail it is.
pub fn footprint(w: &TrailWalker, foot: u8, root_along: f64) -> (DVec3, Vec3, f64) {
    let (side, fore) = layout(w, foot);
    let s = root_along + (fore + Gait::of(w).ahead) as f64;
    let (p, d) = trail_point(s);
    let right = Vec3::new(-d.z, 0.0, d.x);
    let across = side * w.stance + w.lane_at(s);
    (
        w.origin + p + (right * across).as_dvec3() + DVec3::new(0.0, GROUND, 0.0),
        (d + right * w.lane_slope(s)).normalize(),
        s,
    )
}

/// Where the walker's root is (on the trail's surface, in its lane) and which way it faces.
pub fn root(w: &TrailWalker) -> (DVec3, Vec3) {
    let (p, d) = trail_point(w.along);
    let right = Vec3::new(-d.z, 0.0, d.x);
    (
        w.origin + p + (right * w.lane_at(w.along)).as_dvec3() + DVec3::new(0.0, GROUND, 0.0),
        (d + right * w.lane_slope(w.along)).normalize(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn walker(kind: ContactKind, speed: f32) -> TrailWalker {
        TrailWalker::new(DVec3::new(5.0, 0.0, -3.0), 1.0, kind, 70.0, speed)
    }

    /// Walk `w` forward in small steps the way `walk_trail` does, calling `each` after every
    /// move with the feet that touched down.
    fn walk(w: &mut TrailWalker, meters: f32, mut each: impl FnMut(&TrailWalker, &[(u8, f64)])) {
        let dx = 0.004;
        let mut moved = 0.0;
        let mut down = Vec::new();
        while moved < meters {
            moved += dx;
            w.along += dx as f64;
            w.to_next_step -= dx;
            down.clear();
            while w.to_next_step <= 0.0 {
                w.to_next_step += w.step_len;
                let foot = w.next_foot;
                w.next_foot = (foot + 1) % w.feet;
                down.push((foot, w.along + (w.to_next_step - w.step_len) as f64));
            }
            each(w, &down);
        }
    }

    #[test]
    fn a_foot_touches_down_as_its_stride_starts() {
        for (kind, speed) in [
            (ContactKind::Foot, 1.4),
            (ContactKind::Foot, 4.0),
            (ContactKind::Paw, 1.8),
            (ContactKind::Hoof, 1.7),
        ] {
            let mut w = walker(kind, speed);
            let mut touched = 0;
            walk(&mut w, 12.0, |w, down| {
                for &(foot, at) in down {
                    touched += 1;
                    let (u, td) = foot_cycle(w, foot);
                    assert!(
                        !(0.02..0.98).contains(&u),
                        "{kind:?} foot {foot} at u {u} on touchdown"
                    );
                    if u < 0.5 {
                        assert!((td - at).abs() < 0.01, "{td} vs {at}");
                    }
                }
            });
            assert!(touched > 8);
        }
    }

    #[test]
    fn a_planted_footprint_stays_put_until_lift_off() {
        let mut w = walker(ContactKind::Hoof, 1.7);
        let g = Gait::of(&w);
        let mut planted: [Option<DVec3>; 4] = [None; 4];
        walk(&mut w, 10.0, |w, _| {
            for foot in 0..4 {
                let (u, td) = foot_cycle(w, foot);
                let (fp, _, _) = footprint(w, foot, td);
                if u < g.duty {
                    if let Some(p) = planted[foot as usize] {
                        assert!(p.distance(fp) < 1e-3, "foot {foot} slid {}", p.distance(fp));
                    }
                    planted[foot as usize] = Some(fp);
                } else {
                    planted[foot as usize] = None;
                }
            }
        });
    }
}
