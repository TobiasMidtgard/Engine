//! Surface layers of a terrain: what color the ground is and which surface material
//! (footsteps, friction, deformation) it reports. The shader and [`TerrainPalette::classify`]
//! use the same rules, so a patch that looks like snow also crunches like snow.

use engine_core::prelude::*;
use engine_world::{SurfaceId, SurfaceMaterials};

use crate::chunk::Sample;
use crate::edits::PAINT_NAMES;

/// Colors (linear albedo) and rules for a terrain's surface layers.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct TerrainPalette {
    /// Vegetated ground (grass) where it is moist.
    pub lush: [f32; 3],
    /// Dry ground cover (steppe, dry grass) where it is arid.
    pub dry: [f32; 3],
    /// Bare soil (patches, paths).
    pub soil: [f32; 3],
    /// Cliffs and steep slopes.
    pub rock: [f32; 3],
    /// Beaches and deserts.
    pub sand: [f32; 3],
    pub snow: [f32; 3],
    /// Dark lowland (maria, basalt) and the sea floor.
    pub dark: [f32; 3],
    /// Deep water seen from above.
    pub ocean: [f32; 3],
    /// Wet, churned soil.
    pub mud: [f32; 3],
    /// Whether `lush`/`dry` are plants (grass translucency, softer).
    pub vegetation: bool,
    /// 0..1 how much the ground scatters light like lunar dust rather than a matte surface:
    /// a full moon is as bright at its edge as in its middle (Lommel–Seeliger), and dust
    /// lit from straight behind the viewer brightens (the opposition surge). 0 on Earth.
    pub regolith: f32,
    /// Snow lies where the mean temperature is below this, °C.
    pub snow_below_c: f32,
    /// Steepness (1 − cos of the slope angle) where rock shows through. 0.3 ≈ 45°.
    pub rock_slope: f32,
    /// Surface material reported for vegetated ground (by name in `SurfaceMaterials`).
    pub ground_material: String,
    /// Surface material for bare ground on airless or desert worlds.
    pub bare_material: String,
}

impl Default for TerrainPalette {
    fn default() -> Self {
        Self::earth()
    }
}

impl TerrainPalette {
    pub fn earth() -> Self {
        Self {
            lush: [0.085, 0.16, 0.035],
            dry: [0.26, 0.24, 0.11],
            soil: [0.17, 0.12, 0.075],
            rock: [0.26, 0.245, 0.23],
            sand: [0.52, 0.45, 0.32],
            snow: [0.86, 0.89, 0.93],
            dark: [0.10, 0.11, 0.10],
            ocean: [0.006, 0.03, 0.065],
            mud: [0.075, 0.055, 0.036],
            vegetation: true,
            regolith: 0.0,
            snow_below_c: -2.0,
            rock_slope: 0.22,
            ground_material: "grass".into(),
            bare_material: "dirt".into(),
        }
    }

    pub fn moon() -> Self {
        Self {
            lush: [0.21, 0.205, 0.195],
            dry: [0.18, 0.175, 0.168],
            soil: [0.16, 0.155, 0.15],
            rock: [0.24, 0.235, 0.225],
            sand: [0.2, 0.195, 0.185],
            snow: [0.7, 0.72, 0.75],
            dark: [0.065, 0.066, 0.07],
            ocean: [0.0, 0.0, 0.0],
            mud: [0.1, 0.1, 0.1],
            vegetation: false,
            regolith: 1.0,
            snow_below_c: -200.0,
            rock_slope: 0.35,
            ground_material: "regolith".into(),
            bare_material: "regolith".into(),
        }
    }

    pub fn mars() -> Self {
        Self {
            lush: [0.34, 0.15, 0.065],
            dry: [0.44, 0.21, 0.095],
            soil: [0.27, 0.12, 0.06],
            rock: [0.2, 0.11, 0.075],
            sand: [0.5, 0.25, 0.11],
            snow: [0.8, 0.78, 0.76],
            dark: [0.12, 0.075, 0.055],
            ocean: [0.0, 0.0, 0.0],
            mud: [0.15, 0.07, 0.04],
            vegetation: false,
            regolith: 0.3,
            snow_below_c: -95.0,
            rock_slope: 0.3,
            ground_material: "sand".into(),
            bare_material: "dirt".into(),
        }
    }

