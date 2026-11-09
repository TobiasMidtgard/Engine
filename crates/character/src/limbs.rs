//! Limb proportions: longer or shorter arms, legs, fingers, neck and torso, broader shoulders,
//! bigger hands and feet, on top of MakeHuman's whole-body shapes.
//!
//! Each proportion is a slider in `Appearance::shape` (`limb-forearm`, −1..1, 0 as built).
//! The base mesh is reshaped on its own skeleton before anything else is built from it: every
//! vertex moves with the bones it is weighted to, each bone's segment stretched along itself
//! (lengths) or scaled about its joint (hands and feet, whose fingers and toes scale with
//! them), and every joint carried to where its parent's stretched segment puts it. Skin,
//! clothes, the rig and `BodyFit` then all see the new proportions. Longer legs lift the body
//! so the soles stay on the ground.

use engine_core::math::{Mat3, Vec3};

use crate::appearance::Appearance;
use crate::pack::HumanPack;
use crate::skin;

/// How a proportion reshapes its bones.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Stretch {
    /// Along each bone's segment (length only).
    Length,
    /// Every direction about the joint, carried to the bones beyond it (a hand's fingers).
    Size,
}

/// One proportion slider.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Limb {
    /// Slider key in `Appearance::shape`.
    pub key: &'static str,
    pub label: &'static str,
    /// The bones without their side (`_l`/`_r` are both reshaped).
    pub bones: &'static [&'static str],
    pub stretch: Stretch,
    /// Scale at the slider's ends: 1 ± this.
    pub range: f32,
}

/// Every proportion, in the order the creator lists them.
pub const LIMBS: [Limb; 11] = [
    Limb {
        key: "limb-shoulders",
        label: "Shoulder width",
        bones: &["clavicle"],
        stretch: Stretch::Length,
        range: 0.3,
    },
    Limb {
        key: "limb-upper-arm",
        label: "Upper arm length",
        bones: &["upper_arm"],
        stretch: Stretch::Length,
        range: 0.22,
    },
    Limb {
        key: "limb-forearm",
        label: "Forearm length",
        bones: &["lower_arm"],
        stretch: Stretch::Length,
        range: 0.22,
    },
    Limb {
        key: "limb-hand",
        label: "Hand size",
        bones: &["hand"],
        stretch: Stretch::Size,
        range: 0.2,
    },
    Limb {
        key: "limb-fingers",
        label: "Finger length",
        bones: &[
            "index_01",
            "index_02",
            "index_03",
            "middle_01",
            "middle_02",
            "middle_03",
            "ring_01",
            "ring_02",
            "ring_03",
            "pinky_01",
            "pinky_02",
            "pinky_03",
        ],
        stretch: Stretch::Length,
        range: 0.25,
    },
    Limb {
        key: "limb-thumb",
        label: "Thumb length",
        bones: &["thumb_01", "thumb_02", "thumb_03"],
        stretch: Stretch::Length,
        range: 0.25,
    },
    Limb {
        key: "limb-torso",
        label: "Torso length",
        bones: &["spine", "chest"],
        stretch: Stretch::Length,
        range: 0.15,
    },
    Limb {
        key: "limb-neck",
        label: "Neck length",
        bones: &["neck"],
        stretch: Stretch::Length,
        range: 0.4,
    },
    Limb {
        key: "limb-thigh",
        label: "Thigh length",
        bones: &["upper_leg"],
        stretch: Stretch::Length,
        range: 0.18,
    },
    Limb {
        key: "limb-shin",
        label: "Shin length",
        bones: &["lower_leg"],
        stretch: Stretch::Length,
        range: 0.18,
    },
    Limb {
        key: "limb-foot",
        label: "Foot size",
        bones: &["foot"],
        stretch: Stretch::Size,
        range: 0.2,
    },
];

/// The proportion slider with this key.
pub fn limb(key: &str) -> Option<&'static Limb> {
    LIMBS.iter().find(|l| l.key == key)
}

fn base(bone: &str) -> &str {
    bone.strip_suffix("_l")
        .or_else(|| bone.strip_suffix("_r"))
        .unwrap_or(bone)
}

