//! Retargeting: drive a mesh rigged to one skeleton from an animator running another.
//!
//! Imported characters come with their own skeletons (bone names, rest poses and proportions
//! differ per tool and per character). Instead of re-authoring every clip, IK rig and muscle
//! for each, the character runs on a *logic skeleton* that the engine's clips, IK, gaits and
//! muscles already know (for people, [`humanoid::fit_skeleton`](crate::build::humanoid::fit_skeleton)
//! sizes the engine's humanoid to the imported one), and a [`Retarget`] component turns the
//! final logic pose into a pose of the imported skeleton every frame:
//!
//! - **Mapped bones** take the logic bone's position and its rotation, corrected so the target
//!   bone points where the logic bone points (rest poses may differ, e.g. an A-pose against
//!   arms at 30°). Because positions are copied, planted feet and gripping hands land exactly
//!   where IK put them, whatever the proportions.
//! - **In-between bones** (a spine bone the logic skeleton doesn't have) blend the rotations of
//!   their mapped neighbors.
//! - **Hands** close by animation curves (`grip`, `flex`, ...): every finger segment curls
//!   toward the palm.
//! - **Everything else** (face, eyes, twist helpers) follows its parent at its rest offset.
//!
//! The mesh's joints may also name logic bones directly (a ponytail re-weighted to the logic
//! skeleton's spring chain); those read the logic pose.

use std::sync::Arc;

use engine_core::prelude::*;
use serde::{Deserialize, Serialize};

use crate::animator::Animator;
use crate::clip::{AnimationClip, BoneTrack, ClipBinding};
use crate::skeleton::Skeleton;
use crate::xform::{Xf, rotation_between};

/// Which target bones follow which logic bones, and how hands close.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct RetargetMap {
    /// `(logic bone, target bone)`. A logic bone may drive several target bones.
    pub bones: Vec<(String, String)>,
    /// Target bones whose rotation blends two mapped target bones: `(bone, from, to, t)`.
    pub between: Vec<(String, String, String, f32)>,
    pub hands: Vec<HandRig>,
}

/// Fingers of one hand of the target skeleton.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct HandRig {
    pub hand: String,
    /// Finger chains, proximal to distal. The first chain is the thumb.
    pub fingers: Vec<Vec<String>>,
    /// Animation curves of the logic animator that close the hand (the largest wins).
    pub curves: Vec<String>,
    /// How far each segment of a finger curls in a fist, degrees.
    pub curl_deg: [f32; 3],
    /// The thumb's segments, degrees.
    pub thumb_curl_deg: [f32; 3],
    /// How closed the hand is at rest (0..1).
    pub relaxed: f32,
}

enum Drive {
    Mapped { source: u16, correction: Quat },
    Between { a: u16, b: u16, t: f32 },
    Follow,
}

struct FingerBone {
    bone: u16,
    hand: usize,
    /// Curl axis in the bone's own rest frame.
    axis: Vec3,
    angle: f32,
}

/// A prepared mapping from a logic skeleton to a target skeleton.
pub struct Retargeter {
    pub target: Arc<Skeleton>,
    /// Name of the logic skeleton this was built for.
    pub source_name: String,
    drive: Vec<Drive>,
    fingers: Vec<Option<FingerBone>>,
    hands: Vec<HandRig>,
}

impl std::fmt::Debug for Retargeter {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Retargeter")
            .field("source", &self.source_name)
            .field("target", &self.target.name)
            .finish()
    }
}

