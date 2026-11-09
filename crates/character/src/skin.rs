//! The full skeleton's skin: MakeHuman's CC0 weights baked onto the humanoid's body bones, 15
//! finger bones per hand and the twist bones (`tools/bake_skin.py`, format in its docs). The
//! file is small and lives in the crate (`data/hm08_skin.bin`), so every checkout has it; it
//! replaces the human pack's 22-bone weights when the vertex counts match.
//!
//! Twist bones are driven, not posed: each one turns by a share of its limb's roll
//! ([`TWIST`]), so a turned wrist twists the whole forearm, not one ring of skin at the wrist.

use engine_core::math::{Quat, Vec3};

use crate::pack::{HumanPack, SkinWeights};

/// The baked skin (64 bones).
const SKIN: &[u8] = include_bytes!("../data/hm08_skin.bin");

/// The fingers, thumb first, as the bones name them (`index_02_l`).
pub const FINGERS: [&str; 5] = ["thumb", "index", "middle", "ring", "pinky"];

/// A driven twist bone: it sits `at` (share of the limb's length) along `limb`, toward
/// `child`, and takes `share` of a roll: the roll of `driver` about the limb (a forearm's from
/// the hand's) or, for a negative share, the limb's own roll from its parent, undone (the upper
/// arm's at the shoulder). Keep in step with `TWIST_NODES` in `tools/bake_skin.py`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Twist {
    pub bone: &'static str,
    pub limb: &'static str,
    pub child: &'static str,
    pub at: f32,
    pub share: f32,
}

/// The twist bones of one side (`_l` / `_r` appended).
pub const TWIST: [Twist; 6] = [
    Twist {
        bone: "upper_arm_twist_01",
        limb: "upper_arm",
        child: "lower_arm",
        at: 0.10,
        share: -0.9,
    },
    Twist {
        bone: "upper_arm_twist_02",
        limb: "upper_arm",
        child: "lower_arm",
        at: 0.45,
        share: -0.55,
    },
    Twist {
        bone: "lower_arm_twist_01",
        limb: "lower_arm",
        child: "hand",
        at: 0.45,
        share: 0.45,
    },
    Twist {
        bone: "lower_arm_twist_02",
        limb: "lower_arm",
        child: "hand",
        at: 0.85,
        share: 0.85,
    },
    Twist {
        bone: "thigh_twist_01",
        limb: "upper_leg",
        child: "lower_leg",
        at: 0.15,
        share: -0.85,
    },
    Twist {
        bone: "calf_twist_01",
        limb: "lower_leg",
        child: "foot",
        at: 0.75,
        share: 0.75,
    },
];

/// The parent of every bone the skin weights (the humanoid's, `engine_world`'s
/// `HUMANOID_BONES`, without the hair bones).
pub fn parent_of(bone: &str) -> Option<String> {
    let (base, side) = match bone.rsplit_once('_') {
        Some((b, s)) if s == "l" || s == "r" => (b, s),
        _ => (bone, ""),
    };
    let sided = |p: &str| Some(format!("{p}_{side}"));
    match base {
        "root" => None,
        "hips" => Some("root".into()),
        "spine" => Some("hips".into()),
        "chest" => Some("spine".into()),
        "neck" => Some("chest".into()),
        "head" => Some("neck".into()),
        "clavicle" => Some("chest".into()),
        "upper_arm" => sided("clavicle"),
        "lower_arm" => sided("upper_arm"),
        "hand" => sided("lower_arm"),
        "upper_leg" => Some("hips".into()),
        "lower_leg" => sided("upper_leg"),
        "foot" => sided("lower_leg"),
        "toe" => sided("foot"),
        _ => {
            if let Some(t) = TWIST.iter().find(|t| t.bone == base) {
                return sided(t.limb);
            }
            let (finger, joint) = base.rsplit_once('_')?;
            FINGERS.contains(&finger).then_some(())?;
            match joint {
                "01" => sided("hand"),
                "02" => sided(&format!("{finger}_01")),
                "03" => sided(&format!("{finger}_02")),
                _ => None,
            }
        }
    }
}

