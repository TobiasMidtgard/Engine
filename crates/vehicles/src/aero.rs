//! Aircraft: lifting surfaces with real lift, drag and pitching-moment curves (including
//! stall and the flat-plate regime past it), control surfaces, and engines.
//!
//! Each [`AeroSurface`] is a flat panel (a wing half, a tailplane, a fin) with a chord and a
//! span. Every step it sees the local airflow (body velocity at its center, minus wind),
//! works out its angle of attack and pushes with lift and drag at its aerodynamic center.
//! Nothing else makes a plane fly: stability, stalls, spins, the need for a long runway on a
//! hot day and a glider's long glide all come out of these panels.
//!
//! The coefficient model follows Khan & Nahon, "Real-time modeling of agile fixed-wing UAV
//! aerodynamics" (2015): thin-airfoil lift with a finite-wing correction, flap effectiveness
//! from the flap's chord fraction, and a smooth blend into post-stall flat-plate behavior.

use engine_core::math::{Quat, Vec3, lerp};
use engine_core::prelude::*;
use engine_world::VehicleControls;

use crate::chassis::{VehicleForces, VehicleTelemetry};

/// Which control input moves a control surface.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ControlAxis {
    /// `VehicleControls::pitch` (elevator; + = nose up).
    Pitch,
    /// `VehicleControls::steer` (ailerons; + = roll right).
    Roll,
    /// `VehicleControls::yaw` (rudder; + = nose right).
    Yaw,
    /// `Aircraft::flaps` (0..1).
    Flaps,
}

/// A control surface's hinge: how an input moves it.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct SurfaceControl {
    pub axis: ControlAxis,
    /// Deflection per unit input; the sign picks the direction (left and right ailerons
    /// have opposite signs). + = trailing edge toward the surface's −up side (more lift).
    pub gain: f32,
    /// Deflection at full input, degrees.
    pub max_deg: f32,
    /// How fast the surface moves, degrees per second.
    pub rate_deg_s: f32,
}

impl Default for SurfaceControl {
    fn default() -> Self {
        Self {
            axis: ControlAxis::Pitch,
            gain: 1.0,
            max_deg: 25.0,
            rate_deg_s: 120.0,
        }
    }
}

/// Live state of one surface.
#[derive(Debug, Clone, Copy, PartialEq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct SurfaceState {
    /// Flap / control deflection, rad (+ = more lift).
    pub deflection: f32,
    pub aoa: f32,
    pub lift_n: f32,
    pub drag_n: f32,
    pub stalled: bool,
}

/// One lifting surface. In its local frame (from `rotation`) the chord points forward
/// along −Z, the span along +X and lift is positive toward +Y.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct AeroSurface {
    pub name: String,
    /// Aerodynamic center (≈ quarter chord), body local, meters.
    pub position: Vec3,
    /// Orientation in the body. Identity = a horizontal wing; a vertical fin is rotated 90°
    /// about −Z so its lift points sideways.
    pub rotation: Quat,
    pub chord: f32,
    pub span: f32,
    /// Effective aspect ratio; `None` = span / chord. A wing half attached to the fuselage
    /// acts like a full wing: use 2 × span / chord.
    pub aspect_ratio: Option<f32>,
    /// Lift curve slope of the airfoil, per radian (thin airfoil theory: 2π).
    pub lift_slope: f32,
    pub skin_friction: f32,
    /// Angle of attack with zero lift, degrees (cambered airfoils ≈ −2 … −4).
    pub zero_lift_aoa_deg: f32,
    /// Stall angles (without flap deflection), degrees.
    pub stall_high_deg: f32,
    pub stall_low_deg: f32,
    /// Fraction of the chord that is a movable flap or control surface (0 = none).
    pub flap_fraction: f32,
    pub control: Option<SurfaceControl>,
    /// A second input mixed onto the same surface (flaperons: roll + flaps).
    pub control2: Option<SurfaceControl>,
    #[serde(skip)]
    pub state: SurfaceState,
}

