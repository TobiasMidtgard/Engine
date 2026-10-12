//! Wheels: raycast suspension, steering with Ackermann geometry, brakes with ABS,
//! anti-roll bars and slip-based tires. Cars drive them through a [`Drivetrain`]; aircraft
//! use the same wheels as landing gear.
//!
//! Each physics step, every wheel casts a ray from its suspension mount down the body's
//! up axis. The spring and damper turn the compression into the tire's load; the tire turns
//! slip into forces (see [`crate::tire`]). The wheel's spin is integrated in substeps,
//! implicitly against the tire's stiffness, so the stiff tire never makes it oscillate.

use engine_core::math::{DVec3, Quat, Vec3, smoothstep};
use engine_core::prelude::*;
use engine_world::physics::{PhysicsQueries, QueryFilter};
use engine_world::{Gravity, Ground, SurfaceId, SurfaceLayers, SurfaceMaterials, VehicleControls};

use crate::chassis::{VehicleForces, VehicleSettings, VehicleTelemetry};
use crate::drivetrain::Drivetrain;
use crate::tire::{TireModel, sinkage_resistance, surface_grip};

/// The most wheels one vehicle can have.
pub const MAX_WHEELS: usize = 16;
/// Below this speed slip is measured against this speed instead (m/s), so it stays finite.
const V_LOW: f32 = 2.0;

/// Which control turns a wheel.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum SteerInput {
    /// Fixed straight ahead.
    #[default]
    None,
    /// `VehicleControls::steer` (cars).
    Steer,
    /// `VehicleControls::yaw` (an aircraft's nose wheel follows the rudder pedals).
    Yaw,
}

/// Spring and damper of one wheel.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Suspension {
    /// Travel from full droop to full compression, meters.
    pub travel_m: f32,
    /// Spring rate, N/m. Static sag ≈ corner weight / stiffness; aim for 30–50 % of travel.
    pub stiffness: f32,
    /// Damping while compressing, N·s/m.
    pub bump_damping: f32,
    /// Damping while extending, N·s/m (usually higher than bump).
    pub rebound_damping: f32,
}

impl Default for Suspension {
    fn default() -> Self {
        Self {
            travel_m: 0.25,
            stiffness: 45_000.0,
            bump_damping: 3_200.0,
            rebound_damping: 4_500.0,
        }
    }
}

impl Suspension {
    /// A spring that sags `sag` (0..1 of travel) under `corner_mass_kg`, damped to `zeta`
    /// of critical (0.3 comfortable, 0.5 sporty).
    pub fn tuned(travel_m: f32, corner_mass_kg: f32, sag: f32, zeta: f32) -> Self {
        let k = corner_mass_kg * 9.81 / (travel_m * sag).max(1e-3);
        let c = 2.0 * zeta * (k * corner_mass_kg).sqrt();
        Self {
            travel_m,
            stiffness: k,
            bump_damping: c * 0.8,
            rebound_damping: c * 1.2,
        }
    }
}

/// Live state of one wheel (written every step).
#[derive(Debug, Clone, Copy, PartialEq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct WheelState {
    pub grounded: bool,
    /// Spring length from the mount to the wheel center, meters.
    pub spring_length: f32,
    /// 0 at full droop .. `travel_m` at full compression.
    pub compression: f32,
    /// m/s, + compressing.
    pub compression_speed: f32,
    /// Where the tire touches the ground, world.
    pub contact_point: DVec3,
    pub contact_normal: Vec3,
    #[serde(skip)]
    pub contact_entity: Option<Entity>,
    pub surface: Option<SurfaceId>,
    /// Vertical load on the tire, N.
    pub load_n: f32,
    /// Spin, rad/s (+ rolling forward).
    pub spin: f32,
    /// Accumulated spin angle for drawing, rad.
    pub angle: f32,
    /// Current steering angle, rad (+ = right).
    pub steer: f32,
    pub slip_ratio: f32,
    /// rad, + = contact patch sliding to the wheel's right.
    pub slip_angle: f32,
    /// Tire forces, N.
    pub longitudinal_force: f32,
    pub lateral_force: f32,
    /// Fraction of available grip used (> 1 sliding).
    pub grip_used: f32,
    /// Sliding hard enough to leave marks and squeal.
    pub skidding: bool,
    /// Speed of the contact patch over the ground, m/s.
    pub ground_speed: f32,
    /// Meters rolled since the last ground contact event.
    pub since_track_m: f32,
    /// Force this wheel pushed into whatever it stands on, N (world).
    pub reaction: Vec3,
    /// Cached tire curve peaks (slip ratio, slip angle).
    #[serde(skip)]
    pub peaks: Option<(f32, f32)>,
}

