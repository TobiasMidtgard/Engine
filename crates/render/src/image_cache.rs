//! The renderer's GPU copies of [`Image`] assets, shared by every render feature, and the
//! release of their CPU pixels.
//!
//! Features get an image's texture with
//! [`GpuShared::image_texture`](crate::features::GpuShared::image_texture): the first feature
//! that asks uploads it (with its mip chain), and every feature that asks for the same image
//! shares that upload. A new asset version with new pixels uploads it again (a new
//! [`ImageTexture::id`]); images no feature asked for in [`EVICT_AFTER_FRAMES`] frames are
//! dropped from the GPU.
//!
//! **Releasing pixels.** With `RenderSettings::release_image_pixels`, once an uploaded image
//! has kept the same asset version for [`RELEASE_AFTER_FRAMES`] rendered frames, the renderer
//! frees its `data` and `mips` (keeping its size, format and flags): the GPU copy is then the
//! only one. Images that keep changing never get there, since each change uploads them again
//! and restarts the count. Releasing goes through `Assets::get_mut`, so it bumps the asset
//! version; the cache recognizes a released image (empty `data` with the size and format of its
//! GPU copy) and keeps its texture and id, so releasing never uploads anything again. Assigning
//! new `data` later is an ordinary change: it uploads again and shows the new pixels. A released
//! image stays on the GPU however long it goes unused (it could not be uploaded again), until its
//! asset is removed or given new pixels. [`Image::keep_pixels`] opts an image out.
//!
//! **Compression.** With `RenderSettings::texture_compression` and a GPU with BC formats, an
//! uploaded RGBA8 image that has stayed the same for [`COMPRESS_AFTER_FRAMES`] frames is
//! block-compressed on the GPU, from its GPU copy (so it works after its pixels were released),
//! and its texture is swapped for the compressed one under a new [`ImageTexture::id`]. The format
//! follows what the slot reads ([`TextureUse`]), as Unreal's texture compression settings and
//! Godot's importer do: BC7 for color and packed data (1 byte a texel, close to the original;
//! normal maps keep their z, so the length of a filtered normal still tells the shader how much
//! the texels disagree), BC4 for one-channel slots (height, occlusion), BC5 for two-channel
//! normal maps whose shader rebuilds z, and BC1 for opaque color in
//! `TextureCompression::Compact`. The encoders are a WGSL port of Intel's ISPC texture
//! compressor (the `block_compression` crate).
//!
//! The work is spread over frames in slices of rows, as Unreal's runtime virtual textures
//! compress a few pages a frame, within a GPU time budget: `RenderSettings::texture_compression_ms`
//! a frame in play (1 ms), `texture_compression_loading_ms` while a `LoadingBudget` is present
//! (4 ms: the picture still updates when arriving somewhere). Timestamps around the compression
//! pass measure its time per texel (a moving average, [`DEFAULT_MS_PER_MTEXEL`] until measured
//! or without `TIMESTAMP_QUERY`), and a frame takes the slices that fit. So a burst of new 4K
//! textures costs a millisecond or so of GPU a frame instead of stalling one for hundreds. BC7
//! uses ISPC's "fast" profiles. A view goes on showing its RGBA8 texture until its last slice is
//! done. The commands go with the frame's own submit (`GpuShared::submit_with_frame`), not a
//! submit of their own on the main thread, which would wait for the render thread's.
//!
//! Images whose levels don't halve to multiples of 4 texels stay RGBA8, as do
//! [`TextureUse::Exact`] slots and 16-bit float images. Under a software GPU (lavapipe) nothing
//! is compressed unless `RenderSettings::texture_compression_on_cpu` says so: building the
//! encoders takes seconds there and every sample of a compressed texture is decoded in software.
//!
//! Only images uploaded here are ever released, so images that only other features upload
//! themselves (from `Image::data`) keep their pixels. An image that both the renderer and such a
//! feature use needs `keep_pixels` until that feature switches to `image_texture`.

use std::collections::HashMap;
use std::sync::mpsc::{Receiver, TryRecvError};
use std::sync::{Arc, Mutex};

use block_compression::{BC7Settings, CompressionVariant, GpuBlockCompressor};

use engine_assets::{Assets, Handle};

use crate::features::GpuShared;
use crate::settings::TextureCompression;
use crate::texture::{Image, PixelFormat};

/// With `RenderSettings::release_image_pixels`, an uploaded image's pixels are freed once its
/// asset version has stayed the same for this many rendered frames (2 seconds at 60 fps).
///
/// Counted in frames rather than seconds so that a texture written every frame or every few
/// frames is never released at any frame rate, including slow software rendering where two
/// seconds are only a handful of frames.
pub const RELEASE_AFTER_FRAMES: u64 = 120;

/// Uploaded images no feature asked for in this many frames are dropped from the GPU (unless
/// their pixels were released: then the GPU copy is the only one and stays).
pub const EVICT_AFTER_FRAMES: u64 = 600;

/// With `RenderSettings::texture_compression`, an uploaded image is compressed once it has stayed
/// unchanged for this many frames, so textures rewritten every few frames stay RGBA8 instead of
/// being compressed over and over.
pub const COMPRESS_AFTER_FRAMES: u64 = 30;

/// GPU milliseconds per million BC7 texels assumed until timestamps measure it. The lead's PC
/// took 4.5 with ISPC's "basic" profile; "fast" is 2.5 times as fast for opaque images and 1.8
/// times for ones with alpha (measured under lavapipe).
pub const DEFAULT_MS_PER_MTEXEL: f32 = 2.0;

/// The cost of a BC1, BC4 or BC5 texel against a BC7 one (their encoders search far less).
const SIMPLE_TEXEL_COST: f32 = 0.25;

/// Frames that queued less than this (texels of BC7 cost) don't update the measured rate: too
/// little work to time well.
const MIN_TIMED_COST: u64 = 64 << 10;

/// Texels of BC7 cost that fit in `ms` at `ms_per_mtexel`.
fn cost_budget(ms: f32, ms_per_mtexel: f32) -> u64 {
    (ms.max(0.0) / ms_per_mtexel.max(1e-3) * 1e6) as u64
}

/// The measured rate after a frame that took `ms` for `cost` texels of BC7 cost.
fn next_rate(rate: f32, ms: f32, cost: u64) -> f32 {
    let sample = ms / (cost as f32 / 1e6);
    if cost < MIN_TIMED_COST || !sample.is_finite() || sample <= 0.0 {
        return rate;
    }
    rate * 0.7 + sample.clamp(0.01, 10_000.0) * 0.3
}

