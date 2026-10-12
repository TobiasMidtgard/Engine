//! The locomotion contract: one way to say "move like this" for players, NPCs and vehicles,
//! so input, AI and the movement/vehicle modules can each be replaced on their own.
//!
//! * [`MoveIntent`]: what a character wants (written by player input or NPC AI), consumed by
//!   the movement module, which drives the physics `CharacterController` and animation.
//! * [`LocomotionState`]: what the character is actually doing (written by the movement
//!   module), read by AI (is the target dashing? blocking? in the air?), cameras and UI.
//! * [`VehicleControls`]: the same idea for cars, planes and ships.

use engine_core::math::{DVec3, Vec3};
use engine_core::prelude::*;

use crate::events::AttackKind;

/// What a character wants to do this step. World-space directions.
#[derive(Component, Debug, Clone, Copy, PartialEq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct MoveIntent {
    /// Desired move direction (length 0..1 = fraction of top speed).
    pub direction: Vec3,
    /// Where to face/aim (unit vector), `None` = face the move direction.
    pub look: Option<Vec3>,
    pub jump: bool,
    pub sprint: bool,
    pub crouch: bool,
    /// Dash toward this point (gap closers, dodges).
    pub dash_to: Option<DVec3>,
    pub block: bool,
    pub attack: Option<AttackKind>,
}

/// Movement modes the movement module reports.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum LocomotionMode {
    #[default]
    Idle,
    Walk,
    Run,
    Sprint,
    Crouch,
    Air,
    WallRun,
    Slide,
    Dash,
    Mantle,
    Climb,
    Swim,
}

/// What a character is doing now (written by the movement module).
#[derive(Component, Debug, Clone, Copy, PartialEq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct LocomotionState {
    pub mode: LocomotionMode,
    /// m/s, world.
    pub velocity: Vec3,
    pub grounded: bool,
    pub blocking: bool,
    pub attacking: bool,
    /// Recovering from an attack or dash: open to a counter.
    pub vulnerable: bool,
    /// 0..1.
    pub stamina: f32,
}

/// Driver inputs for any vehicle (car, plane, ship). AI and players write it; the vehicles
/// module reads it.
#[derive(Component, Debug, Clone, Copy, PartialEq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct VehicleControls {
    /// 0..1.
    pub throttle: f32,
    /// 0..1.
    pub brake: f32,
    /// −1 (left) .. 1 (right): steering, or roll for aircraft and ships.
    pub steer: f32,
    /// −1..1: pitch (aircraft/ships).
    pub pitch: f32,
    /// −1..1: yaw (rudder).
    pub yaw: f32,
    /// −1..1: vertical thrust (VTOL, ships).
    pub lift: f32,
    /// −1..1: strafe thrust (ships).
    pub strafe: f32,
    pub handbrake: bool,
    pub boost: bool,
}

pub(crate) fn register(app: &mut App) {
    app.register_component::<MoveIntent>(
        "What a character wants: move direction (0..1 of top speed), look, jump, sprint, crouch, \
         dash target, block, attack.",
    )
    .register_component::<LocomotionState>(
        "What a character is doing: mode (walk, wall-run, dash…), velocity, grounded, blocking, \
         attacking, vulnerable, stamina.",
    )
    .register_component::<VehicleControls>(
        "Vehicle driver inputs: throttle, brake, steer/roll, pitch, yaw, lift, strafe, handbrake, boost.",
    );
}
