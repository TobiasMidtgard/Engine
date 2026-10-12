//! Tire forces from slip: a Pacejka "magic formula" per direction, combined slip on a
//! friction ellipse, load sensitivity and surface grip.
//!
//! A tire only pushes when it slips. *Slip ratio* κ is how much faster the tread moves than
//! the ground (wheelspin > 0, locked brakes = −1); *slip angle* α is the angle between where
//! the wheel points and where the contact patch actually travels. Each curve rises steeply,
//! peaks (around κ ≈ 0.1 and α ≈ 6–8°) and then falls off a little: that drop is what makes
//! a locked wheel or a drifting car lose grip and what makes drifting controllable.
//!
//! [`TireModel::forces`] combines both: under hard braking a tire has less sideways grip
//! left, which is why braking in a corner pushes the car wide.

use engine_core::prelude::*;
use engine_world::{Deformation, SurfaceCell, SurfaceMaterial};

/// One Pacejka curve, `sin(C·atan(B·s − E·(B·s − atan(B·s))))`, peaking at 1.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct MagicFormula {
    /// Stiffness: how fast force builds with slip.
    pub b: f32,
    /// Shape: how much force is left past the peak (1.3 lateral-ish … 1.9 longitudinal-ish).
    pub c: f32,
    /// Curvature near the peak (−1 … 1).
    pub e: f32,
}

impl Default for MagicFormula {
    fn default() -> Self {
        Self {
            b: 10.0,
            c: 1.9,
            e: 0.97,
        }
    }
}

impl MagicFormula {
    /// Normalized force (−1 … 1) at slip `s`.
    pub fn eval(&self, s: f32) -> f32 {
        let bs = self.b * s;
        (self.c * (bs - self.e * (bs - bs.atan())).atan()).sin()
    }

    /// Slip at which the curve peaks (searched numerically, 0 … 2).
    pub fn peak_slip(&self) -> f32 {
        // Golden-section search on a unimodal curve.
        let (mut a, mut b) = (1e-4f32, 2.0f32);
        let g = 0.618_034f32;
        let mut c = b - g * (b - a);
        let mut d = a + g * (b - a);
        for _ in 0..40 {
            if self.eval(c) > self.eval(d) {
                b = d;
            } else {
                a = c;
            }
            c = b - g * (b - a);
            d = a + g * (b - a);
        }
        (a + b) * 0.5
    }
}

/// How a tire grips. Defaults are a road-going performance tire.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct TireModel {
    /// Peak friction coefficient on dry asphalt (road tire ≈ 1.0, slick ≈ 1.5, winter ≈ 0.9).
    pub grip: f32,
    /// Force vs slip ratio.
    pub longitudinal: MagicFormula,
    /// Force vs slip angle (radians).
    pub lateral: MagicFormula,
    /// How much grip falls as load rises above `nominal_load_n` (0 = none, 0.1 typical).
    /// Load sensitivity is why weight transfer and anti-roll bars change the balance.
    pub load_sensitivity: f32,
    /// Load at which `grip` is measured, N.
    pub nominal_load_n: f32,
    /// Rolling resistance coefficient on hard ground (≈ 0.012); soft ground adds more.
    pub rolling_resistance: f32,
    /// How much the surface's friction matters (1 = fully; off-road tires use less, so
    /// mud and grass hurt them less).
    pub surface_sensitivity: f32,
}

impl Default for TireModel {
    fn default() -> Self {
        Self {
            grip: 1.05,
            // Peaks near 14 % slip; a locked or spinning wheel keeps ≈ 80 % of the peak.
            longitudinal: MagicFormula {
                b: 14.0,
                c: 1.5,
                e: 0.3,
            },
            // Peaks near 7.5° with a cornering stiffness of ≈ 18 × load per radian, and
            // keeps ≈ 85 % of the peak when sliding sideways.
            lateral: MagicFormula {
                b: 13.0,
                c: 1.4,
                e: -0.5,
            },
            load_sensitivity: 0.08,
            nominal_load_n: 4_000.0,
            rolling_resistance: 0.012,
            surface_sensitivity: 1.0,
        }
    }
}

/// Inputs and outputs of one tire evaluation.
#[derive(Debug, Clone, Copy, PartialEq, Default, Serialize, Deserialize)]
pub struct TireForces {
    /// Along the wheel's rolling direction, N (+ = forward).
    pub longitudinal: f32,
    /// Sideways, N (+ = toward the wheel's right).
    pub lateral: f32,
    /// 0 … 1+: fraction of available grip in use (> ~1 means sliding).
    pub grip_used: f32,
}

