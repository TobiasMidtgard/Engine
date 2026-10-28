//! The paddock's shuttle lane: a runner sprints down the lane, digs in to stop, runs back
//! backwards, turns on the spot and runs sideways back to the start, leaning into every
//! start and sitting back into every stop (the `accel` parameter); and a coach beside the
//! lane who turns where they stand to keep watching, stepping round instead of swivelling
//! on their feet.

use engine_core::prelude::*;
use engine_world::Ground;
use engine_world::animation::AnimationParams;
use engine_world::physics::PhysicsQueries;

use crate::body::accel_param;
use crate::build::humanoid;
use crate::ik::{GroundProbe, IkSurfaces, LookAt};
use crate::xform::smoothing;

/// One leg of a [`ShuttleRun`]: run to `to` (`[x, z]`, m from the origin) facing `facing_deg`
/// (yaw, degrees; 0 faces −Z) at up to `top` m/s.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct ShuttleLeg {
    pub to: [f32; 2],
    pub facing_deg: f32,
    pub top: f32,
}

/// What a [`ShuttleRun`] runner is doing.
#[derive(Debug, Clone, Copy, PartialEq, Default, Serialize, Deserialize)]
pub enum ShuttlePhase {
    /// Turning on the spot to face the next leg.
    #[default]
    Turn,
    /// Standing, ready to go (seconds left).
    Ready(f32),
    /// Running the leg (meters along it).
    Run(f32),
}

/// Runs a loop of straight legs from a standing start to a standing stop, turning on the spot
/// between them. Sets the `speed`, `turn`, `heading`, `move_effort` and `accel` animation
/// parameters.
#[derive(Component, Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ShuttleRun {
    pub origin: DVec3,
    /// Where the loop starts and ends, `[x, z]` m from the origin.
    pub start: [f32; 2],
    pub legs: Vec<ShuttleLeg>,
    /// How hard the runner speeds up and slows down, m/s².
    pub accel: f32,
    pub brake: f32,
    /// Fastest turn on the spot, rad/s.
    pub turn_rate: f32,
    /// How long the runner stands between legs, s.
    pub pause: f32,
    pub phase: ShuttlePhase,
    /// The leg being run (or turned to).
    pub leg: usize,
    #[serde(skip)]
    pub(crate) speed: f32,
    #[serde(skip)]
    pub(crate) yaw: Option<f32>,
    #[serde(skip)]
    pub(crate) yaw_rate: f32,
    #[serde(skip)]
    pub(crate) accel_now: f32,
}

impl Default for ShuttleRun {
    fn default() -> Self {
        Self {
            origin: DVec3::ZERO,
            start: [0.0; 2],
            legs: Vec::new(),
            accel: 4.5,
            brake: 6.5,
            turn_rate: 2.4,
            pause: 0.7,
            phase: ShuttlePhase::Turn,
            leg: 0,
            speed: 0.0,
            yaw: None,
            yaw_rate: 0.0,
            accel_now: 0.0,
        }
    }
}

impl ShuttleRun {
    /// Where leg `i` starts, `[x, z]`.
    fn from(&self, i: usize) -> Vec2 {
        let n = self.legs.len();
        if i == 0 {
            Vec2::from(self.start)
        } else {
            Vec2::from(self.legs[(i - 1) % n].to)
        }
    }

