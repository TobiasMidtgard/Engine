//! Autopilots: simple drivers and pilots that write `VehicleControls`, for demos, tests,
//! traffic and AI that just needs to get somewhere. Smarter behavior (racing lines, combat
//! flying) belongs to the NPC module, which writes the same `VehicleControls`.
//!
//! * [`AutopilotMode::Drive`]: follow a path (pure pursuit steering, braking for corners
//!   from the path's curvature).
//! * [`AutopilotMode::Drift`]: hold a powerslide around a circle, counter-steering and
//!   feathering the throttle to keep the slip angle.
//! * [`AutopilotMode::Fly`]: take off from a runway, climb and fly waypoints with banked,
//!   coordinated turns.
//! * [`AutopilotMode::StallDemo`]: pull up with the power off until the wing stalls, then
//!   recover, again and again.
//! * [`AutopilotMode::Ascend`]: launch to orbit (vertical climb, gravity turn, coast to
//!   apoapsis, circularize). In flat worlds: climb and cruise at the target altitude.
//! * [`AutopilotMode::Hold`]: hover a spacecraft at a point.
//! * [`AutopilotMode::Hop`]: VTOL hops: lift off a pad, hover, land, rest, again.

use engine_core::math::{DVec3, Vec3};
use engine_core::prelude::*;
use engine_world::{Gravity, VehicleControls};

use crate::aero::Aircraft;
use crate::chassis::{VehicleForces, VehicleTelemetry};
use crate::ship::{AssistMode, Spacecraft};
use crate::wheels::Wheels;

/// What the autopilot does.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub enum AutopilotMode {
    /// Leave `VehicleControls` alone.
    #[default]
    Off,
    /// Drive along `path` (world points, meters).
    Drive {
        path: Vec<DVec3>,
        looped: bool,
        /// m/s.
        max_speed: f32,
        /// Cornering the driver dares, in g (0.6 relaxed .. 1.1 racing).
        lateral_g: f32,
    },
    /// Powerslide around a circle.
    Drift {
        center: DVec3,
        radius: f32,
        /// m/s. A slide gives roughly 0.7 μ g sideways, so keep v²/radius below that:
        /// about 10 m/s on a 15 m circle on dirt.
        speed: f32,
        /// Body slip angle to hold, degrees (20–40 looks great).
        slip_deg: f32,
        clockwise: bool,
    },
    /// Take off (if on the ground) and fly the waypoints.
    Fly {
        waypoints: Vec<DVec3>,
        looped: bool,
        /// Airspeed to hold, m/s.
        cruise_speed: f32,
        /// Lift the nose at this airspeed on the runway, m/s.
        rotate_speed: f32,
    },
    /// Climb to `altitude_m`, then stall and recover repeatedly.
    StallDemo { altitude_m: f32, heading: Vec3 },
    /// Launch toward `heading` (world, horizontal) into an orbit `target_altitude_m` high.
    Ascend {
        target_altitude_m: f64,
        /// Start pitching over at this altitude…
        turn_start_m: f64,
        /// …and be nearly horizontal by this one.
        turn_end_m: f64,
        heading: Vec3,
    },
    /// Hover at a point (spacecraft).
    Hold { position: DVec3 },
    /// Lift off from `pad` (a point on the ground), hover `height_m` above it for `hover_s`
    /// seconds, land, rest `rest_s` seconds and go again (spacecraft with lift thrusters).
    Hop {
        pad: DVec3,
        height_m: f32,
        hover_s: f32,
        rest_s: f32,
    },
}

/// Drives or flies the vehicle it is on.
#[derive(Component, Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct Autopilot {
    pub mode: AutopilotMode,
    /// What the autopilot is doing right now ("takeoff roll", "gravity turn", …).
    pub phase: String,
    /// Index of the next path point or waypoint.
    pub target: usize,
    /// Seconds in the current phase.
    pub timer: f32,
    /// Integrator for speed control.
    #[serde(skip)]
    pub integral: f32,
    /// Altitude when the autopilot was engaged (runway height), m.
    #[serde(skip)]
    pub start_altitude: Option<f64>,
    /// Integrator for holding a spot (spacecraft) or a runway's centerline (planes):
    /// sideways error over time, m·s.
    #[serde(skip)]
    pub hold_integral: Vec3,
    /// The runway's line for a take-off: where the roll began and the level direction the
    /// nose pointed.
    #[serde(skip)]
    pub runway: Option<(DVec3, Vec3)>,
}

impl Autopilot {
    pub fn new(mode: AutopilotMode) -> Self {
        Self {
            mode,
            ..Default::default()
        }
    }

    fn set_phase(&mut self, phase: &str) {
        if self.phase != phase {
            self.phase = phase.into();
            self.timer = 0.0;
        }
    }
}

/// Signed angle from `a` to `b` around `axis`, radians.
fn signed_angle(a: Vec3, b: Vec3, axis: Vec3) -> f32 {
    let c = a.cross(b).dot(axis);
    c.atan2(a.dot(b))
}