/// Reference friction of the surface table's paved surfaces (`concrete` ≈ 0.8,
/// `default` ≈ 0.7). Tire grip is quoted against this.
pub const REFERENCE_SURFACE_FRICTION: f32 = 0.75;

impl TireModel {
    /// Peak slip ratio and peak slip angle (rad) of this tire.
    pub fn peaks(&self) -> (f32, f32) {
        (
            self.longitudinal.peak_slip().max(0.02),
            self.lateral.peak_slip().max(0.02),
        )
    }

    /// Peak friction coefficient at a load, before surface effects.
    pub fn mu_at_load(&self, load_n: f32) -> f32 {
        let over = (load_n / self.nominal_load_n.max(1.0) - 1.0).max(-0.5);
        self.grip * (1.0 - self.load_sensitivity * over).clamp(0.3, 1.5)
    }

    /// Tire force for slip ratio `kappa`, slip angle `alpha` (rad, + = contact patch moving
    /// to the wheel's right) and vertical load `load_n`, with `surface_grip` the multiplier
    /// from [`surface_grip`]. `peaks` comes from [`TireModel::peaks`] (cache it).
    pub fn forces(
        &self,
        kappa: f32,
        alpha: f32,
        load_n: f32,
        surface_grip: f32,
        peaks: (f32, f32),
    ) -> TireForces {
        if load_n <= 0.0 {
            return TireForces::default();
        }
        let (k_peak, a_peak) = peaks;
        // Normalized slips: 1 = at the peak of that direction's curve.
        let sx = kappa / k_peak;
        let sy = alpha.clamp(-1.5, 1.5).tan() / a_peak.tan();
        let s = (sx * sx + sy * sy).sqrt();
        if s < 1e-6 {
            return TireForces::default();
        }
        let d = self.mu_at_load(load_n) * surface_grip * load_n;
        let fx = d * self.longitudinal.eval(s * k_peak) * (sx / s);
        let fy = -d * self.lateral.eval(s * a_peak) * (sy / s);
        TireForces {
            longitudinal: fx,
            lateral: fy,
            grip_used: s,
        }
    }
}

/// Grip multiplier for a surface material and its current state (wet, puddles, snow): 1 on
/// dry pavement, ≈ 0.75 wet, ≈ 0.4 on snow, ≈ 0.08 on ice.
///
/// `sensitivity` is [`TireModel::surface_sensitivity`]; `speed` (m/s) matters for
/// aquaplaning in deep puddles.
pub fn surface_grip(
    material: &SurfaceMaterial,
    cell: &SurfaceCell,
    sensitivity: f32,
    speed: f32,
) -> f32 {
    let base = material.friction / REFERENCE_SURFACE_FRICTION;
    // Off-road tires lose less on loose surfaces but never gain on ice.
    let mut grip = if base < 1.0 && material.friction > 0.2 {
        1.0 - (1.0 - base) * sensitivity
    } else {
        base
    };
    // Water on the surface: wet pavement keeps ~75 % grip; deep puddles at speed aquaplane.
    let wet = cell.wetness.clamp(0.0, 1.0) * (1.0 - material.absorbency * 0.5);
    grip *= 1.0 - 0.25 * wet;
    if cell.puddle_m > 0.002 {
        let depth = (cell.puddle_m / 0.01).min(1.0);
        let speed_factor = engine_core::math::smoothstep(15.0, 30.0, speed);
        grip *= 1.0 - 0.7 * depth * speed_factor;
    }
    // Snow on top: blend toward packed-snow grip as the layer deepens.
    if cell.snow_m > 0.0 {
        let cover = (cell.snow_m / 0.03).min(1.0);
        grip = grip + (0.45 - grip).min(0.0) * cover;
    }
    grip.clamp(0.02, 2.0)
}

/// Extra rolling resistance coefficient for soft ground: tires sink into mud, sand and snow.
pub fn sinkage_resistance(material: &SurfaceMaterial, cell: &SurfaceCell) -> f32 {
    let from_material = match material.deformation {
        Deformation::Plastic { depth_m } => depth_m * 2.5,
        Deformation::Granular { depth_m } => depth_m * 3.0,
        Deformation::Snow => 0.05,
        Deformation::Flatten { .. } => 0.02,
        Deformation::None => 0.0,
    };
    let snow = (cell.snow_m * 1.5).min(0.15);
    let mud = if material.absorbency > 0.5 {
        cell.wetness * 0.03
    } else {
        0.0
    };
    from_material + snow + mud
}

