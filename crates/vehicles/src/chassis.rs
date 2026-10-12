//! What every vehicle shares: the [`Vehicle`] marker, the per-step [`VehicleFrame`] (where
//! the body is, its mass and the air around it), the force accumulator that becomes the
//! physics body's `ExternalForce`, body drag and downforce ([`AeroBody`]) and the
//! [`VehicleTelemetry`] gauges.

use engine_core::math::{DVec3, Quat, Vec3};
use engine_core::prelude::*;
use engine_world::physics::{ExternalForce, MassProperties, RigidBody, Velocity, point_velocity};
use engine_world::{Gravity, WeatherState, WindField};

use crate::air::{AirSample, VehicleAir};

/// What kind of machine this is (for UI, AI and the `vehicles.*` API).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum VehicleKind {
    #[default]
    Car,
    Truck,
    Aircraft,
    Spacecraft,
    Boat,
    Hovercraft,
}

/// Marks a physics body as a vehicle. Spawn presets with [`crate::presets`] or build one by
/// hand: `Vehicle`, a dynamic `RigidBody` with a `Collider`, `VehicleControls`, and any of
/// [`crate::Wheels`] (+ [`crate::Drivetrain`]), [`crate::Aircraft`], [`crate::Propulsion`],
/// [`crate::Spacecraft`], [`AeroBody`], [`crate::HeatShield`].
#[derive(Component, Debug, Clone, PartialEq, Serialize, Deserialize)]
#[require(
    VehicleForces,
    VehicleTelemetry,
    engine_world::VehicleControls,
    crate::events::EventTimers
)]
#[serde(default)]
pub struct Vehicle {
    pub name: String,
    pub kind: VehicleKind,
    /// Used when the physics backend hasn't reported mass yet, kg.
    pub mass_kg: f32,
    /// Principal moments of inertia used until the backend reports them, kg·m².
    pub inertia: Vec3,
    /// Center of mass in the body's local space, used until the backend reports it.
    pub center_of_mass: Vec3,
}

impl Default for Vehicle {
    fn default() -> Self {
        Self {
            name: "vehicle".into(),
            kind: VehicleKind::Car,
            mass_kg: 1_000.0,
            inertia: Vec3::new(1_000.0, 1_200.0, 400.0),
            center_of_mass: Vec3::ZERO,
        }
    }
}

impl Vehicle {
    pub fn new(name: &str, kind: VehicleKind, mass_kg: f32, size: Vec3) -> Self {
        Self {
            name: name.into(),
            kind,
            mass_kg,
            inertia: box_inertia(mass_kg, size),
            center_of_mass: Vec3::ZERO,
        }
    }
}

/// Principal inertia of a solid box of `size` (full extents), kg·m².
pub fn box_inertia(mass: f32, size: Vec3) -> Vec3 {
    let s = size * size;
    Vec3::new(s.y + s.z, s.x + s.z, s.x + s.y) * (mass / 12.0)
}

/// The body's state at the start of this step, shared by every vehicle subsystem.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct VehicleFrame {
    pub position: DVec3,
    pub rotation: Quat,
    /// Center of mass, world.
    pub com: DVec3,
    pub mass: f32,
    /// Principal inertia in body axes, kg·m².
    pub inertia: Vec3,
    pub linear: Vec3,
    /// rad/s, world axes.
    pub angular: Vec3,
    /// Gravity at the center of mass, m/s².
    pub gravity: Vec3,
    pub air: AirSample,
}

impl Default for VehicleFrame {
    fn default() -> Self {
        Self {
            position: DVec3::ZERO,
            rotation: Quat::IDENTITY,
            com: DVec3::ZERO,
            mass: 1.0,
            inertia: Vec3::ONE,
            linear: Vec3::ZERO,
            angular: Vec3::ZERO,
            gravity: Vec3::ZERO,
            air: AirSample::VACUUM,
        }
    }
}

impl VehicleFrame {
    /// World velocity of a world point on the body.
    pub fn point_velocity(&self, p: DVec3) -> Vec3 {
        point_velocity(
            &Velocity {
                linear: self.linear,
                angular: self.angular,
            },
            self.com,
            p,
        )
    }

    /// World position of a body-local point.
    pub fn to_world(&self, local: Vec3) -> DVec3 {
        self.position + (self.rotation * local).as_dvec3()
    }

    pub fn forward(&self) -> Vec3 {
        self.rotation * Vec3::NEG_Z
    }

