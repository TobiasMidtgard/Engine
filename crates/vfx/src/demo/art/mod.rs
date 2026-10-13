//! Detailed props and materials for the Spell court, made procedurally (nothing to download or
//! load from disk).
//!
//! A [`Prop`] is a list of meshes in the prop's own space (meters, +Y up, origin on the
//! ground), each tagged with the [`Surface`] it is made of. [`spawn_prop`] builds a prop the
//! first time its key is used, registers its meshes and the surfaces' materials once (every
//! copy shares them) and spawns one entity per part. Without a renderer (tests that only run
//! the simulation) nothing is spawned.
//!
//! - [`materials`]: physically based materials per [`Surface`], with generated textures.
//! - [`meshes`]: mesh building blocks (lathe, beveled boxes, rocks, paving, sweeps, …).
//! - [`props`]: the Spell court's props, built from those blocks.

pub mod materials;
pub mod meshes;
pub mod props;

use std::collections::HashMap;

use engine_assets::{Assets, Handle};
use engine_core::prelude::*;
use engine_render::{
    Image, MaterialRef, Mesh, MeshRef, MeshRenderer, RenderStats, StandardMaterial,
};

/// What a part of a prop is made of. Each surface maps to one shared material.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Surface {
    /// Worn flagstones of the court floor.
    Flagstone,
    /// Dressed, carved light granite: obelisks, portal, dais, fountain, wall.
    CarvedStone,
    /// Dark basalt for trims, plinths and inlays.
    DarkStone,
    /// Rough natural rock: pond rim, boulders, thrown stones, the golem.
    Rock,
    /// Weathered wood: plates, the anvil's stump, torch shafts.
    Wood,
    /// Forged, slightly rusty iron: anvil, braziers, bands.
    Iron,
    /// Aged bronze with green patina: fittings, rings, the fountain spout.
    Bronze,
    Mud,
    Snow,
    Sand,
    Dirt,
    /// Clear water surface (fountain basin, pond, puddles).
    Water,
    /// Shrub and ivy leaves.
    Leaves,
    /// Moss and grass tufts in cracks.
    Moss,
    /// Violet glowing rune inlays (portal).
    RuneArcane,
    /// Blue-white glowing rune inlays (obelisks).
    RuneStorm,
    /// Green glowing rune inlays (healing dais).
    RuneLife,
    /// Orange glowing inlays (caster, braziers' embers).
    RuneEmber,
    /// Faceted magic crystal (obelisk caps, the caster's focus).
    Crystal,
    /// Burning charcoal with glowing fissures (the braziers' fuel).
    Coals,
}

/// One mesh of a prop and what it is made of.
#[derive(Debug, Clone)]
pub struct Part {
    pub mesh: Mesh,
    pub surface: Surface,
}

/// A prop: meshes in the prop's own space.
#[derive(Debug, Clone, Default)]
pub struct Prop {
    pub parts: Vec<Part>,
}

impl Prop {
    pub fn new() -> Self {
        Self::default()
    }

    /// Add a part.
    pub fn with(mut self, surface: Surface, mesh: Mesh) -> Self {
        self.push(surface, mesh);
        self
    }

    /// Add a part, merging it into an existing part of the same surface (fewer draw calls).
    pub fn push(&mut self, surface: Surface, mesh: Mesh) {
        if let Some(p) = self.parts.iter_mut().find(|p| p.surface == surface) {
            p.mesh.merge(&mesh);
        } else {
            self.parts.push(Part { mesh, surface });
        }
    }

    pub fn triangle_count(&self) -> usize {
        self.parts.iter().map(|p| p.mesh.triangle_count()).sum()
    }
}

/// Meshes and materials already registered, so every copy of a prop shares them.
#[derive(Resource)]
pub struct ArtCache {
    props: HashMap<String, Vec<(Handle<Mesh>, Surface)>>,
    materials: HashMap<Surface, Handle<StandardMaterial>>,
    /// Glowing materials and the emission strength each was authored with (for dusk).
    glowing: Vec<(Handle<StandardMaterial>, f32)>,
    /// Factor on authored emission now in effect (see [`follow_exposure`]).
    glow: f32,
}