#[cfg(test)]
mod tests {
    use super::*;
    use engine_world::SurfaceMaterials;

    #[test]
    fn curves_peak_and_fall_off() {
        let t = TireModel::default();
        let (k, a) = t.peaks();
        assert!(k > 0.05 && k < 0.25, "slip ratio peak {k}");
        let a_deg = a.to_degrees();
        assert!(a_deg > 4.0 && a_deg < 14.0, "slip angle peak {a_deg}°");
        let peak = t.longitudinal.eval(k);
        assert!((peak - 1.0).abs() < 0.01);
        // Locked wheel (κ = −1) still brakes, but less than at the peak.
        let locked = t.forces(-1.0, 0.0, 4_000.0, 1.0, t.peaks());
        let best = t.forces(-k, 0.0, 4_000.0, 1.0, t.peaks());
        assert!(locked.longitudinal < 0.0);
        assert!(locked.longitudinal.abs() < best.longitudinal.abs() * 0.97);
        assert!(best.longitudinal.abs() > 4_000.0 * 0.95, "{best:?}");
    }

    #[test]
    fn lateral_force_opposes_slide() {
        let t = TireModel::default();
        // Contact patch sliding to the right → force to the left.
        let f = t.forces(0.0, 0.1, 4_000.0, 1.0, t.peaks());
        assert!(f.lateral < -1_000.0, "{f:?}");
        assert!(f.longitudinal.abs() < 1e-3);
    }

    #[test]
    fn combined_slip_stays_inside_the_friction_circle() {
        let t = TireModel::default();
        let peaks = t.peaks();
        let load = 4_000.0;
        let max = t.mu_at_load(load) * load;
        for i in 0..20 {
            for j in 0..20 {
                let kappa = -1.0 + 2.0 * i as f32 / 19.0;
                let alpha = -0.6 + 1.2 * j as f32 / 19.0;
                let f = t.forces(kappa, alpha, load, 1.0, peaks);
                let mag = (f.longitudinal.powi(2) + f.lateral.powi(2)).sqrt();
                assert!(mag <= max * 1.001, "κ {kappa} α {alpha}: {mag} > {max}");
            }
        }
        // Braking at the peak leaves little cornering force.
        let pure = t.forces(0.0, peaks.1, load, 1.0, peaks).lateral.abs();
        let braking = t.forces(-peaks.0, peaks.1, load, 1.0, peaks).lateral.abs();
        assert!(braking < pure * 0.75, "{braking} vs {pure}");
    }

    #[test]
    fn heavier_load_means_lower_friction_coefficient() {
        let t = TireModel::default();
        assert!(t.mu_at_load(8_000.0) < t.mu_at_load(4_000.0));
        assert!(t.mu_at_load(2_000.0) > t.mu_at_load(4_000.0));
    }

    #[test]
    fn surfaces_change_grip() {
        let mats = SurfaceMaterials::default();
        let dry = SurfaceCell::default();
        let g = |name: &str, cell: &SurfaceCell| {
            surface_grip(mats.get(mats.id(name).unwrap()), cell, 1.0, 20.0)
        };
        let concrete = g("concrete", &dry);
        let ice = g("ice", &dry);
        let snow = g("snow", &dry);
        let mud = g("mud", &dry);
        assert!(concrete > 0.95 && concrete < 1.15, "{concrete}");
        assert!(ice < 0.1, "{ice}");
        assert!(snow < 0.6 && snow > 0.3, "{snow}");
        assert!(mud < 0.7, "{mud}");
        let wet = SurfaceCell {
            wetness: 1.0,
            ..Default::default()
        };
        assert!(g("concrete", &wet) < concrete * 0.85);
        let puddle = SurfaceCell {
            wetness: 1.0,
            puddle_m: 0.01,
            ..Default::default()
        };
        let aquaplane = surface_grip(mats.get(mats.id("concrete").unwrap()), &puddle, 1.0, 35.0);
        assert!(aquaplane < 0.4, "{aquaplane}");
        let snowy = SurfaceCell {
            snow_m: 0.05,
            ..Default::default()
        };
        assert!(g("concrete", &snowy) < 0.5);
        // Soft ground drags.
        assert!(
            sinkage_resistance(mats.get(mats.id("mud").unwrap()), &dry)
                > sinkage_resistance(mats.get(mats.id("concrete").unwrap()), &dry) + 0.1
        );
    }
}