/// Distance from `p` to segment `a`–`b`, and the parameter along it.
fn segment_distance(p: DVec3, a: DVec3, b: DVec3) -> (f64, f64) {
    let ab = b - a;
    let t = ((p - a).dot(ab) / ab.length_squared().max(1e-9)).clamp(0.0, 1.0);
    ((a + ab * t - p).length(), t)
}

/// Turn-rate commands that point the body's nose along `dir` with its top toward `up`.
/// Returns `(pitch, yaw, roll)` rate commands for Rotation assist (−1..1).
fn point_at(
    forces: &VehicleForces,
    dir: Vec3,
    up: Vec3,
    rates_deg: Vec3,
    gain: f32,
) -> (f32, f32, f32) {
    let f = &forces.frame;
    let fwd = f.forward();
    let axis = fwd.cross(dir);
    let angle = fwd.dot(dir).clamp(-1.0, 1.0).acos();
    let err_world = axis.normalize_or_zero() * angle;
    let mut e = f.rotation.inverse() * err_world;
    // Roll so the top faces `up` (only once the nose is roughly there).
    let right = f.right();
    let roll_err = right.dot(up).clamp(-1.0, 1.0).asin();
    e.z = if angle < 0.5 { roll_err } else { 0.0 };
    let rates = rates_deg * (std::f32::consts::PI / 180.0);
    let w = e * gain;
    (
        (w.x / rates.x.max(1e-3)).clamp(-1.0, 1.0),
        (-w.y / rates.y.max(1e-3)).clamp(-1.0, 1.0),
        (w.z / rates.z.max(1e-3)).clamp(-1.0, 1.0),
    )
}

/// Run every autopilot. Writes `VehicleControls` before the vehicles step.
#[allow(clippy::type_complexity)]
pub fn autopilot_system(
    gravity: Res<Gravity>,
    mut q: Query<(
        &mut Autopilot,
        &mut VehicleControls,
        &VehicleForces,
        &VehicleTelemetry,
        Option<&Wheels>,
        Option<&mut Aircraft>,
        Option<&mut Spacecraft>,
    )>,
) {
    q.par_iter_mut().for_each(
        |(mut ap, mut controls, forces, tel, wheels, aircraft, ship)| {
            let f = forces.frame;
            let dt = f.dt;
            if dt <= 0.0 {
                return;
            }
            let ap = &mut *ap;
            ap.timer += dt;
            let up = gravity.up_at(f.com);
            let mode = std::mem::take(&mut ap.mode);
            match &mode {
                AutopilotMode::Off => {}
                AutopilotMode::Drive {
                    path,
                    looped,
                    max_speed,
                    lateral_g,
                } => drive(
                    ap,
                    &mut controls,
                    forces,
                    tel,
                    wheels,
                    path,
                    *looped,
                    *max_speed,
                    *lateral_g,
                    up,
                ),
                AutopilotMode::Drift {
                    center,
                    radius,
                    speed,
                    slip_deg,
                    clockwise,
                } => drift(
                    ap,
                    &mut controls,
                    forces,
                    wheels,
                    *center,
                    *radius,
                    *speed,
                    *slip_deg,
                    *clockwise,
                    up,
                ),
                AutopilotMode::Fly {
                    waypoints,
                    looped,
                    cruise_speed,
                    rotate_speed,
                } => fly(
                    ap,
                    &mut controls,
                    forces,
                    tel,
                    aircraft,
                    waypoints,
                    *looped,
                    *cruise_speed,
                    *rotate_speed,
                    up,
                    dt,
                ),
                AutopilotMode::StallDemo {
                    altitude_m,
                    heading,
                } => stall_demo(
                    ap,
                    &mut controls,
                    forces,
                    tel,
                    aircraft,
                    *altitude_m,
                    *heading,
                    up,
                ),
                AutopilotMode::Ascend {
                    target_altitude_m,
                    turn_start_m,
                    turn_end_m,
                    heading,
                } => {
                    if let Some(mut ship) = ship {
                        ascend(
                            ap,
                            &mut controls,
                            forces,
                            tel,
                            &mut ship,
                            &gravity,
                            *target_altitude_m,
                            *turn_start_m,
                            *turn_end_m,
                            *heading,
                            up,
                        );
                    }
                }
                AutopilotMode::Hold { position } => {
                    if let Some(mut ship) = ship {
                        hold(
                            &mut controls,
                            forces,
                            &mut ship,
                            &mut ap.hold_integral,
                            *position,
                            up,
                            15.0,
                            dt,
                        );
                    }
                }
                AutopilotMode::Hop {
                    pad,
                    height_m,
                    hover_s,
                    rest_s,
                } => {
                    if let Some(mut ship) = ship {
                        hop(
                            ap,
                            &mut controls,
                            forces,
                            tel,
                            wheels,
                            aircraft,
                            &mut ship,
                            *pad,
                            *height_m,
                            *hover_s,
                            *rest_s,
                            up,
                        );
                    }
                }
            }
            ap.mode = mode;
        },
    );
}

