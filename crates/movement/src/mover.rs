//! [`Mover`]: the runtime state of one moving character, written by the movement systems
//! every fixed step and readable by anyone (AI, cameras, UI, the AI bridge).

use engine_core::math::{DVec3, Quat, Vec3};
use engine_core::prelude::*;
use engine_world::LocomotionMode;
use engine_world::animation::AnimationParams;
use engine_world::locomotion::{LocomotionState, MoveIntent};
use engine_world::physics::{CharacterController, CharacterMove, CharacterState};

use crate::platform::Platform;

/// A wall the character touches or runs on.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct WallContact {
    /// Unit normal pointing out of the wall.
    pub normal: Vec3,
    /// Where the wall was found.
    pub point: DVec3,
    #[serde(skip)]
    pub entity: Option<Entity>,
}

/// A dash in progress.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Dash {
    /// Unit direction.
    pub dir: Vec3,
    /// m/s, own time.
    pub speed: f32,
    /// Seconds (own time) left.
    pub secs_left: f32,
    /// Part of a dash-strike: no dodge frames, stops dead to swing.
    pub strike: bool,
    /// Speed kept when the dash ends, m/s.
    pub exit_speed: f32,
}

/// A mantle (climbing onto a ledge) in progress.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Mantle {
    /// The top of the ledge being climbed.
    pub top: DVec3,
    /// Horizontal direction onto the ledge.
    pub forward: Vec3,
    /// Still rising (false = stepping over the edge).
    pub rising: bool,
    /// Horizontal speed to leave with, m/s.
    pub exit_speed: f32,
}

/// Ask the movement module to dash. Combat uses this for dash-strikes; gameplay can use it
/// for scripted dashes. Consumed on the next fixed step.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct DashRequest {
    pub to: DVec3,
    /// A dash-strike: stop at striking distance and plant.
    pub strike: bool,
    /// Longest dash allowed, m (`None` = the profile's `dash_distance`).
    pub max_distance: Option<f32>,
}

/// How combat bends movement this step (written by the combat systems).
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub(crate) struct Influence {
    /// Multiplier on the stick (attacking, blocking).
    pub move_scale: f32,
    /// Forced horizontal velocity on the ground (attack lunge), m/s.
    pub lunge: Option<Vec3>,
    /// Face this way (toward the attack target).
    pub face: Option<Vec3>,
}

/// Runtime movement state. Added automatically with a
/// [`MovementProfile`](crate::MovementProfile); read it, don't write it (except `disabled`).
///
/// Requires the locomotion and physics contract components, so spawning
/// `(Transform, MovementProfile::pilot())` is a complete character (add a
/// `CharacterController` to change the capsule).
#[derive(Component, Debug, Clone, PartialEq, Serialize, Deserialize)]
#[require(
    MoveIntent,
    LocomotionState,
    CharacterMove,
    CharacterState,
    CharacterController,
    AnimationParams,
    TransformInterpolation
)]
#[serde(default)]
pub struct Mover {
    pub mode: LocomotionMode,
    /// Velocity in the character's own time, m/s.
    pub velocity: Vec3,
    /// Horizontal facing, unit.
    pub facing: Vec3,
    pub grounded: bool,
    pub ground_normal: Vec3,
    /// Local up (opposite gravity) at the character, updated every step.
    pub up: Vec3,
    /// Seconds (own time) in the current mode.
    pub mode_secs: f32,
    /// Seconds since leaving the ground.
    pub air_secs: f32,
    pub air_jumps_left: u32,
    /// Dashes available (fractional while recharging).
    pub dash_charges: f32,
    /// The wall being run on.
    pub wall: Option<WallContact>,
    pub dash: Option<Dash>,
    pub mantle: Option<Mantle>,
    /// The capsule is shrunk (crouching or sliding).
    pub crouched: bool,
    /// Remaining dodge invulnerability, s (own time).
    pub evade_secs: f32,
    /// Stop moving on its own (defeated, cutscene). Gravity still applies.
    pub disabled: bool,
    /// Velocity added on the next step (hits, explosions), m/s.
    pub knockback: Vec3,
    /// Dash requested by combat or gameplay, consumed next step.
    pub pending_dash: Option<DashRequest>,
    /// The moving body under the character, if any (see [`crate::platform`]).
    #[serde(skip)]
    pub platform: Option<Platform>,

    pub(crate) coyote: f32,
    pub(crate) jump_buffer: f32,
    pub(crate) slide_cooldown: f32,
    pub(crate) last_wall: Option<WallContact>,
    pub(crate) stand_height: f32,
    pub(crate) stride: f32,
    pub(crate) left_foot: bool,
    pub(crate) fall_speed: f32,
    pub(crate) prev_jump: bool,
    pub(crate) prev_crouch: bool,
    pub(crate) prev_dash: bool,
    pub(crate) initialized: bool,
    /// Crouched under something too low to stand up under.
    pub(crate) low_ceiling: bool,
    #[serde(skip)]
    pub(crate) influence: Influence,
}

