#![doc = include_str!("../README.md")]

pub mod aero;
pub mod api;
pub mod autopilot;
pub mod chassis;
pub mod demo;
pub mod drivetrain;
pub mod entry;
pub mod events;
pub mod exhaust;
pub mod models;
pub mod presets;
pub mod ship;
pub mod tire;
pub mod visuals;
pub mod wheels;

use engine_core::prelude::*;
use engine_world::physics::{ApplyImpulse, ContactEvent};
use engine_world::{DamageEvent, GroundContact, NoiseEvent, SmokeEmission, WorldImpulse};

pub use aero::{
    AeroSurface, Aircraft, ControlAxis, Propulsion, Propulsor, PropulsorKind, SurfaceControl,
};
pub use autopilot::{Autopilot, AutopilotMode};
pub use chassis::{
    AeroBody, Vehicle, VehicleForces, VehicleFrame, VehicleKind, VehicleSettings, VehicleTelemetry,
};
pub use drivetrain::{Differential, DriveAxle, Drivetrain, EngineSpec, Gearbox};
pub use engine_world::air::{AirSample, Atmospheres, BodyAtmosphere};
pub use events::EventTimers;
pub use ship::{AssistMode, FlightAssist, HeatShield, Spacecraft, Thruster, ThrusterRole};
pub use tire::{MagicFormula, TireModel};
pub use wheels::{AntiRollBar, Assists, SteerInput, Suspension, Wheel, Wheels};

/// Where vehicles step inside `FixedUpdate`: after gameplay and AI have written
/// `VehicleControls`, before the physics backend integrates the `ExternalForce` they produce.
#[derive(SystemSet, Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum VehicleSet {
    /// Autopilots write controls.
    Controls,
    /// Wheels, wings, engines and thrusters turn controls into forces.
    Forces,
    /// Messages to the rest of the world (tracks, noise, smoke, wash, crashes).
    Events,
}

/// Cars, aircraft and spacecraft on the `engine_world` physics contract. Needs
/// `engine_world::WorldPlugin` and a physics backend (any crate that implements the
/// contract); with no backend the vehicles compute their forces but nothing moves them.
pub struct VehiclesPlugin;

impl Plugin for VehiclesPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<VehicleSettings>()
            .add_message::<GroundContact>()
            .add_message::<NoiseEvent>()
            .add_message::<SmokeEmission>()
            .add_message::<WorldImpulse>()
            .add_message::<DamageEvent>()
            .add_message::<ApplyImpulse>()
            .add_message::<ContactEvent>()
            .configure_sets(
                FixedUpdate,
                (VehicleSet::Controls, VehicleSet::Forces, VehicleSet::Events)
                    .chain()
                    .after(SimSet::Gameplay)
                    .before(SimSet::Physics),
            )
            .add_systems(
                FixedUpdate,
                (
                    (chassis::prepare_frames, autopilot::autopilot_system)
                        .chain()
                        .in_set(VehicleSet::Controls),
                    (
                        wheels::wheels_system,
                        chassis::aero_body_system,
                        aero::aero_surfaces_system,
                        aero::propulsion_system,
                        ship::spacecraft_system,
                        ship::heating_system,
                        chassis::apply_forces,
                        chassis::telemetry_system,
                    )
                        .chain()
                        .in_set(VehicleSet::Forces),
                    (
                        events::wheel_events_system,
                        events::engine_events_system,
                        events::heat_damage_system,
                        events::engine_and_wheel_state_system,
                        exhaust::exhausts_system,
                        ship::propellant_mass_system,
                    )
                        .chain()
                        .in_set(VehicleSet::Events),
                ),
            )
            // Crashes are reported by the backend during the physics step.
            .add_systems(
                FixedUpdate,
                events::crash_events_system.in_set(SimSet::PostPhysics),
            )
            .add_systems(
                Update,
                (
                    visuals::wheel_visuals,
                    visuals::gear_visuals,
                    visuals::aircraft_visuals,
                    visuals::plume_visuals,
                    visuals::light_visuals,
                    visuals::heat_visuals,
                    visuals::plasma_visuals,
                    models::cars::caliper_visuals,
                )
                    .in_set(UpdateSet::Logic),
            );
        register_types(app);
        api::register(app);
    }
}

fn register_types(app: &mut App) {
    app.register_component::<Vehicle>(
        "A vehicle: name, kind (Car, Truck, Aircraft, Spacecraft, Boat, Hovercraft), fallback \
         mass (kg), inertia (kg·m²) and center of mass (local m) until physics reports them.",
    )
    .register_component::<VehicleTelemetry>(
        "Read-only gauges: speed (m/s), gear, rpm, airspeed, angle of attack, g-load, altitude, \
         apoapsis/periapsis, heat, wheels on the ground, what the vehicle is doing.",
    )
    .register_component::<Wheels>(
        "Wheels with raycast suspension (m, N/m, N·s/m), slip-based tires, brakes (N·m), \
         steering (rad), anti-roll bars and driver assists (ABS, traction control).",
    )
    .register_component::<Drivetrain>(
        "Engine torque curve (N·m vs rpm), gearbox ratios, clutch, automatic shifting and \
         differentials (open, locked, limited slip) per driven axle.",
    )
    .register_component::<AeroBody>(
        "Body drag and downforce: frontal area (m²), drag and lift coefficients, angular \
         damping.",
    )
    .register_component::<Aircraft>(
        "Wings and tail: lifting surfaces with airfoil coefficients, stall, flaps and control \
         surfaces (aileron, elevator, rudder), trim and landing gear.",
    )
    .register_component::<Propulsion>(
        "Air-breathing engines: propellers (power W, diameter m) and jets (thrust N), with \
         spool-up and thrust falling with air density.",
    )
    .register_component::<Spacecraft>(
        "Thrusters (position, direction, max thrust N, Isp s, role), flight assist mode (Off, \
         Rotation, Full, Hover) and rates, dry mass and propellant (kg).",
    )
    .register_component::<HeatShield>(
        "Re-entry heating: nose radius (m), heated area (m²), heat capacity (J/K), \
         emissivity, max temperature (K); current temperature (K).",
    )
    .register_component::<Autopilot>(
        "Built-in driver/pilot that writes VehicleControls: Off, Drive (path), Drift (circle), \
         Fly (waypoints), StallDemo, Ascend (to orbit), Hold (hover at a point), Hop (VTOL \
         hops from a pad).",
    )
    .register_resource::<VehicleSettings>(
        "Vehicle simulation settings: wheel substeps, ground query fallback, track spacing (m), \
         engine noise interval (s).",
    );
}
