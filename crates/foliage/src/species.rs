//! Plant species: what a plant looks like, what it needs, how fast it grows and spreads.
//!
//! Every value is data. [`PlantLibrary`] is a registered resource, so a designer, a data file or
//! an AI agent can tune a species live (`resource.patch PlantLibrary`) or add a new one
//! (`foliage.define_species`). The built-in species are a temperate forest (oak, birch, pine,
//! spruce, hazel, fern, grass, wildflowers, bluebells) and two cave mushrooms that drink
//! mineral water and glow (glowcap, lantern cap).

use engine_core::prelude::*;

/// Index of a species in [`PlantLibrary::species`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
pub struct SpeciesId(pub u16);

/// How a plant responds to one resource level: nothing at or below `min`, best between
/// `opt_min` and `opt_max`, nothing at or above `max`, linear in between.
///
/// Resource levels are 0..1 (water, light, nutrients, minerals) or °C (temperature).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Tolerance {
    pub min: f32,
    pub opt_min: f32,
    pub opt_max: f32,
    pub max: f32,
}

impl Tolerance {
    pub const fn new(min: f32, opt_min: f32, opt_max: f32, max: f32) -> Self {
        Self {
            min,
            opt_min,
            opt_max,
            max,
        }
    }

    /// Accepts any level from 0 to 1 equally (a resource the plant doesn't care about).
    pub const ANY: Tolerance = Tolerance::new(-1.0, -0.5, 1.5, 2.0);

    /// 0..1: how well a plant does at level `v`.
    pub fn response(&self, v: f32) -> f32 {
        if v <= self.min || v >= self.max {
            0.0
        } else if v < self.opt_min {
            (v - self.min) / (self.opt_min - self.min).max(1e-6)
        } else if v > self.opt_max {
            (self.max - v) / (self.max - self.opt_max).max(1e-6)
        } else {
            1.0
        }
    }

    /// The same curve moved by `delta` (genes shift a plant's tolerance a little).
    pub fn shifted(&self, delta: f32) -> Tolerance {
        Tolerance::new(
            self.min + delta,
            self.opt_min + delta,
            self.opt_max + delta,
            self.max + delta,
        )
    }
}

impl Default for Tolerance {
    fn default() -> Self {
        Tolerance::ANY
    }
}

/// The overall shape, which picks the mesh generator.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
pub enum PlantForm {
    /// Branching hardwood with a leafy crown (oak, maple, birch).
    #[default]
    Broadleaf,
    /// Straight trunk with whorls of needle-covered branches (pine, spruce).
    Conifer,
    /// Many stems from the ground with a leafy dome (hazel, berry bushes).
    Shrub,
    /// Arching fronds from a center (ferns).
    Fern,
    /// A clump of blades (grass, reeds).
    Grass,
    /// A clump of flowering stems.
    Flower,
    /// A cluster of mushrooms (caps glow when the species has a [`Glow`]).
    Mushroom,
}

/// Height layer: plants only crowd plants of the same layer, but taller plants shade everything
/// below them.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
pub enum Layer {
    #[default]
    Canopy,
    Understory,
    Ground,
}

impl Layer {
    pub const ALL: [Layer; 3] = [Layer::Canopy, Layer::Understory, Layer::Ground];

    pub fn index(self) -> usize {
        self as usize
    }
}

/// Bioluminescence: the plant glows, brighter when it is healthy and well fed with minerals.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Glow {
    /// Linear RGB glow color.
    pub color: [f32; 3],
    /// Surface luminance of a fully lit cap, cd/m².
    pub nits: f32,
    /// Light a mature, healthy plant casts on its surroundings, lumens.
    pub lumens: f32,
    /// Seconds per slow "breath" of the glow (0 = steady).
    pub pulse_secs: f32,
}

impl Default for Glow {
    fn default() -> Self {
        Self {
            color: [0.2, 1.0, 0.85],
            nits: 30.0,
            lumens: 15.0,
            pulse_secs: 7.0,
        }
    }
}

/// How a deciduous plant follows the year ([`crate::season`]): its buds open in spring, its
/// leaves turn and fall in autumn, and it stands bare (or dies back to the ground) through
/// winter. Days of the year on the northern calendar (0 = 1 January, 100 ≈ 10 April).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Seasons {
    /// Buds open: the crown fills in over the next two weeks or so.
    pub leaf_out_day: f32,
    /// Leaves start to turn.
    pub turn_day: f32,
    /// The last leaves are down.
    pub bare_day: f32,
    /// Leaves fully turned (linear RGB).
    pub autumn_color: [f32; 3],
}