/// Images smaller than this on either side are left uncompressed (too small to matter).
const COMPRESS_MIN_SIZE: u32 = 16;

/// What a texture slot reads from an image, which decides how the renderer may compress it
/// (see the module docs).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TextureUse {
    /// Color: an sRGB RGBA8 image is viewed as sRGB. BC7 (BC1 for opaque images with
    /// `TextureCompression::Compact`).
    Color,
    /// Linear RGBA data: normal maps read with z, packed metallic/roughness. BC7.
    Linear,
    /// Only red is read (height, occlusion, a grayscale mask). BC4: half the memory of BC7 and
    /// finer steps for one channel; green and blue read as 0.
    Red,
    /// Only red and green are read (a normal map whose shader rebuilds z). BC5; blue reads as 0.
    RedGreen,
    /// The pixels as they are (lookup tables, data read with `textureLoad`): never compressed.
    Exact,
}

/// Views per image: linear, sRGB, red, red-green, exact.
pub(crate) const VIEW_SLOTS: usize = 5;

/// Per slot, the other slots whose view can stand in for a released image's (closest first).
const FALLBACK: [[usize; 4]; VIEW_SLOTS] = [
    [4, 1, 3, 2],
    [0, 4, 3, 2],
    [3, 0, 4, 1],
    [0, 4, 1, 2],
    [0, 1, 3, 2],
];

impl TextureUse {
    /// Which of an image's views this use binds (`srgb`: an sRGB view of an RGBA8 image).
    fn slot(self, srgb: bool) -> usize {
        match self {
            TextureUse::Color if srgb => 1,
            TextureUse::Color | TextureUse::Linear => 0,
            TextureUse::Red => 2,
            TextureUse::RedGreen => 3,
            TextureUse::Exact => 4,
        }
    }
}

/// A block-compressed format an uploaded view is due to become.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Encoding {
    Bc1,
    Bc4,
    Bc5,
    Bc7 { alpha: bool },
}

impl Encoding {
    fn format(self, srgb: bool) -> wgpu::TextureFormat {
        use wgpu::TextureFormat as F;
        match (self, srgb) {
            (Encoding::Bc1, false) => F::Bc1RgbaUnorm,
            (Encoding::Bc1, true) => F::Bc1RgbaUnormSrgb,
            (Encoding::Bc4, _) => F::Bc4RUnorm,
            (Encoding::Bc5, _) => F::Bc5RgUnorm,
            (Encoding::Bc7 { .. }, false) => F::Bc7RgbaUnorm,
            (Encoding::Bc7 { .. }, true) => F::Bc7RgbaUnormSrgb,
        }
    }

    /// `fast`: ISPC's "fast" BC7 profiles (the partitioned modes with the most candidates
    /// skipped) instead of "basic" (every mode searched, two refinement passes).
    fn variant(self, fast: bool) -> CompressionVariant {
        match (self, fast) {
            (Encoding::Bc1, _) => CompressionVariant::BC1,
            (Encoding::Bc4, _) => CompressionVariant::BC4,
            (Encoding::Bc5, _) => CompressionVariant::BC5,
            (Encoding::Bc7 { alpha: false }, false) => {
                CompressionVariant::BC7(BC7Settings::opaque_basic())
            }
            (Encoding::Bc7 { alpha: true }, false) => {
                CompressionVariant::BC7(BC7Settings::alpha_basic())
            }
            (Encoding::Bc7 { alpha: false }, true) => {
                CompressionVariant::BC7(BC7Settings::opaque_fast())
            }
            (Encoding::Bc7 { alpha: true }, true) => {
                CompressionVariant::BC7(BC7Settings::alpha_fast())
            }
        }
    }

    /// What a texel costs to encode, against a BC7 one.
    fn cost(self) -> f32 {
        match self {
            Encoding::Bc7 { .. } => 1.0,
            _ => SIMPLE_TEXEL_COST,
        }
    }

    /// Bytes per 4×4 block.
    fn block_bytes(self) -> u32 {
        match self {
            Encoding::Bc1 | Encoding::Bc4 => 8,
            Encoding::Bc5 | Encoding::Bc7 { .. } => 16,
        }
    }

    /// The encoding for `image` read as `usage` (uploaded at `width`×`height` with `levels`
    /// mips), or `None` when it stays as it is.
    fn choose(
        mode: TextureCompression,
        usage: TextureUse,
        image: &Image,
        (width, height, levels): (u32, u32, u32),
    ) -> Option<Encoding> {
        if mode == TextureCompression::Off
            || usage == TextureUse::Exact
            || image.format != PixelFormat::Rgba8
            || width.min(height) < COMPRESS_MIN_SIZE
        {
            return None;
        }
        // Every level at least a block across must be whole blocks (smaller tail levels are
        // padded by the encoder's clamped reads).
        let whole = (0..levels).all(|l| {
            let (w, h) = ((width >> l).max(1), (height >> l).max(1));
            w < 4 || h < 4 || (w % 4 == 0 && h % 4 == 0)
        });
        if !whole {
            return None;
        }
        Some(match usage {
            TextureUse::Red => Encoding::Bc4,
            TextureUse::RedGreen => Encoding::Bc5,
            _ => {
                let opaque = image.data.as_chunks::<4>().0.iter().all(|p| p[3] == 255);
                if opaque && usage == TextureUse::Color && mode == TextureCompression::Compact {
                    Encoding::Bc1
                } else {
                    Encoding::Bc7 { alpha: !opaque }
                }
            }
        })
    }
}

/// An image's texture on the GPU, from [`GpuShared::image_texture`](crate::features::GpuShared::image_texture).
pub struct ImageTexture {
    /// The view to bind: 2D, filterable float, every mip level.
    pub view: wgpu::TextureView,
    /// `view` decodes sRGB (a color slot of an sRGB RGBA8 image).
    pub srgb: bool,
    /// Pixels of the texture's first level (smaller than the image when the image was larger
    /// than the GPU allows: the levels that don't fit are skipped).
    pub width: u32,
    /// See `width`.
    pub height: u32,
    /// Mip levels on the GPU.
    pub mip_levels: u32,
    /// Unique per upload and view. It changes when the image is uploaded again (new pixels), so
    /// a feature rebuilds the bind groups that bind `view` when it sees a new id. It stays the
    /// same when the renderer releases the image's CPU pixels.
    pub id: u64,
    texture: wgpu::Texture,
    /// The texture also accepts a view in the other color space (created with both formats while
    /// releasing was on), so a released image can still be bound both ways.
    aliasable: bool,
}

