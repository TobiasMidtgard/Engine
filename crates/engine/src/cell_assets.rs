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
        // A cell that made and drew nothing has nothing to look for.
        if self.meshes.is_empty()
            && self.materials.is_empty()
            && self.images.is_empty()
            && self.drawn.is_empty()
        {
            return (0, 0);
        }
        // What is still drawn, as bits by slot: every mesh in the world is looked at here, in
        // the frame the cell leaves, so each costs two bits and no hashing.
        let (mut meshes, mut materials, mut images) =
            (Slots::default(), Slots::default(), Slots::default());
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
        if !self.images.is_empty()
            && let Some(all) = world.get_resource::<Assets<StandardMaterial>>()
        {
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

/// Asset slot indices as bits.
#[derive(Default)]
struct Slots(Vec<u64>);

impl Slots {
    fn insert(&mut self, index: u32) {
        let word = (index / 64) as usize;
        if self.0.len() <= word {
            self.0.resize(word + 1, 0);
        }
        self.0[word] |= 1 << (index % 64);
    }

    fn contains(&self, index: u32) -> bool {
        self.0
            .get((index / 64) as usize)
            .is_some_and(|w| w >> (index % 64) & 1 == 1)
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

fn textures(m: &StandardMaterial, out: &mut Slots) {
    for t in texture_handles(m) {
        out.insert(t.index());
    }
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
    used: &Slots,
) -> (u32, u64) {
    if handles.is_empty() {
        return (0, 0);
    }
    let Some(mut assets) = world.get_resource_mut::<Assets<T>>() else {
        return (0, 0);
    };
    let mut keyed = Slots::default();
    for (_, h) in assets.keys() {
        keyed.insert(h.index());
    }
    let go = handles
        .into_iter()
        .filter(|h| !keyed.contains(h.index()) && !used.contains(h.index()));
    remove_all(&mut assets, go)
}

/// Removes the keyed assets among `handles` whose key starts with `prefix` (the renderer makes
/// them again from the key) and that nothing in `used` draws.
fn remove_remade<T: engine_assets::Asset>(
    world: &mut World,
    handles: Vec<Handle<T>>,
    used: &Slots,
    prefix: &str,
) -> (u32, u64) {
    if handles.is_empty() {
        return (0, 0);
    }
    let Some(mut assets) = world.get_resource_mut::<Assets<T>>() else {
        return (0, 0);
    };
    let mut remade = Slots::default();
    for (key, h) in assets.keys() {
        if key.starts_with(prefix) {
            remade.insert(h.index());
        }
    }
    let go = handles
        .into_iter()
        .filter(|h| remade.contains(h.index()) && !used.contains(h.index()));
    remove_all(&mut assets, go)
}

/// Removes `handles` in one go; how many held a value, and their bytes. An old generation
/// (the module removed it itself) and one named twice remove nothing.
fn remove_all<T: engine_assets::Asset>(
    assets: &mut Assets<T>,
    handles: impl Iterator<Item = Handle<T>>,
) -> (u32, u64) {
    let sized: Vec<(Handle<T>, u64)> = handles.map(|h| (h, assets.bytes_of(h) as u64)).collect();
    let mut gone = Slots::default();
    for (h, _) in assets.remove_many(sized.iter().map(|(h, _)| *h)) {
        gone.insert(h.index());
    }
    let (mut n, mut bytes) = (0, 0);
    let mut counted = Slots::default();
    for (h, size) in sized {
        if gone.contains(h.index()) && !counted.contains(h.index()) {
            counted.insert(h.index());
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

#[cfg(test)]
mod tests {
    use super::*;

    /// A world as full as the showcase's when a zone leaves: `drawn` entities drawing meshes
    /// and materials of which most are keyed shapes, and a cell that made a few of them.
    fn full_world(drawn: u32) -> (World, CellAssets) {
        let mut world = World::new();
        let (mut meshes, mut materials, mut images) = (
            Assets::<Mesh>::default(),
            Assets::<StandardMaterial>::default(),
            Assets::<Image>::default(),
        );
        let textures: Vec<Handle<Image>> =
            (0..1_000).map(|_| images.add(Image::default())).collect();
        let shapes: Vec<Handle<Mesh>> = (0..8_000)
            .map(|i| meshes.get_or_insert_keyed(&format!("shape:{i}"), Mesh::default))
            .collect();
        let inline: Vec<Handle<StandardMaterial>> = (0..3_000)
            .map(|i| {
                materials.get_or_insert_keyed(&format!("mat:{i}"), || {
                    StandardMaterial::default().with_base_color_texture(textures[i % 1_000])
                })
            })
            .collect();
        // The cell's own: 150 meshes, 20 materials and 30 images nothing else uses, and 500
        // shapes with inline materials only it drew.
        let mut cell = CellAssets::default();
        cell.images = (0..30).map(|_| images.add(Image::default())).collect();
        cell.meshes = (0..150).map(|_| meshes.add(Mesh::default())).collect();
        cell.materials = (0..20)
            .map(|i| {
                materials.add(StandardMaterial::default().with_base_color_texture(cell.images[i]))
            })
            .collect();
        cell.drawn = (0..500).map(|i| (shapes[i], inline[i])).collect();
        world.insert_resource(meshes);
        world.insert_resource(materials);
        world.insert_resource(images);
        for i in 0..drawn as usize {
            let (mesh, material) = (shapes[500 + i % 7_500], inline[500 + i % 2_500]);
            world.spawn((
                MeshRenderer {
                    mesh: MeshRef::Handle(mesh),
                    material: MaterialRef::Handle(material),
                    ..Default::default()
                },
                ResolvedMesh { mesh, material },
            ));
        }
        (world, cell)
    }

    #[test]
    fn a_release_in_a_full_world_frees_only_the_cells_own() {
        let (mut world, cell) = full_world(2_000);
        let (n, _) = cell.release(&mut world);
        // 150 meshes, 20 materials, 30 images, and the 500 shapes and 500 inline materials
        // only its entities drew.
        assert_eq!(n, 150 + 20 + 30 + 500 + 500);
        assert_eq!(world.resource::<Assets<Mesh>>().iter().count(), 7_500);
        assert_eq!(
            world.resource::<Assets<StandardMaterial>>().iter().count(),
            2_500
        );
        assert_eq!(world.resource::<Assets<Image>>().iter().count(), 1_000);
    }

    /// How long a release takes in a full world (run with `--release --ignored --nocapture`).
    #[test]
    #[ignore]
    fn time_a_release_in_a_full_world() {
        for drawn in [20_000, 60_000, 200_000] {
            let (mut world, cell) = full_world(drawn);
            let started = std::time::Instant::now();
            let (n, _) = cell.release(&mut world);
            println!(
                "{drawn} drawn: released {n} in {:.2} ms",
                started.elapsed().as_secs_f64() * 1e3
            );
            let started = std::time::Instant::now();
            let (n, _) = CellAssets::default().release(&mut world);
            println!(
                "{drawn} drawn: a cell with nothing of its own, {n} in {:.2} ms",
                started.elapsed().as_secs_f64() * 1e3
            );
        }
    }
}
