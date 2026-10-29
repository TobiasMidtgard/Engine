//! The mesh and material library: a few mesh variants per species (each with a cheaper
//! level of detail and a slender sapling form) and the materials every plant of a species
//! shares, so a whole forest draws in a handful of instanced batches.
//!
//! Built once per species the first time it is needed, and rebuilt when a species' data
//! changes (a designer or AI agent editing `PlantLibrary`). The meshes and textures are made
//! on worker threads, a species per thread, and uploaded a few ms a frame
//! ([`crate::loading`]); plants of a species draw once its visuals are in.

use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Instant;

use engine_assets::{Assets, Handle};
use engine_core::prelude::*;
use engine_core::tasks::AsyncComputeTaskPool;
use engine_render::{AlphaMode, Image, Mesh, StandardMaterial};

use crate::loading::FoliageLoading;
use crate::meshgen::bark::{BarkKind, bark_textures};
use crate::meshgen::small::small;
use crate::meshgen::small_textures::{
    Fungus, SMALL_ALPHA_CUTOFF, TextureSet, fern_pinna, fungus, meadow_atlas,
};
use crate::meshgen::textures::{
    ALPHA_CUTOFF, HERO_ATLAS_SIZE, LeafKind, MASS_ATLAS_SIZE, atlas_seed, hero_leaf_textures,
    kind_of, leaf_textures, mass_textures,
};
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
    /// The stump (`base`) weathered, one per `tree::STUMP_WEATHER` step after the first.
    pub base_weathered: Vec<Handle<Mesh>>,
    /// The moss cushions on each of those.
    pub base_moss: Vec<Option<Handle<Mesh>>>,
    pub wood: Option<Handle<Mesh>>,
    pub foliage: Option<Handle<Mesh>>,
    /// The crown leaf by leaf, up close (broadleaves; `tree::PlantMeshes::foliage_hero`),
    /// drawn with `SpeciesVisual::leaves_hero`.
    pub foliage_hero: Option<Handle<Mesh>>,
    /// The middle level, tens of meters off (trees and shrubs; `tree::PlantMeshes::foliage_mid`).
    pub wood_mid: Option<Handle<Mesh>>,
    pub foliage_mid: Option<Handle<Mesh>>,
    pub wood_lod: Option<Handle<Mesh>>,
    pub foliage_lod: Option<Handle<Mesh>>,
    /// The cheapest level, hundreds of meters off (trees and shrubs): the whole trunk (no
    /// base with it) and a few big crown cards.
    pub wood_distant: Option<Handle<Mesh>>,
    pub foliage_distant: Option<Handle<Mesh>>,
    pub sapling_wood: Option<Handle<Mesh>>,
    pub sapling_foliage: Option<Handle<Mesh>>,
    /// Thinned crowns for leaf fall and leaf-out, one per [`crate::meshgen::tree::LEAF_STEPS`]
    /// step (deciduous species; empty otherwise): of `foliage`, `foliage_mid`, `foliage_lod`
    /// and `sapling_foliage`.
    pub foliage_thin: Vec<Handle<Mesh>>,
    pub foliage_mid_thin: Vec<Handle<Mesh>>,
    pub foliage_lod_thin: Vec<Handle<Mesh>>,
    pub foliage_distant_thin: Vec<Handle<Mesh>>,
    pub sapling_foliage_thin: Vec<Handle<Mesh>>,
    /// Limbed trunk for old logs.
    pub log: Option<Handle<Mesh>>,
    /// The same log further on in its decay, mossier (one per `tree::LOG_MOSS` step after
    /// the first).
    pub log_mossy: Vec<Handle<Mesh>>,
    /// The moss cushions on the log at each `tree::LOG_MOSS` step.
    pub log_moss: Vec<Option<Handle<Mesh>>>,
    /// The same trunk standing dead (snags and burnt trees).
    pub snag: Option<Handle<Mesh>>,
    /// Bracket fungi on the log, the snag and the stump.
    pub log_fungi: Option<Handle<Mesh>>,
    pub snag_fungi: Option<Handle<Mesh>>,
    pub stump_fungi: Option<Handle<Mesh>>,
    /// Height of the full mesh, meters.
    pub height: f32,
    pub cut_height: f32,
    /// Capsules over the trunk and the thickest limbs (`tree::PlantMeshes::collision`), and
    /// over the sapling's.
    pub collision: Vec<crate::meshgen::tree::CollisionCapsule>,
    pub sapling_collision: Vec<crate::meshgen::tree::CollisionCapsule>,
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
    /// `leaves` with the single-leaf atlas, double-sided, for crowns drawn leaf by leaf.
    pub leaves_hero: Option<[Handle<StandardMaterial>; TINTS]>,
    /// `leaves` with the mass atlas (clumps of foliage rather than single sprays), for the far
    /// and distant crowns; the same as `leaves` without an atlas.
    pub leaves_far: [Handle<StandardMaterial>; TINTS],
    /// The summer color of each tint of `leaves` (linear RGB); the season recolors from it.
    pub leaf_colors: [[f32; 3]; TINTS],
    /// How much light the summer `leaves` let through (`subsurface`); turning leaves dry out
    /// and let less through ([`crate::season::apply_season_colors`]).
    pub leaf_subsurface: f32,
    pub dead_leaves: Handle<StandardMaterial>,
    /// Leaves that don't sway (falling trees and logs: their mesh lies on its side).
    pub leaves_still: [Handle<StandardMaterial>; TINTS],
    pub dead_leaves_still: Handle<StandardMaterial>,
    pub dead_bark: Handle<StandardMaterial>,
    pub burnt: Handle<StandardMaterial>,
    /// Bracket fungi on deadwood (colors in the vertex colors).
    pub fungi: Handle<StandardMaterial>,
    /// Moss cushions on old logs (shared by every species).
    pub moss: Handle<StandardMaterial>,
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
    /// The moss cushions on old logs ([`moss_material`]), shared by every species.
    moss: Option<Handle<StandardMaterial>>,
    /// What near meshes cast their shadows with ([`crate::shadow_standin`]).
    pub(crate) standins: crate::shadow_standin::StandIns,
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
    /// The single-leaf atlas (color, normal) for crowns drawn leaf by leaf (broadleaves).
    hero: Option<(Handle<Image>, Handle<Image>)>,
    /// The mass atlas (color, normal) for far and distant crowns.
    mass: Option<(Handle<Image>, Handle<Image>)>,
}

