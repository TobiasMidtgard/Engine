//! The locomotion contract: one way to say "move like this" for players, NPCs and vehicles,
//! so input, AI and the movement/vehicle modules can each be replaced on their own.
//!
//! * [`MoveIntent`]: what a character wants (written by player input or NPC AI), consumed by
//!   the movement module, which drives the physics `CharacterController` and animation.
//! * [`LocomotionState`]: what the character is actually doing (written by the movement
//!   module), read by AI (is the target dashing? blocking? in the air?), cameras and UI.
//! * [`VehicleControls`]: the same idea for cars, planes and ships.
//! * [`LocomotionBackend`]: which module turns `MoveIntent` into motion. The movement module
//!   calls [`claim_backend`] in its plugin's `build`; simple fallback movers (such as the NPC
//!   module's built-in walker for games without a movement module) check for the resource at
//!   run time and step aside when it exists.

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

/// Which locomotion module turns [`MoveIntent`] into motion (set by [`claim_backend`]).
///
/// Fallback movers check for this resource when their systems run (for example with the
/// `resource_exists::<LocomotionBackend>` run condition), not in `Plugin::build`, because the
/// movement plugin may be added after theirs.
#[derive(Resource, Debug, Clone, Serialize, Deserialize)]
pub struct LocomotionBackend {
    pub name: String,
}

/// Call from a movement plugin's `build`. Panics with a clear message if another locomotion
/// module is already installed, so two of them can't fight over the same characters.
pub fn claim_backend(app: &mut App, name: &str) {
    if let Some(existing) = app.world.get_resource::<LocomotionBackend>() {
        panic!(
            "Two locomotion modules: \"{}\" is installed and \"{name}\" tried to install too. \
             Add only one movement plugin.",
            existing.name
        );
    }
    app.insert_resource(LocomotionBackend {
        name: name.to_string(),
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn claiming_records_the_backend() {
        let mut app = App::new();
        claim_backend(&mut app, "movement");
        assert_eq!(app.world.resource::<LocomotionBackend>().name, "movement");
    }

    #[test]
    #[should_panic(expected = "Two locomotion modules")]
    fn a_second_claim_panics() {
        let mut app = App::new();
        claim_backend(&mut app, "movement");
        claim_backend(&mut app, "other");
    }
}
