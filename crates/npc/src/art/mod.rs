//! Procedural art for NPCs, animals and the showcase arena: noise, signed-distance sculpting
//! and PBR texture baking. Everything is generated once per process and shared through keyed
//! assets, so a hundred fighters reuse one set of meshes and textures.
//!
//! Without the renderer (headless tests) there are no mesh or image assets; [`ArtAssets`]
//! then reports itself unavailable and callers fall back to primitive shapes.

pub mod animals;
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
        Some(MeshRef::Handle(
            meshes.get_or_insert_keyed(&format!("npc/{key}"), make),
        ))
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
        let set = make();
        Some((
            images.insert_keyed(&names[0], set.albedo),
            images.insert_keyed(&names[1], set.normal),
            images.insert_keyed(&names[2], set.orm),
        ))
    }
}