/// `(finger, joint 0..3)` of a finger bone (`index_02_l` → `(1, 1)`).
pub fn finger_of(bone: &str) -> Option<(usize, usize)> {
    let base = bone
        .strip_suffix("_l")
        .or_else(|| bone.strip_suffix("_r"))?;
    let (finger, joint) = base.rsplit_once('_')?;
    let f = FINGERS.iter().position(|n| *n == finger)?;
    let j = match joint {
        "01" => 0,
        "02" => 1,
        "03" => 2,
        _ => return None,
    };
    Some((f, j))
}

/// Whether a bone is a driven twist bone.
pub fn is_twist(bone: &str) -> bool {
    bone.contains("_twist_")
}

/// The twist of `q` about `axis` (unit): the swing-twist split's twist part.
pub fn twist_about(q: Quat, axis: Vec3) -> Quat {
    let v = Vec3::new(q.x, q.y, q.z);
    let p = axis * v.dot(axis);
    let t = Quat::from_xyzw(p.x, p.y, p.z, q.w);
    if t.length_squared() < 1e-12 {
        Quat::IDENTITY
    } else {
        t.normalize()
    }
}

/// The baked skin, parsed: bones, weights per vertex and joints (bone joints and tips).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Skin {
    pub bones: Vec<String>,
    pub weights: Vec<SkinWeights>,
    pub joints: Vec<(String, Vec<u32>)>,
}

struct Reader<'a> {
    bytes: &'a [u8],
    at: usize,
}

impl<'a> Reader<'a> {
    fn take(&mut self, n: usize) -> Result<&'a [u8], String> {
        let s = self
            .bytes
            .get(self.at..self.at + n)
            .ok_or("the baked skin ends early")?;
        self.at += n;
        Ok(s)
    }
    fn u32(&mut self) -> Result<u32, String> {
        let b = self.take(4)?;
        Ok(u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
    }
    fn string(&mut self) -> Result<String, String> {
        let n = self.take(2)?;
        let n = u16::from_le_bytes([n[0], n[1]]) as usize;
        String::from_utf8(self.take(n)?.to_vec()).map_err(|_| "a bone name isn't UTF-8".into())
    }
}

/// Parse a baked skin file.
pub fn parse(bytes: &[u8]) -> Result<Skin, String> {
    let mut r = Reader { bytes, at: 0 };
    if r.take(4)? != b"MHSK" {
        return Err("not a baked skin (no MHSK header)".into());
    }
    let version = r.u32()?;
    if version != 1 {
        return Err(format!("baked skin version {version} is not supported"));
    }
    let nv = r.u32()? as usize;
    let nb = r.u32()? as usize;
    if nb > 255 || nv > 1 << 22 {
        return Err("the baked skin is damaged".into());
    }
    let mut bones = Vec::with_capacity(nb);
    for _ in 0..nb {
        bones.push(r.string()?);
    }
    let mut weights = Vec::with_capacity(nv);
    for _ in 0..nv {
        let b = r.take(8)?;
        let w = SkinWeights {
            bones: [b[0], b[1], b[2], b[3]],
            weights: [b[4], b[5], b[6], b[7]],
        };
        if w.bones.iter().any(|i| *i as usize >= nb) {
            return Err("the baked skin names a bone it doesn't have".into());
        }
        weights.push(w);
    }
    let nj = r.u32()? as usize;
    let mut joints = Vec::with_capacity(nj.min(1024));
    for _ in 0..nj {
        let name = r.string()?;
        let n = r.u32()? as usize;
        let mut verts = Vec::with_capacity(n.min(1 << 16));
        for _ in 0..n {
            let v = r.u32()?;
            if v as usize >= nv {
                return Err("a joint of the baked skin is out of range".into());
            }
            verts.push(v);
        }
        joints.push((name, verts));
    }
    Ok(Skin {
        bones,
        weights,
        joints,
    })
}