#[allow(clippy::too_many_arguments)]
fn drive(
    ap: &mut Autopilot,
    c: &mut VehicleControls,
    forces: &VehicleForces,
    tel: &VehicleTelemetry,
    wheels: Option<&Wheels>,
    path: &[DVec3],
    looped: bool,
    max_speed: f32,
    lateral_g: f32,
    up: Vec3,
) {
    let n = path.len();
    if n < 2 {
        return;
    }
    let f = &forces.frame;
    let pos = f.position;
    let speed = tel.forward_speed.max(0.0);
    // Closest segment near the last one (search ahead a little, and everywhere when lost).
    let seg = |i: usize| (path[i % n], path[(i + 1) % n]);
    let last = if looped { n } else { n - 1 };
    let mut best = (f64::MAX, ap.target.min(last - 1), 0.0);
    let window: Vec<usize> = if ap.timer < 0.05 || ap.phase.is_empty() {
        (0..last).collect()
    } else {
        (0..12).map(|k| (ap.target + k) % last).collect()
    };
    for i in window {
        let (a, b) = seg(i);
        let (d, t) = segment_distance(pos, a, b);
        if d < best.0 {
            best = (d, i, t);
        }
    }
    if best.0 > 40.0 {
        for i in 0..last {
            let (a, b) = seg(i);
            let (d, t) = segment_distance(pos, a, b);
            if d < best.0 {
                best = (d, i, t);
            }
        }
    }
    ap.target = best.1;
    ap.set_phase("driving");
    // Walk the path from the closest point.
    let walk = |dist: f64| -> DVec3 {
        let (mut i, t) = (best.1, best.2);
        let (a, b) = seg(i);
        let mut p = a + (b - a) * t;
        let mut left = dist;
        loop {
            let (_, b) = seg(i);
            let d = (b - p).length();
            if d >= left {
                return p + (b - p) * (left / d.max(1e-9));
            }
            left -= d;
            p = b;
            i += 1;
            if !looped && i >= n - 1 {
                return path[n - 1];
            }
        }
    };
    let lookahead = (4.0 + 0.55 * speed as f64).clamp(6.0, 35.0);
    let target = walk(lookahead);
    let fwd = (f.forward() - up * f.forward().dot(up)).normalize_or_zero();
    let to = (target - pos).as_vec3();
    let to = (to - up * to.dot(up)).normalize_or_zero();
    let alpha = signed_angle(fwd, to, -up); // + = target to the right
    let wheelbase = wheels.map(|w| w.wheelbase()).unwrap_or(2.6);
    // The lock a full steering input gives at this speed (it shrinks as the car goes faster).
    let lock = wheels
        .map(|w| w.steer_lock(speed))
        .unwrap_or(30f32.to_radians())
        .max(0.02);
    // Pure pursuit; full lock toward the line when it is behind us.
    let delta = if alpha.abs() > std::f32::consts::FRAC_PI_2 {
        alpha.signum() * lock
    } else {
        (2.0 * wheelbase * alpha.sin() / lookahead as f32).atan()
    };
    c.steer = (delta / lock).clamp(-1.0, 1.0);
    // Corner speed: the slowest speed needed anywhere in braking distance ahead.
    let decel = 6.0f32;
    let horizon = (speed * speed / (2.0 * decel) + 25.0) as f64;
    let mut wanted = max_speed;
    let mut s = 5.0;
    while s < horizon {
        let a = walk(s - 5.0);
        let b = walk(s);
        let cc = walk(s + 5.0);
        let k = curvature(a, b, cc);
        if k > 1e-4 {
            let v_corner = (lateral_g * 9.81 / k).sqrt();
            let allowed = (v_corner * v_corner + 2.0 * decel * (s as f32 - 5.0).max(0.0)).sqrt();
            wanted = wanted.min(allowed);
        }
        s += 5.0;
    }
    if !looped && (path[n - 1] - pos).length() < (speed * speed / (2.0 * decel) + 3.0) as f64 {
        wanted = 0.0;
    }
    speed_control(ap, c, speed, wanted);
    c.handbrake = false;
}

fn curvature(a: DVec3, b: DVec3, c: DVec3) -> f32 {
    let (ab, bc, ca) = ((b - a).length(), (c - b).length(), (a - c).length());
    let area2 = (b - a).cross(c - a).length();
    if ab * bc * ca < 1e-9 {
        0.0
    } else {
        (2.0 * area2 / (ab * bc * ca)) as f32
    }
}

fn speed_control(ap: &mut Autopilot, c: &mut VehicleControls, speed: f32, wanted: f32) {
    let err = wanted - speed;
    ap.integral = (ap.integral + err * 0.02).clamp(-0.3, 0.5);
    if err > -0.5 {
        c.throttle = (0.15 * err + 0.25 + ap.integral).clamp(0.0, 1.0);
        c.brake = 0.0;
    } else {
        c.throttle = 0.0;
        c.brake = (-err * 0.12).clamp(0.0, 1.0);
        ap.integral *= 0.95;
    }
}

