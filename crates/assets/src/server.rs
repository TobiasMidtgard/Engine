//! The asset server: load files by path on background threads, apply the results to
//! [`Assets<T>`] once per frame, hot reload, and the `assets.*` API.

use std::any::type_name;
use std::collections::{HashMap, HashSet};
use std::panic::AssertUnwindSafe;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::mpsc::{Receiver, Sender, channel};
use std::time::{Duration, Instant};

use bevy_ecs::prelude::*;
use engine_core::config::EngineConfig;
use engine_core::registry::short_type_name;
use engine_core::schedule::PreUpdate;
use engine_core::{App, Plugin};
use parking_lot::{Mutex, RwLock};
use serde::Serialize;
use serde_json::{Value, json};

use crate::{Asset, Assets, Handle};

/// Changes to the same file within this window are merged into one reload.
const HOT_RELOAD_DEBOUNCE: Duration = Duration::from_millis(100);
/// How often to retry watching an asset root that does not exist (yet).
const WATCH_RETRY: Duration = Duration::from_secs(2);

/// Where a file is in its life cycle.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum LoadStatus {
    /// Nobody asked for this file yet.
    #[default]
    NotLoaded,
    /// Queued or being read/decoded on a background thread (also during a reload).
    Loading,
    /// Its assets are in `Assets<T>`.
    Loaded,
    /// The last load failed; the message says why and what to do.
    Failed(String),
}

impl LoadStatus {
    /// True once the file's assets have been applied.
    pub fn is_loaded(&self) -> bool {
        matches!(self, LoadStatus::Loaded)
    }

    /// True while a load is in flight.
    pub fn is_loading(&self) -> bool {
        matches!(self, LoadStatus::Loading)
    }

    /// The failure message, if the last load failed.
    pub fn error(&self) -> Option<&str> {
        match self {
            LoadStatus::Failed(e) => Some(e),
            _ => None,
        }
    }

    /// `"not_loaded" | "loading" | "loaded" | "failed"`.
    pub fn label(&self) -> &'static str {
        match self {
            LoadStatus::NotLoaded => "not_loaded",
            LoadStatus::Loading => "loading",
            LoadStatus::Loaded => "loaded",
            LoadStatus::Failed(_) => "failed",
        }
    }
}

/// Turns a file's bytes into assets. Register with
/// [`AssetAppExt::register_asset_loader`](crate::AssetAppExt::register_asset_loader).
///
/// ```ignore
/// struct TextLoader;
/// impl AssetLoader for TextLoader {
///     fn extensions(&self) -> &[&str] { &["txt"] }
///     fn load(&self, bytes: &[u8], ctx: &mut LoadContext) -> Result<(), String> {
///         let text = String::from_utf8(bytes.to_vec()).map_err(|e| e.to_string())?;
///         ctx.add_labeled("upper", Text(text.to_uppercase())); // "notes/a.txt#upper"
///         ctx.add_primary(Text(text));                          // "notes/a.txt"
///         Ok(())
///     }
/// }
/// ```
pub trait AssetLoader: Send + Sync + 'static {
    /// Extensions this loader handles: lowercase, without the leading dot (`["png", "jpg"]`).
    /// Compound extensions work too (`["scene.json"]`); the longest match wins.
    fn extensions(&self) -> &[&str];

    /// Decode `bytes` (the whole file) and add assets through `ctx`. Runs on a background
    /// thread; return a plain sentence on failure (it becomes the file's `Failed` status).
    fn load(&self, bytes: &[u8], ctx: &mut LoadContext) -> Result<(), String>;
}

/// Inserts one asset; returns its handle index.
type ApplyFn = Box<dyn FnOnce(&mut World) -> u32 + Send>;

