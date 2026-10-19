//! The locomotion contract: one way to say "move like this" for players, NPCs and vehicles,
//! so input, AI and the movement/vehicle modules can each be replaced on their own.
//!
//! * [`MoveIntent`]: what a character wants (written by player input or NPC AI), consumed by
//!   the movement module, which drives the physics `CharacterController` and animation.
//! * [`LocomotionState`]: what the character is actually doing (written by the movement
//!   module), read by AI (is the target dashing? blocking? in the air?), cameras and UI.
//! * [`VehicleControls`]: the same idea for cars, planes and ships; the vehicles module
//!   reports back [`EngineState`] and [`WheelContacts`] (engine and tyre sounds, HUDs, AI) and
//!   [`Exhausts`], where each nozzle is and how hard it fires (flames, plumes, smoke).
//! * [`Climbable`]: a ladder (or rungs, a climbing net) any module can place; the movement
//!   module climbs it in [`LocomotionMode::Climb`] and NPC navigation can use it as a link.
//! * [`LocomotionBackend`]: which module turns `MoveIntent` into motion. The movement module
//!   calls [`claim_backend`] in its plugin's `build`; simple fallback movers (such as the NPC
//!   module's built-in walker for games without a movement module) check for the resource at
//!   run time and step aside when it exists.

use engine_core::math::{DVec3, Vec3};
use engine_core::prelude::*;

use crate::events::AttackKind;
use crate::physical::SurfaceId;

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

/// What a vehicle's power plant is doing. The vehicles module writes it every frame on the
/// vehicle entity; audio plays the engine from it, and AI hearing and HUDs can read it too.
#[derive(Component, Debug, Clone, Copy, PartialEq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct EngineState {
    pub kind: EngineKind,
    pub running: bool,
    /// Crankshaft or turbine shaft speed, rpm (0 for rockets and thrusters).
    pub rpm: f32,
    pub idle_rpm: f32,
    pub max_rpm: f32,
    /// What the driver asks for, 0..1.
    pub throttle: f32,
    /// How hard it works (torque / max torque), 0..1: loud under load, burbles on overrun.
    pub load: f32,
    /// Current thrust of jets, rockets and thrusters, N.
    pub thrust_n: f32,
    /// Turbo spool or afterburner, 0..1.
    pub boost: f32,
}

/// What makes a vehicle go, so it can sound and behave like one.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum EngineKind {
    /// Combustion engine (cars, props, boats).
    Piston {
        cylinders: u8,
    },
    Electric,
    /// Jet, turboprop or helicopter turbine.
    Turbine,
    Rocket,
    /// Small attitude or ion thrusters.
    Thrusters,
}

impl Default for EngineKind {
    fn default() -> Self {
        EngineKind::Piston { cylinders: 4 }
    }
}

/// The wheels touching the ground this frame, for tyre roll, skids and gravel spray. The
/// vehicles module writes it on the vehicle entity.
#[derive(Component, Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct WheelContacts {
    pub wheels: Vec<WheelContact>,
}

/// One wheel on the ground.
#[derive(Debug, Clone, Copy, PartialEq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct WheelContact {
    /// Contact patch, world meters.
    pub position: DVec3,
    pub surface: SurfaceId,
    /// Speed of the tread at the contact patch, m/s.
    pub rolling_speed: f32,
    /// Combined slip, 0 (grip) .. 1 (sliding).
    pub slip: f32,
    pub load_kg: f32,
    pub radius_m: f32,
}

/// The engine exhausts an entity has (ships, planes, cars, jump kits), and how hard each fires
/// now, for flames, plumes, heat haze and smoke. The owner (vehicles, movement's jump kit)
/// writes it every frame and keeps the nozzles in the same order, since readers match their
/// effects to nozzles by index; a nozzle at level 0 is idle.
///
/// An effects module that draws exhausts (vfx) calls [`draws_exhausts`]; owners that draw
/// simple stand-in plumes of their own hide them while [`ExhaustEffects`] exists, so nothing
/// burns twice.
#[derive(Component, Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Exhausts(pub Vec<Exhaust>);

/// One nozzle, in the entity's own space (so it moves, turns and follows world shifts with it).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Exhaust {
    /// Center of the nozzle exit, m.
    pub position: Vec3,
    /// The way the gas leaves (unit).
    pub direction: Vec3,
    /// Radius of the nozzle exit, m.
    pub radius: f32,
    /// How hard it fires now, 0 (idle) .. 1 (full thrust; a car at full throttle).
    pub level: f32,
    pub kind: ExhaustKind,
}

impl Default for Exhaust {
    /// An idle 0.5 m rocket nozzle at the origin, blowing out the back (+Z, as entities face −Z).
    fn default() -> Self {
        Self::new(ExhaustKind::Rocket, Vec3::ZERO, Vec3::Z, 0.5)
    }
}

impl Exhaust {
    pub fn new(kind: ExhaustKind, position: Vec3, direction: Vec3, radius: f32) -> Self {
        Self {
            position,
            direction: direction.normalize_or(Vec3::Z),
            radius,
            level: 0.0,
            kind,
        }
    }