impl ImageTexture {
    /// The format `view` reads (block-compressed once `RenderSettings::texture_compression` has
    /// compressed it).
    pub fn format(&self) -> wgpu::TextureFormat {
        let f = self.texture.format();
        if self.srgb {
            f.add_srgb_suffix()
        } else {
            f.remove_srgb_suffix()
        }
    }

    /// The GPU texture (to copy from, or read back in tests).
    pub fn texture(&self) -> &wgpu::Texture {
        &self.texture
    }
}

/// Counters of the shared image cache (`render.images_released` and friends in `Diagnostics`).
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct ImageMemoryStats {
    /// Images with a GPU copy now.
    pub gpu_images: u32,
    /// Images whose CPU pixels are released now (their GPU copy is the only one).
    pub released_now: u32,
    /// Uploads since startup (an image used in both color spaces counts twice).
    pub uploads: u64,
    /// Images whose pixels were released since startup.
    pub released: u64,
    /// CPU bytes (level 0 and stored mips) freed by those releases.
    pub bytes_released: u64,
    /// Views block-compressed since startup (`RenderSettings::texture_compression`).
    pub compressed: u64,
    /// GPU bytes those compressions saved (RGBA8 size minus compressed size, all levels).
    pub bytes_compressed_away: u64,
    /// Texels compressed since startup (all levels; whole slices, finished views or not).
    pub texels_compressed: u64,
}

/// One image's GPU copies.
struct CachedImage {
    /// Asset version the GPU copies show (after a release: the version the release left).
    version: u64,
    /// Frame this version was uploaded: the release counts from here.
    since: u64,
    /// Frame a feature last asked for it.
    last_used: u64,
    width: u32,
    height: u32,
    format: PixelFormat,
    /// The CPU pixels were freed: these textures are the only copy.
    released: bool,
    /// Views by [`TextureUse::slot`]: linear, sRGB, red, red-green, exact.
    views: [Option<Arc<ImageTexture>>; VIEW_SLOTS],
    /// Views due to be compressed, and to what.
    pending: [Option<Encoding>; VIEW_SLOTS],
    /// Mean linear color of the pixels as uploaded (RGBA8 images), which outlives a release.
    mean: Option<[f32; 3]>,
}

impl CachedImage {
    /// Whether `image` (with empty data) is this image released, not something else.
    fn same_shape(&self, image: &Image) -> bool {
        (self.width, self.height, self.format) == (image.width, image.height, image.format)
    }
}

#[derive(Default)]
struct Inner {
    /// By canonical image handle index.
    images: HashMap<u32, CachedImage>,
    /// Images that could not be uploaded, by (image index, view slot) → asset version, so a bad
    /// image is reported once per version instead of every frame.
    failed: HashMap<(u32, usize), u64>,
    frame: u64,
    /// `Assets::<Image>::removals()` when the cache was last swept.
    removals: u64,
    release: bool,
    next_id: u64,
    stats: ImageMemoryStats,
    /// Scratch list of images due for release.
    due: Vec<u32>,
    /// `RenderSettings::texture_compression` this frame.
    compression: TextureCompression,
    /// `RenderSettings::texture_compression_on_cpu` this frame.
    compress_on_cpu: bool,
    /// GPU milliseconds compression may take this frame.
    compress_ms: f32,
    /// Measured GPU milliseconds per million texels of BC7 cost (0 until first used).
    ms_per_mtexel: f32,
    /// Timestamps around the compression pass.
    timer: Option<crate::perf::GpuTimer>,
    timed: Vec<crate::perf::GpuFrame>,
    /// The view being compressed, a slice at a time.
    encoding: Option<Encode>,
    /// The GPU encoders, built on a thread the first time an image is due (BC7's pipeline
    /// takes a while to compile on some backends).
    compressor: Compressor,
    /// Another set for the rest of the renderer ([`ImageCache::with_compressor`]): an encoder
    /// set writes its uniforms with `queue.write_buffer`, so two users of one set between two
    /// submits would read each other's.
    side_compressor: Compressor,
    /// Images an extra view renders into (`ViewTarget::Image`), by canonical handle index: their
    /// texture is the view's picture.
    targets: HashMap<u32, Arc<ImageTexture>>,
}

impl Compressor {
    /// Encoders building on a thread.
    fn build(gpu: &GpuShared) -> Self {
        let (tx, rx) = std::sync::mpsc::channel();
        let (device, queue) = (gpu.device.clone(), gpu.queue.clone());
        let spawned = std::thread::Builder::new()
            .name("texture compressor".into())
            .spawn(move || {
                let _ = tx.send(GpuBlockCompressor::new(device, queue));
            });
        match spawned {
            Ok(_) => Compressor::Building(rx),
            Err(_) => Compressor::Failed,
        }
    }
}

#[derive(Default)]
enum Compressor {
    #[default]
    None,
    Building(Receiver<GpuBlockCompressor>),
    Ready(Box<GpuBlockCompressor>),
    /// The thread failed: compression stays off.
    Failed,
}

/// The renderer's uploaded images, shared by every feature (one per renderer, in
/// [`GpuShared`]). See the module docs.
#[derive(Default)]
pub struct ImageCache {
    inner: Mutex<Inner>,
}

impl ImageCache {
    fn lock(&self) -> std::sync::MutexGuard<'_, Inner> {
        self.inner.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// The texture of `handle`, uploading it when it is not on the GPU or its pixels changed.
    /// `color`: the slot holds color, so an sRGB RGBA8 image gets an sRGB view.
    pub fn get(
        &self,
        gpu: &GpuShared,
        images: &Assets<Image>,
        handle: Handle<Image>,
        color: bool,
    ) -> Option<Arc<ImageTexture>> {
        let usage = if color {
            TextureUse::Color
        } else {
            TextureUse::Linear
        };
        self.get_for(gpu, images, handle, usage)
    }

