//! The paddock's wall-run course: two free-standing walls either side of a lane, staggered so
//! one run carries onto the other. A runner (a person on a [`BodyRequest`], dressed as the
//! athlete) sprints down the lane, jumps onto the left wall and runs along it, kicks across
//! onto the right wall, runs that until it slows, drops off and lands, then turns and jogs
//! back to start again. It writes its [`LocomotionState`] the way a movement module does
//! (mode, velocity, on the ground or not) and nothing else, so the body answers the same
//! contract it does in a game: [`crate::parkour`] finds each wall with its rays.

use engine_core::prelude::*;
use engine_render::{MeshRenderer, Shape as MeshShape, StandardMaterial};
use engine_world::animation::BodyRequest;
use engine_world::physics::{Collider, RigidBody};
use engine_world::{LocomotionMode, LocomotionState};

use crate::ik::{IkShape, IkSurface};

/// Where the runner starts, facing +X down the lane (paddock frame, m).
pub const START: [f32; 2] = [-4.0, 24.0];
/// The walls' faces (z) and spans along the lane (x from, x to), m.
pub const WALL_LEFT: (f32, [f32; 2]) = (21.5, [2.0, 9.0]);
pub const WALL_RIGHT: (f32, [f32; 2]) = (26.5, [9.5, 17.0]);
const WALL_HEIGHT: f32 = 3.4;
const WALL_THICKNESS: f32 = 0.3;
/// Where the runner's axis keeps from a wall it runs on (the clips' distance), m.
const OFF_WALL: f32 = crate::build::humanoid::WALL_DISTANCE;
const G: f32 = 9.81;
/// Samples a second along the run.
const RATE: f32 = 120.0;

/// One moment of the run, relative to the start.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CourseSample {
    pub position: Vec3,
    pub velocity: Vec3,
    /// Facing, radians about +Y (0 faces −Z).
    pub yaw: f32,
    pub mode: LocomotionMode,
    pub grounded: bool,
}

/// A runner on the course (published): its run, sampled, and how far along it is.
#[derive(Component, Debug, Clone)]
pub struct CourseRunner {
    pub origin: DVec3,
    pub samples: Vec<CourseSample>,
    pub secs: f32,
}

impl CourseRunner {
    /// Seconds round the course.
    pub fn period(&self) -> f32 {
        self.samples.len() as f32 / RATE
    }

    /// Where the run is `t` seconds in (looping).
    pub fn at(&self, t: f32) -> CourseSample {
        let n = self.samples.len();
        let f = (t.rem_euclid(self.period()) * RATE).min(n as f32 - 1.0);
        let i = f as usize;
        let (a, b) = (self.samples[i], self.samples[(i + 1).min(n - 1)]);
        let w = f.fract();
        CourseSample {
            position: a.position.lerp(b.position, w),
            velocity: a.velocity.lerp(b.velocity, w),
            ..a
        }
    }
}

/// Builds the run step by step.
struct Run {
    p: Vec3,
    v: Vec3,
    yaw: f32,
    out: Vec<CourseSample>,
}

fn yaw_of(dir: Vec3) -> f32 {
    (-dir.x).atan2(-dir.z)
}

impl Run {
    fn push(&mut self, mode: LocomotionMode, grounded: bool) {
        self.out.push(CourseSample {
            position: self.p,
            velocity: self.v,
            yaw: self.yaw,
            mode,
            grounded,
        });
    }

    fn ground_mode(speed: f32) -> LocomotionMode {
        if speed < 0.2 {
            LocomotionMode::Idle
        } else if speed < 2.5 {
            LocomotionMode::Walk
        } else if speed < 5.5 {
            LocomotionMode::Run
        } else {
            LocomotionMode::Sprint
        }
    }

    /// Standing (or turning on the spot to `yaw`) for `secs`.
    fn stand(&mut self, secs: f32, yaw: f32) {
        let n = (secs * RATE) as usize;
        let from = self.yaw;
        let turn = (yaw - from + std::f32::consts::PI).rem_euclid(std::f32::consts::TAU)
            - std::f32::consts::PI;
        self.v = Vec3::ZERO;
        for k in 0..n {
            let u = (k + 1) as f32 / n as f32;
            self.yaw = from + turn * u * u * (3.0 - 2.0 * u);
            self.push(LocomotionMode::Idle, true);
        }
    }

