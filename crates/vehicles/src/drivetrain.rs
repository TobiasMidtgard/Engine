//! Engine → clutch → gearbox → differentials → wheels.
//!
//! The engine makes torque from a dyno-style curve ([`EngineSpec::torque_curve`]), the clutch
//! couples it to the gearbox (an automatic, centrifugal-style clutch that slips at launch so
//! the engine never stalls), the gearbox multiplies it by the gear and final-drive ratios, and
//! each driven axle's differential splits it between its two wheels. The wheels push back
//! through the same chain, so wheelspin revs the engine, engine braking slows the car and a
//! downshift at speed blips the revs up.
//!
//! The car's wheel system calls [`Drivetrain::substep`] several times per physics step.

use engine_core::prelude::*;
use engine_world::EngineKind;

/// rad/s per rpm.
pub const RPM_TO_RAD: f32 = std::f32::consts::TAU / 60.0;

/// The engine.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct EngineSpec {
    /// Full-throttle torque curve, `[rpm, N·m]` pairs in rising rpm order (net, at the
    /// flywheel). Linear between points.
    pub torque_curve: Vec<[f32; 2]>,
    pub idle_rpm: f32,
    /// Fuel is cut above this.
    pub redline_rpm: f32,
    /// Rotating inertia of the engine and flywheel, kg·m².
    pub inertia: f32,
    /// Engine braking torque at the redline with the throttle closed, N·m.
    pub engine_brake_nm: f32,
    /// What it sounds like (written to [`EngineState`](engine_world::EngineState)).
    pub sound: EngineKind,
    /// Turbocharged: boost builds with rpm and load.
    pub turbo: bool,
}

impl Default for EngineSpec {
    fn default() -> Self {
        Self {
            torque_curve: vec![
                [800.0, 220.0],
                [2_000.0, 330.0],
                [4_500.0, 420.0],
                [6_200.0, 380.0],
                [7_200.0, 300.0],
            ],
            idle_rpm: 850.0,
            redline_rpm: 7_200.0,
            inertia: 0.25,
            engine_brake_nm: 80.0,
            sound: EngineKind::Piston { cylinders: 6 },
            turbo: false,
        }
    }
}

impl EngineSpec {
    /// Full-throttle torque at `rpm`, N·m.
    pub fn max_torque_at(&self, rpm: f32) -> f32 {
        let c = &self.torque_curve;
        match c.len() {
            0 => 0.0,
            1 => c[0][1],
            _ => {
                if rpm <= c[0][0] {
                    return c[0][1] * (rpm / c[0][0].max(1.0)).clamp(0.0, 1.0);
                }
                for w in c.windows(2) {
                    if rpm <= w[1][0] {
                        let t = (rpm - w[0][0]) / (w[1][0] - w[0][0]).max(1e-3);
                        return w[0][1] + (w[1][1] - w[0][1]) * t;
                    }
                }
                // Past the end: fall off quickly.
                let last = c[c.len() - 1];
                (last[1] * (1.0 - (rpm - last[0]) / 1_000.0)).max(0.0)
            }
        }
    }

    /// Net engine torque at `rpm` and `throttle` (0..1), including engine braking, N·m.
    pub fn torque(&self, rpm: f32, throttle: f32) -> f32 {
        let throttle = if rpm >= self.redline_rpm {
            0.0
        } else {
            throttle.clamp(0.0, 1.0)
        };
        let braking = self.engine_brake_nm * (rpm / self.redline_rpm.max(1.0)).clamp(0.05, 1.2);
        throttle * self.max_torque_at(rpm) - (1.0 - throttle) * braking
    }

    /// Peak power, watts, and the rpm it happens at.
    pub fn peak_power(&self) -> (f32, f32) {
        let mut best = (0.0, 0.0);
        let mut rpm = self.idle_rpm;
        while rpm <= self.redline_rpm {
            let p = self.max_torque_at(rpm) * rpm * RPM_TO_RAD;
            if p > best.0 {
                best = (p, rpm);
            }
            rpm += 50.0;
        }
        best
    }
}

