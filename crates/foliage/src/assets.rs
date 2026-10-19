//! The mesh and material library: a few mesh variants per species (each with a cheaper
//! level of detail and a slender sapling form) and the materials every plant of a species
//! shares, so a whole forest draws in a handful of instanced batches.
//!
//! Built once per species the first time it is needed, and rebuilt when a species' data
//! changes (a designer or AI agent editing `PlantLibrary`).

use std::collections::HashMap;

use engine_assets::{Assets, Handle};
use engine_core::prelude::*;
use engine_render::{AlphaMode, Image, Mesh, StandardMaterial};

use crate::meshgen::bark::{BarkKind, bark_textures};
use crate::meshgen::small::small;
use crate::meshgen::small_textures::{
    Fungus, SMALL_ALPHA_CUTOFF, TextureSet, fern_pinna, fungus, meadow_atlas,
};
use crate::meshgen::textures::{ALPHA_CUTOFF, LeafKind, kind_of, leaf_textures};
use crate::meshgen::tree::{PlantMeshes, woody};
use crate::species::{LeafStyle, PlantForm, PlantLibrary, PlantSpecies, srgb};

/// Mesh variants per species.
pub const VARIANTS: usize = 4;
/// Brightness steps of glowing materials.
pub const GLOW_LEVELS: usize = 8;
/// Color variants (genes.hue) of leaf and glow materials.
pub const TINTS: usize = 3;

/// Meshes of one variant.
#[derive(Debug, Clone, Default)]
pub struct VariantMeshes {
    pub base: Option<Handle<Mesh>>,
    pub wood: Option<Handle<Mesh>>,
    pub foliage: Option<Handle<Mesh>>,
    pub wood_lod: Option<Handle<Mesh>>,
    pub foliage_lod: Option<Handle<Mesh>>,
    pub sapling_wood: Option<Handle<Mesh>>,
    pub sapling_foliage: Option<Handle<Mesh>>,
    /// Thinned crowns for leaf fall and leaf-out, one per [`crate::meshgen::tree::LEAF_STEPS`]
    /// step (deciduous species; empty otherwise): of `foliage`, `foliage_lod` and
    /// `sapling_foliage`.
    pub foliage_thin: Vec<Handle<Mesh>>,
    pub foliage_lod_thin: Vec<Handle<Mesh>>,
    pub sapling_foliage_thin: Vec<Handle<Mesh>>,
    /// Limbed trunk for old logs.
    pub log: Option<Handle<Mesh>>,
    /// The same trunk standing dead (snags and burnt trees).
    pub snag: Option<Handle<Mesh>>,
    /// Height of the full mesh, meters.
    pub height: f32,
    pub cut_height: f32,
}

/// Everything needed to draw one species.
#[derive(Debug, Clone)]
pub struct SpeciesVisual {
    /// The species data this was built from (rebuilt when it changes).
    pub built_from: PlantSpecies,
    pub variants: Vec<VariantMeshes>,
    /// Swaying wood (trunk above the cut, branches, stems).
    pub bark: Handle<StandardMaterial>,
    /// The stump and roots (rigid).
    pub bark_rigid: Handle<StandardMaterial>,
    pub leaves: [Handle<StandardMaterial>; TINTS],
    /// The summer color of each tint of `leaves` (linear RGB); the season recolors from it.
    pub leaf_colors: [[f32; 3]; TINTS],
    pub dead_leaves: Handle<StandardMaterial>,
    /// Leaves that don't sway (falling trees and logs: their mesh lies on its side).
    pub leaves_still: [Handle<StandardMaterial>; TINTS],
    pub dead_leaves_still: Handle<StandardMaterial>,
    pub dead_bark: Handle<StandardMaterial>,
    pub burnt: Handle<StandardMaterial>,
    /// `glow[tint][level]` for glowing species.
    pub glow: Vec<[Handle<StandardMaterial>; GLOW_LEVELS]>,
}