    /// Mean linear color of `handle`'s pixels (sRGB decoded), known once it was uploaded (as
    /// anything) while its pixels were there; `None` before, and for HDR images. Probe GI's rays
    /// see textured surfaces in this color.
    pub fn mean_color(&self, images: &Assets<Image>, handle: Handle<Image>) -> Option<[f32; 3]> {
        let index = images.canonical(handle).index();
        self.lock().images.get(&index).and_then(|c| c.mean)
    }

    /// The texture of `handle` for a slot that reads it as `usage`, uploading it when it is not
    /// on the GPU or its pixels changed. Each use has its own GPU copy (compressed its own way).
    pub fn get_for(
        &self,
        gpu: &GpuShared,
        images: &Assets<Image>,
        handle: Handle<Image>,
        usage: TextureUse,
    ) -> Option<Arc<ImageTexture>> {
        let handle = images.canonical(handle);
        let index = handle.index();
        if let Some(target) = self.lock().targets.get(&index) {
            return Some(target.clone());
        }
        let version = images.version(handle);
        let image = images.get(handle)?;
        let srgb = usage == TextureUse::Color && image.srgb && image.format == PixelFormat::Rgba8;
        let slot = usage.slot(srgb);
        let mut guard = self.lock();
        let inner = &mut *guard;
        let frame = inner.frame;
        // A new version of an image already on the GPU uploads at once (see `UploadBudget`).
        let mut update = false;
        if let Some(cached) = inner.images.get_mut(&index)
            && cached.version != version
        {
            if image.is_released() && cached.same_shape(image) {
                // Released (by us, or emptied by someone else): the GPU copy is all there is, and
                // it still shows this image. Keep it; nothing to upload.
                cached.version = version;
                cached.released = true;
            } else {
                // New pixels.
                update = cached.views[slot].is_some();
                inner.images.remove(&index);
            }
        }
        if let Some(cached) = inner.images.get_mut(&index) {
            cached.last_used = frame;
            if let Some(view) = &cached.views[slot] {
                return Some(view.clone());
            }
            if image.is_released()
                && slot < 2
                && let Some(other) = cached.views[1 - slot].as_ref().filter(|o| o.aliasable)
            {
                // Only the other color space is on the GPU: view the same texture in this one.
                inner.next_id += 1;
                let view = Arc::new(alias_view(other, srgb, inner.next_id));
                cached.views[slot] = Some(view.clone());
                return Some(view);
            }
        }
        if inner.failed.get(&(index, slot)) == Some(&version) {
            return None;
        }
        if image.is_released()
            && let Some(cached) = inner.images.get_mut(&index)
            && let Some(other) = FALLBACK[slot].iter().find_map(|&o| cached.views[o].clone())
        {
            // Asked for in a use it was never uploaded for, after its pixels went: the copy on
            // the GPU holds the same texels (maybe another color space or block format), which
            // beats drawing without it.
            tracing::debug!("image {index}: released, viewed through its slot {slot} stand-in");
            cached.views[slot] = Some(other.clone());
            return Some(other);
        }
        if image.is_released() {
            tracing::warn!(
                "image {index} ({image:?}) has no pixels: they were released after upload \
                 (RenderSettings::release_image_pixels) and the GPU has no copy of it {}; set \
                 Image::keep_pixels on images that are read or uploaded outside \
                 GpuShared::image_texture",
                if srgb { "as sRGB" } else { "as linear data" }
            );
            inner.failed.insert((index, slot), version);
            return None;
        }
        // Compressed from the GPU copy, which the encoders read as plain RGBA8 (not on GL, which
        // can't reinterpret formats; nor without BC formats).
        let compressible = gpu
            .features
            .contains(wgpu::Features::TEXTURE_COMPRESSION_BC)
            && gpu.adapter_info.backend != wgpu::Backend::Gl
            && (gpu.adapter_info.device_type != wgpu::DeviceType::Cpu || inner.compress_on_cpu)
            && inner.compressor_usable();
        let compression = if compressible {
            inner.compression
        } else {
            TextureCompression::Off
        };
        // Textures that may lose their CPU pixels or be compressed also accept a view in the
        // other color space (not on GL).
        let aliasable = (inner.release && !image.keep_pixels
            || compression != TextureCompression::Off)
            && image.format == PixelFormat::Rgba8
            && gpu.adapter_info.backend != wgpu::Backend::Gl;
        // When this frame's upload budget is spent the caller draws without the image and
        // asks again next frame (mips add a third).
        let bytes = image.data.len() as u64 * if image.mipmaps { 4 } else { 3 } / 3;
        if !gpu.uploads.take(
            index as u64 * VIEW_SLOTS as u64 + slot as u64,
            bytes,
            update,
        ) {
            return None;
        }
        let Some((texture, view)) = upload_image(gpu, image, srgb, aliasable) else {
            inner.failed.insert((index, slot), version);
            return None;
        };
        let encoding = Encoding::choose(
            compression,
            usage,
            image,
            (texture.width(), texture.height(), texture.mip_level_count()),
        );
        inner.failed.remove(&(index, slot));
        inner.next_id += 1;
        inner.stats.uploads += 1;
        let uploaded = Arc::new(ImageTexture {
            srgb,
            width: texture.width(),
            height: texture.height(),
            mip_levels: texture.mip_level_count(),
            id: inner.next_id,
            view,
            texture,
            aliasable,
        });
        let cached = inner.images.entry(index).or_insert_with(|| CachedImage {
            version,
            since: frame,
            last_used: frame,
            width: image.width,
            height: image.height,
            format: image.format,
            released: false,
            views: Default::default(),
            pending: [None; VIEW_SLOTS],
            mean: mean_color(image),
        });
        cached.views[slot] = Some(uploaded.clone());
        cached.pending[slot] = encoding;
        if encoding.is_some() {
            // Ready by the time the image is due.
            inner.start_compressor(gpu);
        }
        Some(uploaded)
    }

    /// From now on image `index` (a canonical handle index) shows `texture`, an extra view's
    /// picture (`ViewTarget::Image`).
    pub(crate) fn set_target(&self, index: u32, texture: &wgpu::Texture) {
        let mut inner = self.lock();
        inner.next_id += 1;
        let target = Arc::new(ImageTexture {
            view: texture.create_view(&Default::default()),
            srgb: texture.format().is_srgb(),
            width: texture.width(),
            height: texture.height(),
            mip_levels: texture.mip_level_count(),
            id: inner.next_id,
            texture: texture.clone(),
            aliasable: false,
        });
        inner.targets.insert(index, target);
    }

