//! Streamed virtual texturing: textures far larger than the GPU could hold whole (16k scans of
//! bark, rock and ground) drawn through a fixed-size cache of tiles, with only the tiles the
//! screen needs, at the resolution it needs, resident.
//!
//! ## Using it
//!
//! Nothing changes on the material side: any [`StandardMaterial`](crate::StandardMaterial)
//! texture slot holding a `Handle<Image>` can be virtual. A handle becomes virtual in one of
//! three ways:
//!
//! - **Big images**: an RGBA8 image wider or taller than
//!   `RenderSettings::virtual_texture_threshold` (4096) is tiled into the cache the first time a
//!   material draws it, instead of being uploaded whole. The image is copied once to the tile
//!   loaders (they cut its tiles and make its missing mip levels).
//! - **Baked files**: [`VirtualTextures::load`] streams a PNG or JPEG from disk. The first time,
//!   it bakes a tiled copy next to the file (`<file>.vtex`, see [`baked`]) on a background
//!   thread; afterwards tiles are read and decoded straight from that copy, so the image never
//!   sits whole in memory or on the GPU. The handle shows a small preview until the bake is done.
//! - **Procedural tiles**: [`VirtualTextures::add_procedural`] takes a [`TileSource`] that paints
//!   tiles on demand (bark and leaf pages painted at any resolution).
//!
//! ```no_run
//! use bevy_ecs::prelude::*;
//! use engine_assets::{AssetServer, Assets};
//! use engine_render::prelude::*;
//!
//! fn setup(
//!     mut vt: ResMut<VirtualTextures>,
//!     server: Res<AssetServer>,
//!     mut images: ResMut<Assets<Image>>,
//! ) -> StandardMaterial {
//!     let rock = vt.load(&server, &mut images, "fetched/render/rock_face/rock_face_diff_16k.jpg", true);
//!     let rock_n = vt.load(&server, &mut images, "fetched/render/rock_face/rock_face_nor_gl_16k.jpg", false);
//!     StandardMaterial::color(1.0, 1.0, 1.0)
//!         .with_base_color_texture(rock)
//!         .with_normal_texture(rock_n)
//! }
//! ```
//!
//! ## How it works
//!
//! Each virtual texture is split into tiles of 128² texels at every mip level down to the level
//! where the whole image fits one tile (the root). Tiles live in a physical cache: one texture
//! for sRGB color and one for linear data, each `RenderSettings::virtual_texture_cache_tiles`
//! slots of 144² texels (the tile plus an 8-texel border copied from its neighbors, so bilinear,
//! trilinear and up to 8× anisotropic filtering never see a seam). Each slot has its own mip
//! levels (coarser copies of the tile), so the hardware filters trilinearly between a tile and
//! its next coarser level with one lookup, and the root tile's slot holds the mip tail, beyond
//! which the texture fades to its mean color. With
//! `RenderSettings::virtual_texture_compression` and a device that supports it, the cache is
//! BC7-compressed (a quarter of the memory); otherwise RGBA8.
//!
//! A page table per texture (in one storage buffer shared by all of them) maps every tile of
//! every level to the slot of the finest resident tile covering it. The shader picks the level
//! from the texture coordinate's screen derivatives, reads the entry and samples the cache. A
//! tile that isn't resident yet falls back to its nearest resident ancestor, and the root is
//! always resident, so nothing ever shows missing; detail sharpens in as tiles arrive.
//!
//! **Feedback**: the depth prepass (and the scene pass for blended materials) records which
//! tile each sampled virtual texture wanted, one pixel in 16 per frame in a rotating pattern,
//! into a small hash table on the GPU. It is read back a few frames later; the renderer touches
//! the tiles that are resident (least recently used tiles are evicted first, only leaves, never
//! a root) and loads the missing ones on worker threads, coarse levels first, committing at most
//! `RenderSettings::virtual_texture_upload_budget` tiles per frame. Devices that can't write
//! storage buffers from fragment shaders keep the coarsest levels that fit the cache instead.
//!
//! **Debugging**: `DebugView::VirtualTextures` colors each surface by the level its first
//! virtual texture shows (blue full detail through cyan, green, yellow, orange, red, magenta,
//! white), darkens tile edges, and turns surfaces that show a coarser fallback than they asked
//! for reddish. [`VirtualTextureStats`] (also in `render.stats` under the mesh feature) has the
//! resident tiles, cache memory, tiles loaded this frame and the feedback misses.

