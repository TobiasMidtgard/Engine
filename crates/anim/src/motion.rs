//! Motion matching: instead of blending a few loops by speed, a character plays whichever
//! moment of its captured motion best continues its pose toward where it is going. Every few
//! frames the [`MotionDatabase`] is searched for the frame whose feet, hips and next second of
//! travel best match the character's current pose and its predicted path; a jump to a better
//! frame is hidden by inertialization (the pose difference decays on a spring, keeping its
//! velocity). This is the approach of Ubisoft's For Honor (Clavet, GDC 2016) and Unreal 5's
//! Pose Search, with the feature set and spring blending of Holden's "Code vs Data Driven
//! Displacement" and "Dead Blending" articles (theorangeduck.com).
//!
//! The database is built from clips on the character's own skeleton that keep their root
//! motion: starts, stops, turns and loops at several speeds, each also mirrored. A simulation
//! root (the hips' ground position and facing, smoothed) is taken out of every frame, so frames
//! play in place facing −Z while gameplay moves the entity. The graph node is
//! [`crate::graph::NodeDef::MotionMatch`]; it hands over to a fallback node (the strafe and
//! backpedal blends) for motion the database doesn't cover.
//!
//! Feature vector (27 numbers, in the frame's root space): both feet's positions (0.75) and
//! velocities (1), the hips' velocity (1), and where the root will be 1/3, 2/3 and 1 s later
//! (1) and which way it will face then (1.5); each group is scaled by its spread over the
//! database.

#![allow(clippy::needless_range_loop)] // feature vectors are indexed by dimension

use std::sync::Arc;

use engine_core::math::{Quat, Vec2, Vec3};
use engine_world::animation::AnimationParams;

use crate::clip::{AnimationClip, ClipBinding};
use crate::skeleton::Skeleton;
use crate::xform::Xf;

/// Frames per second the database is sampled at.
pub const FPS: f32 = 30.0;
/// Future trajectory samples, frames ahead.
const FUTURE: [usize; 3] = [10, 20, 30];
/// Numbers per feature vector.
pub const DIM: usize = 27;
/// Feature groups: (first index, length, weight).
const GROUPS: [(usize, usize, f32); 7] = [
    (0, 3, 0.75), // left foot position
    (3, 3, 0.75), // right foot position
    (6, 3, 1.0),  // left foot velocity
    (9, 3, 1.0),  // right foot velocity
    (12, 3, 1.0), // hips velocity
    (15, 6, 2.0), // trajectory positions (x, z) × 3
    (21, 6, 1.5), // trajectory directions (x, z) × 3
];
/// A looping clip is played round to at least this many seconds.
const LOOP_SECS: f32 = 3.0;
/// Frames a search may land on need this many frames of the same clip after them.
const MIN_PLAY: usize = 6;
/// Seconds at a clip's end whose travel is carried on past it (for the trajectory of frames
/// near the end).
const END_SECS: f32 = 0.2;
/// Seconds over which the simulation root's position and facing are smoothed (half window).
const ROOT_SMOOTH_SECS: f32 = 0.5;

/// A frame of a clip in the database.
#[derive(Debug, Clone, Copy)]
struct Range {
    /// One past the last frame.
    end: usize,
    /// Index into [`MotionDatabase::clip_names`].
    clip: usize,
}

/// Captured motion prepared for motion matching. Build with [`MotionDatabase::build`].
#[derive(Debug, Clone)]
pub struct MotionDatabase {
    pub name: String,
    bones: usize,
    /// Local poses, `bones` per frame, the root taken out (in place, facing −Z).
    poses: Vec<Xf>,
    /// Root velocity (m/s, root space) and turn rate (rad/s, + left) per frame.
    root_velocity: Vec<Vec3>,
    root_turn: Vec<f32>,
    /// Weighted, normalized features per frame.
    features: Vec<[f32; DIM]>,
    /// Normalization: feature = (raw − offset) × scale.
    offset: [f32; DIM],
    scale: [f32; DIM],
    /// Frames a search may land on (with a second of motion after them in the same clip).
    candidate: Vec<bool>,
    ranges: Vec<Range>,
    frame_range: Vec<u32>,
    pub clip_names: Vec<String>,
    /// Events fired on entering each frame: (frame, name, bone).
    events: Vec<(usize, String, Option<String>)>,
    /// Fastest root speed in the database, m/s.
    pub top_speed: f32,
}

/// Bone indices the features read.
struct Rig {
    hips: usize,
    feet: [usize; 2],
}

