//! Animal actions: a horse grazing, drinking, rearing, shaking its head and pawing; a dog
//! sitting, lying down, bowing to play, shaking off and sniffing. Each is a clip played over
//! the animal's graph ([`Animator::play_over`]) that frees the legs it moves from the gait with
//! its `release` curves ([`crate::build::quadruped`]).
//!
//! Three ideas are borrowed from MotionBricks (see
//! `engine-design/anim-animals-wallrun.md`), with no part of it used:
//! - **Target-pose entries.** An action may have several entry clips that end in the same pose
//!   (a dog lies down from standing or from a sit). The one whose first pose is nearest the pose
//!   shown now is played, and its fade-in is set from how far that is ([`fade_for`]).
//! - **Smart spots.** An [`ActionSpot`] (a trough, a patch of grass) names the actions it is
//!   for and the point they are about. An animal that stops within its radius plays the first
//!   of them it has, and while the clip's `reach` curve is up its neck reaches the head for the
//!   point ([`reach_heads`]), so the muzzle meets the water wherever the animal stopped.
//! - **Actions asked for as data.** A game asks with [`AnimalActions::request`] (or a parameter
//!   `act.<action>` rising past 0.5); requests queue, and a queued action cuts into the one
//!   playing at its chain point (a dog that sat lies down from the sit), else waits for it.
//!
//! An idle picker plays one of the animal's idle actions (deterministic by seed) when it has
//! stood still a while.

use std::collections::HashMap;
use std::sync::Arc;

use engine_core::prelude::*;
use engine_world::animation::AnimationParams;

use crate::build::quadruped::sized_name;
use crate::build::{dog, horse};
use crate::xform::rotation_between;
use crate::{AnimationLibrary, Animator};

/// Below this ground speed an animal is standing, m/s (scaled by its size).
const STILL_SPEED: f32 = 0.15;
/// Above it an action under way is cut short, m/s (scaled).
const MOVING_SPEED: f32 = 0.6;
/// How long an animal must have stood before a spot's action starts, s.
const SPOT_SETTLE: f32 = 0.4;
/// Fade-in bounds for an action's entry, s (MotionBricks' transitions run 0.6-1.9 s for whole
/// moves; an overlay onto a standing animal needs far less).
const FADE_MIN: f32 = 0.15;
const FADE_MAX: f32 = 0.6;
/// Seconds of fade per body height of pose distance.
const FADE_PER_HEIGHT: f32 = 3.0;
const FADE_OUT: f32 = 0.4;
/// The most each neck bone turns toward a spot's point, degrees.
const REACH_PER_BONE: f32 = 25.0;

/// Which animal.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum AnimalKind {
    #[default]
    Horse,
    Dog,
}

/// One action an animal has.
struct ActionDef {
    name: &'static str,
    /// Clips that perform it (base names, before any size suffix), first the usual one.
    entries: &'static [&'static str],
    /// Seconds into it from which a queued action may cut in (its hold), if it may.
    chain_from: Option<f32>,
    /// Played by the idle picker.
    idle: bool,
}

const HORSE_ACTIONS: &[ActionDef] = &[
    ActionDef {
        name: "graze",
        entries: &["horse_graze"],
        chain_from: Some(4.0),
        idle: true,
    },
    ActionDef {
        name: "drink",
        entries: &["horse_drink"],
        chain_from: Some(3.8),
        idle: false,
    },
    ActionDef {
        name: "rear",
        entries: &["horse_rear"],
        chain_from: None,
        idle: false,
    },
    ActionDef {
        name: "shake",
        entries: &["horse_shake"],
        chain_from: None,
        idle: true,
    },
    ActionDef {
        name: "paw",
        entries: &["horse_paw"],
        chain_from: None,
        idle: true,
    },
];

