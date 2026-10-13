//! Fuel: what burns, how much, how wet, and how it behaves when it does.
//!
//! Every fire cell gets its fuel when it is first needed: from the ground's surface material
//! (`SurfaceMaterials`: grass, leaves, wood…), then from any [`FuelPatch`] covering it (a dry
//! meadow, a gravel firebreak, a soaked field). Standing water never burns.

use engine_core::math::{DVec3, Vec2};
use engine_core::prelude::*;
use engine_world::SurfaceMaterial;

/// Broad fuel classes. They set burn time, flame height, smoke color, embers and how wet the
/// fuel may be before it stops carrying fire (moisture of extinction).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
pub enum FuelKind {
    /// Nothing to burn (rock, gravel, bare soil, water).
    #[default]
    None,
    /// Cured grass and crops: fast, short flames, light smoke.
    Grass,
    /// Leaf and needle litter on a forest floor: slower, low flames.
    Litter,
    /// Shrubs and undergrowth: tall flames, many embers.
    Brush,
    /// Timber, planks, logs: slow, hot, long-glowing coals, dark smoke.
    Wood,
}

/// Behaviour of one [`FuelKind`].
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FuelBehavior {
    /// Seconds a cell of this fuel flames before only coals are left.
    pub burn_secs: f32,
    /// Fuel moisture (kg water per kg dry fuel) at which fire stops spreading.
    pub extinction_moisture: f32,
    /// Flame length at full intensity, meters.
    pub flame_m: f32,
    /// Seconds coals keep glowing after the flames die.
    pub glow_secs: f32,
    /// Smoke color, 0 white .. 1 black.
    pub smoke_darkness: f32,
    /// Embers thrown per second per burning square meter.
    pub embers: f32,
}

impl FuelKind {
    pub fn behavior(self) -> FuelBehavior {
        match self {
            FuelKind::None => FuelBehavior {
                burn_secs: 1.0,
                extinction_moisture: 0.0,
                flame_m: 0.0,
                glow_secs: 0.0,
                smoke_darkness: 0.3,
                embers: 0.0,
            },
            FuelKind::Grass => FuelBehavior {
                burn_secs: 7.0,
                extinction_moisture: 0.25,
                flame_m: 1.6,
                glow_secs: 4.0,
                smoke_darkness: 0.35,
                embers: 0.4,
            },
            FuelKind::Litter => FuelBehavior {
                burn_secs: 25.0,
                extinction_moisture: 0.3,
                flame_m: 0.8,
                glow_secs: 30.0,
                smoke_darkness: 0.45,
                embers: 0.3,
            },
            FuelKind::Brush => FuelBehavior {
                burn_secs: 35.0,
                extinction_moisture: 0.3,
                flame_m: 3.5,
                glow_secs: 30.0,
                smoke_darkness: 0.55,
                embers: 1.5,
            },
            FuelKind::Wood => FuelBehavior {
                burn_secs: 150.0,
                extinction_moisture: 0.35,
                flame_m: 2.0,
                glow_secs: 240.0,
                smoke_darkness: 0.6,
                embers: 1.0,
            },
        }
    }

    /// Fuel class for a surface material (by name, falling back to its flammability).
    pub fn for_material(m: &SurfaceMaterial) -> FuelKind {
        if m.flammability <= 0.0 || m.fuel_kg_m2 <= 0.0 {
            return FuelKind::None;
        }
        match m.name.as_str() {
            "grass" | "hay" | "straw" | "crops" | "wheat" => FuelKind::Grass,
            "leaves" | "litter" | "needles" | "fabric" | "paper" => FuelKind::Litter,
            "brush" | "shrubs" | "heather" => FuelKind::Brush,
            "wood" | "planks" | "logs" | "timber" => FuelKind::Wood,
            _ if m.fuel_kg_m2 >= 4.0 => FuelKind::Wood,
            _ if m.flammability >= 0.5 => FuelKind::Grass,
            _ => FuelKind::Litter,
        }
    }
}