#[allow(clippy::too_many_arguments)]
fn drift(
    ap: &mut Autopilot,
    c: &mut VehicleControls,
    forces: &VehicleForces,
    wheels: Option<&Wheels>,
    center: DVec3,
    radius: f32,
    speed: f32,
    slip_deg: f32,
    clockwise: bool,
    up: Vec3,
) {
    let f = &forces.frame;
    let flat = |v: Vec3| v - up * v.dot(up);
    let rel = flat((f.position - center).as_vec3());
    let r = rel.length().max(0.1);
    let out = rel / r;
    // Turning left (counter-clockwise seen from above) means the center is on the left.
    let turn = if clockwise { 1.0 } else { -1.0 }; // + = turning right
    let tangent = up.cross(out) * -turn;
    let v = flat(f.linear);
    let v_len = v.length();
    let fwd = flat(f.forward()).normalize_or_zero();
    let max_steer = wheels
        .and_then(|w| w.wheels.iter().map(|w| w.max_steer_deg).reduce(f32::max))
        .unwrap_or(30.0)
        .to_radians();
    if ap.phase.is_empty() || ap.phase == "approach" {
        // Get up to speed on the circle with plain driving, then kick the tail out.
        ap.set_phase("approach");
        let aim = (tangent * 12.0 - out * (r - radius)).normalize_or_zero();
        let alpha = signed_angle(fwd, aim, -up);
        c.steer = (alpha * 1.5 / max_steer).clamp(-1.0, 1.0);
        c.throttle = if v_len < speed { 1.0 } else { 0.4 };
        c.brake = 0.0;
        c.handbrake = false;
        if v_len > speed * 0.9 && (r - radius).abs() < radius * 0.4 {
            ap.set_phase("kick");
        }
        return;
    }
    if ap.phase == "kick" {
        // Turn in hard with the handbrake for a moment.
        c.steer = turn;
        c.handbrake = ap.timer < 0.35;
        c.throttle = if ap.timer < 0.35 { 0.2 } else { 1.0 };
        let beta = if v_len > 1.0 {
            signed_angle(v / v_len, fwd, -up) * turn
        } else {
            0.0
        };
        if beta > slip_deg.to_radians() * 0.6 || ap.timer > 1.5 {
            ap.set_phase("drifting");
        }
        return;
    }
    // Drifting: slip angle β = angle from velocity to nose, + = nose into the turn.
    let beta = if v_len > 1.0 {
        signed_angle(v / v_len, fwd, -up) * turn
    } else {
        0.0
    };
    let target = slip_deg.to_radians();
    let v_out = v.dot(out);
    let r_err = r - radius;
    // Steering holds the slip angle: front wheels roughly along the velocity (counter-steer),
    // more into the turn when the slide is too shallow or the circle too wide.
    let steer_angle = -beta * 0.6 + (target - beta) + (r_err / radius).clamp(-0.5, 0.5) * 0.4;
    c.steer = (steer_angle * turn / max_steer.max(0.05)).clamp(-1.0, 1.0);
    // Throttle holds the circle: in a slide the tires give about μg sideways, so the radius
    // follows speed (r = v²/a). Slow down when wide or drifting outward, speed up when
    // tight. Extra throttle also swings the tail further out.
    let speed_target = speed - r_err * 0.23 - v_out * 1.1;
    c.throttle = (0.62 + (speed_target - v_len) * 0.34 + (target - beta) * 0.77).clamp(0.0, 1.0);
    c.brake = 0.0;
    // A short handbrake pull when the slide dies out.
    if ap.phase == "drifting" && beta < target * 0.35 && ap.timer > 1.5 && v_len > speed * 0.7 {
        ap.set_phase("re-kick");
    }
    c.handbrake = ap.phase == "re-kick" && ap.timer < 0.25;
    if ap.phase == "re-kick" && ap.timer >= 0.25 {
        ap.set_phase("drifting");
    }
    if v_len < speed * 0.4 || beta < -0.35 {
        // Spun out or stalled: start again.
        ap.set_phase("approach");
    }
}

/// Pitch (θ, nose above horizon) and bank (φ, right wing down) in radians.
fn attitude(forces: &VehicleForces, up: Vec3) -> (f32, f32) {
    let f = &forces.frame;
    let pitch = f.forward().dot(up).clamp(-1.0, 1.0).asin();
    let bank = (-f.right().dot(up)).clamp(-1.0, 1.0).asin();
    (pitch, bank)
}

/// Elevator command to hold a pitch angle.
fn hold_pitch(forces: &VehicleForces, target: f32, up: Vec3) -> f32 {
    let (pitch, bank) = attitude(forces, up);
    let q = forces.frame.local_angular().x;
    let bank_comp = (1.0 / bank.cos().max(0.5) - 1.0) * 0.5;
    ((target - pitch) * 3.0 - q * 1.2 + bank_comp).clamp(-1.0, 1.0)
}

/// Aileron command to hold a bank angle.
fn hold_bank(forces: &VehicleForces, target: f32, up: Vec3) -> f32 {
    let (_, bank) = attitude(forces, up);
    let p = -forces.frame.local_angular().z;
    ((target - bank) * 2.0 - p * 0.6).clamp(-1.0, 1.0)
}

