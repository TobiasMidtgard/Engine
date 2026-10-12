//! Destructible materials: how much energy breaks them, what they look like inside, how heavy
//! their debris is and how far they can reach without support.
//!
//! Every number lives in the [`DestructionMaterials`] resource, so a designer or an AI agent can
//! retune a material at runtime (`resource.patch DestructionMaterials`).

use engine_core::prelude::*;

/// What a destructible thing is made of.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DestructibleMaterial {
    /// Drywall/plaster over a stud frame: the "soft wall" of siege games. Bullets go through.
    Plaster,
    /// Planks, beams, posts, crates. Burns.
    Wood,
    /// Fired-clay brick masonry.
    Brick,
    /// Cut-stone masonry (castle walls).
    Stone,
    #[default]
    Concrete,
    /// Concrete with a steel rebar grid: holes keep their rebar until it is cut (thermite).
    ReinforcedConcrete,
    /// Natural rock (cliffs, boulders, caves).
    Rock,
    /// Soil, clay, sand.
    Dirt,
    /// Steel plates and beams: only cutting charges get through.
    Metal,
    /// Panes and bottles: shatters into shards.
    Glass,
    Ice,
}

impl DestructibleMaterial {
    pub const ALL: [DestructibleMaterial; 11] = [
        Self::Plaster,
        Self::Wood,
        Self::Brick,
        Self::Stone,
        Self::Concrete,
        Self::ReinforcedConcrete,
        Self::Rock,
        Self::Dirt,
        Self::Metal,
        Self::Glass,
        Self::Ice,
    ];

    /// Position in [`DestructionMaterials::materials`].
    pub fn index(self) -> usize {
        self as usize
    }

    pub fn name(self) -> &'static str {
        match self {
            Self::Plaster => "plaster",
            Self::Wood => "wood",
            Self::Brick => "brick",
            Self::Stone => "stone",
            Self::Concrete => "concrete",
            Self::ReinforcedConcrete => "reinforced_concrete",
            Self::Rock => "rock",
            Self::Dirt => "dirt",
            Self::Metal => "metal",
            Self::Glass => "glass",
            Self::Ice => "ice",
        }
    }

    /// Parse a material name (`"reinforced_concrete"`, `"wood"`, …).
    pub fn from_name(name: &str) -> Option<Self> {
        let name = name.trim().to_ascii_lowercase().replace([' ', '-'], "_");
        Self::ALL.into_iter().find(|m| m.name() == name)
    }

    /// Names of every material, for error messages.
    pub fn names() -> String {
        Self::ALL.map(|m| m.name()).join(", ")
    }
}

/// Physical and visual properties of one [`DestructibleMaterial`].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct MaterialProps {
    pub material: DestructibleMaterial,
    /// Energy that pulverizes one cubic meter, J/m³. Explosions remove `flux / toughness`
    /// meters of depth (flux in J/m²); bullets remove `energy / toughness` cubic meters.
    pub toughness_j_m3: f32,
    /// kg/m³ (debris mass).
    pub density: f32,
    /// Depth ÷ radius of a bullet crater: high = narrow tunnel (plaster, wood), low = wide
    /// shallow spall (concrete, rock).
    pub crater_aspect: f32,
    /// Random spread of toughness between cells/voxels (0..1): jagged, natural break outlines.
    pub irregularity: f32,
    /// Fraction of the pulverizing energy that snaps a whole piece (beams, pillars, props).
    pub break_fraction: f32,
    /// Linear RGB of the outside surface (paint, weathered stone, bark).
    pub exterior: [f32; 3],
    /// Linear RGB of freshly broken material.
    pub interior: [f32; 3],
    pub roughness: f32,
    pub metallic: f32,
    /// Name in `engine_world::SurfaceMaterials` (friction, footsteps, impact effects).
    pub surface: String,
    /// Valheim-style structural support at the ground (support units).
    pub max_support: f32,
    /// Support lost per meter climbed straight up or down.
    pub vertical_loss_per_m: f32,
    /// Support lost per meter reached sideways (overhangs, beams, bridges).
    pub horizontal_loss_per_m: f32,
    /// Below this support a piece or voxel collapses.
    pub min_support: f32,
    /// Typical debris chip size, meters.
    pub chip_size: f32,
    /// Dust color for `SmokeEmission`: 0 white .. 1 black.
    pub dust_darkness: f32,
    /// Fuel for `engine_world::Combustible` on structural pieces, kg per m³ (0 = doesn't burn).
    pub fuel_kg_m3: f32,
    /// Falling pieces landing faster than this shatter, m/s (0 = never: they stay whole).
    pub shatter_speed: f32,
}