    /// The surface material at a sampled point (`slope` = 1 − cos of the slope angle).
    pub fn classify(&self, s: &Sample, slope: f64, under_sea: bool, ids: &SurfaceIds) -> SurfaceId {
        if under_sea {
            return ids.water;
        }
        if s.paint.1 > 0.5 && (s.paint.0 as usize) < PAINT_NAMES.len() && s.paint.0 > 0 {
            let name = PAINT_NAMES[s.paint.0 as usize];
            return match name {
                // Gravel crunches like rock where no gravel surface is registered.
                "gravel" => ids.by_name_or(name, ids.rock),
                _ => ids.by_name(name),
            };
        }
        let slope = slope as f32;
        if s.temperature_c < self.snow_below_c && slope < 0.5 {
            return ids.snow;
        }
        if slope > self.rock_slope {
            return ids.rock;
        }
        if !self.vegetation {
            return ids.by_name(&self.ground_material);
        }
        if s.height < 4.0 && s.height > -0.5 {
            return ids.sand;
        }
        if s.moisture < 0.15 && s.temperature_c > 15.0 {
            return ids.sand;
        }
        if s.moisture > 0.25 {
            ids.by_name(&self.ground_material)
        } else {
            ids.by_name(&self.bare_material)
        }
    }

    /// The color of each paint layer (index as in [`PAINT_NAMES`]).
    pub fn paint_colors(&self) -> [[f32; 3]; 11] {
        let mix =
            |a: [f32; 3], b: [f32; 3], t: f32| std::array::from_fn(|i| a[i] + (b[i] - a[i]) * t);
        [
            self.soil,
            self.mud,
            self.snow,
            self.sand,
            self.soil,
            self.lush,
            self.rock,
            [0.33, 0.32, 0.30],
            [0.025, 0.024, 0.023],
            mix(self.rock, self.soil, 0.35),
            std::array::from_fn(|i| self.soil[i] * [0.8, 0.75, 0.7][i]),
        ]
    }
}

/// Surface ids looked up once from [`SurfaceMaterials`].
#[derive(Debug, Clone)]
pub struct SurfaceIds {
    pub names: Vec<String>,
    pub water: SurfaceId,
    pub snow: SurfaceId,
    pub rock: SurfaceId,
    pub sand: SurfaceId,
}

impl SurfaceIds {
    pub fn new(materials: &SurfaceMaterials) -> Self {
        Self {
            names: materials.materials.iter().map(|m| m.name.clone()).collect(),
            water: materials.id_or_default("water"),
            snow: materials.id_or_default("snow"),
            rock: materials.id_or_default("rock"),
            sand: materials.id_or_default("sand"),
        }
    }

    pub fn by_name(&self, name: &str) -> SurfaceId {
        self.by_name_or(name, SurfaceId::default())
    }

    /// The id of `name`, or `fallback` when no surface has that name.
    pub fn by_name_or(&self, name: &str, fallback: SurfaceId) -> SurfaceId {
        self.names
            .iter()
            .position(|n| n == name)
            .map_or(fallback, |i| SurfaceId(i as u16))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample(h: f64, m: f32, t: f32) -> Sample {
        Sample {
            height: h,
            moisture: m,
            temperature_c: t,
            mare: 0.0,
            paint: (0, 0.0),
            tint: [0.0; 4],
        }
    }

    #[test]
    fn classify_layers() {
        let mats = SurfaceMaterials::default();
        let ids = SurfaceIds::new(&mats);
        let p = TerrainPalette::earth();
        let name = |id: SurfaceId| mats.get(id).name.clone();
        assert_eq!(
            name(p.classify(&sample(100.0, 0.6, 12.0), 0.05, false, &ids)),
            "grass"
        );
        assert_eq!(
            name(p.classify(&sample(100.0, 0.6, 12.0), 0.5, false, &ids)),
            "rock"
        );
        assert_eq!(
            name(p.classify(&sample(3000.0, 0.6, -10.0), 0.1, false, &ids)),
            "snow"
        );
        assert_eq!(
            name(p.classify(&sample(2.0, 0.6, 12.0), 0.02, false, &ids)),
            "sand"
        );
        assert_eq!(
            name(p.classify(&sample(-20.0, 0.6, 12.0), 0.0, true, &ids)),
            "water"
        );
        let mut painted = sample(100.0, 0.6, 12.0);
        painted.paint = (crate::edits::paint_index("mud"), 1.0);
        assert_eq!(name(p.classify(&painted, 0.05, false, &ids)), "mud");
    }
}
