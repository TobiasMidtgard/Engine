//! The hand pose library: how far each finger joint bends for each `HandShape`, from the
//! relaxed hand (the cascade: index to little finger curling more, as a resting hand does) to
//! grips that close on a handle of a given radius.
//!
//! Angles are anatomical, in degrees, counted from a straight finger: `bend` per joint
//! (knuckle, middle, end; the thumb's base, knuckle, end), `spread` at the knuckle away from
//! the middle finger (the thumb's: out of the palm) and the thumb's `roll` (turning its pad to
//! face the fingers). Every value is clamped to the joint's range of motion ([`ROM`]; normal
//! active ranges from the hand-therapy literature, research-ik-hands-skinning.md 2.3). The rig
//! turns them into finger bones (`Rig::finger_turn`), measuring each person's own axes.

use engine_world::character::{HandControls, HandPose, HandShape};

/// One finger's angles (degrees).
#[derive(Debug, Clone, Copy, PartialEq, Default, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct FingerAngles {
    /// Bend per joint, knuckle first.
    pub bend: [f32; 3],
    /// At the knuckle: away from the middle finger (the thumb: out of the palm).
    pub spread: f32,
    /// At the knuckle: the turn about the finger (the thumb's opposition).
    pub roll: f32,
}

/// Both hands' fingers, thumb first.
#[derive(Debug, Clone, Copy, PartialEq, Default, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct HandAngles {
    pub fingers: [FingerAngles; 5],
}

/// A joint's range of motion, degrees (min, max).
pub type Range = (f32, f32);

/// Per finger (thumb first): bend range per joint, spread range, roll range.
pub const ROM: [([Range; 3], Range, Range); 5] = [
    // Thumb: base across the palm, knuckle, end joint; out of the palm; opposition.
    (
        [(-20.0, 60.0), (-10.0, 60.0), (-20.0, 85.0)],
        (-10.0, 70.0),
        (-20.0, 60.0),
    ),
    // Fingers: knuckle (hyperextends), middle joint, end joint.
    (
        [(-30.0, 95.0), (-5.0, 115.0), (-15.0, 90.0)],
        (-20.0, 25.0),
        (-15.0, 15.0),
    ),
    (
        [(-30.0, 95.0), (-5.0, 115.0), (-15.0, 90.0)],
        (-20.0, 20.0),
        (-15.0, 15.0),
    ),
    (
        [(-30.0, 95.0), (-5.0, 115.0), (-15.0, 90.0)],
        (-20.0, 25.0),
        (-15.0, 15.0),
    ),
    (
        [(-30.0, 100.0), (-5.0, 115.0), (-15.0, 90.0)],
        (-20.0, 35.0),
        (-15.0, 15.0),
    ),
];

const fn f(bend: [f32; 3], spread: f32, roll: f32) -> FingerAngles {
    FingerAngles { bend, spread, roll }
}

/// The hand at rest: the cascade (index 20/25/10° to little finger 44/48/18°), the fingers
/// fanned a little, the thumb beside the index.
pub const RELAXED: HandAngles = HandAngles {
    fingers: [
        f([12.0, 12.0, 10.0], 18.0, 10.0),
        f([20.0, 25.0, 10.0], 3.0, 0.0),
        f([28.0, 32.0, 12.0], 0.0, 0.0),
        f([36.0, 40.0, 15.0], 3.0, 0.0),
        f([44.0, 48.0, 18.0], 8.0, 0.0),
    ],
};

/// Flat and spread.
pub const OPEN: HandAngles = HandAngles {
    fingers: [
        f([-12.0, 0.0, 0.0], 45.0, 0.0),
        f([-5.0, 2.0, 0.0], 12.0, 0.0),
        f([-5.0, 2.0, 0.0], 0.0, 0.0),
        f([-5.0, 2.0, 0.0], 10.0, 0.0),
        f([-5.0, 2.0, 0.0], 22.0, 0.0),
    ],
};

/// A closed fist, the thumb over the middle joints of index and middle finger.
pub const FIST: HandAngles = HandAngles {
    fingers: [
        f([45.0, 50.0, 30.0], 20.0, 35.0),
        f([88.0, 105.0, 65.0], -2.0, 0.0),
        f([90.0, 108.0, 70.0], 0.0, 0.0),
        f([92.0, 110.0, 72.0], -3.0, 0.0),
        f([95.0, 110.0, 75.0], -6.0, 0.0),
    ],
};

