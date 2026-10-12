//! # engine_assets
//!
//! [`Handle<T>`] is a cheap, copyable reference to an asset stored in [`Assets<T>`] (a
//! resource). Assets can be created in code (`assets.add(mesh)`), registered under a key
//! so identical requests share one copy (`assets.get_or_insert_keyed("cube:1", || …)`), or
//! loaded from disk by the [`AssetServer`].
//!
//! ## Loading from disk
//!
//! [`AssetPlugin`] installs the [`AssetServer`]. Paths are relative to the asset root
//! (`EngineConfig::asset_root`, overridden by the `ENGINE_ASSET_ROOT` environment variable),
//! use forward slashes, and double as asset **keys**:
//!
//! ```ignore
//! let server = world.resource::<AssetServer>();
//! server.load_path("models/tree.glb");          // queue (idempotent), loads on a background thread
//! server.status("models/tree.glb");             // NotLoaded / Loading / Loaded / Failed(msg)
//! // Handles are stable: ask for one now, the asset appears when the load finishes.
//! let mesh: Handle<Mesh> = server.load(&mut meshes, "models/tree.glb#Mesh0/Primitive0");
//! ```
//!
//! A file can produce many assets: the primary one under the file's path
//! (`"models/tree.glb"`) and labeled sub-assets under `"path#Label"`
//! (`"models/tree.glb#Material0"`). [`AssetLoader`]s turn bytes into assets through a
//! [`LoadContext`]; finished loads are applied once per frame (in `PreUpdate`) with keyed
//! insertion, so a reload replaces the value behind the **same handle** and bumps its version.
//!
//! ## Hot reload
//!
//! With hot reload on (the default in debug builds) the server watches the asset root and
//! reloads files that change, plus every asset that read a changed file through
//! [`LoadContext::read_file`] (e.g. a `.gltf` and its `.bin`). GPU caches compare versions, so
//! textures and meshes re-upload on their own.
//!
//! ## Change tracking
//!
//! Every slot has a version. GPU-side caches compare versions to know when to re-upload, so
//! editing an asset with [`Assets::get_mut`] is all a game needs to do.
//!
//! ## AI / console API
//!
//! `assets.load {path, wait}`, `assets.status {path}`, `assets.list {}`,
//! `assets.reload {path}`.

mod server;

use std::collections::HashMap;
use std::fmt;
use std::hash::{Hash, Hasher};
use std::marker::PhantomData;

use bevy_ecs::prelude::*;
use serde::{Deserialize, Deserializer, Serialize, Serializer};

pub use server::{
    AssetInfo, AssetLoader, AssetPlugin, AssetResolver, AssetServer, LoadContext, LoadStatus,
    apply_loaded_assets, asset_file, load_now, normalize_path,
};

/// Marker for asset types.
pub trait Asset: Send + Sync + 'static {}

/// Reference to an asset in [`Assets<T>`]. Copy, compare and hash freely.
pub struct Handle<T> {
    index: u32,
    _marker: PhantomData<fn() -> T>,
}

impl<T> Handle<T> {
    /// A handle for slot `index` (normally you get handles from [`Assets`]).
    pub const fn from_index(index: u32) -> Self {
        Self {
            index,
            _marker: PhantomData,
        }
    }

    /// Slot index inside [`Assets<T>`]; also what the handle serializes to.
    pub fn index(&self) -> u32 {
        self.index
    }
}

impl<T> Clone for Handle<T> {
    fn clone(&self) -> Self {
        *self
    }
}
impl<T> Copy for Handle<T> {}
impl<T> PartialEq for Handle<T> {
    fn eq(&self, other: &Self) -> bool {
        self.index == other.index
    }
}
impl<T> Eq for Handle<T> {}
impl<T> Hash for Handle<T> {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.index.hash(state)
    }
}
impl<T> PartialOrd for Handle<T> {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}
impl<T> Ord for Handle<T> {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.index.cmp(&other.index)
    }
}
impl<T> fmt::Debug for Handle<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Handle({})", self.index)
    }
}
impl<T> Serialize for Handle<T> {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        self.index.serialize(s)
    }
}
impl<'de, T> Deserialize<'de> for Handle<T> {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        u32::deserialize(d).map(Handle::from_index)
    }
}