impl Retargeter {
    /// Prepare `map` between `source` (the logic skeleton) and `target` (the mesh's skeleton).
    /// Errors name bones that don't exist.
    pub fn new(
        source: &Skeleton,
        target: Arc<Skeleton>,
        map: &RetargetMap,
    ) -> Result<Self, String> {
        let t_index = |name: &str| {
            target
                .index_of(name)
                .ok_or_else(|| format!("bone {name} is not in skeleton {}", target.name))
        };
        let s_index = |name: &str| {
            source
                .index_of(name)
                .ok_or_else(|| format!("bone {name} is not in skeleton {}", source.name))
        };
        let n = target.len();
        let mut mapped: Vec<Option<usize>> = vec![None; n];
        for (s, t) in &map.bones {
            mapped[t_index(t)?] = Some(s_index(s)?);
        }
        // Each mapped pair's alignment: turn the target bone so it points along the logic bone
        // (toward the next mapped bone down the chain). Chain ends inherit their parent's.
        let is_descendant = |mut b: usize, ancestor: usize| {
            while let Some(p) = target.parents[b] {
                if p as usize == ancestor {
                    return true;
                }
                b = p as usize;
            }
            false
        };
        let mut align: Vec<Option<Quat>> = vec![None; n];
        for tb in 0..n {
            let Some(sb) = mapped[tb] else { continue };
            let mut q = None;
            'children: for &sc in source.children(sb) {
                for (tc, m) in mapped.iter().enumerate() {
                    if *m == Some(sc as usize) && is_descendant(tc, tb) {
                        let ds = source.rest_model[sc as usize].t - source.rest_model[sb].t;
                        let dt = target.rest_model[tc].t - target.rest_model[tb].t;
                        if ds.length() > 1e-5 && dt.length() > 1e-5 {
                            q = Some(rotation_between(dt.normalize(), ds.normalize()));
                        }
                        break 'children;
                    }
                }
            }
            align[tb] = q;
        }
        let mut drive = Vec::with_capacity(n);
        for tb in 0..n {
            drive.push(match mapped[tb] {
                Some(sb) => {
                    // Nearest mapped ancestor's alignment for chain ends.
                    let mut q = align[tb];
                    let mut a = tb;
                    while q.is_none() {
                        match target.parents[a] {
                            Some(p) => {
                                a = p as usize;
                                if mapped[a].is_some() {
                                    q = align[a];
                                }
                            }
                            None => break,
                        }
                    }
                    let q = q.unwrap_or(Quat::IDENTITY);
                    let aligned = q * target.rest_model[tb].r;
                    Drive::Mapped {
                        source: sb as u16,
                        correction: (source.rest_model[sb].r.conjugate() * aligned).normalize(),
                    }
                }
                None => Drive::Follow,
            });
        }
        for (bone, a, b, t) in &map.between {
            let i = t_index(bone)?;
            drive[i] = Drive::Between {
                a: t_index(a)? as u16,
                b: t_index(b)? as u16,
                t: *t,
            };
        }
        let mut fingers: Vec<Option<FingerBone>> = (0..n).map(|_| None).collect();
        for (h, hand) in map.hands.iter().enumerate() {
            let hb = t_index(&hand.hand)?;
            let wrist = target.rest_model[hb].t;
            // Palm normal: across the knuckles, crossed with the hand's direction.
            let bases: Vec<Vec3> = hand
                .fingers
                .iter()
                .skip(1)
                .filter_map(|c| c.first())
                .filter_map(|b| target.index_of(b))
                .map(|b| target.rest_model[b].t)
                .collect();
            let (Some(&first), Some(&last)) = (bases.first(), bases.last()) else {
                continue;
            };
            let center = bases.iter().copied().sum::<Vec3>() / bases.len() as f32;
            let along = (center - wrist).normalize_or(Vec3::NEG_Y);
            let across = (last - first).normalize_or(Vec3::Z);
            let mut palm = along.cross(across).normalize_or(Vec3::NEG_Y);
            // The thumb sits on the palm side.
            if let Some(thumb) = hand
                .fingers
                .first()
                .and_then(|c| c.last())
                .and_then(|b| target.index_of(b))
            {
                let to_thumb = target.rest_model[thumb].t - center;
                if to_thumb.dot(palm) < 0.0 {
                    palm = -palm;
                }
            }
            for (f, chain) in hand.fingers.iter().enumerate() {
                let curls = if f == 0 {
                    hand.thumb_curl_deg
                } else {
                    hand.curl_deg
                };
                for (k, name) in chain.iter().enumerate() {
                    let b = t_index(name)?;
                    let head = target.rest_model[b].t;
                    let dir = match chain.get(k + 1).and_then(|c| target.index_of(c)) {
                        Some(c) => target.rest_model[c].t - head,
                        None => match target.parents[b] {
                            Some(p) => head - target.rest_model[p as usize].t,
                            None => along,
                        },
                    }
                    .normalize_or(along);
                    // Rotating `dir` about `dir × palm` turns it toward the palm.
                    let axis_model = dir.cross(palm).normalize_or(Vec3::X);
                    let axis = target.rest_model[b].r.conjugate() * axis_model;
                    fingers[b] = Some(FingerBone {
                        bone: b as u16,
                        hand: h,
                        axis,
                        angle: curls.get(k).copied().unwrap_or(0.0).to_radians(),
                    });
                }
            }
        }
        Ok(Self {
            target,
            source_name: source.name.clone(),
            drive,
            fingers,
            hands: map.hands.clone(),
        })
    }

    /// How closed each hand is for this animator (its curves, at least the rest closure).
    pub fn hand_closure(&self, animator: &Animator) -> Vec<f32> {
        self.hands
            .iter()
            .map(|h| {
                h.curves
                    .iter()
                    .map(|c| animator.curve(c))
                    .fold(h.relaxed, f32::max)
                    .clamp(0.0, 1.0)
            })
            .collect()
    }

    /// The target skeleton's model-space pose for a logic pose (`source_model`, model space).
    pub fn apply(&self, source_model: &[Xf], closure: &[f32], out: &mut Vec<Xf>) {
        let target = &*self.target;
        let n = target.len();
        out.clear();
        out.extend_from_slice(&target.rest_model);
        let mut done = vec![false; n];
        for (b, d) in self.drive.iter().enumerate() {
            if let Drive::Mapped { source, correction } = d
                && let Some(s) = source_model.get(*source as usize)
            {
                out[b] = Xf::from_tr(s.t, (s.r * *correction).normalize());
                done[b] = true;
            }
        }
        for b in 0..n {
            if done[b] {
                continue;
            }
            let parent = target.parents[b].map(|p| out[p as usize]);
            let mut local = target.rest[b];
            if let Some(f) = &self.fingers[b] {
                let c = closure.get(f.hand).copied().unwrap_or(0.0);
                if c > 0.0 {
                    local.r = (local.r * Quat::from_axis_angle(f.axis, f.angle * c)).normalize();
                }
                debug_assert_eq!(f.bone as usize, b);
            }
            let mut x = match parent {
                Some(p) => p.mul(&local),
                None => local,
            };
            if let Drive::Between { a, b: bb, t } = self.drive[b] {
                // Blend how far each neighbor turned from its rest orientation.
                let (a, bb) = (a as usize, bb as usize);
                let da = out[a].r * target.rest_model[a].r.conjugate();
                let db = out[bb].r * target.rest_model[bb].r.conjugate();
                let db = if da.dot(db) < 0.0 { -db } else { db };
                x.r = (da.slerp(db, t) * target.rest_model[b].r).normalize();
            }
            x.s = Vec3::ONE;
            out[b] = x;
            done[b] = true;
        }
    }
}