impl Rig {
    fn of(skeleton: &Skeleton) -> Option<Self> {
        Some(Self {
            hips: skeleton.index_of("hips")?,
            feet: [skeleton.index_of("foot_l")?, skeleton.index_of("foot_r")?],
        })
    }
}

/// Mirror image (across the character's left-right plane, x → −x) of a rotation.
fn mirror_q(q: Quat) -> Quat {
    Quat::from_xyzw(q.x, -q.y, -q.z, q.w)
}

fn mirror_v(v: Vec3) -> Vec3 {
    Vec3::new(-v.x, v.y, v.z)
}

/// The bone on the other side (`_l` ↔ `_r`), or itself.
fn partner(skeleton: &Skeleton, b: usize) -> usize {
    let name = &skeleton.names[b];
    let other = if let Some(s) = name.strip_suffix("_l") {
        format!("{s}_r")
    } else if let Some(s) = name.strip_suffix("_r") {
        format!("{s}_l")
    } else {
        return b;
    };
    skeleton.index_of(&other).unwrap_or(b)
}

/// Heading (radians about +Y, 0 facing −Z, + to the left) of a horizontal direction.
fn yaw_of(d: Vec3) -> f32 {
    (-d.x).atan2(-d.z)
}

fn facing(yaw: f32) -> Vec3 {
    Vec3::new(-yaw.sin(), 0.0, -yaw.cos())
}

fn wrap(a: f32) -> f32 {
    (a + std::f32::consts::PI).rem_euclid(std::f32::consts::TAU) - std::f32::consts::PI
}

/// Rotation as a scaled axis (axis × angle), the short way round.
pub(crate) fn to_scaled_axis(q: Quat) -> Vec3 {
    let q = if q.w < 0.0 { -q } else { q };
    let (axis, angle) = q.to_axis_angle();
    if angle.abs() < 1e-6 || !axis.is_finite() {
        Vec3::ZERO
    } else {
        axis * angle
    }
}

pub(crate) fn from_scaled_axis(v: Vec3) -> Quat {
    let angle = v.length();
    if angle < 1e-8 {
        Quat::IDENTITY
    } else {
        Quat::from_axis_angle(v / angle, angle)
    }
}

/// Decay `x` (moving at `v`) toward zero on a critically damped spring with this half-life
/// (Holden, "Spring-It-On", exact form).
pub(crate) fn decay_spring(x: &mut Vec3, v: &mut Vec3, halflife: f32, dt: f32) {
    let y = (4.0 * std::f32::consts::LN_2) / halflife.max(1e-4) / 2.0;
    let j1 = *v + *x * y;
    let eydt = (-y * dt).exp();
    *x = eydt * (*x + j1 * dt);
    *v = eydt * (*v - j1 * y * dt);
}