/// The gearbox.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Gearbox {
    /// Forward gear ratios, first gear first.
    pub ratios: Vec<f32>,
    /// Reverse ratio (positive number; the sign is applied for you).
    pub reverse: f32,
    pub final_drive: f32,
    /// Fraction of torque that reaches the wheels.
    pub efficiency: f32,
    /// Shift automatically (players and AI only give throttle and brake).
    pub automatic: bool,
    /// Seconds with the clutch open during a shift.
    pub shift_time: f32,
    /// Shift up above this rpm…
    pub upshift_rpm: f32,
    /// …and down below this one.
    pub downshift_rpm: f32,
    /// Torque the clutch can carry, N·m.
    pub clutch_torque_nm: f32,
    /// The clutch starts to bite at idle and is fully closed at this rpm (launch control).
    pub launch_rpm: f32,
}

impl Default for Gearbox {
    fn default() -> Self {
        Self {
            ratios: vec![3.6, 2.2, 1.55, 1.2, 0.97, 0.8],
            reverse: 3.4,
            final_drive: 3.7,
            efficiency: 0.9,
            automatic: true,
            shift_time: 0.18,
            upshift_rpm: 6_800.0,
            downshift_rpm: 3_000.0,
            clutch_torque_nm: 900.0,
            launch_rpm: 3_200.0,
        }
    }
}

/// How a differential shares torque between its two wheels.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub enum Differential {
    /// Equal torque to both wheels: a wheel in the air spins and the other gets nothing.
    Open,
    /// Limited slip: resists a speed difference with up to `preload_nm` plus `lock` × the
    /// torque going through it (0.3 … 0.6 for a sporty LSD). Helps traction and drifting.
    Limited { preload_nm: f32, lock: f32 },
    /// Both wheels turn together (karts, drift cars, off-roaders in low range).
    Locked,
}

/// A driven pair of wheels.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct DriveAxle {
    /// Indices into the vehicle's `Wheels::wheels`.
    pub left: usize,
    pub right: usize,
    /// Fraction of the gearbox output torque this axle gets (sums to 1 over all axles).
    pub split: f32,
    pub differential: Differential,
}

/// Driver-visible drivetrain state.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct DrivetrainState {
    pub rpm: f32,
    /// −1 reverse, 0 neutral, 1.. forward gears.
    pub gear: i32,
    /// Seconds left in the current shift.
    pub shifting: f32,
    /// 0 open .. 1 closed.
    pub clutch: f32,
    /// Torque the engine makes right now, N·m.
    pub engine_torque_nm: f32,
    /// Throttle actually applied after the rev limiter, shifts and traction control.
    pub throttle: f32,
    /// Seconds the brake has been held at a standstill (for brake-to-reverse).
    pub reverse_hold: f32,
    /// Seconds since the last gear change (keeps the automatic from hunting).
    pub since_shift: f32,
}

impl Default for DrivetrainState {
    fn default() -> Self {
        Self {
            rpm: 850.0,
            gear: 1,
            shifting: 0.0,
            clutch: 0.0,
            engine_torque_nm: 0.0,
            throttle: 0.0,
            reverse_hold: 0.0,
            since_shift: 0.0,
        }
    }
}

/// A complete drivetrain. Put it next to [`crate::Wheels`] on a car.
#[derive(Component, Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Drivetrain {
    pub engine: EngineSpec,
    pub gearbox: Gearbox,
    pub axles: Vec<DriveAxle>,
    /// Holding the brake at a standstill selects reverse, and then the brake pedal drives
    /// backwards (the usual game convention). Off: `brake` only brakes.
    pub brake_to_reverse: bool,
    pub state: DrivetrainState,
}