/// One wheel. Positions are in the vehicle body's local space (meters, −Z forward).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Wheel {
    pub name: String,
    /// Top of the suspension (where the ray starts), body local.
    pub mount: Vec3,
    pub radius: f32,
    pub width: f32,
    /// Spin inertia of wheel, tire and brake disc, kg·m² (a car wheel ≈ 1–1.5).
    pub inertia: f32,
    pub suspension: Suspension,
    pub tire: TireModel,
    pub steer: SteerInput,
    /// Steering lock at full input, degrees (negative steers opposite: rear-wheel steer).
    pub max_steer_deg: f32,
    /// Brake torque at full pedal, N·m.
    pub brake_nm: f32,
    /// Handbrake torque, N·m (usually rear wheels only).
    pub handbrake_nm: f32,
    /// Raise where tire forces act above the contact patch, meters (0 = realistic; a little
    /// makes arcade cars roll less).
    pub tire_force_height: f32,
    #[serde(skip)]
    pub state: WheelState,
}

impl Default for Wheel {
    fn default() -> Self {
        Self {
            name: "wheel".into(),
            mount: Vec3::ZERO,
            radius: 0.33,
            width: 0.24,
            inertia: 1.2,
            suspension: Suspension::default(),
            tire: TireModel::default(),
            steer: SteerInput::None,
            max_steer_deg: 0.0,
            brake_nm: 1_800.0,
            handbrake_nm: 0.0,
            tire_force_height: 0.0,
            state: WheelState::default(),
        }
    }
}

/// Couples the left and right suspension of an axle so the body rolls less in corners.
/// Stiffer at the front = more understeer; at the rear = more oversteer.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct AntiRollBar {
    pub left: usize,
    pub right: usize,
    /// N/m of compression difference.
    pub stiffness: f32,
}

/// Driver aids.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Assists {
    /// Anti-lock brakes: back off a wheel's brake when it starts to lock.
    pub abs: bool,
    /// Traction control: cut throttle while driven wheels spin.
    pub traction_control: bool,
    /// Never steer more than the front tires can use at this speed (0 off .. 1 strong).
    pub steering_limit: f32,
    /// Stability control: take power away while the car slides sideways past this body
    /// slip angle, degrees (0 = off; rally and drift cars turn it off).
    pub stability_deg: f32,
}

impl Default for Assists {
    fn default() -> Self {
        Self {
            abs: true,
            traction_control: true,
            steering_limit: 0.6,
            stability_deg: 5.0,
        }
    }
}

/// All wheels of a vehicle.
#[derive(Component, Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Wheels {
    pub wheels: Vec<Wheel>,
    pub anti_roll: Vec<AntiRollBar>,
    /// How fast the steering turns, rad/s at the wheels.
    pub steer_rate: f32,
    /// Fraction of steering lock left at 40 m/s (speed-sensitive steering).
    pub steer_at_speed: f32,
    /// 0 parallel steering .. 1 full Ackermann (inside wheel turns more).
    pub ackermann: f32,
    pub assists: Assists,
}

impl Default for Wheels {
    fn default() -> Self {
        Self {
            wheels: Vec::new(),
            anti_roll: Vec::new(),
            steer_rate: 2.5,
            steer_at_speed: 0.35,
            ackermann: 0.8,
            assists: Assists::default(),
        }
    }
}

impl Wheels {
    /// Wheels touching the ground.
    pub fn grounded(&self) -> usize {
        self.wheels.iter().filter(|w| w.state.grounded).count()
    }

