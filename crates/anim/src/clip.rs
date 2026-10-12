//! Animation clips: keyframed bone tracks, float curves and events.
//!
//! A clip names bones, so one clip plays on any skeleton with those bone names (bones the
//! clip doesn't mention keep their incoming pose). Rotations are sampled with shortest-arc
//! interpolation; translation and scale linearly; `step` tracks hold each key.
//!
//! Curves are named float channels that blend like poses (`effort` drives muscle flex during
//! a heavy swing). Events fire when playback crosses their time (`footstep_left` with
//! `bone: "foot_l"` becomes a ground contact and a footstep sound).

use std::sync::Arc;

use engine_core::math::{Quat, Vec3};
use serde::{Deserialize, Serialize};

use crate::skeleton::Skeleton;
use crate::xform::{Xf, nlerp};

/// Keys of one channel. `times` are seconds, ascending, same length as `values`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Track<T> {
    pub times: Vec<f32>,
    pub values: Vec<T>,
    /// Hold each key until the next instead of interpolating.
    pub step: bool,
}

/// Interpolation for a track's value type.
pub trait Lerp: Copy {
    fn lerp_to(self, other: Self, w: f32) -> Self;
}

impl Lerp for f32 {
    fn lerp_to(self, other: Self, w: f32) -> Self {
        self + (other - self) * w
    }
}

impl Lerp for Vec3 {
    fn lerp_to(self, other: Self, w: f32) -> Self {
        self.lerp(other, w)
    }
}

impl Lerp for Quat {
    fn lerp_to(self, other: Self, w: f32) -> Self {
        nlerp(self, other, w)
    }
}

impl<T: Lerp> Track<T> {
    pub fn new(times: Vec<f32>, values: Vec<T>) -> Self {
        Self {
            times,
            values,
            step: false,
        }
    }

    pub fn is_empty(&self) -> bool {
        self.values.is_empty()
    }

    /// Value at `t` seconds (clamped to the first/last key).
    pub fn sample(&self, t: f32) -> Option<T> {
        let n = self.values.len().min(self.times.len());
        if n == 0 {
            return None;
        }
        if n == 1 || t <= self.times[0] {
            return Some(self.values[0]);
        }
        if t >= self.times[n - 1] {
            return Some(self.values[n - 1]);
        }
        // First key strictly after t.
        let hi = self.times[..n].partition_point(|&k| k <= t);
        let lo = hi - 1;
        if self.step {
            return Some(self.values[lo]);
        }
        let span = self.times[hi] - self.times[lo];
        let w = if span > 1e-9 {
            (t - self.times[lo]) / span
        } else {
            0.0
        };
        Some(self.values[lo].lerp_to(self.values[hi], w))
    }
}

/// Keyframes for one bone.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct BoneTrack {
    pub bone: String,
    pub translation: Track<Vec3>,
    pub rotation: Track<Quat>,
    pub scale: Track<Vec3>,
}

/// A named float channel (e.g. `effort`, `muscle.biceps_l`).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct CurveTrack {
    pub name: String,
    pub track: Track<f32>,
}

/// A marker that fires when playback crosses `time`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ClipEvent {
    /// Seconds from the clip start.
    pub time: f32,
    /// `footstep_left`, `hit`, `weapon_release`…
    pub name: String,
    /// Where it happens (for footprints and sparks).
    pub bone: Option<String>,
}

/// A keyframed animation.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct AnimationClip {
    pub name: String,
    /// Seconds.
    pub duration: f32,
    /// Loops by default when played (locomotion); one-shots set this to false.
    pub looping: bool,
    pub tracks: Vec<BoneTrack>,
    pub curves: Vec<CurveTrack>,
    pub events: Vec<ClipEvent>,
    /// Bone whose horizontal motion is root motion (`"root"` for most rigs).
    pub root_bone: Option<String>,
}

/// Which skeleton bone each track of a clip drives.
#[derive(Debug, Clone, Default)]
pub struct ClipBinding {
    pub bones: Vec<Option<u16>>,
    pub root: Option<u16>,
}

impl ClipBinding {
    pub fn new(clip: &AnimationClip, skeleton: &Skeleton) -> Self {
        Self {
            bones: clip
                .tracks
                .iter()
                .map(|t| skeleton.index_of(&t.bone).map(|i| i as u16))
                .collect(),
            root: clip
                .root_bone
                .as_deref()
                .and_then(|r| skeleton.index_of(r))
                .map(|i| i as u16),
        }
    }
}

impl AnimationClip {
    pub fn new(name: &str, duration: f32) -> Self {
        Self {
            name: name.into(),
            duration,
            looping: true,
            ..Default::default()
        }
    }