/// A key a load produced: (key, short type name, handle index).
type Produced = (String, &'static str, u32);

/// What a loader produced. Applied to the world on the main thread.
struct LoadOutput {
    ops: Vec<ApplyFn>,
    produced: Vec<(String, &'static str)>,
    dependencies: Vec<String>,
}

/// Passed to [`AssetLoader::load`]: adds assets under keys derived from the file path and
/// reads companion files.
///
/// Keys: the primary asset is stored under the file's path (`"models/tree.glb"`), labeled
/// sub-assets under `"models/tree.glb#Label"`. Assets of any type can be added; their
/// `Assets<T>` storage is created if needed.
pub struct LoadContext<'a> {
    path: &'a str,
    root: &'a Path,
    ops: Vec<ApplyFn>,
    produced: Vec<(String, &'static str)>,
    is_alias: Vec<bool>,
    dependencies: Vec<String>,
}

impl<'a> LoadContext<'a> {
    fn new(path: &'a str, root: &'a Path) -> Self {
        Self {
            path,
            root,
            ops: Vec::new(),
            produced: Vec::new(),
            is_alias: Vec::new(),
            dependencies: Vec::new(),
        }
    }

    fn into_output(self) -> LoadOutput {
        // Aliases go first: a handle someone reserved under an alias before the file loaded
        // becomes the target's slot, so it fills in like any other reserved handle.
        let (aliases, others): (Vec<_>, Vec<_>) = self
            .produced
            .into_iter()
            .zip(self.ops)
            .zip(self.is_alias)
            .partition(|(_, alias)| *alias);
        let (produced, ops) = aliases
            .into_iter()
            .chain(others)
            .map(|(pair, _)| pair)
            .unzip();
        LoadOutput {
            ops,
            produced,
            dependencies: self.dependencies,
        }
    }

    /// The file's path relative to the asset root (also its primary key).
    pub fn path(&self) -> &str {
        self.path
    }

    /// Folder of the file relative to the asset root (`"models"`; `""` at the root).
    pub fn directory(&self) -> &str {
        self.path.rsplit_once('/').map(|(d, _)| d).unwrap_or("")
    }

    /// Lowercase extension of the file (`"glb"`).
    pub fn extension(&self) -> String {
        file_extension(self.path)
    }

    /// Key for a sub-asset label (`Some("Mesh0")` → `"models/tree.glb#Mesh0"`) or the primary
    /// key (`None` → `"models/tree.glb"`).
    pub fn key(&self, label: Option<&str>) -> String {
        match label {
            Some(l) => format!("{}#{l}", self.path),
            None => self.path.to_string(),
        }
    }

    /// Add (or replace, on reload) the file's primary asset. Returns its key.
    pub fn add_primary<T: Asset>(&mut self, value: T) -> String {
        self.add_with(None, move |_| value)
    }

    /// Add (or replace) a labeled sub-asset. Returns its key (`"path#label"`).
    pub fn add_labeled<T: Asset>(&mut self, label: &str, value: T) -> String {
        self.add_with(Some(label), move |_| value)
    }

    /// Add an asset whose value is built on the main thread when the load is applied, with
    /// access to handles of other assets by key (for example a material that points at
    /// textures of the same file). Handles of keys that are not loaded yet are reserved and
    /// fill in when their asset arrives, so order does not matter.
    pub fn add_with<T: Asset>(
        &mut self,
        label: Option<&str>,
        make: impl FnOnce(&mut AssetResolver) -> T + Send + 'static,
    ) -> String {
        let key = self.key(label);
        self.produced
            .push((key.clone(), short_type_name_static::<T>()));
        let k = key.clone();
        self.is_alias.push(false);
        self.ops.push(Box::new(move |world: &mut World| {
            let value = make(&mut AssetResolver { world });
            world
                .get_resource_or_insert_with(Assets::<T>::default)
                .insert_keyed(&k, value)
                .index()
        }));
        key
    }

    /// Make `alias` (a label, or `None` for the primary key) another key for the asset at
    /// `target` (a label of this file). Useful when a file has one obvious asset, e.g. a model
    /// with a single mesh can be referenced by its path alone.
    pub fn alias<T: Asset>(&mut self, alias: Option<&str>, target: &str) {
        let alias_key = self.key(alias);
        let target_key = self.key(Some(target));
        self.produced
            .push((alias_key.clone(), short_type_name_static::<T>()));
        self.is_alias.push(true);
        self.ops.push(Box::new(move |world: &mut World| {
            let mut assets = world.get_resource_or_insert_with(Assets::<T>::default);
            let alias = assets.by_key(&alias_key).filter(|h| assets.is_live(*h));
            let handle = match (alias, assets.by_key(&target_key)) {
                // The alias was requested before the file loaded: the target fills that slot.
                (Some(a), None) => {
                    assets.set_key(&target_key, a);
                    a
                }
                // Both were requested before loading: the alias's reservation follows the
                // target from now on.
                (Some(a), Some(t)) if a != t && assets.get(a).is_none() => {
                    assets.forward(a, t);
                    t
                }
                (_, Some(t)) => t,
                (None, None) => assets.reserve_keyed(&target_key),
            };
            assets.set_key(&alias_key, handle);
            handle.index()
        }));
    }

    /// Key of a file given relative to this file's folder (`"tree.bin"` next to
    /// `"models/tree.gltf"` → `"models/tree.bin"`).
    pub fn resolve(&self, relative: &str) -> String {
        let dir = self.directory();
        if dir.is_empty() {
            normalize_path(relative)
        } else {
            normalize_path(&format!("{dir}/{relative}"))
        }
    }

    /// Read a companion file relative to this file's folder. The file becomes a dependency:
    /// when it changes (or appears, if it was missing), this file is reloaded too.
    pub fn read_file(&mut self, relative: &str) -> Result<Vec<u8>, String> {
        let key = self.resolve(relative);
        let full = self.root.join(&key);
        // Record the dependency before reading so a load that failed because the file was
        // missing is retried when the file shows up.
        if !self.dependencies.contains(&key) {
            self.dependencies.push(key);
        }
        std::fs::read(&full).map_err(|e| {
            format!(
                "could not read '{}' (needed by {}): {e}",
                full.display(),
                self.path
            )
        })
    }

    /// Keys added so far, with their asset type's short name.
    pub fn produced(&self) -> &[(String, &'static str)] {
        &self.produced
    }
}

fn short_type_name_static<T>() -> &'static str {
    // Type names are short-lived strings in the registry; keep one leaked copy per type.
    static NAMES: std::sync::OnceLock<Mutex<HashMap<&'static str, &'static str>>> =
        std::sync::OnceLock::new();
    let full = type_name::<T>();
    let map = NAMES.get_or_init(|| Mutex::new(HashMap::new()));
    let mut map = map.lock();
    map.entry(full)
        .or_insert_with(|| Box::leak(short_type_name(full).into_boxed_str()))
}

/// Given to [`LoadContext::add_with`] builders on the main thread.
pub struct AssetResolver<'w> {
    world: &'w mut World,
}

impl AssetResolver<'_> {
    /// Handle of the asset under `key`, reserved now if it is not loaded yet. A key of another
    /// file (`"textures/bark.png"` from a material file) starts loading that file.
    pub fn handle<T: Asset>(&mut self, key: &str) -> Handle<T> {
        let key = normalize_key(key);
        if let Some(h) = self
            .world
            .get_resource::<Assets<T>>()
            .and_then(|a| a.by_key(&key))
        {
            return h;
        }
        self.world.get_resource_or_insert_with(Assets::<T>::default);
        self.world
            .resource_scope(|world, mut assets: Mut<Assets<T>>| {
                match world.get_resource::<AssetServer>() {
                    Some(server) => server.load(&mut assets, &key),
                    None => assets.reserve_keyed(&key),
                }
            })
    }

    /// The world, for anything else a builder needs.
    pub fn world(&mut self) -> &mut World {
        self.world
    }
}

/// Status and contents of one file, as reported by `assets.status` / `assets.list`.
#[derive(Debug, Clone, Serialize)]
pub struct AssetInfo {
    /// Path relative to the asset root.
    pub path: String,
    pub status: LoadStatus,
    /// Keys the last successful load produced, with their asset type and handle.
    pub assets: Vec<AssetKeyInfo>,
    /// Other files this one read (a change to any of them reloads it).
    pub dependencies: Vec<String>,
}

/// One key produced by a load.
#[derive(Debug, Clone, Serialize)]
pub struct AssetKeyInfo {
    pub key: String,
    /// Short type name (`"Mesh"`, `"Image"`, `"StandardMaterial"`).
    #[serde(rename = "type")]
    pub kind: String,
    /// Handle index (what a `Handle<T>` field accepts in JSON, e.g. a material's
    /// `base_color_texture`).
    pub handle: u32,
}

#[derive(Default)]
struct Entry {
    status: LoadStatus,
    generation: u64,
    produced: Vec<Produced>,
    dependencies: Vec<String>,
}

struct LoadResult {
    path: String,
    generation: u64,
    outcome: Result<LoadOutput, LoadError>,
}

/// Why a load failed, plus the companion files it tried to read.
struct LoadError {
    message: String,
    dependencies: Vec<String>,
}

struct Watch {
    _watcher: notify::RecommendedWatcher,
    events: Receiver<notify::Result<notify::Event>>,
    /// Canonical root, to turn event paths back into keys.
    root: PathBuf,
}

