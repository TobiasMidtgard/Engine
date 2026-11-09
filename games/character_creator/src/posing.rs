//! The design tool's pose: a base pose, every joint's own angles on top, each hand's shape
//! from the hand pose library with per-joint finger angles on top, and the person's look, all
//! in one file the creator saves and loads (`designs/<name>.json`), with undo.

use std::collections::BTreeMap;
use std::path::PathBuf;

use engine::prelude::*;
use engine::world::animation::SkeletonPose;
use engine::world::character::{HandPose, HandShape};
use engine_character::rig::{BoneKind, Rig};
use engine_character::{Appearance, anatomy, hands};
use serde::{Deserialize, Serialize};

/// One hand's shape in the tool.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct HandEdit {
    pub shape: HandShape,
    /// 0 relaxed .. 1 the full shape.
    pub weight: f32,
    /// A grip's handle radius, cm.
    pub handle_cm: f32,
}

impl Default for HandEdit {
    fn default() -> Self {
        Self {
            shape: HandShape::Relaxed,
            weight: 1.0,
            handle_cm: 1.7,
        }
    }
}

impl HandEdit {
    pub fn pose(&self) -> HandPose {
        HandPose {
            shape: self.shape,
            weight: self.weight,
            handle_radius_m: self.handle_cm * 0.01,
            curl: None,
        }
    }
}

/// The subject's pose in the design tool.
#[derive(Component, Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct PoseEdit {
    /// The base pose ([`crate::pose::POSES`]).
    pub base: String,
    /// Per bone: bend, side and twist, degrees, added to the base pose (fingers: added to the
    /// hand shape's angles). Every posable bone has an entry, so sliders can bind to it.
    pub joints: BTreeMap<String, [f32; 3]>,
    /// Left, right.
    pub hands: [HandEdit; 2],
    /// Edits on one side copy to the other.
    pub mirror: bool,
    /// The base pose moves (walk cycles, breathing); off holds it still.
    pub moving: bool,
}

impl Default for PoseEdit {
    fn default() -> Self {
        let mut joints = BTreeMap::new();
        for (bone, _) in engine::world::animation::HUMANOID_BONES {
            if anatomy::movements(bone).is_some() {
                joints.insert(bone.to_string(), [0.0; 3]);
            }
        }
        Self {
            base: "stand".into(),
            joints,
            hands: [HandEdit::default(); 2],
            mirror: true,
            moving: false,
        }
    }
}

impl PoseEdit {
    /// A base pose with nothing on top.
    pub fn base(name: &str) -> Self {
        Self {
            base: name.to_string(),
            ..Default::default()
        }
    }

    /// Every joint clamped to its range, missing joints added (after loading or editing).
    pub fn tidy(&mut self) {
        let full = Self::default();
        for (bone, zero) in full.joints {
            let a = self.joints.entry(bone.clone()).or_insert(zero);
            *a = anatomy::clamp(&bone, *a);
        }
        self.joints.retain(|b, _| anatomy::movements(b).is_some());
        for h in &mut self.hands {
            h.weight = h.weight.clamp(0.0, 1.0);
            h.handle_cm = h.handle_cm.clamp(0.5, 5.0);
        }
    }

    /// With `mirror` on: what changed since `before` on one side copied to the other.
    pub fn mirrored_from(&mut self, before: &PoseEdit) {
        if !self.mirror {
            return;
        }
        let changed: Vec<(String, [f32; 3])> = self
            .joints
            .iter()
            .filter(|(b, a)| before.joints.get(*b) != Some(*a))
            .map(|(b, a)| (b.clone(), *a))
            .collect();
        for (bone, a) in changed {
            let other = anatomy::mirror(&bone);
            if other != bone {
                self.joints.insert(other, a);
            }
        }
        for side in 0..2 {
            if self.hands[side] != before.hands[side] {
                self.hands[1 - side] = self.hands[side];
            }
        }
    }

    /// Joint angles back to the base pose (all, or one bone).
    pub fn reset(&mut self, bone: Option<&str>) {
        for (b, a) in self.joints.iter_mut() {
            if bone.is_none_or(|x| x == b) {
                *a = [0.0; 3];
            }
        }
        if bone.is_none() {
            self.hands = [HandEdit::default(); 2];
        }
    }
}

/// Local turns of every bone of `rig` for `edit` at time `t`: the base pose's turns times
/// the joint's own angles; fingers from the hand shape plus their own angles.
pub fn turns(
    rig: &Rig,
    edit: &PoseEdit,
    hand_override: Option<[HandPose; 2]>,
    t: f32,
) -> Vec<Quat> {
    let base = crate::pose::base_turns(&edit.base, if edit.moving { t } else { 0.35 });
    let hands: [hands::HandAngles; 2] = match hand_override {
        Some(h) => [hands::angles(&h[0]), hands::angles(&h[1])],
        None => [
            hands::angles(&edit.hands[0].pose()),
            hands::angles(&edit.hands[1].pose()),
        ],
    };
    rig.bones
        .iter()
        .enumerate()
        .map(|(b, name)| {
            let own = edit.joints.get(name).copied().unwrap_or([0.0; 3]);
            match rig.kind[b] {
                BoneKind::Finger(f, k, side) => {
                    let lib = hands[side as usize].fingers[f as usize];
                    let mut a = [lib.bend[k as usize], 0.0, 0.0];
                    if k == 0 {
                        a[1] = lib.spread;
                        a[2] = lib.roll;
                    }
                    for i in 0..3 {
                        a[i] += own[i];
                    }
                    anatomy::turn(rig, b, a)
                }
                BoneKind::Twist(..) => Quat::IDENTITY,
                BoneKind::Body => {
                    let from_base = base
                        .iter()
                        .filter(|(bone, _)| name == bone)
                        .fold(Quat::IDENTITY, |q, (_, r)| q * *r);
                    from_base * anatomy::turn(rig, b, own)
                }
            }
        })
        .collect()
}