    /// Image `index` is no extra view's target any more.
    pub(crate) fn remove_target(&self, index: u32) {
        self.lock().targets.remove(&index);
    }

    /// Start frame `frame`; `release` is `RenderSettings::release_image_pixels`; `compress_ms`
    /// the GPU time compression may take this frame.
    pub(crate) fn begin_frame(
        &self,
        frame: u64,
        release: bool,
        (compression, on_cpu, compress_ms): (TextureCompression, bool, f32),
    ) {
        let mut inner = self.lock();
        inner.frame = frame;
        inner.release = release;
        inner.compress_on_cpu = on_cpu;
        inner.compress_ms = compress_ms;
        if compression != inner.compression {
            // Views already compressed stay; pending ones follow the new setting when they are
            // next uploaded.
            if compression == TextureCompression::Off {
                inner
                    .images
                    .values_mut()
                    .for_each(|c| c.pending = [None; VIEW_SLOTS]);
                inner.encoding = None;
            }
            inner.compression = compression;
        }
    }

    /// Compress the views that have been unchanged for [`COMPRESS_AFTER_FRAMES`], a slice of
    /// rows at a time up to this frame's GPU time budget, and swap in the ones finished. The
    /// commands go with the frame's submit.
    pub(crate) fn compress(&self, gpu: &GpuShared) {
        let mut guard = self.lock();
        let inner = &mut *guard;
        let frame = inner.frame;
        if inner.ms_per_mtexel <= 0.0 {
            inner.ms_per_mtexel = DEFAULT_MS_PER_MTEXEL;
        }
        if let Some(timer) = &mut inner.timer {
            timer.collect(&mut inner.timed);
            for f in inner.timed.drain(..) {
                inner.ms_per_mtexel = next_rate(inner.ms_per_mtexel, f.total, f.tag);
            }
        }
        let due = |c: &CachedImage| {
            frame >= c.since + COMPRESS_AFTER_FRAMES && c.pending.iter().any(Option::is_some)
        };
        if inner.compression == TextureCompression::Off {
            inner.encoding = None;
            return;
        }
        if inner.encoding.is_none() && !inner.images.values().any(&due) {
            return;
        }
        let compressor = match &mut inner.compressor {
            Compressor::Ready(c) => c,
            Compressor::None => {
                inner.start_compressor(gpu);
                return;
            }
            Compressor::Building(rx) => match rx.try_recv() {
                Ok(c) => {
                    inner.compressor = Compressor::Ready(Box::new(c));
                    return;
                }
                Err(TryRecvError::Empty) => return,
                Err(TryRecvError::Disconnected) => {
                    tracing::warn!("texture compressor failed to build; images stay RGBA8");
                    inner.compressor = Compressor::Failed;
                    inner
                        .images
                        .values_mut()
                        .for_each(|c| c.pending = [None; VIEW_SLOTS]);
                    return;
                }
            },
            Compressor::Failed => return,
        };

        let mut budget = cost_budget(inner.compress_ms, inner.ms_per_mtexel);
        let (mut queued, mut cost) = (0, 0);
        let mut finished = Vec::new();
        while budget > 0 {
            if inner.encoding.is_none() {
                let next =
                    inner
                        .images
                        .iter_mut()
                        .filter(|(_, c)| due(c))
                        .find_map(|(&index, cached)| {
                            (0..VIEW_SLOTS).find_map(|slot| {
                                let encoding = cached.pending[slot].take()?;
                                let source = cached.views[slot].clone()?;
                                Some((index, slot, encoding, source))
                            })
                        });
                let Some((index, slot, encoding, source)) = next else {
                    break;
                };
                inner.encoding = Some(Encode::new(gpu, index, slot, encoding, true, source));
            }
            let Some(job) = &mut inner.encoding else {
                break;
            };
            // Re-uploaded (new pixels) or dropped since it started: give it up.
            let current = inner
                .images
                .get(&job.index)
                .and_then(|c| c.views[job.slot].as_ref())
                .is_some_and(|v| Arc::ptr_eq(v, &job.source));
            if !current {
                inner.encoding = None;
                continue;
            }
            let (texels, spent) = job.queue(compressor, budget);
            queued += texels;
            cost += spent;
            budget = budget.saturating_sub(spent);
            if job.done() {
                finished.extend(inner.encoding.take());
            }
        }
        if queued == 0 && finished.is_empty() {
            return;
        }

        let mut encoder = gpu
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("texture compression"),
            });
        // One compute pass for every slice queued above, timed; the copies of the finished views
        // into their compressed textures follow it.
        if inner.timer.is_none() {
            inner.timer = crate::perf::GpuTimer::new(&gpu.device, &gpu.queue);
        }
        if let Some(timer) = &mut inner.timer {
            timer.begin_tagged(&mut encoder, "texture compression".into(), cost);
        }
        {
            let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
                label: Some("texture compression"),
                timestamp_writes: None,
            });
            compressor.compress(&mut pass);
        }
        let after = inner
            .timer
            .as_mut()
            .and_then(|t| t.end_deferred(&mut encoder));
        for job in &finished {
            job.out.copy(&mut encoder);
        }
        gpu.submit_with_frame(encoder.finish(), after);
        gpu.side_work.compressed(queued);
        inner.stats.texels_compressed += queued;

        for Encode {
            index,
            slot,
            encoding,
            source,
            out,
            ..
        } in finished
        {
            let Some(cached) = inner.images.get_mut(&index) else {
                continue;
            };
            inner.next_id += 1;
            let view = Arc::new(out.into_texture(&source, inner.next_id));
            // A view of the same texture in the other color space follows it.
            if slot < 2
                && let Some(other) = &cached.views[1 - slot]
                && other.texture == source.texture
            {
                inner.next_id += 1;
                cached.views[1 - slot] =
                    Some(Arc::new(alias_view(&view, !view.srgb, inner.next_id)));
                cached.pending[1 - slot] = None;
            }
            cached.views[slot] = Some(view);
            inner.stats.compressed += 1;
            let rgba = texels_with_mips(source.width, source.height, source.mip_levels) * 4;
            inner.stats.bytes_compressed_away += rgba.saturating_sub(compressed_bytes(
                encoding,
                source.width,
                source.height,
                source.mip_levels,
            ));
        }
    }

    /// After the frame: release the pixels of images that have been on the GPU unchanged for
    /// [`RELEASE_AFTER_FRAMES`] (when releasing is on) and drop images nobody used for a while.
    /// Returns how many images were released.
    pub(crate) fn end_frame(&self, images: &mut Assets<Image>) -> u32 {
        let mut guard = self.lock();
        let inner = &mut *guard;
        let frame = inner.frame;
        let mut released = 0;
        if inner.release {
            inner.due.clear();
            inner.due.extend(
                inner
                    .images
                    .iter()
                    .filter(|(_, c)| !c.released && frame >= c.since + RELEASE_AFTER_FRAMES)
                    .map(|(index, _)| *index),
            );
            for &index in &inner.due {
                let handle = Handle::from_index(index);
                let Some(cached) = inner.images.get_mut(&index) else {
                    continue;
                };
                // Changed since it was uploaded: it uploads again when next drawn.
                if images.version(handle) != cached.version {
                    continue;
                }
                let releasable = images
                    .get(handle)
                    .is_some_and(|i| !i.keep_pixels && !i.is_released());
                if !releasable {
                    continue;
                }
                let freed = images.get_mut(handle).map_or(0, Image::release_pixels);
                cached.version = images.version(handle);
                cached.released = true;
                inner.stats.released += 1;
                inner.stats.bytes_released += freed as u64;
                released += 1;
            }
        }
        // Every couple of seconds, and as soon as images are removed (a streamed zone unloaded
        // and freed its images): their GPU copies go at once.
        let removed = images.removals() != inner.removals;
        if frame.is_multiple_of(120) || removed {
            inner.removals = images.removals();
            inner.images.retain(|&index, cached| {
                if !images.contains(Handle::from_index(index)) {
                    false
                } else if cached.released {
                    // The GPU copy is the only one: keep it while the asset is there and still
                    // released (new pixels upload again when it is next drawn).
                    images
                        .get(Handle::from_index(index))
                        .is_some_and(Image::is_released)
                } else {
                    cached.last_used + EVICT_AFTER_FRAMES >= frame
                }
            });
            inner.failed.retain(|&(index, _), version| {
                images.version(Handle::from_index(index)) == *version
            });
        }
        released
    }

    /// Run `f` with the GPU block encoders (for other parts of the renderer that compress, such as
    /// the impostor atlas): `None` while they are still being built (this starts building them).
    /// Tasks `f` adds must be compressed (`GpuBlockCompressor::compress`) before it returns.
    pub(crate) fn with_compressor<R>(
        &self,
        gpu: &GpuShared,
        f: impl FnOnce(&mut GpuBlockCompressor) -> R,
    ) -> Option<R> {
        let mut guard = self.lock();
        let inner = &mut *guard;
        if let Compressor::Building(rx) = &inner.side_compressor {
            match rx.try_recv() {
                Ok(c) => inner.side_compressor = Compressor::Ready(Box::new(c)),
                Err(TryRecvError::Empty) => return None,
                Err(TryRecvError::Disconnected) => {
                    inner.side_compressor = Compressor::Failed;
                    return None;
                }
            }
        }
        match &mut inner.side_compressor {
            Compressor::Ready(c) => Some(f(c)),
            Compressor::None => {
                inner.side_compressor = Compressor::build(gpu);
                None
            }
            _ => None,
        }
    }

    /// Measured GPU milliseconds per million BC7 texels compressed ([`DEFAULT_MS_PER_MTEXEL`]
    /// until timestamps tell).
    pub fn compress_ms_per_mtexel(&self) -> f32 {
        match self.lock().ms_per_mtexel {
            r if r > 0.0 => r,
            _ => DEFAULT_MS_PER_MTEXEL,
        }
    }

    /// Current counters.
    pub fn stats(&self) -> ImageMemoryStats {
        let inner = self.lock();
        ImageMemoryStats {
            gpu_images: inner.images.len() as u32,
            released_now: inner.images.values().filter(|c| c.released).count() as u32,
            ..inner.stats
        }
    }
}