#[allow(clippy::too_many_arguments)]
fn fly(
    ap: &mut Autopilot,
    c: &mut VehicleControls,
    forces: &VehicleForces,
    tel: &VehicleTelemetry,
    aircraft: Option<Mut<Aircraft>>,
    waypoints: &[DVec3],
    looped: bool,
    cruise_speed: f32,
    rotate_speed: f32,
    up: Vec3,
    dt: f32,
) {
    let f = &forces.frame;
    let Some(mut aircraft) = aircraft else { return };
    if waypoints.is_empty() {
        return;
    }
    let wp = waypoints[ap.target.min(waypoints.len() - 1)];
    let to = (wp - f.position).as_vec3();
    let to_flat = (to - up * to.dot(up)).normalize_or_zero();
    let fwd_flat = (f.forward() - up * f.forward().dot(up)).normalize_or_zero();
    let heading_err = signed_angle(fwd_flat, to_flat, -up); // + = waypoint to the right
    c.brake = 0.0;
    c.handbrake = false;
    if ap.phase.is_empty() && ap.timer < 0.1 {
        // Wheel contacts and telemetry aren't known until the vehicle has run once.
        return;
    }
    // Runway height, from this frame's air sample: the telemetry is a frame old and reads
    // 0 m before the vehicle has run, which for a runway on high ground (or a planet's
    // surface) put the plane far above it.
    let altitude = f.air.altitude_m;
    let start = *ap.start_altitude.get_or_insert(altitude);
    let height = (altitude - start) as f32;
    let on_ground = tel.wheels_grounded > 0 && height < 2.0;
    if ap.phase.is_empty() {
        // Too slow to fly means a take-off, even when the wheels are off the ground for a
        // moment (a plane put down on a runway bounces as it settles). Cruise logic at
        // walking speed would turn the nose into the wind and roll off the runway.
        if on_ground || (tel.speed < rotate_speed * 0.5 && height < 2.0) {
            ap.set_phase("takeoff roll");
            ap.runway = Some((f.position, fwd_flat));
        } else if tel.speed < rotate_speed * 0.5 && ap.timer < 1.0 {
            // Slow and well off the ground: still settling (or falling onto something), so
            // wait a moment before choosing.
            return;
        } else {
            ap.set_phase("cruise");
        }
    }
    // On the runway: steer the nose wheel for a point on the runway's line ahead, which
    // holds the centerline in a crosswind (with an integral for the steady push of the wind
    // on the tail).
    let (start, along) = ap.runway.unwrap_or((f.position, fwd_flat));
    let right = along.cross(up);
    let off = (f.position - start).as_vec3().dot(right);
    if on_ground && ap.runway.is_some() {
        ap.hold_integral.x = (ap.hold_integral.x + off * dt).clamp(-10.0, 10.0);
    }
    let off = off + ap.hold_integral.x * 0.4;
    let runway_yaw = |speed: f32| {
        let ahead = speed.max(10.0);
        let aim = (along * ahead - right * off).normalize_or_zero();
        let err = signed_angle(fwd_flat, aim, -up);
        (err * 3.0 + f.local_angular().y * 0.5).clamp(-1.0, 1.0)
    };
    match ap.phase.as_str() {
        "takeoff roll" => {
            aircraft.flaps = 0.4;
            aircraft.gear_down = true;
            c.throttle = 1.0;
            c.yaw = runway_yaw(tel.speed);
            c.steer = hold_bank(forces, 0.0, up);
            c.pitch = hold_pitch(forces, 0.0, up) * 0.3;
            if tel.airspeed > rotate_speed {
                ap.set_phase("rotate");
            }
        }
        "rotate" | "climb" => {
            c.throttle = 1.0;
            let target_pitch = if ap.phase == "rotate" { 8f32 } else { 10.0 }.to_radians();
            c.pitch = if on_ground {
                // Rotating on the main gear takes a firm pull.
                let (pitch, _) = attitude(forces, up);
                ((target_pitch - pitch) * 10.0 - f.local_angular().x * 2.0).clamp(-1.0, 1.0)
            } else {
                hold_pitch(forces, target_pitch, up)
            };
            if on_ground {
                c.steer = hold_bank(forces, 0.0, up);
                c.yaw = runway_yaw(tel.speed);
            } else {
                c.steer = hold_bank(forces, (heading_err * 0.8).clamp(-0.25, 0.25), up);
                c.yaw = (tel.sideslip_deg.to_radians() * 2.0).clamp(-1.0, 1.0);
            }
            if height > 15.0 {
                ap.set_phase("climb");
                aircraft.gear_down = false;
            }
            if height > 60.0 {
                aircraft.flaps = 0.0;
                ap.set_phase("cruise");
            }
        }
        _ => {
            // Cruise: fly a climb rate toward the waypoint's altitude (pitch = flight path
            // angle + angle of attack), hold the cruise speed, turn with bank.
            let alt_err = (wp - f.position).dot(up.as_dvec3()) as f32;
            let airspeed = tel.airspeed.max(1.0);
            // Don't trade away speed below 75% of cruise to climb.
            let energy = ((airspeed - cruise_speed * 0.75) / (cruise_speed * 0.15)).clamp(0.0, 1.0);
            let mut vs_target = (alt_err * 0.08).clamp(-6.0, 8.0);
            if vs_target > 0.0 {
                vs_target *= energy;
            }
            let gamma = (vs_target / airspeed).clamp(-0.3, 0.3).asin();
            let target_pitch = (gamma
                + tel.aoa_deg.to_radians().clamp(-0.1, 0.2)
                + (vs_target - tel.vertical_speed) * 0.01)
                .clamp(-0.25, 0.3);
            c.pitch = hold_pitch(forces, target_pitch, up);
            let bank = (heading_err * 1.2).clamp(-0.6, 0.6);
            c.steer = hold_bank(forces, bank, up);
            c.yaw = (tel.sideslip_deg.to_radians() * 2.0).clamp(-1.0, 1.0);
            let err = cruise_speed - tel.airspeed;
            ap.integral = (ap.integral + err * 0.002).clamp(-0.3, 0.3);
            c.throttle = if vs_target > 2.0 {
                1.0
            } else {
                (0.55 + err * 0.06 + ap.integral).clamp(0.0, 1.0)
            };
            let flat_dist = {
                let d = (wp - f.position).as_vec3();
                (d - up * d.dot(up)).length()
            };
            if flat_dist < 120.0 {
                ap.target += 1;
                if ap.target >= waypoints.len() {
                    ap.target = if looped { 0 } else { waypoints.len() - 1 };
                }
            }
        }
    }
    // Trim like a pilot: wind the steady elevator input into the trim so the pitch hold
    // has no standing error (an integrator). From rotation on.
    if ap.phase != "takeoff roll" && !tel.stalled {
        aircraft.pitch_trim = (aircraft.pitch_trim + c.pitch * dt * 0.4).clamp(-0.6, 0.6);
    }
}

