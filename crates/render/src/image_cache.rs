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
//! Only images uploaded here are ever released, so images that only other features upload
//! themselves (from `Image::data`) keep their pixels. An image that both the renderer and such a
//! feature use needs `keep_pixels` until that feature switches to `image_texture`.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use engine_assets::{Assets, Handle};

use crate::features::GpuShared;
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
    /// Linear view, then sRGB view.
    views: [Option<Arc<ImageTexture>>; 2],
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
    /// Images that could not be uploaded, by (image index, sRGB view) → asset version, so a bad
    /// image is reported once per version instead of every frame.
    failed: HashMap<(u32, bool), u64>,
    frame: u64,
    release: bool,
    next_id: u64,
    stats: ImageMemoryStats,
    /// Scratch list of images due for release.
    due: Vec<u32>,
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
        let handle = images.canonical(handle);
        let index = handle.index();
        let version = images.version(handle);
        let image = images.get(handle)?;
        let srgb = color && image.srgb && image.format == PixelFormat::Rgba8;
        let slot = srgb as usize;
        let mut guard = self.lock();
        let inner = &mut *guard;
        let frame = inner.frame;
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
                inner.images.remove(&index);
            }
        }
        if let Some(cached) = inner.images.get_mut(&index) {
            cached.last_used = frame;
            if let Some(view) = &cached.views[slot] {
                return Some(view.clone());
            }
            if image.is_released()
                && let Some(other) = cached.views[1 - slot].as_ref().filter(|o| o.aliasable)
            {
                // Only the other color space is on the GPU: view the same texture in this one.
                inner.next_id += 1;
                let view = Arc::new(alias_view(other, srgb, inner.next_id));
                cached.views[slot] = Some(view.clone());
                return Some(view);
            }
        }
        if inner.failed.get(&(index, srgb)) == Some(&version) {
            return None;
        }
        if image.is_released() {
            tracing::warn!(
                "image {index} ({image:?}) has no pixels: they were released after upload \
                 (RenderSettings::release_image_pixels) and the GPU has no copy of it {}; set \
                 Image::keep_pixels on images that are read or uploaded outside \
                 GpuShared::image_texture",
                if srgb { "as sRGB" } else { "as linear data" }
            );
            inner.failed.insert((index, srgb), version);
            return None;
        }
        // Textures that may lose their CPU pixels also accept a view in the other color space
        // (not on GL, which can't reinterpret formats).
        let aliasable = inner.release
            && !image.keep_pixels
            && image.format == PixelFormat::Rgba8
            && gpu.adapter_info.backend != wgpu::Backend::Gl;
        let Some((texture, view)) = upload_image(gpu, image, srgb, aliasable) else {
            inner.failed.insert((index, srgb), version);
            return None;
        };
        inner.failed.remove(&(index, srgb));
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
            views: [None, None],
        });
        cached.views[slot] = Some(uploaded.clone());
        Some(uploaded)
    }

    /// Start frame `frame`; `release` is `RenderSettings::release_image_pixels`.
    pub(crate) fn begin_frame(&self, frame: u64, release: bool) {
        let mut inner = self.lock();
        inner.frame = frame;
        inner.release = release;
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
        if frame.is_multiple_of(120) {
            inner.images.retain(|&index, cached| {
                if cached.released {
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

/// A view of `other`'s texture in the other color space (the texture was created with both).
fn alias_view(other: &ImageTexture, srgb: bool, id: u64) -> ImageTexture {
    let format = if srgb {
        wgpu::TextureFormat::Rgba8UnormSrgb
    } else {
        wgpu::TextureFormat::Rgba8Unorm
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
    let mips: &[Vec<u8>] = if !image.mipmaps {
        &[]
    } else if !image.mips.is_empty() && (image.format != PixelFormat::Rgba8 || srgb == image.srgb) {
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
    let max = gpu.device.limits().max_texture_dimension_2d;
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
        mip_level_count: levels.len() as u32,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format,
        usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
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
    let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
    Some((texture, view))
}
