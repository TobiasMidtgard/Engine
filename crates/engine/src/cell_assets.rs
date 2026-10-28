//! The assets a streamed cell makes, and freeing them when it unloads.
//!
//! A cell's spawn adds meshes, materials and images with `Assets::add` (procedural props, baked
//! textures). Nothing counts references to assets, so before this they stayed after the cell
//! went, and every reload added another copy: the showcase's audit saw +157 MB (plaza) and
//! +236 MB (market) left per unload. Unity's Addressables release what a scene loaded when it
//! unloads; Unreal frees what no live object references any more. Here a cell owns the
//! **unkeyed** assets added while its spawn ran (keyed ones, such as `shape:` meshes and files,
//! are shared by whoever asks for the key) and, when it opts in with
//! [`crate::stream::StreamCell::release_assets`], removes those no [`MeshRenderer`] still
//! draws once its entities are gone.
//!
//! Keyed assets the renderer makes again from their key whenever something asks (`shape:`
//! meshes of [`MeshRef::Shape`], `mat:` materials of [`MaterialRef::Inline`]) are shared but
//! cost nothing to bring back, so a releasing cell also frees the ones its entities drew that
//! nothing draws once they are gone: a scene of hand-sized props makes thousands of distinct
//! shapes, and they would otherwise stay for good.
//!
//! A module that keeps one of those handles in a cache past the unload gets a missing asset
//! (handles carry generations), not another one; the stream audit counts such meshes after a
//! reload, so a zone opts in once its module keeps mesh data, not handles, between loads.

use std::collections::HashSet;

use engine_assets::{Assets, Handle};
use engine_core::prelude::*;
use engine_render::{
    Image, MaterialRef, Mesh, MeshRef, MeshRenderer, ResolvedMesh, StandardMaterial, Vertex,
};

/// Key prefixes of assets the renderer makes again on demand (`Shape::key`,
/// `StandardMaterial::key`).
const REMADE_MESHES: &str = "shape:";
const REMADE_MATERIALS: &str = "mat:";

/// Marks of the three asset stores, taken before and after a spawn step.
#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct Marks([u64; 3]);

impl Marks {
    pub(crate) fn of(world: &World) -> Self {
        Self([
            mark::<Mesh>(world),
            mark::<StandardMaterial>(world),
            mark::<Image>(world),
        ])
    }
}

fn mark<T: engine_assets::Asset>(world: &World) -> u64 {
    world
        .get_resource::<Assets<T>>()
        .map_or(0, Assets::allocations)
}

fn added<T: engine_assets::Asset>(world: &World, from: u64, to: u64, out: &mut Vec<Handle<T>>) {
    if let Some(assets) = world.get_resource::<Assets<T>>() {
        out.extend(assets.added_between(from, to));
    }
}

/// What a cell's spawn added.
#[derive(Debug, Default)]
pub(crate) struct CellAssets {
    meshes: Vec<Handle<Mesh>>,
    materials: Vec<Handle<StandardMaterial>>,
    images: Vec<Handle<Image>>,
    /// What its entities drew (resolved handles), noted as it leaves: keyed ones the renderer
    /// remakes on demand go too if nothing else draws them.
    drawn: HashSet<(Handle<Mesh>, Handle<StandardMaterial>)>,
}

impl CellAssets {
    /// Adds what was allocated between `before` and `after`.
    pub(crate) fn note(&mut self, world: &World, before: Marks, after: Marks) {
        let [m0, t0, i0] = before.0;
        let [m1, t1, i1] = after.0;
        added(world, m0, m1, &mut self.meshes);
        added(world, t0, t1, &mut self.materials);
        added(world, i0, i1, &mut self.images);
    }

    /// Notes what `entities` draw (call before they despawn).
    pub(crate) fn note_drawn(&mut self, world: &World, entities: &[Entity]) {
        for &e in entities {
            if let Some(r) = world.get::<ResolvedMesh>(e) {
                self.drawn.insert((r.mesh, r.material));
            }
        }
    }

    /// How many of them are unkeyed and still live, and their heap bytes.
    pub(crate) fn footprint(&self, world: &World) -> (u32, u64) {
        let (mut n, mut bytes) = (0, 0);
        let mut count = |c: (u32, u64)| {
            n += c.0;
            bytes += c.1;
        };
        count(owned_footprint(world, &self.meshes));
        count(owned_footprint(world, &self.materials));
        count(owned_footprint(world, &self.images));
        (n, bytes)
    }