    pub fn at_level(mut self, level: f32) -> Self {
        self.level = level;
        self
    }

    /// Nozzle position and gas direction in world space for an entity at `at`.
    pub fn in_world(&self, at: &GlobalTransform) -> (DVec3, Vec3) {
        (
            at.transform_point(self.position.as_dvec3()),
            at.transform_vector(self.direction)
                .normalize_or(self.direction),
        )
    }
}

/// What comes out of a nozzle, so it looks and sounds right.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum ExhaustKind {
    /// Rocket engine: long bright plume, shock diamonds at low altitude.
    #[default]
    Rocket,
    /// Jet engine (dry): heat haze, faint flame at high power.
    Jet,
    /// Jet with afterburner lit: long flame with shock diamonds.
    Afterburner,
    /// Small control thruster (RCS): short puffs.
    Thruster,
    /// Combustion engine tailpipe: smoke and heat, a pop of flame on overrun.
    Tailpipe,
    /// Jump kit or jet pack on a character.
    JumpJet,
}

/// Present while an effects module draws [`Exhausts`] (see [`draws_exhausts`]); owners with
/// stand-in plumes of their own hide them while it exists. Check for it when systems run (for
/// example with `resource_exists::<ExhaustEffects>`), not in `Plugin::build`, since the effects
/// plugin may be added later.
#[derive(Resource, Debug, Clone, Serialize, Deserialize)]
pub struct ExhaustEffects {
    pub drawn_by: String,
}

/// Call from an effects plugin's `build` when it draws every [`Exhausts`] nozzle.
pub fn draws_exhausts(app: &mut App, name: &str) {
    if app.world.get_resource::<ExhaustEffects>().is_none() {
        app.insert_resource(ExhaustEffects {
            drawn_by: name.to_string(),
        });
    }
}

/// Something a character can climb, such as a ladder through a roof hatch or up a scaffold. Any
/// module places it on the ladder's entity (destruction's ladders, a game's level); the
/// movement module climbs it, and NPC navigation can treat bottom → [`exit`](Self::exit) as a
/// link.
///
/// Everything is in the entity's own space, so it moves, turns and follows world shifts with the
/// entity. A climber grabs on when it walks into the climb line facing the ladder (or
/// interacts), climbs with [`MoveIntent`] forward and back in [`LocomotionMode::Climb`], and
/// steps off at the top to `exit` or at the bottom to the ground. When the component goes
/// (the ladder is shot away), climbers drop off.
#[derive(Component, Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Climbable {
    /// Where a climber's feet are on the lowest rung, m.
    pub bottom: Vec3,
    /// Where a climber's feet are on the top rung, m.
    pub top: Vec3,
    /// Half the ladder's width, m: how far beside the line a character can be and still grab it.
    pub half_width: f32,
    /// The way a climber faces while on it, toward the ladder (unit, horizontal).
    pub facing: Vec3,
    /// Where a climber stands after stepping off at the top (onto the floor through a hatch,
    /// over a parapet), m.
    pub exit: Vec3,
}

impl Default for Climbable {
    /// A 3 m ladder at the origin, climbed facing −Z, stepped off forward at the top.
    fn default() -> Self {
        Self::ladder(Vec3::ZERO, Vec3::Y * 3.0, Vec3::NEG_Z)
    }
}

impl Climbable {
    /// A 0.5 m wide ladder from `bottom` to `top`, climbed facing `facing`, stepped off 0.6 m
    /// beyond the top in that direction.
    pub fn ladder(bottom: Vec3, top: Vec3, facing: Vec3) -> Self {
        let facing = facing.with_y(0.0).normalize_or(Vec3::NEG_Z);
        Self {
            bottom,
            top,
            half_width: 0.25,
            facing,
            exit: top + facing * 0.6,
        }
    }

    pub fn with_half_width(mut self, half_width: f32) -> Self {
        self.half_width = half_width;
        self
    }

    pub fn with_exit(mut self, exit: Vec3) -> Self {
        self.exit = exit;
        self
    }

    /// The climb in world space for a ladder entity at `at`.
    pub fn in_world(&self, at: &GlobalTransform) -> ClimbLine {
        ClimbLine {
            bottom: at.transform_point(self.bottom.as_dvec3()),
            top: at.transform_point(self.top.as_dvec3()),
            exit: at.transform_point(self.exit.as_dvec3()),
            facing: at.transform_vector(self.facing).normalize_or(self.facing),
            half_width: self.half_width * at.scale().x,
        }
    }
}

/// A [`Climbable`] in world space ([`Climbable::in_world`]).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ClimbLine {
    pub bottom: DVec3,
    pub top: DVec3,
    pub exit: DVec3,
    pub facing: Vec3,
    pub half_width: f32,
}