impl MotionDatabase {
    /// Build from clips on `skeleton` that keep their root motion in the hips' translation.
    /// Each clip is added as captured and mirrored; looping ones (a cycle of a walk) play round
    /// several times. Past a clip's end its trajectory carries on as over its last moments.
    pub fn build(name: &str, skeleton: &Skeleton, clips: &[AnimationClip]) -> Result<Self, String> {
        let rig = Rig::of(skeleton).ok_or("motion matching needs hips, foot_l and foot_r")?;
        let n = skeleton.len();
        let mut db = MotionDatabase {
            name: name.into(),
            bones: n,
            poses: Vec::new(),
            root_velocity: Vec::new(),
            root_turn: Vec::new(),
            features: Vec::new(),
            offset: [0.0; DIM],
            scale: [1.0; DIM],
            candidate: Vec::new(),
            ranges: Vec::new(),
            frame_range: Vec::new(),
            clip_names: Vec::new(),
            events: Vec::new(),
            top_speed: 0.0,
        };
        let mut raw: Vec<[f32; DIM]> = Vec::new();
        let partners: Vec<usize> = (0..n).map(|b| partner(skeleton, b)).collect();
        for clip in clips {
            let binding = ClipBinding::new(clip, skeleton);
            // A looping clip (one cycle of a walk, say) is played round several times, its
            // travel carried on each time, so every frame has a second of motion after it.
            let cycles = if clip.looping && clip.duration > 0.0 {
                (LOOP_SECS / clip.duration).ceil().max(1.0) as usize
            } else {
                1
            };
            let frames = ((clip.duration * cycles as f32 * FPS).round() as usize).max(2);
            let mut first = skeleton.rest.clone();
            clip.sample(0.0, &binding, &mut first);
            let mut end = skeleton.rest.clone();
            clip.sample((clip.duration - 1e-4).max(0.0), &binding, &mut end);
            let travel = end[rig.hips].t - first[rig.hips].t;
            let travel = Vec3::new(travel.x, 0.0, travel.z);
            let mut locals: Vec<Vec<Xf>> = Vec::with_capacity(frames);
            let last = (clip.duration - 1e-4).max(0.0);
            for f in 0..frames {
                let t = f as f32 / FPS;
                let mut local = skeleton.rest.clone();
                if cycles > 1 {
                    let k = (t / clip.duration).floor();
                    clip.sample(t - k * clip.duration, &binding, &mut local);
                    local[rig.hips].t += travel * k;
                } else {
                    // Once through, just short of the end.
                    clip.sample(t.min(last), &binding, &mut local);
                }
                locals.push(local);
            }
            for mirrored in [false, true] {
                let locals: Vec<Vec<Xf>> = if mirrored {
                    locals
                        .iter()
                        .map(|l| {
                            (0..n)
                                .map(|b| {
                                    let src = l[partners[b]];
                                    Xf {
                                        // Only the hips carry translation the clip moves.
                                        t: if b == rig.hips {
                                            mirror_v(src.t)
                                        } else {
                                            l[b].t
                                        },
                                        r: mirror_q(src.r),
                                        s: l[b].s,
                                    }
                                })
                                .collect()
                        })
                        .collect()
                } else {
                    locals.clone()
                };
                let label = if mirrored {
                    format!("{} (mirrored)", clip.name)
                } else {
                    clip.name.clone()
                };
                db.add_clip(
                    skeleton, &rig, &locals, clip, cycles, mirrored, label, &mut raw,
                );
            }
        }
        if db.ranges.is_empty() {
            return Err("no clips".into());
        }
        db.normalize(&raw);
        Ok(db)
    }

