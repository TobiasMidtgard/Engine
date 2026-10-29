//! Components and resources: plants, their genes and life stage, ecology regions, resource
//! sources (springs, mineral pools) and felled logs.

use engine_core::prelude::*;

use crate::species::{Layer, Need, SpeciesId};

/// Per-plant variation, inherited from the parent with small mutations. Two oaks never look or
/// behave quite the same, and a population on dry ground drifts toward drought-tolerant genes.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Genes {
    /// Growth speed multiplier (≈ 0.75..1.25).
    pub vigor: f32,
    /// Mature height multiplier (≈ 0.75..1.25).
    pub height: f32,
    /// Crown width multiplier (≈ 0.75..1.25).
    pub crown: f32,
    /// Leaf color shift toward the species' accent color, -1..1.
    pub hue: f32,
    /// Drought tolerance: shifts the water need down (positive) or up, -1..1.
    pub drought: f32,
    /// Shade tolerance: shifts the light need down (positive) or up, -1..1.
    pub shade: f32,
    /// Which mesh variant draws it.
    pub variant: u8,
    /// Lean of the whole plant, radians (trees reaching for light, wind-bent pines).
    pub lean: f32,
    /// Direction of the lean, radians around +Y.
    pub lean_dir: f32,
}

impl Default for Genes {
    fn default() -> Self {
        Self {
            vigor: 1.0,
            height: 1.0,
            crown: 1.0,
            hue: 0.0,
            drought: 0.0,
            shade: 0.0,
            variant: 0,
            lean: 0.0,
            lean_dir: 0.0,
        }
    }
}

impl Genes {
    /// Random genes for a plant with no parent (seed rain, hand planting).
    pub fn random(rng: &mut Rng) -> Self {
        Self {
            vigor: rng.range(0.85, 1.15),
            height: rng.range(0.82, 1.18),
            crown: rng.range(0.82, 1.18),
            hue: rng.range(-1.0, 1.0),
            drought: rng.range(-0.4, 0.4),
            shade: rng.range(-0.4, 0.4),
            variant: rng.next_u32() as u8,
            lean: rng.range(0.0, 0.06),
            lean_dir: rng.range(0.0, TAU),
        }
    }

    /// A child's genes: the parent's, mutated a little.
    pub fn inherit(&self, rng: &mut Rng) -> Self {
        let m =
            |v: f32, s: f32, lo: f32, hi: f32, rng: &mut Rng| (v + rng.range(-s, s)).clamp(lo, hi);
        Self {
            vigor: m(self.vigor, 0.08, 0.7, 1.3, rng),
            height: m(self.height, 0.08, 0.7, 1.3, rng),
            crown: m(self.crown, 0.08, 0.7, 1.3, rng),
            hue: m(self.hue, 0.35, -1.0, 1.0, rng),
            drought: m(self.drought, 0.15, -1.0, 1.0, rng),
            shade: m(self.shade, 0.15, -1.0, 1.0, rng),
            variant: rng.next_u32() as u8,
            lean: rng.range(0.0, 0.06),
            lean_dir: rng.range(0.0, TAU),
        }
    }
}

/// Where a plant is in its life.
#[derive(Debug, Clone, Copy, PartialEq, Default, Serialize, Deserialize)]
pub enum PlantStage {
    /// Alive and growing (`Plant::size` says how big).
    #[default]
    Alive,
    /// Felled: only the stump is left. Resprouting species grow back from it.
    Stump { years: f32 },
    /// Died standing (drought, shade, old age, trampling): a dry snag or wilted clump.
    Dead { years: f32 },
    /// Burnt by fire: a charred snag that eventually collapses.
    Burnt { years: f32 },
}

impl PlantStage {
    pub fn is_alive(&self) -> bool {
        matches!(self, PlantStage::Alive)
    }
}

/// A living (or dead, or chopped) plant. Put it on an entity with a `Transform` at the base of
/// the plant; the foliage module grows it, spreads its seeds, draws it and reacts to chopping
/// and fire. Spawn plants with [`crate::spawn_plant`] or the `foliage.plant` API.
#[derive(Component, Debug, Clone, PartialEq, Serialize, Deserialize)]
#[require(Transform)]
#[serde(default)]
pub struct Plant {
    pub species: SpeciesId,
    pub age_years: f32,
    /// 0..1 fraction of mature size.
    pub size: f32,
    /// 0..1. Falls when conditions are poor; the plant dies near 0.
    pub health: f32,
    /// 0..1 how good conditions were at the last ecology step.
    pub vigor: f32,
    /// What held it back the most at the last ecology step.
    pub limiting: Need,
    pub stage: PlantStage,
    pub genes: Genes,
    /// Chop damage taken so far (see `PlantSpecies::toughness`).
    pub chop_damage: f32,
}