impl Default for Seasons {
    fn default() -> Self {
        Self {
            leaf_out_day: 115.0,
            turn_day: 270.0,
            bare_day: 312.0,
            autumn_color: srgb(0xc29a2e),
        }
    }
}

/// Which leaf texture a species uses.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
pub enum LeafStyle {
    /// Rounded, toothed leaves on a twig spray (hazel, generic broadleaf).
    #[default]
    Broad,
    /// Many small leaves (birch, willow).
    Small,
    /// Long needles in tufts (pine).
    LongNeedles,
    /// Short dense needles on flat sprays (spruce, fir).
    ShortNeedles,
    /// Fern fronds.
    Frond,
    /// Deeply lobed leaves crowded at the shoot tips (oak).
    Lobed,
}

/// How the bark is colored along the trunk.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
pub enum BarkStyle {
    /// Even bark with a little moss at the foot.
    #[default]
    Plain,
    /// White bark with dark bands, dark and rough near the ground.
    Birch,
    /// Gray-brown at the foot, glowing orange higher up.
    Pine,
}

/// The shape knobs of the mesh generator (per species; each plant also varies by its genes and
/// mesh variant).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct PlantShape {
    /// Where the crown starts, as a fraction of height.
    pub crown_base: f32,
    /// Main branches (branches per whorl for conifers, stems for shrubs and flowers, fronds
    /// for ferns, blades for grass, mushrooms per cluster).
    pub branches: u32,
    /// Angle of the lowest and the highest main branches from vertical, degrees.
    pub angle_low_deg: f32,
    pub angle_high_deg: f32,
    /// Branch tips bend up (positive) or droop (negative), -1..1.
    pub tip_bend: f32,
    /// 0 straight .. 1 gnarled trunk and branches.
    pub gnarl: f32,
    /// Leaf card size, meters.
    pub leaf_size_m: f32,
    /// Leaf cards relative to the default (1).
    pub leaf_density: f32,
    pub leaf: LeafStyle,
    pub bark: BarkStyle,
}

impl Default for PlantShape {
    fn default() -> Self {
        Self {
            crown_base: 0.3,
            branches: 9,
            angle_low_deg: 70.0,
            angle_high_deg: 40.0,
            tip_bend: 0.2,
            gnarl: 0.4,
            leaf_size_m: 1.5,
            leaf_density: 1.0,
            leaf: LeafStyle::Broad,
            bark: BarkStyle::Plain,
        }
    }
}

/// Everything about one kind of plant. Units: meters, years, °C; resource levels 0..1.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct PlantSpecies {
    pub name: String,
    pub form: PlantForm,
    pub layer: Layer,

    // ---- size and growth
    /// Height of a mature plant, meters.
    pub max_height_m: f32,
    /// Crown (or clump) radius of a mature plant, meters.
    pub crown_radius_m: f32,
    /// Trunk radius at the base of a mature tree, meters (stems for small plants).
    pub trunk_radius_m: f32,
    /// Years to reach 80% of full size in ideal conditions.
    pub years_to_mature: f32,
    /// Typical life span, years.
    pub lifespan_years: f32,
    /// How much light the crown blocks (leaf area density; birch 0.5, oak 1.1, spruce 1.5).
    pub canopy_density: f32,

    // ---- needs (levels 0..1; temperature in °C)
    /// Soil moisture.
    pub water: Tolerance,
    /// Light relative to open sky (1 = full sun, 0 = darkness).
    pub light: Tolerance,
    /// Soil nutrients (humus, ash, rotting wood).
    pub nutrients: Tolerance,
    /// Dissolved minerals (mineral water). Most plants tolerate a little; cave fungi need it.
    pub minerals: Tolerance,
    /// Air temperature at the plant, °C (includes heat from nearby fires).
    pub temperature_c: Tolerance,
    /// How much water, nutrients and minerals a mature plant draws, relative to an oak (1).
    pub uptake: f32,

    // ---- spreading
    /// Seeds (or spores) a healthy mature plant releases per year.
    pub seeds_per_year: f32,
    /// Mean distance seeds travel, meters.
    pub seed_range_m: f32,
    /// 0..1: how much the wind carries seeds downwind (0 = acorns, 1 = dandelion fluff).
    pub wind_dispersal: f32,
    /// Chance a seed landing in ideal conditions takes root.
    pub establishment: f32,
    /// Closest distance to another plant of the same layer, meters.
    pub min_spacing_m: f32,
    /// A felled stump sends up new shoots (coppicing), so it regrows without seeds.
    pub resprouts: bool,

    // ---- looks (linear RGB)
    pub bark_color: [f32; 3],
    /// Leaf (needle, blade, cap) color; plants vary around it.
    pub leaf_color: [f32; 3],
    /// Second color plants vary toward (lighter or yellower leaves, petal color for flowers).
    pub accent_color: [f32; 3],
    /// Wind sway at the top of a mature plant, meters.
    pub wind_sway_m: f32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub glow: Option<Glow>,
    /// Deciduous: leafs out, turns and drops its leaves through the year. None = evergreen.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub seasons: Option<Seasons>,
    pub shape: PlantShape,

    // ---- fire
    /// Fuel of a mature plant, kg.
    pub fuel_kg: f32,
    /// Ignition temperature, °C.
    pub ignition_c: f32,
    /// How many chop damage points a mature trunk takes before it falls.
    pub toughness: f32,
}

