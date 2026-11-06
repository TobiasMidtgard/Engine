//! Procedural art for NPCs, animals and the showcase arena: noise, signed-distance sculpting
//! and PBR texture baking. Everything is generated once per process and shared through keyed
//! assets, so a hundred fighters reuse one set of meshes and textures.
//!
//! Without the renderer (headless tests) there are no mesh or image assets; [`ArtAssets`]
//! then reports itself unavailable and callers fall back to primitive shapes.

pub mod animals;
pub mod arena;
pub mod fighter;
pub mod mesh;
pub mod noise;
pub mod sdf;
pub mod tex;

use bevy_ecs::system::SystemParam;
use engine_assets::{Assets, Handle};
use engine_core::prelude::*;
use engine_render::{Image, MaterialRef, Mesh, MeshRef, StandardMaterial};

use tex::TextureSet;

/// Access to the mesh and image stores for procedural art (absent without the renderer).
#[derive(SystemParam)]
pub struct ArtAssets<'w> {
    pub meshes: Option<ResMut<'w, Assets<Mesh>>>,
    pub images: Option<ResMut<'w, Assets<Image>>>,
}

impl ArtAssets<'_> {
    /// True when meshes and images can be created (the renderer is installed).
    pub fn available(&self) -> bool {
        self.meshes.is_some() && self.images.is_some()
    }

    /// The mesh stored under `key`, built by `make` the first time.
    pub fn mesh(&mut self, key: &str, make: impl FnOnce() -> Mesh) -> Option<MeshRef> {
        let meshes = self.meshes.as_mut()?;
        Some(MeshRef::Handle(meshes.get_or_insert_keyed(
            &format!("npc/{key}"),
            || {
                // Made on the spot: a trace names what a frame spent building.
                let _span = tracing::info_span!("npc art: make mesh", key).entered();
                make()
            },
        )))
    }

    /// A textured material: `base` with the three maps baked by `make` (once per `key`).
    pub fn material(
        &mut self,
        key: &str,
        base: StandardMaterial,
        make: impl FnOnce() -> TextureSet,
    ) -> Option<MaterialRef> {
        let (albedo, normal, orm) = self.textures(key, make)?;
        let mut m = base;
        m.base_color_texture = Some(albedo);
        m.normal_texture = Some(normal);
        m.metallic_roughness_texture = Some(orm);
        m.occlusion_texture = Some(orm);
        // The ORM map carries roughness and metalness; the factors scale it.
        m.perceptual_roughness = 1.0;
        m.metallic = 1.0;
        Some(MaterialRef::Inline(m))
    }

    /// Handles of the albedo, normal and ORM images stored under `key`.
    pub fn textures(
        &mut self,
        key: &str,
        make: impl FnOnce() -> TextureSet,
    ) -> Option<(Handle<Image>, Handle<Image>, Handle<Image>)> {
        let images = self.images.as_mut()?;
        let k = format!("npc/{key}");
        let names = [
            format!("{k}/albedo"),
            format!("{k}/normal"),
            format!("{k}/orm"),
        ];
        if let (Some(a), Some(n), Some(o)) = (
            images.loaded_by_key(&names[0]),
            images.loaded_by_key(&names[1]),
            images.loaded_by_key(&names[2]),
        ) {
            return Some((a, n, o));
        }
        let set = {
            let _span = tracing::info_span!("npc art: make textures", key).entered();
            make()
        };
        Some((
            images.insert_keyed(&names[0], set.albedo),
            images.insert_keyed(&names[1], set.normal),
            images.insert_keyed(&names[2], set.orm),
        ))
    }
}