const DOG_ACTIONS: &[ActionDef] = &[
    ActionDef {
        name: "sit",
        entries: &["dog_sit"],
        chain_from: Some(1.4),
        idle: true,
    },
    ActionDef {
        name: "lie",
        entries: &["dog_lie", "dog_lie_from_sit"],
        chain_from: None,
        idle: true,
    },
    ActionDef {
        name: "play_bow",
        entries: &["dog_play_bow"],
        chain_from: None,
        idle: true,
    },
    ActionDef {
        name: "shake_off",
        entries: &["dog_shake_off"],
        chain_from: None,
        idle: false,
    },
    ActionDef {
        name: "sniff",
        entries: &["dog_sniff"],
        chain_from: None,
        idle: true,
    },
];

impl AnimalKind {
    /// The animal a skeleton is, if it is one with actions.
    pub fn of_skeleton(name: &str) -> Option<AnimalKind> {
        if name == horse::SKELETON {
            Some(AnimalKind::Horse)
        } else if name == dog::SKELETON || name.starts_with("dog@") {
            Some(AnimalKind::Dog)
        } else {
            None
        }
    }

    fn actions(self) -> &'static [ActionDef] {
        match self {
            AnimalKind::Horse => HORSE_ACTIONS,
            AnimalKind::Dog => DOG_ACTIONS,
        }
    }

    /// The action names it has.
    pub fn action_names(self) -> impl Iterator<Item = &'static str> {
        self.actions().iter().map(|a| a.name)
    }

    fn neck(self) -> &'static [&'static str] {
        match self {
            AnimalKind::Horse => &["neck_1", "neck_2", "neck_3", "head"],
            AnimalKind::Dog => &["neck_1", "neck_2", "head"],
        }
    }

    /// Where the muzzle is in the head's frame at full size, and the head's rest height there.
    fn muzzle(self) -> (Vec3, f32) {
        match self {
            AnimalKind::Horse => (horse::MUZZLE, 0.0),
            AnimalKind::Dog => (dog::MUZZLE, 0.69),
        }
    }
}

/// An action under way.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PlayingAction {
    pub action: String,
    /// The clip chosen for it (with its size suffix).
    pub clip: String,
    /// Seconds into it.
    pub time: f32,
    /// How far the pose was from the clip's first one when it started (RMS over the bones, m),
    /// and the fade-in that gave.
    pub distance: f32,
    pub fade_in: f32,
}

/// A horse's or a dog's actions (published): what is playing, what is asked for, and the
/// idle picker. Given to every horse and dog body; a game turns the picker off with
/// `idle_after` 0.
#[derive(Component, Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct AnimalActions {
    pub kind: AnimalKind,
    /// Seconds standing still before the idle picker plays an action (0 = never).
    pub idle_after: f32,
    pub seed: u32,
    /// Actions asked for, played in turn.
    pub queue: Vec<String>,
    pub playing: Option<PlayingAction>,
    /// The point the head reaches for while a clip's `reach` curve is up (world), from the
    /// spot it stopped at.
    pub reach_to: Option<DVec3>,
    #[serde(skip)]
    still: f32,
    #[serde(skip)]
    last: Option<DVec3>,
    #[serde(skip)]
    picks: u32,
    /// Parameters `act.<action>` that were up last frame.
    #[serde(skip)]
    raised: Vec<String>,
    /// Stopped at a spot (until it moves again), and the action it is for.
    #[serde(skip)]
    spotted: bool,
    #[serde(skip)]
    spot_action: Option<String>,
}

impl Default for AnimalActions {
    fn default() -> Self {
        Self::new(AnimalKind::Horse, 0)
    }
}

impl AnimalActions {
    pub fn new(kind: AnimalKind, seed: u32) -> Self {
        Self {
            kind,
            idle_after: 9.0,
            seed,
            queue: Vec::new(),
            playing: None,
            reach_to: None,
            still: 0.0,
            last: None,
            picks: 0,
            raised: Vec::new(),
            spotted: false,
            spot_action: None,
        }
    }

    /// Ask for an action by name (`graze`, `sit`...); unknown names are ignored.
    pub fn request(&mut self, action: &str) {
        if self.kind.action_names().any(|a| a == action) {
            self.queue.push(action.to_string());
        }
    }
}

/// A place an animal does something (published): the actions it is for (the first one the
/// animal has is played), the point they are about (the water's surface, the grass; local,
/// m) and how near the animal must stop, m.
#[derive(Component, Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct ActionSpot {
    pub actions: Vec<String>,
    pub point: Vec3,
    pub radius: f32,
}