impl Default for Drivetrain {
    fn default() -> Self {
        Self {
            engine: EngineSpec::default(),
            gearbox: Gearbox::default(),
            axles: vec![DriveAxle {
                left: 2,
                right: 3,
                split: 1.0,
                differential: Differential::Limited {
                    preload_nm: 80.0,
                    lock: 0.35,
                },
            }],
            brake_to_reverse: true,
            state: DrivetrainState::default(),
        }
    }
}

impl Drivetrain {
    /// Total ratio between engine and wheels in the current gear (negative in reverse,
    /// 0 in neutral).
    pub fn ratio(&self) -> f32 {
        let g = &self.gearbox;
        match self.state.gear {
            0 => 0.0,
            n if n < 0 => -g.reverse * g.final_drive,
            n => g.ratios.get(n as usize - 1).copied().unwrap_or(0.0) * g.final_drive,
        }
    }

    /// Wheel spin (rad/s) at which the engine would be at `rpm` in gear `gear`.
    pub fn wheel_speed_for_rpm(&self, gear: i32, rpm: f32) -> f32 {
        let g = &self.gearbox;
        let ratio = match gear {
            0 => return 0.0,
            n if n < 0 => -g.reverse,
            n => g.ratios.get(n as usize - 1).copied().unwrap_or(1.0),
        } * g.final_drive;
        rpm * RPM_TO_RAD / ratio
    }

    /// Average spin of the driven wheels weighted by axle split, rad/s.
    pub fn driven_speed(&self, omega: &[f32]) -> f32 {
        let mut sum = 0.0;
        let mut weight = 0.0;
        for a in &self.axles {
            let (l, r) = (
                omega.get(a.left).copied().unwrap_or(0.0),
                omega.get(a.right).copied().unwrap_or(0.0),
            );
            sum += a.split * (l + r) * 0.5;
            weight += a.split;
        }
        if weight > 0.0 { sum / weight } else { 0.0 }
    }

    /// Choose the gear for this step: automatic shifting and brake-to-reverse.
    /// `pedal_throttle`/`pedal_brake` are the driver's pedals, `forward_speed` m/s.
    /// Returns `(throttle, brake)` after reverse swapping.
    pub fn select_gear(
        &mut self,
        pedal_throttle: f32,
        pedal_brake: f32,
        forward_speed: f32,
        driven_omega: f32,
        dt: f32,
    ) -> (f32, f32) {
        let st = &mut self.state;
        st.shifting = (st.shifting - dt).max(0.0);
        st.since_shift += dt;
        let mut throttle = pedal_throttle;
        let mut brake = pedal_brake;
        if self.brake_to_reverse {
            let stopped = forward_speed.abs() < 1.0;
            if st.gear >= 0 && stopped && pedal_brake > 0.2 && pedal_throttle < 0.05 {
                st.reverse_hold += dt;
                if st.reverse_hold > 0.35 {
                    st.gear = -1;
                    st.reverse_hold = 0.0;
                }
            } else if st.gear < 0 && stopped && pedal_throttle > 0.2 && pedal_brake < 0.05 {
                st.reverse_hold += dt;
                if st.reverse_hold > 0.2 {
                    st.gear = 1;
                    st.reverse_hold = 0.0;
                }
            } else {
                st.reverse_hold = 0.0;
            }
            if st.gear < 0 {
                // Reverse: the brake pedal drives, the throttle brakes.
                std::mem::swap(&mut throttle, &mut brake);
            }
        }
        if self.gearbox.automatic && st.gear > 0 && st.shifting <= 0.0 {
            let n = self.gearbox.ratios.len() as i32;
            // Engine speed the wheels would give in a gear (ignores clutch slip at launch).
            let rpm_in = |gear: i32| {
                let ratio = self.gearbox.ratios[gear as usize - 1] * self.gearbox.final_drive;
                driven_omega * ratio / RPM_TO_RAD
            };
            // No new shift right after one unless the engine is about to hit the limiter
            // or bog down: a car sliding or bouncing would otherwise hunt between gears.
            let settled = st.since_shift > 1.0;
            let rpm = rpm_in(st.gear);
            if st.gear < n
                && throttle > 0.1
                && (rpm > self.gearbox.upshift_rpm && settled
                    || rpm > self.engine.redline_rpm * 0.99)
            {
                // Over the limiter (a car thrown or towed faster than its gear allows), skip
                // straight to the gear that fits instead of one at a time, each held back by
                // engine braking hard enough to lock the driven wheels.
                let over = rpm > self.engine.redline_rpm * 0.99;
                st.gear += 1;
                while over && st.gear < n && rpm_in(st.gear) > self.gearbox.upshift_rpm {
                    st.gear += 1;
                }
                st.shifting = self.gearbox.shift_time;
                st.since_shift = 0.0;
            } else if st.gear > 1
                && (rpm < self.gearbox.downshift_rpm && settled || rpm < self.engine.idle_rpm * 1.5)
            {
                // Only drop a gear if the lower one won't over-rev.
                if rpm_in(st.gear - 1) < self.gearbox.upshift_rpm * 0.95 {
                    st.gear -= 1;
                    st.shifting = self.gearbox.shift_time * 0.7;
                    st.since_shift = 0.0;
                }
            }
        }
        (throttle, brake)
    }

