//! [`Mover`]: the runtime state of one moving character, written by the movement systems
//! every fixed step and readable by anyone (AI, cameras, UI, the AI bridge).

use engine_core::math::{DVec3, Quat, Vec3};
use engine_core::prelude::*;
use engine_world::LocomotionMode;
use engine_world::SurfaceId;
use engine_world::animation::AnimationParams;
use engine_world::locomotion::{LocomotionState, MoveIntent};
use engine_world::physics::{CharacterController, CharacterMove, CharacterState};

use crate::platform::{Pin, Platform};

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
    /// The body being climbed: the ledge (and `forward`) move with it.
    #[serde(skip)]
    pub on: Option<Pin>,
}

/// Hanging from a ledge.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Hang {
    /// The ledge's edge in front of the hands: the wall's face at the top's height.
    pub top: DVec3,
    /// Out of the wall, horizontal unit.
    pub normal: Vec3,
    /// The body hung from: the ledge (and `normal`) move with it.
    #[serde(skip)]
    pub on: Option<Pin>,
    /// Going around a corner onto the next face.
    #[serde(default)]
    pub corner: Option<Corner>,
    /// After a corner: a stick still held the old way along the ledge (`.0`) shimmies the new
    /// way (`.1`), so holding one direction carries on around.
    #[serde(default)]
    pub carry: Option<(Vec3, Vec3)>,
}

impl Hang {
    pub fn new(top: DVec3, normal: Vec3, on: Option<Pin>) -> Self {
        Self {
            top,
            normal,
            on,
            corner: None,
            carry: None,
        }
    }
}

/// Going around a corner of a ledge: the body follows a curve from the hold on the face hung
/// from to the hold on the next face, an outside corner (the next face turned toward the
/// shimmy) or an inside one (a wall across the way).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Corner {
    /// The curve of the body's center: start, bend, end (a quadratic Bézier).
    pub from: DVec3,
    pub via: DVec3,
    pub to: DVec3,
    /// The ledge and its normal before the corner.
    pub from_top: DVec3,
    pub from_normal: Vec3,
    /// The ledge and its normal on the next face.
    pub top: DVec3,
    pub normal: Vec3,
    /// Along the next face, away from the corner.
    pub along: Vec3,
    /// The way the shimmy came (for [`Hang::carry`]).
    pub came: Vec3,
    /// The next face's body.
    #[serde(skip)]
    pub wall: Option<Entity>,
    /// Progress, 0..1.
    pub t: f32,
    /// How long the whole turn takes, s.
    pub secs: f32,
}

impl Corner {
    /// The body's center at progress `t`.
    pub fn at(&self, t: f32) -> DVec3 {
        let t = t.clamp(0.0, 1.0) as f64;
        self.from * ((1.0 - t) * (1.0 - t)) + self.via * (2.0 * t * (1.0 - t)) + self.to * (t * t)
    }
}

/// A grapple hook: flying out to where it was fired, or bitten in and reeling the character
/// in (see [`crate::grapple`]).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Grapple {
    /// Where the hook flies: the point it bites on a surface, or where it turns back at full
    /// range after a miss.
    pub anchor: DVec3,
    /// Out of the surface at the anchor (zero after a miss).
    pub normal: Vec3,
    /// How far the hook has flown from the hand, m.
    pub reach: f32,
    /// The hook has bitten: the rope reels the character in.
    pub attached: bool,
    /// Missed: the hook flies out to full range and is reeled back in.
    pub miss: bool,
    /// The taut rope's length, m (it only ever gets shorter).
    pub length: f32,
    /// Own seconds since the hook bit.
    pub secs: f32,
    /// The body the hook flies at or bit into: the anchor (and `normal`) move with it.
    #[serde(skip)]
    pub on: Option<Pin>,
    /// How fast the anchor moves (with its body), m/s in the character's own seconds.
    #[serde(default)]
    pub anchor_velocity: Vec3,
}

/// A zip line being ridden (see [`crate::zipline`]).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Zip {
    /// The line's entity (the one with the [`Zipline`](crate::zipline::Zipline)).
    #[serde(skip)]
    pub line: Option<Entity>,
    /// Where along the line the hand is: 0 at its near anchor, 1 at the far one.
    pub s: f32,
    /// +1 riding toward the far anchor, −1 toward the near one.
    pub dir: f32,
    /// Speed along the cable, m/s (own time).
    pub speed: f32,
    /// The way the rider travels, unit (world, down or up the slope).
    pub heading: Vec3,
    /// Own seconds on the line.
    pub secs: f32,
}

/// A ladder being climbed (see [`crate::ladder`]).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Ladder {
    /// The ladder's entity (the one with the
    /// [`Climbable`](engine_world::locomotion::Climbable)).
    #[serde(skip)]
    pub entity: Option<Entity>,
    /// How far up the climb the feet are: 0 on the lowest rung, 1 on the top one.
    pub t: f32,
    /// The same in meters above the lowest rung.
    pub climbed: f32,
    /// Speed up the ladder, m/s (own time; negative going down).
    pub speed: f32,
    /// The way the climber faces (toward the ladder), horizontal unit, world.
    pub facing: Vec3,
    /// Sliding down with the hands and feet on the rails.
    pub sliding: bool,
    /// Own seconds on the ladder.
    pub secs: f32,
    /// The rung (counted from the lowest, [`crate::ladder::RUNG`] apart) the last rung
    /// contact went out for.
    #[serde(default)]
    pub(crate) rung: i32,
}