    /// Wrap or clamp a playback time into the clip.
    pub fn local_time(&self, t: f32) -> f32 {
        if self.duration <= 0.0 {
            0.0
        } else if self.looping {
            t.rem_euclid(self.duration)
        } else {
            t.clamp(0.0, self.duration)
        }
    }

    /// Write the clip's pose at `t` into `out` (bones the clip has no track for are left
    /// untouched; initialize `out` with the rest pose).
    pub fn sample(&self, t: f32, binding: &ClipBinding, out: &mut [Xf]) {
        let t = self.local_time(t);
        for (track, bone) in self.tracks.iter().zip(&binding.bones) {
            let Some(b) = bone else { continue };
            let Some(x) = out.get_mut(*b as usize) else {
                continue;
            };
            if let Some(v) = track.translation.sample(t) {
                x.t = v;
            }
            if let Some(v) = track.rotation.sample(t) {
                x.r = v;
            }
            if let Some(v) = track.scale.sample(t) {
                x.s = v;
            }
        }
    }

    /// Value of a curve at `t`, if the clip has it.
    pub fn curve(&self, name: &str, t: f32) -> Option<f32> {
        let t = self.local_time(t);
        self.curves
            .iter()
            .find(|c| c.name == name)
            .and_then(|c| c.track.sample(t))
    }

    /// Translation of the root bone's track at playback time `t` (wrapped into the clip).
    pub fn root_translation(&self, binding: &ClipBinding, t: f32) -> Option<Vec3> {
        self.root_at(binding, self.local_time(t))
    }

    /// Root translation at a clip-local time (not wrapped, so `duration` is the last key).
    fn root_at(&self, binding: &ClipBinding, local: f32) -> Option<Vec3> {
        let root = binding.root?;
        let i = binding.bones.iter().position(|b| *b == Some(root))?;
        self.tracks[i].translation.sample(local)
    }

    /// Root displacement between playback times `from` and `to` (seconds, `to >= from`),
    /// following loops: a looping clip that wraps adds a full cycle's travel.
    pub fn root_delta(&self, binding: &ClipBinding, from: f32, to: f32) -> Vec3 {
        let Some(start) = self.root_at(binding, 0.0) else {
            return Vec3::ZERO;
        };
        if self.duration <= 0.0 || to <= from {
            return Vec3::ZERO;
        }
        if !self.looping {
            let a = self.root_translation(binding, from).unwrap_or(start);
            let b = self.root_translation(binding, to).unwrap_or(start);
            return b - a;
        }
        let end = self.root_at(binding, self.duration).unwrap_or(start);
        let cycle = end - start;
        let cycles_from = (from / self.duration).floor();
        let cycles_to = (to / self.duration).floor();
        let a = self.root_translation(binding, from).unwrap_or(start);
        let b = self.root_translation(binding, to).unwrap_or(start);
        b - a + cycle * (cycles_to - cycles_from)
    }