/// Where a bone's segment ends (its child's joint or a finger's tip), base mesh space.
fn segment_end(pack: &HumanPack, joints: &[Vec3], b: usize, tips: &[Option<Vec3>]) -> Option<Vec3> {
    if let Some(t) = tips[b] {
        return Some(t);
    }
    let name = &pack.bones[b];
    // The child along the limb: the first bone whose parent this is, preferring the limb's
    // own (a hand's middle finger, a chest's neck).
    let prefer = match base(name) {
        "hand" => Some("middle_01"),
        "chest" => Some("neck"),
        "hips" => Some("spine"),
        _ => None,
    };
    let children: Vec<usize> = (0..pack.bones.len())
        .filter(|c| {
            skin::parent_of(&pack.bones[*c]).as_deref() == Some(name.as_str())
                && !skin::is_twist(&pack.bones[*c])
        })
        .collect();
    let pick = prefer
        .and_then(|p| children.iter().find(|c| base(&pack.bones[**c]) == p))
        .or(children.first())?;
    Some(joints[*pick])
}

/// Reshape `morphed` (the base mesh after the shape targets) for `a`'s proportions.
pub fn apply(pack: &HumanPack, a: &Appearance, morphed: &mut [Vec3]) {
    if !pack.has_rig() || morphed.len() != pack.positions.len() {
        return;
    }
    let value = |l: &Limb| a.shape.get(l.key).copied().unwrap_or(0.0).clamp(-1.0, 1.0);
    if LIMBS.iter().all(|l| value(l) == 0.0) {
        return;
    }
    let n = pack.bones.len();
    let mean = |verts: &[u32]| {
        verts.iter().map(|v| morphed[*v as usize]).sum::<Vec3>() / verts.len().max(1) as f32
    };
    let index = |name: &str| pack.bones.iter().position(|b| b == name);
    let parents: Vec<Option<usize>> = pack
        .bones
        .iter()
        .map(|b| skin::parent_of(b).and_then(|p| index(&p)))
        .collect();
    let mut joints: Vec<Vec3> = (0..n)
        .map(|b| pack.joints.get(b).map_or(Vec3::ZERO, |v| mean(v)))
        .collect();
    // Twist bones move with their limbs: same joint, same reshaping.
    let carrier: Vec<usize> = (0..n)
        .map(|b| {
            if skin::is_twist(&pack.bones[b]) {
                parents[b].unwrap_or(b)
            } else {
                b
            }
        })
        .collect();
    for b in 0..n {
        if carrier[b] != b {
            joints[b] = joints[carrier[b]];
        }
    }
    let mut tips = vec![None; n];
    for (name, verts) in &pack.tips {
        if let Some((finger, side)) = name.split_once("_tip_")
            && let Some(b) = index(&format!("{finger}_03_{side}"))
            && !verts.is_empty()
        {
            tips[b] = Some(mean(verts));
        }
    }
    // Each bone's own reshaping, and the size it carries to the bones beyond it.
    let mut own = vec![Mat3::IDENTITY; n];
    let mut carried = vec![1.0f32; n];
    for b in 0..n {
        let c = carrier[b];
        let inherited = parents[c].map_or(1.0, |p| carried[p]);
        let mut size = 1.0;
        let mut m = Mat3::IDENTITY;
        for l in LIMBS
            .iter()
            .filter(|l| l.bones.contains(&base(&pack.bones[c])))
        {
            let s = 1.0 + value(l) * l.range;
            match l.stretch {
                Stretch::Size => size *= s,
                Stretch::Length => {
                    if let Some(end) = segment_end(pack, &joints, c, &tips) {
                        let d = (end - joints[c]).normalize_or_zero();
                        m = (Mat3::IDENTITY + (s - 1.0) * outer(d, d)) * m;
                    }
                }
            }
        }
        // Sizes carry to fingers and toes only (a longer forearm doesn't grow the hand).
        let carries = matches!(base(&pack.bones[c]), "hand" | "foot")
            || skin::finger_of(&pack.bones[c]).is_some()
            || base(&pack.bones[c]) == "toe";
        carried[b] = if carries { inherited * size } else { size };
        let hand_inherit = if carries { inherited } else { 1.0 };
        own[b] = m * (hand_inherit * size);
    }
    // New joints down the skeleton: each where its parent's reshaped segment puts it.
    let mut moved = joints.clone();
    for b in 0..n {
        if carrier[b] != b {
            continue;
        }
        if let Some(p) = parents[b] {
            moved[b] = moved[p] + own[p] * (joints[b] - joints[p]);
        }
    }
    for b in 0..n {
        moved[b] = moved[carrier[b]];
    }
    let before = morphed.to_vec();
    for (v, p) in morphed.iter_mut().enumerate() {
        let w = pack.skin[v];
        let total: u32 = w.weights.iter().map(|x| *x as u32).sum();
        if total == 0 {
            continue;
        }
        let mut out = Vec3::ZERO;
        for k in 0..4 {
            if w.weights[k] == 0 {
                continue;
            }
            let b = w.bones[k] as usize;
            out += (moved[b] + own[b] * (before[v] - joints[b])) * (w.weights[k] as f32 / 255.0);
        }
        *p = out;
    }
    // Stand on the same ground: the soles back where they were.
    let feet: Vec<usize> = ["foot_l", "foot_r"]
        .iter()
        .filter_map(|f| index(f))
        .collect();
    if !feet.is_empty() {
        let drop: f32 =
            feet.iter().map(|f| joints[*f].y - moved[*f].y).sum::<f32>() / feet.len() as f32;
        let ground = pack.group_vertices("joint-ground");
        let mut fixed = vec![false; morphed.len()];
        for g in ground {
            fixed[g as usize] = true;
        }
        for (v, p) in morphed.iter_mut().enumerate() {
            if !fixed[v] {
                p.y += drop;
            }
        }
    }
}