impl Inner {
    /// Start building the encoders on a thread (when they aren't built or building yet).
    fn start_compressor(&mut self, gpu: &GpuShared) {
        if matches!(self.compressor, Compressor::None) {
            self.compressor = Compressor::build(gpu);
        }
    }

    /// Whether the encoders exist or may still be built.
    fn compressor_usable(&self) -> bool {
        !matches!(self.compressor, Compressor::Failed)
    }
}

/// Texels in a `width`×`height` texture with `levels` mips.
fn texels_with_mips(width: u32, height: u32, levels: u32) -> u64 {
    (0..levels)
        .map(|l| ((width >> l).max(1) as u64) * ((height >> l).max(1) as u64))
        .sum()
}

/// Bytes of a `width`×`height` texture with `levels` mips in `encoding` (whole blocks).
fn compressed_bytes(encoding: Encoding, width: u32, height: u32, levels: u32) -> u64 {
    (0..levels)
        .map(|l| {
            let (w, h) = ((width >> l).max(1), (height >> l).max(1));
            (w.div_ceil(4) * h.div_ceil(4) * encoding.block_bytes()) as u64
        })
        .sum()
}

/// A compressed copy being recorded: the blocks land in `buffer`, then in `texture`.
struct Compressed {
    texture: wgpu::Texture,
    buffer: wgpu::Buffer,
    /// Byte offset of each level's blocks in `buffer`.
    offsets: Vec<u64>,
    block_bytes: u32,
}

impl Compressed {
    /// Record the copies of every level's blocks into the texture (after the compute pass).
    fn copy(&self, encoder: &mut wgpu::CommandEncoder) {
        for (level, &offset) in self.offsets.iter().enumerate() {
            let level = level as u32;
            let size = (
                (self.texture.width() >> level).max(1),
                (self.texture.height() >> level).max(1),
            );
            let at = (level, 0);
            copy_blocks(
                encoder,
                &self.buffer,
                offset,
                &self.texture,
                at,
                size,
                self.block_bytes,
            );
        }
    }

    /// The finished texture, standing in for `source` under `id`.
    fn into_texture(self, source: &ImageTexture, id: u64) -> ImageTexture {
        ImageTexture {
            view: self
                .texture
                .create_view(&wgpu::TextureViewDescriptor::default()),
            srgb: source.srgb,
            width: source.width,
            height: source.height,
            mip_levels: source.mip_levels,
            id,
            aliasable: source.aliasable && {
                let f = self.texture.format();
                f.add_srgb_suffix() != f.remove_srgb_suffix()
            },
            texture: self.texture,
        }
    }
}