/// The library of built species visuals.
#[derive(Resource, Default)]
pub struct FoliageVisuals {
    pub species: Vec<Option<SpeciesVisual>>,
    /// Leaf texture and its normal map per leaf style (trees and shrubs).
    textures: HashMap<LeafStyle, LeafImages>,
    /// Bark texture sets, one per kind of bark, shared by every species with that bark.
    bark_textures: HashMap<BarkKind, BarkImages>,
    small_textures: HashMap<SmallKind, SmallMaps>,
}

/// The images of one bark texture set ([`crate::meshgen::bark`]).
#[derive(Debug, Clone, Copy)]
struct BarkImages {
    kind: BarkKind,
    albedo: Handle<Image>,
    normal: Handle<Image>,
    /// Occlusion (R) and roughness (G).
    orm: Handle<Image>,
}

/// Which texture set a small plant or fungus uses.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum SmallKind {
    /// Grass, wildflowers and bluebells share the meadow atlas.
    Meadow,
    Fern,
    Fungus(Fungus),
}

impl SmallKind {
    fn of(sp: &PlantSpecies) -> Option<SmallKind> {
        match sp.form {
            PlantForm::Grass | PlantForm::Flower => Some(SmallKind::Meadow),
            PlantForm::Fern => Some(SmallKind::Fern),
            PlantForm::Mushroom if sp.max_height_m > 1.0 => {
                Some(SmallKind::Fungus(Fungus::Lantern))
            }
            PlantForm::Mushroom => Some(SmallKind::Fungus(Fungus::Glowcap)),
            _ => None,
        }
    }

    fn paint(self) -> TextureSet {
        match self {
            SmallKind::Meadow => meadow_atlas(512),
            SmallKind::Fern => fern_pinna(256),
            SmallKind::Fungus(kind) => fungus(kind, 256),
        }
    }
}

/// Uploaded textures of a small plant or fungus.
#[derive(Debug, Clone, Copy)]
struct SmallMaps {
    albedo: Handle<Image>,
    normal: Handle<Image>,
    emissive: Option<Handle<Image>>,
}

impl FoliageVisuals {
    pub fn get(&self, id: crate::species::SpeciesId) -> Option<&SpeciesVisual> {
        self.species.get(id.0 as usize).and_then(|s| s.as_ref())
    }
}

/// A species' leaf texture (color with alpha cut-out) and its normal map.
#[derive(Debug, Clone, Copy)]
struct LeafImages {
    color: Handle<Image>,
    normal: Option<Handle<Image>>,
}

/// Paint the leaf images of a style: a 1024² atlas of sprays with a normal map for trees and
/// shrubs, a single 256² image for fern fronds.
fn paint_leaves(style: LeafStyle, images: &mut Assets<Image>) -> LeafImages {
    let kind = kind_of(style);
    let seed = 0x1eaf ^ style as u64;
    if kind == LeafKind::Frond {
        let t = leaf_textures(kind, 256, seed);
        return LeafImages {
            color: images.add(t.color),
            normal: None,
        };
    }
    let t = leaf_textures(kind, 1024, seed);
    LeafImages {
        color: images.add(t.color),
        normal: Some(images.add(t.normal)),
    }
}

fn build_meshes(sp: &PlantSpecies, seed: u64) -> (PlantMeshes, Option<PlantMeshes>) {
    match sp.form {
        PlantForm::Broadleaf | PlantForm::Conifer => {
            (woody(sp, seed, false), Some(woody(sp, seed ^ 0x5a, true)))
        }
        PlantForm::Shrub => (woody(sp, seed, false), None),
        _ => (small(sp, seed), None),
    }
}

fn name_seed(name: &str) -> u64 {
    name.bytes().fold(0xcbf2_9ce4_8422_2325u64, |h, b| {
        (h ^ b as u64).wrapping_mul(0x100_0000_01b3)
    })
}