impl Default for PlantSpecies {
    fn default() -> Self {
        Self {
            name: "plant".into(),
            form: PlantForm::Broadleaf,
            layer: Layer::Canopy,
            max_height_m: 12.0,
            crown_radius_m: 4.0,
            trunk_radius_m: 0.3,
            years_to_mature: 20.0,
            lifespan_years: 120.0,
            canopy_density: 1.0,
            water: Tolerance::new(0.1, 0.3, 0.8, 1.0),
            light: Tolerance::new(0.1, 0.5, 1.0, 1.01),
            nutrients: Tolerance::new(0.05, 0.3, 1.0, 1.01),
            minerals: Tolerance::new(-1.0, 0.0, 0.5, 0.85),
            temperature_c: Tolerance::new(-30.0, 5.0, 28.0, 42.0),
            uptake: 1.0,
            seeds_per_year: 5.0,
            seed_range_m: 8.0,
            wind_dispersal: 0.3,
            establishment: 0.2,
            min_spacing_m: 2.5,
            resprouts: false,
            bark_color: srgb(0x5d5247),
            leaf_color: srgb(0x4a6b2a),
            accent_color: srgb(0x7d8f2e),
            wind_sway_m: 0.25,
            glow: None,
            seasons: None,
            shape: PlantShape::default(),
            fuel_kg: 400.0,
            ignition_c: 300.0,
            toughness: 100.0,
        }
    }
}

impl PlantSpecies {
    /// Overall suitability of a spot (0..1): the scarcest resource decides (Liebig's law of
    /// the minimum). Also returns which need is limiting.
    pub fn suitability(&self, levels: &ResourceLevels, genes: &crate::plant::Genes) -> (f32, Need) {
        let checks = [
            (
                Need::Water,
                self.water
                    .shifted(-genes.drought * 0.1)
                    .response(levels.water),
            ),
            (
                Need::Light,
                self.light
                    .shifted(-genes.shade * 0.1)
                    .response(levels.light),
            ),
            (Need::Nutrients, self.nutrients.response(levels.nutrients)),
            (Need::Minerals, self.minerals.response(levels.minerals)),
            (
                Need::Temperature,
                self.temperature_c.response(levels.temperature_c),
            ),
        ];
        let mut best = (1.0f32, Need::None);
        for (need, v) in checks {
            if v < best.0 {
                best = (v, need);
            }
        }
        best
    }

    /// True for plants with a woody trunk that can be chopped and leaves a stump.
    pub fn is_tree(&self) -> bool {
        matches!(self.form, PlantForm::Broadleaf | PlantForm::Conifer)
    }

    /// Woody plants (trees and shrubs) leave a snag when they die; soft plants just wilt.
    pub fn is_woody(&self) -> bool {
        matches!(
            self.form,
            PlantForm::Broadleaf | PlantForm::Conifer | PlantForm::Shrub
        )
    }
}

/// Resource levels at one spot, as a plant there experiences them.
#[derive(Debug, Clone, Copy, PartialEq, Default, Serialize, Deserialize)]
pub struct ResourceLevels {
    pub water: f32,
    pub light: f32,
    pub nutrients: f32,
    pub minerals: f32,
    pub temperature_c: f32,
}

/// Which need holds a plant back the most.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum Need {
    /// Nothing: conditions are ideal.
    #[default]
    None,
    Water,
    Light,
    Nutrients,
    Minerals,
    Temperature,
    /// Too many neighbors drinking from the same soil.
    Crowding,
}