struct State {
    root: PathBuf,
    hot_reload: bool,
    entries: HashMap<String, Entry>,
    /// Dependency file → files that read it.
    dependents: HashMap<String, HashSet<String>>,
    /// Keys handed out by `load::<T>` per file, to warn about keys a file does not provide.
    requested: HashMap<String, HashSet<(String, &'static str)>>,
    /// Changed files waiting for the debounce window.
    changed: HashMap<String, Instant>,
    watch: Option<Watch>,
    last_watch_attempt: Option<Instant>,
}

/// Loads assets from files under the asset root. A resource installed by [`AssetPlugin`].
///
/// All methods take `&self`, so systems can request loads with `Res<AssetServer>`.
#[derive(Resource)]
pub struct AssetServer {
    state: Mutex<State>,
    loaders: RwLock<Vec<Arc<dyn AssetLoader>>>,
    sender: Sender<LoadResult>,
    results: Mutex<Receiver<LoadResult>>,
}

impl AssetServer {
    /// A server reading from `root` (absolute, or relative to the current directory).
    pub fn new(root: impl Into<PathBuf>, hot_reload: bool) -> Self {
        let (sender, receiver) = channel();
        Self {
            state: Mutex::new(State {
                root: absolute(root.into()),
                hot_reload,
                entries: HashMap::new(),
                dependents: HashMap::new(),
                requested: HashMap::new(),
                changed: HashMap::new(),
                watch: None,
                last_watch_attempt: None,
            }),
            loaders: RwLock::new(Vec::new()),
            sender,
            results: Mutex::new(receiver),
        }
    }

    /// Root from (in order of priority) `root_override`, the `ENGINE_ASSET_ROOT` environment
    /// variable, or `EngineConfig::asset_root`.
    pub(crate) fn from_config(
        world: &World,
        root_override: Option<&Path>,
        hot_reload: bool,
    ) -> Self {
        let root = match root_override {
            Some(r) => r.to_path_buf(),
            None => match std::env::var("ENGINE_ASSET_ROOT") {
                Ok(r) if !r.trim().is_empty() => PathBuf::from(r),
                _ => PathBuf::from(
                    world
                        .get_resource::<EngineConfig>()
                        .map(|c| c.asset_root.clone())
                        .unwrap_or_else(|| "assets".into()),
                ),
            },
        };
        Self::new(root, hot_reload)
    }

    /// Absolute asset root.
    pub fn root(&self) -> PathBuf {
        self.state.lock().root.clone()
    }

    /// Change the asset root (already loaded assets stay; the watcher restarts).
    pub fn set_root(&self, root: impl Into<PathBuf>) {
        let mut st = self.state.lock();
        st.root = absolute(root.into());
        st.watch = None;
        st.last_watch_attempt = None;
    }

    /// Whether changed files are reloaded automatically.
    pub fn hot_reload(&self) -> bool {
        self.state.lock().hot_reload
    }

    /// Turn hot reload on or off.
    pub fn set_hot_reload(&self, on: bool) {
        let mut st = self.state.lock();
        st.hot_reload = on;
        if !on {
            st.watch = None;
        }
        st.last_watch_attempt = None;
    }

    /// True while a file watcher is running on the asset root.
    pub fn is_watching(&self) -> bool {
        self.state.lock().watch.is_some()
    }

    /// Register a loader. Later registrations win for the same extension.
    pub fn register_loader(&self, loader: impl AssetLoader) {
        self.loaders.write().push(Arc::new(loader));
    }

    /// Every extension some loader handles.
    pub fn loader_extensions(&self) -> Vec<String> {
        let mut out: Vec<String> = self
            .loaders
            .read()
            .iter()
            .flat_map(|l| l.extensions().iter().map(|e| e.to_string()))
            .collect();
        out.sort();
        out.dedup();
        out
    }

    fn find_loader(&self, path: &str) -> Result<Arc<dyn AssetLoader>, String> {
        let lower = path.to_ascii_lowercase();
        let found = {
            let loaders = self.loaders.read();
            let mut best: Option<(usize, Arc<dyn AssetLoader>)> = None;
            // Later registrations win ties: walk newest first, keep strictly longer matches.
            for loader in loaders.iter().rev() {
                for ext in loader.extensions() {
                    let ext = ext.trim_start_matches('.').to_ascii_lowercase();
                    if lower.ends_with(&format!(".{ext}"))
                        && best.as_ref().is_none_or(|(len, _)| ext.len() > *len)
                    {
                        best = Some((ext.len(), loader.clone()));
                    }
                }
            }
            best.map(|(_, l)| l)
        };
        found.ok_or_else(|| {
            format!(
                "no loader for '{path}'. Supported extensions: {}",
                self.loader_extensions().join(", ")
            )
        })
    }

    /// Queue loading `path` (relative to the asset root; a `#label` suffix is ignored).
    /// Does nothing if the file was already requested; use [`AssetServer::reload`] to force.
    pub fn load_path(&self, path: &str) {
        let path = normalize_path(asset_file(path));
        let mut st = self.state.lock();
        if st.entries.contains_key(&path) {
            return;
        }
        self.start_load(&mut st, path);
    }

    /// Queue loading the file behind `key` and return a handle for `key` right away. The
    /// handle is stable: the asset fills in when the load finishes (and is replaced in place
    /// on reload).
    ///
    /// ```ignore
    /// let tex: Handle<Image> = server.load(&mut images, "textures/bark.png");
    /// let mesh: Handle<Mesh> = server.load(&mut meshes, "models/tree.glb#Mesh0/Primitive0");
    /// ```
    pub fn load<T: Asset>(&self, assets: &mut Assets<T>, key: &str) -> Handle<T> {
        let key = normalize_key(key);
        let file = asset_file(&key).to_string();
        let handle = assets.reserve_keyed(&key);
        let kind = short_type_name_static::<T>();
        {
            let mut st = self.state.lock();
            st.requested
                .entry(file.clone())
                .or_default()
                .insert((key.clone(), kind));
            match st.entries.get(&file) {
                // Already loaded: a key it did not produce will never fill in.
                Some(entry) if entry.status.is_loaded() => {
                    if !entry
                        .produced
                        .iter()
                        .any(|(k, t, _)| *k == key && *t == kind)
                    {
                        tracing::warn!("{}", missing_key_message(&key, kind, &file, entry));
                    }
                }
                Some(_) => {}
                None => self.start_load(&mut st, file),
            }
        }
        handle
    }

    /// Load `path` again (even if it is loaded or failed), plus every file that depends on it.
    pub fn reload(&self, path: &str) {
        let path = normalize_path(asset_file(path));
        let mut st = self.state.lock();
        let mut targets: HashSet<String> = st.dependents.get(&path).cloned().unwrap_or_default();
        targets.insert(path);
        for t in targets {
            self.start_load(&mut st, t);
        }
    }