#[allow(clippy::too_many_arguments)]
fn stall_demo(
    ap: &mut Autopilot,
    c: &mut VehicleControls,
    forces: &VehicleForces,
    tel: &VehicleTelemetry,
    aircraft: Option<Mut<Aircraft>>,
    altitude_m: f32,
    heading: Vec3,
    up: Vec3,
) {
    let f = &forces.frame;
    if let Some(mut a) = aircraft {
        a.flaps = 0.0;
        a.gear_down = false;
    }
    if ap.phase.is_empty() {
        ap.set_phase("climb");
    }
    let fwd_flat = (f.forward() - up * f.forward().dot(up)).normalize_or_zero();
    let heading = (heading - up * heading.dot(up)).normalize_or_zero();
    let heading_err = if heading == Vec3::ZERO {
        0.0
    } else {
        signed_angle(fwd_flat, heading, -up)
    };
    c.yaw = (tel.sideslip_deg.to_radians() * 2.0).clamp(-1.0, 1.0);
    match ap.phase.as_str() {
        "climb" => {
            c.throttle = 1.0;
            c.pitch = hold_pitch(forces, 0.15, up);
            c.steer = hold_bank(forces, (heading_err * 1.0).clamp(-0.4, 0.4), up);
            if tel.altitude_m as f32 > altitude_m && ap.timer > 5.0 {
                ap.set_phase("pull up");
            }
        }
        "pull up" => {
            // Power off, nose up and keep pulling: speed bleeds off until the wing stalls.
            c.throttle = 0.0;
            c.pitch = (hold_pitch(forces, 0.35, up) + ap.timer * 0.12).clamp(-1.0, 1.0);
            c.steer = hold_bank(forces, 0.0, up);
            if tel.stalled && ap.timer > 1.0 {
                ap.set_phase("stalled");
            }
            if ap.timer > 25.0 {
                ap.set_phase("recover");
            }
        }
        "stalled" => {
            // Hold the stall a moment so it's visible: the nose drops, a wing may drop.
            c.throttle = 0.0;
            c.pitch = 1.0;
            c.steer = 0.0;
            if ap.timer > 2.0 {
                ap.set_phase("recover");
            }
        }
        _ => {
            // Recovery: nose down, full power, wings level, then climb back.
            c.throttle = 1.0;
            c.pitch = hold_pitch(forces, -0.2, up);
            c.steer = hold_bank(forces, 0.0, up);
            if !tel.stalled && tel.airspeed > 38.0 {
                ap.set_phase("climb");
            }
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn ascend(
    ap: &mut Autopilot,
    c: &mut VehicleControls,
    forces: &VehicleForces,
    tel: &VehicleTelemetry,
    ship: &mut Spacecraft,
    gravity: &Gravity,
    target_altitude: f64,
    turn_start: f64,
    turn_end: f64,
    heading: Vec3,
    up: Vec3,
) {
    let f = &forces.frame;
    let rates = ship.assist.max_rate_deg_s;
    let heading = (heading - up * heading.dot(up)).normalize_or_zero();
    let heading = if heading == Vec3::ZERO {
        up.any_orthonormal_vector()
    } else {
        heading
    };
    // This frame's air sample: the telemetry is a frame old.
    let alt = f.air.altitude_m;
    let planet = !gravity.bodies.is_empty();
    c.strafe = 0.0;
    c.lift = 0.0;
    c.brake = 0.0;
    c.boost = false;
    let aim = |c: &mut VehicleControls, dir: Vec3, roll_up: Vec3| {
        let (p, y, r) = point_at(forces, dir.normalize_or_zero(), roll_up, rates, 1.2);
        c.pitch = p;
        c.yaw = y;
        c.steer = r;
    };
    if ap.phase.is_empty() && ap.timer < 0.1 {
        // The telemetry's altitude reads 0 m until the vehicle has run once.
        return;
    }
    let start = *ap.start_altitude.get_or_insert(alt);
    if ap.phase.is_empty() {
        // A ship standing on its tail goes straight up; one lying on its belly (a VTOL
        // spaceplane) lifts off on its hover engines first and then stands up.
        let nose_up = f.forward().dot(up);
        ap.set_phase(if nose_up > 0.7 {
            "liftoff"
        } else {
            "vertical lift"
        });
    }
    match ap.phase.as_str() {
        "vertical lift" => {
            ship.assist.mode = AssistMode::Hover;
            c.throttle = 0.0;
            c.lift = 1.0;
            let nose = (f.forward() - up * f.forward().dot(up)).normalize_or_zero();
            aim(c, if nose == Vec3::ZERO { heading } else { nose }, up);
            if alt - start > 40.0 {
                ap.set_phase("pitch up");
            }
        }
        "pitch up" => {
            // Hover engines carry the weight while the nose comes up; the main engines take
            // over as it rises.
            ship.assist.mode = AssistMode::Rotation;
            let elevation = f.forward().dot(up).clamp(-1.0, 1.0).asin();
            c.lift = elevation.cos().max(0.0);
            c.throttle = elevation.sin().max(0.25);
            aim(c, up, -heading);
            if elevation > 80f32.to_radians() {
                ap.set_phase("liftoff");
            }
        }
        "liftoff" => {
            ship.assist.mode = AssistMode::Rotation;
            c.throttle = 1.0;
            aim(c, up, -heading);
            if alt > turn_start {
                ap.set_phase("gravity turn");
            }
        }
        "gravity turn" => {
            ship.assist.mode = AssistMode::Rotation;
            let x = ((alt - turn_start) / (turn_end - turn_start).max(1.0)).clamp(0.0, 1.0);
            let angle = (x.sqrt() * 85f64.to_radians()) as f32; // from vertical
            let dir = up * angle.cos() + heading * angle.sin();
            // Never point far from where we're going once moving fast (less drag, no tumbling).
            let vel = f.linear.normalize_or_zero();
            let dir = if f.linear.length() > 200.0 && f.air.density > 0.01 {
                (dir + vel * 0.5).normalize_or_zero()
            } else {
                dir
            };
            aim(c, dir, -heading);
            c.throttle = 1.0;
            let reached = if planet {
                tel.apoapsis_m >= target_altitude
            } else {
                alt >= target_altitude
            };
            if reached {
                ap.set_phase(if planet { "coast" } else { "cruise" });
            }
        }
        "coast" => {
            ship.assist.mode = AssistMode::Rotation;
            c.throttle = 0.0;
            let horizontal = (f.linear - up * f.linear.dot(up)).normalize_or_zero();
            aim(c, horizontal, up);
            // Burn when near apoapsis (climb rate small) — and keep apoapsis up if drag ate it.
            if tel.apoapsis_m < target_altitude * 0.98 {
                c.throttle = 0.3;
            }
            if tel.vertical_speed < 40.0 && alt > target_altitude * 0.8 {
                ap.set_phase("circularize");
            }
        }
        "circularize" => {
            ship.assist.mode = AssistMode::Rotation;
            let horizontal = (f.linear - up * f.linear.dot(up)).normalize_or_zero();
            // Pitch up a little if falling, down if still climbing.
            let dir = (horizontal + up * (-tel.vertical_speed * 0.01).clamp(-0.3, 0.3))
                .normalize_or_zero();
            aim(c, dir, up);
            let pe_short = (target_altitude * 0.9 - tel.periapsis_m).max(0.0);
            c.throttle = (pe_short / 20_000.0).clamp(0.05, 1.0) as f32;
            if tel.periapsis_m >= target_altitude * 0.9 {
                ap.set_phase("orbit");
            }
        }
        "orbit" => {
            ship.assist.mode = AssistMode::Rotation;
            c.throttle = 0.0;
            let horizontal = (f.linear - up * f.linear.dot(up)).normalize_or_zero();
            aim(c, horizontal, up);
        }
        _ => {
            // Flat world "cruise": hover at altitude, fly level along the heading.
            ship.assist.mode = AssistMode::Hover;
            let alt_err = (target_altitude - alt) as f32;
            aim(c, heading, up);
            c.lift = (alt_err * 0.02 - tel.vertical_speed * 0.1).clamp(-1.0, 1.0);
            c.throttle = if tel.forward_speed < 250.0 { 0.4 } else { 0.0 };
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn hold(
    c: &mut VehicleControls,
    forces: &VehicleForces,
    ship: &mut Spacecraft,
    integral: &mut Vec3,
    target: DVec3,
    up: Vec3,
    max_speed: f32,
    dt: f32,
) {
    let f = &forces.frame;
    ship.assist.mode = AssistMode::Hover;
    let err = (target - f.com).as_vec3();
    // Integrate the sideways error, so a steady wind can't hold the ship off its spot.
    let sideways = err - up * err.dot(up);
    *integral = (*integral + sideways * dt).clamp_length_max(60.0);
    let v_want = (err * 0.4 + *integral * 0.05).clamp_length_max(max_speed);
    // Close the velocity gap in about 0.8 s with whatever thrust each direction has.
    let accel = f.rotation.inverse() * (v_want - f.linear) / 0.8;
    let cap = crate::ship::axis_capacity(&ship.thrusters);
    let input = |a: f32, pos: usize| {
        let most = if a >= 0.0 { cap[pos] } else { cap[pos + 1] };
        (a * f.mass / most.max(1.0)).clamp(-1.0, 1.0)
    };
    c.strafe = input(accel.x, 0);
    c.lift = input(accel.y, 2);
    // + backward (brake), − forward (throttle).
    let back = input(accel.z, 4);
    c.throttle = (-back).max(0.0);
    c.brake = back.max(0.0);
    // Keep level, nose where it points.
    let nose = (f.forward() - up * f.forward().dot(up)).normalize_or_zero();
    let (p, y, r) = point_at(forces, nose, up, ship.assist.max_rate_deg_s, 1.0);
    c.pitch = p;
    c.yaw = y;
    c.steer = r;
}

#[allow(clippy::too_many_arguments)]
fn hop(
    ap: &mut Autopilot,
    c: &mut VehicleControls,
    forces: &VehicleForces,
    tel: &VehicleTelemetry,
    wheels: Option<&Wheels>,
    aircraft: Option<Mut<Aircraft>>,
    ship: &mut Spacecraft,
    pad: DVec3,
    height: f32,
    hover_s: f32,
    rest_s: f32,
    up: Vec3,
) {
    let f = &forces.frame;
    let dt = f.dt;
    let on_ground = wheels.is_some_and(|w| w.grounded() > 0);
    // Gear: tucked away while hovering high, out again before coming down.
    let mut aircraft = aircraft;
    let gear_out = aircraft.as_ref().map_or(1.0, |a| a.gear);
    let mut gear = |down: bool| {
        if let Some(a) = aircraft.as_mut()
            && a.gear_down != down
        {
            a.gear_down = down;
        }
    };
    if ap.phase.is_empty() && ap.timer < 0.1 {
        // Wheel contacts aren't known until the wheels have run once.
        return;
    }
    // Height of the center of mass above the pad when resting on the gear.
    let rest_height = *ap.start_altitude.get_or_insert_with(|| {
        if on_ground {
            (f.com - pad).dot(up.as_dvec3()).max(0.5)
        } else {
            3.0
        }
    });
    let above = (f.com - pad).dot(up.as_dvec3());
    // Where the center of mass sits with the ship's origin on the pad, so it always comes
    // back down where it stood.
    let lean = f.com - f.position;
    let spot = pad + lean - up.as_dvec3() * lean.dot(up.as_dvec3());
    if ap.phase.is_empty() {
        ap.set_phase(if on_ground { "on the pad" } else { "hover" });
    }
    let idle = |c: &mut VehicleControls, ship: &mut Spacecraft| {
        ship.assist.mode = AssistMode::Off;
        *c = VehicleControls::default();
        // Parking brake (the foot brake would fire the retro thrusters).
        c.handbrake = true;
    };
    match ap.phase.as_str() {
        "on the pad" => {
            gear(true);
            idle(c, ship);
            if on_ground && ap.timer > 0.5 && ap.timer < 1.0 {
                // Settled on the gear: remember how high the ship sits.
                ap.start_altitude = Some(above.max(0.5));
            }
            if ap.timer > rest_s {
                ap.set_phase("lift off");
            }
        }
        "lift off" | "hover" => {
            let target = spot + (up * (height + rest_height as f32)).as_dvec3();
            hold(c, forces, ship, &mut ap.hold_integral, target, up, 12.0, dt);
            if ap.phase == "lift off" && (above - rest_height) > (height * 0.8) as f64 {
                ap.set_phase("hover");
            }
            if ap.phase == "hover" && height > 10.0 {
                gear(false);
            }
            if ap.phase == "hover" && ap.timer > hover_s {
                ap.set_phase("landing");
            }
        }
        "landing" => {
            let over_pad = spot + up.as_dvec3() * above;
            hold(
                c,
                forces,
                ship,
                &mut ap.hold_integral,
                over_pad,
                up,
                12.0,
                dt,
            );
            // Come down fast high up, gently near the pad; wait a few meters up for the
            // gear to come all the way down.
            gear(true);
            let h = (above - rest_height) as f32;
            let sink = if gear_out < 1.0 && h < 5.0 {
                0.0
            } else {
                (h * 0.35).clamp(0.6, 8.0)
            };
            let v_up = tel.vertical_speed;
            let dv = -sink - v_up;
            c.lift = (dv * 0.4).clamp(-1.0, 1.0);
            if on_ground && v_up.abs() < 1.0 {
                ap.set_phase("on the pad");
            }
        }
        _ => ap.set_phase("hover"),
    }
}