    /// Front-to-back distance between the steered and the fixed wheels, meters.
    pub fn wheelbase(&self) -> f32 {
        let zs = self.wheels.iter().map(|w| w.mount.z);
        let (min, max) = zs.fold((f32::MAX, f32::MIN), |(a, b), z| (a.min(z), b.max(z)));
        (max - min).max(0.5)
    }
}

/// What a ray down from a wheel mount found.
struct GroundHit {
    distance: f32,
    point: DVec3,
    normal: Vec3,
    surface: SurfaceId,
    entity: Option<Entity>,
}

fn cast_down(
    queries: &PhysicsQueries,
    ground: Option<&Ground>,
    origin: DVec3,
    down: Vec3,
    max: f32,
    filter: &QueryFilter,
) -> Option<GroundHit> {
    let mut best = queries
        .raycast(origin, down, max, filter)
        .map(|h| GroundHit {
            distance: h.distance,
            point: h.point,
            normal: h.normal,
            surface: h.surface,
            entity: Some(h.entity),
        });
    if let Some(ground) = ground
        && let Some(s) = ground.sample(origin)
    {
        let n = s.normal.normalize_or_zero();
        let denom = n.dot(down);
        if denom < -0.1 {
            let plane_point = DVec3::new(origin.x, s.height, origin.z);
            let t = (n.as_dvec3().dot(plane_point - origin) / denom as f64) as f32;
            // Colliders win ties: the ground query is the fallback for terrain without them.
            if (0.0..=max).contains(&t) && best.as_ref().is_none_or(|b| t < b.distance - 0.05) {
                best = Some(GroundHit {
                    distance: t,
                    point: origin + (down * t).as_dvec3(),
                    normal: n,
                    surface: s.surface,
                    entity: None,
                });
            }
        }
    }
    best
}

/// Steering angle for one wheel with Ackermann geometry. `center` is the angle of an
/// imaginary wheel on the centerline (rad, + right), `x` the wheel's lateral offset.
fn ackermann_angle(center: f32, x: f32, wheelbase: f32, amount: f32) -> f32 {
    if center.abs() < 1e-4 {
        return center;
    }
    let radius = wheelbase / center.tan(); // + = turning right, center to the right
    let exact = (wheelbase / (radius - x)).atan();
    let exact = if exact.signum() != center.signum() {
        center
    } else {
        exact
    };
    center + (exact - center) * amount
}