    /// Events whose time lies in `(from, to]` (handles wrapping for looping clips; `from` and
    /// `to` are raw playback times, `to >= from`).
    pub fn events_between<'a>(
        &'a self,
        from: f32,
        to: f32,
    ) -> impl Iterator<Item = &'a ClipEvent> + 'a {
        let d = self.duration.max(1e-6);
        let looping = self.looping;
        self.events.iter().filter(move |e| {
            if to <= from {
                return false;
            }
            if !looping {
                return e.time > from && e.time <= to;
            }
            // Smallest k with e.time + k·d > from; fires if that occurrence is <= to.
            let k = ((from - e.time) / d).floor() + 1.0;
            e.time + k * d <= to
        })
    }

    /// Build a clip by sampling `pose_at(phase 0..1)` at `keys` evenly spaced keys (inclusive of
    /// both ends, so looping clips line up). `pose_at` returns `(bone, rotation)` pairs and an
    /// optional root translation, which makes hand-authored procedural cycles regular clips.
    pub fn from_fn(
        name: &str,
        duration: f32,
        keys: usize,
        looping: bool,
        mut pose_at: impl FnMut(f32) -> Vec<(&'static str, Xf)>,
    ) -> Self {
        let keys = keys.max(2);
        let mut clip = AnimationClip::new(name, duration);
        clip.looping = looping;
        let mut index: Vec<&'static str> = Vec::new();
        for k in 0..keys {
            let phase = k as f32 / (keys - 1) as f32;
            let t = phase * duration;
            for (bone, x) in pose_at(phase) {
                let i = match index.iter().position(|b| *b == bone) {
                    Some(i) => i,
                    None => {
                        index.push(bone);
                        clip.tracks.push(BoneTrack {
                            bone: bone.into(),
                            ..Default::default()
                        });
                        index.len() - 1
                    }
                };
                let tr = &mut clip.tracks[i];
                tr.rotation.times.push(t);
                tr.rotation.values.push(x.r);
                if x.t != Vec3::ZERO {
                    tr.translation.times.push(t);
                    tr.translation.values.push(x.t);
                }
            }
        }
        // Tracks that only got translations at some keys are inconsistent; drop partial ones.
        for tr in &mut clip.tracks {
            if tr.translation.times.len() != tr.rotation.times.len() {
                tr.translation = Track::default();
            }
        }
        clip
    }

    /// Add a curve sampled from `f(phase)` with `keys` keys.
    pub fn with_curve(mut self, name: &str, keys: usize, f: impl Fn(f32) -> f32) -> Self {
        let keys = keys.max(2);
        let mut track = Track::default();
        for k in 0..keys {
            let phase = k as f32 / (keys - 1) as f32;
            track.times.push(phase * self.duration);
            track.values.push(f(phase));
        }
        self.curves.push(CurveTrack {
            name: name.into(),
            track,
        });
        self
    }

    /// Add an event at a phase (0..1) of the clip.
    pub fn with_event(mut self, phase: f32, name: &str, bone: Option<&str>) -> Self {
        self.events.push(ClipEvent {
            time: phase * self.duration,
            name: name.into(),
            bone: bone.map(Into::into),
        });
        self
    }

    pub fn with_root(mut self, bone: &str) -> Self {
        self.root_bone = Some(bone.into());
        self
    }

    pub fn into_arc(self) -> Arc<AnimationClip> {
        Arc::new(self)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::skeleton::{BoneDef, Skeleton};

    fn skel() -> Skeleton {
        Skeleton::new(
            "s",
            vec![
                BoneDef::new("root", None, Vec3::ZERO),
                BoneDef::new("arm", Some("root"), Vec3::Y),
            ],
        )
        .unwrap()
    }

    #[test]
    fn samples_between_keys() {
        let tr: Track<f32> = Track::new(vec![0.0, 1.0, 2.0], vec![0.0, 10.0, 0.0]);
        assert_eq!(tr.sample(0.5), Some(5.0));
        assert_eq!(tr.sample(1.5), Some(5.0));
        assert_eq!(tr.sample(-1.0), Some(0.0));
        assert_eq!(tr.sample(9.0), Some(0.0));
        let mut st = tr.clone();
        st.step = true;
        assert_eq!(st.sample(0.9), Some(0.0));
    }

    #[test]
    fn clip_from_fn_drives_named_bones() {
        let clip = AnimationClip::from_fn("wave", 1.0, 9, true, |p| {
            vec![(
                "arm",
                Xf::from_tr(Vec3::ZERO, Quat::from_rotation_z(p * 1.0)),
            )]
        });
        let s = skel();
        let b = ClipBinding::new(&clip, &s);
        let mut pose = s.rest.clone();
        clip.sample(0.5, &b, &mut pose);
        assert!(pose[1].r.angle_between(Quat::from_rotation_z(0.5)) < 1e-3);
        // Untouched bones keep the rest pose.
        assert_eq!(pose[0], s.rest[0]);
    }

    #[test]
    fn events_fire_once_per_loop() {
        let clip = AnimationClip::new("walk", 1.0)
            .with_event(0.25, "footstep_left", Some("foot_l"))
            .with_event(0.75, "footstep_right", Some("foot_r"));
        let names = |a: f32, b: f32| {
            clip.events_between(a, b)
                .map(|e| e.name.clone())
                .collect::<Vec<_>>()
        };
        assert_eq!(names(0.0, 0.3), vec!["footstep_left"]);
        assert_eq!(names(0.3, 0.7), Vec::<String>::new());
        // Wrapping from 0.9 into the next loop at 1.3 crosses 0.25 of loop 2.
        assert_eq!(names(0.9, 1.3), vec!["footstep_left"]);
        // A big step crosses both.
        assert_eq!(names(0.1, 1.05).len(), 2);
    }

    #[test]
    fn root_delta_accumulates_over_loops() {
        let mut clip = AnimationClip::new("walk", 1.0).with_root("root");
        clip.tracks.push(BoneTrack {
            bone: "root".into(),
            translation: Track::new(vec![0.0, 1.0], vec![Vec3::ZERO, Vec3::new(0.0, 0.0, -1.4)]),
            ..Default::default()
        });
        let s = skel();
        let b = ClipBinding::new(&clip, &s);
        let d = clip.root_delta(&b, 0.5, 2.5);
        assert!((d.z + 2.8).abs() < 1e-4, "{d:?}");
    }
}