    pub fn up(&self) -> Vec3 {
        self.rotation * Vec3::Y
    }

    pub fn right(&self) -> Vec3 {
        self.rotation * Vec3::X
    }

    /// Velocity relative to the air (world), m/s.
    pub fn airspeed_vector(&self) -> Vec3 {
        self.linear - self.air.wind
    }

    /// Angular velocity in body axes.
    pub fn local_angular(&self) -> Vec3 {
        self.rotation.inverse() * self.angular
    }

    /// Torque (world) that would change angular velocity by `delta` (world, rad/s) in
    /// `seconds`, using the body's inertia.
    pub fn torque_for(&self, delta_world: Vec3, seconds: f32) -> Vec3 {
        let local = self.rotation.inverse() * delta_world;
        self.rotation * (local * self.inertia / seconds.max(1e-3))
    }
}

/// Forces the vehicle's systems add this step; written to the body's `ExternalForce`.
/// Vehicles own their `ExternalForce`: use `ApplyImpulse` to push one from outside.
#[derive(Component, Debug, Clone, Copy, PartialEq, Default)]
pub struct VehicleForces {
    pub frame: VehicleFrame,
    /// World force, N.
    pub force: Vec3,
    /// World torque about the center of mass, N·m.
    pub torque: Vec3,
}

impl VehicleForces {
    /// Add a world force acting at a world point.
    pub fn add_at(&mut self, force: Vec3, point: DVec3) {
        self.force += force;
        self.torque += (point - self.frame.com).as_vec3().cross(force);
    }

    pub fn add_force(&mut self, force: Vec3) {
        self.force += force;
    }

    pub fn add_torque(&mut self, torque: Vec3) {
        self.torque += torque;
    }
}

/// Body drag and downforce for the whole vehicle (fuselage, car body, ship hull).
#[derive(Component, Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct AeroBody {
    /// Drag coefficient × frontal area for air along the body's x (side), y (top) and z
    /// (front) axes, m². A sedan's front ≈ 0.65, a sports car ≈ 0.55, a light plane's
    /// fuselage ≈ 0.4.
    pub drag_area: Vec3,
    /// Lift coefficient × area pushing the body down with forward speed, m² (race car
    /// wings ≈ 1–3, road car ≈ 0.1; negative = lift).
    pub downforce_area: f32,
    /// Where downforce acts, body local (move it back for more rear grip).
    pub downforce_center: Vec3,
    /// Rotational air damping, N·m per (rad/s)² per Pa of dynamic pressure; keeps
    /// tumbling bodies from spinning forever in air.
    pub angular_drag: f32,
}

impl Default for AeroBody {
    fn default() -> Self {
        Self {
            drag_area: Vec3::new(1.6, 2.0, 0.65),
            downforce_area: 0.1,
            downforce_center: Vec3::ZERO,
            angular_drag: 0.5,
        }
    }
}

/// Gauges for HUDs, AI and tests. Written every step.
#[derive(Component, Debug, Clone, Copy, PartialEq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct VehicleTelemetry {
    /// Ground speed, m/s.
    pub speed: f32,
    /// Speed along the vehicle's nose, m/s (negative reversing).
    pub forward_speed: f32,
    /// Speed relative to the air, m/s.
    pub airspeed: f32,
    pub mach: f32,
    /// Height above sea level of the dominant body, m.
    pub altitude_m: f64,
    /// Climb rate along local up, m/s.
    pub vertical_speed: f32,
    /// Air density, kg/m³.
    pub air_density: f32,
    /// ½ρv², Pa.
    pub dynamic_pressure: f32,
    /// Angle of attack and sideslip of the whole body, degrees.
    pub aoa_deg: f32,
    pub sideslip_deg: f32,
    /// Load factor felt by the occupants, g (1 at rest on the ground).
    pub g_load: f32,
    /// Engine rpm and gear (cars).
    pub rpm: f32,
    pub gear: i32,
    /// Wheels touching the ground.
    pub wheels_grounded: u32,
    /// Any wheel sliding past its grip.
    pub skidding: bool,
    /// Any wing past its stall angle.
    pub stalled: bool,
    /// Hottest heat shield temperature, K.
    pub hull_temperature_k: f32,
    /// Orbit around the dominant body (spacecraft): periapsis and apoapsis altitude, m.
    pub periapsis_m: f64,
    pub apoapsis_m: f64,
    /// Main engine throttle actually applied, 0..1.
    pub throttle: f32,
    #[serde(skip)]
    pub last_velocity: Vec3,
}