    #[allow(clippy::too_many_arguments)]
    fn add_clip(
        &mut self,
        skeleton: &Skeleton,
        rig: &Rig,
        locals: &[Vec<Xf>],
        clip: &AnimationClip,
        cycles: usize,
        mirrored: bool,
        label: String,
        raw: &mut Vec<[f32; DIM]>,
    ) {
        let frames = locals.len();
        let n = self.bones;
        let mut models: Vec<Vec<Xf>> = Vec::with_capacity(frames);
        for l in locals {
            let mut m = Vec::new();
            skeleton.to_model(l, &mut m);
            models.push(m);
        }
        // The simulation root: the hips over the ground, facing where the hips face, smoothed.
        let rest_hips = skeleton.rest_model[rig.hips].r;
        let raw_pos: Vec<Vec3> = models
            .iter()
            .map(|m| Vec3::new(m[rig.hips].t.x, 0.0, m[rig.hips].t.z))
            .collect();
        let raw_dir: Vec<Vec3> = models
            .iter()
            .map(|m| {
                let f = m[rig.hips].r * rest_hips.conjugate() * Vec3::NEG_Z;
                Vec3::new(f.x, 0.0, f.z).normalize_or(Vec3::NEG_Z)
            })
            .collect();
        let half = (ROOT_SMOOTH_SECS * FPS) as isize;
        let smooth = |v: &[Vec3], f: usize| -> Vec3 {
            let mut sum = Vec3::ZERO;
            let mut total = 0.0;
            for k in -half..=half {
                let i = (f as isize + k).clamp(0, frames as isize - 1) as usize;
                let w = 1.0 - (k as f32 / (half as f32 + 1.0)).abs();
                sum += v[i] * w;
                total += w;
            }
            sum / total
        };
        let root_pos: Vec<Vec3> = (0..frames).map(|f| smooth(&raw_pos, f)).collect();
        let root_yaw: Vec<f32> = (0..frames)
            .map(|f| yaw_of(smooth(&raw_dir, f).normalize_or(Vec3::NEG_Z)))
            .collect();
        // Past the end the root carries on as over the clip's last moments.
        let tail = ((END_SECS * FPS) as usize).clamp(1, frames - 1);
        let end_velocity = (root_pos[frames - 1] - root_pos[frames - 1 - tail]) * FPS / tail as f32;
        let end_turn = wrap(root_yaw[frames - 1] - root_yaw[frames - 1 - tail]) * FPS / tail as f32;
        let root_yaw_at = |f: usize| -> f32 {
            if f < frames {
                root_yaw[f]
            } else {
                root_yaw[frames - 1] + end_turn * (f + 1 - frames) as f32 / FPS
            }
        };
        let root = |f: usize| -> Xf {
            if f < frames {
                Xf::from_tr(root_pos[f], Quat::from_rotation_y(root_yaw[f]))
            } else {
                let past = (f + 1 - frames) as f32 / FPS;
                Xf::from_tr(
                    root_pos[frames - 1] + end_velocity * past,
                    Quat::from_rotation_y(root_yaw_at(f)),
                )
            }
        };
        let start = self.frame_count();
        let range = self.ranges.len();
        let dt = 1.0 / FPS;
        let world_of = |f: usize, bone: usize| models[f.min(frames - 1)][bone].t;
        let velocity_of = |f: usize, bone: usize| -> Vec3 {
            let (a, b) = (f.saturating_sub(1), (f + 1).min(frames - 1));
            if b == a {
                Vec3::ZERO
            } else {
                (world_of(b, bone) - world_of(a, bone)) / ((b - a) as f32 * dt)
            }
        };
        for f in 0..frames {
            let r = root(f);
            let inv = r.inverse();
            // Pose in place: the hips relative to the root.
            let mut pose = locals[f].clone();
            if let Some(parent) = skeleton.parents[rig.hips].map(usize::from) {
                let hips_model = inv.mul(&models[f][rig.hips]);
                pose[rig.hips] = skeleton.rest_model[parent].inverse().mul(&hips_model);
                pose[parent] = skeleton.rest[parent];
            } else {
                pose[rig.hips] = inv.mul(&locals[f][rig.hips]);
            }
            self.poses.extend_from_slice(&pose[..n]);
            // Root motion this frame (central difference), in root space.
            let (a, b) = (f.saturating_sub(1), (f + 1).min(frames - 1));
            let span = ((b - a).max(1)) as f32 * dt;
            self.root_velocity
                .push(inv.transform_vector((root_pos[b] - root_pos[a]) / span));
            self.root_turn.push(wrap(root_yaw[b] - root_yaw[a]) / span);
            // Features.
            let mut x = [0.0f32; DIM];
            for (k, &foot) in rig.feet.iter().enumerate() {
                let p = inv.transform_point(world_of(f, foot));
                let v = inv.transform_vector(velocity_of(f, foot));
                x[k * 3..k * 3 + 3].copy_from_slice(&p.to_array());
                x[6 + k * 3..6 + k * 3 + 3].copy_from_slice(&v.to_array());
            }
            let hv = inv.transform_vector(velocity_of(f, rig.hips));
            x[12..15].copy_from_slice(&hv.to_array());
            for (k, &ahead) in FUTURE.iter().enumerate() {
                let g = f + ahead;
                let p = inv.transform_point(root(g).t);
                let d = inv.transform_vector(facing(root_yaw_at(g)));
                x[15 + k * 2] = p.x;
                x[16 + k * 2] = p.z;
                x[21 + k * 2] = d.x;
                x[22 + k * 2] = d.z;
            }
            raw.push(x);
            self.candidate.push(f + MIN_PLAY < frames);
            self.frame_range.push(range as u32);
            self.top_speed = self
                .top_speed
                .max(self.root_velocity.last().unwrap().length());
        }
        // Events, every time round (left and right swap in the mirror).
        let duration = clip.duration.max(1e-4);
        for k in 0..cycles {
            for e in &clip.events {
                let f =
                    ((e.time.clamp(0.0, duration) + k as f32 * duration) * FPS).round() as usize;
                if f >= frames {
                    continue;
                }
                let bone = e.bone.as_ref().map(|b| {
                    if mirrored {
                        let i = skeleton.index_of(b);
                        i.map_or(b.clone(), |i| skeleton.names[partner(skeleton, i)].clone())
                    } else {
                        b.clone()
                    }
                });
                self.events.push((start + f, e.name.clone(), bone));
            }
        }
        self.clip_names.push(label);
        self.ranges.push(Range {
            end: start + frames,
            clip: self.clip_names.len() - 1,
        });
    }