    /// Status of the file behind `path` (a `#label` suffix is ignored).
    pub fn status(&self, path: &str) -> LoadStatus {
        let path = normalize_path(asset_file(path));
        self.state
            .lock()
            .entries
            .get(&path)
            .map(|e| e.status.clone())
            .unwrap_or_default()
    }

    /// True once the file behind `path` has been loaded.
    pub fn is_loaded(&self, path: &str) -> bool {
        self.status(path).is_loaded()
    }

    /// Number of loads in flight.
    pub fn pending(&self) -> usize {
        self.state
            .lock()
            .entries
            .values()
            .filter(|e| e.status.is_loading())
            .count()
    }

    /// Handle of the asset under `key` once its value is present (`None` while loading).
    pub fn handle<T: Asset>(world: &World, key: &str) -> Option<Handle<T>> {
        world
            .get_resource::<Assets<T>>()?
            .loaded_by_key(&normalize_key(key))
    }

    /// Status and produced keys of one file.
    pub fn info(&self, path: &str) -> AssetInfo {
        let path = normalize_path(asset_file(path));
        let st = self.state.lock();
        match st.entries.get(&path) {
            Some(e) => entry_info(&path, e),
            None => AssetInfo {
                path,
                status: LoadStatus::NotLoaded,
                assets: Vec::new(),
                dependencies: Vec::new(),
            },
        }
    }

    /// Every file that was requested, sorted by path.
    pub fn list(&self) -> Vec<AssetInfo> {
        let st = self.state.lock();
        let mut out: Vec<AssetInfo> = st.entries.iter().map(|(p, e)| entry_info(p, e)).collect();
        out.sort_by(|a, b| a.path.cmp(&b.path));
        out
    }

    fn start_load(&self, st: &mut State, path: String) {
        let generation = {
            let entry = st.entries.entry(path.clone()).or_default();
            entry.generation += 1;
            entry.generation
        };
        let loader = match self.find_loader(&path) {
            Ok(l) => l,
            Err(e) => {
                tracing::warn!("asset load failed: {e}");
                if let Some(entry) = st.entries.get_mut(&path) {
                    entry.status = LoadStatus::Failed(e);
                }
                return;
            }
        };
        if let Some(entry) = st.entries.get_mut(&path) {
            entry.status = LoadStatus::Loading;
        }
        let root = st.root.clone();
        let sender = self.sender.clone();
        spawn_io(move || {
            let outcome = run_loader(&*loader, &root, &path);
            let _ = sender.send(LoadResult {
                path,
                generation,
                outcome,
            });
        });
    }

    /// Begin a synchronous load: bump the generation and pick the loader.
    fn begin_now(&self, path: &str) -> Result<(u64, Arc<dyn AssetLoader>, PathBuf), String> {
        let mut st = self.state.lock();
        let entry = st.entries.entry(path.to_string()).or_default();
        entry.generation += 1;
        let generation = entry.generation;
        match self.find_loader(path) {
            Ok(loader) => {
                if let Some(entry) = st.entries.get_mut(path) {
                    entry.status = LoadStatus::Loading;
                }
                Ok((generation, loader, st.root.clone()))
            }
            Err(e) => {
                if let Some(entry) = st.entries.get_mut(path) {
                    entry.status = LoadStatus::Failed(e.clone());
                }
                Err(e)
            }
        }
    }

    fn drain_results(&self) -> Vec<LoadResult> {
        let rx = self.results.lock();
        let mut out = Vec::new();
        while let Ok(r) = rx.try_recv() {
            out.push(r);
        }
        out
    }

    fn is_current(&self, path: &str, generation: u64) -> bool {
        self.state
            .lock()
            .entries
            .get(path)
            .is_some_and(|e| e.generation == generation)
    }

    /// Record a finished load: status, produced keys and the files it read (also for failed
    /// loads, so a file that failed because a companion file was missing reloads when that
    /// file appears).
    fn finish(&self, path: &str, result: Result<Vec<Produced>, String>, dependencies: Vec<String>) {
        let mut st = self.state.lock();
        let st = &mut *st;
        for deps in st.dependents.values_mut() {
            deps.remove(path);
        }
        for dep in &dependencies {
            st.dependents
                .entry(dep.clone())
                .or_default()
                .insert(path.to_string());
        }
        st.dependents.retain(|_, d| !d.is_empty());
        let Some(entry) = st.entries.get_mut(path) else {
            return;
        };
        entry.dependencies = dependencies;
        match result {
            Ok(produced) => {
                tracing::debug!("loaded {path} ({} assets)", produced.len());
                entry.status = LoadStatus::Loaded;
                entry.produced = produced;
                // Keys someone asked for that this file does not provide.
                if let Some(requested) = st.requested.get(path) {
                    for (key, kind) in requested {
                        if !entry.produced.iter().any(|(k, t, _)| k == key && t == kind) {
                            tracing::warn!("{}", missing_key_message(key, kind, path, entry));
                        }
                    }
                }
            }
            Err(e) => {
                tracing::warn!("asset load failed: {e}");
                entry.status = LoadStatus::Failed(e);
            }
        }
    }