/// Global vehicle simulation settings.
#[derive(Resource, Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct VehicleSettings {
    /// Wheel/drivetrain substeps per physics step (stability of stiff tires).
    pub wheel_substeps: u32,
    /// Also find the ground through the `engine_world::Ground` query when no physics
    /// collider is under a wheel (flat worlds only): terrain without colliders still
    /// carries cars.
    pub use_ground_query: bool,
    /// Emit `GroundContact` for tracks every this many meters a wheel rolls.
    pub track_spacing_m: f32,
    /// Engine `NoiseEvent`s are sent this often, seconds.
    pub noise_interval_s: f32,
}

impl Default for VehicleSettings {
    fn default() -> Self {
        Self {
            wheel_substeps: 6,
            use_ground_query: true,
            track_spacing_m: 0.35,
            noise_interval_s: 0.5,
        }
    }
}

/// Body state, mass and air for every vehicle; clears the force accumulators.
#[allow(clippy::type_complexity)]
pub fn prepare_frames(
    air: Res<VehicleAir>,
    weather: Res<WeatherState>,
    gravity: Res<Gravity>,
    wind: Res<WindField>,
    mut q: Query<(
        &Vehicle,
        &Transform,
        Option<&Velocity>,
        Option<&MassProperties>,
        Option<&RigidBody>,
        &mut VehicleForces,
    )>,
) {
    q.par_iter_mut()
        .for_each(|(vehicle, t, vel, mass_props, body, mut forces)| {
            let (mass, com_local, inertia) = match mass_props {
                Some(m) if m.mass_kg > 0.0 => (
                    m.mass_kg,
                    m.center_of_mass,
                    if m.inertia.min_element() > 0.0 {
                        m.inertia
                    } else {
                        vehicle.inertia
                    },
                ),
                _ => (
                    body.and_then(|b| b.mass_kg).unwrap_or(vehicle.mass_kg),
                    vehicle.center_of_mass,
                    vehicle.inertia,
                ),
            };
            let com = t.translation + (t.rotation * com_local).as_dvec3();
            let v = vel.copied().unwrap_or_default();
            forces.frame = VehicleFrame {
                position: t.translation,
                rotation: t.rotation,
                com,
                mass: mass.max(1e-3),
                inertia: inertia.max(Vec3::splat(1e-3)),
                linear: v.linear,
                angular: v.angular,
                gravity: gravity.at(com),
                air: air.sample(&weather, &gravity, &wind, com),
            };
            forces.force = Vec3::ZERO;
            forces.torque = Vec3::ZERO;
        });
}

/// Drag, downforce and rotational damping from [`AeroBody`].
pub fn aero_body_system(mut q: Query<(&AeroBody, &mut VehicleForces)>) {
    q.par_iter_mut().for_each(|(aero, mut forces)| {
        let f = forces.frame;
        if f.air.density <= 0.0 {
            return;
        }
        let air = f.airspeed_vector();
        let local = f.rotation.inverse() * air;
        let speed = local.length();
        if speed > 1e-3 {
            let drag_local = -0.5 * f.air.density * speed * local * aero.drag_area;
            forces.add_force(f.rotation * drag_local);
            let forward = -local.z;
            if forward > 0.0 && aero.downforce_area != 0.0 {
                let down =
                    -f.up() * (0.5 * f.air.density * forward * forward * aero.downforce_area);
                let at = f.to_world(aero.downforce_center);
                forces.add_at(down, at);
            }
        }
        let w = f.angular;
        let q = 0.5 * f.air.density * speed.max(10.0).powi(2);
        forces.add_torque(-w * (1.0 + w.length()) * aero.angular_drag * q * 1e-3);
    });
}

/// Write the accumulated forces to each vehicle body's `ExternalForce`.
pub fn apply_forces(
    mut commands: Commands,
    mut q: Query<(Entity, &VehicleForces, Option<&mut ExternalForce>)>,
) {
    for (e, forces, ext) in &mut q {
        let value = ExternalForce {
            force: forces.force,
            torque: forces.torque,
            at: None,
        };
        match ext {
            Some(mut ext) => {
                if *ext != value {
                    *ext = value;
                }
            }
            None => {
                commands.entity(e).insert(value);
            }
        }
    }
}