impl Retargeter {
    /// The logic pose (model space) that [`apply`](Self::apply) turns into `target_model`:
    /// for converting clips authored on the target skeleton (motion capture) into clips for
    /// the logic skeleton `source`. Logic bones no target bone follows keep their rest offsets.
    pub fn invert(&self, source: &Skeleton, target_model: &[Xf], out: &mut Vec<Xf>) {
        let n = source.len();
        // When a logic bone drives several target bones, the last one (deepest) decides.
        let mut from: Vec<Option<(usize, Quat)>> = vec![None; n];
        for (b, d) in self.drive.iter().enumerate() {
            if let Drive::Mapped {
                source: s,
                correction,
            } = d
                && let Some(slot) = from.get_mut(*s as usize)
            {
                *slot = Some((b, *correction));
            }
        }
        out.clear();
        for (s, f) in from.iter().enumerate() {
            let x = match f.and_then(|(b, c)| target_model.get(b).map(|t| (t, c))) {
                Some((t, c)) => Xf::from_tr(t.t, (t.r * c.conjugate()).normalize()),
                None => match source.parents[s] {
                    Some(p) => out[p as usize].mul(&source.rest[s]),
                    None => source.rest[s],
                },
            };
            out.push(x);
        }
    }

    /// Convert `clip`, authored on the target skeleton, into a clip named `name` for the logic
    /// skeleton `source`: a rotation track for every logic bone a target bone follows, and
    /// translation tracks for the `moving` bones (usually the hips). Keys stay where the
    /// clip's first track has them.
    pub fn clip_to_source(
        &self,
        source: &Skeleton,
        clip: &AnimationClip,
        name: &str,
        moving: &[&str],
    ) -> AnimationClip {
        let target = &*self.target;
        let binding = ClipBinding::new(clip, target);
        let times: Vec<f32> = clip
            .tracks
            .iter()
            .map(|t| &t.rotation.times)
            .find(|t| !t.is_empty())
            .cloned()
            .unwrap_or_else(|| vec![0.0, clip.duration]);
        let followed: Vec<bool> = {
            let mut f = vec![false; source.len()];
            for d in &self.drive {
                if let Drive::Mapped { source: s, .. } = d {
                    f[*s as usize] = true;
                }
            }
            f
        };
        let mut out = AnimationClip {
            name: name.into(),
            duration: clip.duration,
            looping: clip.looping,
            tracks: (0..source.len())
                .filter(|&b| followed[b])
                .map(|b| BoneTrack {
                    bone: source.names[b].clone(),
                    ..Default::default()
                })
                .collect(),
            ..Default::default()
        };
        let track_of: Vec<Option<usize>> = {
            let mut k = 0;
            (0..source.len())
                .map(|b| {
                    followed[b].then(|| {
                        k += 1;
                        k - 1
                    })
                })
                .collect()
        };
        let mut local = Vec::new();
        let mut target_model = Vec::new();
        let mut model = Vec::new();
        for &t in &times {
            local.clear();
            local.extend_from_slice(&target.rest);
            clip.sample(t, &binding, &mut local);
            target.to_model(&local, &mut target_model);
            self.invert(source, &target_model, &mut model);
            for b in 0..source.len() {
                let Some(k) = track_of[b] else { continue };
                let l = match source.parents[b] {
                    Some(p) => model[p as usize].inverse().mul(&model[b]),
                    None => model[b],
                };
                let track = &mut out.tracks[k];
                let r = match track.rotation.values.last() {
                    Some(prev) if prev.dot(l.r) < 0.0 => -l.r,
                    _ => l.r,
                };
                track.rotation.times.push(t);
                track.rotation.values.push(r);
                if moving.contains(&source.names[b].as_str()) {
                    track.translation.times.push(t);
                    track.translation.values.push(l.t);
                }
            }
        }
        out
    }
}

