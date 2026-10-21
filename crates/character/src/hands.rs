//! Finger curl: how closed each hand is (`HandControls::grip`) bends the fingers toward the
//! palm and the thumb across it.
//!
//! The pack's skeleton has no finger bones, so the fingers bend in the skin shader instead:
//! every finger bends at its three joints, the palm side inside, as far as a fist's at a full
//! grip, so the tips come back to the palm. The thumb turns about the hand's long axis toward the palm. Each hand's frame
//! (knuckle line, long axis, palm normal) is measured from the built body; [`curl`] is the
//! shader's `curl_hand` in Rust, for the tests.

use engine_core::math::{Vec2, Vec3};

use crate::pack::HumanPack;

/// Where a finger's (or the thumb's) joints are, as a share of its length from its root.
const JOINTS: [f32; 3] = [0.0, 0.45, 0.75];
/// How far each finger joint bends at a full grip, rad (knuckle, middle and end joints, as in
/// a fist), and the thumb's (its root turns instead, by [`THUMB`]).
const FINGER_BENDS: [f32; 3] = [1.6, 1.9, 1.25];
const THUMB_BENDS: [f32; 3] = [0.0, 0.7, 1.0];
/// Half the length of finger a joint bends over, for a typical hand, m.
const JOINT_SPAN: f32 = 0.007;
/// Steps along a finger to trace its bent line.
const STEPS: usize = 16;
/// How far the thumb turns across the palm at a full grip, rad.
pub const THUMB: f32 = 0.9;
/// The knuckles, as a share of the hand's length from the wrist.
const KNUCKLES: f32 = 0.45;
/// A typical hand's length, wrist to fingertip, m.
const HAND: f32 = 0.177;
/// The thumb turns about an axis this far from the wrist toward it, for a typical hand, m.
const THUMB_ROOT: f32 = 0.015;

/// One hand's frame, in the built body's model space.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct HandFrame {
    /// The middle of the knuckle line, at the fingers' mid-thickness.
    pub knuckles: Vec3,
    /// From the knuckles to the fingertips, m.
    pub fingers: f32,
    /// Along the hand, wrist to fingertips.
    pub along: Vec3,
    /// Out of the palm.
    pub palm: Vec3,
    /// The wrist joint.
    pub wrist: Vec3,
    /// Across the hand toward the thumb.
    pub thumb: Vec3,
    /// The thumb's tip.
    pub thumb_tip: Vec3,
    /// The hand's bone (the skin's bone index).
    pub bone: u32,
}

impl HandFrame {
    /// The frame as the shader reads it: knuckles and finger length, along and bone, palm and
    /// which side the thumb is on (of along × palm), wrist, the thumb's tip.
    pub fn gpu(&self) -> [[f32; 4]; 5] {
        let e = |v: Vec3, w: f32| [v.x, v.y, v.z, w];
        let side = self.thumb.dot(self.along.cross(self.palm)).signum();
        [
            e(self.knuckles, self.fingers),
            e(self.along, self.bone as f32),
            e(self.palm, side),
            e(self.wrist, 0.0),
            e(self.thumb_tip, 0.0),
        ]
    }
}

/// Both hands' frames (left, right) of a morphed body. `None` when the pack has no hands.
pub fn frames(pack: &HumanPack, morphed: &[Vec3]) -> Option<[HandFrame; 2]> {
    let left = frame(pack, morphed, "hand_l")?;
    let right = frame(pack, morphed, "hand_r")?;
    Some([left, right])
}

