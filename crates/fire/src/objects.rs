//! Burning objects: trees, crates, wooden walls, anything with a [`Combustible`].
//!
//! Objects heat up from the fire field under them and from other burning objects nearby,
//! dry out, ignite at their ignition temperature, burn through their fuel and smolder out.
//! While they burn they heat the ground under them (so a burning tree lights the grass and a
//! grass fire lights the tree), throw embers and send smoke. Owners react to
//! `Combustible::state` (foliage swaps a burnt tree, destruction drops a burnt beam); for
//! simple props, [`Charring`] darkens the object's material and shrinks foliage as it burns.

use engine_core::math::{DVec2, DVec3, Vec3};
use engine_core::prelude::*;
use engine_world::{BurnState, Combustible};

use crate::field::{COMBUSTION_EFFICIENCY, FUEL_SPECIFIC_HEAT, HEAT_OF_COMBUSTION, LATENT_HEAT};

/// Where an object's flames are: a vertical cylinder of `radius` and `height`, its base at the
/// entity's position plus `offset` (world axes, meters). A tree's canopy might be
/// `{radius: 2.5, height: 5, offset: [0, 2, 0]}`. Without it, flames are sized from the fuel.
#[derive(Component, Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct FlameShape {
    pub radius: f32,
    pub height: f32,
    pub offset: [f32; 3],
}

impl Default for FlameShape {
    fn default() -> Self {
        Self {
            radius: 0.5,
            height: 1.0,
            offset: [0.0; 3],
        }
    }
}

impl FlameShape {
    /// A guess from the fuel mass when an object has no shape.
    pub fn from_fuel(fuel_kg: f32) -> Self {
        let r = (0.25 * fuel_kg.max(0.1).cbrt()).clamp(0.15, 3.0);
        Self {
            radius: r,
            height: r * 2.0,
            offset: [0.0; 3],
        }
    }

    /// Center of the flame volume for an object at `pos`.
    pub fn center(&self, pos: DVec3) -> DVec3 {
        pos + Vec3::from(self.offset).as_dvec3() + DVec3::Y * (self.height as f64 * 0.5)
    }

    pub fn base(&self, pos: DVec3) -> DVec3 {
        pos + Vec3::from(self.offset).as_dvec3()
    }

    /// The face that heats up from fire below or beside it, m².
    pub fn face_area(&self) -> f32 {
        std::f32::consts::PI * self.radius * self.radius
    }

    /// Heat flux (W/m²) flames of this shape at `pos`, releasing `hrr_w`, put on `other` at
    /// `other_pos` where they touch it: a quarter of the heat of the part of the flames inside
    /// the other, over the patch they lick (flame impingement tops out near 150 kW/m²).
    pub fn flame_contact(
        &self,
        pos: DVec3,
        hrr_w: f32,
        other: &FlameShape,
        other_pos: DVec3,
    ) -> f32 {
        let (a, b) = (self.base(pos), other.base(other_pos));
        let reach = self.radius + other.radius;
        let d = DVec2::new(a.x - b.x, a.z - b.z).length() as f32;
        if d >= reach || other.height <= 0.0 {
            return 0.0;
        }
        let lo = a.y.max(b.y);
        let hi = (a.y + self.height as f64).min(b.y + other.height as f64);
        let overlap = ((hi - lo).max(0.0) as f32 / self.height.max(0.1)).min(1.0);
        let r = self.radius.min(other.radius).max(0.1);
        let patch = std::f32::consts::PI * r * r;
        (0.25 * hrr_w * overlap * (1.0 - d / reach).sqrt() / patch).min(150_000.0)
    }
}

/// Visual reaction to burning for simple props: the material darkens to char, glows while
/// smoldering, and `shrink` of the object's size burns away (leaves, hay bales).
#[derive(Component, Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Charring {
    /// Fraction of the object's size that burns away (0 = keeps its shape, 0.9 = foliage).
    pub shrink: f32,
    /// Char color (linear RGB).
    pub char_color: [f32; 3],
}

impl Default for Charring {
    fn default() -> Self {
        Self {
            shrink: 0.0,
            char_color: [0.025, 0.022, 0.02],
        }
    }
}

/// A fire kept in its place: a campfire ring, brazier, fireplace or torch. It never lights the
/// ground under it and its sparks can't start fires, but its flames still burn whoever walks
/// into them and its heat still reaches things nearby. `perpetual` keeps it fed and relights
/// it when it goes out (someone tends it); without it, it burns out like anything else.
#[derive(Component, Debug, Clone, Copy, PartialEq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct ContainedFire {
    pub perpetual: bool,
}