/// Where art made by a builder generic over it goes: the stores ([`ArtAssets`]), or a
/// [`Plan`] that only lists what is missing so a worker can make it off the frame.
pub trait ArtStore {
    /// True when meshes and images can be created (the renderer is installed).
    fn available(&self) -> bool;
    /// The mesh stored under `key`, built by `make` the first time.
    fn mesh(&mut self, key: &str, make: impl FnOnce() -> Mesh + Send + 'static) -> Option<MeshRef>;
    /// Handles of the albedo, normal and ORM images stored under `key`.
    fn textures(
        &mut self,
        key: &str,
        make: impl FnOnce() -> TextureSet + Send + 'static,
    ) -> Option<(Handle<Image>, Handle<Image>, Handle<Image>)>;
    /// A textured material: `base` with the three maps baked by `make` (once per `key`).
    fn material(
        &mut self,
        key: &str,
        base: StandardMaterial,
        make: impl FnOnce() -> TextureSet + Send + 'static,
    ) -> Option<MaterialRef> {
        let (albedo, normal, orm) = self.textures(key, make)?;
        let mut m = base;
        m.base_color_texture = Some(albedo);
        m.normal_texture = Some(normal);
        m.metallic_roughness_texture = Some(orm);
        m.occlusion_texture = Some(orm);
        // The ORM map carries roughness and metalness; the factors scale it.
        m.perceptual_roughness = 1.0;
        m.metallic = 1.0;
        Some(MaterialRef::Inline(m))
    }
}

impl ArtStore for ArtAssets<'_> {
    fn available(&self) -> bool {
        ArtAssets::available(self)
    }
    fn mesh(&mut self, key: &str, make: impl FnOnce() -> Mesh + Send + 'static) -> Option<MeshRef> {
        ArtAssets::mesh(self, key, make)
    }
    fn textures(
        &mut self,
        key: &str,
        make: impl FnOnce() -> TextureSet + Send + 'static,
    ) -> Option<(Handle<Image>, Handle<Image>, Handle<Image>)> {
        ArtAssets::textures(self, key, make)
    }
}

/// A dry run of a builder: what it would make on the spot, as jobs for a worker
/// ([`animals::Made`]), with stand-in handles for the missing pieces. Run the jobs, put what
/// they made in with [`animals::add_species`], and the real build finds everything stored.
pub struct Plan<'a, 'w> {
    art: &'a ArtAssets<'w>,
    pub jobs: Vec<animals::Job>,
    keys: Vec<String>,
}

impl<'a, 'w> Plan<'a, 'w> {
    pub fn new(art: &'a ArtAssets<'w>) -> Self {
        Self {
            art,
            jobs: Vec::new(),
            keys: Vec::new(),
        }
    }

    /// Each job with the key of what it makes (to tell a piece already on its way).
    pub fn into_jobs(self) -> Vec<(String, animals::Job)> {
        self.keys.into_iter().zip(self.jobs).collect()
    }

    fn missing(&mut self, key: &str) -> bool {
        !self.keys.iter().any(|k| k == key) && {
            self.keys.push(key.to_string());
            true
        }
    }
}

/// What a dry run hands back for a piece that is not made yet (never drawn: the real build
/// runs once the piece is in).
const PLANNED: u32 = u32::MAX;

impl ArtStore for Plan<'_, '_> {
    fn available(&self) -> bool {
        self.art.available()
    }
    fn mesh(&mut self, key: &str, make: impl FnOnce() -> Mesh + Send + 'static) -> Option<MeshRef> {
        let meshes = self.art.meshes.as_ref()?;
        if let Some(h) = meshes.loaded_by_key(&format!("npc/{key}")) {
            return Some(MeshRef::Handle(h));
        }
        if self.missing(key) {
            let key = key.to_string();
            self.jobs
                .push(Box::new(move || vec![animals::Made::Mesh(key, make())]));
        }
        Some(MeshRef::Handle(Handle::from_index(PLANNED)))
    }
    fn textures(
        &mut self,
        key: &str,
        make: impl FnOnce() -> TextureSet + Send + 'static,
    ) -> Option<(Handle<Image>, Handle<Image>, Handle<Image>)> {
        let images = self.art.images.as_ref()?;
        let k = format!("npc/{key}");
        if let (Some(a), Some(n), Some(o)) = (
            images.loaded_by_key(&format!("{k}/albedo")),
            images.loaded_by_key(&format!("{k}/normal")),
            images.loaded_by_key(&format!("{k}/orm")),
        ) {
            return Some((a, n, o));
        }
        if self.missing(&k) {
            let key = key.to_string();
            self.jobs
                .push(Box::new(move || vec![animals::Made::Tex(key, make())]));
        }
        let planned = Handle::from_index(PLANNED);
        Some((planned, planned, planned))
    }
}