fn frame(pack: &HumanPack, morphed: &[Vec3], name: &str) -> Option<HandFrame> {
    let bone = pack.bones.iter().position(|b| b == name)?;
    let joint = pack.joints.get(bone)?;
    if joint.is_empty() {
        return None;
    }
    let wrist = joint.iter().map(|v| morphed[*v as usize]).sum::<Vec3>() / joint.len() as f32;
    let body = pack.group("body")?;
    let mut seen = vec![false; pack.positions.len()];
    let mut pts: Vec<Vec3> = Vec::new();
    for q in pack.quads.iter().filter(|q| q.group == body) {
        for v in q.v {
            let v = v as usize;
            if seen[v] {
                continue;
            }
            seen[v] = true;
            let w = &pack.skin[v];
            if (0..4).any(|k| w.bones[k] as usize == bone && w.weights[k] > 128) {
                pts.push(morphed[v]);
            }
        }
    }
    if pts.len() < 32 {
        return None;
    }
    // Along: toward the fingertips, the far end of the hand from the wrist.
    let rough = (pts.iter().sum::<Vec3>() / pts.len() as f32 - wrist).normalize_or_zero();
    let reach = pts
        .iter()
        .map(|p| (*p - wrist).dot(rough))
        .fold(0.0f32, f32::max);
    let tips: Vec<Vec3> = pts
        .iter()
        .copied()
        .filter(|p| (*p - wrist).dot(rough) > reach * 0.85)
        .collect();
    let along = (tips.iter().sum::<Vec3>() / tips.len().max(1) as f32 - wrist).normalize_or_zero();
    if along == Vec3::ZERO {
        return None;
    }
    let length = pts
        .iter()
        .map(|p| (*p - wrist).dot(along))
        .fold(0.0f32, f32::max);
    // Across the hand: the palm is wide and thin, so its normal is the direction it varies
    // least in, across the long axis.
    let u = along.any_orthonormal_vector();
    let v = along.cross(u);
    let (mut xx, mut xy, mut yy) = (0.0f32, 0.0f32, 0.0f32);
    let palm_pts: Vec<Vec3> = pts
        .iter()
        .copied()
        .filter(|p| {
            let s = (*p - wrist).dot(along);
            s > length * 0.2 && s < length * KNUCKLES
        })
        .collect();
    let mid = palm_pts.iter().sum::<Vec3>() / palm_pts.len().max(1) as f32;
    for p in &palm_pts {
        let d = *p - mid;
        let (x, y) = (d.dot(u), d.dot(v));
        xx += x * x;
        xy += x * y;
        yy += y * y;
    }
    // The 2x2 covariance's smaller eigenvector.
    let half = (xx - yy) * 0.5;
    let small = (xx + yy) * 0.5 - (half * half + xy * xy).sqrt();
    let e = if xy.abs() > 1e-9 {
        Vec2::new(xy, small - xx)
    } else if xx < yy {
        Vec2::X
    } else {
        Vec2::Y
    };
    let e = e.normalize_or(Vec2::X);
    let mut palm = (u * e.x + v * e.y).normalize();
    // The thumb sticks out past the side of the palm, short of the knuckles; the other side
    // runs straight. It lies in front of the palm, which says which way the palm faces.
    let side = along.cross(palm).normalize();
    let across = |lo_s: f32, hi_s: f32| {
        pts.iter()
            .filter(|p| {
                let s = (**p - wrist).dot(along);
                s > length * lo_s && s < length * hi_s
            })
            .fold(((f32::MAX, mid), (f32::MIN, mid)), |(lo, hi), p| {
                let w = (*p - wrist).dot(side);
                (
                    if w < lo.0 { (w, *p) } else { lo },
                    if w > hi.0 { (w, *p) } else { hi },
                )
            })
    };
    let (palm_lo, palm_hi) = across(KNUCKLES - 0.08, KNUCKLES - 0.02);
    let (lo, hi) = across(0.25, 0.55);
    let (thumb, tip) = if hi.0 - palm_hi.0 > palm_lo.0 - lo.0 {
        (side, hi.1)
    } else {
        (-side, lo.1)
    };
    let line = |p: Vec3| wrist + along * (p - wrist).dot(along);
    if (tip - line(tip)).dot(palm) < 0.0 {
        palm = -palm;
    }
    // The fingers' own middle, through their thickness (not the thumb's).
    let size = length / HAND;
    let fingers: Vec<Vec3> = pts
        .iter()
        .copied()
        .filter(|p| {
            let s = (*p - wrist).dot(along);
            s > length * KNUCKLES && (*p - wrist).dot(thumb) < thumb_edge(s, size)
        })
        .collect();
    let depth =
        fingers.iter().map(|p| (*p - wrist).dot(palm)).sum::<f32>() / fingers.len().max(1) as f32;
    let knuckles = wrist + along * (length * KNUCKLES) + palm * depth;
    Some(HandFrame {
        knuckles,
        fingers: length * (1.0 - KNUCKLES),
        along,
        palm,
        wrist,
        thumb,
        thumb_tip: tip,
        bone: bone as u32,
    })
}