/// Speed, altitude, AoA and g-load gauges (wheel, engine and wing gauges are filled in by
/// their systems).
pub fn telemetry_system(
    time: Res<Time>,
    gravity: Res<Gravity>,
    mut q: Query<(&VehicleForces, &mut VehicleTelemetry)>,
) {
    let dt = time.delta_secs().max(1e-6);
    q.par_iter_mut().for_each(|(forces, mut tel)| {
        let f = forces.frame;
        let up = gravity.up_at(f.com);
        tel.speed = f.linear.length();
        tel.forward_speed = f.linear.dot(f.forward());
        let air = f.airspeed_vector();
        tel.airspeed = air.length();
        tel.mach = tel.airspeed / f.air.speed_of_sound.max(1.0);
        tel.altitude_m = f.air.altitude_m;
        tel.vertical_speed = f.linear.dot(up);
        tel.air_density = f.air.density;
        tel.dynamic_pressure = f.air.dynamic_pressure(tel.airspeed);
        let local = f.rotation.inverse() * air;
        if tel.airspeed > 1.0 {
            tel.aoa_deg = (-local.y).atan2(-local.z).to_degrees();
            tel.sideslip_deg = (local.x / tel.airspeed)
                .clamp(-1.0, 1.0)
                .asin()
                .to_degrees();
        } else {
            tel.aoa_deg = 0.0;
            tel.sideslip_deg = 0.0;
        }
        // Proper acceleration (what an accelerometer reads), along the body's up axis.
        let accel = (f.linear - tel.last_velocity) / dt;
        let proper = accel - f.gravity;
        let g0 = 9.80665;
        let measured = proper.dot(f.up()) / g0;
        tel.g_load = if tel.last_velocity == Vec3::ZERO && accel.length() > 50.0 {
            1.0
        } else {
            tel.g_load + (measured - tel.g_load) * (dt * 10.0).min(1.0)
        };
        tel.last_velocity = f.linear;
        // Orbit about the dominant body.
        let (_, body) = crate::air::altitude(&gravity, f.com);
        if let Some(i) = body {
            let b = gravity.bodies[i];
            let (pe, ap) = orbit_apsides(f.com - b.center, f.linear.as_dvec3(), b.mu);
            tel.periapsis_m = pe - b.radius;
            tel.apoapsis_m = ap - b.radius;
        } else {
            tel.periapsis_m = f64::NEG_INFINITY;
            tel.apoapsis_m = f64::NEG_INFINITY;
        }
    });
}

/// Periapsis and apoapsis radius (m) of the orbit through `r` (m) with velocity `v` (m/s)
/// around a body with parameter `mu`. Apoapsis is infinite on escape trajectories.
pub fn orbit_apsides(r: DVec3, v: DVec3, mu: f64) -> (f64, f64) {
    let rn = r.length();
    let energy = v.length_squared() * 0.5 - mu / rn;
    let h = r.cross(v).length();
    let e = (1.0 + 2.0 * energy * h * h / (mu * mu)).max(0.0).sqrt();
    if energy >= 0.0 {
        let p = h * h / mu;
        return (p / (1.0 + e), f64::INFINITY);
    }
    let a = -mu / (2.0 * energy);
    (a * (1.0 - e), a * (1.0 + e))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn circular_orbit_has_equal_apsides() {
        let mu: f64 = 3.986e14;
        let r = 6.771e6;
        let v = (mu / r).sqrt();
        let (pe, ap) = orbit_apsides(DVec3::new(r, 0.0, 0.0), DVec3::new(0.0, 0.0, v), mu);
        assert!((pe - r).abs() < 1.0 && (ap - r).abs() < 1.0, "{pe} {ap}");
        let (pe, ap) = orbit_apsides(DVec3::new(r, 0.0, 0.0), DVec3::new(0.0, 0.0, v * 1.05), mu);
        assert!((pe - r).abs() < 1.0, "periapsis stays at the burn point");
        assert!(ap > r + 500_000.0, "{ap}");
        let (_, escape) = orbit_apsides(DVec3::new(r, 0.0, 0.0), DVec3::new(0.0, 0.0, v * 1.5), mu);
        assert!(escape.is_infinite());
    }

    #[test]
    fn box_inertia_is_largest_about_the_short_axis() {
        let i = box_inertia(1_200.0, Vec3::new(1.8, 1.3, 4.4));
        assert!(i.y > i.x && i.y > i.z);
        assert!(i.z < i.x);
    }
}