/// Linear RGB from an sRGB hex color like `0x4a6b2a`.
pub fn srgb(hex: u32) -> [f32; 3] {
    let c = |v: u32| {
        let s = v as f32 / 255.0;
        if s <= 0.04045 {
            s / 12.92
        } else {
            ((s + 0.055) / 1.055).powf(2.4)
        }
    };
    [c((hex >> 16) & 0xff), c((hex >> 8) & 0xff), c(hex & 0xff)]
}

/// Every species the ecology knows. A registered resource: tune needs and looks live.
#[derive(Resource, Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct PlantLibrary {
    pub species: Vec<PlantSpecies>,
}

impl Default for PlantLibrary {
    fn default() -> Self {
        Self {
            species: builtin_species(),
        }
    }
}

impl PlantLibrary {
    pub fn get(&self, id: SpeciesId) -> &PlantSpecies {
        self.species
            .get(id.0 as usize)
            .unwrap_or_else(|| &self.species[0])
    }

    pub fn id(&self, name: &str) -> Option<SpeciesId> {
        self.species
            .iter()
            .position(|s| s.name.eq_ignore_ascii_case(name))
            .map(|i| SpeciesId(i as u16))
    }

    /// Id for `name`, or a readable error listing the known species.
    pub fn id_or_err(&self, name: &str) -> Result<SpeciesId, String> {
        self.id(name).ok_or_else(|| {
            let names: Vec<&str> = self.species.iter().map(|s| s.name.as_str()).collect();
            format!(
                "Unknown species \"{name}\". Use one of: {}, or add one with foliage.define_species.",
                names.join(", ")
            )
        })
    }

    /// Add a species, or replace the one with the same name. Returns its id.
    pub fn add(&mut self, species: PlantSpecies) -> SpeciesId {
        if let Some(id) = self.id(&species.name) {
            self.species[id.0 as usize] = species;
            id
        } else {
            self.species.push(species);
            SpeciesId(self.species.len() as u16 - 1)
        }
    }

    pub fn len(&self) -> usize {
        self.species.len()
    }

    pub fn is_empty(&self) -> bool {
        self.species.is_empty()
    }

    /// The largest crown radius of any species (bounds neighbor searches).
    pub fn max_crown_radius(&self) -> f32 {
        self.species
            .iter()
            .map(|s| s.crown_radius_m)
            .fold(1.0, f32::max)
    }
}