struct Slot<T> {
    value: Option<T>,
    version: u64,
    /// Allocated: holds a value, or is reserved for a keyed asset that is still loading.
    live: bool,
    /// A reserved slot that turned out to name the same asset as another slot (see
    /// [`Assets::forward`]): reads and writes go to that slot.
    forward: Option<u32>,
}

/// Storage for every asset of type `T`.
///
/// Slots can be *reserved* under a key before their value exists
/// ([`Assets::reserve_keyed`]): the handle is valid immediately, [`Assets::get`] returns
/// `None` and [`Assets::version`] returns 0 until the value is inserted.
#[derive(Resource)]
pub struct Assets<T: Asset> {
    slots: Vec<Slot<T>>,
    free: Vec<u32>,
    keys: HashMap<String, Handle<T>>,
    next_version: u64,
}

impl<T: Asset> Default for Assets<T> {
    fn default() -> Self {
        Self {
            slots: Vec::new(),
            free: Vec::new(),
            keys: HashMap::new(),
            next_version: 1,
        }
    }
}

impl<T: Asset> Assets<T> {
    fn bump(&mut self) -> u64 {
        self.next_version += 1;
        self.next_version
    }

    fn alloc(&mut self, value: Option<T>) -> Handle<T> {
        let version = if value.is_some() { self.bump() } else { 0 };
        let slot = Slot {
            value,
            version,
            live: true,
            forward: None,
        };
        if let Some(index) = self.free.pop() {
            self.slots[index as usize] = slot;
            Handle::from_index(index)
        } else {
            self.slots.push(slot);
            Handle::from_index(self.slots.len() as u32 - 1)
        }
    }

    fn is_live(&self, h: Handle<T>) -> bool {
        self.slots.get(h.index as usize).is_some_and(|s| s.live)
    }

    /// Index of the slot that holds `h`'s value (follows a forwarded reservation).
    fn resolve(&self, h: Handle<T>) -> usize {
        match self.slots.get(h.index as usize).and_then(|s| s.forward) {
            Some(target) => target as usize,
            None => h.index as usize,
        }
    }

    /// The handle whose slot actually holds `h`'s value: `h` itself, or the target of a
    /// forwarded reservation (see [`Assets::forward`]). GPU caches key by this so two handles
    /// that name the same asset share one upload.
    pub fn canonical(&self, h: Handle<T>) -> Handle<T> {
        Handle::from_index(self.resolve(h) as u32)
    }

    /// Make the empty reserved slot `from` an alias of `to`: from now on `from` reads and
    /// writes `to`'s value. Used when an asset was requested under two keys before loading and
    /// both turned out to name the same asset. Returns false (and does nothing) unless `from`
    /// is reserved and empty and `to` is allocated.
    pub fn forward(&mut self, from: Handle<T>, to: Handle<T>) -> bool {
        let target = self.resolve(to) as u32;
        let from_ok = self
            .slots
            .get(from.index as usize)
            .is_some_and(|s| s.live && s.value.is_none() && s.forward.is_none());
        if !from_ok || target == from.index || !self.is_live(Handle::from_index(target)) {
            return false;
        }
        for slot in &mut self.slots {
            if slot.forward == Some(from.index) {
                slot.forward = Some(target);
            }
        }
        self.slots[from.index as usize].forward = Some(target);
        true
    }

    /// Store a new asset.
    pub fn add(&mut self, value: T) -> Handle<T> {
        self.alloc(Some(value))
    }

    /// Return the asset registered under `key`, creating it with `make` the first time (or
    /// filling a slot reserved with [`Assets::reserve_keyed`]).
    pub fn get_or_insert_keyed(&mut self, key: &str, make: impl FnOnce() -> T) -> Handle<T> {
        if let Some(h) = self.keys.get(key).copied()
            && self.is_live(h)
        {
            if self.get(h).is_none() {
                self.set(h, make());
            }
            return h;
        }
        let h = self.add(make());
        self.keys.insert(key.to_string(), h);
        h
    }

    /// Insert or replace the asset under `key`. Replacing keeps the handle and bumps the
    /// version (this is how reloads reach the GPU).
    pub fn insert_keyed(&mut self, key: &str, value: T) -> Handle<T> {
        if let Some(h) = self.keys.get(key).copied()
            && self.is_live(h)
        {
            self.set(h, value);
            return h;
        }
        let h = self.add(value);
        self.keys.insert(key.to_string(), h);
        h
    }