impl Default for ArtCache {
    fn default() -> Self {
        Self {
            props: HashMap::new(),
            materials: HashMap::new(),
            glowing: Vec::new(),
            glow: 1.0,
        }
    }
}

/// Exposure that glowing art (rune inlays, crystals, burning coals) is authored for: dusk.
pub const GLOW_AUTHORED_EV100: f32 = 5.5;

/// How much brighter than authored glowing art is drawn at `ev100`: it follows the camera's
/// exposure as particle glow does (`weight` is [`VfxSettings::glow_exposure`]), so it keeps its
/// color at night and still reads by day.
///
/// [`VfxSettings::glow_exposure`]: crate::VfxSettings::glow_exposure
pub fn glow_factor(ev100: f32, weight: f32) -> f32 {
    use engine_render::frame::ev100_to_exposure;
    let at = |ev: f32| crate::feature::glow_scale(ev100_to_exposure(ev), weight);
    at(ev100) / at(GLOW_AUTHORED_EV100)
}

/// Each frame, scale the glowing materials' emission to the camera's exposure (last frame's,
/// from [`RenderStats`]). Without a renderer nothing changes.
pub fn follow_exposure(
    stats: Option<Res<RenderStats>>,
    settings: Res<crate::VfxSettings>,
    cache: Option<ResMut<ArtCache>>,
    materials: Option<ResMut<Assets<StandardMaterial>>>,
) {
    let (Some(stats), Some(mut cache), Some(mut materials)) = (stats, cache, materials) else {
        return;
    };
    if stats.width == 0 {
        return;
    }
    let k = glow_factor(stats.ev100, settings.glow_exposure);
    if (k / cache.glow - 1.0).abs() < 0.02 {
        return;
    }
    cache.glow = k;
    for (handle, authored) in &cache.glowing {
        if let Some(m) = materials.get_mut(*handle) {
            m.emissive_strength = authored * k;
        }
    }
}

/// Spawn the prop registered as `key` at `at`, building it with `build` the first time the key
/// is used. Returns nothing: the entities appear when the command runs.
pub fn spawn_prop(
    commands: &mut Commands,
    key: impl Into<String>,
    at: Transform,
    build: impl FnOnce() -> Prop + Send + 'static,
) {
    let key = key.into();
    commands.queue(move |world: &mut World| {
        if !world.contains_resource::<Assets<Mesh>>()
            || !world.contains_resource::<Assets<StandardMaterial>>()
            || !world.contains_resource::<Assets<Image>>()
        {
            return;
        }
        let parts = register(world, &key, build);
        for (mesh, material) in parts {
            world.spawn((
                at,
                MeshRenderer::new(MeshRef::Handle(mesh), MaterialRef::Handle(material)),
            ));
        }
    });
}

/// Register a prop's meshes and materials (once per key) and return them.
pub fn register(
    world: &mut World,
    key: &str,
    build: impl FnOnce() -> Prop,
) -> Vec<(Handle<Mesh>, Handle<StandardMaterial>)> {
    world.init_resource::<ArtCache>();
    world.resource_scope(|world, mut cache: Mut<ArtCache>| {
        if !cache.props.contains_key(key) {
            let prop = build();
            let mut meshes = world.resource_mut::<Assets<Mesh>>();
            let parts = prop
                .parts
                .into_iter()
                .map(|p| (meshes.add(p.mesh), p.surface))
                .collect();
            cache.props.insert(key.to_string(), parts);
        }
        let parts = cache.props[key].clone();
        parts
            .into_iter()
            .map(|(mesh, surface)| {
                let material = match cache.materials.get(&surface) {
                    Some(m) => *m,
                    None => {
                        let glow = cache.glow;
                        let (m, authored) =
                            world.resource_scope(|world, mut images: Mut<Assets<Image>>| {
                                let mut made = materials::make(surface, &mut images);
                                let authored = made.emissive_strength;
                                made.emissive_strength *= glow;
                                let m = world.resource_mut::<Assets<StandardMaterial>>().add(made);
                                (m, authored)
                            });
                        if authored > 0.0 {
                            cache.glowing.push((m, authored));
                        }
                        cache.materials.insert(surface, m);
                        m
                    }
                };
                (mesh, material)
            })
            .collect()
    })
}