    fn normalize(&mut self, raw: &[[f32; DIM]]) {
        let count = raw.len().max(1) as f32;
        let mut mean = [0.0f32; DIM];
        for x in raw {
            for d in 0..DIM {
                mean[d] += x[d] / count;
            }
        }
        for &(first, len, weight) in &GROUPS {
            let mut var = 0.0;
            for x in raw {
                for d in first..first + len {
                    var += (x[d] - mean[d]).powi(2);
                }
            }
            let std = (var / (count * len as f32)).sqrt().max(1e-3);
            for d in first..first + len {
                self.offset[d] = mean[d];
                self.scale[d] = weight / std;
            }
        }
        self.features = raw
            .iter()
            .map(|x| {
                let mut y = [0.0; DIM];
                for d in 0..DIM {
                    y[d] = (x[d] - self.offset[d]) * self.scale[d];
                }
                y
            })
            .collect();
    }

    /// One line per move: frames, average pace, and how far it turns.
    pub fn summary(&self) -> Vec<String> {
        self.ranges
            .iter()
            .enumerate()
            .map(|(k, r)| {
                let start = if k == 0 { 0 } else { self.ranges[k - 1].end };
                let frames = r.end - start;
                let pace = self.root_velocity[start..r.end]
                    .iter()
                    .map(|v| v.length())
                    .sum::<f32>()
                    / frames as f32;
                let turn: f32 = self.root_turn[start..r.end].iter().sum::<f32>() / FPS;
                let ahead = self.root_velocity[start..r.end]
                    .iter()
                    .map(|v| -v.z)
                    .sum::<f32>()
                    / frames as f32;
                format!(
                    "{:40} {:4} frames  pace {:4.2} m/s (forward {:5.2})  turns {:+5.0}°",
                    self.clip_names[r.clip],
                    frames,
                    pace,
                    ahead,
                    turn.to_degrees()
                )
            })
            .collect()
    }

    pub fn frame_count(&self) -> usize {
        self.root_velocity.len()
    }

    pub fn bone_count(&self) -> usize {
        self.bones
    }

    /// Which clip a frame belongs to (its name, mirrored or not).
    pub fn clip_of(&self, frame: usize) -> &str {
        let r = self.ranges[self.frame_range[frame] as usize];
        &self.clip_names[r.clip]
    }

    /// Whether `frame + 1` follows `frame` in the same clip.
    fn continues(&self, frame: usize) -> bool {
        frame + 1 < self.frame_count() && self.frame_range[frame] == self.frame_range[frame + 1]
    }

    fn end_of(&self, frame: usize) -> usize {
        self.ranges[self.frame_range[frame] as usize].end
    }

    /// Pose of a frame.
    pub fn pose(&self, frame: usize) -> &[Xf] {
        &self.poses[frame * self.bones..(frame + 1) * self.bones]
    }

    /// Root velocity (root space, m/s) of a frame.
    pub fn root_velocity(&self, frame: usize) -> Vec3 {
        self.root_velocity[frame]
    }

    /// Normalize a query's trajectory part (raw root-space positions and directions) into the
    /// last 12 numbers of `query`.
    fn set_trajectory(&self, query: &mut [f32; DIM], trajectory: &[(Vec2, Vec2); 3]) {
        for (k, (p, d)) in trajectory.iter().enumerate() {
            let raw = [
                (15 + k * 2, p.x),
                (16 + k * 2, p.y),
                (21 + k * 2, d.x),
                (22 + k * 2, d.y),
            ];
            for (i, v) in raw {
                query[i] = (v - self.offset[i]) * self.scale[i];
            }
        }
    }

    /// The candidate frame closest to `query`, and its cost.
    pub fn search(&self, query: &[f32; DIM]) -> Option<(usize, f32)> {
        let mut best: Option<(usize, f32)> = None;
        for (i, f) in self.features.iter().enumerate() {
            if !self.candidate[i] {
                continue;
            }
            let limit = best.map_or(f32::INFINITY, |b| b.1);
            let mut cost = 0.0;
            for d in 0..DIM {
                cost += (f[d] - query[d]).powi(2);
                if cost >= limit {
                    break;
                }
            }
            if cost < limit {
                best = Some((i, cost));
            }
        }
        best
    }

    /// Where a frame's root will be (root space) 1/3, 2/3 and 1 s later, and which way it
    /// will face.
    pub fn trajectory(&self, frame: usize) -> [(Vec2, Vec2); 3] {
        let raw = |d: usize| self.features[frame][d] / self.scale[d] + self.offset[d];
        std::array::from_fn(|k| {
            (
                Vec2::new(raw(15 + k * 2), raw(16 + k * 2)),
                Vec2::new(raw(21 + k * 2), raw(22 + k * 2)),
            )
        })
    }