/// Fuel of one ground cell, before it burns.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CellFuel {
    pub kind: FuelKind,
    /// Dry fuel load, kg/m².
    pub load_kg_m2: f32,
    /// Fuel moisture, kg water per kg dry fuel (cured grass ≈ 0.06, green grass ≈ 0.6). From
    /// [`LIVE_MOISTURE`](crate::field::LIVE_MOISTURE) up it is green fuel and stays that wet.
    pub moisture: f32,
    /// °C at which the dried fuel ignites.
    pub ignition_c: f32,
    /// Seconds after burning out before it regrows (0 = never).
    pub regrow_secs: f32,
}

impl CellFuel {
    pub const NONE: CellFuel = CellFuel {
        kind: FuelKind::None,
        load_kg_m2: 0.0,
        moisture: 0.0,
        ignition_c: 900.0,
        regrow_secs: 0.0,
    };

    /// Fuel from a ground surface material.
    pub fn from_material(m: &SurfaceMaterial) -> CellFuel {
        let kind = FuelKind::for_material(m);
        if kind == FuelKind::None {
            return CellFuel::NONE;
        }
        CellFuel {
            kind,
            // The material table lists fuel for a whole surface; surface fuels that carry a
            // fire (cured grass, litter) are a fraction of a kilogram per square meter.
            load_kg_m2: m.fuel_kg_m2 * m.flammability.clamp(0.05, 1.0),
            moisture: 0.08,
            ignition_c: m.ignition_c,
            regrow_secs: 0.0,
        }
    }

    pub fn burns(&self) -> bool {
        self.kind != FuelKind::None && self.load_kg_m2 > 0.0
    }
}

/// Shape of a [`FuelPatch`] on the ground, in the entity's local X/Z (yaw from its rotation).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub enum PatchShape {
    /// Circle of `radius` meters.
    Disc { radius: f32 },
    /// Rectangle with half extents along local X and Z, meters.
    Rect { half_extents: [f32; 2] },
}

impl Default for PatchShape {
    fn default() -> Self {
        PatchShape::Disc { radius: 10.0 }
    }
}

/// Overrides the ground's fuel inside an area. Use it for a dry meadow (`Grass`, 0.4 kg/m²),
/// a gravel firebreak (`None`), a soaked field (high `moisture`), a forest floor (`Litter`).
/// Higher `priority` wins where patches overlap. Changing a patch refuels cells that are not
/// burning.
#[derive(Component, Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct FuelPatch {
    pub shape: PatchShape,
    pub kind: FuelKind,
    /// Dry fuel load, kg/m² (cured grass 0.2–0.6, litter 0.5–1.5, brush 2–5).
    pub load_kg_m2: f32,
    /// Fuel moisture, kg water per kg dry fuel (cured grass 0.04–0.1, green 0.5+). From
    /// [`LIVE_MOISTURE`](crate::field::LIVE_MOISTURE) up it is green fuel: weather doesn't
    /// dry it out.
    pub moisture: f32,
    /// °C at which the dried fuel ignites.
    pub ignition_c: f32,
    /// Seconds after burning out before the fuel grows back (0 = never). Lets a demo plot burn
    /// again and again.
    pub regrow_secs: f32,
    pub priority: i32,
}

impl Default for FuelPatch {
    fn default() -> Self {
        Self {
            shape: PatchShape::default(),
            kind: FuelKind::Grass,
            load_kg_m2: 0.35,
            moisture: 0.06,
            ignition_c: 300.0,
            regrow_secs: 0.0,
            priority: 0,
        }
    }
}

impl FuelPatch {
    /// A patch of dry grass.
    pub fn grass(shape: PatchShape) -> Self {
        Self {
            shape,
            ..Default::default()
        }
    }

    /// A strip that carries no fire (gravel, plowed soil, a road).
    pub fn firebreak(shape: PatchShape) -> Self {
        Self {
            shape,
            kind: FuelKind::None,
            load_kg_m2: 0.0,
            priority: 10,
            ..Default::default()
        }
    }