/// Copy compressed blocks (rows in a row, tightly packed, from `offset` in `buffer`) into level
/// and array layer `at` of `texture`, which is `size` texels there. Buffer rows must be 256-byte
/// aligned to copy more than one at a time, so narrow levels go a block row per copy.
pub(crate) fn copy_blocks(
    encoder: &mut wgpu::CommandEncoder,
    buffer: &wgpu::Buffer,
    offset: u64,
    texture: &wgpu::Texture,
    (mip_level, layer): (u32, u32),
    (width, height): (u32, u32),
    block_bytes: u32,
) {
    let (w, h) = (width.div_ceil(4), height.div_ceil(4));
    let row = w * block_bytes;
    let rows: Vec<(u32, u32)> = if row.is_multiple_of(wgpu::COPY_BYTES_PER_ROW_ALIGNMENT) || h == 1
    {
        vec![(0, h)]
    } else {
        (0..h).map(|y| (y, 1)).collect()
    };
    for (y, count) in rows {
        encoder.copy_buffer_to_texture(
            wgpu::TexelCopyBufferInfo {
                buffer,
                layout: wgpu::TexelCopyBufferLayout {
                    offset: offset + (y * row) as u64,
                    bytes_per_row: (count > 1).then_some(row),
                    rows_per_image: (count > 1).then_some(count),
                },
            },
            wgpu::TexelCopyTextureInfo {
                texture,
                mip_level,
                origin: wgpu::Origin3d {
                    x: 0,
                    y: y * 4,
                    z: layer,
                },
                aspect: wgpu::TextureAspect::All,
            },
            wgpu::Extent3d {
                width: w * 4,
                height: count * 4,
                depth_or_array_layers: 1,
            },
        );
    }
}

/// A view being compressed over one or more frames.
struct Encode {
    /// Its image (canonical handle index) and view slot.
    index: u32,
    slot: usize,
    encoding: Encoding,
    /// ISPC's "fast" BC7 profile (in play) rather than "basic".
    fast: bool,
    /// The RGBA8 view it compresses (still bound until this is done).
    source: Arc<ImageTexture>,
    out: Compressed,
    /// The next slice: level, and first row (a multiple of 4).
    level: u32,
    row: u32,
}

impl Encode {
    /// Create the texture and block buffer the compressed `source` lands in.
    fn new(
        gpu: &GpuShared,
        index: u32,
        slot: usize,
        encoding: Encoding,
        fast: bool,
        source: Arc<ImageTexture>,
    ) -> Self {
        let (width, height, levels) = (source.width, source.height, source.mip_levels);
        let format = encoding.format(source.srgb);
        let alias = [if source.srgb {
            format.remove_srgb_suffix()
        } else {
            format.add_srgb_suffix()
        }];
        let view_formats: &[wgpu::TextureFormat] = if source.aliasable && alias[0] != format {
            &alias
        } else {
            &[]
        };
        let texture = gpu.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("compressed material image"),
            size: wgpu::Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
            mip_level_count: levels,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats,
        });
        let mut offsets = Vec::with_capacity(levels as usize);
        let mut total = 0u64;
        for l in 0..levels {
            offsets.push(total);
            total += compressed_bytes(encoding, (width >> l).max(1), (height >> l).max(1), 1);
        }
        let buffer = gpu.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("compressed blocks"),
            size: total,
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
            mapped_at_creation: false,
        });
        Self {
            index,
            slot,
            encoding,
            fast,
            source,
            out: Compressed {
                texture,
                buffer,
                offsets,
                block_bytes: encoding.block_bytes(),
            },
            level: 0,
            row: 0,
        }
    }

    /// Queue the next slices (read as plain RGBA8), about `budget` texels of BC7 cost (at least
    /// one block row). Returns the texels queued and their cost.
    fn queue(&mut self, compressor: &mut GpuBlockCompressor, budget: u64) -> (u64, u64) {
        let source = &self.source;
        let weight = self.encoding.cost();
        let (mut queued, mut cost) = (0, 0);
        while self.level < source.mip_levels && cost < budget {
            let l = self.level;
            // Levels under a block across are padded by the encoder's clamped reads.
            let w = (source.width >> l).max(1).next_multiple_of(4);
            let h = (source.height >> l).max(1).next_multiple_of(4);
            let row_cost = (w as f32 * weight).max(1.0);
            let fit = (((budget - cost) as f32 / row_cost) as u64).min(h as u64) as u32 / 4 * 4;
            let rows = fit.clamp(4, h - self.row);
            let view = source.texture.create_view(&wgpu::TextureViewDescriptor {
                label: Some("compression source"),
                format: Some(source.texture.format().remove_srgb_suffix()),
                base_mip_level: l,
                mip_level_count: Some(1),
                ..Default::default()
            });
            let offset = self.out.offsets[l as usize]
                + (self.row / 4) as u64 * (w / 4) as u64 * self.out.block_bytes as u64;
            compressor.add_compression_task(
                self.encoding.variant(self.fast),
                &view,
                w,
                rows,
                &self.out.buffer,
                Some(self.row),
                Some(offset as u32),
            );
            queued += (w * rows) as u64;
            cost += (row_cost * rows as f32) as u64;
            self.row += rows;
            if self.row >= h {
                self.level += 1;
                self.row = 0;
            }
        }
        (queued, cost)
    }

    /// Every level is queued.
    fn done(&self) -> bool {
        self.level >= self.source.mip_levels
    }
}

/// A view of `other`'s texture in the other color space (the texture was created with both).
fn alias_view(other: &ImageTexture, srgb: bool, id: u64) -> ImageTexture {
    let base = other.texture.format();
    let format = if srgb {
        base.add_srgb_suffix()
    } else {
        base.remove_srgb_suffix()
    };
    let view = other.texture.create_view(&wgpu::TextureViewDescriptor {
        format: Some(format),
        ..Default::default()
    });
    ImageTexture {
        view,
        srgb,
        width: other.width,
        height: other.height,
        mip_levels: other.mip_levels,
        id,
        texture: other.texture.clone(),
        aliasable: true,
    }
}