/// Build (or rebuild) the visuals of every species that is missing or out of date.
pub fn ensure_species_visuals(
    library: Res<PlantLibrary>,
    mut visuals: ResMut<FoliageVisuals>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    mut images: ResMut<Assets<Image>>,
) {
    if visuals.species.len() == library.len() && !library.is_changed() {
        return;
    }
    visuals.species.resize(library.len(), None);
    for (i, sp) in library.species.iter().enumerate() {
        if visuals.species[i]
            .as_ref()
            .is_some_and(|v| &v.built_from == sp)
        {
            continue;
        }
        let texture = match sp.form {
            PlantForm::Broadleaf | PlantForm::Conifer | PlantForm::Shrub => {
                // Ferns paint their own pinna maps (`SmallKind::Fern`).
                let style = sp.shape.leaf;
                Some(
                    *visuals
                        .textures
                        .entry(style)
                        .or_insert_with(|| paint_leaves(style, &mut images)),
                )
            }
            _ => None,
        };
        let bark = BarkKind::for_species(sp).map(|kind| {
            *visuals.bark_textures.entry(kind).or_insert_with(|| {
                let painted = bark_textures(kind);
                BarkImages {
                    kind,
                    albedo: images.add(painted.albedo.clone()),
                    normal: images.add(painted.normal.clone()),
                    orm: images.add(painted.orm.clone()),
                }
            })
        });
        let small_maps = SmallKind::of(sp).map(|kind| {
            *visuals.small_textures.entry(kind).or_insert_with(|| {
                let set = kind.paint();
                SmallMaps {
                    albedo: images.add(set.albedo),
                    normal: images.add(set.normal),
                    emissive: set.emissive.map(|e| images.add(e)),
                }
            })
        });
        let visual = build_species(sp, texture, bark, small_maps, &mut meshes, &mut materials);
        visuals.species[i] = Some(visual);
        tracing::debug!("foliage: built visuals for {}", sp.name);
    }
}