    /// Advance by `dt`: returns where the runner stands (`[x, z]` from the origin), its yaw
    /// and its velocity (`[x, z]`, m/s).
    pub fn step(&mut self, dt: f32) -> (Vec2, f32, Vec2) {
        let n = self.legs.len();
        if n == 0 {
            return (Vec2::from(self.start), 0.0, Vec2::ZERO);
        }
        self.leg %= n;
        let leg = self.legs[self.leg];
        let from = self.from(self.leg);
        let to = Vec2::from(leg.to);
        let along_dir = (to - from).normalize_or_zero();
        let length = (to - from).length();
        let face = leg.facing_deg.to_radians();
        let yaw = *self.yaw.get_or_insert(face);
        let mut new_yaw = yaw;
        let mut at = from;
        let mut velocity = Vec2::ZERO;
        match self.phase {
            ShuttlePhase::Turn => {
                let off = wrap(face - yaw);
                // Ease in and out of the turn rather than snapping round.
                let rate = self.turn_rate.min(off.abs() * 3.0 + 0.3);
                let turn = off.signum() * (rate * dt).min(off.abs());
                new_yaw = wrap(yaw + turn);
                if wrap(face - new_yaw).abs() < 1e-3 {
                    new_yaw = face;
                    self.phase = ShuttlePhase::Ready(self.pause);
                }
            }
            ShuttlePhase::Ready(left) => {
                let left = left - dt;
                self.phase = if left <= 0.0 {
                    ShuttlePhase::Run(0.0)
                } else {
                    ShuttlePhase::Ready(left)
                };
            }
            ShuttlePhase::Run(done) => {
                // Speed up to the top and brake to stop exactly at the end.
                let rest = (length - done).max(0.0);
                let stopping = (2.0 * self.brake * rest).sqrt();
                self.speed = (self.speed + self.accel * dt).min(leg.top).min(stopping);
                let done = (done + self.speed * dt).min(length);
                at = from + along_dir * done;
                velocity = along_dir * self.speed;
                if length - done < 0.005 || (self.speed < 0.05 && rest < 0.05) {
                    at = to;
                    velocity = Vec2::ZERO;
                    self.speed = 0.0;
                    self.leg = (self.leg + 1) % n;
                    self.phase = ShuttlePhase::Turn;
                } else {
                    self.phase = ShuttlePhase::Run(done);
                }
            }
        }
        if dt > 0.0 {
            self.yaw_rate += (wrap(new_yaw - yaw) / dt - self.yaw_rate) * smoothing(dt, 0.15);
        }
        self.yaw = Some(new_yaw);
        (at, new_yaw, velocity)
    }

    /// Which way the current leg runs relative to the runner's facing, radians (positive
    /// left, ±π backwards).
    fn heading(&self) -> f32 {
        let n = self.legs.len();
        if n == 0 {
            return 0.0;
        }
        let leg = self.legs[self.leg % n];
        let d = Vec2::from(leg.to) - self.from(self.leg % n);
        let along = (-d.x).atan2(-d.y);
        wrap(along - leg.facing_deg.to_radians())
    }
}

fn wrap(a: f32) -> f32 {
    use std::f32::consts::{PI, TAU};
    (a + PI).rem_euclid(TAU) - PI
}

/// Turns where they stand to keep watching `target`: the head follows at once, the feet only
/// once it has gone well off to one side.
#[derive(Component, Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Watch {
    #[serde(skip)]
    pub target: Option<Entity>,
    /// How far off (radians) the target goes before the feet step round, and how close they
    /// bring it back.
    pub start_turn: f32,
    pub stop_turn: f32,
    /// Fastest turn, rad/s.
    pub turn_rate: f32,
    /// Height on the target to look at, m.
    pub look_height: f32,
    #[serde(skip)]
    pub(crate) turning: bool,
    #[serde(skip)]
    pub(crate) yaw_rate: f32,
}

impl Default for Watch {
    fn default() -> Self {
        Self {
            target: None,
            start_turn: 0.6,
            stop_turn: 0.1,
            turn_rate: 1.6,
            look_height: 1.45,
            turning: false,
            yaw_rate: 0.0,
        }
    }
}

#[allow(clippy::type_complexity)]
pub(crate) fn shuttle_system(
    time: Res<Time>,
    ground: Res<Ground>,
    physics: Option<Res<PhysicsQueries>>,
    surfaces: Res<IkSurfaces>,
    mut q: Query<(
        Entity,
        &mut ShuttleRun,
        &mut Transform,
        Option<&mut AnimationParams>,
        Option<&LocalTimeScale>,
    )>,
) {
    for (entity, mut shuttle, mut t, params, local) in &mut q {
        let dt = time.delta_for(local);
        let was = shuttle.speed;
        let (at, yaw, velocity) = shuttle.step(dt);
        let probe = GroundProbe {
            ground: &ground,
            physics: physics.as_deref(),
            surfaces: &surfaces.0,
            exclude: Some(entity),
        };
        let mut pos = shuttle.origin + DVec3::new(at.x as f64, 0.0, at.y as f64);
        pos.y = probe
            .probe(DVec3::new(pos.x, t.translation.y + 0.3, pos.z), 0.7, 3.0)
            .map_or(shuttle.origin.y, |h| h.height);
        t.translation = pos;
        t.rotation = Quat::from_rotation_y(yaw);
        if dt > 0.0 {
            // Speeding up along the leg, as seen from where the runner faces.
            let forward = Vec2::new(-yaw.sin(), -yaw.cos());
            let dir = velocity.normalize_or_zero();
            let gain = (shuttle.speed - was) / dt * dir.dot(forward);
            shuttle.accel_now += (gain - shuttle.accel_now) * smoothing(dt, 0.1);
        }
        if let Some(mut params) = params {
            params.set("speed", shuttle.speed);
            params.set("turn", (shuttle.yaw_rate / 1.2).clamp(-1.0, 1.0));
            params.set("heading", shuttle.heading());
            params.set(
                "move_effort",
                (shuttle.speed / humanoid::RUN_SPEED).clamp(0.0, 1.0),
            );
            params.set("accel", accel_param(shuttle.accel_now));
        }
    }
}

