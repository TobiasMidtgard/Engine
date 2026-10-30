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
use crate::grip::{PALM, STANDARD_RADIUS};
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
    /// Which side each hand is on (from its logic bone's `_l`/`_r` name), for [`HandGrip`].
    right_hand: Vec<Option<bool>>,
    /// Each hand's measurements, in `hands` order.
    fits: Vec<HandFit>,
}

/// A hand's palm in the target's rest pose: the hand bone, its side (0 left, 1 right), the
/// knuckles' centre, the direction along the hand and the palm's normal (model space).
type RestPalm = (usize, Option<usize>, Vec3, Vec3, Vec3);

/// One visible hand, measured for holding things ([`crate::grip`]).
#[derive(Debug, Clone)]
struct HandFit {
    /// The target hand bone.
    bone: u16,
    /// 0 left, 1 right, when the logic bone's name says.
    side: Option<usize>,
    /// The mapped hand's correction (logic hand frame → target hand frame).
    correction: Quat,
    /// The measured fist in the logic hand bone's frame.
    fist: Xf,
    /// The fingers' bones, proximal to distal, thumb first.
    chains: Vec<Vec<u16>>,
    /// Where each chain's last segment ends, in its last bone's frame.
    tips: Vec<Vec3>,
    /// Half a finger's thickness, meters.
    finger_radius: f32,
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
        // Each hand's palm in the target's rest pose.
        let side_of = |hand: &HandRig| {
            let logic = &map.bones.iter().find(|(_, t)| *t == hand.hand)?.0;
            crate::grip::side_of(logic)
        };
        let mut palms: Vec<Option<RestPalm>> = Vec::new();
        for hand in &map.hands {
            let hb = t_index(&hand.hand)?;
            palms.push(
                palm_frame(&target, &target.rest_model, hb, hand)
                    .map(|(c, a, p)| (hb, side_of(hand), c, a, p)),
            );
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
                    let mut q = q.unwrap_or(Quat::IDENTITY);
                    // A hand turns so its palm faces the standard fist's way, knuckles and all
                    // (the props and two-handed grips are placed in that frame).
                    if let Some((_, Some(side), _, along, palm)) =
                        palms.iter().flatten().find(|p| p.0 == tb)
                    {
                        let logic_r = source.rest_model[sb].r;
                        q = frame_turn(
                            [*along, *palm],
                            [logic_r * Vec3::NEG_Y, logic_r * crate::grip::PALM[*side]],
                        );
                    }
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
            let Some((_, _, _, along, palm)) = palms[h] else {
                continue;
            };
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
        let right_hand = map
            .hands
            .iter()
            .map(|h| {
                let logic = &map.bones.iter().find(|(_, t)| *t == h.hand)?.0;
                if logic.ends_with("_r") {
                    Some(true)
                } else if logic.ends_with("_l") {
                    Some(false)
                } else {
                    None
                }
            })
            .collect();
        let mut fits = Vec::new();
        for (hand, palm) in map.hands.iter().zip(&palms) {
            let Some(&(hb, side, centre, along, palm)) = palm.as_ref() else {
                fits.push(HandFit {
                    bone: t_index(&hand.hand)? as u16,
                    side: None,
                    correction: Quat::IDENTITY,
                    fist: Xf::from_t(crate::grip::GRIP),
                    chains: Vec::new(),
                    tips: Vec::new(),
                    finger_radius: 0.0,
                });
                continue;
            };
            let correction = match drive[hb] {
                Drive::Mapped { correction, .. } => correction,
                _ => Quat::IDENTITY,
            };
            let rest = target.rest_model[hb];
            // Wrist to knuckles is about 9.5 cm on an adult hand.
            let size = centre.distance(rest.t) / 0.095;
            let fist_model =
                centre - along * FIST_BACK * size + palm * (FIST_DEPTH * size + STANDARD_RADIUS);
            let fist_vis = rest.inverse().transform_point(fist_model);
            let chains: Vec<Vec<u16>> = hand
                .fingers
                .iter()
                .map(|c| {
                    c.iter()
                        .filter_map(|b| target.index_of(b))
                        .map(|b| b as u16)
                        .collect()
                })
                .collect();
            let tips = chains
                .iter()
                .map(|c| match c.last() {
                    // The last segment about as long as the one before it, a little shorter.
                    Some(&last) => {
                        let l = target.rest[last as usize];
                        l.r.conjugate() * l.t * 0.85
                    }
                    None => Vec3::ZERO,
                })
                .collect();
            fits.push(HandFit {
                bone: hb as u16,
                side,
                correction,
                fist: Xf::from_tr(correction * fist_vis, Quat::IDENTITY),
                chains,
                tips,
                finger_radius: FINGER_RADIUS * size,
            });
        }
        Ok(Self {
            target,
            source_name: source.name.clone(),
            drive,
            fingers,
            hands: map.hands.clone(),
            right_hand,
            fits,
        })
    }