impl ContainedFire {
    /// Keep a perpetual fire fed and lit (call before [`step_object`]).
    pub fn tend(&self, c: &mut Combustible, b: &Burning) {
        if !self.perpetual {
            return;
        }
        if b.fuel0_kg > 0.0 {
            c.fuel_kg = c.fuel_kg.max(b.fuel0_kg);
        }
        c.moisture = c.moisture.min(0.3);
        if c.state != BurnState::Burning {
            c.state = BurnState::Intact;
            c.temperature_c = c.temperature_c.max(c.ignition_c + 50.0);
        }
    }
}

/// Fire's bookkeeping on a [`Combustible`] (added automatically).
#[derive(Component, Debug, Clone, Copy, PartialEq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Burning {
    /// Fuel when first seen, kg.
    pub fuel0_kg: f32,
    /// Flame intensity 0..1.
    pub intensity: f32,
    /// Coal glow 0..1.
    pub glow: f32,
    /// Heat release, W.
    pub hrr_w: f32,
    /// Fuel burned this step, kg (for smoke).
    pub consumed_kg: f32,
    /// 0 untouched .. 1 fully charred.
    pub char: f32,
    /// Seconds the object has been burning.
    pub burning_secs: f32,
}

/// Moisture above which an object won't sustain flames.
const OBJECT_EXTINCTION_MOISTURE: f32 = 0.6;
/// Below this fraction of its fuel an object only glows; it can't flame again.
const FLAMING_FRACTION: f32 = 0.04;
/// Fuel in the surface layer that heats up to ignite, kg per m² of face (a few mm of wood).
const SURFACE_KG_M2: f32 = 0.6;
/// Convective heat loss to the air, W/m²K.
const CONVECTION_W_M2K: f32 = 15.0;
const EMISSIVITY: f32 = 0.9;
const STEFAN_BOLTZMANN: f32 = 5.67e-8;

/// Seconds an object of `fuel_kg` flames for at full intensity.
pub fn burn_secs(fuel_kg: f32) -> f32 {
    (60.0 * (fuel_kg.max(0.1) / 5.0).powf(0.4)).clamp(20.0, 900.0)
}

/// Inputs to one object's step.
pub struct ObjectEnv {
    pub dt: f32,
    pub ambient_c: f32,
    pub rain_mm_h: f32,
    /// Heat flux on the object's hottest spot, W/m² (ignition starts where it's hottest).
    pub flux_w_m2: f32,
}

/// Advance one object: heating, ignition, burning, smoldering.
pub fn step_object(c: &mut Combustible, b: &mut Burning, shape: &FlameShape, env: &ObjectEnv) {
    let dt = env.dt;
    if b.fuel0_kg <= 0.0 {
        b.fuel0_kg = c.fuel_kg.max(0.0);
    }
    // Rain soaks what isn't burning hot.
    if env.rain_mm_h > 0.0 {
        let area = std::f32::consts::PI * shape.radius * shape.radius;
        let water = env.rain_mm_h / 3600.0 * area * 0.2 * dt;
        c.moisture = (c.moisture + water / c.fuel_kg.max(0.5)).min(2.0);
    }
    let damping = moisture_damping(c.moisture, OBJECT_EXTINCTION_MOISTURE);
    b.consumed_kg = 0.0;
    match c.state {
        BurnState::Intact | BurnState::Smoldering if b.intensity <= 0.0 && c.fuel_kg > 0.0 => {
            // Only the surface facing the fire heats toward ignition (bark, planks, leaves);
            // its water must boil off first, and it loses heat to the air by convection and
            // radiation, so weak heat never lights it however long it lasts.
            // Everything per m² of that surface.
            let face = shape.face_area();
            let surface_kg = (c.fuel_kg / face).clamp(0.05, SURFACE_KG_M2);
            let span = (c.ignition_c - env.ambient_c).max(50.0);
            let capacity = surface_kg * (FUEL_SPECIFIC_HEAT + c.moisture * LATENT_HEAT / span);
            let (t, ta) = (c.temperature_c + 273.15, env.ambient_c + 273.15);
            let loss = CONVECTION_W_M2K * (c.temperature_c - env.ambient_c)
                + EMISSIVITY * STEFAN_BOLTZMANN * (t.powi(4) - ta.powi(4));
            c.temperature_c += (env.flux_w_m2 - loss) / capacity * dt;
            if env.flux_w_m2 > 0.0 {
                // Heat drives water out of the heated surface.
                let water = env.flux_w_m2 * face * dt * 0.2 / LATENT_HEAT;
                c.moisture = (c.moisture - water / c.fuel_kg.max(0.5)).max(0.0);
            }
            let flammable = c.fuel_kg > b.fuel0_kg * FLAMING_FRACTION;
            if c.temperature_c >= c.ignition_c && damping > 0.05 && flammable {
                c.state = BurnState::Burning;
                b.intensity = 0.2;
            }
            if c.state == BurnState::Smoldering {
                b.glow = (b.glow - dt / (burn_secs(b.fuel0_kg) * 0.6)).max(0.0);
                if b.glow <= 0.0 {
                    c.state = BurnState::Burnt;
                }
            }
        }
        BurnState::Burning => {
            let flaming = c.fuel_kg > b.fuel0_kg * FLAMING_FRACTION;
            let target = if flaming { damping } else { 0.0 };
            let rate = if target > b.intensity { 0.4 } else { 0.8 };
            b.intensity += (target - b.intensity) * (rate * dt).min(1.0);
            let consumed = (b.fuel0_kg / burn_secs(b.fuel0_kg) * b.intensity * dt).min(c.fuel_kg);
            c.fuel_kg -= consumed;
            b.consumed_kg = consumed;
            b.hrr_w = consumed / dt * HEAT_OF_COMBUSTION * COMBUSTION_EFFICIENCY;
            b.burning_secs += dt;
            b.glow = (b.glow + dt / 20.0).min(1.0);
            b.char = b.char.max(1.0 - c.fuel_kg / b.fuel0_kg.max(1e-3));
            c.temperature_c = env.ambient_c + 900.0 * b.intensity.max(0.3);
            if b.intensity < 0.02 {
                b.intensity = 0.0;
                b.hrr_w = 0.0;
                c.state = if c.fuel_kg > b.fuel0_kg * 0.3 {
                    // Put out with fuel left (rain, water): can burn again later.
                    BurnState::Intact
                } else {
                    BurnState::Smoldering
                };
                c.temperature_c = env.ambient_c + 300.0;
            }
            return;
        }
        _ => {}
    }
    b.hrr_w = b.glow * 2000.0 * b.fuel0_kg.sqrt();
    if c.state == BurnState::Burnt {
        b.hrr_w = 0.0;
        c.temperature_c += (env.ambient_c - c.temperature_c) * (dt / 30.0).min(1.0);
    }
}