impl Default for Plant {
    fn default() -> Self {
        Self {
            species: SpeciesId(0),
            age_years: 0.0,
            size: 0.05,
            health: 1.0,
            vigor: 1.0,
            limiting: Need::None,
            stage: PlantStage::Alive,
            genes: Genes::default(),
            chop_damage: 0.0,
        }
    }
}

impl Plant {
    pub fn new(species: SpeciesId, size: f32, genes: Genes) -> Self {
        Self {
            species,
            size,
            genes,
            ..Default::default()
        }
    }

    /// Current height, meters.
    pub fn height(&self, species: &crate::species::PlantSpecies) -> f32 {
        species.max_height_m * self.size * self.genes.height
    }

    /// Current crown radius, meters.
    pub fn crown_radius(&self, species: &crate::species::PlantSpecies) -> f32 {
        species.crown_radius_m * self.size.max(0.05) * self.genes.crown
    }

    /// Mature enough to make seeds.
    pub fn is_mature(&self) -> bool {
        self.stage.is_alive() && self.size > 0.55
    }
}

/// An area where plants grow, compete and spread on their own: a forest, a meadow, a cave.
///
/// Soil moisture, nutrients and minerals come from this region's base values with patchy
/// noise, plus every [`ResourceSource`] inside it, plus live wetness from rain (the weather's
/// `SurfaceLayers`) and the ground surface material (rock holds little water, soil a lot).
/// Light comes from `sky_light` minus the shade of taller plants. Temperature comes from the
/// weather and any heat (fire) at the plant.
///
/// Regions far from the player stop simulating and catch up (in coarse steps) when the player
/// returns, so a planet full of forests costs nothing until you visit one.
#[derive(Component, Debug, Clone, PartialEq, Serialize, Deserialize)]
#[require(Transform)]
#[serde(default)]
pub struct EcologyRegion {
    /// Half size along world X and Z, meters.
    pub half_extents: [f32; 2],
    /// Elliptical (with a ragged, natural edge) instead of rectangular.
    pub round: bool,
    /// Circles where nothing grows: ponds, paths, building plots. Local `[x, z, radius]` in
    /// meters from the region's center.
    pub keep_clear: Vec<[f32; 3]>,
    /// Circles kept free of trees and shrubs, though ground plants still grow there: sight
    /// lines, the view from a bench. Local `[x, z, radius]` like `keep_clear`.
    pub sight_lines: Vec<[f32; 3]>,
    /// Soil grid cell size, meters.
    pub cell_m: f32,
    /// Species that can arrive from outside on the wind or with animals (names).
    pub species: Vec<String>,
    /// Seeds per hectare per year that arrive from outside, so bare ground recolonizes.
    pub seed_rain: f32,
    /// Light reaching the ground before plants shade it (1 open sky, 0.02 deep cave).
    pub sky_light: f32,
    /// Base soil moisture, nutrients and minerals, 0..1.
    pub moisture: f32,
    pub nutrients: f32,
    pub minerals: f32,
    /// 0..1: how patchy the soil is.
    pub patchiness: f32,
    /// Most plants per layer (canopy, understory, ground).
    pub max_plants: [u32; 3],
    /// Simulated years per ecology step.
    pub step_years: f32,
    /// Multiplies the global growth speed in this region only (a time-lapse plot in a
    /// showcase, an enchanted grove). 1 = normal speed.
    pub time_scale: f32,
    /// Stop simulating when the nearest focus is farther than this, meters.
    pub active_radius_m: f32,
    /// History simulated once when the region first runs (an old-growth forest, a clearing
    /// cut years ago).
    pub history: RegionHistory,
    pub seed: u64,
}

impl Default for EcologyRegion {
    fn default() -> Self {
        Self {
            half_extents: [40.0, 40.0],
            round: false,
            keep_clear: Vec::new(),
            sight_lines: Vec::new(),
            cell_m: 2.0,
            species: Vec::new(),
            seed_rain: 2.0,
            sky_light: 1.0,
            moisture: 0.4,
            nutrients: 0.45,
            minerals: 0.0,
            patchiness: 0.5,
            max_plants: [600, 600, 3000],
            step_years: 0.25,
            time_scale: 1.0,
            active_radius_m: 1500.0,
            history: RegionHistory::default(),
            seed: 1,
        }
    }
}

impl EcologyRegion {
    /// Whether a plant of `layer` may take root at `local` (meters from the region's center):
    /// only ground plants grow in the sight lines.
    pub fn may_root(&self, layer: Layer, local: DVec2) -> bool {
        layer == Layer::Ground
            || !self
                .sight_lines
                .iter()
                .any(|c| local.distance(DVec2::new(c[0] as f64, c[1] as f64)) < c[2] as f64)
    }
}

/// What happened to a region before the game starts. Simulated in coarse steps when the region
/// first runs: grow for `grow_years`, apply `disturbance`, then grow `years_since` more.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct RegionHistory {
    pub grow_years: f32,
    pub disturbance: Disturbance,
    pub years_since: f32,
    /// Plant this many random seedlings per layer before growing (a head start).
    pub initial_plants: [u32; 3],
}