impl Default for MaterialProps {
    fn default() -> Self {
        props(DestructibleMaterial::Concrete)
    }
}

/// sRGB hex to linear RGB.
pub fn linear(hex: u32) -> [f32; 3] {
    let c = |v: u32| {
        let s = (v & 0xff) as f32 / 255.0;
        if s <= 0.04045 {
            s / 12.92
        } else {
            ((s + 0.055) / 1.055).powf(2.4)
        }
    };
    [c(hex >> 16), c(hex >> 8), c(hex)]
}

/// Built-in values for `material`.
pub fn props(material: DestructibleMaterial) -> MaterialProps {
    use DestructibleMaterial as M;
    let base = MaterialProps {
        material,
        toughness_j_m3: 3.0e6,
        density: 2400.0,
        crater_aspect: 0.5,
        irregularity: 0.35,
        break_fraction: 0.08,
        exterior: linear(0xa8a49c),
        interior: linear(0xc4c0b6),
        roughness: 0.85,
        metallic: 0.0,
        surface: "concrete".into(),
        max_support: 1000.0,
        vertical_loss_per_m: 15.0,
        horizontal_loss_per_m: 150.0,
        min_support: 10.0,
        chip_size: 0.08,
        dust_darkness: 0.25,
        fuel_kg_m3: 0.0,
        shatter_speed: 8.0,
    };
    match material {
        M::Plaster => MaterialProps {
            shatter_speed: 4.0,
            toughness_j_m3: 3.0e5,
            density: 900.0,
            crater_aspect: 3.0,
            irregularity: 0.3,
            exterior: linear(0xd9d1c0),
            interior: linear(0xf1ede4),
            roughness: 0.9,
            max_support: 100.0,
            vertical_loss_per_m: 10.0,
            horizontal_loss_per_m: 45.0,
            chip_size: 0.06,
            dust_darkness: 0.05,
            ..base
        },
        M::Wood => MaterialProps {
            shatter_speed: 0.0,
            toughness_j_m3: 1.5e6,
            density: 600.0,
            crater_aspect: 2.0,
            irregularity: 0.45,
            break_fraction: 0.05,
            exterior: linear(0x7d5734),
            interior: linear(0xc9a46e),
            roughness: 0.8,
            surface: "wood".into(),
            max_support: 100.0,
            vertical_loss_per_m: 5.0,
            horizontal_loss_per_m: 20.0,
            chip_size: 0.07,
            dust_darkness: 0.3,
            fuel_kg_m3: 450.0,
            ..base
        },
        M::Brick => MaterialProps {
            shatter_speed: 6.0,
            toughness_j_m3: 2.0e6,
            density: 1900.0,
            crater_aspect: 0.6,
            exterior: linear(0x8f4f3a),
            interior: linear(0xb2735a),
            roughness: 0.9,
            max_support: 1000.0,
            vertical_loss_per_m: 20.0,
            horizontal_loss_per_m: 350.0,
            dust_darkness: 0.35,
            ..base
        },
        M::Stone => MaterialProps {
            shatter_speed: 7.0,
            toughness_j_m3: 2.5e6,
            density: 2500.0,
            crater_aspect: 0.45,
            exterior: linear(0x9c8a72),
            interior: linear(0xc2b49c),
            roughness: 0.9,
            surface: "rock".into(),
            max_support: 1000.0,
            vertical_loss_per_m: 20.0,
            horizontal_loss_per_m: 400.0,
            chip_size: 0.1,
            dust_darkness: 0.2,
            ..base
        },
        M::Concrete => base,
        M::ReinforcedConcrete => MaterialProps {
            shatter_speed: 10.0,
            toughness_j_m3: 4.0e6,
            exterior: linear(0x8f8c86),
            interior: linear(0xb9b5ab),
            horizontal_loss_per_m: 60.0,
            ..base
        },
        M::Rock => MaterialProps {
            shatter_speed: 7.0,
            toughness_j_m3: 2.0e6,
            density: 2600.0,
            crater_aspect: 0.4,
            irregularity: 0.5,
            exterior: linear(0x8a8279),
            interior: linear(0xa59d92),
            roughness: 0.9,
            surface: "rock".into(),
            max_support: 1000.0,
            vertical_loss_per_m: 10.0,
            horizontal_loss_per_m: 330.0,
            chip_size: 0.1,
            dust_darkness: 0.3,
            ..base
        },
        M::Dirt => MaterialProps {
            shatter_speed: 3.0,
            toughness_j_m3: 3.0e5,
            density: 1500.0,
            crater_aspect: 1.0,
            irregularity: 0.3,
            exterior: linear(0x5e4a30),
            interior: linear(0x4f3c27),
            roughness: 0.95,
            surface: "dirt".into(),
            max_support: 100.0,
            vertical_loss_per_m: 5.0,
            horizontal_loss_per_m: 60.0,
            chip_size: 0.06,
            dust_darkness: 0.5,
            ..base
        },
        M::Metal => MaterialProps {
            shatter_speed: 0.0,
            toughness_j_m3: 5.0e8,
            density: 7800.0,
            crater_aspect: 0.3,
            irregularity: 0.1,
            break_fraction: 0.02,
            exterior: linear(0x6f747a),
            interior: linear(0xc8ccd4),
            roughness: 0.45,
            metallic: 1.0,
            surface: "metal".into(),
            max_support: 1500.0,
            vertical_loss_per_m: 10.0,
            horizontal_loss_per_m: 75.0,
            chip_size: 0.04,
            dust_darkness: 0.6,
            ..base
        },
        M::Glass => MaterialProps {
            shatter_speed: 2.0,
            toughness_j_m3: 2.0e5,
            density: 2500.0,
            crater_aspect: 0.2,
            irregularity: 0.6,
            break_fraction: 0.01,
            exterior: linear(0xbcd6dd),
            interior: linear(0xd8eef2),
            roughness: 0.05,
            surface: "glass".into(),
            max_support: 50.0,
            horizontal_loss_per_m: 50.0,
            chip_size: 0.05,
            dust_darkness: 0.0,
            ..base
        },
        M::Ice => MaterialProps {
            shatter_speed: 4.0,
            toughness_j_m3: 1.0e6,
            density: 917.0,
            crater_aspect: 0.5,
            exterior: linear(0xc7e3ee),
            interior: linear(0xe6f4fa),
            roughness: 0.15,
            surface: "ice".into(),
            max_support: 400.0,
            horizontal_loss_per_m: 120.0,
            dust_darkness: 0.0,
            ..base
        },
    }
}