    /// Start the watcher if needed, collect file events and start debounced reloads.
    fn poll_hot_reload(&self) {
        let mut guard = self.state.lock();
        let st = &mut *guard;
        if !st.hot_reload {
            st.changed.clear();
            return;
        }
        let now = Instant::now();
        if st.watch.is_none()
            && st
                .last_watch_attempt
                .is_none_or(|t| now.duration_since(t) >= WATCH_RETRY)
        {
            st.last_watch_attempt = Some(now);
            st.watch = start_watch(&st.root);
        }
        if let Some(watch) = &st.watch {
            while let Ok(event) = watch.events.try_recv() {
                let Ok(event) = event else { continue };
                use notify::EventKind;
                use notify::event::ModifyKind;
                let relevant = match event.kind {
                    EventKind::Create(_) => true,
                    EventKind::Modify(ModifyKind::Metadata(_)) => false,
                    EventKind::Modify(_) => true,
                    _ => false,
                };
                if !relevant {
                    continue;
                }
                for p in &event.paths {
                    if let Ok(rel) = p.strip_prefix(&watch.root) {
                        let key = normalize_path(&rel.to_string_lossy());
                        if !key.is_empty() {
                            st.changed.insert(key, now);
                        }
                    }
                }
            }
        }
        let ready: Vec<String> = st
            .changed
            .iter()
            .filter(|(_, t)| now.duration_since(**t) >= HOT_RELOAD_DEBOUNCE)
            .map(|(k, _)| k.clone())
            .collect();
        for key in ready {
            st.changed.remove(&key);
            let mut targets: HashSet<String> = st.dependents.get(&key).cloned().unwrap_or_default();
            if st.entries.contains_key(&key) {
                targets.insert(key.clone());
            }
            for t in targets {
                tracing::info!("hot reload: {t}");
                self.start_load(st, t);
            }
        }
    }
}

/// "'models/tree.glb#Mesh9' was requested as Mesh but …": for keys a file does not provide.
fn missing_key_message(key: &str, kind: &str, path: &str, entry: &Entry) -> String {
    let available: Vec<&str> = entry
        .produced
        .iter()
        .filter(|(_, t, _)| *t == kind)
        .map(|(k, _, _)| k.as_str())
        .collect();
    format!(
        "'{key}' was requested as {kind} but {path} has no {kind} under that key. Available: {}",
        if available.is_empty() {
            "none".to_string()
        } else {
            available.join(", ")
        }
    )
}

fn entry_info(path: &str, e: &Entry) -> AssetInfo {
    AssetInfo {
        path: path.to_string(),
        status: e.status.clone(),
        assets: e
            .produced
            .iter()
            .map(|(k, t, h)| AssetKeyInfo {
                key: k.clone(),
                kind: t.to_string(),
                handle: *h,
            })
            .collect(),
        dependencies: e.dependencies.clone(),
    }
}

fn start_watch(root: &Path) -> Option<Watch> {
    use notify::Watcher;
    if !root.is_dir() {
        return None;
    }
    let canonical = std::fs::canonicalize(root).unwrap_or_else(|_| root.to_path_buf());
    let (tx, rx) = channel();
    let mut watcher = match notify::recommended_watcher(tx) {
        Ok(w) => w,
        Err(e) => {
            tracing::warn!("hot reload unavailable: {e}");
            return None;
        }
    };
    if let Err(e) = watcher.watch(&canonical, notify::RecursiveMode::Recursive) {
        tracing::warn!("hot reload: cannot watch {}: {e}", canonical.display());
        return None;
    }
    tracing::info!("hot reload: watching {}", canonical.display());
    Some(Watch {
        _watcher: watcher,
        events: rx,
        root: canonical,
    })
}

fn absolute(path: PathBuf) -> PathBuf {
    if path.is_absolute() {
        path
    } else {
        std::env::current_dir()
            .map(|d| d.join(&path))
            .unwrap_or(path)
    }
}

fn spawn_io(job: impl FnOnce() + Send + 'static) {
    if let Some(pool) = engine_core::tasks::IoTaskPool::try_get() {
        pool.spawn(async move { job() }).detach();
    } else if let Err(e) = std::thread::Builder::new()
        .name("asset loader".into())
        .spawn(job)
    {
        tracing::error!("could not start an asset loading thread: {e}");
    }
}

fn run_loader(loader: &dyn AssetLoader, root: &Path, path: &str) -> Result<LoadOutput, LoadError> {
    let full = root.join(path);
    let bytes = std::fs::read(&full).map_err(|e| LoadError {
        message: format!(
            "could not read '{}': {e}. Paths are relative to the asset root {}",
            full.display(),
            root.display()
        ),
        dependencies: Vec::new(),
    })?;
    let mut ctx = LoadContext::new(path, root);
    let outcome = std::panic::catch_unwind(AssertUnwindSafe(|| loader.load(&bytes, &mut ctx)));
    let message = match outcome {
        Ok(Ok(())) => return Ok(ctx.into_output()),
        Ok(Err(e)) => format!("{path}: {e}"),
        Err(_) => format!("{path}: the loader panicked (see the log)"),
    };
    Err(LoadError {
        message,
        dependencies: ctx.dependencies,
    })
}

/// Apply one finished load to the world. Stale results (superseded by a newer load of the
/// same file) are dropped.
fn apply_result(world: &mut World, result: LoadResult) -> Result<(), String> {
    let LoadResult {
        path,
        generation,
        outcome,
    } = result;
    let current = world
        .get_resource::<AssetServer>()
        .is_some_and(|s| s.is_current(&path, generation));
    if !current {
        return Ok(());
    }
    match outcome {
        Ok(output) => {
            let produced: Vec<Produced> = output
                .produced
                .into_iter()
                .zip(output.ops)
                .map(|((key, kind), op)| {
                    let index = op(world);
                    (key, kind, index)
                })
                .collect();
            world
                .resource::<AssetServer>()
                .finish(&path, Ok(produced), output.dependencies);
            Ok(())
        }
        Err(LoadError {
            message,
            dependencies,
        }) => {
            world
                .resource::<AssetServer>()
                .finish(&path, Err(message.clone()), dependencies);
            Err(message)
        }
    }
}

/// Engine system (`PreUpdate`): apply finished loads and start hot reloads.
pub fn apply_loaded_assets(world: &mut World) {
    let Some(server) = world.get_resource::<AssetServer>() else {
        return;
    };
    server.poll_hot_reload();
    let results = server.drain_results();
    for result in results {
        let _ = apply_result(world, result);
    }
}

/// Load a file right now on the calling thread and apply its assets before returning (tools,
/// tests, API calls that need the result immediately). Supersedes a background load of the
/// same file that may be in flight.
pub fn load_now(world: &mut World, path: &str) -> Result<(), String> {
    let path = normalize_path(asset_file(path));
    let (generation, loader, root) = world
        .get_resource::<AssetServer>()
        .ok_or("no AssetServer in the world: add AssetPlugin (RenderPlugin adds it)")?
        .begin_now(&path)?;
    let outcome = run_loader(&*loader, &root, &path);
    apply_result(
        world,
        LoadResult {
            path,
            generation,
            outcome,
        },
    )
}

/// Lowercase extension of the file name (`"models.v2/Tree.GLB"` → `"glb"`; `""` if none).
fn file_extension(path: &str) -> String {
    let name = path.rsplit(['/', '\\']).next().unwrap_or(path);
    name.rsplit_once('.')
        .map(|(_, e)| e.to_ascii_lowercase())
        .unwrap_or_default()
}

/// The file part of an asset key: `"models/tree.glb#Mesh0"` → `"models/tree.glb"`.
pub fn asset_file(key: &str) -> &str {
    key.split_once('#').map(|(f, _)| f).unwrap_or(key)
}

/// Canonical form of a path relative to the asset root: forward slashes, no `.` segments,
/// `..` resolved lexically (never above the root), no leading slash.
pub fn normalize_path(path: &str) -> String {
    let mut parts: Vec<&str> = Vec::new();
    for part in path.split(['/', '\\']) {
        match part {
            "" | "." => {}
            ".." => {
                parts.pop();
            }
            p => parts.push(p),
        }
    }
    parts.join("/")
}

/// [`normalize_path`] for the file part of a key, keeping the `#label` as written.
fn normalize_key(key: &str) -> String {
    match key.split_once('#') {
        Some((file, label)) => format!("{}#{label}", normalize_path(file)),
        None => normalize_path(key),
    }
}

/// Installs the [`AssetServer`], applies finished loads each frame (`PreUpdate`) and registers
/// the `assets.*` API.
#[derive(Debug, Clone)]
pub struct AssetPlugin {
    /// Watch the asset root and reload changed files. Default: on in debug builds.
    pub hot_reload: bool,
    /// Asset root override. Default: `ENGINE_ASSET_ROOT`, else `EngineConfig::asset_root`
    /// (relative paths are relative to the current directory).
    pub root: Option<PathBuf>,
}

impl Default for AssetPlugin {
    fn default() -> Self {
        Self {
            hot_reload: cfg!(debug_assertions),
            root: None,
        }
    }
}

impl Plugin for AssetPlugin {
    fn build(&self, app: &mut App) {
        if let Some(server) = app.world.get_resource::<AssetServer>() {
            server.set_hot_reload(self.hot_reload);
            if let Some(root) = &self.root {
                server.set_root(root);
            }
        } else {
            let server =
                AssetServer::from_config(&app.world, self.root.as_deref(), self.hot_reload);
            app.insert_resource(server);
        }
        app.add_systems(PreUpdate, apply_loaded_assets);
        register_api(app);
    }
}

fn server(world: &World) -> Result<&AssetServer, String> {
    world
        .get_resource::<AssetServer>()
        .ok_or_else(|| "no AssetServer in the world: add AssetPlugin".to_string())
}

fn path_arg(args: &Value) -> Result<&str, String> {
    args.get("path")
        .and_then(|p| p.as_str())
        .or_else(|| args.as_str())
        .filter(|p| !p.is_empty())
        .ok_or_else(|| {
            "missing 'path': a file path relative to the asset root, e.g. \"models/tree.glb\""
                .into()
        })
}

fn info_json(world: &World, path: &str) -> Result<Value, String> {
    let info = server(world)?.info(path);
    let mut v = serde_json::to_value(&info).map_err(|e| e.to_string())?;
    v["status"] = json!(info.status.label());
    if let Some(e) = info.status.error() {
        v["error"] = json!(e);
    }
    Ok(v)
}

fn register_api(app: &mut App) {
    app.register_api(
        "assets.load",
        "Load a file from the asset root (models .gltf/.glb, textures .png/.jpg/.hdr, …). \
         Returns its status and the asset keys it produced (use them in MeshRenderer, e.g. \
         {\"mesh\": {\"asset\": \"models/tree.glb#Mesh0/Primitive0\"}}). With wait=true the load \
         finishes before the call returns. A file that is already loaded is not loaded again \
         (use assets.reload).",
        r#"{"path": "models/tree.glb", "wait": true}"#,
        |world, args| {
            let path = path_arg(&args)?.to_string();
            if args.get("wait").and_then(|w| w.as_bool()).unwrap_or(false) {
                if !server(world)?.is_loaded(&path) {
                    load_now(world, &path)?;
                }
            } else {
                server(world)?.load_path(&path);
            }
            info_json(world, &path)
        },
    )
    .register_api(
        "assets.status",
        "Status of a file: not_loaded / loading / loaded / failed (with error), and its asset keys.",
        r#"{"path": "models/tree.glb"}"#,
        |world, args| {
            let path = path_arg(&args)?.to_string();
            info_json(world, &path)
        },
    )
    .register_api(
        "assets.list",
        "Every requested file with its status and asset keys, plus the asset root, hot reload \
         state and supported extensions.",
        "{}",
        |world, _| {
            let server = server(world)?;
            let list: Vec<Value> = server
                .list()
                .into_iter()
                .map(|info| {
                    let mut v = serde_json::to_value(&info).unwrap_or_default();
                    v["status"] = json!(info.status.label());
                    if let Some(e) = info.status.error() {
                        v["error"] = json!(e);
                    }
                    v
                })
                .collect();
            Ok(json!({
                "root": server.root().display().to_string(),
                "hot_reload": server.hot_reload(),
                "watching": server.is_watching(),
                "extensions": server.loader_extensions(),
                "pending": server.pending(),
                "assets": list,
            }))
        },
    )
    .register_api(
        "assets.reload",
        "Reload a file (and every file that depends on it) even if it is already loaded.",
        r#"{"path": "textures/bark.png", "wait": false}"#,
        |world, args| {
            let path = path_arg(&args)?.to_string();
            if args.get("wait").and_then(|w| w.as_bool()).unwrap_or(false) {
                let dependents: Vec<String> = {
                    let st = server(world)?.state.lock();
                    st.dependents
                        .get(&normalize_path(asset_file(&path)))
                        .map(|d| d.iter().cloned().collect())
                        .unwrap_or_default()
                };
                load_now(world, &path)?;
                for d in dependents {
                    load_now(world, &d)?;
                }
            } else {
                server(world)?.reload(&path);
            }
            info_json(world, &path)
        },
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::AssetAppExt;
    use engine_core::registry::call_api;
    use std::sync::atomic::{AtomicU32, Ordering};

    #[derive(Debug, PartialEq)]
    struct Text(String);
    impl Asset for Text {}

    /// `*.txt` → primary Text plus "#upper" (also reachable as the alias "#shout"). A first
    /// line `include: other.txt` appends that file (and makes it a dependency).
    struct TextLoader;
    impl AssetLoader for TextLoader {
        fn extensions(&self) -> &[&str] {
            &["txt"]
        }
        fn load(&self, bytes: &[u8], ctx: &mut LoadContext) -> Result<(), String> {
            let mut text = std::str::from_utf8(bytes)
                .map_err(|e| format!("not UTF-8 text: {e}"))?
                .to_string();
            if let Some(rest) = text.strip_prefix("include:") {
                let other = rest.trim().to_string();
                let extra = ctx.read_file(&other)?;
                text = String::from_utf8_lossy(&extra).to_string();
            }
            if text.contains("FAIL") {
                return Err("the text asked to fail".into());
            }
            ctx.add_labeled("upper", Text(text.to_uppercase()));
            ctx.alias::<Text>(Some("shout"), "upper");
            ctx.add_primary(Text(text));
            Ok(())
        }
    }

    fn temp_root(name: &str) -> PathBuf {
        static COUNTER: AtomicU32 = AtomicU32::new(0);
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.subsec_nanos())
            .unwrap_or(0);
        let dir = std::env::temp_dir().join(format!(
            "engine_assets_{name}_{}_{}_{nanos}",
            std::process::id(),
            COUNTER.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir_all(dir.join("notes")).unwrap();
        dir
    }

    fn test_app(root: &Path, hot_reload: bool) -> App {
        let mut app = App::new();
        app.add_plugins(AssetPlugin {
            hot_reload,
            root: Some(root.to_path_buf()),
        })
        .init_asset::<Text>()
        .register_asset_loader(TextLoader);
        app
    }

    /// Run frames until `done` or a timeout.
    fn run_until(app: &mut App, mut done: impl FnMut(&mut App) -> bool) -> bool {
        let start = Instant::now();
        while start.elapsed() < Duration::from_secs(10) {
            app.update();
            if done(app) {
                return true;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        false
    }

    fn text(app: &App, key: &str) -> Option<String> {
        let assets = app.world.resource::<Assets<Text>>();
        assets.get(assets.by_key(key)?).map(|t| t.0.clone())
    }

    #[test]
    fn loads_primary_and_labeled_assets_with_stable_handles() {
        let root = temp_root("load");
        std::fs::write(root.join("notes/a.txt"), "hello").unwrap();
        let mut app = test_app(&root, false);

        // A handle requested before the load exists right away and fills in later, also
        // when it is requested through an alias.
        let (early, shout): (Handle<Text>, Handle<Text>) = {
            let world = &mut app.world;
            world.resource_scope(|world, mut texts: Mut<Assets<Text>>| {
                let server = world.resource::<AssetServer>();
                (
                    server.load(&mut texts, "./notes/a.txt#upper"),
                    server.load(&mut texts, "notes/a.txt#shout"),
                )
            })
        };
        assert_eq!(
            app.world.resource::<AssetServer>().status("notes/a.txt"),
            LoadStatus::Loading
        );
        assert!(run_until(&mut app, |a| a
            .world
            .resource::<AssetServer>()
            .is_loaded("notes/a.txt")));
        assert_eq!(text(&app, "notes/a.txt").as_deref(), Some("hello"));
        assert_eq!(text(&app, "notes/a.txt#upper").as_deref(), Some("HELLO"));
        assert_eq!(
            AssetServer::handle::<Text>(&app.world, "notes/a.txt#upper"),
            Some(early)
        );
        assert_eq!(
            app.world
                .resource::<Assets<Text>>()
                .get(shout)
                .map(|t| t.0.as_str()),
            Some("HELLO")
        );

        // load_path is idempotent: no second load starts.
        app.world.resource::<AssetServer>().load_path("notes/a.txt");
        assert!(app.world.resource::<AssetServer>().is_loaded("notes/a.txt"));

        // Reload keeps handles and bumps versions.
        let primary = app
            .world
            .resource::<Assets<Text>>()
            .by_key("notes/a.txt")
            .unwrap();
        let v0 = app.world.resource::<Assets<Text>>().version(primary);
        std::fs::write(root.join("notes/a.txt"), "changed").unwrap();
        app.world.resource::<AssetServer>().reload("notes/a.txt");
        assert!(run_until(&mut app, |a| text(a, "notes/a.txt").as_deref()
            == Some("changed")));
        let assets = app.world.resource::<Assets<Text>>();
        assert_eq!(assets.by_key("notes/a.txt"), Some(primary));
        assert!(assets.version(primary) > v0);
        assert_eq!(assets.by_key("notes/a.txt#upper"), Some(early));
        assert_eq!(text(&app, "notes/a.txt#upper").as_deref(), Some("CHANGED"));
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn failures_have_readable_messages() {
        let root = temp_root("fail");
        std::fs::write(root.join("bad.txt"), "please FAIL").unwrap();
        std::fs::write(root.join("x.unknown"), "?").unwrap();
        let mut app = test_app(&root, false);
        let server = app.world.resource::<AssetServer>();
        server.load_path("missing.txt");
        server.load_path("bad.txt");
        server.load_path("x.unknown");
        // Unknown extensions fail immediately.
        let status = server.status("x.unknown");
        assert!(status.error().unwrap().contains("no loader"), "{status:?}");
        assert!(status.error().unwrap().contains("txt"), "{status:?}");
        assert!(run_until(&mut app, |a| a
            .world
            .resource::<AssetServer>()
            .pending()
            == 0));
        let server = app.world.resource::<AssetServer>();
        let missing = server.status("missing.txt");
        assert!(
            missing.error().unwrap().contains("could not read"),
            "{missing:?}"
        );
        let bad = server.status("bad.txt");
        assert!(bad.error().unwrap().contains("asked to fail"), "{bad:?}");
        // A fixed file loads after an explicit reload.
        std::fs::write(root.join("bad.txt"), "fine").unwrap();
        server.reload("bad.txt");
        assert!(run_until(&mut app, |a| a
            .world
            .resource::<AssetServer>()
            .is_loaded("bad.txt")));
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn dependencies_reload_their_dependents() {
        let root = temp_root("deps");
        std::fs::write(root.join("notes/part.txt"), "part one").unwrap();
        std::fs::write(root.join("notes/whole.txt"), "include: part.txt").unwrap();
        let mut app = test_app(&root, false);
        load_now(&mut app.world, "notes/whole.txt").unwrap();
        assert_eq!(text(&app, "notes/whole.txt").as_deref(), Some("part one"));
        let info = app.world.resource::<AssetServer>().info("notes/whole.txt");
        assert_eq!(info.dependencies, vec!["notes/part.txt".to_string()]);

        std::fs::write(root.join("notes/part.txt"), "part two").unwrap();
        app.world.resource::<AssetServer>().reload("notes/part.txt");
        assert!(run_until(&mut app, |a| text(a, "notes/whole.txt")
            .as_deref()
            == Some("part two")));

        // A load that failed because its companion file was missing still records it, so
        // the file is retried when the companion appears (hot reload or reload()).
        std::fs::write(root.join("notes/early.txt"), "include: late.txt").unwrap();
        assert!(load_now(&mut app.world, "notes/early.txt").is_err());
        let info = app.world.resource::<AssetServer>().info("notes/early.txt");
        assert_eq!(info.dependencies, vec!["notes/late.txt".to_string()]);
        std::fs::write(root.join("notes/late.txt"), "finally").unwrap();
        app.world.resource::<AssetServer>().reload("notes/late.txt");
        assert!(run_until(&mut app, |a| text(a, "notes/early.txt")
            .as_deref()
            == Some("finally")));
        let _ = std::fs::remove_dir_all(root);
    }

    /// `*.link` → a `Link` to the `Text` whose key is the file's content (resolved on the main
    /// thread through `add_with`).
    struct Link(Handle<Text>);
    impl Asset for Link {}
    struct LinkLoader;
    impl AssetLoader for LinkLoader {
        fn extensions(&self) -> &[&str] {
            &["link"]
        }
        fn load(&self, bytes: &[u8], ctx: &mut LoadContext) -> Result<(), String> {
            let key = String::from_utf8_lossy(bytes).trim().to_string();
            ctx.add_with(None, move |resolver| Link(resolver.handle::<Text>(&key)));
            Ok(())
        }
    }

    #[test]
    fn resolved_keys_of_other_files_start_loading() {
        let root = temp_root("link");
        std::fs::write(root.join("notes/target.txt"), "linked").unwrap();
        std::fs::write(root.join("notes/a.link"), "notes/target.txt#upper").unwrap();
        let mut app = test_app(&root, false);
        app.register_asset_loader(LinkLoader);
        load_now(&mut app.world, "notes/a.link").unwrap();
        let link = {
            let links = app.world.resource::<Assets<Link>>();
            links.get(links.by_key("notes/a.link").unwrap()).unwrap().0
        };
        // The referenced file was not requested by anyone else; resolving its key loads it.
        assert!(run_until(&mut app, |a| a
            .world
            .resource::<Assets<Text>>()
            .get(link)
            .is_some()));
        assert_eq!(
            app.world.resource::<Assets<Text>>().get(link).unwrap().0,
            "LINKED"
        );
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn api_functions() {
        let root = temp_root("api");
        std::fs::write(root.join("notes/b.txt"), "api").unwrap();
        let mut app = test_app(&root, false);
        let w = &mut app.world;
        let st = call_api(w, "assets.status", json!({"path": "notes/b.txt"})).unwrap();
        assert_eq!(st["status"], "not_loaded");
        let loaded = call_api(
            w,
            "assets.load",
            json!({"path": "notes/b.txt", "wait": true}),
        )
        .unwrap();
        assert_eq!(loaded["status"], "loaded", "{loaded}");
        let keys: Vec<&str> = loaded["assets"]
            .as_array()
            .unwrap()
            .iter()
            .map(|a| a["key"].as_str().unwrap())
            .collect();
        assert!(
            keys.contains(&"notes/b.txt") && keys.contains(&"notes/b.txt#upper"),
            "{loaded}"
        );
        assert_eq!(loaded["assets"][0]["type"], "Text");
        let handle = loaded["assets"][0]["handle"].as_u64().unwrap() as u32;
        let key = loaded["assets"][0]["key"].as_str().unwrap().to_string();
        assert_eq!(
            w.resource::<Assets<Text>>().by_key(&key),
            Some(Handle::from_index(handle))
        );
        // Loading a loaded file again (even with wait) is a no-op: same value, same version.
        let version = w
            .resource::<Assets<Text>>()
            .version(Handle::from_index(handle));
        let again = call_api(
            w,
            "assets.load",
            json!({"path": "notes/b.txt", "wait": true}),
        )
        .unwrap();
        assert_eq!(again["status"], "loaded");
        assert_eq!(
            w.resource::<Assets<Text>>()
                .version(Handle::from_index(handle)),
            version
        );
        let list = call_api(w, "assets.list", json!({})).unwrap();
        assert_eq!(list["assets"][0]["path"], "notes/b.txt", "{list}");
        assert_eq!(list["extensions"], json!(["txt"]));
        let failed =
            call_api(w, "assets.load", json!({"path": "nope.txt", "wait": true})).unwrap_err();
        assert!(failed.contains("could not read"), "{failed}");
        let st = call_api(w, "assets.status", json!({"path": "nope.txt"})).unwrap();
        assert_eq!(st["status"], "failed");
        assert!(
            call_api(w, "assets.load", json!({}))
                .unwrap_err()
                .contains("missing 'path'")
        );
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn hot_reload_picks_up_file_changes() {
        let root = temp_root("hot");
        std::fs::write(root.join("notes/live.txt"), "v1").unwrap();
        let mut app = test_app(&root, true);
        app.world
            .resource::<AssetServer>()
            .load_path("notes/live.txt");
        assert!(run_until(&mut app, |a| text(a, "notes/live.txt")
            .as_deref()
            == Some("v1")));
        if !app.world.resource::<AssetServer>().is_watching() {
            eprintln!("skipping: file watching is not available here");
            return;
        }
        // Give the watcher a moment, then change the file on disk.
        std::thread::sleep(Duration::from_millis(50));
        std::fs::write(root.join("notes/live.txt"), "v2").unwrap();
        assert!(
            run_until(&mut app, |a| text(a, "notes/live.txt").as_deref()
                == Some("v2")),
            "the change was not picked up"
        );
        // A file that failed because its companion was missing loads once the companion is
        // written.
        std::fs::write(root.join("notes/needs.txt"), "include: later.txt").unwrap();
        app.world
            .resource::<AssetServer>()
            .load_path("notes/needs.txt");
        assert!(run_until(&mut app, |a| a
            .world
            .resource::<AssetServer>()
            .status("notes/needs.txt")
            .error()
            .is_some()));
        std::fs::write(root.join("notes/later.txt"), "arrived").unwrap();
        assert!(
            run_until(&mut app, |a| text(a, "notes/needs.txt").as_deref()
                == Some("arrived")),
            "the new companion file did not trigger a reload"
        );
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn paths_normalize() {
        assert_eq!(
            normalize_path("./models\\trees/../tree.glb"),
            "models/tree.glb"
        );
        assert_eq!(normalize_path("/a//b/./c"), "a/b/c");
        assert_eq!(normalize_path("../../x"), "x");
        assert_eq!(
            normalize_key("./m/t.glb#Mesh0/Primitive1"),
            "m/t.glb#Mesh0/Primitive1"
        );
        assert_eq!(asset_file("m/t.glb#Mesh0"), "m/t.glb");
        assert_eq!(file_extension("models.v2/Tree.GLB"), "glb");
        assert_eq!(file_extension("models.v2/README"), "");
    }

    #[test]
    fn root_comes_from_config() {
        let mut app = App::new();
        app.world.resource_mut::<EngineConfig>().asset_root = "my_assets".into();
        app.add_plugins(AssetPlugin {
            hot_reload: false,
            root: None,
        });
        let root = app.world.resource::<AssetServer>().root();
        match std::env::var("ENGINE_ASSET_ROOT") {
            Ok(r) if !r.trim().is_empty() => assert!(root.ends_with(r.trim_start_matches("./"))),
            _ => {
                assert!(root.is_absolute());
                assert!(root.ends_with("my_assets"), "{}", root.display());
            }
        }
    }
}