    pub fn with_moisture(mut self, moisture: f32) -> Self {
        self.moisture = moisture;
        self
    }

    pub fn with_load(mut self, kg_m2: f32) -> Self {
        self.load_kg_m2 = kg_m2;
        self
    }

    pub fn with_regrow(mut self, secs: f32) -> Self {
        self.regrow_secs = secs;
        self
    }

    pub fn with_priority(mut self, priority: i32) -> Self {
        self.priority = priority;
        self
    }

    pub fn fuel(&self) -> CellFuel {
        if self.kind == FuelKind::None {
            return CellFuel {
                regrow_secs: self.regrow_secs,
                ..CellFuel::NONE
            };
        }
        CellFuel {
            kind: self.kind,
            load_kg_m2: self.load_kg_m2.max(0.0),
            moisture: self.moisture.max(0.0),
            ignition_c: self.ignition_c,
            regrow_secs: self.regrow_secs.max(0.0),
        }
    }
}

/// A [`FuelPatch`] placed in the world, ready for point tests.
#[derive(Debug, Clone, Copy)]
pub struct PlacedPatch {
    pub entity: Entity,
    pub center: DVec3,
    /// Unit X axis of the patch in world X/Z.
    pub axis_x: Vec2,
    pub patch: FuelPatch,
}

impl PlacedPatch {
    pub fn contains(&self, p: DVec3) -> bool {
        let d = Vec2::new((p.x - self.center.x) as f32, (p.z - self.center.z) as f32);
        match self.patch.shape {
            PatchShape::Disc { radius } => d.length_squared() <= radius * radius,
            PatchShape::Rect { half_extents } => {
                let x = d.dot(self.axis_x);
                let z = d.dot(self.axis_x.perp());
                x.abs() <= half_extents[0] && z.abs() <= half_extents[1]
            }
        }
    }

    /// The same fuel in the same place (to within 0.1 mm and a hundredth of a degree): what a
    /// world shift leaves of a patch it moved.
    pub fn same_as(&self, other: &PlacedPatch) -> bool {
        self.patch == other.patch
            && self.center.distance_squared(other.center) < 1e-8
            && self.axis_x.distance_squared(other.axis_x) < 3e-8
    }

    /// Bounding radius around `center`, meters.
    pub fn radius(&self) -> f32 {
        match self.patch.shape {
            PatchShape::Disc { radius } => radius,
            PatchShape::Rect { half_extents } => Vec2::from(half_extents).length(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use engine_world::SurfaceMaterials;

    #[test]
    fn surface_materials_map_to_fuel() {
        let m = SurfaceMaterials::default();
        let grass = CellFuel::from_material(m.get(m.id("grass").unwrap()));
        assert_eq!(grass.kind, FuelKind::Grass);
        assert!(
            grass.load_kg_m2 > 0.1 && grass.load_kg_m2 < 1.0,
            "{grass:?}"
        );
        assert!(!CellFuel::from_material(m.get(m.id("rock").unwrap())).burns());
        assert!(!CellFuel::from_material(m.get(m.id("water").unwrap())).burns());
        assert_eq!(
            CellFuel::from_material(m.get(m.id("wood").unwrap())).kind,
            FuelKind::Wood
        );
    }

    #[test]
    fn rotated_rect_patch() {
        let p = PlacedPatch {
            entity: Entity::PLACEHOLDER,
            center: DVec3::ZERO,
            axis_x: Vec2::new(0.0, 1.0),
            patch: FuelPatch::firebreak(PatchShape::Rect {
                half_extents: [10.0, 1.0],
            }),
        };
        // Local X runs along world Z.
        assert!(p.contains(DVec3::new(0.5, 0.0, 9.0)));
        assert!(!p.contains(DVec3::new(9.0, 0.0, 0.5)));
    }
}