/// Something that cleared part of a region in its history.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub enum Disturbance {
    #[default]
    None,
    /// Every tree within `radius` of `center` (local X/Z offset) was felled.
    ClearCut { center: [f32; 2], radius: f32 },
    /// Fire burnt everything within `radius`.
    Burn { center: [f32; 2], radius: f32 },
    /// Several of the above.
    Many(Vec<Disturbance>),
}

/// Adds water, nutrients or minerals to the soil around it: a spring, a mineral pool, a
/// compost heap, a dry rocky ridge (negative values). Levels are added at the center and fade
/// to zero at `radius_m`.
#[derive(Component, Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[require(Transform)]
#[serde(default)]
pub struct ResourceSource {
    pub radius_m: f32,
    pub water: f32,
    pub nutrients: f32,
    pub minerals: f32,
}

impl Default for ResourceSource {
    fn default() -> Self {
        Self {
            radius_m: 10.0,
            water: 0.5,
            nutrients: 0.0,
            minerals: 0.0,
        }
    }
}

/// Global ecology settings.
#[derive(Resource, Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct EcologySettings {
    /// Simulated years per second of world time. Real trees take decades; games compress it
    /// (the default grows an oak in about five hours of play). Time dilation applies on top.
    pub years_per_second: f32,
    /// Pause growth everywhere.
    pub paused: bool,
    /// Draw small plants (grass, flowers, ferns, mushrooms) up to this distance, meters.
    pub small_plant_distance_m: f32,
    /// Small plants beyond this distance use their simplified mesh, meters.
    pub small_plant_lod_distance_m: f32,
    /// Trees beyond this distance use their simplified mesh, meters, scaled by
    /// `0.5 + size`: a grown tree switches at 1.5× this (105 m by default), a sapling sooner.
    pub tree_lod_distance_m: f32,
    /// Trees and shrubs beyond this distance draw their middle level (the near crown's cards
    /// flat, the limbs without twigs: about a fifth of the triangles), meters, scaled like
    /// `tree_lod_distance_m` (30 m for a grown tree by default).
    pub tree_mid_distance_m: f32,
    /// Broadleaf trees and shrubs within this distance draw their crown leaf by leaf (in full
    /// leaf), meters, scaled like `tree_lod_distance_m` (12 m for a grown tree by default:
    /// past that a leaf is under about 6 pixels at 1080p, where the near crown's cards show
    /// the same leaves for a tenth of the triangles).
    pub tree_hero_distance_m: f32,
    /// Trees beyond this distance use their cheapest mesh (a trunk and a few big crown
    /// cards, a few hundred triangles), meters, scaled like `tree_lod_distance_m` (360 m for a
    /// grown tree by default): for valleys of scenery trees seen from afar.
    pub tree_distant_distance_m: f32,
    /// Scenery trees ([`crate::SceneryTree`]) beyond this distance are not drawn, meters (not
    /// scaled by size).
    pub scenery_tree_draw_distance_m: f32,
    /// Only trees this close to a `FocusPoints` focus have a trunk collider, meters (colliders
    /// stream in and out as the focus moves; every tree has one when there is no focus).
    pub collider_distance_m: f32,
    /// Scenery trees this close to a focus are solid (trunk and thick limbs,
    /// [`crate::solids`]), meters.
    pub tree_solid_distance_m: f32,
    /// Most catch-up years a far region simulates when the player returns.
    pub max_catch_up_years: f32,
    /// Draw each region's grass as GPU blades (`engine_render::GrassField`, fed from the
    /// ecology) instead of one mesh per tuft.
    pub gpu_grass: bool,
}

impl Default for EcologySettings {
    fn default() -> Self {
        Self {
            years_per_second: 1.0 / 600.0,
            paused: false,
            small_plant_distance_m: 110.0,
            small_plant_lod_distance_m: 22.0,
            tree_lod_distance_m: 70.0,
            tree_mid_distance_m: 20.0,
            tree_hero_distance_m: 8.0,
            tree_distant_distance_m: 240.0,
            scenery_tree_draw_distance_m: 6000.0,
            collider_distance_m: 150.0,
            tree_solid_distance_m: 80.0,
            max_catch_up_years: 40.0,
            gpu_grass: true,
        }
    }
}

/// A felled tree lying on the ground (after it has fallen). It rots away over `rot_years`,
/// feeding the soil (a nurse log).
#[derive(Component, Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[require(Transform)]
#[serde(default)]
pub struct FelledLog {
    pub species: SpeciesId,
    /// Years since it fell.
    pub years: f32,
    pub rot_years: f32,
    /// Size of the tree when it fell (0..1).
    pub size: f32,
}

impl Default for FelledLog {
    fn default() -> Self {
        Self {
            species: SpeciesId(0),
            years: 0.0,
            rot_years: 12.0,
            size: 1.0,
        }
    }
}