/// How far toward the thumb a point must be to belong to the thumb rather than the index
/// finger, by how far along the hand it is (from the wrist; `size`: the hand's length over a
/// typical hand's).
fn thumb_edge(s: f32, size: f32) -> f32 {
    (0.012 + 0.42 * s / size).min(0.05) * size
}

/// A point and normal of a hand bent by `grip` (0 open .. 1 closed): `curl_hand` in
/// common.wgsl.
pub fn curl(p: Vec3, n: Vec3, h: &HandFrame, grip: f32) -> (Vec3, Vec3) {
    let grip = grip.clamp(0.0, 1.0);
    if grip <= 0.0 {
        return (p, n);
    }
    let size = h.fingers / (1.0 - KNUCKLES) / HAND;
    let v = p - h.wrist;
    let edge = thumb_edge(v.dot(h.along), size);
    // Not the wrist (or a cuff around it) on the thumb's side.
    let thumbness = smooth(edge - 0.004, edge + 0.004, v.dot(h.thumb))
        * smooth(0.0, 0.02 * size, v.dot(h.along));
    let (mut p, mut n) = (p, n);
    // The fingers bend at their joints, the palm side inside, and so does the thumb.
    let span = JOINT_SPAN * size;
    (p, n) = bend_digit(
        (p, n),
        (h.knuckles, h.along, h.palm),
        h.fingers,
        FINGER_BENDS.map(|b| b * grip * (1.0 - thumbness)),
        span,
    );
    let root = h.wrist + h.thumb * (THUMB_ROOT * size);
    let toward = (h.thumb_tip - root).normalize_or(h.along);
    // The thumb flexes across the palm, toward the little finger.
    let across = h.palm * 0.3 - h.thumb;
    let up = (across - toward * across.dot(toward)).normalize_or(h.palm);
    (p, n) = bend_digit(
        (p, n),
        (root, toward, up),
        (h.thumb_tip - root).length(),
        THUMB_BENDS.map(|b| b * grip * thumbness),
        span,
    );
    // The thumb turns about an axis along the hand, at its root, toward the palm.
    let a = grip * THUMB * thumbness;
    if a > 0.0 {
        let v = p - root;
        let (x, y) = (v.dot(h.thumb), v.dot(h.palm));
        let (sin, cos) = a.sin_cos();
        p += h.thumb * (x * cos - y * sin - x) + h.palm * (x * sin + y * cos - y);
        n = bend(n, h.thumb, h.palm, sin, cos);
    }
    (p, n)
}