pub mod baked;
pub(crate) mod bc7;
pub(crate) mod provider;
pub mod residency;
pub(crate) mod system;

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::thread::JoinHandle;

use bevy_ecs::prelude::*;
use engine_assets::{AssetServer, Assets, Handle};
use serde::Serialize;

pub use baked::{BakeReport, TileCodec, bake, baked_path};
pub use residency::{BORDER as VT_TILE_BORDER, SLOT as VT_TILE_SLOT, TILE as VT_TILE_SIZE};

use crate::texture::{Image, PixelFormat};
use provider::{ProceduralProvider, TileProvider, root_image};

/// Paints the tiles of a procedural virtual texture on demand. Register one with
/// [`VirtualTextures::add_procedural`]; the returned `Handle<Image>` works in any
/// `StandardMaterial` texture slot.
///
/// Tiles are painted on the renderer's loader threads, several at once and in any order, so
/// painting must be thread-safe and deterministic (the same tile always paints the same
/// texels: a tile may be evicted and painted again later). Neighboring tiles must agree on the
/// texels they share: paint each texel from its coordinates, as [`paint_tile_with`] does, and
/// the borders line up by construction.
pub trait TileSource: Send + Sync + 'static {
    /// Texels of level 0, `(width, height)`. Every level must halve evenly down to the level
    /// that fits one 128² tile (width and height divisible by that power of two: any power of
    /// two from 128 to 131072 works), or registering fails.
    fn size(&self) -> (u32, u32);

    /// The texels are sRGB-encoded color (base color, emissive); `false` for data (normal
    /// maps, roughness, height).
    fn srgb(&self) -> bool;

    /// Pixel format of the painted texels. Only [`PixelFormat::Rgba8`] is supported.
    fn format(&self) -> PixelFormat {
        PixelFormat::Rgba8
    }

    /// Paint tile (`x`, `y`) of level `mip` with its border into `out`: [`VT_TILE_SLOT`]²
    /// RGBA8 texels (144 × 144 × 4 bytes), rows top to bottom. Texel (i, j) of `out` is texel
    /// (x·128 − 8 + i, y·128 − 8 + j) of level `mip`, wrapped around the level's edges (the
    /// level is `size() >> mip` texels). Level `mip` should look like level 0 averaged down
    /// 2^mip times (filter or band-limit the pattern to the level's texel size), or distant
    /// surfaces shimmer.
    fn paint_tile(&self, mip: u32, x: u32, y: u32, out: &mut [u8]);
}

/// Fill a tile for [`TileSource::paint_tile`] by calling `texel(tx, ty, level_width,
/// level_height)` for every texel, with the texel's wrapped coordinates in level `mip` of a
/// texture of `size` texels at level 0.
pub fn paint_tile_with(
    size: (u32, u32),
    mip: u32,
    x: u32,
    y: u32,
    out: &mut [u8],
    mut texel: impl FnMut(u32, u32, u32, u32) -> [u8; 4],
) {
    let (w, h) = ((size.0 >> mip).max(1), (size.1 >> mip).max(1));
    let x0 = (x * VT_TILE_SIZE) as i64 - VT_TILE_BORDER as i64;
    let y0 = (y * VT_TILE_SIZE) as i64 - VT_TILE_BORDER as i64;
    for (j, row) in out
        .as_chunks_mut::<{ (VT_TILE_SLOT * 4) as usize }>()
        .0
        .iter_mut()
        .take(VT_TILE_SLOT as usize)
        .enumerate()
    {
        let ty = (y0 + j as i64).rem_euclid(h as i64) as u32;
        for (i, px) in row.as_chunks_mut::<4>().0.iter_mut().enumerate() {
            let tx = (x0 + i as i64).rem_euclid(w as i64) as u32;
            *px = texel(tx, ty, w, h);
        }
    }
}

/// Where a virtual texture handle is.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub enum VirtualTextureStatus {
    /// Not registered here (an ordinary image; it may still go virtual by size).
    NotVirtual,
    /// Opening or baking its tile file; the image is a stand-in until then.
    Baking,
    /// Streaming.
    Ready,
    /// Could not be opened or baked (the reason). The image stays a plain stand-in.
    Failed(String),
}

type Opening = JoinHandle<Result<(Arc<baked::BakedProvider>, Option<BakeReport>), String>>;