fn moisture_damping(m: f32, mx: f32) -> f32 {
    let r = (m / mx).clamp(0.0, 1.0);
    (1.0 - 2.59 * r + 5.11 * r * r - 3.52 * r * r * r).clamp(0.0, 1.0)
}

/// Radiant heat flux (W/m²) from a fire releasing `hrr_w`, `distance` meters away.
pub fn radiant_flux(hrr_w: f32, distance: f32) -> f32 {
    // ~30% of flame heat leaves as radiation, spread over a sphere.
    0.3 * hrr_w / (4.0 * std::f32::consts::PI * distance.max(0.5).powi(2))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn heated_object_ignites_burns_and_smolders_out() {
        let mut c = Combustible {
            fuel_kg: 20.0,
            moisture: 0.15,
            ignition_c: 300.0,
            temperature_c: 20.0,
            state: BurnState::Intact,
        };
        let mut b = Burning::default();
        let shape = FlameShape::from_fuel(20.0);
        let mut env = ObjectEnv {
            dt: 0.05,
            ambient_c: 20.0,
            rain_mm_h: 0.0,
            flux_w_m2: 25_000.0,
        };
        let mut t = 0.0;
        while c.state == BurnState::Intact {
            step_object(&mut c, &mut b, &shape, &env);
            t += env.dt;
            assert!(t < 120.0, "never ignited: {c:?}");
        }
        assert!(t > 2.0, "ignited instantly");
        env.flux_w_m2 = 0.0;
        let mut peak = 0.0f32;
        while c.state == BurnState::Burning {
            step_object(&mut c, &mut b, &shape, &env);
            peak = peak.max(b.hrr_w);
            t += env.dt;
            assert!(t < 2000.0, "burned forever: {c:?} {b:?}");
        }
        assert!(
            peak > 100_000.0,
            "a 20 kg object should release > 100 kW, got {peak}"
        );
        assert!(c.fuel_kg < 20.0 * 0.1);
        while c.state == BurnState::Smoldering {
            step_object(&mut c, &mut b, &shape, &env);
            t += env.dt;
            assert!(t < 4000.0, "smoldered forever: {c:?} {b:?}");
        }
        assert_eq!(c.state, BurnState::Burnt);
    }

    #[test]
    fn wet_objects_do_not_ignite() {
        let mut c = Combustible {
            fuel_kg: 5.0,
            moisture: 1.5,
            ignition_c: 300.0,
            temperature_c: 20.0,
            state: BurnState::Intact,
        };
        let mut b = Burning::default();
        let env = ObjectEnv {
            dt: 0.1,
            ambient_c: 20.0,
            rain_mm_h: 10.0,
            flux_w_m2: 4_000.0,
        };
        for _ in 0..600 {
            step_object(&mut c, &mut b, &FlameShape::default(), &env);
        }
        assert_eq!(c.state, BurnState::Intact, "{c:?}");
    }
}
