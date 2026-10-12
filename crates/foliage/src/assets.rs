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
use crate::meshgen::textures::{ALPHA_CUTOFF, LeafKind, leaf_texture};
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
    /// Limbed trunk for old logs.
    pub log: Option<Handle<Mesh>>,
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
    textures: HashMap<LeafStyle, Handle<Image>>,
    /// Bark texture sets, one per kind of bark, shared by every species with that bark.
    bark_textures: HashMap<BarkKind, BarkImages>,
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

impl FoliageVisuals {
    pub fn get(&self, id: crate::species::SpeciesId) -> Option<&SpeciesVisual> {
        self.species.get(id.0 as usize).and_then(|s| s.as_ref())
    }
}

fn leaf_kind(style: LeafStyle) -> LeafKind {
    match style {
        LeafStyle::Broad => LeafKind::Broad,
        LeafStyle::Small => LeafKind::Small,
        LeafStyle::LongNeedles => LeafKind::LongNeedles,
        LeafStyle::ShortNeedles => LeafKind::ShortNeedles,
        LeafStyle::Frond => LeafKind::Frond,
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
            PlantForm::Broadleaf | PlantForm::Conifer | PlantForm::Shrub | PlantForm::Fern => {
                let style = if sp.form == PlantForm::Fern {
                    LeafStyle::Frond
                } else {
                    sp.shape.leaf
                };
                Some(*visuals.textures.entry(style).or_insert_with(|| {
                    images.add(leaf_texture(leaf_kind(style), 256, 0x1eaf ^ style as u64))
                }))
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
        let visual = build_species(sp, texture, bark, &mut meshes, &mut materials);
        visuals.species[i] = Some(visual);
        tracing::debug!("foliage: built visuals for {}", sp.name);
    }
}

fn build_species(
    sp: &PlantSpecies,
    texture: Option<Handle<Image>>,
    bark_images: Option<BarkImages>,
    meshes: &mut Assets<Mesh>,
    materials: &mut Assets<StandardMaterial>,
) -> SpeciesVisual {
    let seed = name_seed(&sp.name);
    let mut variants = Vec::with_capacity(VARIANTS);
    for v in 0..VARIANTS as u64 {
        let (full, sapling) = build_meshes(sp, seed ^ (v * 0x9e37_79b9));
        let mut add = |m: Option<Mesh>| m.map(|m| meshes.add(m));
        let (sapling_wood, sapling_foliage) = match sapling {
            Some(s) => (add(s.wood), add(s.foliage)),
            None => (None, None),
        };
        variants.push(VariantMeshes {
            base: add(full.base),
            wood: add(full.wood),
            foliage: add(full.foliage),
            wood_lod: add(full.wood_lod),
            foliage_lod: add(full.foliage_lod),
            sapling_wood,
            sapling_foliage,
            log: add(full.log),
            height: full.height,
            cut_height: full.cut_height,
        });
    }

    let small = matches!(
        sp.form,
        PlantForm::Grass | PlantForm::Flower | PlantForm::Fern
    );
    let [bark, bark_rigid, dead_bark, burnt] = bark_materials(sp, bark_images);
    let leaf_material = |color: [f32; 3]| {
        let mut m = StandardMaterial::color(color[0], color[1], color[2])
            .with_roughness(0.78)
            .with_subsurface(0.3)
            .with_wind_sway(sp.wind_sway_m);
        // Leaves reflect less than the default dielectric (F0 ≈ 0.03).
        m.reflectance = 0.35;
        if let Some(t) = texture {
            m = m.with_base_color_texture(t);
            m.alpha_mode = AlphaMode::Mask(ALPHA_CUTOFF);
        }
        if sp.form == PlantForm::Flower {
            m.base_color = [1.0, 1.0, 1.0, 1.0];
            m.subsurface = 0.35;
        }
        if small {
            m.perceptual_roughness = 0.75;
        }
        m
    };
    let mix = |a: [f32; 3], b: [f32; 3], t: f32| [0, 1, 2].map(|i| a[i] + (b[i] - a[i]) * t);
    let tint_colors = [0.0, 0.35, 0.7].map(|t| mix(sp.leaf_color, sp.accent_color, t));
    let leaves = tint_colors.map(|c| materials.add(leaf_material(c)));
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