    /// First frame of the move named `name`.
    pub fn first_frame(&self, name: &str) -> Option<usize> {
        let k = self
            .ranges
            .iter()
            .position(|r| self.clip_names[r.clip] == name)?;
        Some(if k == 0 { 0 } else { self.ranges[k - 1].end })
    }

    /// The best landing frame of each move for these parameters, starting from `frame`'s
    /// pose, cheapest first: (move, frame, pose cost, trajectory cost).
    pub fn explain(
        &self,
        params: &AnimationParams,
        frame: usize,
    ) -> Vec<(String, usize, f32, f32)> {
        let mut query = self.features[frame];
        self.set_trajectory(&mut query, &MotionPlayer::predict(params));
        let mut best: Vec<(String, usize, f32, f32)> = Vec::new();
        for (k, r) in self.ranges.iter().enumerate() {
            let start = if k == 0 { 0 } else { self.ranges[k - 1].end };
            let pick = (start..r.end)
                .filter(|&i| self.candidate[i])
                .map(|i| {
                    let part = |a: usize, b: usize| -> f32 {
                        (a..b)
                            .map(|d| (self.features[i][d] - query[d]).powi(2))
                            .sum()
                    };
                    (i, part(0, 15), part(15, DIM))
                })
                .min_by(|a, b| (a.1 + a.2).total_cmp(&(b.1 + b.2)));
            if let Some((i, p, t)) = pick {
                best.push((self.clip_names[r.clip].clone(), i - start, p, t));
            }
        }
        best.sort_by(|a, b| (a.2 + a.3).total_cmp(&(b.2 + b.3)));
        best
    }

    fn cost(&self, frame: usize, query: &[f32; DIM]) -> f32 {
        self.features[frame]
            .iter()
            .zip(query)
            .map(|(a, b)| (a - b).powi(2))
            .sum()
    }
}

/// How often a character searches, seconds.
const SEARCH_SECS: f32 = 0.1;
/// A search only jumps when it beats carrying on by this much (cost units).
const SWITCH_MARGIN: f32 = 0.5;
/// Time constant of a change of pace in the predicted path, seconds.
const ACCEL_SECS: f32 = 0.3;
/// Half-life of the pose difference after a jump, seconds.
const BLEND_HALFLIFE: f32 = 0.1;
/// Travel more than this far from straight ahead (radians) uses the fallback (strafes,
/// backpedals: the database has forward motion only).
const FALLBACK_HEADING: f32 = 0.7;

/// The playing state of one character's motion matching (inside the graph node).
pub struct MotionPlayer {
    pub db: Arc<MotionDatabase>,
    /// The skeleton's hierarchy and the bones the features read (to measure a pose that
    /// didn't come from the database, such as the fallback's).
    parents: Arc<[Option<u16>]>,
    hips: usize,
    feet: [usize; 2],
    /// Frame playing (fractional), `None` before the first search.
    frame: Option<f32>,
    since_search: f32,
    /// Inertialization offsets per bone: translation and its velocity, rotation (scaled axis)
    /// and its velocity.
    offset_t: Vec<(Vec3, Vec3)>,
    offset_r: Vec<(Vec3, Vec3)>,
    /// How much the fallback node shows (0..1).
    pub fallback: f32,
    /// The last pose shown and the one before (for velocities at a jump), and the step
    /// between them.
    last: Vec<Xf>,
    before: Vec<Xf>,
    last_dt: f32,
}

impl MotionPlayer {
    pub fn new(db: Arc<MotionDatabase>, skeleton: &Skeleton) -> Result<Self, String> {
        let rig = Rig::of(skeleton).ok_or("motion matching needs hips, foot_l and foot_r")?;
        if skeleton.len() != db.bone_count() {
            return Err(format!(
                "motion database {} has {} bones, the skeleton {} has {}",
                db.name,
                db.bone_count(),
                skeleton.name,
                skeleton.len()
            ));
        }
        let n = db.bone_count();
        Ok(Self {
            db,
            parents: skeleton.parents.clone(),
            hips: rig.hips,
            feet: rig.feet,
            frame: None,
            since_search: f32::INFINITY,
            offset_t: vec![(Vec3::ZERO, Vec3::ZERO); n],
            offset_r: vec![(Vec3::ZERO, Vec3::ZERO); n],
            fallback: 0.0,
            last: Vec::new(),
            before: Vec::new(),
            last_dt: 0.0,
        })
    }