/// Drives this entity's [`SkinnedMeshRenderer`](crate::SkinnedMeshRenderer) mesh, which is
/// rigged to `retargeter.target`, from its [`Animator`] (the logic skeleton).
#[derive(Component, Debug, Clone)]
pub struct Retarget {
    pub retargeter: Arc<Retargeter>,
    /// The target skeleton's pose this frame (model space).
    pub model: Vec<Xf>,
}

impl Retarget {
    pub fn new(retargeter: Arc<Retargeter>) -> Self {
        Self {
            model: retargeter.target.rest_model.clone(),
            retargeter,
        }
    }
}

pub(crate) fn retarget_poses(mut q: Query<(&Animator, &mut Retarget)>) {
    q.par_iter_mut().for_each(|(animator, mut retarget)| {
        let r = &mut *retarget;
        let closure = r.retargeter.hand_closure(animator);
        r.retargeter.apply(&animator.model, &closure, &mut r.model);
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::skeleton::BoneDef;

    fn arm(name: &str, angle_deg: f32, scale: f32) -> Skeleton {
        // Root, shoulder, elbow, wrist; the arm hangs `angle_deg` out from straight down.
        let r = Quat::from_rotation_z(angle_deg.to_radians());
        Skeleton::new(
            name,
            vec![
                BoneDef::new("root", None, Vec3::ZERO),
                BoneDef::new("shoulder", Some("root"), Vec3::new(0.2, 1.4, 0.0) * scale),
                BoneDef::new(
                    "elbow",
                    Some("shoulder"),
                    r * Vec3::new(0.0, -0.3, 0.0) * scale,
                ),
                BoneDef::new(
                    "wrist",
                    Some("elbow"),
                    r * Vec3::new(0.0, -0.25, 0.0) * scale,
                ),
            ],
        )
        .unwrap()
    }

    #[test]
    fn mapped_bones_follow_positions_and_directions() {
        let logic = arm("logic", 30.0, 1.0);
        let target = Arc::new(arm("target", 45.0, 1.0));
        let map = RetargetMap {
            bones: ["root", "shoulder", "elbow", "wrist"]
                .iter()
                .map(|b| (b.to_string(), b.to_string()))
                .collect(),
            ..Default::default()
        };
        let r = Retargeter::new(&logic, target.clone(), &map).unwrap();
        // Raise the logic arm forward 90° at the shoulder.
        let mut local = logic.rest.clone();
        local[1].r = Quat::from_rotation_x(-90f32.to_radians());
        let mut model = Vec::new();
        logic.to_model(&local, &mut model);
        let mut out = Vec::new();
        r.apply(&model, &[], &mut out);
        for b in 0..4 {
            assert!((out[b].t - model[b].t).length() < 1e-5, "bone {b} position");
        }
        // The target's upper arm points where the logic upper arm points.
        let logic_dir = (model[2].t - model[1].t).normalize();
        let target_rest_dir = (target.rest_model[2].t - target.rest_model[1].t).normalize();
        let local_dir = target.rest_model[1].r.conjugate() * target_rest_dir;
        let target_dir = out[1].r * local_dir;
        assert!(
            target_dir.dot(logic_dir) > 0.9999,
            "{target_dir} vs {logic_dir}"
        );
    }

    #[test]
    fn hands_curl_toward_the_palm() {
        let target = Arc::new(
            Skeleton::new(
                "hand",
                vec![
                    BoneDef::new("hand", None, Vec3::ZERO),
                    BoneDef::new("thumb", Some("hand"), Vec3::new(0.03, -0.02, -0.02)),
                    BoneDef::new("thumb_tip", Some("thumb"), Vec3::new(0.0, -0.03, -0.01)),
                    BoneDef::new("index", Some("hand"), Vec3::new(0.02, -0.08, 0.0)),
                    BoneDef::new("index_tip", Some("index"), Vec3::new(0.0, -0.04, 0.0)),
                    BoneDef::new("pinky", Some("hand"), Vec3::new(-0.03, -0.07, 0.0)),
                    BoneDef::new("pinky_tip", Some("pinky"), Vec3::new(0.0, -0.03, 0.0)),
                ],
            )
            .unwrap(),
        );
        let logic = Skeleton::new("l", vec![BoneDef::new("hand", None, Vec3::ZERO)]).unwrap();
        let map = RetargetMap {
            bones: vec![("hand".into(), "hand".into())],
            hands: vec![HandRig {
                hand: "hand".into(),
                fingers: vec![
                    vec!["thumb".into(), "thumb_tip".into()],
                    vec!["index".into(), "index_tip".into()],
                    vec!["pinky".into(), "pinky_tip".into()],
                ],
                curves: vec!["grip".into()],
                curl_deg: [80.0, 90.0, 60.0],
                thumb_curl_deg: [20.0, 30.0, 30.0],
                relaxed: 0.0,
            }],
            ..Default::default()
        };
        let r = Retargeter::new(&logic, target.clone(), &map).unwrap();
        let mut open = Vec::new();
        r.apply(&logic.rest_model, &[0.0], &mut open);
        let mut fist = Vec::new();
        r.apply(&logic.rest_model, &[1.0], &mut fist);
        let tip = target.index_of("index_tip").unwrap();
        // The thumb is on the −Z side here, so the palm faces −Z: the fist pulls the
        // fingertip up toward the wrist and over to the palm side.
        let tip_tip = |pose: &[Xf]| pose[tip].transform_point(Vec3::new(0.0, -0.02, 0.0));
        assert!(fist[tip].t.y > open[tip].t.y + 0.02, "{:?}", fist[tip].t);
        assert!(tip_tip(&fist).z < -0.02, "{:?}", tip_tip(&fist));
        for (b, x) in open.iter().enumerate() {
            assert!(x.t.distance(target.rest_model[b].t) < 1e-5);
        }
    }
}