/// Index out, the rest in a fist.
pub const POINT: HandAngles = HandAngles {
    fingers: [
        f([40.0, 40.0, 25.0], 18.0, 30.0),
        f([0.0, 4.0, 2.0], 2.0, 0.0),
        f([88.0, 105.0, 65.0], 0.0, 0.0),
        f([92.0, 110.0, 70.0], -3.0, 0.0),
        f([95.0, 110.0, 75.0], -6.0, 0.0),
    ],
};

/// Thumb and index tips together, the other fingers loosely curled.
pub const PINCH: HandAngles = HandAngles {
    fingers: [
        f([38.0, 22.0, 18.0], 40.0, 45.0),
        f([48.0, 42.0, 22.0], -4.0, 0.0),
        f([38.0, 45.0, 20.0], 0.0, 0.0),
        f([45.0, 55.0, 25.0], 3.0, 0.0),
        f([55.0, 60.0, 25.0], 8.0, 0.0),
    ],
};

/// Drawing a bowstring: index, middle and ring hooked on the string, the thumb tucked.
pub const BOW: HandAngles = HandAngles {
    fingers: [
        f([35.0, 40.0, 30.0], 10.0, 25.0),
        f([20.0, 80.0, 50.0], 2.0, 0.0),
        f([22.0, 82.0, 52.0], 0.0, 0.0),
        f([26.0, 84.0, 55.0], -2.0, 0.0),
        f([60.0, 95.0, 60.0], -4.0, 0.0),
    ],
};

/// A grip closing on a handle of radius `r` (m): a thin hilt (1.5 cm) closes nearly to a
/// fist, a thick shaft (3.5 cm) only half way; the cascade is kept.
pub fn power_grip(r: f32) -> HandAngles {
    let k = ((0.035 - r) / 0.02).clamp(0.0, 1.0);
    let thick = HandAngles {
        fingers: [
            f([30.0, 25.0, 20.0], 30.0, 20.0),
            f([40.0, 58.0, 30.0], 0.0, 0.0),
            f([44.0, 60.0, 32.0], 0.0, 0.0),
            f([48.0, 62.0, 34.0], -2.0, 0.0),
            f([52.0, 64.0, 36.0], -4.0, 0.0),
        ],
    };
    let thin = HandAngles {
        fingers: [
            f([42.0, 40.0, 30.0], 25.0, 30.0),
            f([68.0, 98.0, 55.0], -1.0, 0.0),
            f([72.0, 100.0, 58.0], 0.0, 0.0),
            f([76.0, 102.0, 60.0], -2.0, 0.0),
            f([80.0, 104.0, 62.0], -5.0, 0.0),
        ],
    };
    lerp(&thick, &thin, k)
}

/// Through a shield's straps, the fingers round its grip, the thumb along it.
pub fn shield_strap(r: f32) -> HandAngles {
    let mut a = power_grip(r.max(0.02));
    a.fingers[0] = f([18.0, 10.0, 5.0], 15.0, 10.0);
    a
}

/// A shape at full weight (`r`: the handle's radius for grips, m; 0 for a sword's hilt).
pub fn shape(shape: HandShape, r: f32) -> HandAngles {
    let r = if r > 0.0 { r } else { 0.017 };
    match shape {
        HandShape::Relaxed => RELAXED,
        HandShape::Fist => FIST,
        HandShape::PowerGrip => power_grip(r),
        HandShape::ShieldStrap => shield_strap(r),
        HandShape::Bow => BOW,
        HandShape::Open => OPEN,
        HandShape::Point => POINT,
        HandShape::Pinch => PINCH,
    }
}

/// Every shape, in the order the creator lists them.
pub const SHAPES: [HandShape; 8] = [
    HandShape::Relaxed,
    HandShape::Open,
    HandShape::Fist,
    HandShape::PowerGrip,
    HandShape::ShieldStrap,
    HandShape::Bow,
    HandShape::Point,
    HandShape::Pinch,
];

/// A shape's name in the creator.
pub fn shape_name(s: HandShape) -> &'static str {
    match s {
        HandShape::Relaxed => "Relaxed",
        HandShape::Open => "Open",
        HandShape::Fist => "Fist",
        HandShape::PowerGrip => "Grip",
        HandShape::ShieldStrap => "Shield",
        HandShape::Bow => "Bow",
        HandShape::Point => "Point",
        HandShape::Pinch => "Pinch",
    }
}