impl Default for AeroSurface {
    fn default() -> Self {
        Self {
            name: "surface".into(),
            position: Vec3::ZERO,
            rotation: Quat::IDENTITY,
            chord: 1.5,
            span: 5.0,
            aspect_ratio: None,
            lift_slope: std::f32::consts::TAU,
            skin_friction: 0.02,
            zero_lift_aoa_deg: 0.0,
            stall_high_deg: 15.0,
            stall_low_deg: -15.0,
            flap_fraction: 0.0,
            control: None,
            control2: None,
            state: SurfaceState::default(),
        }
    }
}

/// Lift, drag and pitching-moment coefficients at one angle of attack.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Coefficients {
    pub lift: f32,
    pub drag: f32,
    /// About the aerodynamic center, + = leading edge up.
    pub moment: f32,
    pub stalled: bool,
}

impl AeroSurface {
    pub fn area(&self) -> f32 {
        self.chord * self.span
    }

    fn aspect(&self) -> f32 {
        self.aspect_ratio
            .unwrap_or(self.span / self.chord.max(1e-3))
            .max(0.1)
    }

    /// Coefficients at angle of attack `aoa` (rad) with flap deflection `flap` (rad).
    pub fn coefficients(&self, aoa: f32, flap: f32) -> Coefficients {
        use std::f32::consts::{FRAC_PI_2, PI};
        let ar = self.aspect();
        let slope = self.lift_slope * ar / (ar + 2.0 * (ar + 4.0) / (ar + 2.0));
        let zero_base = self.zero_lift_aoa_deg.to_radians();
        let (high_base, low_base) = (
            self.stall_high_deg.to_radians(),
            self.stall_low_deg.to_radians(),
        );
        let ff = self.flap_fraction.clamp(0.0, 0.9);
        let (zero, high, low) = if ff > 0.0 {
            let theta = (2.0 * ff - 1.0).acos();
            let effectiveness = 1.0 - (theta - theta.sin()) / PI;
            let correction =
                lerp(0.8, 0.4, (flap.to_degrees().abs() - 10.0) / 50.0).clamp(0.4, 0.8);
            let delta_lift = slope * effectiveness * correction * flap;
            let zero = zero_base - delta_lift / slope;
            let max_fraction = (1.0 - 0.5 * (ff - 0.1) / 0.3).clamp(0.0, 1.0);
            let cl_high = slope * (high_base - zero_base) + delta_lift * max_fraction;
            let cl_low = slope * (low_base - zero_base) + delta_lift * max_fraction;
            (zero, zero + cl_high / slope, zero + cl_low / slope)
        } else {
            (zero_base, high_base, low_base)
        };
        let flap_deg = flap.to_degrees();
        let pad_high = lerp(15.0, 5.0, ((flap_deg + 50.0) / 100.0).clamp(0.0, 1.0)).to_radians();
        let pad_low = lerp(15.0, 5.0, ((-flap_deg + 50.0) / 100.0).clamp(0.0, 1.0)).to_radians();
        let (padded_high, padded_low) = (high + pad_high, low - pad_low);

        let torque_prop = |eff: f32| 0.25 - 0.175 * (1.0 - 2.0 * eff.abs() / PI);
        let friction_90 = 1.98 - 4.26e-2 * flap * flap + 2.1e-1 * flap;

        let attached = |aoa: f32| {
            let cl = slope * (aoa - zero);
            let induced = cl / (PI * ar);
            let eff = aoa - zero - induced;
            let ct = self.skin_friction * eff.cos();
            let cn = (cl + eff.sin() * ct) / eff.cos();
            Coefficients {
                lift: cl,
                drag: cn * eff.sin() + ct * eff.cos(),
                moment: -cn * torque_prop(eff),
                stalled: false,
            }
        };
        let separated = |aoa: f32| {
            let (cl_low_aoa, lerp_param) = if aoa > high {
                (
                    slope * (high - zero),
                    (FRAC_PI_2 - aoa.clamp(-FRAC_PI_2, FRAC_PI_2)) / (FRAC_PI_2 - high),
                )
            } else {
                (
                    slope * (low - zero),
                    (-FRAC_PI_2 - aoa.clamp(-FRAC_PI_2, FRAC_PI_2)) / (-FRAC_PI_2 - low),
                )
            };
            let induced = lerp(0.0, cl_low_aoa / (PI * ar), lerp_param.clamp(0.0, 1.0));
            let eff = aoa - zero - induced;
            let cn = friction_90 * eff.sin() * (1.0 / (0.56 + 0.44 * eff.sin().abs()));
            let ct = 0.5 * self.skin_friction * eff.cos();
            Coefficients {
                lift: cn * eff.cos() - ct * eff.sin(),
                drag: cn * eff.sin() + ct * eff.cos(),
                moment: -cn * torque_prop(eff),
                stalled: true,
            }
        };
        if aoa > low && aoa < high {
            attached(aoa)
        } else if aoa > padded_high || aoa < padded_low {
            separated(aoa)
        } else {
            // Blend through the stall so lift never jumps.
            let (a, b, t) = if aoa > high {
                (
                    attached(high),
                    separated(padded_high),
                    (aoa - high) / (padded_high - high),
                )
            } else {
                (
                    attached(low),
                    separated(padded_low),
                    (low - aoa) / (low - padded_low),
                )
            };
            let blend = |x: f32, y: f32| x + (y - x) * t;
            Coefficients {
                lift: blend(a.lift, b.lift),
                drag: blend(a.drag, b.drag),
                moment: blend(a.moment, b.moment),
                stalled: true,
            }
        }
    }
}