/// Give every horse and dog body its [`AnimalActions`].
pub(crate) fn give_animal_actions(
    mut commands: Commands,
    q: Query<(Entity, &Animator), Without<AnimalActions>>,
) {
    for (entity, animator) in &q {
        if let Some(kind) = AnimalKind::of_skeleton(&animator.skeleton.name) {
            commands
                .entity(entity)
                .try_insert(AnimalActions::new(kind, entity.to_bits() as u32));
        }
    }
}

/// How long to fade into a clip whose first pose is `distance` (RMS, m) from the pose shown,
/// for a body `height` m tall: the farther the pose has to travel, the longer.
pub fn fade_for(distance: f32, height: f32) -> f32 {
    (FADE_MIN + FADE_PER_HEIGHT * distance / height.max(0.1)).clamp(FADE_MIN, FADE_MAX)
}

/// A clip's first pose in model space (bone positions).
fn first_pose(clip: &crate::clip::AnimationClip, s: &crate::skeleton::Skeleton) -> Vec<Vec3> {
    let binding = crate::clip::ClipBinding::new(clip, s);
    let mut local = s.rest.clone();
    clip.sample(0.0, &binding, &mut local);
    let mut model = s.rest_model.clone();
    s.to_model(&local, &mut model);
    model.iter().map(|x| x.t).collect()
}

/// RMS distance between two poses' bone positions, m.
pub fn pose_distance(a: &[Vec3], b: &[Vec3]) -> f32 {
    let n = a.len().min(b.len()).max(1);
    let sum: f32 = a.iter().zip(b).map(|(p, q)| p.distance_squared(*q)).sum();
    (sum / n as f32).sqrt()
}

/// Of an action's entry clips, the one to play from the pose shown: (clip, distance).
fn choose_entry(
    def: &ActionDef,
    animator: &Animator,
    library: &AnimationLibrary,
    cache: &mut HashMap<String, Arc<Vec<Vec3>>>,
) -> Option<(Arc<crate::clip::AnimationClip>, f32)> {
    let s = &animator.skeleton;
    let shown: Vec<Vec3> = animator.model.iter().map(|x| x.t).collect();
    def.entries
        .iter()
        .filter_map(|base| {
            let name = sized_name(base, &s.name);
            let clip = library.clips.get(&name)?.clone();
            let pose = cache
                .entry(name)
                .or_insert_with(|| Arc::new(first_pose(&clip, s)))
                .clone();
            Some((clip, pose_distance(&shown, &pose)))
        })
        .min_by(|a, b| a.1.total_cmp(&b.1))
}

fn hash(seed: u32, n: u32) -> u32 {
    let mut x = seed.wrapping_mul(0x9e37_79b9) ^ n.wrapping_mul(0x85eb_ca6b);
    x ^= x >> 16;
    x = x.wrapping_mul(0x7feb_352d);
    x ^ (x >> 15)
}