    /// Along the ground to `to` (y 0), speeding up toward `cruise` at `accel` and slowing to
    /// `end` by the time it gets there; facing where it goes unless `facing` is given.
    fn ground_to(&mut self, to: Vec3, cruise: f32, end: f32, accel: f32, facing: Option<f32>) {
        let dt = 1.0 / RATE;
        let mut speed = self.v.with_y(0.0).length();
        self.p.y = 0.0;
        loop {
            let off = (to - self.p).with_y(0.0);
            let left = off.length();
            if left < 1e-3 || (speed * dt >= left && speed > 0.0) {
                self.p = to;
                self.v = off.normalize_or_zero() * end;
                break;
            }
            let brake = (speed * speed - end * end) / (2.0 * accel);
            speed = if left <= brake {
                (speed - accel * dt).max(end.max(0.3))
            } else {
                (speed + accel * dt).min(cruise)
            };
            let dir = off / left;
            self.v = dir * speed;
            self.p += self.v * dt;
            self.yaw = facing.unwrap_or_else(|| yaw_of(dir));
            self.push(Self::ground_mode(speed), true);
        }
    }

    /// Through the air to `to` in `secs` under gravity scaled by `gravity`, in `mode`.
    fn fly_to(&mut self, to: Vec3, secs: f32, gravity: f32, mode: LocomotionMode) {
        let g = G * gravity;
        let from = self.p;
        let flat = (to - from).with_y(0.0) / secs;
        let vy = (to.y - from.y + 0.5 * g * secs * secs) / secs;
        let n = (secs * RATE).round() as usize;
        for k in 1..=n {
            let t = k as f32 / RATE;
            self.p = from + flat * t + Vec3::Y * (vy * t - 0.5 * g * t * t);
            self.v = flat + Vec3::Y * (vy - g * t);
            self.push(mode, false);
        }
        self.p = to;
    }
}

/// The run: start, sprint, onto the left wall, across onto the right one, down, back.
pub fn course_samples() -> Vec<CourseSample> {
    let start = Vec3::new(START[0], 0.0, START[1]);
    let ahead = yaw_of(Vec3::X);
    let mut r = Run {
        p: start,
        v: Vec3::ZERO,
        yaw: ahead,
        out: Vec::new(),
    };
    let lane = START[1];
    let on_left = WALL_LEFT.0 + OFF_WALL;
    let on_right = WALL_RIGHT.0 - OFF_WALL;
    r.stand(0.6, ahead);
    // Sprint up to 7 m/s and take off at x 2.3.
    r.ground_to(Vec3::new(2.3, 0.0, lane), 7.0, 7.0, 7.0, Some(ahead));
    r.fly_to(
        Vec3::new(4.8, 0.95, on_left),
        0.36,
        1.0,
        LocomotionMode::Air,
    );
    // Along the left wall, light on it at first (a movement module's lower gravity there).
    r.fly_to(
        Vec3::new(8.65, 1.05, on_left),
        0.55,
        0.3,
        LocomotionMode::WallRun,
    );
    // Kicked off across the lane onto the right wall.
    r.fly_to(
        Vec3::new(13.0, 1.15, on_right),
        0.62,
        1.0,
        LocomotionMode::Air,
    );
    // Along the right wall, slowing and sinking.
    r.fly_to(
        Vec3::new(15.75, 0.8, on_right),
        0.5,
        0.45,
        LocomotionMode::WallRun,
    );
    // Dropping off onto the ground and landing.
    r.fly_to(Vec3::new(17.35, 0.0, 25.3), 0.5, 1.0, LocomotionMode::Air);
    r.v = Vec3::new(3.2, 0.0, -1.3);
    r.ground_to(Vec3::new(17.9, 0.0, 25.1), 3.2, 0.0, 8.0, Some(ahead));
    r.stand(0.6, ahead);
    // Turn round and jog back.
    r.stand(0.7, yaw_of(Vec3::NEG_X));
    r.ground_to(start, 3.5, 0.0, 3.0, None);
    r.stand(0.7, ahead);
    r.stand(0.4, ahead);
    r.out
}