/// An aircraft: its lifting surfaces plus flap setting and trim.
#[derive(Component, Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Aircraft {
    pub surfaces: Vec<AeroSurface>,
    /// Flap setting, 0 (up) .. 1 (landing).
    pub flaps: f32,
    /// Pitch trim added to the pitch input (−1..1).
    pub pitch_trim: f32,
    /// Landing gear extended (drawing; wheels still need `Wheels` to touch down).
    pub gear_down: bool,
    /// How far the gear is out, 0 (stowed) .. 1 (down): runs toward `gear_down` over
    /// [`GEAR_TRAVEL_S`], doors first, then legs.
    pub gear: f32,
}

/// Seconds for the landing gear to go all the way up or down.
pub const GEAR_TRAVEL_S: f32 = 6.0;

impl Default for Aircraft {
    fn default() -> Self {
        Self {
            surfaces: Vec::new(),
            flaps: 0.0,
            pitch_trim: 0.0,
            gear_down: true,
            gear: 1.0,
        }
    }
}

impl Aircraft {
    /// Total lifting area, m².
    pub fn wing_area(&self) -> f32 {
        self.surfaces
            .iter()
            .filter(|s| s.rotation.mul_vec3(Vec3::Y).y.abs() > 0.7)
            .map(|s| s.area())
            .sum()
    }
}