/// A point and normal of a finger (or thumb) bent at its joints (`root`, `along` it and `up`
/// toward the palm side; `len` its length; `bends` each joint's angle, rad; `span` half the
/// length a joint bends over): traces the bent middle line of the finger out to the point and
/// carries the point's height above it round with it.
fn bend_digit(
    (p, n): (Vec3, Vec3),
    (root, along, up): (Vec3, Vec3, Vec3),
    len: f32,
    bends: [f32; 3],
    span: f32,
) -> (Vec3, Vec3) {
    let d = p - root;
    let s = d.dot(along);
    if s <= -span || bends.iter().all(|b| *b <= 0.0) {
        return (p, n);
    }
    let angle = |t: f32| {
        JOINTS
            .iter()
            .zip(bends)
            .map(|(at, bend)| bend * smooth(at * len - span, at * len + span, t))
            .sum::<f32>()
    };
    let step = (s + span) / STEPS as f32;
    let mut line = along * -span;
    for i in 0..STEPS {
        let (sin, cos) = angle(-span + (i as f32 + 0.5) * step).sin_cos();
        line += (along * cos + up * sin) * step;
    }
    let height = d.dot(up);
    let (sin, cos) = angle(s).sin_cos();
    (
        p + line - along * s - up * height + (up * cos - along * sin) * height,
        bend(n, along, up, sin, cos),
    )
}

/// `n` turned in the plane of `a` and `b`, from `a` toward `b`.
fn bend(n: Vec3, a: Vec3, b: Vec3, sin: f32, cos: f32) -> Vec3 {
    let (x, y) = (n.dot(a), n.dot(b));
    n - a * x - b * y + a * (x * cos - y * sin) + b * (x * sin + y * cos)
}

fn smooth(e0: f32, e1: f32, x: f32) -> f32 {
    let t = ((x - e0) / (e1 - e0)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hand() -> HandFrame {
        HandFrame {
            knuckles: Vec3::new(0.0, -0.1, 0.0),
            fingers: 0.085,
            along: Vec3::NEG_Y,
            palm: Vec3::Z,
            wrist: Vec3::ZERO,
            thumb: Vec3::X,
            thumb_tip: Vec3::new(0.07, -0.09, 0.01),
            bone: 0,
        }
    }

    #[test]
    fn a_full_grip_brings_the_fingertips_back_to_the_palm() {
        let h = hand();
        let tip = h.knuckles + h.along * h.fingers;
        // Open: nothing moves.
        assert_eq!(curl(tip, h.along, &h, 0.0).0, tip);
        // Closed: the tip comes round in front of the palm, back toward the wrist.
        let (p, n) = curl(tip, h.along, &h, 1.0);
        assert!((p - h.knuckles).dot(h.along) < 0.03, "{p}");
        assert!((p - h.knuckles).dot(h.palm) > 0.005, "{p}");
        // Its direction turned with it: round into the palm.
        assert!(n.dot(h.palm) < -0.5, "{n}");
        // The back of the hand short of the knuckles stays put.
        let back = h.knuckles - h.along * 0.01;
        assert_eq!(curl(back, h.palm, &h, 1.0).0, back);
        // The palm doesn't move, and the thumb swings toward the palm.
        let palm = h.wrist + h.along * 0.05;
        assert_eq!(curl(palm, h.palm, &h, 1.0).0, palm);
        let thumb = h.wrist + h.along * 0.06 + h.thumb * 0.05;
        let (t, _) = curl(thumb, h.thumb, &h, 1.0);
        assert!((t - thumb).dot(h.palm) > 0.01, "{t}");
    }

    #[test]
    fn the_shader_bends_the_same() {
        let src = include_str!("shaders/common.wgsl");
        let v = |a: [f32; 3]| format!("vec3<f32>({:?}, {:?}, {:?})", a[0], a[1], a[2]);
        assert!(src.contains(&v(FINGER_BENDS)));
        assert!(src.contains(&v(THUMB_BENDS)));
        for (name, value) in [
            ("KNUCKLES", KNUCKLES),
            ("HAND", HAND),
            ("THUMB_ROOT", THUMB_ROOT),
            ("JOINT_SPAN", JOINT_SPAN),
            ("THUMB_TURN", THUMB),
        ] {
            assert!(
                src.contains(&format!("const {name}: f32 = {value:?};")),
                "{name}"
            );
        }
    }
}