/// The crate's baked skin.
pub fn baked() -> Skin {
    parse(SKIN).unwrap_or_else(|e| {
        tracing::warn!("character skin: {e}");
        Skin::default()
    })
}

/// Give a pack the full skeleton's skin (fingers and twist), when it is the base mesh the skin
/// was baked for. Returns whether it did.
pub fn upgrade(pack: &mut HumanPack, skin: &Skin) -> bool {
    if !pack.has_rig() || skin.weights.len() != pack.positions.len() || skin.bones.is_empty() {
        return false;
    }
    let joint = |name: &str| {
        skin.joints
            .iter()
            .find(|(n, _)| n == name)
            .map(|(_, v)| v.clone())
            .unwrap_or_default()
    };
    pack.joints = skin.bones.iter().map(|b| joint(b)).collect();
    pack.tips = skin
        .joints
        .iter()
        .filter(|(n, _)| n.contains("_tip_"))
        .cloned()
        .collect();
    pack.bones = skin.bones.clone();
    pack.skin = skin.weights.clone();
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_baked_skin_has_every_finger_and_twist_bone_with_its_parent() {
        let skin = baked();
        assert_eq!(skin.bones.len(), 64);
        assert_eq!(skin.weights.len(), 19158);
        let world: Vec<&str> = engine_world::animation::HUMANOID_BONES
            .iter()
            .map(|b| b.0)
            .filter(|b| !b.starts_with("ponytail"))
            .collect();
        for b in &world {
            assert!(skin.bones.iter().any(|s| s == b), "{b} missing");
        }
        for (name, parent) in engine_world::animation::HUMANOID_BONES {
            if name.starts_with("ponytail") {
                continue;
            }
            assert_eq!(parent_of(name).as_deref(), parent, "{name}");
        }
        // Every finger bone moves some skin; every bone but the root and twist bones has a
        // joint, and every finger a tip.
        for (i, b) in skin.bones.iter().enumerate() {
            let n = skin
                .weights
                .iter()
                .filter(|w| (0..4).any(|k| w.bones[k] as usize == i && w.weights[k] > 0))
                .count();
            if b != "root" {
                assert!(n > 50, "{b} moves {n} vertices");
            }
            let joint = skin.joints.iter().find(|(n, _)| n == b);
            assert_eq!(joint.is_some(), !is_twist(b), "{b}");
        }
        for s in ["l", "r"] {
            for f in FINGERS {
                assert!(
                    skin.joints
                        .iter()
                        .any(|(n, v)| *n == format!("{f}_tip_{s}") && !v.is_empty())
                );
            }
        }
        for w in &skin.weights {
            let sum: u32 = w.weights.iter().map(|x| *x as u32).sum();
            assert!(sum == 255 || sum == 0);
        }
    }

    #[test]
    fn fingers_and_twists_are_named_as_the_humanoid_names_them() {
        assert_eq!(finger_of("index_02_l"), Some((1, 1)));
        assert_eq!(finger_of("thumb_03_r"), Some((0, 2)));
        assert_eq!(finger_of("hand_l"), None);
        assert_eq!(finger_of("lower_arm_twist_01_l"), None);
        assert!(is_twist("calf_twist_01_r"));
        assert_eq!(parent_of("calf_twist_01_r").as_deref(), Some("lower_leg_r"));
    }

    #[test]
    fn twist_is_the_roll_about_the_axis() {
        let axis = Vec3::Y;
        let roll = Quat::from_rotation_y(0.7);
        let swing = Quat::from_rotation_x(0.4);
        let t = twist_about(swing * roll, axis);
        // A swing about a perpendicular axis after the roll: the twist part is close to the
        // roll (exact for swing-twist of `swing * roll` decomposed as swing' * twist).
        let (ax, angle) = t.to_axis_angle();
        assert!(ax.dot(axis).abs() > 0.999);
        assert!((angle - 0.7).abs() < 0.05, "{angle}");
        assert_eq!(
            twist_about(Quat::from_rotation_x(0.5), axis),
            Quat::IDENTITY
        );
    }
}