/// Run each animal's actions (`AnimSet::Setup`): spots, parameters and the idle picker ask for
/// them, the queue starts them with the nearest entry, and moving off cuts them short.
#[allow(clippy::type_complexity)]
pub(crate) fn run_animal_actions(
    time: Res<Time>,
    library: Res<AnimationLibrary>,
    spots: Query<(&GlobalTransform, &ActionSpot)>,
    mut q: Query<(
        &GlobalTransform,
        &mut Animator,
        &mut AnimalActions,
        Option<&AnimationParams>,
        Option<&LocalTimeScale>,
    )>,
    mut cache: Local<HashMap<String, Arc<Vec<Vec3>>>>,
) {
    for (gt, mut animator, mut actions, params, local) in &mut q {
        let dt = time.delta_for(local);
        if dt <= 0.0 {
            continue;
        }
        let actions = &mut *actions;
        let kind = actions.kind;
        let height = animator
            .skeleton
            .index_of("body")
            .map_or(1.0, |b| animator.skeleton.rest_model[b].t.y);
        let size = gt.scale().y * height;
        let here = gt.translation();
        let speed = actions
            .last
            .map_or(0.0, |p| (here - p).with_y(0.0).length() as f32 / dt);
        actions.last = Some(here);
        if speed < STILL_SPEED * size {
            actions.still += dt;
        } else {
            actions.still = 0.0;
            actions.spotted = false;
            actions.spot_action = None;
            actions.reach_to = None;
        }

        // What is playing: the overlay it started, or nothing if something else took over.
        if let Some(p) = &mut actions.playing {
            match animator.overlay() {
                Some(o) if o.clip.name == p.clip => p.time = o.time,
                _ => actions.playing = None,
            }
        }
        if let Some(p) = &actions.playing
            && p.time >= library.clips.get(&p.clip).map_or(0.0, |c| c.duration)
        {
            actions.playing = None;
            actions.still = 0.0;
        }
        if actions.playing.is_some() && speed > MOVING_SPEED * size {
            animator.stop_overlay(0.3);
            actions.playing = None;
            actions.queue.clear();
        }

        // Asked for by parameters.
        if let Some(params) = params {
            for name in kind.action_names() {
                let key = format!("act.{name}");
                let up = params.get(&key) > 0.5;
                let was = actions.raised.contains(&key);
                if up && !was {
                    actions.raised.push(key);
                    actions.request(name);
                } else if !up && was {
                    actions.raised.retain(|r| *r != key);
                }
            }
        }

        // A spot it has stopped at: the head reaches for its point, and its action is asked
        // for unless it already has been.
        if !actions.spotted && actions.still > SPOT_SETTLE {
            let near = spots.iter().find_map(|(sgt, spot)| {
                let center = sgt.translation();
                let off = (center - here).with_y(0.0).length() as f32;
                if off > spot.radius {
                    return None;
                }
                let action = spot
                    .actions
                    .iter()
                    .find(|a| kind.action_names().any(|n| n == a.as_str()))?;
                Some((action.clone(), sgt.transform_point(spot.point.as_dvec3())))
            });
            if let Some((action, point)) = near {
                actions.spotted = true;
                actions.reach_to = Some(point);
                let asked = actions.queue.contains(&action)
                    || actions.playing.as_ref().is_some_and(|p| p.action == action);
                if !asked {
                    actions.request(&action);
                }
                actions.spot_action = Some(action);
            }
        }
        let idle = actions.playing.is_none() && actions.queue.is_empty();

        // The idle picker.
        if idle && actions.idle_after > 0.0 && actions.still > actions.idle_after {
            let pool: Vec<&str> = kind
                .actions()
                .iter()
                .filter(|a| a.idle)
                .map(|a| a.name)
                .collect();
            let pick = pool[hash(actions.seed, actions.picks) as usize % pool.len()];
            actions.picks += 1;
            actions.still = 0.0;
            actions.request(pick);
        }

        // Start the next action asked for, cutting in at the playing one's chain point.
        let ready = match &actions.playing {
            None => true,
            Some(p) => kind
                .actions()
                .iter()
                .find(|a| a.name == p.action)
                .and_then(|a| a.chain_from)
                .is_some_and(|from| p.time >= from),
        };
        if ready && !actions.queue.is_empty() {
            let name = actions.queue.remove(0);
            let Some(def) = kind.actions().iter().find(|a| a.name == name) else {
                continue;
            };
            if let Some((clip, distance)) = choose_entry(def, &animator, &library, &mut cache) {
                // The head reaches only in the action its spot is for.
                if actions.spot_action.as_deref() != Some(name.as_str()) {
                    actions.reach_to = None;
                }
                let fade_in = fade_for(distance, height);
                actions.playing = Some(PlayingAction {
                    action: name,
                    clip: clip.name.clone(),
                    time: 0.0,
                    distance,
                    fade_in,
                });
                animator.play_over(clip, fade_in, FADE_OUT);
            }
        }
    }
}