    /// Select the forward gear an automatic would hold with the driven wheels turning at
    /// `driven_omega` (rad/s), spin the engine up to match and close the clutch: for a car
    /// that starts out moving (spawned or placed at speed), which would otherwise sit in
    /// first at idle and lock its driven wheels with engine braking.
    pub fn match_speed(&mut self, driven_omega: f32) {
        let gb = &self.gearbox;
        let rpm_in = |gear: usize| driven_omega * gb.ratios[gear - 1] * gb.final_drive / RPM_TO_RAD;
        let n = gb.ratios.len().max(1);
        let mut gear = 1;
        while gear < n && rpm_in(gear) > gb.upshift_rpm * 0.9 {
            gear += 1;
        }
        let st = &mut self.state;
        st.gear = gear as i32;
        st.rpm = if gb.ratios.is_empty() {
            self.engine.idle_rpm
        } else {
            rpm_in(gear).clamp(self.engine.idle_rpm, self.engine.redline_rpm)
        };
        st.clutch = 1.0;
        st.shifting = 0.0;
        st.since_shift = 0.0;
        st.reverse_hold = 0.0;
    }

    /// One substep of the coupled engine/clutch/wheel system.
    ///
    /// `omega` and `inertia` are every wheel's spin (rad/s) and spin inertia (kg·m²); the
    /// drive torque for each wheel (N·m, + = forward) is **added** to `torque_out`.
    /// `traction_limit` (traction control) caps the total torque sent to the wheels, N·m:
    /// the engine is throttled back and the clutch never passes more.
    pub fn substep(
        &mut self,
        throttle: f32,
        omega: &[f32],
        inertia: &[f32],
        dt: f32,
        traction_limit: Option<f32>,
        torque_out: &mut [f32],
    ) {
        let engine = &self.engine;
        let rpm_now = self.state.rpm;
        let mut w_e = rpm_now * RPM_TO_RAD;
        let shifting = self.state.shifting > 0.0;
        // Idle governor: open the throttle a little when the engine sags toward idle.
        let idle_throttle =
            ((engine.idle_rpm * 1.05 - rpm_now) / (engine.idle_rpm * 0.25)).clamp(0.0, 0.4);
        let ratio = self.ratio();
        let mut throttle = if shifting { 0.0 } else { throttle };
        // Traction control: back the throttle off to what the tires can put down.
        let eff = self.gearbox.efficiency;
        let wheel_cap = traction_limit.map(|l| l.max(0.0) / (ratio.abs() * eff).max(1e-3));
        if let Some(cap) = wheel_cap {
            let full = engine.torque(rpm_now, throttle).max(1e-3);
            if full > cap {
                throttle *= (cap / full).clamp(0.0, 1.0);
            }
        }
        let throttle = throttle.max(idle_throttle);
        let t_engine = engine.torque(rpm_now, throttle);
        self.state.engine_torque_nm = t_engine;
        self.state.throttle = throttle;

        let mut t_clutch = 0.0;
        if ratio != 0.0 && !shifting && !self.axles.is_empty() {
            let w_t = self.driven_speed(omega) * ratio;
            let i_wheels: f32 = self
                .axles
                .iter()
                .map(|a| {
                    inertia.get(a.left).copied().unwrap_or(1.0)
                        + inertia.get(a.right).copied().unwrap_or(1.0)
                })
                .sum::<f32>()
                .max(0.1);
            // Centrifugal-style engagement: slips from idle up to the launch rpm, but always
            // grips when the wheels drive the engine faster than idle (engine braking).
            // The bite point sits below the launch rpm so a full-throttle start holds the
            // engine near it instead of bogging down to idle.
            let launch = self.gearbox.launch_rpm.max(engine.idle_rpm + 200.0);
            let bite = (launch - 800.0).max(engine.idle_rpm + 50.0);
            let by_engine = ((rpm_now - bite) / (launch - bite)).clamp(0.0, 1.0);
            let by_wheels = ((w_t.abs() / RPM_TO_RAD - engine.idle_rpm) / 300.0).clamp(0.0, 1.0);
            let engagement = by_engine.max(by_wheels);
            self.state.clutch = engagement;
            let mut cap = self.gearbox.clutch_torque_nm * engagement;
            if let Some(limit) = wheel_cap {
                cap = cap.min(limit.max(5.0));
            }
            // Torque that makes engine and gearbox input turn at the same speed after this
            // substep (a friction clutch acts like this until it slips).
            let lock = ((w_e - w_t) / dt + t_engine / engine.inertia)
                / (1.0 / engine.inertia + ratio * ratio * eff / i_wheels);
            t_clutch = lock.clamp(-cap, cap);
            let t_out = t_clutch * ratio * eff;
            for a in &self.axles {
                let t_axle = t_out * a.split;
                let (wl, wr) = (
                    omega.get(a.left).copied().unwrap_or(0.0),
                    omega.get(a.right).copied().unwrap_or(0.0),
                );
                let (il, ir) = (
                    inertia.get(a.left).copied().unwrap_or(1.0),
                    inertia.get(a.right).copied().unwrap_or(1.0),
                );
                // Torque moved from the faster wheel to the slower one.
                let equalize = (wl - wr) / dt / (1.0 / il + 1.0 / ir);
                let transfer = match a.differential {
                    Differential::Open => 0.0,
                    Differential::Limited { preload_nm, lock } => {
                        let cap = preload_nm + lock * t_axle.abs();
                        equalize.clamp(-cap, cap)
                    }
                    Differential::Locked => equalize,
                };
                if let Some(t) = torque_out.get_mut(a.left) {
                    *t += t_axle * 0.5 - transfer;
                }
                if let Some(t) = torque_out.get_mut(a.right) {
                    *t += t_axle * 0.5 + transfer;
                }
            }
        } else {
            self.state.clutch = 0.0;
        }
        w_e += (t_engine - t_clutch) / engine.inertia * dt;
        // The engine never stalls (the clutch slips) and the limiter cuts fuel at redline.
        let w_min = engine.idle_rpm * 0.6 * RPM_TO_RAD;
        let w_max = engine.redline_rpm * 1.05 * RPM_TO_RAD;
        self.state.rpm = w_e.clamp(w_min, w_max) / RPM_TO_RAD;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn torque_curve_interpolates_and_limits() {
        let e = EngineSpec::default();
        assert!((e.max_torque_at(4_500.0) - 420.0).abs() < 1e-3);
        let mid = e.max_torque_at(3_250.0);
        assert!(mid > 330.0 && mid < 420.0);
        assert!(e.torque(7_300.0, 1.0) < 0.0, "rev limiter cuts fuel");
        assert!(e.torque(3_000.0, 0.0) < 0.0, "engine braking");
        let (power_w, rpm) = e.peak_power();
        let hp = power_w / 745.7;
        assert!(hp > 250.0 && hp < 400.0, "{hp} hp at {rpm}");
    }

    #[test]
    fn engine_revs_up_in_neutral_and_settles_at_idle() {
        let mut d = Drivetrain::default();
        d.state.gear = 0;
        let omega = [0.0; 4];
        let inertia = [1.0; 4];
        let mut out = [0.0; 4];
        for _ in 0..240 {
            d.substep(1.0, &omega, &inertia, 1.0 / 240.0, None, &mut out);
        }
        assert!(d.state.rpm > 6_500.0, "rpm {}", d.state.rpm);
        for _ in 0..2_400 {
            d.substep(0.0, &omega, &inertia, 1.0 / 240.0, None, &mut out);
        }
        let idle = d.engine.idle_rpm;
        assert!(
            (d.state.rpm - idle).abs() < idle * 0.15,
            "settles at idle: {}",
            d.state.rpm
        );
        assert_eq!(out, [0.0; 4], "neutral sends nothing to the wheels");
    }

    #[test]
    fn first_gear_drives_the_rear_wheels_forward() {
        let mut d = Drivetrain::default();
        d.state.rpm = 3_500.0;
        let omega = [0.0; 4];
        let inertia = [1.2; 4];
        let mut out = [0.0; 4];
        d.substep(1.0, &omega, &inertia, 1.0 / 240.0, None, &mut out);
        assert_eq!(out[0], 0.0);
        assert!(out[2] > 100.0 && out[3] > 100.0, "{out:?}");
        assert!((out[2] - out[3]).abs() < 1e-3, "equal speeds, equal torque");
    }

    #[test]
    fn limited_slip_feeds_the_slower_wheel() {
        let mut d = Drivetrain::default();
        d.state.rpm = 4_000.0;
        // Left rear spinning in the air, right rear gripping.
        let omega = [0.0, 0.0, 80.0, 20.0];
        let inertia = [1.2; 4];
        let mut out = [0.0; 4];
        d.substep(1.0, &omega, &inertia, 1.0 / 240.0, None, &mut out);
        assert!(out[3] > out[2], "{out:?}");
        d.axles[0].differential = Differential::Open;
        let mut open = [0.0; 4];
        d.substep(1.0, &omega, &inertia, 1.0 / 240.0, None, &mut open);
        assert!((open[2] - open[3]).abs() < 1e-3);
    }

    #[test]
    fn automatic_upshifts_and_brake_selects_reverse() {
        let mut d = Drivetrain::default();
        let w = d.wheel_speed_for_rpm(1, 7_000.0);
        // Past the shift point it changes up once it has been in gear a moment (no hunting).
        d.select_gear(1.0, 0.0, 15.0, w, 1.0 / 60.0);
        assert_eq!(d.state.gear, 1, "not straight after engaging");
        for _ in 0..90 {
            d.select_gear(1.0, 0.0, 15.0, w, 1.0 / 60.0);
            if d.state.gear != 1 {
                break;
            }
        }
        assert_eq!(d.state.gear, 2);
        assert!(d.state.shifting > 0.0);

        let mut d = Drivetrain::default();
        for _ in 0..40 {
            d.select_gear(0.0, 1.0, 0.0, 0.0, 1.0 / 60.0);
        }
        assert_eq!(d.state.gear, -1);
        let (throttle, brake) = d.select_gear(0.0, 1.0, -0.5, 0.0, 1.0 / 60.0);
        assert_eq!(
            (throttle, brake),
            (1.0, 0.0),
            "brake pedal drives in reverse"
        );
        assert!(d.ratio() < 0.0);
    }
}