/// Move control surfaces and push the body with every surface's lift and drag.
pub fn aero_surfaces_system(
    mut q: Query<(
        &mut Aircraft,
        &VehicleControls,
        &mut VehicleForces,
        &mut VehicleTelemetry,
    )>,
) {
    q.par_iter_mut()
        .for_each(|(mut aircraft, controls, mut forces, mut tel)| {
            let f = forces.frame;
            let dt = f.dt;
            let target = if aircraft.gear_down { 1.0 } else { 0.0 };
            let step = dt / GEAR_TRAVEL_S;
            let gear = aircraft.gear + (target - aircraft.gear).clamp(-step, step);
            if gear != aircraft.gear {
                aircraft.gear = gear;
            }
            let flaps = aircraft.flaps.clamp(0.0, 1.0);
            let pitch = (controls.pitch + aircraft.pitch_trim).clamp(-1.0, 1.0);
            let mut stalled = false;
            for s in &mut aircraft.surfaces {
                let input = |c: &SurfaceControl| {
                    let v = match c.axis {
                        ControlAxis::Pitch => pitch,
                        ControlAxis::Roll => controls.steer.clamp(-1.0, 1.0),
                        ControlAxis::Yaw => controls.yaw.clamp(-1.0, 1.0),
                        ControlAxis::Flaps => flaps,
                    };
                    (
                        v * c.gain * c.max_deg.to_radians(),
                        c.rate_deg_s.to_radians(),
                    )
                };
                let mut target = 0.0;
                let mut rate = f32::MAX;
                for c in [s.control, s.control2].iter().flatten() {
                    let (t, r) = input(c);
                    target += t;
                    rate = rate.min(r);
                }
                let limit = s
                    .control
                    .map(|c| c.max_deg.to_radians())
                    .unwrap_or(0.0)
                    .max(s.control2.map(|c| c.max_deg.to_radians()).unwrap_or(0.0));
                let target = target.clamp(-limit.max(0.0), limit.max(0.0));
                let step = if rate == f32::MAX {
                    f32::MAX
                } else {
                    rate * dt
                };
                s.state.deflection += (target - s.state.deflection).clamp(-step, step);

                if f.air.density <= 0.0 {
                    s.state.lift_n = 0.0;
                    s.state.drag_n = 0.0;
                    continue;
                }
                let rot = f.rotation * s.rotation;
                let at = f.to_world(s.position);
                let u = f.point_velocity(at) - f.air.wind;
                let local = rot.inverse() * u;
                // Airflow in the chord plane only: spanwise flow makes no lift.
                let chordwise = Vec3::new(0.0, local.y, local.z);
                let speed = chordwise.length();
                if speed < 0.5 {
                    continue;
                }
                let aoa = (-local.y).atan2(-local.z);
                let c = s.coefficients(aoa, s.state.deflection);
                let q = 0.5 * f.air.density * speed * speed * s.area();
                let drag_dir = -chordwise / speed;
                let lift_dir = drag_dir.cross(Vec3::X);
                let force_local = lift_dir * (c.lift * q) + drag_dir * (c.drag * q);
                let torque_local = Vec3::X * (c.moment * q * s.chord);
                forces.add_at(rot * force_local, at);
                forces.add_torque(rot * torque_local);
                s.state.aoa = aoa;
                s.state.lift_n = c.lift * q;
                s.state.drag_n = c.drag * q;
                s.state.stalled = c.stalled && speed > 5.0;
                stalled |= s.state.stalled && s.area() > 1.0;
            }
            tel.stalled = stalled;
        });
}

/// What kind of engine a [`Propulsor`] is.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub enum PropulsorKind {
    /// Thrust falls linearly from `max_thrust_n` at rest to zero at `zero_thrust_speed`
    /// (m/s airspeed): a fixed-pitch propeller.
    Propeller { zero_thrust_speed: f32 },
    /// Roughly constant thrust that scales with air density (turbofans, turbojets).
    Jet,
    /// Thrust independent of air (a little higher in vacuum).
    Rocket { vacuum_thrust_n: f32 },
}

/// One engine pushing along `direction`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Propulsor {
    pub name: String,
    /// Where thrust acts, body local.
    pub position: Vec3,
    /// Thrust direction, body local (−Z = forward).
    pub direction: Vec3,
    /// Static sea-level thrust, N.
    pub max_thrust_n: f32,
    pub kind: PropulsorKind,
    /// Seconds to spool from idle to full (engines don't respond instantly).
    pub spool_secs: f32,
    /// Thrust at zero throttle as a fraction (idle).
    pub idle: f32,
    /// Current spool, 0..1.
    pub level: f32,
    /// Current thrust, N.
    pub thrust_n: f32,
    /// Spin angle for drawing a propeller, rad.
    #[serde(skip)]
    pub spin_angle: f32,
}

impl Default for Propulsor {
    fn default() -> Self {
        Self {
            name: "engine".into(),
            position: Vec3::ZERO,
            direction: Vec3::NEG_Z,
            max_thrust_n: 2_500.0,
            kind: PropulsorKind::Propeller {
                zero_thrust_speed: 80.0,
            },
            spool_secs: 1.0,
            idle: 0.05,
            level: 0.0,
            thrust_n: 0.0,
            spin_angle: 0.0,
        }
    }
}

/// Engines driven by `VehicleControls::throttle` (`boost` adds 30 %, an afterburner).
#[derive(Component, Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Propulsion {
    pub engines: Vec<Propulsor>,
}