    /// Handle for `key`, reserving an empty slot if nothing is registered under it yet.
    /// The handle stays the same when the value arrives (or is replaced) later.
    pub fn reserve_keyed(&mut self, key: &str) -> Handle<T> {
        if let Some(h) = self.keys.get(key).copied()
            && self.is_live(h)
        {
            return h;
        }
        let h = self.alloc(None);
        self.keys.insert(key.to_string(), h);
        h
    }

    /// Make `key` another name for the asset behind `handle` (e.g. a model file's primary key
    /// pointing at its only mesh). Returns false if the handle is not allocated.
    pub fn set_key(&mut self, key: &str, handle: Handle<T>) -> bool {
        if !self.is_live(handle) {
            return false;
        }
        self.keys.insert(key.to_string(), handle);
        true
    }

    /// Handle registered under `key` (possibly reserved and not loaded yet; see
    /// [`Assets::loaded_by_key`]).
    pub fn by_key(&self, key: &str) -> Option<Handle<T>> {
        self.keys.get(key).copied()
    }

    /// Handle registered under `key`, only if its value is present.
    pub fn loaded_by_key(&self, key: &str) -> Option<Handle<T>> {
        self.by_key(key).filter(|h| self.get(*h).is_some())
    }

    /// Every key and the handle it names (aliases included).
    pub fn keys(&self) -> impl Iterator<Item = (&str, Handle<T>)> {
        self.keys.iter().map(|(k, h)| (k.as_str(), *h))
    }

    /// The asset's value, if present.
    pub fn get(&self, h: Handle<T>) -> Option<&T> {
        self.slots.get(self.resolve(h))?.value.as_ref()
    }

    /// True if the handle's value is present (false while a reserved asset is loading).
    pub fn contains(&self, h: Handle<T>) -> bool {
        self.get(h).is_some()
    }

    /// Mutable access; marks the asset changed.
    pub fn get_mut(&mut self, h: Handle<T>) -> Option<&mut T> {
        let version = self.bump();
        let index = self.resolve(h);
        let slot = self.slots.get_mut(index)?;
        if slot.value.is_some() {
            slot.version = version;
        }
        slot.value.as_mut()
    }

    /// Replace an allocated asset's value, or fill a reserved slot (marks it changed).
    /// Does nothing for handles whose slot was removed.
    pub fn set(&mut self, h: Handle<T>, value: T) {
        let version = self.bump();
        let index = self.resolve(h);
        if let Some(slot) = self.slots.get_mut(index)
            && slot.live
        {
            slot.value = Some(value);
            slot.version = version;
        }
    }

    /// Remove an asset (or a reservation) and every key that names it.
    pub fn remove(&mut self, h: Handle<T>) -> Option<T> {
        let slot = self.slots.get_mut(h.index as usize)?;
        if !slot.live {
            return None;
        }
        let v = slot.value.take();
        slot.live = false;
        slot.version = 0;
        slot.forward = None;
        self.free.push(h.index);
        self.keys.retain(|_, k| *k != h);
        // Reservations forwarded here go back to waiting (their keys were removed above or
        // never pointed at them), so a reused index can never leak into them.
        for slot in &mut self.slots {
            if slot.forward == Some(h.index) {
                slot.forward = None;
            }
        }
        v
    }

    /// Version of the asset's current value (changes on every edit). 0 if missing or still
    /// loading.
    pub fn version(&self, h: Handle<T>) -> u64 {
        self.slots
            .get(self.resolve(h))
            .filter(|s| s.value.is_some())
            .map(|s| s.version)
            .unwrap_or(0)
    }

    /// Highest version of any asset: changes whenever anything was added or edited.
    pub fn latest_version(&self) -> u64 {
        self.next_version
    }

    /// Number of assets with a value.
    pub fn len(&self) -> usize {
        self.slots.iter().filter(|s| s.value.is_some()).count()
    }

    /// True if no asset has a value.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Capacity of the index space (largest handle index + 1).
    pub fn index_capacity(&self) -> usize {
        self.slots.len()
    }

    /// Every asset with a value.
    pub fn iter(&self) -> impl Iterator<Item = (Handle<T>, &T)> {
        self.slots
            .iter()
            .enumerate()
            .filter_map(|(i, s)| s.value.as_ref().map(|v| (Handle::from_index(i as u32), v)))
    }
}