#[allow(clippy::type_complexity)]
pub(crate) fn watch_system(
    time: Res<Time>,
    targets: Query<&GlobalTransform, Without<Watch>>,
    mut q: Query<(
        &mut Watch,
        &mut Transform,
        Option<&mut LookAt>,
        Option<&mut AnimationParams>,
        Option<&LocalTimeScale>,
    )>,
) {
    for (mut watch, mut t, look, params, local) in &mut q {
        let dt = time.delta_for(local);
        let Some(target) = watch.target.and_then(|e| targets.get(e).ok()) else {
            continue;
        };
        let to = (target.translation() - t.translation).as_vec3().with_y(0.0);
        if to.length() < 0.3 || dt <= 0.0 {
            continue;
        }
        let f = t.rotation * Vec3::NEG_Z;
        let yaw = (-f.x).atan2(-f.z);
        let off = wrap((-to.x).atan2(-to.z) - yaw);
        if off.abs() > watch.start_turn {
            watch.turning = true;
        } else if off.abs() < watch.stop_turn {
            watch.turning = false;
        }
        let want = if watch.turning {
            off.signum() * watch.turn_rate.min(off.abs() * 2.5 + 0.2)
        } else {
            0.0
        };
        watch.yaw_rate += (want - watch.yaw_rate) * smoothing(dt, 0.2);
        let turn = watch.yaw_rate * dt;
        t.rotation = Quat::from_rotation_y(yaw + turn);
        if let Some(mut look) = look {
            look.target = target.translation() + DVec3::Y * watch.look_height as f64;
        }
        if let Some(mut params) = params {
            params.set("speed", 0.0);
            params.set("turn", (watch.yaw_rate / 1.2).clamp(-1.0, 1.0));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lane() -> ShuttleRun {
        ShuttleRun {
            start: [0.0, 7.0],
            legs: vec![
                ShuttleLeg {
                    to: [0.0, -8.0],
                    facing_deg: 0.0,
                    top: 5.0,
                },
                ShuttleLeg {
                    to: [0.0, 0.0],
                    facing_deg: 0.0,
                    top: 2.6,
                },
                ShuttleLeg {
                    to: [0.0, 7.0],
                    facing_deg: -90.0,
                    top: 3.0,
                },
            ],
            ..Default::default()
        }
    }

    #[test]
    fn runs_the_legs_from_standing_stops_and_comes_back() {
        let mut s = lane();
        let dt = 1.0 / 60.0;
        let mut top = 0.0f32;
        let mut legs_seen = vec![];
        let (mut last, mut far) = (Vec2::new(0.0, 7.0), 0.0f32);
        for _ in 0..(40.0 / dt) as usize {
            let before = s.speed;
            let (at, _, v) = s.step(dt);
            top = top.max(v.length());
            // Never jumps: it moves no faster than it runs (but for settling the last few mm).
            let most = before.max(s.speed) + s.accel * dt;
            assert!(
                (at - last).length() <= most * dt + 0.006,
                "jumped to {at} from {last}"
            );
            last = at;
            far = far.min(at.y);
            if legs_seen.last() != Some(&s.leg) {
                legs_seen.push(s.leg);
            }
        }
        assert!((4.9..=5.0).contains(&top), "top speed {top}");
        assert!((far + 8.0).abs() < 0.01, "got to {far}");
        // Went round more than once.
        assert!(legs_seen.len() > 4, "legs {legs_seen:?}");
    }

    #[test]
    fn faces_each_leg_and_heads_the_right_way() {
        let mut s = lane();
        s.leg = 1;
        assert!((s.heading().abs() - std::f32::consts::PI).abs() < 1e-3);
        s.leg = 2;
        // Facing +X and running towards +Z: to its right.
        assert!((s.heading() + std::f32::consts::FRAC_PI_2).abs() < 1e-3);
    }
}