fn build_species(
    sp: &PlantSpecies,
    texture: Option<LeafImages>,
    bark_images: Option<BarkImages>,
    small_maps: Option<SmallMaps>,
    meshes: &mut Assets<Mesh>,
    materials: &mut Assets<StandardMaterial>,
) -> SpeciesVisual {
    let seed = name_seed(&sp.name);
    let mut variants = Vec::with_capacity(VARIANTS);
    for v in 0..VARIANTS as u64 {
        let (full, sapling) = build_meshes(sp, seed ^ (v * 0x9e37_79b9));
        let mut add = |m: Option<Mesh>| m.map(|m| meshes.add(m));
        let (sapling_wood, sapling_foliage, sapling_thin) = match sapling {
            Some(s) => (add(s.wood), add(s.foliage), s.foliage_thin),
            None => (None, None, Vec::new()),
        };
        let mut add_all =
            |v: Vec<Mesh>| -> Vec<Handle<Mesh>> { v.into_iter().map(|m| meshes.add(m)).collect() };
        let foliage_thin = add_all(full.foliage_thin);
        let foliage_lod_thin = add_all(full.foliage_lod_thin);
        let sapling_foliage_thin = add_all(sapling_thin);
        let mut add = |m: Option<Mesh>| m.map(|m| meshes.add(m));
        variants.push(VariantMeshes {
            base: add(full.base),
            wood: add(full.wood),
            foliage: add(full.foliage),
            wood_lod: add(full.wood_lod),
            foliage_lod: add(full.foliage_lod),
            sapling_wood,
            sapling_foliage,
            foliage_thin,
            foliage_lod_thin,
            sapling_foliage_thin,
            log: add(full.log),
            snag: add(full.snag),
            height: full.height,
            cut_height: full.cut_height,
        });
    }

    let small = matches!(
        sp.form,
        PlantForm::Grass | PlantForm::Flower | PlantForm::Fern
    );
    let [mut bark, mut bark_rigid, dead_bark, burnt] = bark_materials(sp, bark_images);
    if sp.form == PlantForm::Mushroom
        && let Some(maps) = small_maps
    {
        // Buttons at the base of a cluster use the fungus maps (their UVs follow its layout):
        // pale, moist, a little translucent.
        bark = StandardMaterial::color(1.0, 1.0, 1.0)
            .with_roughness(0.5)
            .with_subsurface(0.6)
            .with_wind_sway(sp.wind_sway_m);
        bark.base_color_texture = Some(maps.albedo);
        bark.normal_texture = Some(maps.normal);
        bark_rigid = bark;
        bark_rigid.wind_sway = 0.0;
    }
    let needles = matches!(
        sp.shape.leaf,
        LeafStyle::LongNeedles | LeafStyle::ShortNeedles
    );
    let leaf_material = |color: [f32; 3]| {
        // Thin leaves let sunlight through (they glow yellow-green against the sun); needles
        // are thicker. Both are matte enough that a grazing sun doesn't turn them white.
        let mut m = StandardMaterial::color(color[0], color[1], color[2])
            .with_roughness(if needles { 0.82 } else { 0.86 })
            .with_subsurface(if needles { 0.45 } else { 0.65 })
            .with_wind_sway(sp.wind_sway_m);
        // Leaves reflect less than the default dielectric (F0 ≈ 0.03).
        m.reflectance = 0.3;
        if let Some(t) = texture {
            m = m.with_base_color_texture(t.color);
            m.alpha_mode = AlphaMode::Mask(ALPHA_CUTOFF);
            if let Some(n) = t.normal {
                // Each leaf in a spray tilts its own way and folds along its midrib.
                m = m.with_normal_texture(n);
                m.normal_scale = 1.0;
            }
        }
        // Small plants and fungi: their own albedo + normal maps (cut out for plants).
        if let Some(maps) = small_maps {
            m.base_color_texture = Some(maps.albedo);
            m.normal_texture = Some(maps.normal);
            if sp.form != PlantForm::Mushroom {
                m.alpha_mode = AlphaMode::Mask(SMALL_ALPHA_CUTOFF);
            }
        }
        if sp.form == PlantForm::Flower {
            m.base_color = [1.0, 1.0, 1.0, 1.0];
            m.subsurface = 0.35;
        }
        if small {
            m.perceptual_roughness = 0.75;
            m.reflectance = 0.35;
            if sp.form != PlantForm::Flower {
                m.subsurface = 0.3;
            }
        }
        if matches!(sp.form, PlantForm::Grass | PlantForm::Fern) {
            // Thin blades and pinnae glow when the sun is behind them.
            m.subsurface = 0.45;
        }
        m
    };
    let mix = |a: [f32; 3], b: [f32; 3], t: f32| [0, 1, 2].map(|i| a[i] + (b[i] - a[i]) * t);
    let tint_colors = [0.0, 0.35, 0.7].map(|t| mix(sp.leaf_color, sp.accent_color, t));
    let leaves = tint_colors.map(|c| materials.add(leaf_material(c)));
    // What the season recolors from (flowers keep white: their petals are in the texture).
    let leaf_colors = tint_colors.map(|c| {
        let [r, g, b, _] = leaf_material(c).base_color;
        [r, g, b]
    });
    let dead_leaves = materials.add(leaf_material(srgb(0x8a6a3c)));
    let still = |mut m: StandardMaterial| {
        m.wind_sway = 0.0;
        m
    };
    let leaves_still = tint_colors.map(|c| materials.add(still(leaf_material(c))));
    let dead_leaves_still = materials.add(still(leaf_material(srgb(0x8a6a3c))));
    let dead_bark = materials.add(dead_bark);
    let burnt = materials.add(burnt);

    let glow = match sp.glow {
        Some(g) => {
            let c = g.color;
            let tints = [
                c,
                [c[0] * 0.75, c[1], (c[2] * 1.25).min(1.0)],
                [(c[0] * 1.3).min(1.0), c[1] * 0.95, c[2] * 0.8],
            ];
            tints
                .iter()
                .map(|tint| {
                    std::array::from_fn(|level| {
                        let mut m = StandardMaterial::color(
                            sp.leaf_color[0],
                            sp.leaf_color[1],
                            sp.leaf_color[2],
                        )
                        .with_roughness(0.4)
                        .with_subsurface(0.7)
                        .with_emissive(*tint, g.nits * (level as f32 + 0.5) / GLOW_LEVELS as f32);
                        m.receives_wetness = false;
                        // Fungi carry their colors in vertex colors; the emission map lights
                        // the gills, rims and spots while cap tops stay dark.
                        if let Some(maps) = small_maps.filter(|_| sp.form == PlantForm::Mushroom) {
                            m.base_color = [1.0, 1.0, 1.0, 1.0];
                            m.base_color_texture = Some(maps.albedo);
                            m.normal_texture = Some(maps.normal);
                            m.emissive_texture = maps.emissive;
                            m.perceptual_roughness = 0.42;
                            m.subsurface = if sp.max_height_m > 1.0 { 0.9 } else { 0.75 };
                        }
                        materials.add(m)
                    })
                })
                .collect()
        }
        None => Vec::new(),
    };

    SpeciesVisual {
        built_from: sp.clone(),
        variants,
        bark: materials.add(bark),
        bark_rigid: materials.add(bark_rigid),
        leaves,
        leaf_colors,
        dead_leaves,
        leaves_still,
        dead_leaves_still,
        dead_bark,
        burnt,
        glow,
    }
}