enum State {
    Opening(Opening),
    Ready(Arc<dyn TileProvider>),
    Failed(String),
}

struct Registered {
    state: State,
    /// What it was made from (a path, or "procedural").
    name: String,
    /// Changes when the entry is replaced, so the renderer drops tiles of the old one.
    generation: u64,
    bake: Option<BakeReport>,
}

/// Virtual textures from files and procedural sources, by image handle. A resource added by
/// `RenderPlugin`. See the module docs.
#[derive(Resource, Default)]
pub struct VirtualTextures {
    entries: HashMap<u32, Registered>,
    next_generation: u64,
}

impl VirtualTextures {
    fn insert(&mut self, handle: Handle<Image>, name: String, state: State) {
        self.next_generation += 1;
        self.entries.insert(
            handle.index(),
            Registered {
                state,
                name,
                generation: self.next_generation,
                bake: None,
            },
        );
    }

    /// Stream the image file at asset key `key` (a PNG or JPEG under the asset root, such as
    /// `fetched/render/rock/rock_diff_16k.jpg`) as a virtual texture. `srgb`: the file holds
    /// color (base color, emissive) rather than data (normal, roughness, height maps).
    ///
    /// Opens the baked tile copy next to the file, baking it first on a background thread when
    /// it is missing or stale (once per file; a 16k texture takes a minute or so). Until then the
    /// handle shows a mid-gray stand-in, then a 128² preview. A missing file logs a warning
    /// and leaves the stand-in ([`VirtualTextures::status`] says `Failed`).
    pub fn load(
        &mut self,
        server: &AssetServer,
        images: &mut Assets<Image>,
        key: &str,
        srgb: bool,
    ) -> Handle<Image> {
        let path = server.root().join(engine_assets::normalize_path(key));
        self.load_file(images, path, srgb)
    }

    /// [`VirtualTextures::load`] for a file path (absolute, or relative to the working
    /// directory).
    pub fn load_file(
        &mut self,
        images: &mut Assets<Image>,
        path: impl Into<PathBuf>,
        srgb: bool,
    ) -> Handle<Image> {
        let path: PathBuf = path.into();
        let mut stand_in = Image::solid([128, 128, 128, 255], srgb);
        stand_in.keep_pixels = true;
        let handle = images.add(stand_in);
        let name = path.display().to_string();
        let task = std::thread::Builder::new()
            .name("virtual texture bake".into())
            .spawn(move || {
                baked::open_or_bake(&path, srgb).map(|(p, report)| (Arc::new(p), report))
            });
        let state = match task {
            Ok(task) => State::Opening(task),
            Err(e) => State::Failed(e.to_string()),
        };
        self.insert(handle, name, state);
        handle
    }

    /// Register a procedural virtual texture; its tiles are painted on demand by `source`
    /// ([`TileSource`]). Paints the root tile right away for the handle's preview image.
    /// Fails when the size can't be virtual or the format isn't RGBA8.
    pub fn add_procedural(
        &mut self,
        images: &mut Assets<Image>,
        source: impl TileSource,
    ) -> Result<Handle<Image>, String> {
        if source.format() != PixelFormat::Rgba8 {
            return Err("procedural virtual textures must paint RGBA8 tiles".into());
        }
        let provider: Arc<dyn TileProvider> = Arc::new(ProceduralProvider(Arc::new(source)));
        let preview = root_image(provider.as_ref())?;
        let handle = images.add(preview);
        self.insert(handle, "procedural".into(), State::Ready(provider));
        Ok(handle)
    }

    /// Whether `handle` is one of these, and how far along it is.
    pub fn status(&self, handle: Handle<Image>) -> VirtualTextureStatus {
        match self.entries.get(&handle.index()).map(|e| &e.state) {
            None => VirtualTextureStatus::NotVirtual,
            Some(State::Opening(_)) => VirtualTextureStatus::Baking,
            Some(State::Ready(_)) => VirtualTextureStatus::Ready,
            Some(State::Failed(why)) => VirtualTextureStatus::Failed(why.clone()),
        }
    }

    /// The bake this handle's file needed when it was opened (`None` when its tile file was
    /// already there, for procedural textures, or before it is ready).
    pub fn bake_report(&self, handle: Handle<Image>) -> Option<&BakeReport> {
        self.entries.get(&handle.index())?.bake.as_ref()
    }

