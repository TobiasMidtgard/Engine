//! # engine_assets
//!
//! [`Handle<T>`] is a cheap, copyable reference to an asset stored in [`Assets<T>`] (a
//! resource). Assets can be created in code (`assets.add(mesh)`), registered under a key
//! so identical requests share one copy (`assets.get_or_insert_keyed("cube:1", || …)`), or
//! loaded from disk by the asset server.
//!
//! Change tracking: every slot has a version. GPU-side caches compare versions to know when
//! to re-upload, so editing an asset with [`Assets::get_mut`] is all a game needs to do.

use std::collections::HashMap;
use std::fmt;
use std::hash::{Hash, Hasher};
use std::marker::PhantomData;

use bevy_ecs::prelude::*;
use serde::{Deserialize, Deserializer, Serialize, Serializer};

/// Marker for asset types.
pub trait Asset: Send + Sync + 'static {}

/// Reference to an asset in [`Assets<T>`]. Copy, compare and hash freely.
pub struct Handle<T> {
    index: u32,
    _marker: PhantomData<fn() -> T>,
}

impl<T> Handle<T> {
    pub const fn from_index(index: u32) -> Self {
        Self {
            index,
            _marker: PhantomData,
        }
    }

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
}

/// Storage for every asset of type `T`.
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

    /// Store a new asset.
    pub fn add(&mut self, value: T) -> Handle<T> {
        let version = self.bump();
        if let Some(index) = self.free.pop() {
            self.slots[index as usize] = Slot {
                value: Some(value),
                version,
            };
            Handle::from_index(index)
        } else {
            self.slots.push(Slot {
                value: Some(value),
                version,
            });
            Handle::from_index(self.slots.len() as u32 - 1)
        }
    }

    /// Return the asset registered under `key`, creating it with `make` the first time.
    pub fn get_or_insert_keyed(&mut self, key: &str, make: impl FnOnce() -> T) -> Handle<T> {
        if let Some(h) = self.keys.get(key)
            && self.get(*h).is_some()
        {
            return *h;
        }
        let h = self.add(make());
        self.keys.insert(key.to_string(), h);
        h
    }

    /// Insert or replace the asset under `key`.
    pub fn insert_keyed(&mut self, key: &str, value: T) -> Handle<T> {
        if let Some(h) = self.keys.get(key).copied()
            && self.get(h).is_some()
        {
            self.set(h, value);
            return h;
        }
        let h = self.add(value);
        self.keys.insert(key.to_string(), h);
        h
    }

    pub fn by_key(&self, key: &str) -> Option<Handle<T>> {
        self.keys.get(key).copied()
    }

    pub fn get(&self, h: Handle<T>) -> Option<&T> {
        self.slots.get(h.index as usize)?.value.as_ref()
    }

    /// Mutable access; marks the asset changed.
    pub fn get_mut(&mut self, h: Handle<T>) -> Option<&mut T> {
        let version = self.bump();
        let slot = self.slots.get_mut(h.index as usize)?;
        slot.version = version;
        slot.value.as_mut()
    }

    /// Replace an existing asset's value (marks it changed).
    pub fn set(&mut self, h: Handle<T>, value: T) {
        let version = self.bump();
        if let Some(slot) = self.slots.get_mut(h.index as usize) {
            slot.value = Some(value);
            slot.version = version;
        }
    }

    pub fn remove(&mut self, h: Handle<T>) -> Option<T> {
        let slot = self.slots.get_mut(h.index as usize)?;
        let v = slot.value.take();
        if v.is_some() {
            self.free.push(h.index);
            self.keys.retain(|_, k| *k != h);
        }
        v
    }

    /// Version of the asset's current value (changes on every edit). 0 if missing.
    pub fn version(&self, h: Handle<T>) -> u64 {
        self.slots
            .get(h.index as usize)
            .filter(|s| s.value.is_some())
            .map(|s| s.version)
            .unwrap_or(0)
    }

    /// Highest version of any asset: changes whenever anything was added or edited.
    pub fn latest_version(&self) -> u64 {
        self.next_version
    }

    pub fn len(&self) -> usize {
        self.slots.iter().filter(|s| s.value.is_some()).count()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Capacity of the index space (largest handle index + 1).
    pub fn index_capacity(&self) -> usize {
        self.slots.len()
    }

    pub fn iter(&self) -> impl Iterator<Item = (Handle<T>, &T)> {
        self.slots
            .iter()
            .enumerate()
            .filter_map(|(i, s)| s.value.as_ref().map(|v| (Handle::from_index(i as u32), v)))
    }
}

/// `app.init_asset::<T>()` installs `Assets<T>`.
pub trait AssetAppExt {
    fn init_asset<T: Asset>(&mut self) -> &mut Self;
}

impl AssetAppExt for engine_core::App {
    fn init_asset<T: Asset>(&mut self) -> &mut Self {
        if !self.world.contains_resource::<Assets<T>>() {
            self.world.insert_resource(Assets::<T>::default());
        }
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
}