/// Living bark (swaying), living bark (rigid: stumps, roots, logs), dead grey bark and charred
/// bark for a species. With a bark texture set, the textures carry the bark's color, relief
/// and roughness, and the vertex colors (relative to the texture's reference color) tint it:
/// moss at the foot, a dark birch base, an orange pine crown, a designer's own bark color.
fn bark_materials(sp: &PlantSpecies, images: Option<BarkImages>) -> [StandardMaterial; 4] {
    let mut bark = StandardMaterial::color(1.0, 1.0, 1.0)
        .with_roughness(0.92)
        .with_wind_sway(sp.wind_sway_m);
    if sp.form == PlantForm::Mushroom {
        bark = bark.with_subsurface(0.6).with_roughness(0.55);
    }
    let mut dead = StandardMaterial::hex(0x6a6258).with_roughness(0.95);
    let mut burnt = StandardMaterial::hex(0x141211).with_roughness(0.98);
    if let Some(img) = images {
        let painted = bark_textures(img.kind);
        let reference = painted.reference;
        let mean = painted.mean_albedo;
        // `gain` multiplies the painted albedo; vertex colors tint it relative to the
        // reference. Roughness and occlusion come from the texture.
        let textured = |m: StandardMaterial, gain: [f32; 3], roughness: f32| {
            let mut m = m
                .with_base_color_texture(img.albedo)
                .with_normal_texture(img.normal)
                .with_metallic_roughness_texture(img.orm)
                .with_occlusion_texture(img.orm)
                .with_roughness(roughness);
            let [r, g, b] = [0, 1, 2].map(|c| gain[c] / reference[c].max(1e-3));
            m.base_color = [r, g, b, 1.0];
            m.normal_scale = img.kind.normal_scale();
            m
        };
        bark = textured(bark, [1.0; 3], 1.0);
        // Dead wood weathers to silvery grey, keeping a hint of the bark's own color.
        let grey = srgb(0x7a756d);
        let dead_color = [0, 1, 2].map(|c| (0.25 * mean[c] + 0.75 * grey[c]) / mean[c].max(1e-3));
        dead = textured(dead, dead_color, 1.0);
        // Charcoal: near black, a dull sheen on the ridges.
        let char_color = [0.014, 0.012, 0.011];
        let burnt_color = [0, 1, 2].map(|c| char_color[c] / mean[c].max(1e-3));
        burnt = textured(burnt, burnt_color, 0.85);
    }
    let mut rigid = bark;
    rigid.wind_sway = 0.0;
    [bark, rigid, dead, burnt]
}