fn outer(a: Vec3, b: Vec3) -> Mat3 {
    Mat3::from_cols(a * b.x, a * b.y, a * b.z)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pack::SkinWeights;
    use engine_core::math::Vec2;

    /// An arm: shoulder, elbow, wrist on a line out along +X, a vertex on each segment.
    fn arm() -> HumanPack {
        let joints = [
            ("root", Vec3::new(0.0, -1.0, 0.0)),
            ("hips", Vec3::ZERO),
            ("upper_arm_l", Vec3::new(0.2, 0.5, 0.0)),
            ("lower_arm_l", Vec3::new(0.5, 0.5, 0.0)),
            ("hand_l", Vec3::new(0.8, 0.5, 0.0)),
            ("middle_01_l", Vec3::new(0.9, 0.5, 0.0)),
            ("foot_l", Vec3::new(0.1, -0.9, 0.0)),
        ];
        let mut positions: Vec<Vec3> = joints.iter().map(|j| j.1).collect();
        // Vertices halfway along the upper arm and forearm, and on the hand.
        positions.extend([
            Vec3::new(0.35, 0.55, 0.0),
            Vec3::new(0.65, 0.55, 0.0),
            Vec3::new(0.85, 0.5, 0.02),
        ]);
        let mut pack = HumanPack::new(positions, vec![Vec2::ZERO], vec![], vec![], vec![]);
        pack.bones = joints.iter().map(|j| j.0.to_string()).collect();
        let w = |b: u8| SkinWeights {
            bones: [b, 0, 0, 0],
            weights: [255, 0, 0, 0],
        };
        pack.skin = (0..7).map(|i| w(i as u8)).collect();
        // The joint vertices themselves follow their bone's parent (as a joint does).
        pack.skin[3] = w(2);
        pack.skin[4] = w(3);
        pack.skin[5] = w(4);
        pack.skin.extend([w(2), w(3), w(4)]);
        pack.joints = (0..7u32).map(|i| vec![i]).collect();
        pack
    }

    #[test]
    fn a_longer_forearm_moves_the_hand_out_and_keeps_its_size() {
        let pack = arm();
        let mut a = Appearance::default();
        a.shape.insert("limb-forearm".into(), 1.0);
        let mut m = pack.positions.clone();
        apply(&pack, &a, &mut m);
        let s = 1.0 + LIMBS[2].range;
        // The forearm's middle vertex: 0.15 from the elbow along +X becomes 0.15 s; off-axis
        // height stays.
        assert!((m[8].x - (0.5 + 0.15 * s)).abs() < 1e-5, "{}", m[8]);
        assert!((m[8].y - 0.55).abs() < 1e-5);
        // The hand moved out by the forearm's growth, at its own size.
        let grow = 0.3 * (s - 1.0);
        assert!(
            (m[9] - (pack.positions[9] + Vec3::X * grow)).length() < 1e-5,
            "{}",
            m[9]
        );
        // The upper arm didn't change.
        assert!((m[7] - pack.positions[7]).length() < 1e-6);
    }

    #[test]
    fn a_bigger_hand_scales_about_the_wrist_and_nothing_changes_at_zero() {
        let pack = arm();
        let mut m = pack.positions.clone();
        apply(&pack, &Appearance::default(), &mut m);
        assert_eq!(m, pack.positions);
        let mut a = Appearance::default();
        a.shape.insert("limb-hand".into(), 1.0);
        apply(&pack, &a, &mut m);
        let s = 1.0 + LIMBS[3].range;
        let wrist = pack.positions[4];
        assert!((m[9] - (wrist + (pack.positions[9] - wrist) * s)).length() < 1e-5);
    }
}