/// `a` toward `b` by `t`.
pub fn lerp(a: &HandAngles, b: &HandAngles, t: f32) -> HandAngles {
    let mut out = *a;
    for (o, (x, y)) in out
        .fingers
        .iter_mut()
        .zip(a.fingers.iter().zip(b.fingers.iter()))
    {
        for k in 0..3 {
            o.bend[k] = x.bend[k] + (y.bend[k] - x.bend[k]) * t;
        }
        o.spread = x.spread + (y.spread - x.spread) * t;
        o.roll = x.roll + (y.roll - x.roll) * t;
    }
    out
}

/// Angles clamped to [`ROM`].
pub fn clamped(mut a: HandAngles) -> HandAngles {
    for (finger, (bends, spread, roll)) in a.fingers.iter_mut().zip(ROM) {
        for (bend, range) in finger.bend.iter_mut().zip(bends) {
            *bend = bend.clamp(range.0, range.1);
        }
        finger.spread = finger.spread.clamp(spread.0, spread.1);
        finger.roll = finger.roll.clamp(roll.0, roll.1);
    }
    a
}

/// One hand's angles from its pose: relaxed toward the shape by its weight, then each finger
/// with a curl override bent from open (0) to a fist (1).
pub fn angles(p: &HandPose) -> HandAngles {
    let mut a = lerp(
        &RELAXED,
        &shape(p.shape, p.handle_radius_m),
        p.weight.clamp(0.0, 1.0),
    );
    if let Some(curl) = p.curl {
        for (finger, c) in curl.iter().enumerate() {
            let c = c.clamp(0.0, 1.0);
            for k in 0..3 {
                let open = OPEN.fingers[finger].bend[k];
                a.fingers[finger].bend[k] = open + (FIST.fingers[finger].bend[k] - open) * c;
            }
        }
    }
    clamped(a)
}

/// The angles of hand `side` (0 left, 1 right) from `HandControls`: its pose, or for a writer
/// that only sets `grip`, relaxed closing to a grip on a hilt.
pub fn from_controls(h: &HandControls, side: usize) -> HandAngles {
    let p = h.hands.get(side).copied().unwrap_or_default();
    if p == HandPose::default() {
        let g = h.grip(side);
        return angles(&HandPose {
            shape: HandShape::PowerGrip,
            weight: g,
            ..Default::default()
        });
    }
    angles(&p)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_relaxed_hand_cascades_and_every_shape_stays_in_range() {
        let r = angles(&HandPose::default());
        for f in 1..4 {
            for k in 0..3 {
                assert!(r.fingers[f + 1].bend[k] > r.fingers[f].bend[k], "{f} {k}");
            }
        }
        for s in SHAPES {
            for radius in [0.0, 0.01, 0.02, 0.04] {
                let a = angles(&HandPose {
                    shape: s,
                    weight: 1.0,
                    handle_radius_m: radius,
                    curl: None,
                });
                assert_eq!(a, clamped(a), "{s:?}");
            }
        }
    }

    #[test]
    fn grips_close_more_on_thin_handles_and_curl_overrides_fingers() {
        let thin = power_grip(0.015);
        let thick = power_grip(0.035);
        for f in 1..5 {
            assert!(thin.fingers[f].bend[1] > thick.fingers[f].bend[1] + 20.0);
        }
        let p = HandPose {
            shape: HandShape::Fist,
            weight: 1.0,
            curl: Some([1.0, 0.0, 1.0, 1.0, 1.0]),
            ..Default::default()
        };
        let a = angles(&p);
        assert_eq!(a.fingers[1].bend, OPEN.fingers[1].bend);
        assert_eq!(a.fingers[2].bend, FIST.fingers[2].bend);
    }

    #[test]
    fn a_grip_alone_closes_the_hand_on_a_hilt() {
        let h = HandControls {
            grip: [0.0, 1.0],
            ..Default::default()
        };
        assert_eq!(from_controls(&h, 0), angles(&HandPose::default()));
        assert_eq!(
            from_controls(&h, 1),
            clamped(lerp(&RELAXED, &power_grip(0.017), 1.0))
        );
    }
}