    /// Stop treating `handle` as virtual (its tiles leave the cache within a few frames).
    pub fn remove(&mut self, handle: Handle<Image>) {
        self.entries.remove(&handle.index());
    }

    /// Whether every registered texture has finished opening (none is still baking).
    pub fn all_opened(&self) -> bool {
        !self
            .entries
            .values()
            .any(|e| matches!(e.state, State::Opening(_)))
    }

    /// The provider of a ready entry and its generation.
    pub(crate) fn provider(&self, index: u32) -> Option<(Arc<dyn TileProvider>, u64)> {
        match self.entries.get(&index) {
            Some(Registered {
                state: State::Ready(p),
                generation,
                ..
            }) => Some((p.clone(), *generation)),
            _ => None,
        }
    }

    /// Whether `index` is registered (ready or not): such images are never treated as ordinary
    /// big images.
    pub(crate) fn is_registered(&self, index: u32) -> bool {
        self.entries.contains_key(&index)
    }

    /// Finish opened files: put their preview in the image, or log why they failed.
    fn poll(&mut self, images: &mut Assets<Image>) {
        for (&index, entry) in &mut self.entries {
            let State::Opening(task) = &entry.state else {
                continue;
            };
            if !task.is_finished() {
                continue;
            }
            let State::Opening(task) =
                std::mem::replace(&mut entry.state, State::Failed(String::new()))
            else {
                continue;
            };
            let result = task
                .join()
                .unwrap_or_else(|_| Err("the bake thread panicked".into()))
                .and_then(|(provider, report)| {
                    let preview = root_image(provider.as_ref())?;
                    Ok((provider, report, preview))
                });
            match result {
                Ok((provider, report, preview)) => {
                    if let Some(r) = &report {
                        tracing::info!(
                            "baked virtual texture {} ({}x{}, {} tiles, {:.1} MB, {:.1} s)",
                            entry.name,
                            r.width,
                            r.height,
                            r.tiles,
                            r.bytes as f64 / 1e6,
                            r.seconds
                        );
                    }
                    entry.bake = report;
                    images.set(Handle::from_index(index), preview);
                    entry.state = State::Ready(provider);
                }
                Err(why) => {
                    tracing::warn!("virtual texture {} unavailable: {why}", entry.name);
                    entry.state = State::Failed(why);
                }
            }
        }
    }
}

/// Finish virtual textures whose tile files were opened or baked in the background (runs every
/// frame in `Extract`, added by `RenderPlugin`).
pub fn poll_virtual_textures(
    textures: Option<ResMut<VirtualTextures>>,
    images: Option<ResMut<Assets<Image>>>,
) {
    if let (Some(mut textures), Some(mut images)) = (textures, images) {
        if textures.all_opened() {
            return;
        }
        textures.poll(&mut images);
    }
}

/// Counters of the virtual texture cache, updated every frame by the mesh feature (also in its
/// `render.stats` description).
#[derive(Resource, Debug, Clone, Default, Serialize)]
pub struct VirtualTextureStats {
    /// Virtual textures drawn recently.
    pub textures: u32,
    /// Tiles resident in the cache (both cache textures).
    pub resident_tiles: u32,
    /// Slots of the cache textures created so far.
    pub cache_tiles: u32,
    /// GPU memory of the cache textures created so far (with their slot mips), bytes.
    pub cache_bytes: u64,
    /// The part of `cache_bytes` holding resident tiles.
    pub resident_bytes: u64,
    /// Tiles made resident this frame.
    pub tiles_loaded: u32,
    /// Tiles made resident since startup.
    pub tiles_loaded_total: u64,
    /// Distinct tiles the last feedback asked for.
    pub feedback_requests: u32,
    /// Of those, tiles that weren't resident (shown with a coarser fallback).
    pub feedback_misses: u32,
    /// Tiles being loaded or waiting for upload.
    pub pending: u32,
    /// Tiles evicted since startup.
    pub evictions: u64,
    /// Resident tiles per level (0 = full detail).
    pub resident_by_level: Vec<u32>,
    /// The cache is BC7-compressed.
    pub compressed: bool,
    /// The GPU writes feedback (else the coarsest levels that fit are kept).
    pub feedback: bool,
}

/// Whether `path` names a file that exists (for code that falls back when a fetched file is
/// missing).
pub fn file_exists(server: &AssetServer, key: &str) -> bool {
    Path::new(&server.root().join(engine_assets::normalize_path(key))).is_file()
}