/// The built-in species.
pub fn builtin_species() -> Vec<PlantSpecies> {
    let t = Tolerance::new;
    vec![
        PlantSpecies {
            name: "oak".into(),
            form: PlantForm::Broadleaf,
            layer: Layer::Canopy,
            max_height_m: 17.0,
            crown_radius_m: 6.0,
            trunk_radius_m: 0.42,
            years_to_mature: 35.0,
            lifespan_years: 300.0,
            canopy_density: 1.1,
            water: t(0.1, 0.3, 0.75, 0.97),
            light: t(0.04, 0.4, 1.0, 1.01),
            nutrients: t(0.12, 0.4, 1.0, 1.01),
            uptake: 1.0,
            seeds_per_year: 6.0,
            seed_range_m: 7.0,
            wind_dispersal: 0.0,
            establishment: 0.3,
            min_spacing_m: 3.2,
            resprouts: true,
            bark_color: srgb(0x6a6055),
            leaf_color: srgb(0x51702c),
            accent_color: srgb(0x7b8a2a),
            wind_sway_m: 0.22,
            // Late to leaf, late to turn: russet and brown, holding on into November.
            seasons: Some(Seasons {
                leaf_out_day: 122.0,
                turn_day: 282.0,
                bare_day: 325.0,
                autumn_color: srgb(0xa8702c),
            }),
            shape: PlantShape {
                crown_base: 0.26,
                branches: 9,
                angle_low_deg: 78.0,
                angle_high_deg: 38.0,
                tip_bend: 0.3,
                gnarl: 0.7,
                leaf_size_m: 1.25,
                leaf_density: 1.0,
                leaf: LeafStyle::Lobed,
                bark: BarkStyle::Plain,
            },
            fuel_kg: 900.0,
            toughness: 140.0,
            ..Default::default()
        },
        PlantSpecies {
            name: "birch".into(),
            form: PlantForm::Broadleaf,
            layer: Layer::Canopy,
            max_height_m: 15.0,
            crown_radius_m: 3.2,
            trunk_radius_m: 0.2,
            years_to_mature: 14.0,
            lifespan_years: 80.0,
            canopy_density: 0.55,
            water: t(0.2, 0.4, 0.95, 1.01),
            light: t(0.25, 0.6, 1.0, 1.01),
            nutrients: t(-0.1, 0.1, 1.0, 1.01),
            uptake: 0.7,
            seeds_per_year: 10.0,
            seed_range_m: 18.0,
            wind_dispersal: 0.8,
            establishment: 0.08,
            min_spacing_m: 1.8,
            resprouts: true,
            bark_color: srgb(0xe2ddd2),
            leaf_color: srgb(0x6f9234),
            accent_color: srgb(0xa3b13a),
            wind_sway_m: 0.35,
            // First to turn: clear yellow.
            seasons: Some(Seasons {
                leaf_out_day: 110.0,
                turn_day: 262.0,
                bare_day: 302.0,
                autumn_color: srgb(0xd8b13a),
            }),
            shape: PlantShape {
                crown_base: 0.34,
                branches: 14,
                angle_low_deg: 58.0,
                angle_high_deg: 24.0,
                tip_bend: -0.45,
                gnarl: 0.2,
                leaf_size_m: 0.95,
                leaf_density: 1.0,
                leaf: LeafStyle::Small,
                bark: BarkStyle::Birch,
            },
            fuel_kg: 250.0,
            toughness: 70.0,
            ..Default::default()
        },
        PlantSpecies {
            name: "pine".into(),
            form: PlantForm::Conifer,
            layer: Layer::Canopy,
            max_height_m: 20.0,
            crown_radius_m: 3.8,
            trunk_radius_m: 0.32,
            years_to_mature: 28.0,
            lifespan_years: 250.0,
            canopy_density: 0.8,
            water: t(-0.05, 0.1, 0.6, 0.92),
            light: t(0.3, 0.6, 1.0, 1.01),
            nutrients: t(-0.1, 0.05, 1.0, 1.01),
            uptake: 0.6,
            seeds_per_year: 10.0,
            seed_range_m: 22.0,
            wind_dispersal: 0.7,
            establishment: 0.1,
            min_spacing_m: 2.4,
            bark_color: srgb(0x8a5a3c),
            // Scots pine needles are a dark grey- to blue-green.
            leaf_color: srgb(0x384c3b),
            accent_color: srgb(0x48594a),
            wind_sway_m: 0.3,
            shape: PlantShape {
                crown_base: 0.58,
                branches: 6,
                angle_low_deg: 84.0,
                angle_high_deg: 62.0,
                tip_bend: 0.35,
                gnarl: 0.35,
                leaf_size_m: 1.1,
                leaf_density: 1.0,
                leaf: LeafStyle::LongNeedles,
                bark: BarkStyle::Pine,
            },
            fuel_kg: 700.0,
            ignition_c: 260.0,
            toughness: 110.0,
            ..Default::default()
        },
        PlantSpecies {
            name: "spruce".into(),
            form: PlantForm::Conifer,
            layer: Layer::Canopy,
            max_height_m: 19.0,
            crown_radius_m: 3.4,
            trunk_radius_m: 0.3,
            years_to_mature: 32.0,
            lifespan_years: 200.0,
            canopy_density: 1.5,
            water: t(0.2, 0.4, 0.9, 1.01),
            light: t(0.02, 0.25, 1.0, 1.01),
            nutrients: t(0.05, 0.2, 1.0, 1.01),
            temperature_c: t(-45.0, -5.0, 22.0, 34.0),
            uptake: 0.8,
            seeds_per_year: 10.0,
            seed_range_m: 14.0,
            wind_dispersal: 0.6,
            establishment: 0.12,
            min_spacing_m: 2.2,
            bark_color: srgb(0x5b4a3e),
            leaf_color: srgb(0x283e2c),
            accent_color: srgb(0x36523b),
            wind_sway_m: 0.25,
            shape: PlantShape {
                crown_base: 0.05,
                branches: 6,
                angle_low_deg: 102.0,
                angle_high_deg: 72.0,
                tip_bend: 0.25,
                gnarl: 0.05,
                leaf_size_m: 0.9,
                leaf_density: 1.0,
                leaf: LeafStyle::ShortNeedles,
                bark: BarkStyle::Plain,
            },
            fuel_kg: 650.0,
            ignition_c: 260.0,
            toughness: 100.0,
            ..Default::default()
        },
        PlantSpecies {
            name: "hazel".into(),
            form: PlantForm::Shrub,
            layer: Layer::Understory,
            max_height_m: 3.6,
            crown_radius_m: 2.0,
            trunk_radius_m: 0.05,
            years_to_mature: 6.0,
            lifespan_years: 60.0,
            canopy_density: 0.8,
            water: t(0.12, 0.3, 0.85, 1.0),
            light: t(0.06, 0.3, 1.0, 1.01),
            nutrients: t(0.1, 0.3, 1.0, 1.01),
            uptake: 0.3,
            seeds_per_year: 3.0,
            seed_range_m: 9.0,
            wind_dispersal: 0.0,
            establishment: 0.18,
            min_spacing_m: 1.8,
            resprouts: true,
            bark_color: srgb(0x7a6553),
            leaf_color: srgb(0x557a2e),
            accent_color: srgb(0x86993a),
            wind_sway_m: 0.25,
            // Early leaves, butter yellow in autumn.
            seasons: Some(Seasons {
                leaf_out_day: 104.0,
                turn_day: 272.0,
                bare_day: 312.0,
                autumn_color: srgb(0xc9b040),
            }),
            shape: PlantShape {
                crown_base: 0.2,
                branches: 8,
                angle_low_deg: 38.0,
                angle_high_deg: 12.0,
                tip_bend: -0.1,
                gnarl: 0.4,
                leaf_size_m: 0.8,
                leaf_density: 1.0,
                leaf: LeafStyle::Broad,
                bark: BarkStyle::Plain,
            },
            fuel_kg: 60.0,
            toughness: 25.0,
            ..Default::default()
        },
        PlantSpecies {
            name: "fern".into(),
            form: PlantForm::Fern,
            layer: Layer::Ground,
            max_height_m: 1.0,
            crown_radius_m: 0.8,
            trunk_radius_m: 0.01,
            years_to_mature: 2.0,
            lifespan_years: 25.0,
            canopy_density: 0.3,
            water: t(0.25, 0.42, 1.0, 1.01),
            light: t(-0.1, 0.08, 0.6, 1.2),
            nutrients: t(0.1, 0.3, 1.0, 1.01),
            uptake: 0.05,
            seeds_per_year: 8.0,
            seed_range_m: 9.0,
            wind_dispersal: 0.6,
            establishment: 0.2,
            min_spacing_m: 1.3,
            bark_color: srgb(0x4f5a2a),
            leaf_color: srgb(0x4c7a2c),
            accent_color: srgb(0x6e9233),
            wind_sway_m: 0.12,
            // Bracken: fronds unroll in May, turn rust and die back to the ground.
            seasons: Some(Seasons {
                leaf_out_day: 128.0,
                turn_day: 258.0,
                bare_day: 308.0,
                autumn_color: srgb(0x8c4e24),
            }),
            shape: PlantShape {
                crown_base: 0.0,
                branches: 9,
                angle_low_deg: 65.0,
                angle_high_deg: 30.0,
                tip_bend: -0.3,
                gnarl: 0.2,
                leaf_size_m: 0.5,
                leaf_density: 1.0,
                leaf: LeafStyle::Frond,
                bark: BarkStyle::Plain,
            },
            fuel_kg: 2.0,
            ignition_c: 280.0,
            toughness: 2.0,
            ..Default::default()
        },
        PlantSpecies {
            name: "grass".into(),
            form: PlantForm::Grass,
            layer: Layer::Ground,
            max_height_m: 0.55,
            crown_radius_m: 0.6,
            trunk_radius_m: 0.004,
            years_to_mature: 0.6,
            lifespan_years: 12.0,
            canopy_density: 0.05,
            water: t(0.06, 0.22, 0.85, 1.0),
            light: t(0.3, 0.65, 1.0, 1.01),
            nutrients: t(0.03, 0.2, 1.0, 1.01),
            uptake: 0.03,
            seeds_per_year: 14.0,
            seed_range_m: 3.0,
            wind_dispersal: 0.4,
            establishment: 0.35,
            min_spacing_m: 0.85,
            bark_color: srgb(0x6b7a3a),
            leaf_color: srgb(0x5f8a33),
            accent_color: srgb(0x8f9c40),
            wind_sway_m: 0.12,
            shape: PlantShape {
                crown_base: 0.0,
                branches: 90,
                angle_low_deg: 35.0,
                angle_high_deg: 5.0,
                tip_bend: -0.3,
                gnarl: 0.3,
                leaf_size_m: 0.0,
                leaf_density: 1.0,
                leaf: LeafStyle::Broad,
                bark: BarkStyle::Plain,
            },
            fuel_kg: 0.6,
            ignition_c: 250.0,
            toughness: 1.0,
            ..Default::default()
        },
        PlantSpecies {
            name: "wildflowers".into(),
            form: PlantForm::Flower,
            layer: Layer::Ground,
            max_height_m: 0.5,
            crown_radius_m: 0.45,
            trunk_radius_m: 0.004,
            years_to_mature: 0.7,
            lifespan_years: 6.0,
            canopy_density: 0.05,
            water: t(0.04, 0.16, 0.75, 0.95),
            light: t(0.4, 0.75, 1.0, 1.01),
            nutrients: t(0.04, 0.18, 1.0, 1.01),
            uptake: 0.03,
            seeds_per_year: 16.0,
            seed_range_m: 4.0,
            wind_dispersal: 0.5,
            establishment: 0.18,
            min_spacing_m: 0.9,
            bark_color: srgb(0x5b7a33),
            leaf_color: srgb(0xf2efe6),
            accent_color: srgb(0xd8392b),
            wind_sway_m: 0.1,
            // Flowers through summer, straw-colored seed heads by autumn.
            seasons: Some(Seasons {
                leaf_out_day: 112.0,
                turn_day: 240.0,
                bare_day: 285.0,
                autumn_color: srgb(0x8f7a3a),
            }),
            shape: PlantShape {
                crown_base: 0.0,
                branches: 9,
                angle_low_deg: 25.0,
                angle_high_deg: 5.0,
                tip_bend: 0.0,
                gnarl: 0.2,
                leaf_size_m: 0.04,
                leaf_density: 1.0,
                leaf: LeafStyle::Broad,
                bark: BarkStyle::Plain,
            },
            fuel_kg: 0.3,
            ignition_c: 250.0,
            toughness: 1.0,
            ..Default::default()
        },
        PlantSpecies {
            name: "bluebell".into(),
            form: PlantForm::Flower,
            layer: Layer::Ground,
            max_height_m: 0.35,
            crown_radius_m: 0.35,
            trunk_radius_m: 0.003,
            years_to_mature: 1.0,
            lifespan_years: 10.0,
            canopy_density: 0.05,
            water: t(0.2, 0.3, 0.6, 0.85),
            light: t(0.03, 0.12, 0.5, 0.9),
            nutrients: t(0.15, 0.35, 1.0, 1.01),
            uptake: 0.02,
            seeds_per_year: 3.0,
            seed_range_m: 2.5,
            wind_dispersal: 0.0,
            establishment: 0.1,
            min_spacing_m: 1.0,
            bark_color: srgb(0x4f7a35),
            leaf_color: srgb(0x5b62c8),
            accent_color: srgb(0x7b5bd0),
            wind_sway_m: 0.06,
            // Up early in spring, gone by late summer.
            seasons: Some(Seasons {
                leaf_out_day: 85.0,
                turn_day: 215.0,
                bare_day: 245.0,
                autumn_color: srgb(0x8a7a40),
            }),
            shape: PlantShape {
                crown_base: 0.0,
                branches: 7,
                angle_low_deg: 25.0,
                angle_high_deg: 10.0,
                tip_bend: -0.6,
                gnarl: 0.2,
                leaf_size_m: 0.02,
                leaf_density: 1.0,
                leaf: LeafStyle::Broad,
                bark: BarkStyle::Plain,
            },
            fuel_kg: 0.2,
            ignition_c: 260.0,
            toughness: 1.0,
            ..Default::default()
        },
        PlantSpecies {
            name: "glowcap".into(),
            form: PlantForm::Mushroom,
            layer: Layer::Ground,
            max_height_m: 0.4,
            crown_radius_m: 0.4,
            trunk_radius_m: 0.03,
            years_to_mature: 0.6,
            lifespan_years: 6.0,
            canopy_density: 0.05,
            water: t(0.3, 0.55, 1.0, 1.01),
            light: t(-1.0, -0.5, 0.15, 0.5),
            nutrients: t(0.02, 0.15, 1.0, 1.01),
            minerals: t(0.22, 0.45, 1.0, 1.01),
            uptake: 0.02,
            seeds_per_year: 20.0,
            seed_range_m: 2.5,
            wind_dispersal: 0.05,
            establishment: 0.25,
            min_spacing_m: 0.6,
            bark_color: srgb(0x9fb2ad),
            leaf_color: srgb(0x163c44),
            accent_color: srgb(0x1d5a52),
            wind_sway_m: 0.0,
            glow: Some(Glow {
                color: [0.15, 1.0, 0.8],
                nits: 28.0,
                lumens: 14.0,
                pulse_secs: 7.0,
            }),
            shape: PlantShape {
                crown_base: 0.0,
                branches: 7,
                angle_low_deg: 25.0,
                angle_high_deg: 5.0,
                tip_bend: 0.2,
                gnarl: 0.3,
                leaf_size_m: 0.0,
                leaf_density: 1.0,
                leaf: LeafStyle::Broad,
                bark: BarkStyle::Plain,
            },
            fuel_kg: 0.2,
            ignition_c: 320.0,
            toughness: 1.0,
            ..Default::default()
        },
        PlantSpecies {
            name: "lantern cap".into(),
            form: PlantForm::Mushroom,
            layer: Layer::Understory,
            max_height_m: 2.4,
            crown_radius_m: 1.1,
            trunk_radius_m: 0.12,
            years_to_mature: 4.0,
            lifespan_years: 30.0,
            canopy_density: 0.3,
            water: t(0.35, 0.55, 1.0, 1.01),
            light: t(-1.0, -0.5, 0.1, 0.4),
            nutrients: t(0.05, 0.2, 1.0, 1.01),
            minerals: t(0.25, 0.5, 1.0, 1.01),
            uptake: 0.1,
            seeds_per_year: 3.0,
            seed_range_m: 4.0,
            wind_dispersal: 0.0,
            establishment: 0.4,
            min_spacing_m: 2.4,
            bark_color: srgb(0x9a8fae),
            leaf_color: srgb(0x2c1f48),
            accent_color: srgb(0x40285e),
            wind_sway_m: 0.0,
            glow: Some(Glow {
                color: [0.62, 0.32, 1.0],
                nits: 22.0,
                lumens: 90.0,
                pulse_secs: 11.0,
            }),
            shape: PlantShape {
                crown_base: 0.0,
                branches: 2,
                angle_low_deg: 12.0,
                angle_high_deg: 4.0,
                tip_bend: 0.2,
                gnarl: 0.3,
                leaf_size_m: 0.0,
                leaf_density: 1.0,
                leaf: LeafStyle::Broad,
                bark: BarkStyle::Plain,
            },
            fuel_kg: 6.0,
            ignition_c: 320.0,
            toughness: 8.0,
            ..Default::default()
        },
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::plant::Genes;

    #[test]
    fn tolerance_curve() {
        let t = Tolerance::new(0.0, 0.2, 0.6, 1.0);
        assert_eq!(t.response(-0.1), 0.0);
        assert!((t.response(0.1) - 0.5).abs() < 1e-5);
        assert_eq!(t.response(0.4), 1.0);
        assert!((t.response(0.8) - 0.5).abs() < 1e-5);
        assert_eq!(t.response(1.0), 0.0);
    }

    #[test]
    fn scarcest_resource_limits() {
        let lib = PlantLibrary::default();
        let oak = lib.get(lib.id("oak").unwrap());
        let wet_dark = ResourceLevels {
            water: 0.5,
            light: 0.05,
            nutrients: 0.6,
            minerals: 0.0,
            temperature_c: 15.0,
        };
        let (s, need) = oak.suitability(&wet_dark, &Genes::default());
        assert!(s < 0.1, "{s}");
        assert_eq!(need, Need::Light);

        let glowcap = lib.get(lib.id("glowcap").unwrap());
        let no_minerals = ResourceLevels {
            water: 0.8,
            light: 0.0,
            nutrients: 0.5,
            minerals: 0.0,
            temperature_c: 12.0,
        };
        let (s, need) = glowcap.suitability(&no_minerals, &Genes::default());
        assert_eq!(s, 0.0);
        assert_eq!(need, Need::Minerals);
        let mineral_water = ResourceLevels {
            minerals: 0.7,
            ..no_minerals
        };
        assert!(glowcap.suitability(&mineral_water, &Genes::default()).0 > 0.9);
    }

    #[test]
    fn library_lookup_and_replace() {
        let mut lib = PlantLibrary::default();
        assert!(lib.id_or_err("baobab").unwrap_err().contains("oak"));
        let n = lib.len();
        let id = lib.add(PlantSpecies {
            name: "Oak".into(),
            max_height_m: 30.0,
            ..lib.get(lib.id("oak").unwrap()).clone()
        });
        assert_eq!(lib.len(), n);
        assert_eq!(lib.get(id).max_height_m, 30.0);
    }
}