/// Turn the neck of an animal at a spot so its muzzle meets the spot's point, as far as the
/// playing clip's `reach` curve asks (`AnimSet::Ik`): cyclic coordinate descent from the head
/// down the neck, each bone turning at most [`REACH_PER_BONE`].
pub(crate) fn reach_heads(mut q: Query<(&GlobalTransform, &mut Animator, &AnimalActions)>) {
    for (gt, mut animator, actions) in &mut q {
        let Some(point) = actions.reach_to else {
            continue;
        };
        let w = animator.curve("reach").clamp(0.0, 1.0);
        if w <= 1e-3 {
            continue;
        }
        let skeleton = animator.skeleton.clone();
        let bones: Vec<usize> = actions
            .kind
            .neck()
            .iter()
            .filter_map(|b| skeleton.index_of(b))
            .collect();
        let Some(&head) = bones.last() else {
            continue;
        };
        let (muzzle, rest_height) = actions.kind.muzzle();
        let scale = if rest_height > 0.0 {
            skeleton.rest_model[head].t.y / rest_height
        } else {
            1.0
        };
        let muzzle = muzzle * scale;
        let inv = gt.inverse();
        let target = inv.transform_point3(point).as_vec3();
        let limit = REACH_PER_BONE.to_radians() * w;
        let mut edit = animator.edit();
        for _ in 0..2 {
            for &b in bones.iter().rev() {
                let p = edit.position(b);
                let tip = edit.model[head].transform_point(muzzle);
                let (from, to) = (tip - p, target - p);
                if from.length_squared() < 1e-6 || to.length_squared() < 1e-6 {
                    continue;
                }
                let turn = rotation_between(from.normalize(), to.normalize());
                let (axis, angle) = turn.to_axis_angle();
                let angle = angle.min(limit) * w;
                if angle > 1e-4 {
                    edit.rotate_model(b, Quat::from_axis_angle(axis, angle));
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::build::{dog, horse};

    #[test]
    fn a_far_pose_fades_in_longer_within_bounds() {
        assert_eq!(fade_for(0.0, 0.5), FADE_MIN);
        assert!(fade_for(0.05, 0.5) > FADE_MIN);
        assert_eq!(fade_for(5.0, 0.5), FADE_MAX);
    }

    #[test]
    fn a_sitting_dog_lies_down_from_the_sit() {
        let s = dog::skeleton(1.0);
        let clips = dog::action_clips(&s, 1.0);
        let get = |n: &str| clips.iter().find(|c| c.name == n).expect(n);
        let sit = get("dog_sit");
        // Halfway through the sit's hold.
        let binding = crate::clip::ClipBinding::new(sit, &s);
        let mut local = s.rest.clone();
        sit.sample(2.0, &binding, &mut local);
        let mut model = s.rest_model.clone();
        s.to_model(&local, &mut model);
        let sitting: Vec<Vec3> = model.iter().map(|x| x.t).collect();
        let standing: Vec<Vec3> = s.rest_model.iter().map(|x| x.t).collect();
        let from_stand = first_pose(get("dog_lie"), &s);
        let from_sit = first_pose(get("dog_lie_from_sit"), &s);
        assert!(pose_distance(&sitting, &from_sit) < pose_distance(&sitting, &from_stand));
        assert!(pose_distance(&standing, &from_stand) < pose_distance(&standing, &from_sit));
    }

    #[test]
    fn every_action_has_its_clips_at_every_size() {
        let h = horse::skeleton();
        let names: Vec<String> = horse::clips(&h).into_iter().map(|c| c.name).collect();
        for def in HORSE_ACTIONS {
            for e in def.entries {
                assert!(names.iter().any(|n| n == e), "{e}");
            }
        }
        for scale in [1.0, dog::scale_for_height(0.35)] {
            let s = dog::skeleton(scale);
            let names: Vec<String> = dog::action_clips(&s, scale)
                .into_iter()
                .map(|c| c.name)
                .collect();
            for def in DOG_ACTIONS {
                for e in def.entries {
                    let want = sized_name(e, &s.name);
                    assert!(names.contains(&want), "{want}");
                }
            }
        }
    }

    #[test]
    fn requests_ignore_actions_the_animal_lacks() {
        let mut a = AnimalActions::new(AnimalKind::Dog, 1);
        a.request("graze");
        a.request("sit");
        assert_eq!(a.queue, vec!["sit".to_string()]);
    }
}