impl Default for Mover {
    fn default() -> Self {
        Self {
            mode: LocomotionMode::Idle,
            velocity: Vec3::ZERO,
            facing: Vec3::NEG_Z,
            grounded: false,
            ground_normal: Vec3::Y,
            up: Vec3::Y,
            mode_secs: 0.0,
            air_secs: 0.0,
            air_jumps_left: 0,
            dash_charges: 0.0,
            wall: None,
            dash: None,
            mantle: None,
            crouched: false,
            evade_secs: 0.0,
            disabled: false,
            knockback: Vec3::ZERO,
            pending_dash: None,
            platform: None,
            coyote: 0.0,
            jump_buffer: 0.0,
            slide_cooldown: 0.0,
            last_wall: None,
            stand_height: 0.0,
            stride: 0.0,
            left_foot: false,
            fall_speed: 0.0,
            prev_jump: false,
            prev_crouch: false,
            prev_dash: false,
            initialized: false,
            low_ceiling: false,
            influence: Influence {
                move_scale: 1.0,
                lunge: None,
                face: None,
            },
        }
    }
}

impl Mover {
    /// Speed along the ground plane (own time), m/s.
    pub fn horizontal_speed(&self) -> f32 {
        flat(self.velocity, self.up).length()
    }

    /// Which side the wall being run on is: +1 = on the right, −1 = on the left, 0 = none.
    pub fn wall_side(&self) -> f32 {
        match (self.mode, &self.wall) {
            (LocomotionMode::WallRun, Some(w)) => {
                let right = self.facing.cross(self.up);
                if right.dot(-w.normal) >= 0.0 {
                    1.0
                } else {
                    -1.0
                }
            }
            _ => 0.0,
        }
    }

    /// A body lean for presentation (puppets, afterimages): tilt into walls while
    /// wall-running, lean back in slides, forward in dashes. Relative to the facing frame.
    pub fn body_tilt(&self) -> Quat {
        match self.mode {
            LocomotionMode::WallRun => Quat::from_rotation_z(-self.wall_side() * 0.45),
            LocomotionMode::Slide => Quat::from_rotation_x(0.55),
            LocomotionMode::Dash => Quat::from_rotation_x(-0.35),
            LocomotionMode::Sprint => Quat::from_rotation_x(-0.15),
            LocomotionMode::Mantle => Quat::from_rotation_x(-0.25),
            _ => Quat::IDENTITY,
        }
    }

    /// Teleport bookkeeping: clear velocity and any dash, mantle or wall-run.
    pub fn reset_motion(&mut self) {
        self.velocity = Vec3::ZERO;
        self.knockback = Vec3::ZERO;
        self.dash = None;
        self.mantle = None;
        self.wall = None;
        self.last_wall = None;
        self.pending_dash = None;
        self.mode = LocomotionMode::Idle;
        self.mode_secs = 0.0;
        self.fall_speed = 0.0;
    }

    pub(crate) fn set_mode(&mut self, mode: LocomotionMode) {
        if self.mode != mode {
            self.mode = mode;
            self.mode_secs = 0.0;
        }
    }
}

/// `v` without its component along `up`.
pub fn flat(v: Vec3, up: Vec3) -> Vec3 {
    v - up * v.dot(up)
}

/// Move `current` toward `target` by at most `max_delta`.
pub fn move_towards(current: Vec3, target: Vec3, max_delta: f32) -> Vec3 {
    let d = target - current;
    let len = d.length();
    if len <= max_delta || len < 1e-6 {
        target
    } else {
        current + d / len * max_delta
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mover_round_trips_through_json() {
        let m = Mover {
            mode: LocomotionMode::WallRun,
            velocity: Vec3::new(9.0, 0.5, 0.0),
            wall: Some(WallContact {
                normal: Vec3::Z,
                point: DVec3::new(1.0, 2.0, 3.0),
                entity: None,
            }),
            ..Default::default()
        };
        let json = serde_json::to_value(&m).unwrap();
        assert_eq!(json["mode"], "WallRun");
        let back: Mover = serde_json::from_value(json).unwrap();
        assert_eq!(back.velocity, m.velocity);
        assert_eq!(back.wall.unwrap().normal, Vec3::Z);
    }

    #[test]
    fn wall_side_and_tilt() {
        let mut m = Mover {
            mode: LocomotionMode::WallRun,
            facing: Vec3::NEG_Z,
            ..Default::default()
        };
        // Facing −Z, right is +X. A wall on the right has its normal pointing −X.
        m.wall = Some(WallContact {
            normal: Vec3::NEG_X,
            point: DVec3::ZERO,
            entity: None,
        });
        assert_eq!(m.wall_side(), 1.0);
        m.wall.as_mut().unwrap().normal = Vec3::X;
        assert_eq!(m.wall_side(), -1.0);
        assert_ne!(m.body_tilt(), Quat::IDENTITY);
    }

    #[test]
    fn move_towards_clamps() {
        let v = move_towards(Vec3::ZERO, Vec3::new(10.0, 0.0, 0.0), 2.0);
        assert_eq!(v, Vec3::new(2.0, 0.0, 0.0));
        let v = move_towards(Vec3::ZERO, Vec3::new(1.0, 0.0, 0.0), 2.0);
        assert_eq!(v, Vec3::new(1.0, 0.0, 0.0));
    }
}
