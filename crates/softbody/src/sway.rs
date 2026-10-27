//! Far flags play back their own flapping instead of being simulated.
//!
//! Far from the camera a flag streaming in the wind is a few dozen pixels, but stepped it
//! costs as much as near; frozen, it reads as broken. Engines move far cloth and foliage
//! without simulating it: SpeedTree and Unreal's foliage wind in the vertex stage, Chaos
//! Cloth's LOD handing far cloth to an animated pose, and baked cloth caches (Alembic, the
//! "vertex animation textures" of Houdini → Unreal) looped for background flags. Here a far
//! flag records a few seconds of its own simulated flapping (stepping every fixed step
//! meanwhile, so the folds stay sharp), finds a stretch that comes back to where it began,
//! and loops it with a short crossfade over the seam, at a blend of two poses a particle.
//! Gusts play it faster or slower. It is simulated again when it comes near, tears, or when
//! the wind it recorded in turns or changes strength.

use engine_core::prelude::*;

use crate::body::{SoftBodySim, SoftKind};

/// Fixed steps it records before playing back (4 s at 60 Hz: several flaps).
pub const LEARN_STEPS: u32 = 240;
/// Wind it plays back in, m/s: in less it hangs, and sleeps instead.
pub const MIN_WIND: f32 = 1.5;
/// How far the wind (smoothed over about a second) may turn from the mean it recorded in,
/// radians, and by what factor its speed may change (gusts are ±30% in the weather's
/// default), before it is simulated again. Between, the loop plays faster or slower.
const WIND_TURN: f32 = 0.45;
const WIND_CHANGE: f32 = 1.6;
/// Crossfade over the loop's seam, and the shortest loop, seconds.
const FADE: f32 = 0.3;
const MIN_LOOP: f32 = 1.2;

#[derive(Debug, Clone)]
pub enum FarSway {
    Learning(Box<Recording>),
    Swaying(Box<Swaying>),
}

/// Poses as it stepped, and when (seconds since recording began).
#[derive(Debug, Clone, Default)]
pub struct Recording {
    poses: Vec<Vec<Vec3>>,
    times: Vec<f32>,
    clock: f32,
    steps: u32,
    wind_sum: Vec3,
    topology: u64,
}

#[derive(Debug, Clone)]
pub struct Swaying {
    take: Recording,
    /// The loop: from `start` for `len` seconds; its last `FADE` blends into the time before
    /// `start`.
    start: f32,
    len: f32,
    time: f32,
    wind: Vec3,
    smoothed: Vec3,
}

/// Whether `b` can sway when far: plain pinned cloth, nothing carrying, burning or soaking
/// it, in enough wind.
pub fn can_sway(b: &SoftBodySim, wind: Vec3) -> bool {
    b.kind == SoftKind::Cloth
        && b.sim.params.self_collide <= 0.0
        && b.strands.is_none()
        && b.bridge.is_none()
        && !b.carried
        && !b.sim.pins.is_empty()
        && b.sim.loads.is_empty()
        && b.sim.water.is_none()
        && b.cloth.as_ref().is_none_or(|c| c.burning == 0)
        && wind.length() > MIN_WIND
}

/// Record one more fixed step of `dt` (its particles where the last step left them). True
/// once it has recorded enough and found a loop in it.
pub fn learn(b: &mut SoftBodySim, wind: Vec3, dt: f32) -> bool {
    if !matches!(&b.far, Some(FarSway::Learning(r)) if r.topology == b.topology_version) {
        b.far = Some(FarSway::Learning(Box::new(Recording {
            topology: b.topology_version,
            ..Recording::default()
        })));
    }
    let Some(FarSway::Learning(r)) = &mut b.far else {
        unreachable!()
    };
    // A far body steps every few fixed steps: keep a pose when it stepped.
    if r.poses.last() != Some(&b.sim.pos) {
        r.poses.push(b.sim.pos.clone());
        r.times.push(r.clock);
    }
    r.clock += dt;
    r.steps += 1;
    r.wind_sum += wind;
    if r.steps < LEARN_STEPS {
        return false;
    }
    let Some(FarSway::Learning(r)) = b.far.take() else {
        unreachable!()
    };
    match find_loop(*r) {
        Some(s) => {
            b.far = Some(FarSway::Swaying(Box::new(s)));
            true
        }
        // Too few poses to loop: record again.
        None => false,
    }
}