/// Upload an image with its mip chain. `srgb` selects an sRGB view format for RGBA8 data
/// (mips are re-generated in the matching color space when the stored ones don't match).
/// Levels larger than the device allows are skipped. `aliasable` also lets the RGBA8 texture be
/// viewed in the other color space.
/// Mean linear color of an RGBA8 image's pixels (about 4096 of them, evenly spread), sRGB
/// decoded when the image is sRGB.
fn mean_color(image: &Image) -> Option<[f32; 3]> {
    if image.format != PixelFormat::Rgba8 || image.data.len() < 4 {
        return None;
    }
    let pixels = image.data.len() / 4;
    let step = (pixels / 4096).max(1);
    let decode = |c: u8| {
        let c = c as f32 / 255.0;
        if !image.srgb {
            c
        } else if c <= 0.04045 {
            c / 12.92
        } else {
            ((c + 0.055) / 1.055).powf(2.4)
        }
    };
    let mut sum = [0.0f64; 3];
    let mut n = 0.0f64;
    for p in image.data.as_chunks::<4>().0.iter().step_by(step) {
        for c in 0..3 {
            sum[c] += decode(p[c]) as f64;
        }
        n += 1.0;
    }
    Some(sum.map(|s| (s / n) as f32))
}

pub(crate) fn upload_image(
    gpu: &GpuShared,
    image: &Image,
    srgb: bool,
    aliasable: bool,
) -> Option<(wgpu::Texture, wgpu::TextureView)> {
    if !image.is_valid() {
        tracing::warn!(
            "image data or mip chain does not match its size ({image:?}); not uploading it"
        );
        return None;
    }
    let format = match image.format {
        PixelFormat::Rgba8 if srgb => wgpu::TextureFormat::Rgba8UnormSrgb,
        PixelFormat::Rgba8 => wgpu::TextureFormat::Rgba8Unorm,
        PixelFormat::Rgba16Float => wgpu::TextureFormat::Rgba16Float,
    };
    let alias_formats = [match format {
        wgpu::TextureFormat::Rgba8UnormSrgb => wgpu::TextureFormat::Rgba8Unorm,
        _ => wgpu::TextureFormat::Rgba8UnormSrgb,
    }];
    let view_formats: &[wgpu::TextureFormat] = if aliasable && image.format == PixelFormat::Rgba8 {
        &alias_formats
    } else {
        &[]
    };
    let generated;
    let max = gpu.device.limits().max_texture_dimension_2d;
    let stored =
        !image.mips.is_empty() && (image.format != PixelFormat::Rgba8 || srgb == image.srgb);
    // A large image without a fitting chain gets it drawn on the GPU (`mipmaps`) rather than
    // made on this thread.
    let gpu_mips = image.mipmaps
        && !stored
        && image.width as u64 * image.height as u64 >= crate::mipmaps::GPU_MIPS_MIN_TEXELS
        && image.width.max(image.height) <= max
        && crate::mipmaps::MipGenerator::supports(format);
    let mips: &[Vec<u8>] = if !image.mipmaps || gpu_mips {
        &[]
    } else if stored {
        &image.mips
    } else {
        generated = image.mip_chain(srgb);
        &generated
    };
    let mut levels: Vec<(u32, u32, &[u8])> = Vec::with_capacity(mips.len() + 1);
    levels.push((image.width, image.height, &image.data));
    for (i, m) in mips.iter().enumerate() {
        let (w, h) = image.level_size(i as u32 + 1);
        levels.push((w, h, m));
    }
    let skip = levels
        .iter()
        .take_while(|(w, h, _)| *w > max || *h > max)
        .count();
    let levels = &levels[skip.min(levels.len())..];
    let Some(&(width, height, _)) = levels.first() else {
        tracing::warn!(
            "image {}x{} is larger than the GPU allows ({max})",
            image.width,
            image.height
        );
        return None;
    };
    let texture = gpu.device.create_texture(&wgpu::TextureDescriptor {
        label: Some("material image"),
        size: wgpu::Extent3d {
            width,
            height,
            depth_or_array_layers: 1,
        },
        mip_level_count: if gpu_mips {
            image.full_mip_count()
        } else {
            levels.len() as u32
        },
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format,
        usage: wgpu::TextureUsages::TEXTURE_BINDING
            | wgpu::TextureUsages::COPY_DST
            | if gpu_mips {
                // Drawn into; copied from to check the chain (tests/gpu_mips.rs).
                wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC
            } else {
                wgpu::TextureUsages::empty()
            },
        view_formats,
    });
    let bpp = image.format.bytes_per_pixel() as u32;
    for (level, (w, h, data)) in levels.iter().enumerate() {
        gpu.queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &texture,
                mip_level: level as u32,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            data,
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(w * bpp),
                rows_per_image: Some(*h),
            },
            wgpu::Extent3d {
                width: *w,
                height: *h,
                depth_or_array_layers: 1,
            },
        );
    }
    if gpu_mips {
        gpu.mips.generate(gpu, &texture, format);
    }
    let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
    Some((texture, view))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_textures_mean_color_is_linear() {
        // Half black, half sRGB 188 (linear 0.5): a mean of 0.25, not of the encoded values.
        let mut data = Vec::new();
        for i in 0..64 {
            let c = if i % 2 == 0 { 0 } else { 188 };
            data.extend_from_slice(&[c, c, 0, 255]);
        }
        let mean = mean_color(&Image::new(8, 8, data.clone(), true)).expect("RGBA8");
        assert!((mean[0] - 0.25).abs() < 0.01 && mean[2] == 0.0, "{mean:?}");
        let linear = mean_color(&Image::new(8, 8, data, false)).expect("RGBA8");
        assert!((linear[1] - 94.0 / 255.0).abs() < 0.01, "{linear:?}");
    }

    #[test]
    fn the_budget_follows_the_measured_rate() {
        // 1 ms at 2 ms per million texels: half a million.
        assert_eq!(cost_budget(1.0, 2.0), 500_000);
        assert_eq!(cost_budget(0.0, 2.0), 0);
        // A frame that took 4 ms for a million texels moves the rate towards 4.
        let rate = next_rate(2.0, 4.0, 1 << 20);
        assert!(rate > 2.0 && rate < 4.0, "{rate}");
        let mut rate = 2.0;
        for _ in 0..30 {
            rate = next_rate(rate, 4.0, 1_000_000);
        }
        assert!((rate - 4.0).abs() < 0.01, "{rate}");
        // Too little work, or no time, says nothing.
        assert_eq!(next_rate(2.0, 9.0, 1000), 2.0);
        assert_eq!(next_rate(2.0, 0.0, 1 << 20), 2.0);
    }
}