    /// How closed each hand is for this animator (its curves, at least the rest closure and
    /// what `grip` holds).
    pub fn hand_closure(&self, animator: &Animator, grip: Option<&HandGrip>) -> Vec<f32> {
        self.hands
            .iter()
            .zip(&self.right_hand)
            .map(|(h, right)| {
                let held = match (grip, right) {
                    (Some(g), Some(true)) => g.right,
                    (Some(g), Some(false)) => g.left,
                    _ => 0.0,
                };
                h.curves
                    .iter()
                    .map(|c| animator.curve(c))
                    .fold(h.relaxed.max(held), f32::max)
                    .clamp(0.0, 1.0)
            })
            .collect()
    }

    /// The target skeleton's model-space pose for a logic pose (`source_model`, model space).
    pub fn apply(&self, source_model: &[Xf], closure: &[f32], out: &mut Vec<Xf>) {
        self.apply_holding(source_model, closure, &[], out);
    }

    /// [`apply`](Self::apply), with the hands that hold something closing round it instead
    /// of into a fist: `wraps[hand]` is that hand's [`wrap`](Self::wrap), in `hands` order.
    pub fn apply_holding(
        &self,
        source_model: &[Xf],
        closure: &[f32],
        wraps: &[Option<&[f32]>],
        out: &mut Vec<Xf>,
    ) {
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
                let angle = match wraps.get(f.hand).copied().flatten() {
                    Some(w) => w.get(b).copied().unwrap_or(f.angle),
                    None => f.angle,
                };
                if c > 0.0 {
                    local.r = (local.r * Quat::from_axis_angle(f.axis, angle * c)).normalize();
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

/// How far a fist's handle axis sits behind the knuckles toward the wrist, and below the
/// knuckle joints' centres toward the palm (to the palm's skin, the handle's radius further),
/// on an adult hand (scaled by hand size), meters.
const FIST_BACK: f32 = 0.008;
const FIST_DEPTH: f32 = 0.012;
/// Half a finger's thickness on an adult hand, meters.
const FINGER_RADIUS: f32 = 0.0085;
/// How far past its fist curl a finger may close round something, as a fraction.
const WRAP_LIMIT: f32 = 1.3;
/// The angle step a wrapping finger closes by, radians.
const WRAP_STEP: f32 = 0.02;

/// A hand's palm in the target's rest pose (model space): the knuckles' centre, the direction
/// from the wrist to them, and the palm's normal (on the thumb's side), from the fingers'
/// first bones. `None` without at least two fingers past the thumb.
fn palm_frame(
    target: &Skeleton,
    model: &[Xf],
    hand_bone: usize,
    hand: &HandRig,
) -> Option<(Vec3, Vec3, Vec3)> {
    let wrist = model[hand_bone].t;
    let bases: Vec<Vec3> = hand
        .fingers
        .iter()
        .skip(1)
        .filter_map(|c| c.first())
        .filter_map(|b| target.index_of(b))
        .map(|b| model[b].t)
        .collect();
    let (&first, &last) = (bases.first()?, bases.last()?);
    if bases.len() < 2 {
        return None;
    }
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
        let to_thumb = model[thumb].t - center;
        if to_thumb.dot(palm) < 0.0 {
            palm = -palm;
        }
    }
    Some((center, along, palm))
}

/// The rotation taking the frame of `from` (a direction and a second one across it) to that
/// of `to`.
fn frame_turn(from: [Vec3; 2], to: [Vec3; 2]) -> Quat {
    let basis = |[a, b]: [Vec3; 2]| {
        let x = a.normalize_or(Vec3::Y);
        let y = (b - x * b.dot(x)).normalize_or(x.any_orthonormal_vector());
        Mat3::from_cols(x, y, x.cross(y))
    };
    Quat::from_mat3(&(basis(to) * basis(from).transpose())).normalize()
}

/// The shortest distance between segments `p0`–`p1` and `q0`–`q1`.
fn segment_distance(p0: Vec3, p1: Vec3, q0: Vec3, q1: Vec3) -> f32 {
    let (d1, d2, r) = (p1 - p0, q1 - q0, p0 - q0);
    let (a, e, f) = (d1.dot(d1), d2.dot(d2), d2.dot(r));
    let (s, t) = if a < 1e-12 && e < 1e-12 {
        (0.0, 0.0)
    } else if a < 1e-12 {
        (0.0, (f / e).clamp(0.0, 1.0))
    } else {
        let c = d1.dot(r);
        if e < 1e-12 {
            ((-c / a).clamp(0.0, 1.0), 0.0)
        } else {
            let b = d1.dot(d2);
            let denom = a * e - b * b;
            let mut s = if denom > 1e-12 {
                ((b * f - c * e) / denom).clamp(0.0, 1.0)
            } else {
                0.0
            };
            let mut t = (b * s + f) / e;
            if t < 0.0 {
                t = 0.0;
                s = (-c / a).clamp(0.0, 1.0);
            } else if t > 1.0 {
                t = 1.0;
                s = ((b - c) / a).clamp(0.0, 1.0);
            }
            (s, t)
        }
    };
    (p0 + d1 * s).distance(q0 + d2 * t)
}

impl Retargeter {
    /// The measured fist of hand `side` (0 left, 1 right) in the logic hand bone's frame
    /// ([`crate::grip`]), if this model has that hand.
    pub fn fist(&self, side: usize) -> Option<Xf> {
        self.fits
            .iter()
            .find(|f| f.side == Some(side))
            .map(|f| f.fist)
    }

    /// How the visible hand `side` of `target_model` (this retargeter's pose of `logic_model`)
    /// sits on `handle`, held in that hand's fitted fist.
    pub fn grip_report(
        &self,
        logic_model: &[Xf],
        logic: &Skeleton,
        target_model: &[Xf],
        side: usize,
        handle: &crate::grip::Handle,
    ) -> Option<crate::grip::GripReport> {
        let fit = &self.fits[self.hand_of(side)?];
        let hand = logic_model[logic.index_of(["hand_l", "hand_r"][side])?];
        let fist = hand.mul(&crate::grip::fist_correction(Some(self), side));
        let [a, b] = handle.ends(side).map(|p| fist.transform_point(p));
        let axis = (b - a).normalize_or(Vec3::Z);
        let off_axis = |p: Vec3| {
            let d = p - a;
            d - axis * d.dot(axis)
        };
        let point = |bone: u16| target_model[bone as usize].t;
        let wrist = point(fit.bone);
        let bases: Vec<Vec3> = fit.chains[1..]
            .iter()
            .filter_map(|c| c.first().map(|&b| point(b)))
            .collect();
        let knuckles = bases.iter().copied().sum::<Vec3>() / bases.len().max(1) as f32;
        let size = knuckles.distance(wrist) / 0.095;
        // The palm faces where the standard fist's does, carried by the posed hand.
        let palm_dir = fist.r * PALM[side];
        let skin = knuckles + palm_dir * FIST_DEPTH * size;
        let palm = off_axis(skin).length() - handle.radius;
        let mut deepest = 0.0f32;
        let mut widest = 0.0f32;
        let segments = |c: usize| -> Vec<(Vec3, Vec3)> {
            let chain = &fit.chains[c];
            (0..chain.len())
                .map(|k| {
                    let x = target_model[chain[k] as usize];
                    let end = match chain.get(k + 1) {
                        Some(&n) => point(n),
                        None => x.transform_point(fit.tips[c]),
                    };
                    (x.t, end)
                })
                .collect()
        };
        let reach = handle.radius + fit.finger_radius;
        for c in 1..fit.chains.len() {
            let mut closest = f32::MAX;
            for (p0, p1) in segments(c) {
                let d = segment_distance(p0, p1, a - axis, b + axis) - reach;
                deepest = deepest.min(d);
                closest = closest.min(d);
            }
            widest = widest.max(closest);
        }
        let thumb_tip = segments(0).last().map_or(wrist, |s| s.1);
        let thumb_opposed = off_axis(thumb_tip).dot(off_axis(skin)) < 0.0;
        Some(crate::grip::GripReport {
            palm,
            deepest: -deepest.min(0.0),
            widest,
            thumb_opposed,
            size: handle.radius * 2.0 / (0.19 * size),
        })
    }

    /// Which of this retargeter's hands (in [`hand_closure`](Self::hand_closure)'s order) is
    /// `side` (0 left, 1 right).
    pub fn hand_of(&self, side: usize) -> Option<usize> {
        self.fits.iter().position(|f| f.side == Some(side))
    }

    /// How far each finger bone of hand `side` curls (radians, indexed by target bone) to
    /// close round `handle` (in that hand's standard fist frame, [`crate::grip::Handle`]):
    /// from the knuckle out, each segment curls until it touches the handle or reaches
    /// [`WRAP_LIMIT`] times its fist curl, then the next one curls from there. `None` if
    /// this model has no such hand.
    pub fn wrap(&self, side: usize, handle: &crate::grip::Handle) -> Option<Vec<f32>> {
        let h = self.hand_of(side)?;
        let fit = &self.fits[h];
        let target = &*self.target;
        // The handle in the visible hand bone's rest frame.
        let correction = crate::grip::fist_correction(Some(self), side);
        let [a, b] = handle
            .ends(side)
            .map(|p| fit.correction.conjugate() * correction.transform_point(p));
        let reach = handle.radius + fit.finger_radius;
        let hand_rest = target.rest_model[fit.bone as usize].inverse();
        let mut angles = vec![0.0; target.len()];
        for (chain, tip) in fit.chains.iter().zip(&fit.tips) {
            let Some(&first) = chain.first() else {
                continue;
            };
            let mut parent = match target.parents[first as usize] {
                Some(p) => hand_rest.mul(&target.rest_model[p as usize]),
                None => Xf::IDENTITY,
            };
            for (k, &bone) in chain.iter().enumerate() {
                let Some(f) = &self.fingers[bone as usize] else {
                    continue;
                };
                let rest = target.rest[bone as usize];
                // Where the segment ends, in the bone's own frame.
                let end = match chain.get(k + 1) {
                    Some(&next) => target.rest[next as usize].t,
                    None => *tip,
                };
                let posed = |angle: f32| {
                    let mut local = rest;
                    local.r = (rest.r * Quat::from_axis_angle(f.axis, angle)).normalize();
                    parent.mul(&local)
                };
                let touches = |x: &Xf| segment_distance(x.t, x.transform_point(end), a, b) <= reach;
                let limit = f.angle.abs() * WRAP_LIMIT;
                let sign = f.angle.signum();
                let mut angle = 0.0;
                if !touches(&posed(0.0)) {
                    while angle < limit {
                        let next = (angle + WRAP_STEP).min(limit);
                        if touches(&posed(next * sign)) {
                            break;
                        }
                        angle = next;
                    }
                }
                angles[bone as usize] = angle * sign;
                parent = posed(angle * sign);
            }
        }
        Some(angles)
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
    /// What each hand closes round (in `retargeter`'s hand order) and the finger curls for it.
    wraps: Vec<Option<(crate::grip::Handle, Vec<f32>)>>,
}

impl Retarget {
    pub fn new(retargeter: Arc<Retargeter>) -> Self {
        Self {
            model: retargeter.target.rest_model.clone(),
            wraps: vec![None; retargeter.hands.len()],
            retargeter,
        }
    }

    /// Re-solve the finger wraps for what the hands hold now.
    fn hold(&mut self, held: Option<&crate::grip::HeldItems>) {
        for (h, fit) in self.retargeter.fits.iter().enumerate() {
            let handle = fit.side.and_then(|side| held.and_then(|i| i.hands[side]));
            let current = self.wraps[h].as_ref().map(|(handle, _)| *handle);
            if handle == current {
                continue;
            }
            self.wraps[h] = handle.and_then(|handle| {
                let side = fit.side?;
                Some((handle, self.retargeter.wrap(side, &handle)?))
            });
        }
    }
}

/// Keeps a character's hands closed at least this much (0..1) whatever its clips do, as when
/// it carries something (a hammer in the right hand). Closes an imported character's fingers.
#[derive(Component, Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct HandGrip {
    pub left: f32,
    pub right: f32,
}

pub(crate) fn retarget_poses(
    mut q: Query<(
        &Animator,
        &mut Retarget,
        Option<&HandGrip>,
        Option<&crate::grip::HeldItems>,
    )>,
) {
    q.par_iter_mut()
        .for_each(|(animator, mut retarget, grip, held)| {
            if !animator.posed() {
                return;
            }
            let r = &mut *retarget;
            r.hold(held);
            let mut closure = r.retargeter.hand_closure(animator, grip);
            // A hand holding something is closed on it.
            for (c, wrap) in closure.iter_mut().zip(&r.wraps) {
                if wrap.is_some() {
                    *c = 1.0;
                }
            }
            let wraps: Vec<Option<&[f32]>> = r
                .wraps
                .iter()
                .map(|w| w.as_ref().map(|(_, a)| a.as_slice()))
                .collect();
            r.retargeter
                .apply_holding(&animator.model, &closure, &wraps, &mut r.model);
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

    #[test]
    fn a_hand_grip_closes_only_its_side() {
        let logic =
            Arc::new(Skeleton::new("l", vec![BoneDef::new("hand_r", None, Vec3::ZERO)]).unwrap());
        let target =
            Arc::new(Skeleton::new("t", vec![BoneDef::new("R Hand", None, Vec3::ZERO)]).unwrap());
        let map = RetargetMap {
            bones: vec![("hand_r".into(), "R Hand".into())],
            hands: vec![HandRig {
                hand: "R Hand".into(),
                curves: vec!["grip".into()],
                relaxed: 0.1,
                ..Default::default()
            }],
            ..Default::default()
        };
        let r = Retargeter::new(&logic, target, &map).unwrap();
        let animator = Animator::procedural(logic);
        assert_eq!(r.hand_closure(&animator, None), vec![0.1]);
        let carry = HandGrip {
            left: 0.0,
            right: 0.9,
        };
        assert_eq!(r.hand_closure(&animator, Some(&carry)), vec![0.9]);
        let other = HandGrip {
            left: 1.0,
            right: 0.0,
        };
        assert_eq!(r.hand_closure(&animator, Some(&other)), vec![0.1]);
    }

    /// A right hand pointing down (−Y), its palm toward −X and the thumb on the −Z side,
    /// rolled `roll` degrees about the forearm, with three-segment fingers and a two-segment
    /// thumb. The logic skeleton is just its hand.
    fn right_hand(roll: f32) -> (Skeleton, Arc<Skeleton>, RetargetMap) {
        let r = Quat::from_rotation_y(roll.to_radians());
        let mut bones = vec![BoneDef::new("R Hand", None, Vec3::new(0.3, 1.0, 0.0))];
        let mut fingers = Vec::new();
        let thumb = [
            Vec3::new(-0.02, -0.03, -0.025),
            Vec3::new(-0.012, -0.03, -0.012),
            Vec3::new(-0.008, -0.025, -0.004),
        ];
        let segments = [
            Vec3::new(0.0, -0.045, 0.0),
            Vec3::new(0.0, -0.028, 0.0),
            Vec3::new(0.0, -0.02, 0.0),
        ];
        for (f, z) in [-0.03f32, -0.01, 0.01, 0.028].iter().enumerate() {
            let base = Vec3::new(0.0, -0.095, *z);
            let mut chain = Vec::new();
            let mut parent = "R Hand".to_string();
            for (k, seg) in [base, segments[0], segments[1]].iter().enumerate() {
                let name = format!("F{}{k}", f + 1);
                bones.push(BoneDef::new(&name, Some(&parent), r * *seg));
                chain.push(name.clone());
                parent = name;
            }
            fingers.push(chain);
        }
        let mut chain = Vec::new();
        let mut parent = "R Hand".to_string();
        for (k, seg) in thumb.iter().enumerate() {
            let name = format!("F0{k}");
            bones.push(BoneDef::new(&name, Some(&parent), r * *seg));
            chain.push(name.clone());
            parent = name;
        }
        fingers.insert(0, chain);
        let target = Arc::new(Skeleton::new("hand", bones).unwrap());
        let logic = Skeleton::new(
            "l",
            vec![BoneDef::new("hand_r", None, Vec3::new(0.3, 1.0, 0.0))],
        )
        .unwrap();
        let map = RetargetMap {
            bones: vec![("hand_r".into(), "R Hand".into())],
            hands: vec![HandRig {
                hand: "R Hand".into(),
                fingers,
                curves: vec!["grip".into()],
                curl_deg: [72.0, 95.0, 55.0],
                thumb_curl_deg: [10.0, 35.0, 40.0],
                relaxed: 0.0,
            }],
            ..Default::default()
        };
        (logic, target, map)
    }

    #[test]
    fn a_hand_turns_so_its_palm_faces_the_standard_way() {
        for roll in [0.0, 35.0, -60.0] {
            let (logic, target, map) = right_hand(roll);
            let r = Retargeter::new(&logic, target.clone(), &map).unwrap();
            let mut out = Vec::new();
            r.apply(&logic.rest_model, &[0.0], &mut out);
            let (_, along, palm) = palm_frame(&target, &out, 0, &map.hands[0]).unwrap();
            assert!(along.dot(Vec3::NEG_Y) > 0.999, "roll {roll}: along {along}");
            assert!(
                palm.dot(crate::grip::PALM[1]) > 0.999,
                "roll {roll}: palm {palm}"
            );
            // The fist sits in front of the knuckles' line, on the palm side.
            let fist = r.fist(1).unwrap();
            assert!(fist.t.x < -0.02 && fist.t.y < -0.08, "{}", fist.t);
            assert!(r.fist(0).is_none());
        }
    }

    #[test]
    fn fingers_wrap_a_handle_and_close_less_round_a_thick_one() {
        let (logic, target, map) = right_hand(20.0);
        let r = Retargeter::new(&logic, target, &map).unwrap();
        let sword = crate::grip::Handle::new(Vec3::NEG_Z, 0.017, [-0.07, 0.07]);
        let bow = crate::grip::Handle::new(Vec3::NEG_Z, 0.025, [-0.07, 0.07]);
        let thin = r.wrap(1, &sword).unwrap();
        let thick = r.wrap(1, &bow).unwrap();
        let total = |w: &[f32]| w.iter().map(|a| a.abs()).sum::<f32>();
        assert!(
            total(&thick) < total(&thin) - 0.3,
            "{} vs {}",
            total(&thick),
            total(&thin)
        );
        assert!(r.wrap(0, &sword).is_none());
        for (handle, wrap) in [(sword, &thin), (bow, &thick)] {
            let mut out = Vec::new();
            r.apply_holding(&logic.rest_model, &[1.0], &[Some(wrap)], &mut out);
            let report = r
                .grip_report(&logic.rest_model, &logic, &out, 1, &handle)
                .unwrap();
            assert!(report.deepest < 0.002, "{report:?}");
            assert!(report.widest < 0.006, "{report:?}");
            assert!(report.palm.abs() < 0.003, "{report:?}");
        }
    }
}