/// The pose on `rig`'s rest skeleton for these local turns (forward kinematics).
pub fn skeleton(rig: &Rig, turns: &[Quat]) -> SkeletonPose {
    crate::pose::forward(&rig.rest_pose(), turns)
}

/// A saved design: the look and the pose.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Design {
    #[serde(default)]
    pub appearance: Appearance,
    pub pose: PoseEdit,
}

/// Where designs are saved.
pub fn designs_dir() -> PathBuf {
    PathBuf::from("designs")
}

/// Save a design as `designs/<name>.json`.
pub fn save(name: &str, design: &Design) -> Result<PathBuf, String> {
    let dir = designs_dir();
    std::fs::create_dir_all(&dir).map_err(|e| format!("can't make {}: {e}", dir.display()))?;
    let path = dir.join(format!("{name}.json"));
    let text = serde_json::to_string_pretty(design).map_err(|e| e.to_string())?;
    std::fs::write(&path, text).map_err(|e| format!("can't write {}: {e}", path.display()))?;
    Ok(path)
}

/// Load `designs/<name>.json` (or a path).
pub fn load(name: &str) -> Result<Design, String> {
    let path = if name.ends_with(".json") {
        PathBuf::from(name)
    } else {
        designs_dir().join(format!("{name}.json"))
    };
    let text = std::fs::read_to_string(&path)
        .map_err(|e| format!("can't read {}: {e}", path.display()))?;
    let mut d: Design =
        serde_json::from_str(&text).map_err(|e| format!("{}: {e}", path.display()))?;
    d.pose.tidy();
    Ok(d)
}

/// Undo and redo of the subject's look and pose.
#[derive(Resource, Debug, Default)]
pub struct History {
    undo: Vec<Design>,
    redo: Vec<Design>,
    /// The state last recorded.
    last: Option<Design>,
    /// Frames since the last change (a slider drag records once it settles).
    quiet: u32,
}

impl History {
    /// Call every frame with the current state: a change recorded once it holds still for a
    /// few frames, so one slider drag is one step.
    pub fn track(&mut self, now: &Design) {
        match &self.last {
            None => self.last = Some(now.clone()),
            Some(last) if last != now => {
                if self.quiet > 0 {
                    // The first frame of a change: remember where it started.
                    self.undo.push(last.clone());
                    if self.undo.len() > 200 {
                        self.undo.remove(0);
                    }
                    self.redo.clear();
                }
                self.quiet = 0;
                self.last = Some(now.clone());
            }
            _ => self.quiet = self.quiet.saturating_add(1),
        }
    }

    pub fn undo(&mut self, now: &Design) -> Option<Design> {
        let d = self.undo.pop()?;
        self.redo.push(now.clone());
        self.last = Some(d.clone());
        self.quiet = 1;
        Some(d)
    }

    pub fn redo(&mut self, now: &Design) -> Option<Design> {
        let d = self.redo.pop()?;
        self.undo.push(now.clone());
        self.last = Some(d.clone());
        self.quiet = 1;
        Some(d)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use engine_character::AppearanceExt;

    #[test]
    fn mirror_copies_a_changed_side_and_tidy_clamps() {
        let before = PoseEdit::default();
        let mut p = before.clone();
        p.joints.insert("upper_arm_l".into(), [40.0, 10.0, 0.0]);
        p.hands[1].shape = HandShape::Fist;
        p.mirrored_from(&before);
        assert_eq!(p.joints["upper_arm_r"], [40.0, 10.0, 0.0]);
        assert_eq!(p.hands[0].shape, HandShape::Fist);
        p.joints.insert("lower_arm_l".into(), [500.0, 3.0, 0.0]);
        p.joints.remove("head");
        p.tidy();
        assert_eq!(p.joints["lower_arm_l"], [150.0, 0.0, 0.0]);
        assert_eq!(p.joints["head"], [0.0; 3]);
        p.reset(Some("upper_arm_r"));
        assert_eq!(p.joints["upper_arm_r"], [0.0; 3]);
        assert_eq!(p.joints["upper_arm_l"], [40.0, 10.0, 0.0]);
    }

    #[test]
    fn a_drag_is_one_undo_step() {
        let mut h = History::default();
        let mut d = Design {
            appearance: Appearance::default(),
            pose: PoseEdit::default(),
        };
        h.track(&d);
        h.track(&d);
        let start = d.clone();
        for i in 1..10 {
            d.pose.joints.insert("head".into(), [i as f32, 0.0, 0.0]);
            h.track(&d);
        }
        h.track(&d);
        h.track(&d);
        let back = h.undo(&d).unwrap();
        assert_eq!(back, start);
        assert!(h.undo.is_empty());
        assert_eq!(h.redo(&back).unwrap(), d);
    }

    #[test]
    fn designs_round_trip_as_json() {
        let d = Design {
            appearance: Appearance::preset("Mei").unwrap(),
            pose: PoseEdit::base("wave"),
        };
        let text = serde_json::to_string(&d).unwrap();
        let back: Design = serde_json::from_str(&text).unwrap();
        assert_eq!(back, d);
    }
}