    pub fn reset(&mut self) {
        self.frame = None;
        self.since_search = f32::INFINITY;
        self.fallback = 0.0;
        for o in self.offset_t.iter_mut().chain(self.offset_r.iter_mut()) {
            *o = (Vec3::ZERO, Vec3::ZERO);
        }
        self.last.clear();
        self.before.clear();
    }

    /// Something else is showing (the fallback): remember its pose, so that coming back
    /// searches from it and blends out of it.
    pub fn hold(&mut self, shown: &[Xf], dt: f32) {
        self.frame = None;
        self.since_search = f32::INFINITY;
        if shown.len() == self.db.bone_count() {
            std::mem::swap(&mut self.before, &mut self.last);
            self.last.clear();
            self.last.extend_from_slice(shown);
            self.last_dt = dt;
        }
    }

    /// Model-space position of a bone in a local pose.
    fn model_position(&self, pose: &[Xf], bone: usize) -> Vec3 {
        let mut x = pose[bone];
        let mut b = bone;
        while let Some(p) = self.parents[b] {
            b = p as usize;
            x = pose[b].mul(&x);
        }
        x.t
    }

    /// The pose part of a query, measured from the poses shown (when no frame plays).
    fn measure(&self) -> Option<[f32; DIM]> {
        let n = self.db.bone_count();
        if self.last.len() != n || self.before.len() != n || self.last_dt <= 0.0 {
            return None;
        }
        let mut x = [0.0f32; DIM];
        let mut put = |at: usize, v: Vec3| x[at..at + 3].copy_from_slice(&v.to_array());
        for (k, &foot) in self.feet.iter().enumerate() {
            let p = self.model_position(&self.last, foot);
            let q = self.model_position(&self.before, foot);
            put(k * 3, p);
            put(6 + k * 3, (p - q) / self.last_dt);
        }
        let h = self.model_position(&self.last, self.hips)
            - self.model_position(&self.before, self.hips);
        put(12, h / self.last_dt);
        let mut query = [0.0; DIM];
        for d in 0..15 {
            query[d] = (x[d] - self.db.offset[d]) * self.db.scale[d];
        }
        Some(query)
    }

    /// Whether the motion asked for is outside the database (strafing, backpedalling).
    pub fn wants_fallback(params: &AnimationParams) -> bool {
        params.get("speed") > 0.4 && params.get("heading").abs() > FALLBACK_HEADING
    }

    /// The root's path over the next second, from the motion parameters: travel at `speed`
    /// toward `heading`, turning at the `turn` rate, speeding up by `accel`.
    pub fn predict(params: &AnimationParams) -> [(Vec2, Vec2); 3] {
        let speed = params.get("speed").max(0.0);
        let heading = params.get("heading");
        let yaw_rate = params.get("turn") * 1.2;
        let accel = params.get("accel") * crate::body::FULL_LEAN_ACCEL;
        let mut out = [(Vec2::ZERO, Vec2::new(0.0, -1.0)); 3];
        let steps = FUTURE[2];
        let dt = 1.0 / FPS;
        let (mut p, mut yaw, mut s) = (Vec3::ZERO, 0.0f32, speed);
        for f in 1..=steps {
            // Speeding up or slowing down eases off as the pace is reached (a gameplay
            // controller's spring toward its target speed, about this time constant).
            let t = f as f32 * dt;
            s = (s + accel * dt * (-t / ACCEL_SECS).exp()).max(0.0);
            yaw += yaw_rate * dt;
            p += facing(yaw + heading) * s * dt;
            if let Some(k) = FUTURE.iter().position(|&a| a == f) {
                let d = facing(yaw);
                out[k] = (Vec2::new(p.x, p.z), Vec2::new(d.x, d.z));
            }
        }
        out
    }