/// Spool engines and push.
pub fn propulsion_system(
    mut q: Query<(
        &mut Propulsion,
        &VehicleControls,
        &mut VehicleForces,
        &mut VehicleTelemetry,
    )>,
) {
    q.par_iter_mut()
        .for_each(|(mut prop, controls, mut forces, mut tel)| {
            let f = forces.frame;
            let dt = f.dt;
            let throttle = controls.throttle.clamp(0.0, 1.0);
            let boost = if controls.boost { 1.3 } else { 1.0 };
            let density_ratio = f.air.density / 1.225;
            let airspeed = f.airspeed_vector().dot(f.forward()).max(0.0);
            for e in &mut prop.engines {
                let target = e.idle + (1.0 - e.idle) * throttle;
                let rate = dt / e.spool_secs.max(0.01);
                e.level += (target - e.level).clamp(-rate, rate);
                let base = match e.kind {
                    PropulsorKind::Propeller { zero_thrust_speed } => {
                        e.max_thrust_n
                            * (1.0 - airspeed / zero_thrust_speed.max(1.0)).max(0.0)
                            * density_ratio
                    }
                    PropulsorKind::Jet => e.max_thrust_n * density_ratio.min(1.2),
                    PropulsorKind::Rocket { vacuum_thrust_n } => {
                        let p = (f.air.pressure_pa / 101_325.0).clamp(0.0, 1.0);
                        e.max_thrust_n * p + vacuum_thrust_n.max(e.max_thrust_n) * (1.0 - p)
                    }
                };
                let afterburner = if matches!(e.kind, PropulsorKind::Jet) {
                    boost
                } else {
                    1.0
                };
                e.thrust_n = base * e.level * afterburner;
                e.spin_angle = (e.spin_angle + e.level * 200.0 * dt) % std::f32::consts::TAU;
                let dir = f.rotation * e.direction.normalize_or_zero();
                let at = f.to_world(e.position);
                forces.add_at(dir * e.thrust_n, at);
            }
            if let Some(e) = prop.engines.first() {
                tel.throttle = e.level;
            }
        });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn wing() -> AeroSurface {
        AeroSurface {
            chord: 1.5,
            span: 5.0,
            aspect_ratio: Some(7.0),
            zero_lift_aoa_deg: -2.0,
            flap_fraction: 0.25,
            ..Default::default()
        }
    }

    #[test]
    fn lift_rises_then_stalls() {
        let w = wing();
        let c0 = w.coefficients(0.0, 0.0);
        assert!(c0.lift > 0.1, "cambered wing lifts at 0°: {}", c0.lift);
        let c10 = w.coefficients(10f32.to_radians(), 0.0);
        assert!(c10.lift > 0.9 && c10.lift < 1.3, "{}", c10.lift);
        let peak = (0..40)
            .map(|d| w.coefficients((d as f32).to_radians(), 0.0).lift)
            .fold(0.0, f32::max);
        let deep = w.coefficients(35f32.to_radians(), 0.0);
        assert!(deep.stalled);
        assert!(
            deep.lift < peak * 0.9,
            "lift drops past the stall: {} vs {peak}",
            deep.lift
        );
        assert!(deep.drag > c10.drag * 3.0, "stalled drag soars");
        assert!(c10.moment < 0.0, "pitch-down moment keeps it stable");
    }

    #[test]
    fn flaps_add_lift_and_lower_the_stall_angle() {
        let w = wing();
        let clean = w.coefficients(5f32.to_radians(), 0.0);
        let flaps = w.coefficients(5f32.to_radians(), 20f32.to_radians());
        assert!(
            flaps.lift > clean.lift + 0.3,
            "{} vs {}",
            flaps.lift,
            clean.lift
        );
        assert!(flaps.drag > clean.drag);
        let up = w.coefficients(5f32.to_radians(), -20f32.to_radians());
        assert!(up.lift < clean.lift - 0.3);
    }

    #[test]
    fn negative_aoa_is_symmetric_for_a_flat_plate() {
        let w = AeroSurface::default();
        let a = w.coefficients(0.1, 0.0);
        let b = w.coefficients(-0.1, 0.0);
        assert!((a.lift + b.lift).abs() < 1e-4);
        assert!((a.drag - b.drag).abs() < 1e-4);
    }

    #[test]
    fn coefficients_are_finite_all_the_way_around() {
        let w = wing();
        for d in -180..=180 {
            let c = w.coefficients((d as f32).to_radians(), 0.3);
            assert!(
                c.lift.is_finite() && c.drag.is_finite() && c.moment.is_finite(),
                "{d}°"
            );
            assert!(
                c.drag >= -1e-3,
                "drag never pulls forward at {d}°: {}",
                c.drag
            );
        }
    }
}