/// Every destructible material's properties, indexed by [`DestructibleMaterial::index`].
#[derive(Resource, Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct DestructionMaterials {
    pub materials: Vec<MaterialProps>,
    /// Linear RGB of exposed rebar.
    pub rebar_color: [f32; 3],
    /// Energy flux that cuts rebar, J/m². Only charges right against the wall or thermite do.
    pub rebar_cut_flux: f32,
}

impl Default for DestructionMaterials {
    fn default() -> Self {
        Self {
            materials: DestructibleMaterial::ALL.into_iter().map(props).collect(),
            rebar_color: linear(0x5a3422),
            rebar_cut_flux: 2.5e6,
        }
    }
}

impl DestructionMaterials {
    /// Properties of `material` (built-in values if the table was edited down).
    pub fn get(&self, material: DestructibleMaterial) -> MaterialProps {
        self.materials
            .iter()
            .find(|p| p.material == material)
            .cloned()
            .unwrap_or_else(|| props(material))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_material_has_sane_props() {
        let table = DestructionMaterials::default();
        assert_eq!(table.materials.len(), DestructibleMaterial::ALL.len());
        for m in DestructibleMaterial::ALL {
            let p = table.get(m);
            assert_eq!(p.material, m);
            assert!(p.toughness_j_m3 > 0.0 && p.density > 0.0, "{m:?}");
            assert!(p.max_support > p.min_support, "{m:?}");
            assert_eq!(DestructibleMaterial::from_name(m.name()), Some(m));
        }
        assert_eq!(
            DestructibleMaterial::from_name("Reinforced Concrete"),
            Some(DestructibleMaterial::ReinforcedConcrete)
        );
    }

    #[test]
    fn soft_materials_are_weaker_than_hard_ones() {
        let t = |m| props(m).toughness_j_m3;
        use DestructibleMaterial as M;
        assert!(t(M::Plaster) < t(M::Wood));
        assert!(t(M::Wood) < t(M::Concrete));
        assert!(t(M::Concrete) < t(M::ReinforcedConcrete));
        assert!(t(M::ReinforcedConcrete) < t(M::Metal));
    }
}