    /// Removes the unkeyed ones nothing draws any more: no [`MeshRenderer`] uses the mesh or
    /// the material, and no remaining material or inline material uses the image. Returns how
    /// many went and their bytes.
    pub(crate) fn release(self, world: &mut World) -> (u32, u64) {
        let mut meshes = HashSet::new();
        let mut materials = HashSet::new();
        let mut images = HashSet::new();
        for r in world.query::<&ResolvedMesh>().iter(world) {
            meshes.insert(r.mesh.index());
            materials.insert(r.material.index());
        }
        for r in world.query::<&MeshRenderer>().iter(world) {
            if let MeshRef::Handle(h) = &r.mesh {
                meshes.insert(h.index());
            }
            match &r.material {
                MaterialRef::Handle(h) => {
                    materials.insert(h.index());
                }
                MaterialRef::Inline(m) => textures(m, &mut images),
                MaterialRef::Asset(_) => {}
            }
        }
        let (remade_meshes, remade_materials): (Vec<_>, Vec<_>) = self.drawn.into_iter().unzip();
        let (mut n, mut bytes) = remove_unused(world, self.meshes, &meshes);
        let (n2, b2) = remove_unused(world, self.materials, &materials);
        let (n4, b4) = remove_remade(world, remade_meshes, &meshes, REMADE_MESHES);
        let (n5, b5) = remove_remade(world, remade_materials, &materials, REMADE_MATERIALS);
        // Images: what the materials left standing still use stays.
        if let Some(all) = world.get_resource::<Assets<StandardMaterial>>() {
            for (_, m) in all.iter() {
                textures(m, &mut images);
            }
        }
        let (n3, b3) = remove_unused(world, self.images, &images);
        n += n2 + n3 + n4 + n5;
        bytes += b2 + b3 + b4 + b5;
        (n, bytes)
    }
}

fn texture_handles(m: &StandardMaterial) -> impl Iterator<Item = Handle<Image>> {
    [
        m.base_color_texture,
        m.normal_texture,
        m.metallic_roughness_texture,
        m.emissive_texture,
        m.occlusion_texture,
        m.height_texture,
    ]
    .into_iter()
    .flatten()
}

fn textures(m: &StandardMaterial, out: &mut HashSet<u32>) {
    out.extend(texture_handles(m).map(|t| t.index()));
}

fn owned_footprint<T: engine_assets::Asset>(world: &World, handles: &[Handle<T>]) -> (u32, u64) {
    let Some(assets) = world.get_resource::<Assets<T>>() else {
        return (0, 0);
    };
    let keyed = assets.keyed_indices();
    handles
        .iter()
        .filter(|h| assets.contains(**h) && !keyed.contains(&h.index()))
        .fold((0, 0), |(n, b), h| (n + 1, b + assets.bytes_of(*h) as u64))
}

fn remove_unused<T: engine_assets::Asset>(
    world: &mut World,
    handles: Vec<Handle<T>>,
    used: &HashSet<u32>,
) -> (u32, u64) {
    let Some(mut assets) = world.get_resource_mut::<Assets<T>>() else {
        return (0, 0);
    };
    let keyed = assets.keyed_indices();
    let (mut n, mut bytes) = (0, 0);
    for h in handles {
        if keyed.contains(&h.index()) || used.contains(&h.index()) {
            continue;
        }
        let size = assets.bytes_of(h) as u64;
        // An old generation (the module removed it itself) removes nothing.
        if assets.remove(h).is_some() {
            n += 1;
            bytes += size;
        }
    }
    (n, bytes)
}

/// Removes the keyed assets among `handles` whose key starts with `prefix` (the renderer makes
/// them again from the key) and that nothing in `used` draws.
fn remove_remade<T: engine_assets::Asset>(
    world: &mut World,
    handles: Vec<Handle<T>>,
    used: &HashSet<u32>,
    prefix: &str,
) -> (u32, u64) {
    let Some(mut assets) = world.get_resource_mut::<Assets<T>>() else {
        return (0, 0);
    };
    let remade: HashSet<u32> = assets
        .keys()
        .filter(|(k, _)| k.starts_with(prefix))
        .map(|(_, h)| h.index())
        .collect();
    let (mut n, mut bytes) = (0, 0);
    let mut seen = HashSet::new();
    for h in handles {
        if !remade.contains(&h.index()) || used.contains(&h.index()) || !seen.insert(h.index()) {
            continue;
        }
        let size = assets.bytes_of(h) as u64;
        if assets.remove(h).is_some() {
            n += 1;
            bytes += size;
        }
    }
    (n, bytes)
}

/// Counts the heap bytes of meshes and images (for the cells' footprints and memory reports).
pub(crate) fn install_sizers(world: &mut World) {
    if let Some(mut meshes) = world.get_resource_mut::<Assets<Mesh>>() {
        meshes.set_sizer(|m| {
            m.vertices.capacity() * size_of::<Vertex>() + m.indices.capacity() * size_of::<u32>()
        });
    }
    if let Some(mut images) = world.get_resource_mut::<Assets<Image>>() {
        images.set_sizer(|i| i.data.capacity() + i.mips.iter().map(Vec::capacity).sum::<usize>());
    }
}