/// Vaulting over a low obstacle (see [`crate::vault`]).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Vault {
    /// Where the hand plants: the near edge of the obstacle's top, in line with the body.
    pub plant: DVec3,
    /// The way over, horizontal unit.
    pub forward: Vec3,
    /// How deep the obstacle's top is along `forward`, m.
    pub depth: f32,
    /// Where the vault started, m along `forward` from the plant (negative: before it).
    pub from: f32,
    /// Horizontal speed over it, m/s (own time).
    pub speed: f32,
    /// Rising speed until the feet clear the top, m/s.
    pub lift: f32,
    /// Still rising (false = going over).
    pub rising: bool,
    /// Own seconds into the vault.
    pub secs: f32,
    /// What the obstacle's top is made of (the hand plant's contact).
    #[serde(skip)]
    pub(crate) surface: Option<SurfaceId>,
    /// The body vaulted: the plant (and `forward`) move with it.
    #[serde(skip)]
    pub on: Option<Pin>,
}

impl Vault {
    /// 0..1 through the vault for a body at `pos` with capsule `radius`: from where it
    /// started to clear of the far side.
    pub fn progress(&self, pos: DVec3, radius: f32) -> f32 {
        let along = (pos - self.plant).dot(self.forward.as_dvec3()) as f32;
        let end = self.depth + radius;
        ((along - self.from) / (end - self.from).max(0.1)).clamp(0.0, 1.0)
    }
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
    /// The ledge being hung from (mode [`LocomotionMode::Climb`]).
    pub hang: Option<Hang>,
    /// The grapple hook, while it flies or holds.
    pub grapple: Option<Grapple>,
    /// The zip line ridden.
    pub zip: Option<Zip>,
    /// The ladder climbed (mode [`LocomotionMode::Climb`]).
    pub ladder: Option<Ladder>,
    /// The obstacle vaulted (mode [`LocomotionMode::Mantle`]).
    pub vault: Option<Vault>,
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
    /// Fire the grapple hook at this point (players aim with the camera, brains at a target),
    /// consumed next step. Firing while the hook is out lets go instead.
    pub pending_grapple: Option<DVec3>,
    /// The moving body under the character, if any (see [`crate::platform`]).
    #[serde(skip)]
    pub platform: Option<Platform>,

    pub(crate) coyote: f32,
    pub(crate) jump_buffer: f32,
    pub(crate) slide_cooldown: f32,
    /// No ledge grabs until this runs out (after letting go or kicking off), s.
    pub(crate) grab_cooldown: f32,
    /// No grapple shots until this runs out, s.
    pub(crate) grapple_cooldown: f32,
    /// No catching a zip line until this runs out, s.
    pub(crate) zip_cooldown: f32,
    /// The zip line last let go of: not caught again before touching the ground.
    #[serde(skip)]
    pub(crate) zip_last: Option<Entity>,
    /// No grabbing a ladder until this runs out, s.
    pub(crate) ladder_cooldown: f32,
    /// The ladder last kicked off: not grabbed again before touching the ground.
    #[serde(skip)]
    pub(crate) ladder_last: Option<Entity>,
    pub(crate) last_wall: Option<WallContact>,
    pub(crate) stand_height: f32,
    pub(crate) stride: f32,
    pub(crate) left_foot: bool,
    pub(crate) fall_speed: f32,
    /// Just landed and still coming down onto the ground at this speed, m/s (see
    /// `locomotion::settle`).
    pub(crate) settle: f32,
    /// Down onto ground a step below this step only, m/s (`locomotion::step_one`'s step-down).
    #[serde(skip)]
    pub(crate) step_down: f32,
    /// A wall or too-steep slope touched lately on foot, and seconds left to keep sliding along
    /// it (contact with something run into comes and goes step by step).
    #[serde(skip)]
    pub(crate) blocked_by: Option<(Vec3, f32)>,
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
            hang: None,
            grapple: None,
            zip: None,
            ladder: None,
            vault: None,
            crouched: false,
            evade_secs: 0.0,
            disabled: false,
            knockback: Vec3::ZERO,
            pending_dash: None,
            pending_grapple: None,
            platform: None,
            coyote: 0.0,
            jump_buffer: 0.0,
            slide_cooldown: 0.0,
            grab_cooldown: 0.0,
            grapple_cooldown: 0.0,
            zip_cooldown: 0.0,
            zip_last: None,
            ladder_cooldown: 0.0,
            ladder_last: None,
            last_wall: None,
            stand_height: 0.0,
            stride: 0.0,
            left_foot: false,
            fall_speed: 0.0,
            settle: 0.0,
            step_down: 0.0,
            blocked_by: None,
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

    /// Seconds until the grapple can fire again (0 = ready).
    pub fn grapple_cooldown(&self) -> f32 {
        self.grapple_cooldown
    }

    /// Teleport bookkeeping: clear velocity and any dash, mantle, vault, ledge hang, grapple,
    /// zip line, ladder or wall-run.
    pub fn reset_motion(&mut self) {
        self.velocity = Vec3::ZERO;
        self.knockback = Vec3::ZERO;
        self.dash = None;
        self.mantle = None;
        self.hang = None;
        self.grapple = None;
        self.pending_grapple = None;
        self.grapple_cooldown = 0.0;
        self.zip = None;
        self.zip_cooldown = 0.0;
        self.zip_last = None;
        self.ladder = None;
        self.ladder_cooldown = 0.0;
        self.vault = None;
        self.ladder_last = None;
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