    /// Advance by `dt` and write the pose (local, in place) into `out`. Returns the events
    /// of the frames entered.
    pub fn update(
        &mut self,
        params: &AnimationParams,
        dt: f32,
        out: &mut [Xf],
        events: &mut Vec<(String, Option<String>)>,
    ) {
        let db = self.db.clone();
        let n = db.bone_count();
        let trajectory = Self::predict(params);
        // Carry on from the current frame, playing faster or slower to keep pace.
        let speed = params.get("speed");
        if let Some(frame) = self.frame.as_mut() {
            let f = *frame as usize;
            let clip_speed = db.root_velocity(f).length();
            let rate = if clip_speed > 0.4 && speed > 0.4 {
                (speed / clip_speed).clamp(0.8, 1.25)
            } else {
                1.0
            };
            let next = *frame + dt * FPS * rate;
            for k in f + 1..=(next as usize) {
                if k >= db.end_of(f) {
                    break;
                }
                events.extend(
                    db.events
                        .iter()
                        .filter(|e| e.0 == k)
                        .map(|e| (e.1.clone(), e.2.clone())),
                );
            }
            *frame = next;
        }
        self.since_search += dt;
        let at_end = self
            .frame
            .is_none_or(|f| f as usize + 1 >= db.end_of(f as usize) || !db.continues(f as usize));
        if self.since_search >= SEARCH_SECS || at_end {
            self.since_search = 0.0;
            // The query: the current frame's pose features, the wanted trajectory.
            let current = self
                .frame
                .map(|f| (f as usize).min(db.end_of(f as usize) - 1));
            // (No frame playing: measured from the poses shown, or else the database's mean.)
            let mut query = match current {
                Some(c) => db.features[c],
                None => self.measure().unwrap_or([0.0; DIM]),
            };
            db.set_trajectory(&mut query, &trajectory);
            if let Some((best, cost)) = db.search(&query) {
                let keep = current
                    .filter(|&c| db.candidate[c] && !at_end)
                    .map(|c| db.cost(c, &query));
                let jump = match keep {
                    None => true,
                    Some(k) => {
                        cost + SWITCH_MARGIN < k
                            && current.is_some_and(|c| {
                                best.abs_diff(c) > 3 || db.frame_range[best] != db.frame_range[c]
                            })
                    }
                };
                if jump {
                    self.jump_to(best, dt);
                }
            }
        }
        // Sample the frame (between two frames) and add the decaying offsets.
        let frame = self.frame.unwrap_or(0.0);
        let f0 = (frame as usize).min(db.frame_count() - 1);
        let f1 = if db.continues(f0) { f0 + 1 } else { f0 };
        let w = frame.fract();
        let (a, b) = (db.pose(f0), db.pose(f1));
        std::mem::swap(&mut self.before, &mut self.last);
        self.last.clear();
        for i in 0..n {
            let mut x = a[i].lerp(&b[i], w);
            let (ot, vt) = &mut self.offset_t[i];
            decay_spring(ot, vt, BLEND_HALFLIFE, dt);
            let (or, vr) = &mut self.offset_r[i];
            decay_spring(or, vr, BLEND_HALFLIFE, dt);
            x.t += *ot;
            x.r = (from_scaled_axis(*or) * x.r).normalize();
            self.last.push(x);
        }
        self.last_dt = dt;
        let m = n.min(out.len());
        out[..m].copy_from_slice(&self.last[..m]);
    }

    /// Root velocity (m/s, root space) of the frame playing.
    pub fn root_velocity(&self) -> Vec3 {
        self.frame.map_or(Vec3::ZERO, |f| {
            self.db
                .root_velocity((f as usize).min(self.db.frame_count() - 1))
        })
    }

    /// Jump to `frame`, carrying the pose difference (and its velocity) as offsets.
    fn jump_to(&mut self, frame: usize, dt: f32) {
        let db = self.db.clone();
        let n = db.bone_count();
        if self.last.len() == n {
            let dst = db.pose(frame);
            let next = db.pose(if db.continues(frame) {
                frame + 1
            } else {
                frame
            });
            let dt = if self.last_dt > 0.0 { self.last_dt } else { dt };
            let have_before = self.before.len() == n && dt > 0.0;
            for i in 0..n {
                let src = self.last[i];
                // Where the old motion was heading (its velocity) against the new one's.
                let (src_vt, src_vr) = if have_before {
                    let b = self.before[i];
                    (
                        (src.t - b.t) / dt,
                        to_scaled_axis(src.r * b.r.conjugate()) / dt,
                    )
                } else {
                    (Vec3::ZERO, Vec3::ZERO)
                };
                let dst_vt = (next[i].t - dst[i].t) * FPS;
                let dst_vr = to_scaled_axis(next[i].r * dst[i].r.conjugate()) * FPS;
                // The output already includes the old offset, so the new offset is the whole
                // difference between what shows and the new frame.
                self.offset_t[i] = (src.t - dst[i].t, src_vt - dst_vt);
                self.offset_r[i] = (
                    to_scaled_axis(src.r * dst[i].r.conjugate()),
                    src_vr - dst_vr,
                );
            }
        }
        self.frame = Some(frame as f32);
    }

    /// The frame playing.
    pub fn frame(&self) -> Option<usize> {
        self.frame.map(|f| f as usize)
    }
}