/// Asset setup on [`engine_core::App`].
pub trait AssetAppExt {
    /// Install `Assets<T>` (idempotent).
    fn init_asset<T: Asset>(&mut self) -> &mut Self;

    /// Register a loader with the [`AssetServer`] (creating the server if [`AssetPlugin`] has
    /// not been added yet). Later registrations win for the same extension.
    fn register_asset_loader(&mut self, loader: impl AssetLoader) -> &mut Self;
}

impl AssetAppExt for engine_core::App {
    fn init_asset<T: Asset>(&mut self) -> &mut Self {
        if !self.world.contains_resource::<Assets<T>>() {
            self.world.insert_resource(Assets::<T>::default());
        }
        self
    }

    fn register_asset_loader(&mut self, loader: impl AssetLoader) -> &mut Self {
        if !self.world.contains_resource::<AssetServer>() {
            let server = AssetServer::from_config(&self.world, None, cfg!(debug_assertions));
            self.world.insert_resource(server);
        }
        self.world.resource::<AssetServer>().register_loader(loader);
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Thing(u32);
    impl Asset for Thing {}

    #[test]
    fn keyed_assets_are_shared_and_versioned() {
        let mut a = Assets::<Thing>::default();
        let h1 = a.get_or_insert_keyed("x", || Thing(1));
        let h2 = a.get_or_insert_keyed("x", || Thing(2));
        assert_eq!(h1, h2);
        let v = a.version(h1);
        a.get_mut(h1).unwrap().0 = 5;
        assert!(a.version(h1) > v);
        assert_eq!(a.get(h1).unwrap().0, 5);
        a.remove(h1);
        assert!(a.get(h1).is_none());
        let h3 = a.add(Thing(9));
        assert_eq!(h3.index(), h1.index());
    }

    #[test]
    fn reserved_handles_fill_in_place() {
        let mut a = Assets::<Thing>::default();
        let h = a.reserve_keyed("model.glb#Mesh0");
        assert!(a.get(h).is_none());
        assert_eq!(a.version(h), 0);
        assert_eq!(a.len(), 0);
        assert_eq!(a.loaded_by_key("model.glb#Mesh0"), None);
        // Another add must not reuse the reserved slot.
        let other = a.add(Thing(7));
        assert_ne!(other, h);
        assert_eq!(a.insert_keyed("model.glb#Mesh0", Thing(1)), h);
        let v1 = a.version(h);
        assert!(v1 > 0);
        // Replacing keeps the handle and bumps the version.
        assert_eq!(a.insert_keyed("model.glb#Mesh0", Thing(2)), h);
        assert!(a.version(h) > v1);
        assert_eq!(a.get(h).unwrap().0, 2);
        // get_or_insert_keyed fills reservations too.
        let r = a.reserve_keyed("late");
        assert_eq!(a.get_or_insert_keyed("late", || Thing(3)), r);
        assert_eq!(a.get(r).unwrap().0, 3);
        // Aliases.
        assert!(a.set_key("model.glb", h));
        assert_eq!(a.by_key("model.glb"), Some(h));
        a.remove(h);
        assert_eq!(a.by_key("model.glb"), None);
        assert_eq!(a.by_key("model.glb#Mesh0"), None);
        // Removed slots are not resurrected by `set`.
        a.set(h, Thing(4));
        assert!(a.get(h).is_none());
        // Two reservations that turn out to be the same asset.
        let alias = a.reserve_keyed("tree.glb");
        let mesh = a.reserve_keyed("tree.glb#Mesh0");
        assert!(a.forward(alias, mesh));
        assert!(!a.forward(alias, mesh), "already forwarded");
        a.insert_keyed("tree.glb#Mesh0", Thing(5));
        assert_eq!(a.get(alias).map(|t| t.0), Some(5));
        assert_eq!(a.version(alias), a.version(mesh));
        assert_eq!(a.canonical(alias), mesh);
        assert_eq!(a.canonical(mesh), mesh);
        assert_eq!(a.len(), 3);
        a.remove(mesh);
        assert!(a.get(alias).is_none());
        let reused = a.add(Thing(6));
        assert!(
            a.get(alias).is_none(),
            "{reused:?} must not leak into the old reservation"
        );
    }
}