/// Simulate every vehicle's wheels for one physics step.
#[allow(clippy::type_complexity, clippy::too_many_arguments)]
pub fn wheels_system(
    settings: Res<VehicleSettings>,
    queries: Res<PhysicsQueries>,
    ground: Res<Ground>,
    gravity: Res<Gravity>,
    materials: Res<SurfaceMaterials>,
    surface_layers: Res<SurfaceLayers>,
    mut q: Query<(
        Entity,
        &mut Wheels,
        Option<&mut Drivetrain>,
        &VehicleControls,
        &mut VehicleForces,
        &mut VehicleTelemetry,
    )>,
) {
    let use_ground = settings.use_ground_query && gravity.bodies.is_empty();
    let substeps = settings.wheel_substeps.clamp(1, 32);
    q.par_iter_mut().for_each(
        |(entity, mut wheels, mut drivetrain, controls, mut forces, mut tel)| {
            let wheels = &mut *wheels;
            let n = wheels.wheels.len().min(MAX_WHEELS);
            let frame = forces.frame;
            let dt = frame.dt;
            if n == 0 || dt <= 0.0 {
                return;
            }
            let up = frame.up();
            let down = -up;
            let filter = QueryFilter::excluding(entity);
            let forward_speed = frame.linear.dot(frame.forward());
            let wheelbase = wheels.wheelbase();

            // ---- steering
            let speed_factor =
                1.0 - (1.0 - wheels.steer_at_speed) * (forward_speed.abs() / 40.0).min(1.0);
            let mut center_target = [0.0f32; MAX_WHEELS];
            for (i, w) in wheels.wheels.iter().take(n).enumerate() {
                let input = match w.steer {
                    SteerInput::None => 0.0,
                    SteerInput::Steer => controls.steer,
                    SteerInput::Yaw => controls.yaw,
                }
                .clamp(-1.0, 1.0);
                let mut lock = w.max_steer_deg.to_radians() * speed_factor;
                // Don't steer past the slip angle the tire can use (steering limit assist).
                if wheels.assists.steering_limit > 0.0 && forward_speed.abs() > 8.0 {
                    let (_, a_peak) = w.state.peaks.unwrap_or_else(|| w.tire.peaks());
                    let yaw_lock =
                        (wheelbase * 1.5 / forward_speed.abs().max(1.0)).atan() + a_peak * 1.2;
                    let limited = lock.abs().min(yaw_lock.max(a_peak));
                    lock = lock.signum()
                        * (lock.abs() + (limited - lock.abs()) * wheels.assists.steering_limit);
                }
                center_target[i] = input * lock;
            }
            let max_step = wheels.steer_rate * dt;
            let ackermann = wheels.ackermann;
            for (i, w) in wheels.wheels.iter_mut().take(n).enumerate() {
                if w.steer == SteerInput::None {
                    w.state.steer = 0.0;
                    continue;
                }
                let target = ackermann_angle(center_target[i], w.mount.x, wheelbase, ackermann);
                let delta = (target - w.state.steer).clamp(-max_step, max_step);
                w.state.steer += delta;
            }

            // ---- suspension
            let mut spring = [0.0f32; MAX_WHEELS];
            let mut hits: [Option<GroundHit>; MAX_WHEELS] = Default::default();
            for (i, w) in wheels.wheels.iter_mut().take(n).enumerate() {
                let mount = frame.to_world(w.mount);
                let s = &w.suspension;
                let max = s.travel_m + w.radius;
                let hit = cast_down(
                    &queries,
                    use_ground.then_some(&*ground),
                    mount,
                    down,
                    max,
                    &filter,
                );
                let st = &mut w.state;
                if st.peaks.is_none() {
                    st.peaks = Some(w.tire.peaks());
                }
                match &hit {
                    Some(h) => {
                        let spring_length = (h.distance - w.radius).clamp(0.0, s.travel_m);
                        let compression = s.travel_m - spring_length;
                        let speed = if st.grounded {
                            (compression - st.compression) / dt
                        } else {
                            0.0
                        };
                        let damping = if speed > 0.0 {
                            s.bump_damping
                        } else {
                            s.rebound_damping
                        };
                        // Bump stop: past full compression the tire itself squashes, hard.
                        let bottomed = (w.radius - h.distance).max(0.0);
                        spring[i] = (s.stiffness * compression
                            + damping * speed
                            + bottomed * s.stiffness * 20.0)
                            .max(0.0);
                        st.grounded = true;
                        st.spring_length = spring_length;
                        st.compression = compression;
                        st.compression_speed = speed;
                        st.contact_point = h.point;
                        st.contact_normal = h.normal;
                        st.contact_entity = h.entity;
                        st.surface = Some(h.surface);
                    }
                    None => {
                        st.grounded = false;
                        st.spring_length = s.travel_m;
                        st.compression = 0.0;
                        st.compression_speed = 0.0;
                        st.contact_entity = None;
                        st.surface = None;
                        st.load_n = 0.0;
                    }
                }
                hits[i] = hit;
            }
            for bar in &wheels.anti_roll {
                let (l, r) = (bar.left, bar.right);
                if l >= n || r >= n {
                    continue;
                }
                let (wl, wr) = (&wheels.wheels[l].state, &wheels.wheels[r].state);
                if !(wl.grounded && wr.grounded) {
                    continue;
                }
                let f = bar.stiffness * (wl.compression - wr.compression);
                spring[l] = (spring[l] + f).max(0.0);
                spring[r] = (spring[r] - f).max(0.0);
            }

            // ---- pedals and gears
            let (mut throttle, mut brake) = (
                controls.throttle.clamp(0.0, 1.0),
                controls.brake.clamp(0.0, 1.0),
            );
            let mut omega = [0.0f32; MAX_WHEELS];
            for (i, w) in wheels.wheels.iter().take(n).enumerate() {
                omega[i] = w.state.spin;
            }
            if let Some(d) = drivetrain.as_deref_mut() {
                // Shift on road speed, not on a wheel spinning in the mud: a spinning wheel
                // would upshift, bog down and downshift again.
                let mut radius = [0.0f32; MAX_WHEELS];
                for (i, w) in wheels.wheels.iter().take(n).enumerate() {
                    radius[i] = w.radius;
                }
                let r = d.driven_speed(&radius[..n]).max(0.05);
                let road = forward_speed / r;
                let spin = d.driven_speed(&omega[..n]);
                let driven = if spin.abs() > road.abs() * 1.15 + 2.0 {
                    road * 1.15 + 2.0 * road.signum()
                } else {
                    spin
                };
                (throttle, brake) = d.select_gear(throttle, brake, forward_speed, driven, dt);
            }
            // ---- stability control: no power while the tail is out
            if wheels.assists.stability_deg > 0.0 && forward_speed > 5.0 {
                let v_local = frame.rotation.inverse() * frame.linear;
                let beta = v_local.x.atan2(-v_local.z).abs().to_degrees();
                let over = (beta - wheels.assists.stability_deg) / 6.0;
                if over > 0.0 {
                    throttle *= (1.0 - over).clamp(0.0, 1.0);
                }
            }
            let handbrake = if controls.handbrake { 1.0 } else { 0.0 };
            let total_load: f32 = spring[..n].iter().sum::<f32>().max(1.0);

            // ---- contact kinematics
            struct Patch {
                fwd: Vec3,
                right: Vec3,
                vx: f32,
                vy: f32,
                load: f32,
                grip: f32,
                rolling: f32,
                mu: f32,
            }
            let mut patches: [Option<Patch>; MAX_WHEELS] = Default::default();
            for (i, w) in wheels.wheels.iter_mut().take(n).enumerate() {
                let Some(h) = &hits[i] else { continue };
                let nrm = h.normal;
                let wheel_fwd =
                    frame.rotation * (Quat::from_rotation_y(-w.state.steer) * Vec3::NEG_Z);
                let fwd = (wheel_fwd - nrm * wheel_fwd.dot(nrm)).normalize_or_zero();
                if fwd == Vec3::ZERO {
                    continue;
                }
                let right = fwd.cross(nrm);
                let v = frame.point_velocity(h.point);
                let material = materials.get(h.surface);
                let cell = surface_layers.sample(h.point);
                let vx = v.dot(fwd);
                let vy = v.dot(right);
                let grip = surface_grip(material, &cell, w.tire.surface_sensitivity, vx.abs());
                let rolling = w.tire.rolling_resistance + sinkage_resistance(material, &cell);
                let load = spring[i] * up.dot(nrm).max(0.2);
                w.state.load_n = load;
                w.state.ground_speed = (vx * vx + vy * vy).sqrt();
                patches[i] = Some(Patch {
                    fwd,
                    right,
                    vx,
                    vy,
                    load,
                    grip,
                    rolling,
                    mu: w.tire.mu_at_load(load) * grip,
                });
            }

            // ---- substeps: drivetrain + wheel spin + tire forces
            let h = dt / substeps as f32;
            let mut fx_sum = [0.0f32; MAX_WHEELS];
            let mut fy_sum = [0.0f32; MAX_WHEELS];
            let mut used_max = [0.0f32; MAX_WHEELS];
            let mut brake_torque = [0.0f32; MAX_WHEELS];
            for (i, w) in wheels.wheels.iter().take(n).enumerate() {
                let mut b = brake * w.brake_nm;
                // ABS: back off when the wheel slips past the tire's peak.
                if wheels.assists.abs
                    && let Some(p) = &patches[i]
                    && p.vx.abs() > 3.0
                {
                    let (k_peak, _) = w.state.peaks.unwrap_or((0.1, 0.1));
                    let kappa = (w.state.spin * w.radius - p.vx) / p.vx.abs();
                    if kappa * p.vx.signum() < -k_peak * 1.2 {
                        b *= 0.25;
                    }
                }
                brake_torque[i] = b + handbrake * w.handbrake_nm;
            }
            for _ in 0..substeps {
                let mut drive = [0.0f32; MAX_WHEELS];
                let mut inertia = [0.0f32; MAX_WHEELS];
                let mut slope = [0.0f32; MAX_WHEELS];
                for (i, w) in wheels.wheels.iter().take(n).enumerate() {
                    inertia[i] = w.inertia.max(0.05);
                    if let Some(p) = &patches[i] {
                        let v_ref = p.vx.abs().max(V_LOW);
                        let kappa = (omega[i] * w.radius - p.vx) / v_ref;
                        let alpha = p.vy.atan2(p.vx.abs().max(V_LOW));
                        let peaks = w.state.peaks.unwrap_or((0.1, 0.1));
                        let eps = 0.01;
                        let f1 = w
                            .tire
                            .forces(kappa + eps, alpha, p.load, p.grip, peaks)
                            .longitudinal;
                        let f0 = w
                            .tire
                            .forces(kappa - eps, alpha, p.load, p.grip, peaks)
                            .longitudinal;
                        slope[i] = ((f1 - f0) / (2.0 * eps)).max(0.0);
                        // Inertia the drivetrain feels: a gripping tire resists spinning up.
                        inertia[i] += h * w.radius * w.radius * slope[i] / v_ref;
                    }
                }
                if let Some(d) = drivetrain.as_deref_mut() {
                    // Traction control: no more drive torque than the driven tires can turn
                    // into thrust (what's left after cornering, by the friction circle),
                    // and less as soon as a wheel slips past the peak.
                    let limit = wheels.assists.traction_control.then(|| {
                        let mut sum = 0.0;
                        let mut over: f32 = 0.0;
                        for a in &d.axles {
                            for i in [a.left, a.right] {
                                let (Some(Some(p)), Some(w)) =
                                    (patches.get(i), wheels.wheels.get(i))
                                else {
                                    continue;
                                };
                                let full = p.mu * p.load;
                                let lat = w.state.lateral_force.abs().min(full);
                                let left = (full * full - lat * lat).sqrt().max(full * 0.15);
                                sum += left * w.radius;
                                let k_target = 0.8 * w.state.peaks.map_or(0.12, |pk| pk.0);
                                let kappa = (omega[i] * w.radius - p.vx) / p.vx.abs().max(V_LOW);
                                over = over.max((kappa - k_target) / k_target);
                            }
                        }
                        sum * 0.95 * (1.0 - over).clamp(0.25, 1.0)
                    });
                    d.substep(
                        throttle,
                        &omega[..n],
                        &inertia[..n],
                        h,
                        limit,
                        &mut drive[..n],
                    );
                }
                for (i, w) in wheels.wheels.iter().take(n).enumerate() {
                    let r = w.radius;
                    let i_w = w.inertia.max(0.05);
                    match &patches[i] {
                        Some(p) => {
                            let v_ref = p.vx.abs().max(V_LOW);
                            let alpha = p.vy.atan2(p.vx.abs().max(V_LOW));
                            let peaks = w.state.peaks.unwrap_or((0.1, 0.1));
                            let kappa = (omega[i] * r - p.vx) / v_ref;
                            let f = w.tire.forces(kappa, alpha, p.load, p.grip, peaks);
                            // Implicit in the tire's stiffness: stable for any substep.
                            let denom = 1.0 + h * r * r * slope[i] / (i_w * v_ref);
                            let free =
                                omega[i] + h * (drive[i] - f.longitudinal * r) / (i_w * denom);
                            let friction = brake_torque[i] + p.rolling * p.load * r;
                            let stop = friction * h / (i_w * denom);
                            omega[i] = if free.abs() <= stop {
                                0.0
                            } else {
                                free - free.signum() * stop
                            };
                            let kappa = (omega[i] * r - p.vx) / v_ref;
                            let f = w.tire.forces(kappa, alpha, p.load, p.grip, peaks);
                            fx_sum[i] += f.longitudinal;
                            fy_sum[i] += f.lateral;
                            used_max[i] = used_max[i].max(f.grip_used);
                        }
                        None => {
                            // In the air: driven wheels spin up, brakes stop them, bearings
                            // slowly slow them down.
                            let free = omega[i] + h * drive[i] / i_w;
                            let stop = (brake_torque[i] + 2.0) * h / i_w;
                            omega[i] = if free.abs() <= stop {
                                0.0
                            } else {
                                free - free.signum() * stop
                            };
                        }
                    }
                }
            }

            // ---- forces on the body
            let inv = 1.0 / substeps as f32;
            let locked_count = (0..n)
                .filter(|&i| patches[i].is_some() && omega[i] == 0.0 && brake_torque[i] > 0.0)
                .count()
                .max(1) as f32;
            let mut grounded = 0;
            let mut skidding = false;
            for (i, w) in wheels.wheels.iter_mut().take(n).enumerate() {
                let st = &mut w.state;
                st.spin = omega[i];
                st.angle = (st.angle + omega[i] * dt).rem_euclid(std::f32::consts::TAU * 64.0);
                let Some(p) = &patches[i] else {
                    st.longitudinal_force = 0.0;
                    st.lateral_force = 0.0;
                    st.slip_ratio = 0.0;
                    st.slip_angle = 0.0;
                    st.grip_used = 0.0;
                    st.skidding = false;
                    st.reaction = Vec3::ZERO;
                    continue;
                };
                grounded += 1;
                let mut fx = fx_sum[i] * inv;
                let mut fy = fy_sum[i] * inv;
                // Static friction at walking pace: hold the car still on slopes and stop
                // the last creep, which slip-based forces alone can't (they need slip to
                // push). Each tire cancels its share of the sliding velocity and of the
                // pull of gravity along the ground; braked wheels hold the whole car.
                let still = 1.0 - smoothstep(0.3, 2.5, st.ground_speed);
                if still > 0.0 {
                    let share = p.load / total_load;
                    let m_eff = frame.mass * share;
                    let cap = p.mu * p.load;
                    let g_y = frame.gravity.dot(p.right) * frame.mass * share;
                    let stick_y = (-m_eff * p.vy / dt * 0.5 - g_y).clamp(-cap, cap);
                    fy += (stick_y - fy) * still;
                    let locked = omega[i] == 0.0 && brake_torque[i] > 0.0;
                    if locked {
                        let g_x = frame.gravity.dot(p.fwd) * frame.mass / locked_count;
                        let stick_x = (-m_eff * p.vx / dt * 0.5 - g_x).clamp(-cap, cap);
                        fx += (stick_x - fx) * still;
                    }
                }
                st.longitudinal_force = fx;
                st.lateral_force = fy;
                st.slip_ratio = (omega[i] * w.radius - p.vx) / p.vx.abs().max(V_LOW);
                st.slip_angle = p.vy.atan2(p.vx.abs().max(V_LOW));
                st.grip_used = used_max[i];
                st.skidding = used_max[i] > 1.15 && st.ground_speed > 2.0;
                skidding |= st.skidding;
                let tire = p.fwd * fx + p.right * fy;
                let total = up * spring[i] + tire;
                let at = st.contact_point + (up * w.tire_force_height).as_dvec3();
                forces.add_at(total, at);
                st.reaction = -total;
                st.since_track_m += st.ground_speed * dt;
            }
            tel.wheels_grounded = grounded;
            tel.skidding = skidding;
            if let Some(d) = drivetrain.as_deref() {
                tel.rpm = d.state.rpm;
                tel.gear = d.state.gear;
                tel.throttle = d.state.throttle;
            }
        },
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn inside_wheel_steers_more() {
        let center = 0.3;
        let right = ackermann_angle(center, 0.8, 2.6, 1.0);
        let left = ackermann_angle(center, -0.8, 2.6, 1.0);
        assert!(right > center && left < center, "{left} {center} {right}");
        let parallel = ackermann_angle(center, 0.8, 2.6, 0.0);
        assert_eq!(parallel, center);
        let l = ackermann_angle(-center, -0.8, 2.6, 1.0);
        assert!(l < -center, "left turn: left wheel is inside");
    }

    #[test]
    fn tuned_suspension_sags_as_asked() {
        let s = Suspension::tuned(0.2, 350.0, 0.4, 0.4);
        let sag = 350.0 * 9.81 / s.stiffness;
        assert!((sag - 0.08).abs() < 1e-3);
        assert!(s.rebound_damping > s.bump_damping);
    }
}