/// The stretch after `FADE` whose end comes closest to its start.
fn find_loop(take: Recording) -> Option<Swaying> {
    let first = take.times.iter().position(|t| *t >= FADE)?;
    let start = take.times[first];
    let a = &take.poses[first];
    let stride = (a.len() / 64).max(1);
    let (end, _) = take
        .times
        .iter()
        .enumerate()
        .skip(first + 1)
        .filter(|(_, t)| **t - start >= MIN_LOOP)
        .map(|(i, _)| {
            let d: f32 = a
                .iter()
                .zip(&take.poses[i])
                .step_by(stride)
                .map(|(p, q)| p.distance_squared(*q))
                .sum();
            (i, d)
        })
        .min_by(|x, y| x.1.total_cmp(&y.1))?;
    let len = take.times[end] - start;
    let wind = take.wind_sum / take.steps as f32;
    Some(Swaying {
        take,
        start,
        len,
        time: 0.0,
        wind,
        smoothed: wind,
    })
}

impl Recording {
    /// Add the pose at `t` seconds (between the poses either side), times `weight`.
    fn add_pose(&self, t: f32, out: &mut [Vec3], weight: f32) {
        let k = self
            .times
            .partition_point(|x| *x <= t)
            .clamp(1, self.times.len() - 1);
        let (t0, t1) = (self.times[k - 1], self.times[k]);
        let u = ((t - t0) / (t1 - t0).max(1e-6)).clamp(0.0, 1.0);
        let (a, b) = (&self.poses[k - 1], &self.poses[k]);
        for ((o, p), q) in out.iter_mut().zip(a).zip(b) {
            *o += p.lerp(*q, u) * weight;
        }
    }
}

/// Play one fixed step of `dt`. False when it should be simulated again (the wind changed,
/// it tore).
pub fn sway(b: &mut SoftBodySim, wind: Vec3, dt: f32) -> bool {
    let Some(FarSway::Swaying(s)) = &mut b.far else {
        return false;
    };
    s.smoothed += (wind - s.smoothed) * dt.min(1.0);
    let ratio = s.smoothed.length() / s.wind.length().max(1e-3);
    let changed = s.take.topology != b.topology_version
        || s.smoothed.angle_between(s.wind) > WIND_TURN
        || !(1.0 / WIND_CHANGE..WIND_CHANGE).contains(&ratio);
    if changed {
        stop(b);
        return false;
    }
    let Some(FarSway::Swaying(s)) = &mut b.far else {
        return false;
    };
    s.time = (s.time + dt * ratio) % s.len;
    let sim = &mut b.sim;
    sim.step_start.clone_from(&sim.pos);
    sim.pos.fill(Vec3::ZERO);
    let t = s.start + s.time;
    // Over the seam, fade into what led up to the loop's start.
    let fade = ((s.time - (s.len - FADE)) / FADE).clamp(0.0, 1.0);
    let fade = fade * fade * (3.0 - 2.0 * fade);
    s.take.add_pose(t, &mut sim.pos, 1.0 - fade);
    if fade > 0.0 {
        s.take.add_pose(t - s.len, &mut sim.pos, fade);
    }
    true
}

/// Back to being simulated: from where it swayed to, at rest.
pub fn stop(b: &mut SoftBodySim) {
    if matches!(b.far.take(), Some(FarSway::Swaying(_))) {
        b.sim.rest_here();
    }
}