/// Build the course and its runner at `origin` (the paddock's).
pub fn spawn_course(world: &mut World, origin: DVec3) -> Entity {
    let concrete = StandardMaterial::hex(0x9c978e).with_roughness(0.85);
    for (face, [x0, x1], side) in [
        (WALL_LEFT.0, WALL_LEFT.1, -1.0f32),
        (WALL_RIGHT.0, WALL_RIGHT.1, 1.0),
    ] {
        let len = x1 - x0;
        let center = Vec3::new(
            (x0 + x1) * 0.5,
            WALL_HEIGHT * 0.5,
            face + side * WALL_THICKNESS * 0.5,
        );
        let t = Transform::from_translation(origin + center.as_dvec3());
        world.spawn((
            Name::new("Wall-run wall"),
            t,
            GlobalTransform::from(t),
            IkSurface {
                shape: IkShape::Box,
                half_extents: [len * 0.5, WALL_HEIGHT * 0.5, WALL_THICKNESS * 0.5],
                surface: 3,
            },
            RigidBody::fixed(),
            Collider::cuboid(Vec3::new(
                len * 0.5,
                WALL_HEIGHT * 0.5,
                WALL_THICKNESS * 0.5,
            )),
            MeshRenderer::new(
                MeshShape::Cuboid {
                    size: [len, WALL_HEIGHT, WALL_THICKNESS],
                },
                concrete,
            ),
        ));
    }
    let samples = course_samples();
    let first = samples[0];
    let t = Transform::from_translation(origin + first.position.as_dvec3())
        .with_rotation(Quat::from_rotation_y(first.yaw));
    world
        .spawn((
            Name::new("Wall runner"),
            t,
            GlobalTransform::from(t),
            BodyRequest::human(5).with_model("anim/characters/athlete.glb"),
            LocomotionState::default(),
            CourseRunner {
                origin,
                samples,
                secs: 0.0,
            },
        ))
        .id()
}

/// Move each runner along its course and report its state.
pub(crate) fn run_courses(
    time: Res<Time>,
    mut q: Query<(
        &mut CourseRunner,
        &mut Transform,
        &mut LocomotionState,
        Option<&LocalTimeScale>,
    )>,
) {
    for (mut runner, mut t, mut state, local) in &mut q {
        runner.secs += time.delta_for(local);
        let s = runner.at(runner.secs);
        t.translation = runner.origin + s.position.as_dvec3();
        t.rotation = Quat::from_rotation_y(s.yaw);
        state.mode = s.mode;
        state.velocity = s.velocity;
        state.grounded = s.grounded;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_run_keeps_the_clips_distance_off_each_wall_and_comes_back_to_start() {
        let samples = course_samples();
        let on_wall: Vec<&CourseSample> = samples
            .iter()
            .filter(|s| s.mode == LocomotionMode::WallRun)
            .collect();
        assert!(on_wall.len() > 100, "{}", on_wall.len());
        let (mut left, mut right) = (0, 0);
        for s in on_wall {
            let p = s.position;
            if p.z < START[1] {
                left += 1;
                assert!((p.z - (WALL_LEFT.0 + OFF_WALL)).abs() < 1e-3);
                assert!(p.x > WALL_LEFT.1[0] && p.x < WALL_LEFT.1[1], "{p}");
            } else {
                right += 1;
                assert!((p.z - (WALL_RIGHT.0 - OFF_WALL)).abs() < 1e-3);
                assert!(p.x > WALL_RIGHT.1[0] && p.x < WALL_RIGHT.1[1], "{p}");
            }
            assert!(!s.grounded);
            assert!(p.y > 0.4 && p.y < 1.8, "{p}");
        }
        assert!(left > 40 && right > 40);
        // Sprinting at take-off, and back at the start (looping) at the end.
        let top = samples
            .iter()
            .map(|s| s.velocity.length())
            .fold(0.0, f32::max);
        assert!(top > 6.5, "{top}");
        let (a, b) = (samples[0].position, samples.last().unwrap().position);
        assert!(a.distance(b) < 0.05, "{a} {b}");
        // Nothing passes through either wall's slab.
        for s in &samples {
            for (face, [x0, x1], side) in [
                (WALL_LEFT.0, WALL_LEFT.1, -1.0f32),
                (WALL_RIGHT.0, WALL_RIGHT.1, 1.0),
            ] {
                let inside = (s.position.z - face) * side;
                if s.position.x > x0 && s.position.x < x1 {
                    assert!(inside < -0.2, "{} inside a wall", s.position);
                }
            }
        }
    }
}