/// Texels per side of a tree or shrub leaf atlas (up close, broadleaf crowns are drawn leaf by
/// leaf with the single-leaf atlas instead).
const LEAF_ATLAS_SIZE: u32 = 1024;

/// The painted images of a leaf style, not yet uploaded.
struct PaintedLeaves {
    color: Image,
    normal: Option<Image>,
    hero: Option<(Image, Image)>,
    mass: Option<(Image, Image)>,
}

impl PaintedLeaves {
    fn upload(self, images: &mut Assets<Image>) -> LeafImages {
        let mut pair = |(c, n): (Image, Image)| (images.add(c), images.add(n));
        let hero = self.hero.map(&mut pair);
        let mass = self.mass.map(&mut pair);
        LeafImages {
            color: images.add(self.color),
            normal: self.normal.map(|n| images.add(n)),
            hero,
            mass,
        }
    }
}

/// Paint the leaf images of a style: a [`LEAF_ATLAS_SIZE`]² atlas of sprays with a normal map
/// for trees and shrubs, a single 256² image for fern fronds.
fn paint_leaves(style: LeafStyle) -> PaintedLeaves {
    let kind = kind_of(style);
    let seed = atlas_seed(style);
    if kind == LeafKind::Frond {
        let t = leaf_textures(kind, 256, seed);
        return PaintedLeaves {
            color: t.color,
            normal: None,
            hero: None,
            mass: None,
        };
    }
    let t = leaf_textures(kind, LEAF_ATLAS_SIZE, seed);
    let hero = (!kind.is_needles()).then(|| {
        let h = hero_leaf_textures(kind, HERO_ATLAS_SIZE, seed);
        (h.color, h.normal)
    });
    let m = mass_textures(kind, MASS_ATLAS_SIZE, seed);
    PaintedLeaves {
        color: t.color,
        normal: Some(t.normal),
        hero,
        mass: Some((m.color, m.normal)),
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

/// One piece of the slow, CPU-only part of building species visuals.
#[derive(Clone)]
enum Job {
    Leaves(LeafStyle),
    /// Paints the bark kind into its shared cache ([`bark_textures`]) and copies it out.
    Bark(BarkKind),
    Small(SmallKind),
    Moss,
    Meshes(usize, Box<PlantSpecies>),
}

/// What a [`Job`] made.
enum Baked {
    Leaves(LeafStyle, PaintedLeaves),
    Bark(BarkKind, [Image; 3]),
    Small(SmallKind, TextureSet),
    Moss([Image; 3]),
    Meshes(usize, PlantSpecies, Vec<(PlantMeshes, Option<PlantMeshes>)>),
}

impl Job {
    fn run(self) -> Baked {
        match self {
            Job::Leaves(style) => Baked::Leaves(style, paint_leaves(style)),
            Job::Bark(kind) => {
                let t = bark_textures(kind);
                Baked::Bark(kind, [t.albedo.clone(), t.normal.clone(), t.orm.clone()])
            }
            Job::Small(kind) => Baked::Small(kind, kind.paint()),
            Job::Moss => Baked::Moss(paint_moss()),
            Job::Meshes(i, sp) => {
                let sp = *sp;
                let seed = name_seed(&sp.name);
                let parts = (0..VARIANTS as u64)
                    .map(|v| build_meshes(&sp, seed ^ (v * 0x9e37_79b9)))
                    .collect();
                Baked::Meshes(i, sp, parts)
            }
        }
    }
}

/// Run the jobs on every core, each thread taking the next job when it is done, and return
/// what they made in the jobs' order.
fn bake(jobs: Vec<Job>) -> Vec<Baked> {
    let next = AtomicUsize::new(0);
    let threads = std::thread::available_parallelism()
        .map_or(4, |c| c.get())
        .min(jobs.len())
        .max(1);
    let jobs = &jobs;
    let mut done: Vec<(usize, Baked)> = std::thread::scope(|scope| {
        let workers: Vec<_> = (0..threads)
            .map(|_| {
                scope.spawn(|| {
                    let mut out = Vec::new();
                    loop {
                        let k = next.fetch_add(1, Ordering::Relaxed);
                        let Some(job) = jobs.get(k) else {
                            break out;
                        };
                        out.push((k, job.clone().run()));
                    }
                })
            })
            .collect();
        workers
            .into_iter()
            .flat_map(|w| w.join().unwrap_or_default())
            .collect()
    });
    done.sort_by_key(|(k, _)| *k);
    done.into_iter().map(|(_, b)| b).collect()
}

/// The species visuals build in flight and what is waiting to be uploaded.
#[derive(Default)]
pub struct SpeciesLoad {
    bake: Option<Arc<Mutex<Option<Vec<Baked>>>>>,
    ready: VecDeque<Baked>,
}

fn is_built(visuals: &FoliageVisuals, i: usize, sp: &PlantSpecies) -> bool {
    visuals
        .species
        .get(i)
        .and_then(|v| v.as_ref())
        .is_some_and(|v| &v.built_from == sp)
}

/// Build (or rebuild) the visuals of every species that is missing or out of date: meshes and
/// textures on worker threads, then uploaded within [`FoliageLoading::budget_ms`] a frame.
#[allow(clippy::too_many_arguments)]
pub fn ensure_species_visuals(
    library: Res<PlantLibrary>,
    mut visuals: ResMut<FoliageVisuals>,
    mut loading: ResMut<FoliageLoading>,
    screen: Option<Res<engine_world::LoadingBudget>>,
    mut state: Local<SpeciesLoad>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    mut images: ResMut<Assets<Image>>,
) {
    if let Some(done) = state
        .bake
        .as_ref()
        .and_then(|slot| slot.lock().ok().and_then(|mut s| s.take()))
    {
        state.bake = None;
        state.ready.extend(done);
    }
    // Start building what is missing or out of date, one build at a time.
    let look = !loading.species_checked
        || loading.species_building > 0
        || library.is_changed()
        || visuals.species.len() != library.len();
    if look && state.bake.is_none() && state.ready.is_empty() {
        loading.species_checked = true;
        let todo: Vec<(usize, PlantSpecies)> = library
            .species
            .iter()
            .enumerate()
            .filter(|(i, sp)| !is_built(&visuals, *i, sp))
            .map(|(i, sp)| (i, sp.clone()))
            .collect();
        loading.species_building = todo.len();
        if todo.is_empty() {
            if visuals.species.len() != library.len() {
                visuals.species.truncate(library.len());
            }
        } else {
            // Shared textures first: the species need them when they are uploaded.
            let (mut leaves, mut barks, mut smalls) =
                (HashSet::new(), HashSet::new(), HashSet::new());
            let mut jobs = Vec::new();
            for (_, sp) in &todo {
                let style = sp.shape.leaf;
                if matches!(
                    sp.form,
                    PlantForm::Broadleaf | PlantForm::Conifer | PlantForm::Shrub
                ) && !visuals.textures.contains_key(&style)
                    && leaves.insert(style)
                {
                    jobs.push(Job::Leaves(style));
                }
                if let Some(kind) = BarkKind::for_species(sp)
                    && !visuals.bark_textures.contains_key(&kind)
                    && barks.insert(kind)
                {
                    jobs.push(Job::Bark(kind));
                }
                if let Some(kind) = SmallKind::of(sp)
                    && !visuals.small_textures.contains_key(&kind)
                    && smalls.insert(kind)
                {
                    jobs.push(Job::Small(kind));
                }
            }
            if visuals.moss.is_none() {
                jobs.push(Job::Moss);
            }
            jobs.extend(todo.into_iter().map(|(i, sp)| Job::Meshes(i, Box::new(sp))));
            match AsyncComputeTaskPool::try_get().filter(|_| loading.background) {
                Some(pool) => {
                    let slot = Arc::new(Mutex::new(None));
                    let out = slot.clone();
                    pool.spawn(async move {
                        let baked = bake(jobs);
                        if let Ok(mut s) = out.lock() {
                            *s = Some(baked);
                        }
                    })
                    .detach();
                    state.bake = Some(slot);
                }
                None => state.ready.extend(bake(jobs)),
            }
        }
    }
    if state.ready.is_empty() {
        return;
    }
    // Upload, within the frame's budget when loading in the background.
    let started = Instant::now();
    let budget = loading.frame_budget_ms(screen.as_deref());
    let visuals = &mut *visuals;
    visuals.species.resize(library.len(), None);
    while let Some(baked) = state.ready.pop_front() {
        let item = Instant::now();
        let what = match &baked {
            Baked::Leaves(style, _) => format!("{style:?} leaves"),
            Baked::Bark(kind, _) => format!("{kind:?} bark"),
            Baked::Small(kind, _) => format!("{kind:?} maps"),
            Baked::Moss(_) => "moss".to_string(),
            Baked::Meshes(_, sp, _) => sp.name.clone(),
        };
        match baked {
            Baked::Leaves(style, painted) => {
                visuals
                    .textures
                    .entry(style)
                    .or_insert_with(|| painted.upload(&mut images));
            }
            Baked::Bark(kind, [albedo, normal, orm]) => {
                visuals
                    .bark_textures
                    .entry(kind)
                    .or_insert_with(|| BarkImages {
                        kind,
                        albedo: images.add(albedo),
                        normal: images.add(normal),
                        orm: images.add(orm),
                    });
            }
            Baked::Small(kind, set) => {
                visuals
                    .small_textures
                    .entry(kind)
                    .or_insert_with(|| upload_small(set, &mut images));
            }
            Baked::Moss(painted) => {
                if visuals.moss.is_none() {
                    visuals.moss = Some(moss_material(painted, &mut images, &mut materials));
                }
            }
            Baked::Meshes(i, sp, parts) => {
                // A species edited meanwhile is built again from its new data.
                if library.species.get(i) == Some(&sp) {
                    let visual = upload_species(
                        &sp,
                        parts,
                        visuals,
                        &mut meshes,
                        &mut materials,
                        &mut images,
                    );
                    visuals.standins.add(&visual);
                    visuals.species[i] = Some(visual);
                    loading.species_building = loading.species_building.saturating_sub(1);
                    tracing::debug!("foliage: built visuals for {}", sp.name);
                }
            }
        }
        let ms = item.elapsed().as_secs_f32() * 1000.0;
        if ms > 4.0 {
            tracing::info!("foliage: uploading {what} took {ms:.0} ms");
        }
        if started.elapsed().as_secs_f32() * 1000.0 >= budget {
            break;
        }
    }
    if state.ready.is_empty() && state.bake.is_none() {
        loading.species_building = library
            .species
            .iter()
            .enumerate()
            .filter(|(i, sp)| !is_built(visuals, *i, sp))
            .count();
    }
}

fn upload_bark(kind: BarkKind, images: &mut Assets<Image>) -> BarkImages {
    let painted = bark_textures(kind);
    BarkImages {
        kind,
        albedo: images.add(painted.albedo.clone()),
        normal: images.add(painted.normal.clone()),
        orm: images.add(painted.orm.clone()),
    }
}

fn upload_small(set: TextureSet, images: &mut Assets<Image>) -> SmallMaps {
    SmallMaps {
        albedo: images.add(set.albedo),
        normal: images.add(set.normal),
        emissive: set.emissive.map(|e| images.add(e)),
    }
}

/// A species' visuals from its baked meshes, with the shared textures (painted in the frame
/// if a build somehow left one out).
fn upload_species(
    sp: &PlantSpecies,
    parts: Vec<(PlantMeshes, Option<PlantMeshes>)>,
    visuals: &mut FoliageVisuals,
    meshes: &mut Assets<Mesh>,
    materials: &mut Assets<StandardMaterial>,
    images: &mut Assets<Image>,
) -> SpeciesVisual {
    let texture = match sp.form {
        PlantForm::Broadleaf | PlantForm::Conifer | PlantForm::Shrub => {
            // Ferns paint their own pinna maps (`SmallKind::Fern`).
            let style = sp.shape.leaf;
            Some(
                *visuals
                    .textures
                    .entry(style)
                    .or_insert_with(|| paint_leaves(style).upload(images)),
            )
        }
        _ => None,
    };
    let bark = BarkKind::for_species(sp).map(|kind| {
        *visuals
            .bark_textures
            .entry(kind)
            .or_insert_with(|| upload_bark(kind, images))
    });
    let small_maps = SmallKind::of(sp).map(|kind| {
        *visuals
            .small_textures
            .entry(kind)
            .or_insert_with(|| upload_small(kind.paint(), images))
    });
    let moss = *visuals
        .moss
        .get_or_insert_with(|| moss_material(paint_moss(), images, materials));
    build_species(
        sp, parts, texture, bark, small_maps, moss, meshes, materials,
    )
}

#[allow(clippy::too_many_arguments)]
fn build_species(
    sp: &PlantSpecies,
    parts: Vec<(PlantMeshes, Option<PlantMeshes>)>,
    texture: Option<LeafImages>,
    bark_images: Option<BarkImages>,
    small_maps: Option<SmallMaps>,
    moss: Handle<StandardMaterial>,
    meshes: &mut Assets<Mesh>,
    materials: &mut Assets<StandardMaterial>,
) -> SpeciesVisual {
    let mut variants = Vec::with_capacity(parts.len());
    for (full, sapling) in parts {
        let mut add = |m: Option<Mesh>| m.map(|m| meshes.add(m));
        let (sapling_wood, sapling_foliage, sapling_thin, sapling_collision) = match sapling {
            Some(s) => (add(s.wood), add(s.foliage), s.foliage_thin, s.collision),
            None => (None, None, Vec::new(), Vec::new()),
        };
        let mut add_all =
            |v: Vec<Mesh>| -> Vec<Handle<Mesh>> { v.into_iter().map(|m| meshes.add(m)).collect() };
        let foliage_thin = add_all(full.foliage_thin);
        let foliage_mid_thin = add_all(full.foliage_mid_thin);
        let foliage_lod_thin = add_all(full.foliage_lod_thin);
        let foliage_distant_thin = add_all(full.foliage_distant_thin);
        let sapling_foliage_thin = add_all(sapling_thin);
        let log_mossy = add_all(full.log_mossy);
        let base_weathered = add_all(full.base_weathered);
        let base_moss = full
            .base_moss
            .into_iter()
            .map(|m| m.map(|m| meshes.add(m)))
            .collect();
        let log_moss = full
            .log_moss
            .into_iter()
            .map(|m| m.map(|m| meshes.add(m)))
            .collect();
        let mut add = |m: Option<Mesh>| m.map(|m| meshes.add(m));
        variants.push(VariantMeshes {
            base: add(full.base),
            base_weathered,
            base_moss,
            wood: add(full.wood),
            foliage: add(full.foliage),
            foliage_hero: add(full.foliage_hero),
            wood_mid: add(full.wood_mid),
            foliage_mid: add(full.foliage_mid),
            wood_lod: add(full.wood_lod),
            foliage_lod: add(full.foliage_lod),
            wood_distant: add(full.wood_distant),
            foliage_distant: add(full.foliage_distant),
            sapling_wood,
            sapling_foliage,
            foliage_thin,
            foliage_mid_thin,
            foliage_lod_thin,
            foliage_distant_thin,
            sapling_foliage_thin,
            log: add(full.log),
            log_mossy,
            log_moss,
            snag: add(full.snag),
            log_fungi: add(full.log_fungi),
            snag_fungi: add(full.snag_fungi),
            stump_fungi: add(full.stump_fungi),
            height: full.height,
            cut_height: full.cut_height,
            collision: full.collision,
            sapling_collision,
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
            .with_subsurface(if needles { 0.3 } else { 0.65 })
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
    let leaves_hero = texture.and_then(|t| t.hero).map(|(color, normal)| {
        tint_colors.map(|c| {
            let mut m = leaf_material(c);
            m.base_color_texture = Some(color);
            m.normal_texture = Some(normal);
            // One quad per leaf, seen from either side.
            m.double_sided = true;
            materials.add(m)
        })
    });
    let leaves_far = match texture.and_then(|t| t.mass) {
        Some((color, normal)) => tint_colors.map(|c| {
            let mut m = leaf_material(c);
            m.base_color_texture = Some(color);
            m.normal_texture = Some(normal);
            // A clump is many leaves deep: less light makes it through.
            m.subsurface *= 0.5;
            materials.add(m)
        }),
        None => leaves,
    };
    // What the season recolors from (flowers keep white: their petals are in the texture).
    let leaf_colors = tint_colors.map(|c| {
        let [r, g, b, _] = leaf_material(c).base_color;
        [r, g, b]
    });
    let leaf_subsurface = leaf_material(tint_colors[0]).subsurface;
    // Dead leaves have dried out: light scatters inside rather than passing through, so they
    // don't glow against the sun (with a living leaf's translucency a brown frond lit from
    // behind shone signal red).
    let dead = || {
        let mut m = leaf_material(srgb(0x8a6a3c));
        m.subsurface = 0.1;
        m
    };
    let dead_leaves = materials.add(dead());
    let still = |mut m: StandardMaterial| {
        m.wind_sway = 0.0;
        m
    };
    let leaves_still = tint_colors.map(|c| materials.add(still(leaf_material(c))));
    let dead_leaves_still = materials.add(still(dead()));
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
        leaves_hero,
        leaves_far,
        leaf_colors,
        leaf_subsurface,
        dead_leaves,
        leaves_still,
        dead_leaves_still,
        dead_bark,
        burnt,
        fungi: materials.add(
            StandardMaterial::color(1.0, 1.0, 1.0)
                .with_roughness(0.82)
                .with_subsurface(0.15),
        ),
        moss,
        glow,
    }
}

/// The moss cushions on old logs: the forest floor's moss layer
/// ([`crate::scenery::GroundLayer::Moss`]) as an alpha-masked material. The texture's alpha
/// is the cushions' height and the mesh's vertex alpha the cover, so thin cover keeps only the
/// tallest cushions. Painted: albedo and height, normal, occlusion (R) / roughness (G).
fn paint_moss() -> [Image; 3] {
    use crate::scenery::{GroundLayer, paint_layer};
    let n = 256;
    let layer = paint_layer(GroundLayer::Moss, n);
    // The layer packs normal xy, roughness and occlusion into one image; split it into a
    // normal map and an occlusion (R) / roughness (G) map.
    let mut normal = Vec::with_capacity(n * n * 4);
    let mut orm = Vec::with_capacity(n * n * 4);
    for px in layer.normal_rough_ao.as_chunks::<4>().0 {
        let x = px[0] as f32 / 127.5 - 1.0;
        let y = px[1] as f32 / 127.5 - 1.0;
        let z = (1.0 - x * x - y * y).max(0.0).sqrt();
        normal.extend_from_slice(&[px[0], px[1], ((z * 0.5 + 0.5) * 255.0).round() as u8, 255]);
        orm.extend_from_slice(&[px[3], px[2], 0, 255]);
    }
    let size = n as u32;
    [
        Image::new(size, size, layer.albedo_height, true),
        Image::new(size, size, normal, false),
        Image::new(size, size, orm, false),
    ]
}

fn moss_material(
    [albedo, normal, orm]: [Image; 3],
    images: &mut Assets<Image>,
    materials: &mut Assets<StandardMaterial>,
) -> Handle<StandardMaterial> {
    let albedo = images.add(albedo);
    let normal = images.add(normal);
    let orm = images.add(orm);
    let mut m = StandardMaterial::color(1.0, 1.0, 1.0)
        .with_base_color_texture(albedo)
        .with_normal_texture(normal)
        .with_metallic_roughness_texture(orm)
        .with_occlusion_texture(orm)
        .with_roughness(1.0)
        .with_subsurface(0.15);
    m.alpha_mode = AlphaMode::Mask(0.45);
    materials.add(m)
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
            let (strength, scale) = img.kind.detail();
            m.with_detail(strength, scale)
        };
        bark = textured(bark, [1.0; 3], 1.0);
        // Dead bark on the damp forest floor weathers to a dull grey-brown (only sun-dried wood
        // silvers), keeping some of the bark's own color.
        let grey = srgb(0x5e564c);
        let dead_color = [0, 1, 2].map(|c| (0.4 * mean[c] + 0.6 * grey[c]) / mean[c].max(1e-3));
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