impl ClimbLine {
    /// How far up the climb the point nearest `p` is: 0 at the bottom rung, 1 at the top.
    pub fn along(&self, p: DVec3) -> f64 {
        let line = self.top - self.bottom;
        ((p - self.bottom).dot(line) / line.length_squared().max(1e-12)).clamp(0.0, 1.0)
    }

    /// The point on the climb line `t` of the way up (0 bottom, 1 top).
    pub fn at(&self, t: f64) -> DVec3 {
        self.bottom.lerp(self.top, t.clamp(0.0, 1.0))
    }

    /// How far `p` is from the climb line, m.
    pub fn distance(&self, p: DVec3) -> f64 {
        p.distance(self.at(self.along(p)))
    }
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
    )
    .register_component::<EngineState>(
        "What a vehicle's engine is doing (written by vehicles): kind, running, rpm, throttle, \
         load, thrust (N), boost.",
    )
    .register_component::<WheelContacts>(
        "Wheels on the ground this frame (written by vehicles): position, surface, rolling \
         speed, slip, load.",
    )
    .register_component::<Exhausts>(
        "Engine exhausts firing now (written by their owner every frame, nozzles in a fixed \
         order): position and direction in the entity's space, radius (m), level 0..1, kind \
         (rocket, jet, afterburner, thruster, tailpipe, jump jet).",
    )
    .register_component::<Climbable>(
        "A ladder characters can climb, in the entity's space: bottom and top (feet on the \
         lowest and top rung), half width, facing (toward the ladder), exit (where you step off \
         at the top).",
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
    use engine_core::math::Quat;

    #[test]
    fn a_ladder_turns_and_moves_with_its_entity() {
        let ladder = Climbable::ladder(Vec3::ZERO, Vec3::Y * 4.0, Vec3::NEG_Z);
        assert_eq!(ladder.exit, Vec3::new(0.0, 4.0, -0.6));
        // Turned a quarter to the left and standing at (10, 2, 5).
        let at = GlobalTransform::from(Transform {
            translation: DVec3::new(10.0, 2.0, 5.0),
            rotation: Quat::from_rotation_y(std::f32::consts::FRAC_PI_2),
            ..Transform::IDENTITY
        });
        let line = ladder.in_world(&at);
        assert!(line.bottom.distance(DVec3::new(10.0, 2.0, 5.0)) < 1e-6);
        assert!(line.top.distance(DVec3::new(10.0, 6.0, 5.0)) < 1e-6);
        assert!(line.facing.distance(Vec3::NEG_X) < 1e-6, "{}", line.facing);
        assert!(
            line.exit.distance(DVec3::new(9.4, 6.0, 5.0)) < 1e-5,
            "{}",
            line.exit
        );
        let halfway_beside = DVec3::new(10.0, 4.0, 5.3);
        assert!((line.along(halfway_beside) - 0.5).abs() < 1e-9);
        assert!((line.distance(halfway_beside) - 0.3).abs() < 1e-9);
        assert_eq!(line.along(DVec3::new(10.0, -3.0, 5.0)), 0.0);
    }

    #[test]
    fn an_exhaust_points_out_of_its_turned_entity() {
        // A nozzle under the tail, blowing backward; the ship turned a quarter to the left.
        let jet = Exhaust::new(
            ExhaustKind::Jet,
            Vec3::new(0.0, -1.0, 6.0),
            Vec3::Z * 2.0,
            0.6,
        )
        .at_level(0.8);
        assert_eq!(jet.direction, Vec3::Z);
        let at = GlobalTransform::from(Transform {
            translation: DVec3::new(100.0, 50.0, 0.0),
            rotation: Quat::from_rotation_y(std::f32::consts::FRAC_PI_2),
            ..Transform::IDENTITY
        });
        let (nozzle, out) = jet.in_world(&at);
        assert!(
            nozzle.distance(DVec3::new(106.0, 49.0, 0.0)) < 1e-5,
            "{nozzle}"
        );
        assert!(out.distance(Vec3::X) < 1e-6, "{out}");
        let from_data: Exhausts =
            serde_json::from_str(r#"[{"kind": "Tailpipe", "radius": 0.04}]"#).unwrap();
        assert_eq!(from_data.0[0].direction, Vec3::Z);
        assert_eq!(from_data.0[0].level, 0.0);
    }

    #[test]
    fn the_first_effects_module_to_draw_exhausts_is_recorded() {
        let mut app = App::new();
        assert!(app.world.get_resource::<ExhaustEffects>().is_none());
        draws_exhausts(&mut app, "vfx");
        draws_exhausts(&mut app, "heat haze");
        assert_eq!(app.world.resource::<ExhaustEffects>().drawn_by, "vfx");
    }

    #[test]
    fn ladders_from_data_fill_in_the_rest() {
        let ladder: Climbable = serde_json::from_str(r#"{"top": [0, 5, 0]}"#).unwrap();
        assert_eq!(ladder.top, Vec3::Y * 5.0);
        assert_eq!(ladder.facing, Vec3::NEG_Z);
        assert_eq!(ladder.half_width, 0.25);
    }

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
